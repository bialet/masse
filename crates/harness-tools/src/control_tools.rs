//! Control tools that influence the agent loop itself rather than the project.

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::tool::{opt_bool, opt_str, schema_object, Tool, ToolContext, ToolResult};

/// Declares the task finished and stops the current turn.
///
/// The agent special-cases the [`crate::tool::Terminate`] flag this sets, so
/// calling `finish` is an explicit, unambiguous completion signal (as opposed
/// to merely running out of things to say).
pub struct Finish;

#[async_trait]
impl Tool for Finish {
    fn name(&self) -> &str {
        "finish"
    }

    fn description(&self) -> &str {
        "Declare the task complete and stop. Call this exactly once you have fully satisfied the user's request (or shown it is impossible), with a short summary of the result. Do not call it while you still have steps to carry out."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "summary",
                    "string",
                    "One or two sentences describing the final result.",
                ),
                (
                    "success",
                    "boolean",
                    "Whether the task succeeded. Default: true.",
                ),
            ],
            &["summary"],
        )
    }

    async fn call(&self, _ctx: &ToolContext, args: Json) -> ToolResult {
        let summary = opt_str(&args, "summary").unwrap_or_else(|| "task finished".to_string());
        let success = opt_bool(&args, "success", true);
        ToolResult::finish(summary, success)
    }
}
