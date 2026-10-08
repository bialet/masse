//! `masse` — an AI coding harness with a TUI and an HTTP/WebSocket server,
//! both sharing one persisted `session.csv`.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("masse only supports Linux and macOS");

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use harness_core::{
    build_system_prompt, Agent, AgentConfig, AgentEvent, AgentHandle, MockProvider, OpenAiProvider,
    ProjectContext, Provider,
};
use harness_server::AppState;
use harness_session::SessionStore;
use harness_tools::skill_tools::SkillTool;
use harness_tools::{builtin_registry, ToolContext};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

#[derive(Debug, Parser)]
#[command(
    name = "masse",
    version,
    about = "An AI coding harness (TUI + server) that shares one session.csv",
    long_about = None,
)]
struct Cli {
    /// Project working directory. All tool paths are confined to it.
    #[arg(long, short = 'C', default_value = ".")]
    workspace: PathBuf,

    /// Session file. Defaults to <workspace>/session.csv.
    #[arg(long)]
    session: Option<PathBuf>,

    /// Run the terminal UI (default).
    #[arg(long, conflicts_with = "serve")]
    tui: bool,

    /// Run the HTTP/WebSocket server instead of the terminal UI.
    #[arg(long, conflicts_with = "tui")]
    serve: bool,

    /// Address to bind in serve mode.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind in serve mode.
    #[arg(long, default_value_t = 7001)]
    port: u16,

    /// Model id passed to the provider.
    #[arg(long, env = "MASSE_MODEL", default_value = "deepseek-chat")]
    model: String,

