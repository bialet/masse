//! The agent runtime: the tool-calling loop and the actor that owns it.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use harness_session::{encode_json_args, Event, SessionStore, Status as SessionStatus};
use harness_tools::cli::{run as run_cmd, CmdSpec};
use harness_tools::{ToolContext, ToolRegistry, ToolStatus};
use tokio::sync::{broadcast, mpsc};

use crate::message::ChatMessage;
use crate::provider::{ChatRequest, Provider};

/// Tunables for a single agent.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_iterations: usize,
    /// How many times a transient provider error is retried per model call.
    pub max_retries: usize,
    /// Base delay for the retry backoff, in milliseconds.
    pub retry_delay_ms: u64,
    /// Commit the workspace after every turn (and `git init` if needed).
    pub auto_commit: bool,
    /// After committing, `git push origin HEAD` so the remote deploys. Requires
    /// `auto_commit`.
    pub auto_push: bool,
    pub system_prompt: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            model: String::new(),
            temperature: 0.2,
            max_iterations: 50,
            max_retries: 3,
            retry_delay_ms: 800,
            auto_commit: false,
            auto_push: false,
            system_prompt: String::new(),
        }
    }
}

/// Exponential backoff, capped so a turn never stalls for long.
fn retry_delay(base_ms: u64, attempt: u32) -> u64 {
    let factor = 1u64 << attempt.saturating_sub(1).min(6);
    base_ms.saturating_mul(factor).min(15_000)
}

/// First line of `text`, trimmed and truncated to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        line.chars().take(max).collect()
    }
}

/// How a turn ended.
enum TurnOutcome {
    /// The model stopped calling tools on its own.
    Stopped(String),
    /// The `finish` tool was called.
    Finished { summary: String, success: bool },
}

impl TurnOutcome {
    fn text(&self) -> &str {
        match self {
            TurnOutcome::Stopped(text) => text,
            TurnOutcome::Finished { summary, .. } => summary,
        }
    }
}

/// Streamed progress emitted while a turn runs.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    UserMessage(String),
    AssistantMessage(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        status: String,
        output: String,
        exit_code: Option<i32>,
    },
    /// A transient provider error is being retried.
    Retrying {
        attempt: u32,
        max: u32,
        reason: String,
    },
    /// Informational note (e.g. the iteration cap was reached).
    Notice(String),
    /// The agent declared the task finished via the `finish` tool.
    Finished {
        summary: String,
        success: bool,
    },
    Error(String),
    /// The model stopped on its own without calling `finish`.
    TurnFinished,
}

/// Owns the conversation state and drives the provider/tool loop.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    ctx: ToolContext,
    session: Arc<SessionStore>,
    config: AgentConfig,
    messages: Vec<ChatMessage>,
    /// Whether the git repo was prepared for auto-commit: `None` = not tried,
    /// `Some(true)` = ready, `Some(false)` = setup failed.
    repo_ready: Option<bool>,
}

impl Agent {
    /// Build an agent and rehydrate the conversation from the session file.
    pub async fn new(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        ctx: ToolContext,
        session: Arc<SessionStore>,
        config: AgentConfig,
    ) -> Result<Self> {
        let events = session.read_all().await?;
        if events.is_empty() {
            session.append(&Event::session_start()).await?;
        }
        let mut messages = rebuild_messages(&events, &config.system_prompt);
        if messages.is_empty() {
            messages.push(ChatMessage::system(config.system_prompt.clone()));
        }
        Ok(Agent {
            provider,
            tools,
            ctx,
            session,
            config,
            messages,
            repo_ready: None,
        })
    }

    /// Run a git command in the workspace root.
    async fn git(&self, args: &[&str]) -> harness_tools::cli::CmdOutput {
        let spec = CmdSpec::new("git").args(args.iter().map(|a| a.to_string()));
        run_cmd(&spec, &self.ctx.root, Duration::from_secs(30)).await
    }

