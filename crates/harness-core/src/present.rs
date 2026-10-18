//! Presentation filter for user mode.
//!
//! masse has two output modes. `--debug` (the default) shows the full log:
//! every tool call, every result, every retry. User mode hides all of that and
//! keeps only what matters: the user's own messages, the *final* assistant
//! message of each turn — either the question the model is asking or the
//! summary it writes when it is done — errors, and the `finish` outcome. Tool
//! traffic, retries, notices and the model's step-by-step narration are dropped.
//!
//! [`UserModeFilter`] performs that collapse. It is deliberately front-end
//! agnostic so the TUI and the server can share one definition of "relevant".

use harness_session::Event as SessionEvent;

use crate::agent::AgentEvent;

/// Something worth showing to the user in user mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    User(String),
    Assistant(String),
    Error(String),
    Finished { summary: String, success: bool },
}

/// Collapses an event stream into [`Shown`] messages.
#[derive(Default)]
pub struct UserModeFilter {
    /// The latest assistant message of the current turn, held back until the
    /// turn ends so intermediate narration never reaches the screen.
    pending: Option<String>,
}

impl UserModeFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter one live [`AgentEvent`].
    pub fn on_agent(&mut self, event: &AgentEvent) -> Option<Shown> {
        match event {
            AgentEvent::UserMessage(text) => {
                self.pending = None;
                Some(Shown::User(text.clone()))
            }
            AgentEvent::AssistantMessage(text) => {
                if !text.trim().is_empty() {
                    self.pending = Some(text.clone());
                }
                None
            }
            // A `finish` call ends the turn; prefer the explanation the model
            // wrote over the one-line tool summary when both exist.
            AgentEvent::Finished { summary, success } => match self.pending.take() {
                Some(text) => Some(Shown::Assistant(text)),
                None => Some(Shown::Finished {
                    summary: summary.clone(),
                    success: *success,
                }),
            },
            AgentEvent::TurnFinished => self.pending.take().map(Shown::Assistant),
            AgentEvent::Error(message) => {
                self.pending = None;
                Some(Shown::Error(message.clone()))
            }
            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::Retrying { .. }
            | AgentEvent::Notice(_) => None,
        }
    }

    /// Filter one rehydrated [`SessionEvent`].
    ///
    /// History has no turn markers, so a turn ends at the next `user_message`
    /// or at [`Self::flush`] for the final one.
    pub fn on_session(&mut self, event: &SessionEvent) -> Option<Shown> {
        match event.event_type.as_str() {
            "user_message" => {
                self.pending = None;
                Some(Shown::User(event.output.clone()))
            }
            "assistant_message" => {
                if !event.output.trim().is_empty() {
                    self.pending = Some(event.output.clone());
                }
                None
            }
            _ => None,
        }
    }

    /// Emit the assistant message still held back, if any (end of a turn or of
    /// a replayed history).
    pub fn flush(&mut self) -> Option<Shown> {
        self.pending.take().map(Shown::Assistant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(text: &str) -> AgentEvent {
        AgentEvent::AssistantMessage(text.into())
    }

    #[test]
    fn keeps_only_the_last_assistant_message_of_a_turn() {
        let mut f = UserModeFilter::new();
        assert_eq!(
            f.on_agent(&AgentEvent::UserMessage("hi".into())),
            Some(Shown::User("hi".into()))
        );
        assert_eq!(f.on_agent(&assistant("thinking out loud")), None);
        assert_eq!(f.on_agent(&assistant("the real answer")), None);
        assert_eq!(
            f.on_agent(&AgentEvent::TurnFinished),
            Some(Shown::Assistant("the real answer".into()))
        );
    }

    #[test]
    fn hides_tool_traffic_retries_and_notices() {
        let mut f = UserModeFilter::new();
        assert_eq!(
            f.on_agent(&AgentEvent::ToolCall {
                name: "read_file".into(),
                args: "{}".into()
            }),
            None
        );
        assert_eq!(
            f.on_agent(&AgentEvent::ToolResult {
                name: "read_file".into(),
                status: "ok".into(),
                output: "x".into(),
                exit_code: Some(0)
            }),
            None
        );
        assert_eq!(
            f.on_agent(&AgentEvent::Retrying {
                attempt: 1,
                max: 3,
                reason: "x".into()
            }),
            None
        );
        assert_eq!(f.on_agent(&AgentEvent::Notice("n".into())), None);
    }

    #[test]
    fn error_reaches_the_user_and_clears_pending() {
        let mut f = UserModeFilter::new();
        f.on_agent(&assistant("half a thought"));
        assert_eq!(
            f.on_agent(&AgentEvent::Error("boom".into())),
            Some(Shown::Error("boom".into()))
        );
        assert_eq!(f.flush(), None);
    }

    #[test]
    fn finish_falls_back_to_the_summary_without_assistant_text() {
        let mut f = UserModeFilter::new();
        assert_eq!(
            f.on_agent(&AgentEvent::Finished {
                summary: "done".into(),
                success: true
            }),
            Some(Shown::Finished {
                summary: "done".into(),
                success: true
            })
        );
    }

    #[test]
    fn history_emits_the_last_assistant_message_at_flush() {
        let mut f = UserModeFilter::new();
        let ev = |ty: &str, out: &str| SessionEvent {
            event_type: ty.into(),
            output: out.into(),
            ..Default::default()
        };
        assert_eq!(
            f.on_session(&ev("user_message", "hola")),
            Some(Shown::User("hola".into()))
        );
        assert_eq!(f.on_session(&ev("assistant_message", "paso 1")), None);
        assert_eq!(f.on_session(&ev("tool_call", "")), None);
        assert_eq!(
            f.on_session(&ev("assistant_message", "listo, lo hice")),
            None
        );
        assert_eq!(f.flush(), Some(Shown::Assistant("listo, lo hice".into())));
    }
}
