//! Tool registry: lookup, schemas and dispatch.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value as Json;

use crate::tool::{Tool, ToolContext, ToolResult};

/// An ordered, name-indexed collection of tools.
#[derive(Default)]
pub struct ToolRegistry {
    order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if !self.tools.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Names in registration order.
    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.order.iter().filter_map(|n| self.get(n)).collect()
    }

    /// OpenAI-style function schemas for every registered tool.
    pub fn openai_schemas(&self) -> Vec<Json> {
        self.tools()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema(),
                    }
                })
            })
            .collect()
    }

    /// Dispatch a call by name. Unknown tools yield an error result instead of
    /// a panic, so a hallucinated tool name never crashes the loop.
    pub async fn call(&self, name: &str, ctx: &ToolContext, args: Json) -> ToolResult {
        match self.get(name) {
            Some(tool) => tool.call(ctx, args).await,
            None => ToolResult::err(format!(
                "unknown tool `{name}`; available tools: {}",
                self.names().join(", ")
            )),
        }
    }
}
