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
    build_system_prompt, Agent, AgentConfig, AgentHandle, MockProvider, OpenAiProvider,
    ProjectContext, Provider,
};
use harness_server::AppState;
use harness_session::SessionStore;
use harness_tools::skill_tools::SkillTool;
use harness_tools::{builtin_registry, ToolContext};

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

    /// Maximum tool iterations per user turn.
    #[arg(long, default_value_t = 24)]
    max_iterations: usize,

    /// Sampling temperature.
    #[arg(long, default_value_t = 0.2)]
    temperature: f32,

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
        .unwrap_or_else(|| root.join("session.csv"));

    // Project context: AGENTS.md, skills, docs.
    let project = ProjectContext::load(&root, cli.bialet_docs.clone()).await;

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
        Arc::new(OpenAiProvider::new(
            cli.base_url.clone(),
            cli.api_key.clone().unwrap_or_default(),
            cli.model.clone(),
        ))
    };

    let system_prompt = build_system_prompt(&project);
    let config = AgentConfig {
        model: cli.model.clone(),
        temperature: cli.temperature,
        max_iterations: cli.max_iterations,
        system_prompt,
    };

    let session = Arc::new(SessionStore::new(session_path.clone()));
    let history = session.read_all().await?;
    let registry = Arc::new(registry);

    if cli.print_config {
        print_config(&cli, &root, &session_path, &project, &tool_names);
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

    let result = if !cli.serve {
        eprintln!(
            "[masse] TUI · provider={} model={} tools={} session={}",
            provider.name(),
            cli.model,
            tool_names.len(),
            session_path.display()
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
        let state = AppState {
            handle: handle.clone(),
            session: session.clone(),
            tools: registry.clone(),
            provider_name: provider.name().to_string(),
            model: cli.model.clone(),
            root: root.display().to_string(),
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
    project: &ProjectContext,
    tool_names: &[String],
) {
    println!("workspace      : {}", root.display());
    println!("session        : {}", session_path.display());
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
    println!("tools ({}):", tool_names.len());
    for name in tool_names {
        println!("  - {name}");
    }
}
