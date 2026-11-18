//! Model providers.
//!
//! The default provider speaks the OpenAI `chat/completions` protocol, which is
//! also implemented by DeepSeek, Groq, Ollama, OpenRouter, vLLM and friends. A
//! [`MockProvider`] is provided for offline development and tests.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::message::{ChatMessage, ToolCall};
use crate::usage::TokenUsage;

/// Provider failure, classified so the agent knows whether a retry can help.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ProviderError {
    /// Transient: timeouts, connection drops, HTTP 429/5xx. Worth retrying.
    #[error("{0}")]
    Retryable(String),
    /// Permanent: bad request, auth failure, malformed response. Do not retry.
    #[error("{0}")]
    Permanent(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProviderError::Retryable(_))
    }
}

/// Result of a provider call.
pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

/// How the harness identifies itself to providers. Gateways such as OpenCode
/// Go reject generic SDK user agents, so the version travels with every call.
pub const USER_AGENT: &str = concat!("masse/", env!("CARGO_PKG_VERSION"));

/// A single completion request.
#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// OpenAI-format tool schemas.
    pub tools: Vec<Json>,
    pub temperature: f32,
    /// Stable identifier for the conversation this request belongs to. Providers
    /// that cache or route per conversation use it; most ignore it.
    pub session_id: Option<String>,
}

/// A single completion response.
#[derive(Clone, Debug, Default)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    /// Tokens the provider reported for this completion. Zero when it sends no
    /// `usage` block (some local servers omit it).
    pub usage: TokenUsage,
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
    async fn complete(&self, request: ChatRequest) -> ProviderResult<ChatResponse>;
}

/// An OpenAI-compatible HTTP provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    /// True for OpenCode Console endpoints, which want `x-opencode-session` on
    /// every request so they can keep a conversation on one route.
    opencode: bool,
}

impl OpenAiProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::with_timeout(base_url, api_key, model, Duration::from_secs(120))
    }

    /// Build a provider with an explicit per-request timeout. A short connect
    /// timeout keeps an unreachable endpoint from hanging a turn for minutes.
    pub fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(request_timeout)
            .build()
            .unwrap_or_default();
        let base_url = base_url.into().trim_end_matches('/').to_string();
        OpenAiProvider {
            opencode: is_opencode_endpoint(&base_url),
            client,
            base_url,
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

/// Whether `base_url` points at OpenCode Console, where the harness should send
/// its conversation id in `x-opencode-session`.
fn is_opencode_endpoint(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| host == "opencode.ai" || host.ends_with(".opencode.ai"))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn complete(&self, request: ChatRequest) -> ProviderResult<ChatResponse> {
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

        let mut http = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body);
        if self.opencode {
            if let Some(id) = request.session_id.as_deref().filter(|id| !id.is_empty()) {
                http = http.header("x-opencode-session", id);
            }
        }

        let response = match http.send().await {
            Ok(r) => r,
            Err(e) => {
                // Network failures (timeout, refused, reset) are transient; a
                // construction error is not.
                let msg = format!("request to {} failed: {e}", self.endpoint());
                return Err(if e.is_builder() {
                    ProviderError::Permanent(msg)
                } else {
                    ProviderError::Retryable(msg)
                });
            }
        };

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.is_server_error();
            let msg = format!("provider returned HTTP {status}: {text}");
            return Err(if retryable {
                ProviderError::Retryable(msg)
            } else {
                ProviderError::Permanent(msg)
            });
        }

        let parsed: Json = serde_json::from_str(&text).map_err(|e| {
            ProviderError::Permanent(format!("provider returned invalid JSON: {e}: {text}"))
        })?;
        let message = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .ok_or_else(|| {
                ProviderError::Permanent(format!(
                    "provider response missing choices[0].message: {text}"
                ))
            })?;

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
            usage: parse_usage(&parsed),
        })
    }
}

/// Read the OpenAI-format `usage` block. Missing fields default to zero; a
/// provider that sends no `usage` at all yields an empty [`TokenUsage`].
fn parse_usage(parsed: &Json) -> TokenUsage {
    let field = |name: &str| {
        parsed
            .get("usage")
            .and_then(|u| u.get(name))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    };
    TokenUsage {
        prompt_tokens: field("prompt_tokens"),
        completion_tokens: field("completion_tokens"),
        total_tokens: field("total_tokens"),
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

    async fn complete(&self, request: ChatRequest) -> ProviderResult<ChatResponse> {
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
                "[mock] You said: {last_user}\n\nConfigure a real provider with `masse config set-api-key` (or --api-key / MASSE_API_KEY) to get model responses."
            ),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Serve exactly one request, returning its head and a valid completion.
    async fn serve_once() -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let mut reader = BufReader::new(reader);
            let mut head = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).await.unwrap();

            let reply = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            writer.write_all(response.as_bytes()).await.unwrap();
            head
        });
        (format!("http://{addr}/v1"), handle)
    }

    fn request(session: Option<&str>) -> ChatRequest {
        ChatRequest {
            model: "test-model".into(),
            session_id: session.map(str::to_string),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn identifies_itself_with_its_own_user_agent() {
        let (base_url, server) = serve_once().await;
        let provider = OpenAiProvider::new(base_url, "sk-test", "test-model");
        provider.complete(request(None)).await.unwrap();
        let head = server.await.unwrap().to_ascii_lowercase();
        assert!(
            head.contains(&format!(
                "user-agent: {}\r\n",
                USER_AGENT.to_ascii_lowercase()
            )),
            "user agent missing from:\n{head}"
        );
        assert!(!head.contains("x-opencode-session"));
    }

    #[tokio::test]
    async fn sends_the_session_to_opencode_endpoints() {
        let (base_url, server) = serve_once().await;
        let mut provider = OpenAiProvider::new(base_url, "sk-test", "test-model");
        // The local test server is not opencode.ai; force the flag.
        provider.opencode = true;
        provider
            .complete(request(Some("20261118-120000-abc")))
            .await
            .unwrap();
        let head = server.await.unwrap().to_ascii_lowercase();
        assert!(
            head.contains("x-opencode-session: 20261118-120000-abc\r\n"),
            "session header missing from:\n{head}"
        );
    }

    #[test]
    fn recognizes_opencode_endpoints_only() {
        assert!(is_opencode_endpoint(
            "https://opencode.ai/inference/go/openai/v1"
        ));
        assert!(is_opencode_endpoint("https://api.opencode.ai/v1"));
        assert!(!is_opencode_endpoint("https://api.deepseek.com/v1"));
        assert!(!is_opencode_endpoint("not a url"));
    }
}
