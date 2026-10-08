//! Model providers.
//!
//! The default provider speaks the OpenAI `chat/completions` protocol, which is
//! also implemented by DeepSeek, Groq, Ollama, OpenRouter, vLLM and friends. A
//! [`MockProvider`] is provided for offline development and tests.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::message::{ChatMessage, ToolCall};

/// A single completion request.
#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// OpenAI-format tool schemas.
    pub tools: Vec<Json>,
    pub temperature: f32,
}

/// A single completion response.
#[derive(Clone, Debug, Default)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl ChatResponse {
    pub fn wants_tools(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Anything that can turn a conversation into the next assistant message.
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse>;
}

/// An OpenAI-compatible HTTP provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        OpenAiProvider {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    fn endpoint(&self) -> String {
        if self.base_url.ends_with("/chat/completions") {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let messages: Vec<Json> = request.messages.iter().map(|m| m.to_openai()).collect();
        let mut body = json!({
            "model": if request.model.is_empty() { self.model.clone() } else { request.model },
            "messages": messages,
            "temperature": request.temperature,
        });
        if !request.tools.is_empty() {
            body["tools"] = Json::Array(request.tools.clone());
            body["tool_choice"] = Json::String("auto".into());
        }

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("sending completion request")?;

        let status = response.status();
        let text = response
            .text()
            .await
            .context("reading completion response")?;
        if !status.is_success() {
            return Err(anyhow!("provider returned HTTP {status}: {text}"));
        }

        let parsed: Json = serde_json::from_str(&text).context("provider returned invalid JSON")?;
        let message = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .ok_or_else(|| anyhow!("provider response missing choices[0].message: {text}"))?;

        let content = match message.get("content") {
            Some(Json::String(s)) => s.clone(),
            Some(Json::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };

        let mut tool_calls = Vec::new();
        if let Some(Json::Array(calls)) = message.get("tool_calls") {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .map(|v| match v {
                        Json::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "{}".to_string());
                if !name.is_empty() {
                    tool_calls.push(ToolCall {
                        id: if id.is_empty() {
                            format!("call_{}", tool_calls.len())
                        } else {
                            id
                        },
                        name,
                        arguments,
                    });
                }
            }
        }

        let finish_reason = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
        })
    }
}

/// A deterministic provider for offline development and tests.
pub struct MockProvider {
    model: String,
}

impl MockProvider {
    pub fn new() -> Self {
        MockProvider {
            model: "mock".to_string(),
        }
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let _ = self.model;
        // If the last message is a tool result, acknowledge it instead of looping.
        if let Some(last) = request.messages.last() {
            if last.role == crate::message::Role::Tool {
                return Ok(ChatResponse {
                    content: format!(
                        "[mock] received tool result from `{}` ({} chars). No further action.",
                        last.name.clone().unwrap_or_default(),
                        last.content.len()
                    ),
                    ..Default::default()
                });
            }
        }
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == crate::message::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChatResponse {
            content: format!(
                "[mock] You said: {last_user}\n\nConfigure a real provider with --api-key (or MASSE_API_KEY) to get model responses."
            ),
            ..Default::default()
        })
    }
}
