//! SQLite tools backed by the `sqlite3` binary.

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::{truncate_lines, CmdSpec};
use crate::tool::{opt_str, opt_u64, req_str, schema_object, Tool, ToolContext, ToolResult};

const MAX_OUTPUT_LINES: usize = 500;

fn open_args(ctx: &ToolContext, db: Option<&str>) -> Result<String, String> {
    let db = db.unwrap_or("_db.sqlite3");
    // Resolve (not necessarily existing) so the DB always lives in the project.
    Ok(ctx.resolve(db)?.display().to_string())
}

// ---------------------------------------------------------------------------
// sqlite_query
// ---------------------------------------------------------------------------

pub struct SqliteQuery;

#[async_trait]
impl Tool for SqliteQuery {
    fn name(&self) -> &str {
        "sqlite_query"
    }

    fn description(&self) -> &str {
        "Run a read-only SQL query against a SQLite database and return rows as JSON. The database defaults to _db.sqlite3."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("sql", "string", "SELECT statement to run."),
                (
                    "db",
                    "string",
                    "Database path inside the project. Default: _db.sqlite3.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["sql"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let sql = match req_str(&args, "sql") {
            Ok(s) => s,
            Err(e) => return ToolResult::err(e),
        };
        let db = match open_args(ctx, opt_str(&args, "db").as_deref()) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(e),
        };
        let spec = CmdSpec::new("sqlite3").args(vec!["-json".into(), "-readonly".into(), db, sql]);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        let (stdout, _) = truncate_lines(&out.stdout, MAX_OUTPUT_LINES);
        if out.error.is_some() || out.timed_out || out.exit_code != Some(0) {
            let mut result = ToolResult::from_cmd(&out);
            if result.output.trim().is_empty() {
                result.output = "sqlite3 reported an error with no message".into();
            }
            return result;
        }
        ToolResult {
            status: crate::tool::ToolStatus::Ok,
            output: if stdout.trim().is_empty() {
                "[]".into()
            } else {
                stdout
            },
            exit_code: out.exit_code,
            terminate: None,
        }
    }
}

// ---------------------------------------------------------------------------
// sqlite_execute
// ---------------------------------------------------------------------------

pub struct SqliteExecute;

#[async_trait]
impl Tool for SqliteExecute {
    fn name(&self) -> &str {
        "sqlite_execute"
    }

    fn description(&self) -> &str {
        "Run a write statement (INSERT/UPDATE/DELETE/DDL) against a SQLite database. Uses -bail so the first error aborts."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("sql", "string", "SQL statement to execute."),
                (
                    "db",
                    "string",
                    "Database path inside the project. Default: _db.sqlite3.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["sql"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let sql = match req_str(&args, "sql") {
            Ok(s) => s,
            Err(e) => return ToolResult::err(e),
        };
        let db = match open_args(ctx, opt_str(&args, "db").as_deref()) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(e),
        };
        let spec = CmdSpec::new("sqlite3").args(vec!["-bail".into(), db, sql]);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        if out.success() {
            let mut text = out.stdout.clone();
            if text.trim().is_empty() {
                text = "ok".into();
            }
            ToolResult {
                status: crate::tool::ToolStatus::Ok,
                output: text,
                exit_code: out.exit_code,
                terminate: None,
            }
        } else {
            ToolResult::from_cmd(&out)
        }
    }
}