    /// Make sure the workspace is a git repo able to commit: `git init` when
    /// there is no repository, and a local identity when none is configured.
    pub async fn ensure_git_repo(&self) -> Result<()> {
        // Use the enclosing repository if there is one; otherwise make the
        // workspace its own repo.
        let inside = self.git(&["rev-parse", "--is-inside-work-tree"]).await;
        if !inside.success() || inside.stdout.trim() != "true" {
            let out = self.git(&["init", "-q"]).await;
            if !out.success() {
                return Err(anyhow::anyhow!(
                    "git init failed in {}: {}",
                    self.ctx.root.display(),
                    out.to_agent_text()
                ));
            }
        }
        let email = self.git(&["config", "user.email"]).await;
        if email.stdout.trim().is_empty() {
            let _ = self.git(&["config", "user.email", "masse@localhost"]).await;
            let _ = self.git(&["config", "user.name", "masse"]).await;
        }
        Ok(())
    }

    /// Commit the workspace after a turn. Returns a short report when something
    /// was committed, or `None` when the tree was already clean.
    pub async fn commit_turn(&self, label: &str) -> Result<Option<String>> {
        let status = self.git(&["status", "--porcelain"]).await;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "git status failed: {}",
                status.to_agent_text()
            ));
        }
        if status.stdout.trim().is_empty() {
            return Ok(None);
        }
        let add = self.git(&["add", "-A"]).await;
        if !add.success() {
            return Err(anyhow::anyhow!("git add failed: {}", add.to_agent_text()));
        }
        let message = format!("masse: {}", first_line(label, 72));
        let commit = self.git(&["commit", "-q", "-m", &message]).await;
        if !commit.success() {
            return Err(anyhow::anyhow!(
                "git commit failed: {}",
                commit.to_agent_text()
            ));
        }
        let hash = self.git(&["rev-parse", "--short", "HEAD"]).await;
        let short = hash.stdout.trim().to_string();
        Ok(Some(format!("{short} {message}")))
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn session(&self) -> &Arc<SessionStore> {
        &self.session
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Run one user turn to completion.
    ///
    /// Exactly one terminal event (`Finished`, `TurnFinished` or `Error`) is
    /// emitted on every path, so a front end can never get stuck waiting.
    pub async fn send(
        &mut self,
        text: &str,
        sink: Option<&broadcast::Sender<AgentEvent>>,
    ) -> Result<String> {
        // Prepare the repo once, so auto-commit can work.
        if self.config.auto_commit && self.repo_ready.is_none() {
            match self.ensure_git_repo().await {
                Ok(()) => self.repo_ready = Some(true),
                Err(e) => {
                    self.repo_ready = Some(false);
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("auto-commit disabled: {e}")));
                    }
                }
            }
        }

        let result = self.run_turn(text, sink).await;
        if let Some(s) = sink {
            match &result {
                Ok(TurnOutcome::Stopped(_)) => {
                    let _ = s.send(AgentEvent::TurnFinished);
                }
                Ok(TurnOutcome::Finished { summary, success }) => {
                    let _ = s.send(AgentEvent::Finished {
                        summary: summary.clone(),
                        success: *success,
                    });
                }
                Err(e) => {
                    let _ = s.send(AgentEvent::Error(e.to_string()));
                }
            }
        }

        // Commit whatever the turn changed.
        if self.config.auto_commit && self.repo_ready == Some(true) {
            match self.commit_turn(text).await {
                Ok(Some(report)) => {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("committed {report}")));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("auto-commit failed: {e}")));
                    }
                }
            }

            // Push so the deploy hook fires. A no-op push is harmless.
            if self.config.auto_push {
                let push = self.git(&["push", "origin", "HEAD"]).await;
                if !push.success() {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!(
                            "auto-push failed: {}",
                            push.to_agent_text()
                        )));
                    }
                }
            }
        }

        result.map(|outcome| outcome.text().to_string())
    }

    async fn run_turn(
        &mut self,
        text: &str,
        sink: Option<&broadcast::Sender<AgentEvent>>,
    ) -> Result<TurnOutcome> {
        macro_rules! emit {
            ($ev:expr) => {
                if let Some(s) = sink {
                    let _ = s.send($ev);
                }
            };
        }

        emit!(AgentEvent::UserMessage(text.to_string()));
        self.session.append(&Event::user_message(text)).await?;
        self.messages.push(ChatMessage::user(text));

        for _ in 0..self.config.max_iterations {
            let request = ChatRequest {
                model: self.config.model.clone(),
                messages: self.messages.clone(),
                tools: self.tools.openai_schemas(),
                temperature: self.config.temperature,
            };
            // Retry only transient errors, a bounded number of times; a
            // permanent error (bad request, auth, malformed response) fails
            // immediately.
            let mut attempt: u32 = 0;
            let response = loop {
                match self.provider.complete(request.clone()).await {
                    Ok(response) => break response,
                    Err(e) => {
                        if e.is_retryable() && attempt < self.config.max_retries as u32 {
                            attempt += 1;
                            let delay = retry_delay(self.config.retry_delay_ms, attempt);
                            emit!(AgentEvent::Retrying {
                                attempt,
                                max: self.config.max_retries as u32,
                                reason: e.to_string(),
                            });
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            continue;
                        }
                        return Err(anyhow::anyhow!(e.to_string()));
                    }
                }
            };

            if !response.content.trim().is_empty() {
                emit!(AgentEvent::AssistantMessage(response.content.clone()));
                self.session
                    .append(&Event::assistant_message(&response.content))
                    .await?;
            }

            let calls = response.tool_calls.clone();
            self.messages.push(ChatMessage::assistant_with_calls(
                response.content.clone(),
                calls.clone(),
            ));

            if calls.is_empty() {
                return Ok(TurnOutcome::Stopped(response.content));
            }

            for call in calls {
                let args_value: serde_json::Value =
                    serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
                let args_encoded = encode_json_args(&args_value);

                emit!(AgentEvent::ToolCall {
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                });
                self.session
                    .append(&Event::tool_call(&call.name, &args_encoded))
                    .await?;

                let result = self.tools.call(&call.name, &self.ctx, args_value).await;
                let terminate = result.terminate.clone();
                let session_status = if result.status == ToolStatus::Ok {
                    SessionStatus::Ok
                } else {
                    SessionStatus::Error
                };
                self.session
                    .append(&Event::tool_result(
                        &call.name,
                        &args_encoded,
                        session_status,
                        &result.output,
                        result.exit_code,
                    ))
                    .await?;

                emit!(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    status: result.status.as_str().to_string(),
                    output: result.output.clone(),
                    exit_code: result.exit_code,
                });
                self.messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    call.name.clone(),
                    result.output,
                ));

                // The `finish` tool ends the turn with an explicit outcome.
                if let Some(t) = terminate {
                    return Ok(TurnOutcome::Finished {
                        summary: t.summary,
                        success: t.success,
                    });
                }
            }
        }

        // We reached the iteration cap. Rather than failing the turn, ask the
        // model once more with tools disabled so it produces a real answer.
        emit!(AgentEvent::Notice(format!(
            "tool-call iteration limit ({}) reached; requesting a final answer without tools",
            self.config.max_iterations
        )));
        let mut wrap_messages = self.messages.clone();
        wrap_messages.push(ChatMessage::user(
            "You have reached the maximum number of tool calls for this turn. \
             Do not call any more tools. Reply with a concise summary of what you \
             did, what remains to be done, and your best final answer.",
        ));
        let wrap_request = ChatRequest {
            model: self.config.model.clone(),
            messages: wrap_messages,
            tools: Vec::new(),
            temperature: self.config.temperature,
        };
        match self.provider.complete(wrap_request).await {
            Ok(response) => {
                if !response.content.trim().is_empty() {
                    emit!(AgentEvent::AssistantMessage(response.content.clone()));
                    self.session
                        .append(&Event::assistant_message(&response.content))
                        .await?;
                    self.messages.push(ChatMessage::assistant(response.content.clone()));
                }
                Ok(TurnOutcome::Stopped(response.content))
            }
            Err(e) => Err(anyhow::anyhow!(
                "agent reached the tool-call iteration limit ({}) and the wrap-up request failed: {e}",
                self.config.max_iterations
            )),
        }
    }
}

