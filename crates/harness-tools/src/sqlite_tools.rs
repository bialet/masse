//! SQLite tools backed by the `sqlite3` binary.
//!
//! Three tools split by blast radius:
//!
//! * `sqlite_query`  — reads only; the database is opened `-readonly`.
//! * `sqlite_execute`— row changes (DML); schema changes are refused.
//! * `sqlite_schema` — structure changes (DDL); the exception, not the rule.
//!
//! The read/write boundary is real (the `-readonly` open flag). The DML/DDL
//! split is a workflow guard: Bialet keeps its schema in `_migration.wren`, so
//! ad-hoc DDL should stand out instead of hiding inside a data edit.

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

/// What a SQL statement is, as far as the workflow guard cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Reads: SELECT, VALUES, EXPLAIN.
    Read,
    /// Row changes and transactions: INSERT, UPDATE, DELETE, WITH, ...
    Data,
    /// Structure changes: CREATE, ALTER, DROP, INDEX, VACUUM, ...
    Schema,
    /// Nothing recognized (empty input or an unknown leading verb).
    Other,
}

/// The first significant SQL keyword, skipping leading whitespace and
/// `--` / `/* */` comments. Returns it uppercased, or `None` when absent.
fn first_keyword(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        break;
    }
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == start {
        None
    } else {
        Some(sql[start..i].to_ascii_uppercase())
    }
}

/// Classify a statement by its leading verb.
///
/// Multi-statement scripts are judged by their first statement on purpose:
/// this is a soft workflow guard, not a SQL parser. `WITH` is treated as data,
/// since a CTE can precede a SELECT/INSERT/UPDATE/DELETE but never DDL.
fn classify(sql: &str) -> Kind {
    match first_keyword(sql).as_deref() {
        Some("SELECT" | "VALUES" | "EXPLAIN") => Kind::Read,
        Some(
            "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "WITH" | "BEGIN" | "COMMIT" | "END"
            | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "PRAGMA",
        ) => Kind::Data,
        Some(
            "CREATE" | "ALTER" | "DROP" | "VACUUM" | "REINDEX" | "ANALYZE" | "ATTACH" | "DETACH",
        ) => Kind::Schema,
        _ => Kind::Other,
    }
}

// ---------------------------------------------------------------------------
// sqlite_query (read-only)
// ---------------------------------------------------------------------------

pub struct SqliteQuery;

#[async_trait]
impl Tool for SqliteQuery {
    fn name(&self) -> &str {
        "sqlite_query"
    }