    /// OpenAI-compatible base URL (no trailing /chat/completions needed).
    #[arg(
        long,
        env = "MASSE_BASE_URL",
        default_value = "https://api.deepseek.com/v1"
    )]
    base_url: String,

    /// API key. When absent (and --mock is not set) masse runs the mock provider.
    #[arg(long, env = "MASSE_API_KEY")]
    api_key: Option<String>,

    /// Force the offline mock provider.
    #[arg(long)]
    mock: bool,

    /// Bialet binary to invoke.
    #[arg(long, env = "MASSE_BIALET_BIN", default_value = "bialet")]
    bialet_bin: String,

    /// Directory with Bialet/Wren documentation for the bialet_docs tool.
    #[arg(long, env = "MASSE_BIALET_DOCS")]
    bialet_docs: Option<PathBuf>,

    /// Default timeout for external commands, in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Maximum tool iterations per user turn (safety cap).
    #[arg(long, default_value_t = 50)]
    max_iterations: usize,

    /// Disable the automatic git commit after every turn.
    #[arg(long)]
    no_auto_commit: bool,

    /// Sampling temperature.
    #[arg(long, default_value_t = 0.2)]
    temperature: f32,

    /// Retries per model call for transient provider errors (0 disables).
    #[arg(long, default_value_t = 3)]
    max_retries: usize,

    /// Base delay for the retry backoff, in milliseconds.
    #[arg(long, default_value_t = 800)]
    retry_delay_ms: u64,

    /// Per-request provider timeout, in seconds. Prevents a hung endpoint from
    /// freezing a turn.
    #[arg(long, default_value_t = 120)]
    request_timeout: u64,

    /// Extra initial prompt file appended to the system prompt (for example a
    /// framework's AI prompt such as Bialet's docs/ai-prompt.md). Relative paths
    /// resolve against the workspace.
    #[arg(long, env = "MASSE_PROMPT_FILE")]
    prompt_file: Option<PathBuf>,

    /// Print the resolved configuration and exit without starting a front end.
    #[arg(long)]
    print_config: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("masse: error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    let root = std::fs::canonicalize(&cli.workspace)
        .with_context(|| format!("cannot resolve workspace {}", cli.workspace.display()))?;
    if !root.is_dir() {
        bail!("workspace is not a directory: {}", root.display());
    }

    let session_path = cli
        .session
        .clone()
        .map(|p| if p.is_absolute() { p } else { root.join(p) })
        .unwrap_or_else(|| root.join(".masse").join("session.csv"));

    // Project context: AGENTS.md, skills, docs.
    let project = ProjectContext::load(&root, cli.bialet_docs.clone()).await;

    // Keep runtime state out of the user's Git history.
    if let Err(e) = harness_core::context::ensure_workspace_gitignore(&root).await {
        eprintln!("[masse] warning: could not update .gitignore: {e}");
    }

    // Human-readable runtime log: this is where errors and retries are written,
    // so there is always a place to see what the agent ran into.
    let runtime_log = root.join(".masse").join("masse.log");

    // Tools: built-ins plus one tool per discovered skill.
    let mut registry = builtin_registry();
    for skill in &project.skills {
        registry.register(Arc::new(SkillTool::new(skill.clone())));
    }
    let tool_names = registry.names();

    let mut ctx = ToolContext::new(&root);
    ctx.default_timeout = Duration::from_secs(cli.timeout);
    ctx.bialet_bin = cli.bialet_bin.clone();
    ctx.docs_dir = project.docs_dir.clone();
    let processes = ctx.processes.clone();

    // Provider.
    let provider: Arc<dyn Provider> = if cli.mock || cli.api_key.is_none() {
        if !cli.mock {
            eprintln!(
                "[masse] no API key set; using the offline mock provider (pass --api-key or MASSE_API_KEY)"
            );
        }
        Arc::new(MockProvider::new())
    } else {
        Arc::new(OpenAiProvider::with_timeout(
            cli.base_url.clone(),
            cli.api_key.clone().unwrap_or_default(),
            cli.model.clone(),
            Duration::from_secs(cli.request_timeout),
        ))
    };

    let mut system_prompt = build_system_prompt(&project);
    if let Some(path) = &cli.prompt_file {
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        let extra = std::fs::read_to_string(&resolved)
            .with_context(|| format!("cannot read prompt file {}", resolved.display()))?;
        system_prompt.push_str(&format!(
            "\n\n# Project-supplied initial prompt ({})\n\n",
            resolved.display()
        ));
        system_prompt.push_str(extra.trim());
        system_prompt.push('\n');
    }
    let config = AgentConfig {
        model: cli.model.clone(),
        temperature: cli.temperature,
        max_iterations: cli.max_iterations,
        max_retries: cli.max_retries,
        retry_delay_ms: cli.retry_delay_ms,
        auto_commit: !cli.no_auto_commit,
        system_prompt,
    };

    let session = Arc::new(SessionStore::new(session_path.clone()));
    let history = session.read_all().await?;
    let registry = Arc::new(registry);

    if cli.print_config {
        print_config(
            &cli,
            &root,
            &session_path,
            &runtime_log,
            &project,
            &tool_names,
        );
        return Ok(());
    }

    let agent = Agent::new(
        provider.clone(),
        registry.clone(),
        ctx,
        session.clone(),
        config,
    )
    .await
    .context("initializing agent")?;
    let handle = AgentHandle::spawn(agent);

    // Subscribe a durable logger to every event, independent of the front end.
    spawn_event_logger(handle.subscribe(), runtime_log.clone());

    let result = if !cli.serve {
        eprintln!(
            "[masse] TUI · provider={} model={} tools={} session={} log={}",
            provider.name(),
            cli.model,
            tool_names.len(),
            session_path.display(),
            runtime_log.display()
        );
        let tui_config = harness_tui::TuiConfig {
            provider_name: provider.name().to_string(),
            model: cli.model.clone(),
            session_path: session_path.display().to_string(),
            tool_count: tool_names.len(),
            history,
            root: root.display().to_string(),
        };
        harness_tui::run(tui_config, handle.clone()).await
    } else {
        let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
            .parse()
            .with_context(|| format!("invalid bind address {}:{}", cli.host, cli.port))?;
        eprintln!(
            "[masse] server · provider={} model={} tools={} session={} log={}",
            provider.name(),
            cli.model,
            tool_names.len(),
            session_path.display(),
            runtime_log.display()
        );
        let state = AppState {
            handle: handle.clone(),
            session: session.clone(),
            tools: registry.clone(),
            provider_name: provider.name().to_string(),
            model: cli.model.clone(),
            root: root.display().to_string(),
            log_path: Some(runtime_log.clone()),
        };
        harness_server::serve(state, addr).await
    };

    // Kill any background process we started, then stop the actor cleanly.
    processes.stop_all().await;
    handle.shutdown();
    result
}