/// Reconstruct a provider-ready conversation from session events.
///
/// The CSV does not store OpenAI `tool_call_id`s, so tool results are replayed
/// as user-role notes. This keeps the transcript valid without losing content.
pub fn rebuild_messages(events: &[Event], system_prompt: &str) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(system_prompt)];
    let mut pending_tool: Option<String> = None;
    for ev in events {
        match ev.event_type.as_str() {
            "user_message" => messages.push(ChatMessage::user(ev.output.clone())),
            "assistant_message" => {
                if !ev.output.trim().is_empty() {
                    messages.push(ChatMessage::assistant(ev.output.clone()));
                }
            }
            "tool_call" => pending_tool = Some(ev.tool.clone()),
            "tool_result" => {
                let name = if ev.tool.is_empty() {
                    pending_tool.take().unwrap_or_default()
                } else {
                    ev.tool.clone()
                };
                let note = format!("[tool {name} → {}]\n{}", ev.status, ev.output);
                messages.push(ChatMessage::user(note));
            }
            _ => {}
        }
    }
    messages
}

/// Commands the actor accepts.
enum Command {
    User(String),
    Cancel,
    Shutdown,
}

/// A cheap, cloneable handle to a running [`Agent`].
///
/// The actor pattern keeps the single-threaded conversation invariant while
/// exposing a `Send` handle to the TUI and the HTTP server.
#[derive(Clone)]
pub struct AgentHandle {
    tx: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<AgentEvent>,
}

