//! masse harness core: the agent runtime that ties a provider, the tool
//! registry and session persistence together.

pub mod agent;
pub mod context;
pub mod error;
pub mod message;
pub mod prompt;
pub mod provider;

pub use agent::{rebuild_messages, Agent, AgentConfig, AgentEvent, AgentHandle};
pub use context::ProjectContext;
pub use error::{CoreError, Result};
pub use message::{ChatMessage, Role, ToolCall};
pub use prompt::build_system_prompt;
pub use provider::{ChatRequest, ChatResponse, MockProvider, OpenAiProvider, Provider};
