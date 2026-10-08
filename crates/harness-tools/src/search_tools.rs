//! Search tools backed by the `grep` and `find` binaries.
//!
//! Arguments are always passed as an array. Output is filtered and truncated in
//! Rust rather than through shell pipes.

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::{truncate_lines, CmdSpec};
use crate::tool::{
    opt_bool, opt_str, opt_u64, req_str, schema_object, Tool, ToolContext, ToolResult,
};

const MAX_OUTPUT_LINES: usize = 400;

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with the system `grep`. Recursive by default. Returns matching lines with file names."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("pattern", "string", "Regular expression to search for."),
                (
                    "path",
                    "string",
                    "File or directory to search. Default: project root.",
                ),
                (
                    "include",
                    "string",
                    "Only search files matching this glob, e.g. '*.wren'.",
                ),
                (
                    "case_insensitive",
                    "boolean",
                    "Ignore case. Default: false.",
                ),
                (
                    "fixed",
                    "boolean",
                    "Treat pattern as a literal string. Default: false.",
                ),
                (
                    "max_count",
                    "integer",
                    "Stop after this many matches per file.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["pattern"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let pattern = match req_str(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let rel = opt_str(&args, "path").unwrap_or_default();
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };

        let mut args_vec: Vec<String> = vec!["-r".into(), "-n".into()];
        if opt_bool(&args, "case_insensitive", false) {
            args_vec.push("-i".into());
        }
        if opt_bool(&args, "fixed", false) {
            args_vec.push("-F".into());
        }
        let count = opt_u64(&args, "max_count");
        if let Some(c) = count {
            args_vec.push("-m".into());
            args_vec.push(c.to_string());
        }
        if let Some(inc) = opt_str(&args, "include") {
            args_vec.push("--include".into());
            args_vec.push(inc);
        }
        // `--` terminates option parsing so a pattern starting with '-' is safe.
        args_vec.push("--".into());
        args_vec.push(pattern);
        args_vec.push(path.display().to_string());

        let spec = CmdSpec::new("grep").args(args_vec);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        // grep exits 1 when there are simply no matches; that is not an error.
        if out.exit_code == Some(1) {
            return ToolResult::ok("no matches");
        }
        let (stdout, _) = truncate_lines(&out.stdout, MAX_OUTPUT_LINES);
        if out.error.is_some() || out.timed_out {
            return ToolResult::from_cmd(&out);
        }
        if out.exit_code == Some(0) {
            ToolResult {
                status: crate::tool::ToolStatus::Ok,
                output: stdout,
                exit_code: out.exit_code,
            }
        } else {
            ToolResult::from_cmd(&out)
        }
    }
}

// ---------------------------------------------------------------------------
// glob (find)
// ---------------------------------------------------------------------------

pub struct FindGlob;

#[async_trait]
impl Tool for FindGlob {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files or directories by name pattern using the system `find`. Supports a max depth."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "pattern",
                    "string",
                    "Shell-style name pattern, e.g. '*.wren'.",
                ),
                (
                    "path",
                    "string",
                    "Directory to search. Default: project root.",
                ),
                (
                    "type",
                    "string",
                    "'f' for files (default) or 'd' for directories.",
                ),
                ("max_depth", "integer", "Maximum search depth."),
                ("name", "string", "Deprecated alias for `pattern`."),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let pattern = opt_str(&args, "pattern")
            .or_else(|| opt_str(&args, "name"))
            .unwrap_or_else(|| "*".to_string());
        let rel = opt_str(&args, "path").unwrap_or_default();
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let kind = opt_str(&args, "type").unwrap_or_else(|| "f".to_string());
        let kind = if kind == "d" { "d" } else { "f" };

        let mut args_vec: Vec<String> = vec![path.display().to_string()];
        if let Some(d) = opt_u64(&args, "max_depth") {
            args_vec.push("-maxdepth".into());
            args_vec.push(d.to_string());
        }
        args_vec.push("-type".into());
        args_vec.push(kind.into());
        args_vec.push("-name".into());
        args_vec.push(pattern);

        let spec = CmdSpec::new("find").args(args_vec);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        if out.error.is_some() || out.timed_out {
            return ToolResult::from_cmd(&out);
        }
        let (stdout, _) = truncate_lines(&out.stdout, MAX_OUTPUT_LINES);
        if stdout.trim().is_empty() {
            return ToolResult::ok("no matches");
        }
        ToolResult {
            status: crate::tool::ToolStatus::Ok,
            output: stdout,
            exit_code: out.exit_code,
        }
    }
}