impl AgentHandle {
    pub fn spawn(mut agent: Agent) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
        let (events, _) = broadcast::channel::<AgentEvent>(2048);
        let emitter = events.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::User(text) => {
                        // Guard against panics so the actor never dies silently
                        // and leaves a front end stuck on "thinking".
                        let fut = std::panic::AssertUnwindSafe(agent.send(&text, Some(&emitter)))
                            .catch_unwind();
                        tokio::pin!(fut);
                        let mut shutdown = false;
                        loop {
                            tokio::select! {
                                outcome = &mut fut => {
                                    if outcome.is_err() {
                                        let _ = emitter.send(AgentEvent::Error(
                                            "internal error: agent task panicked; turn aborted".into(),
                                        ));
                                    }
                                    break;
                                }
                                queued = rx.recv() => {
                                    match queued {
                                        Some(Command::Cancel) => {
                                            // Dropping `fut` cancels the in-flight turn.
                                            let _ = emitter.send(AgentEvent::Error(
                                                "turn cancelled".into(),
                                            ));
                                            break;
                                        }
                                        Some(Command::Shutdown) | None => {
                                            shutdown = true;
                                            break;
                                        }
                                        // Ignore new messages while busy.
                                        Some(Command::User(_)) => {}
                                    }
                                }
                            }
                        }
                        if shutdown {
                            break;
                        }
                    }
                    Command::Cancel => {
                        let _ = emitter.send(AgentEvent::Error("nothing to cancel".into()));
                    }
                    Command::Shutdown => break,
                }
            }
        });
        AgentHandle { tx, events }
    }

    /// Queue a user message. Returns false if the actor has stopped.
    pub fn send_user(&self, text: impl Into<String>) -> bool {
        self.tx.send(Command::User(text.into())).is_ok()
    }

    /// Ask the actor to abort the in-flight turn, if any.
    pub fn cancel(&self) -> bool {
        self.tx.send(Command::Cancel).is_ok()
    }

    /// Subscribe to the event stream. Each subscriber sees every future event.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}
