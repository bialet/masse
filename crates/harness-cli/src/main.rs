//! `masse` — an AI coding harness with a TUI and an HTTP/WebSocket server,
//! both sharing one persisted `session.csv`.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("masse only supports Linux and macOS");

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use harness_core::{
    build_system_prompt, format_cost, format_tokens, Agent, AgentConfig, AgentEvent, AgentHandle,
    MockProvider, OpenAiProvider, ProjectContext, Provider,
};
use harness_server::AppState;
use harness_session::SessionStore;
use harness_tools::skill_tools::SkillTool;
use harness_tools::{builtin_registry, ToolContext};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

mod config;
use config::ConfigAction;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";
const DEFAULT_MODEL: &str = "deepseek-chat";

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

    /// Model id passed to the provider. Defaults to the config file, then
    /// `deepseek-chat`.
    #[arg(long, env = "MASSE_MODEL")]
    model: Option<String>,

    /// OpenAI-compatible base URL (no trailing /chat/completions needed).
    /// Defaults to the config file, then `https://api.deepseek.com/v1`.
    #[arg(long, env = "MASSE_BASE_URL")]
    base_url: Option<String>,

    /// Named provider profile from the config file (its `base_url`, `model` and
    /// `api_key`). Overrides the file's top-level `provider` key.
    #[arg(long, env = "MASSE_PROVIDER")]
    provider: Option<String>,

    /// API key. When absent (and --mock is not set) masse runs the mock provider.
    #[arg(long, env = "MASSE_API_KEY")]
    api_key: Option<String>,

    /// Read the API key from a file instead of passing it on the command line,
    /// which is visible to other users via `ps`. Suits containers and systemd
    /// (for example a mounted secret). Takes precedence over --api-key.
    #[arg(long, env = "MASSE_API_KEY_FILE", value_name = "PATH")]
    api_key_file: Option<PathBuf>,

    /// Configuration file. Defaults to <user config dir>/masse/config.
    #[arg(long, env = config::CONFIG_FILE_ENV, value_name = "PATH")]
    config_file: Option<PathBuf>,

    /// Force the offline mock provider.
    #[arg(long)]
    mock: bool,

    #[command(subcommand)]
    command: Option<Command>,

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
    #[arg(long, default_value_t = 120)]
    max_iterations: usize,

    /// Disable the automatic git commit after every turn.
    #[arg(long)]
    no_auto_commit: bool,

    /// After each auto-commit, `git push origin HEAD` (the remote's hook
    /// deploys). Ignored when --no-auto-commit is set.
    #[arg(long, env = "MASSE_AUTO_PUSH")]
    auto_push: bool,

    /// Show the full log (tool calls, results, retries, intermediate messages).
    /// This is the default; the flag is accepted so existing invocations keep
    /// working. Use `--user-mode` to collapse the output.
    #[arg(long, conflicts_with = "user_mode")]
    debug: bool,

    /// Collapse each turn to the question the model asks and its final summary,
    /// hiding tool calls, results, retries and intermediate narration.
    #[arg(long, env = "MASSE_USER_MODE")]
    user_mode: bool,

    /// Sampling temperature.
    #[arg(long, default_value_t = 0.2)]
    temperature: f32,

    /// Retries per model call for transient provider errors (0 disables).
    #[arg(long, default_value_t = 3)]
    max_retries: usize,

    /// Base delay for the retry backoff, in milliseconds.
    #[arg(long, default_value_t = 800)]
    retry_delay_ms: u64,

    /// Price for prompt (input) tokens in USD per million, overriding the
    /// built-in table. Pair it with --price-output for a fully custom model.
    #[arg(long, env = "MASSE_PRICE_INPUT")]
    price_input: Option<f64>,

    /// Price for completion (output) tokens in USD per million, overriding the
    /// built-in table.
    #[arg(long, env = "MASSE_PRICE_OUTPUT")]
    price_output: Option<f64>,

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

