//! The agent runtime: the tool-calling loop and the actor that owns it.

use std::sync::Arc;

use anyhow::Result;
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
    pub system_prompt: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            model: String::new(),
            temperature: 0.2,
            max_iterations: 24,
            system_prompt: String::new(),
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
    Error(String),
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

    /// Run one user turn to completion. Returns the final assistant text.
    pub async fn send(
        &mut self,
        text: &str,
        sink: Option<&broadcast::Sender<AgentEvent>>,
    ) -> Result<String> {
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
            let response = match self.provider.complete(request).await {
                Ok(r) => r,
                Err(e) => {
                    emit!(AgentEvent::Error(e.to_string()));
                    return Err(e);
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
                emit!(AgentEvent::TurnFinished);
                return Ok(response.content);
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
            }
        }

        let message = format!(
            "agent exceeded {} tool iterations without finishing",
            self.config.max_iterations
        );
        emit!(AgentEvent::Error(message.clone()));
        Err(anyhow::anyhow!(message))
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
                        let _ = agent.send(&text, Some(&emitter)).await;
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

    /// Subscribe to the event stream. Each subscriber sees every future event.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}
