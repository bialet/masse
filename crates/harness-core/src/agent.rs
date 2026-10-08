//! The agent runtime: the tool-calling loop and the actor that owns it.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use harness_session::{encode_json_args, Event, SessionStore, Status as SessionStatus};
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
            system_prompt: String::new(),
        }
    }
}

/// Exponential backoff, capped so a turn never stalls for long.
fn retry_delay(base_ms: u64, attempt: u32) -> u64 {
    let factor = 1u64 << attempt.saturating_sub(1).min(6);
    base_ms.saturating_mul(factor).min(15_000)
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
        })
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