#[derive(Debug, Subcommand)]
enum Command {
    /// Read or write the persistent user configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// Provider settings after layering the config file, the environment and the
/// command line (lowest to highest precedence).
struct Resolved {
    base_url: String,
    model: String,
    api_key: Option<String>,
    /// Where the key came from, for `--print-config`.
    api_key_source: String,
    /// The profile in effect, if any.
    profile: Option<String>,
    config_path: Option<PathBuf>,
    config_loaded: bool,
}

fn from_flag(matches: &clap::ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

fn from_env(matches: &clap::ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::EnvVariable)
}

/// Resolve the provider settings. A flag beats an env var, an env var beats the
/// config file, and within one layer a key *file* beats a bare key value (the
/// file is the deployment-safe choice).
fn resolve(cli: &Cli, matches: &clap::ArgMatches) -> Result<Resolved> {
    let config_path = config::path(cli.config_file.as_deref());
    let file = match &config_path {
        Some(p) => config::load(p)?,
        None => config::Config::default(),
    };

    // An explicit --provider/MASSE_PROVIDER wins over the file's `provider` key.
    let profile = cli.provider.clone().or_else(|| file.provider.clone());
    let selected = file.resolve(profile.as_deref());

    let base_url = cli
        .base_url
        .clone()
        .or(selected.base_url)
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = cli
        .model
        .clone()
        .or(selected.model)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let (api_key, api_key_source) = if from_flag(matches, "api_key_file") {
        let p = cli.api_key_file.clone().unwrap_or_default();
        (
            Some(read_key_file(&p)?),
            format!("--api-key-file {}", p.display()),
        )
    } else if from_flag(matches, "api_key") {
        (cli.api_key.clone(), "--api-key".to_string())
    } else if from_env(matches, "api_key_file") {
        let p = cli.api_key_file.clone().unwrap_or_default();
        (
            Some(read_key_file(&p)?),
            format!("MASSE_API_KEY_FILE {}", p.display()),
        )
    } else if from_env(matches, "api_key") {
        (cli.api_key.clone(), "MASSE_API_KEY".to_string())
    } else if selected.api_key.is_some() {
        let where_ = match &profile {
            Some(p) => format!("config file [{p}]"),
            None => "config file".to_string(),
        };
        (selected.api_key, where_)
    } else {
        (None, "none".to_string())
    };

    if let Some(key) = &api_key {
        if key.trim().is_empty() {
            bail!("API key is empty (use --mock to run offline)");
        }
    }

    Ok(Resolved {
        base_url,
        model,
        api_key,
        api_key_source,
        profile,
        config_path,
        config_loaded: file.loaded,
    })
}

fn read_key_file(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read API key file {}", path.display()))?;
    let key = raw.trim();
    if key.is_empty() {
        bail!("API key file {} is empty", path.display());
    }
    Ok(key.to_string())
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
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    // `masse config …` is workspace-independent: handle it before touching the
    // project so it works from anywhere (including before the first run).
    if let Some(Command::Config { action }) = &cli.command {
        return config::run(action, cli.config_file.clone()).await;
    }

    let resolved = resolve(&cli, &matches)?;

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
    let provider: Arc<dyn Provider> = if cli.mock || resolved.api_key.is_none() {
        if !cli.mock {
            eprintln!(
                "[masse] no API key set; using the offline mock provider \
                 (set one with `masse config set-api-key`, --api-key, --api-key-file or MASSE_API_KEY)"
            );
        }
        Arc::new(MockProvider::new())
    } else {
        Arc::new(OpenAiProvider::with_timeout(
            resolved.base_url.clone(),
            resolved.api_key.clone().unwrap_or_default(),
            resolved.model.clone(),
            Duration::from_secs(cli.request_timeout),
        ))
    };

    let mut system_prompt = build_system_prompt(&project, cli.show_full_log());
    if let Some(path) = &cli.prompt_file {
        let prompt_path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        let extra = std::fs::read_to_string(&prompt_path)
            .with_context(|| format!("cannot read prompt file {}", prompt_path.display()))?;
        system_prompt.push_str(&format!(
            "\n\n# Project-supplied initial prompt ({})\n\n",
            prompt_path.display()
        ));
        system_prompt.push_str(extra.trim());
        system_prompt.push('\n');
    }
    let config = AgentConfig {
        model: resolved.model.clone(),
        temperature: cli.temperature,
        max_iterations: cli.max_iterations,
        max_retries: cli.max_retries,
        retry_delay_ms: cli.retry_delay_ms,
        auto_commit: !cli.no_auto_commit,
        auto_push: cli.auto_push && !cli.no_auto_commit,
        system_prompt,
        price_input: cli.price_input,
        price_output: cli.price_output,
    };

    let session = Arc::new(SessionStore::new(session_path.clone()));
    let history = session.read_all().await?;
    let registry = Arc::new(registry);

    if cli.print_config {
        print_config(
            &cli,
            &resolved,
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
            resolved.model,
            tool_names.len(),
            session_path.display(),
            runtime_log.display()
        );
        let tui_config = harness_tui::TuiConfig {
            provider_name: provider.name().to_string(),
            model: resolved.model.clone(),
            session_path: session_path.display().to_string(),
            tool_count: tool_names.len(),
            history,
            root: root.display().to_string(),
            debug: cli.show_full_log(),
        };
        harness_tui::run(tui_config, handle.clone()).await
    } else {
        let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
            .parse()
            .with_context(|| format!("invalid bind address {}:{}", cli.host, cli.port))?;
        eprintln!(
            "[masse] server · provider={} model={} tools={} session={} log={}",
            provider.name(),
            resolved.model,
            tool_names.len(),
            session_path.display(),
            runtime_log.display()
        );
        let state = AppState {
            handle: handle.clone(),
            session: session.clone(),
            tools: registry.clone(),
            provider_name: provider.name().to_string(),
            model: resolved.model.clone(),
            root: root.display().to_string(),
            log_path: Some(runtime_log.clone()),
            debug: cli.show_full_log(),
            turn: Arc::new(std::sync::Mutex::new(harness_server::TurnState::default())),
        };
        harness_server::serve(state, addr).await
    };

    // Kill any background process we started, then stop the actor cleanly.
    processes.stop_all().await;
    handle.shutdown();
    result
}

impl Cli {
    /// The full log is shown unless `--user-mode` was requested. `--debug` is
    /// the default and is accepted so older invocations keep working.
    fn show_full_log(&self) -> bool {
        self.debug || !self.user_mode
    }
}

fn print_config(
    cli: &Cli,
    resolved: &Resolved,
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
        if cli.mock || resolved.api_key.is_none() {
            "mock (no API key)".to_string()
        } else {
            let name = resolved.profile.as_deref().unwrap_or("default");
            format!("openai-compatible [profile {name}]")
        }
    );
    match &resolved.config_path {
        Some(p) => println!(
            "config file    : {} ({})",
            p.display(),
            if resolved.config_loaded {
                "loaded"
            } else {
                "absent"
            }
        ),
        None => println!("config file    : (no config directory)"),
    }
    println!("base url       : {}", resolved.base_url);
    println!("model          : {}", resolved.model);
    match &resolved.api_key {
        Some(k) => println!(
            "api key        : {} (from {})",
            config::mask(k),
            resolved.api_key_source
        ),
        None => println!("api key        : (none)"),
    }
    println!("request timeout: {}s", cli.request_timeout);
    println!(
        "retries        : {} (base {}ms)",
        cli.max_retries, cli.retry_delay_ms
    );
    match (cli.price_input, cli.price_output) {
        (None, None) => println!("pricing        : built-in table (USD / 1M tokens)"),
        (input, output) => println!(
            "pricing        : override in={} out={} (USD / 1M tokens)",
            input
                .map(|v| format!("${v}"))
                .unwrap_or_else(|| "table".into()),
            output
                .map(|v| format!("${v}"))
                .unwrap_or_else(|| "table".into()),
        ),
    }
    println!(
        "auto-commit    : {}",
        if cli.no_auto_commit {
            "off"
        } else {
            "on (git init if needed)"
        }
    );
    println!(
        "auto-push      : {}",
        if cli.auto_push && !cli.no_auto_commit {
            "on (git push origin HEAD)"
        } else {
            "off"
        }
    );
    println!(
        "mode           : {}",
        if cli.show_full_log() {
            "full log (tool calls, results, retries)"
        } else {
            "user (questions and final summary only)"
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
        AgentEvent::Usage(report) => {
            let cost = match (report.turn_cost_usd, report.total_cost_usd) {
                (Some(turn), Some(total)) => {
                    format!(" turn={} total={}", format_cost(turn), format_cost(total))
                }
                _ => String::new(),
            };
            let ctx = match report.context_percent() {
                Some(pct) => format!(
                    " ctx={}/{} ({pct:.0}%)",
                    format_tokens(report.context_tokens),
                    format_tokens(report.context_window.unwrap_or(0))
                ),
                None => String::new(),
            };
            format!(
                "{ts} USAGE      turn_tokens={} total_tokens={}{cost}{ctx}\n",
                report.turn.normalized_total(),
                report.total.normalized_total(),
            )
        }
        AgentEvent::Finished { summary, success } => {
            format!("{ts} FINISHED   success={success}: {summary}\n")
        }
        AgentEvent::Error(message) => format!("{ts} ERROR      {message}\n"),
        AgentEvent::TurnFinished => format!("{ts} TURN_FINISHED\n"),
    }
}
