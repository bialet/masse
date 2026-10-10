//! The tool trait and its execution context.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::{self, CmdOutput, CmdSpec};
use crate::path;
use crate::process::ProcessRegistry;

/// Outcome classification, mapped onto the session `status` column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Ok,
    Error,
}

impl ToolStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolStatus::Ok => "ok",
            ToolStatus::Error => "error",
        }
    }

    pub fn is_error(self) -> bool {
        self == ToolStatus::Error
    }
}

/// A tool's request to end the current turn (used by the `finish` tool).
#[derive(Clone, Debug)]
pub struct Terminate {
    /// Whether the task succeeded.
    pub success: bool,
    /// One-line summary of the outcome.
    pub summary: String,
}

/// What a tool returns to the agent and to the session log.
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub status: ToolStatus,
    /// The text handed to the model.
    pub output: String,
    /// Process exit code when the tool ran an external binary.
    pub exit_code: Option<i32>,
    /// When set, the agent stops the turn after recording this result.
    pub terminate: Option<Terminate>,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        ToolResult {
            status: ToolStatus::Ok,
            output: output.into(),
            exit_code: None,
            terminate: None,
        }
    }

    pub fn err(output: impl Into<String>) -> Self {
        ToolResult {
            status: ToolStatus::Error,
            output: output.into(),
            exit_code: None,
            terminate: None,
        }
    }

    /// Ask the agent to end the turn with a success/failure summary.
    pub fn finish(output: impl Into<String>, success: bool) -> Self {
        let output = output.into();
        ToolResult {
            status: if success {
                ToolStatus::Ok
            } else {
                ToolStatus::Error
            },
            output: output.clone(),
            exit_code: None,
            terminate: Some(Terminate {
                success,
                summary: output,
            }),
        }
    }

    pub fn is_error(&self) -> bool {
        self.status.is_error()
    }

    /// Wrap a captured CLI result into a `ToolResult`.
    ///
    /// A command that ran but exited non-zero (a failed `bialet_validate`, a
    /// red `bialet_test`, a `sqlite3` error, ...) is an error, not a success:
    /// the model needs a real red signal, not just prose to interpret. Tools
    /// where a non-zero exit is expected (e.g. `grep` finding no matches)
    /// handle it before calling this.
    pub fn from_cmd(cmd: &CmdOutput) -> Self {
        let status = if cmd.success() {
            ToolStatus::Ok
        } else {
            ToolStatus::Error
        };
        ToolResult {
            status,
            output: cmd.to_agent_text(),
            exit_code: cmd.exit_code,
            terminate: None,
        }
    }
}

/// Shared, immutable context handed to every tool invocation.
#[derive(Clone)]
pub struct ToolContext {
    /// Absolute, canonical project working directory.
    pub root: PathBuf,
    /// Default timeout for external commands.
    pub default_timeout: Duration,
    /// In-memory registry of background processes.
    pub processes: Arc<ProcessRegistry>,
    /// Bialet binary name or path.
    pub bialet_bin: String,
    /// Optional directory holding Bialet/Wren documentation.
    pub docs_dir: Option<PathBuf>,
}

impl ToolContext {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        ToolContext {
            root,
            default_timeout: Duration::from_secs(30),
            processes: Arc::new(ProcessRegistry::new()),
            bialet_bin: "bialet".to_string(),
            docs_dir: None,
        }
    }

    pub fn timeout(&self, override_secs: Option<u64>) -> Duration {
        override_secs
            .map(Duration::from_secs)
            .unwrap_or(self.default_timeout)
    }

    /// Resolve a path argument, refusing to leave the project root.
    pub fn resolve(&self, user: &str) -> Result<PathBuf, String> {
        path::resolve_within(&self.root, user)
    }

    /// Resolve a path that must already exist, following symlinks safely.
    pub fn resolve_existing(&self, user: &str) -> Result<PathBuf, String> {
        path::resolve_existing_within(&self.root, user)
    }

    /// A human-readable, root-relative display path.
    pub fn display_path(&self, p: &std::path::Path) -> String {
        p.strip_prefix(&self.root)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    }

    /// Run an external command in the project root.
    pub async fn run(&self, spec: &CmdSpec, timeout: Duration) -> CmdOutput {
        cli::run(spec, &self.root, timeout).await
    }
}

/// A single, atomic capability exposed to the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier used in tool calls and in the session log.
    fn name(&self) -> &str;
    /// Human/model-facing description.
    fn description(&self) -> &str;
    /// JSON Schema describing the parameters object.
    fn schema(&self) -> Json;
    /// Whether the harness must ask the user before running it.
    fn requires_approval(&self) -> bool {
        false
    }
    /// Execute the tool. Never panics: errors become `ToolResult::err`.
    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult;
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Fetch a required string argument.
pub fn req_str(args: &Json, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required string argument `{key}`"))
}

/// Fetch an optional string argument (empty strings become `None`).
pub fn opt_str(args: &Json, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Fetch an optional boolean argument with a default.
pub fn opt_bool(args: &Json, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Fetch an optional unsigned integer argument.
pub fn opt_u64(args: &Json, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// Fetch an optional string array argument.
pub fn opt_str_array(args: &Json, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Json::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Build a simple JSON Schema object from `(name, type, description)` triples.
pub fn schema_object(properties: &[(&str, &str, &str)], required: &[&str]) -> Json {
    let mut props = serde_json::Map::new();
    for (name, ty, desc) in properties {
        props.insert(
            (*name).to_string(),
            serde_json::json!({ "type": ty, "description": desc }),
        );
    }
    serde_json::json!({
        "type": "object",
        "properties": Json::Object(props),
        "required": required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(exit_code: Option<i32>, timed_out: bool, error: Option<&str>) -> CmdOutput {
        CmdOutput {
            exit_code,
            timed_out,
            error: error.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_zero_exit_is_ok() {
        assert_eq!(
            ToolResult::from_cmd(&cmd(Some(0), false, None)).status,
            ToolStatus::Ok
        );
    }

    #[test]
    fn a_non_zero_exit_is_an_error() {
        assert_eq!(
            ToolResult::from_cmd(&cmd(Some(1), false, None)).status,
            ToolStatus::Error
        );
        assert_eq!(
            ToolResult::from_cmd(&cmd(Some(128), false, None)).status,
            ToolStatus::Error
        );
    }

    #[test]
    fn a_timeout_or_spawn_failure_is_an_error() {
        assert_eq!(
            ToolResult::from_cmd(&cmd(None, true, None)).status,
            ToolStatus::Error
        );
        assert_eq!(
            ToolResult::from_cmd(&cmd(None, false, Some("spawn failed"))).status,
            ToolStatus::Error
        );
    }
}