fn print_config(
    cli: &Cli,
    root: &std::path::Path,
    session_path: &std::path::Path,
    runtime_log: &std::path::Path,
    project: &ProjectContext,
    tool_names: &[String],
) {
    println!("workspace      : {}", root.display());
    println!("session        : {}", session_path.display());
    println!("runtime log    : {}", runtime_log.display());
    let front = if cli.serve { "serve" } else { "tui" };
    println!("front end      : {front}");
    if cli.serve {
        println!("bind           : {}:{}", cli.host, cli.port);
    }
    println!(
        "provider       : {}",
        if cli.mock || cli.api_key.is_none() {
            "mock"
        } else {
            "openai-compatible"
        }
    );
    println!("base url       : {}", cli.base_url);
    println!("model          : {}", cli.model);
    println!("request timeout: {}s", cli.request_timeout);
    println!(
        "retries        : {} (base {}ms)",
        cli.max_retries, cli.retry_delay_ms
    );
    println!(
        "auto-commit    : {}",
        if cli.no_auto_commit {
            "off"
        } else {
            "on (git init if needed)"
        }
    );
    match &cli.prompt_file {
        Some(p) => println!("prompt file    : {}", p.display()),
        None => println!("prompt file    : (none)"),
    }
    println!("bialet binary  : {}", cli.bialet_bin);
    match &project.docs_dir {
        Some(d) => println!("bialet docs    : {}", d.display()),
        None => println!("bialet docs    : (not found)"),
    }
    println!(
        "AGENTS.md      : {}",
        if project.agents_md.is_some() {
            "loaded"
        } else {
            "absent"
        }
    );
    println!("skills         : {:?}", project.skill_names());
    match &project.framework_prompt {
        Some(fp) => println!("framework guide: {} (live)", fp.source.display()),
        None => println!("framework guide: bundled with masse"),
    }
    println!("tools ({}):", tool_names.len());
    for name in tool_names {
        println!("  - {name}");
    }
}

/// Append every agent event to a human-readable runtime log, independently of
/// the front end. This is where provider errors and retries are recorded, so
/// there is always somewhere to look when a turn fails.
fn spawn_event_logger(mut rx: broadcast::Receiver<AgentEvent>, path: PathBuf) {
    tokio::spawn(async move {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let line = format_event(&event);
                    if append_line(&path, &line).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn append_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await
}

fn format_event(event: &AgentEvent) -> String {
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    match event {
        AgentEvent::UserMessage(text) => format!("{ts} USER       {text}\n"),
        AgentEvent::AssistantMessage(text) => format!("{ts} ASSISTANT  {text}\n"),
        AgentEvent::ToolCall { name, args } => format!("{ts} TOOL_CALL  {name} {args}\n"),
        AgentEvent::ToolResult {
            name,
            status,
            output,
            exit_code,
        } => format!(
            "{ts} TOOL_RESULT [{status}] {name} exit={}\n{output}\n",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into())
        ),
        AgentEvent::Retrying {
            attempt,
            max,
            reason,
        } => format!("{ts} RETRY      {attempt}/{max}: {reason}\n"),
        AgentEvent::Notice(message) => format!("{ts} NOTICE     {message}\n"),
        AgentEvent::Finished { summary, success } => {
            format!("{ts} FINISHED   success={success}: {summary}\n")
        }
        AgentEvent::Error(message) => format!("{ts} ERROR      {message}\n"),
        AgentEvent::TurnFinished => format!("{ts} TURN_FINISHED\n"),
    }
}
