//! Skills exposed as first-class tools.
//!
//! Each `skills/<name>/SKILL.md` becomes a tool whose name and description come
//! from the YAML frontmatter. Invoking the tool returns the markdown body, which
//! the agent runtime injects into the conversation as tool output.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::tool::{schema_object, Tool, ToolContext, ToolResult};

/// A loaded skill definition.
#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

/// A [`Tool`] wrapping a [`Skill`].
pub struct SkillTool {
    skill: Arc<Skill>,
}

impl SkillTool {
    pub fn new(skill: Skill) -> Self {
        SkillTool {
            skill: Arc::new(skill),
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        &self.skill.name
    }

    fn description(&self) -> &str {
        &self.skill.description
    }

    fn schema(&self) -> Json {
        schema_object(
            &[(
                "input",
                "string",
                "Optional context from the user to prime the skill.",
            )],
            &[],
        )
    }

    async fn call(&self, _ctx: &ToolContext, args: Json) -> ToolResult {
        let mut out = String::new();
        if let Some(input) = args.get("input").and_then(|v| v.as_str()) {
            if !input.trim().is_empty() {
                out.push_str("User context for this skill:\n");
                out.push_str(input);
                out.push_str("\n\n---\n\n");
            }
        }
        out.push_str(&self.skill.body);
        ToolResult::ok(out)
    }
}