    fn description(&self) -> &str {
        "Run a read-only SQL query against a SQLite database and return rows as JSON. The database is opened `-readonly`, so any write fails at the connection. The database defaults to _db.sqlite3."
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
// sqlite_execute (data changes only)
// ---------------------------------------------------------------------------

pub struct SqliteExecute;

#[async_trait]
impl Tool for SqliteExecute {
    fn name(&self) -> &str {
        "sqlite_execute"
    }

    fn description(&self) -> &str {
        "Change rows in a SQLite database: INSERT, UPDATE, DELETE, REPLACE, or a transaction. Uses -bail so the first error aborts. Schema changes (CREATE/ALTER/DROP/...) are refused — put those in _migration.wren or use sqlite_schema."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("sql", "string", "INSERT/UPDATE/DELETE statement to run."),
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
        if classify(&sql) == Kind::Schema {
            return ToolResult::err(format!(
                "sqlite_execute is for data changes (INSERT/UPDATE/DELETE). \
                 `{}` is a schema change: add it to _migration.wren, or use \
                 sqlite_schema to apply it directly.",
                first_keyword(&sql).unwrap_or_default()
            ));
        }
        run_write(ctx, &args, sql).await
    }
}

// ---------------------------------------------------------------------------
// sqlite_schema (structure changes only)
// ---------------------------------------------------------------------------

pub struct SqliteSchema;

#[async_trait]
impl Tool for SqliteSchema {
    fn name(&self) -> &str {
        "sqlite_schema"
    }

    fn description(&self) -> &str {
        "Apply a schema change to a SQLite database: CREATE/ALTER/DROP TABLE, INDEX, VIEW or TRIGGER, VACUUM. Prefer declaring the change in _migration.wren so it is reproducible; use this to apply or inspect a structure change directly. Uses -bail so the first error aborts."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "sql",
                    "string",
                    "DDL statement to run (CREATE/ALTER/DROP/...).",
                ),
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
        if classify(&sql) != Kind::Schema {
            return ToolResult::err(
                "sqlite_schema only runs structure changes (CREATE/ALTER/DROP/INDEX/...). \
                 Use sqlite_execute for data or sqlite_query to read.",
            );
        }
        run_write(ctx, &args, sql).await
    }
}

/// Shared write path for `sqlite_execute` and `sqlite_schema`.
async fn run_write(ctx: &ToolContext, args: &Json, sql: String) -> ToolResult {
    let db = match open_args(ctx, opt_str(args, "db").as_deref()) {
        Ok(d) => d,
        Err(e) => return ToolResult::err(e),
    };
    let spec = CmdSpec::new("sqlite3").args(vec!["-bail".into(), db, sql]);
    let out = ctx
        .run(&spec, ctx.timeout(opt_u64(args, "timeout_secs")))
        .await;
    if out.success() {
        ToolResult {
            status: crate::tool::ToolStatus::Ok,
            output: if out.stdout.trim().is_empty() {
                "ok".into()
            } else {
                out.stdout.clone()
            },
            exit_code: out.exit_code,
            terminate: None,
        }
    } else {
        ToolResult::from_cmd(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_keyword_skips_whitespace_and_comments() {
        assert_eq!(first_keyword("  \n\tSELECT 1").as_deref(), Some("SELECT"));
        assert_eq!(
            first_keyword("-- hi\nINSERT INTO t VALUES (1)").as_deref(),
            Some("INSERT")
        );
        assert_eq!(
            first_keyword("/* c */ UPDATE t SET a=1").as_deref(),
            Some("UPDATE")
        );
        assert_eq!(first_keyword("-- only a comment\n").as_deref(), None);
        assert_eq!(first_keyword("").as_deref(), None);
    }

    #[test]
    fn classify_maps_the_leading_verb() {
        assert_eq!(classify("SELECT * FROM t"), Kind::Read);
        assert_eq!(classify("explain select 1"), Kind::Read);
        assert_eq!(classify("INSERT INTO t VALUES (1)"), Kind::Data);
        assert_eq!(classify("WITH x AS (SELECT 1) DELETE FROM t"), Kind::Data);
        assert_eq!(classify("CREATE TABLE t(a)"), Kind::Schema);
        assert_eq!(classify("DROP INDEX i"), Kind::Schema);
        assert_eq!(classify("VACUUM"), Kind::Schema);
        assert_eq!(classify("not sql"), Kind::Other);
    }

    #[tokio::test]
    async fn execute_refuses_schema_changes() {
        let ctx = ToolContext::new(std::env::temp_dir());
        let r = SqliteExecute
            .call(&ctx, serde_json::json!({"sql": "CREATE TABLE t(a)"}))
            .await;
        assert!(r.is_error());
        assert!(r.output.contains("schema change"), "{}", r.output);
        assert!(r.output.contains("_migration.wren"), "{}", r.output);
    }

    #[tokio::test]
    async fn schema_refuses_data_changes() {
        let ctx = ToolContext::new(std::env::temp_dir());
        let r = SqliteSchema
            .call(&ctx, serde_json::json!({"sql": "INSERT INTO t VALUES (1)"}))
            .await;
        assert!(r.is_error());
        assert!(r.output.contains("structure changes"), "{}", r.output);
    }
}
