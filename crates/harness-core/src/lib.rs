//! masse harness core: the agent runtime that ties a provider, the tool
//! registry and session persistence together.

pub mod agent;
pub mod context;
pub mod error;
pub mod markdown;
pub mod message;
pub mod present;
pub mod prompt;
pub mod provider;
pub mod usage;

pub use agent::{
    rebuild_messages, Agent, AgentConfig, AgentEvent, AgentHandle, DEFAULT_COMPACT_AT_PERCENT,
};
pub use context::ProjectContext;
pub use error::{CoreError, Result};
pub use markdown::{Block, Inline};
pub use message::{ChatMessage, Role, ToolCall};
pub use present::{Shown, UserModeFilter};
pub use prompt::build_system_prompt;
pub use provider::{
    ChatRequest, ChatResponse, MockProvider, OpenAiProvider, Provider, ProviderError,
    ProviderResult,
};
pub use usage::{
    context_window_for, format_cost, format_tokens, pricing_for, ModelPricing, TokenUsage,
    UsageReport,
};
