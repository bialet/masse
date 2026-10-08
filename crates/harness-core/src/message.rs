//! Chat message model, independent of any provider wire format.

use serde_json::Value as Json;

/// Who authored a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A tool invocation requested by the model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments string as returned by the model.
    pub arguments: String,
}

/// One turn in the conversation.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        ChatMessage {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        ChatMessage {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }

    /// Serialize to the OpenAI chat-completions message shape.
    pub fn to_openai(&self) -> Json {
        let mut obj = serde_json::Map::new();
        obj.insert("role".into(), Json::String(self.role.as_str().into()));

        match self.role {
            Role::Assistant if !self.tool_calls.is_empty() => {
                if self.content.is_empty() {
                    obj.insert("content".into(), Json::Null);
                } else {
                    obj.insert("content".into(), Json::String(self.content.clone()));
                }
                let calls: Vec<Json> = self
                    .tool_calls
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "type": "function",
                            "function": { "name": c.name, "arguments": c.arguments }
                        })
                    })
                    .collect();
                obj.insert("tool_calls".into(), Json::Array(calls));
            }
            Role::Tool => {
                obj.insert("content".into(), Json::String(self.content.clone()));
                if let Some(id) = &self.tool_call_id {
                    obj.insert("tool_call_id".into(), Json::String(id.clone()));
                }
            }
            _ => {
                obj.insert("content".into(), Json::String(self.content.clone()));
            }
        }
        Json::Object(obj)
    }
}
