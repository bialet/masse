//! Bialet tools. Every invocation goes straight to the `bialet` binary with an
//! argument array; background servers are tracked in the in-memory registry.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::{truncate_lines, CmdSpec};
use crate::tool::{
    opt_bool, opt_str, opt_u64, req_str, schema_object, Tool, ToolContext, ToolResult,
};

const DEFAULT_SERVER_KEY: &str = "bialet_server";

fn log_path(ctx: &ToolContext) -> PathBuf {
    ctx.root.join(".masse").join("log.txt")
}

/// The last `n` lines of a text blob.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.to_string();
    }
    lines[lines.len() - n..].join("\n")
}

// ---------------------------------------------------------------------------
// bialet_run
// ---------------------------------------------------------------------------

pub struct BialetRun;

#[async_trait]
impl Tool for BialetRun {
    fn name(&self) -> &str {
        "bialet_run"
    }

    fn description(&self) -> &str {
        "Run a snippet of Wren code with `bialet -r <code>` and return its output."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("code", "string", "Wren source to execute."),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["code"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let code = match req_str(&args, "code") {
            Ok(c) => c,
            Err(e) => return ToolResult::err(e),
        };
        let spec = CmdSpec::new(&ctx.bialet_bin).args(vec!["-r".into(), code]);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        ToolResult::from_cmd(&out)
    }
}

// ---------------------------------------------------------------------------
// bialet_validate
// ---------------------------------------------------------------------------

pub struct BialetValidate;

#[async_trait]
impl Tool for BialetValidate {
    fn name(&self) -> &str {
        "bialet_validate"
    }

    fn description(&self) -> &str {
        "Validate the syntax of a Wren file with `bialet -q -t <file>`. Quiet by default: success returns exit 0 with no output; on failure the compiler diagnostics are returned."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "file",
                    "string",
                    "Wren file to validate, relative to the project root.",
                ),
                (
                    "quiet",
                    "boolean",
                    "Pass -q so a successful check prints nothing. Default: true.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["file"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "file") {
            Ok(f) => f,
            Err(e) => return ToolResult::err(e),
        };
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let mut a: Vec<String> = Vec::new();
        if opt_bool(&args, "quiet", true) {
            a.push("-q".into());
        }
        a.push("-t".into());
        a.push(path.display().to_string());
        let spec = CmdSpec::new(&ctx.bialet_bin).args(a);
        let out = ctx
            .run(&spec, ctx.timeout(opt_u64(&args, "timeout_secs")))
            .await;
        ToolResult::from_cmd(&out)
    }
}

// ---------------------------------------------------------------------------
// bialet_test
// ---------------------------------------------------------------------------

pub struct BialetTest;

#[async_trait]
impl Tool for BialetTest {
    fn name(&self) -> &str {
        "bialet_test"
    }

    fn description(&self) -> &str {
        "Run the Bialet test suite with `bialet [-q] -T [dir]`."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "dir",
                    "string",
                    "Directory containing _tests/. Default: project root.",
                ),
                (
                    "quiet",
                    "boolean",
                    "Pass -q for quiet, CI-friendly output. Default: false.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        // `-T` may take an optional directory, so flags must precede it.
        let mut a: Vec<String> = Vec::new();
        if opt_bool(&args, "quiet", false) {
            a.push("-q".into());
        }
        a.push("-T".into());
        if let Some(rel) = opt_str(&args, "dir") {
            let path = match ctx.resolve(&rel) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(e),
            };
            a.push(path.display().to_string());
        }
        let spec = CmdSpec::new(&ctx.bialet_bin).args(a);
        // Tests can legitimately take longer than the default.
        let timeout = ctx.timeout(opt_u64(&args, "timeout_secs").or(Some(120)));
        let out = ctx.run(&spec, timeout).await;
        ToolResult::from_cmd(&out)
    }
}

// ---------------------------------------------------------------------------
// bialet_server_start
// ---------------------------------------------------------------------------

pub struct BialetServerStart;

#[async_trait]
impl Tool for BialetServerStart {
    fn name(&self) -> &str {
        "bialet_server_start"
    }

    fn description(&self) -> &str {
        "Start a Bialet server in the background (`bialet -p <port> <path>`). Output is written to .masse/log.txt. Only one server per name is tracked, and a server that exits immediately is reported as an error with its log."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("port", "integer", "Port to bind. Default: 7001."),
                (
                    "path",
                    "string",
                    "App directory to serve. Default: project root.",
                ),
                (
                    "name",
                    "string",
                    "Registry key for this server. Default: bialet_server.",
                ),
                ("log", "string", "Log file path. Default: .masse/log.txt."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let port = opt_u64(&args, "port").unwrap_or(7001);
        let rel = opt_str(&args, "path").unwrap_or_default();
        let app_dir = match ctx.resolve(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let name = opt_str(&args, "name").unwrap_or_else(|| DEFAULT_SERVER_KEY.to_string());
        let log = match opt_str(&args, "log") {
            Some(l) => match ctx.resolve(&l) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(e),
            },
            None => log_path(ctx),
        };

        // Let bialet log to stdout and capture stdout+stderr into the log file.
        // This also records startup errors (e.g. a bind failure) that would
        // otherwise be lost.
        let cmd_args = vec![
            "-p".to_string(),
            port.to_string(),
            app_dir.display().to_string(),
        ];
        match ctx
            .processes
            .start(&name, &ctx.bialet_bin, &cmd_args, &ctx.root, Some(&log))
            .await
        {
            Ok(pid) => {
                // A bind failure (e.g. the port is taken) makes bialet exit
                // immediately; surface that instead of a false "started".
                tokio::time::sleep(Duration::from_millis(400)).await;
                if !ctx.processes.is_running(&name).await {
                    let log_text = tokio::fs::read_to_string(&log).await.unwrap_or_default();
                    let tail = tail_lines(log_text.trim_end(), 20);
                    let detail = if tail.is_empty() {
                        "(log is empty)".to_string()
                    } else {
                        tail
                    };
                    return ToolResult::err(format!(
                        "`{} -p {}` exited immediately (pid {pid}); log {}:\n{}",
                        ctx.bialet_bin,
                        port,
                        ctx.display_path(&log),
                        detail
                    ));
                }
                let shown_dir = {
                    let d = ctx.display_path(&app_dir);
                    if d.is_empty() {
                        ".".to_string()
                    } else {
                        d
                    }
                };
                ToolResult::ok(format!(
                    "started `{} -p {} {}` (pid {pid}); log: {}",
                    ctx.bialet_bin,
                    port,
                    shown_dir,
                    ctx.display_path(&log)
                ))
            }
            Err(e) => ToolResult::err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// bialet_server_stop
// ---------------------------------------------------------------------------

pub struct BialetServerStop;

#[async_trait]
impl Tool for BialetServerStop {
    fn name(&self) -> &str {
        "bialet_server_stop"
    }

    fn description(&self) -> &str {
        "Stop a background Bialet server started by bialet_server_start."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "name",
                    "string",
                    "Registry key of the server. Default: bialet_server.",
                ),
                (
                    "all",
                    "boolean",
                    "Stop every tracked background process. Default: false.",
                ),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        if opt_bool(&args, "all", false) {
            ctx.processes.stop_all().await;
            return ToolResult::ok("stopped all background processes");
        }
        let name = opt_str(&args, "name").unwrap_or_else(|| DEFAULT_SERVER_KEY.to_string());
        if ctx.processes.stop(&name).await {
            ToolResult::ok(format!("stopped `{name}`"))
        } else {
            ToolResult::err(format!("no running process named `{name}`"))
        }
    }
}

// ---------------------------------------------------------------------------
// bialet_log_read
// ---------------------------------------------------------------------------

pub struct BialetLogRead;

#[async_trait]
impl Tool for BialetLogRead {
    fn name(&self) -> &str {
        "bialet_log_read"
    }

    fn description(&self) -> &str {
        "Read the Bialet server log (.masse/log.txt by default). Returns the tail of the file."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("file", "string", "Log file path. Default: .masse/log.txt."),
                (
                    "tail",
                    "integer",
                    "Return only the last N lines. Default: 200.",
                ),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = opt_str(&args, "file").unwrap_or_else(|| ".masse/log.txt".to_string());
        let path = match ctx.resolve(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::err(format!("log file not found: {}", ctx.display_path(&path)))
            }
            Err(e) => return ToolResult::err(format!("cannot read log: {e}")),
        };
        let tail = opt_u64(&args, "tail").unwrap_or(200) as usize;
        let mut lines: Vec<&str> = content.lines().collect();
        if lines.len() > tail {
            lines = lines.split_off(lines.len() - tail);
        }
        ToolResult::ok(format!("{}\n", lines.join("\n")))
    }
}

// ---------------------------------------------------------------------------
// bialet_docs
// ---------------------------------------------------------------------------

pub struct BialetDocs;

#[async_trait]
impl Tool for BialetDocs {
    fn name(&self) -> &str {
        "bialet_docs"
    }

    fn description(&self) -> &str {
        "Consult the local Bialet/Wren documentation. Provide `topic` to search, `file` to read a whole page, or `list` to enumerate pages."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "topic",
                    "string",
                    "Case-insensitive text to search across the docs.",
                ),
                ("file", "string", "Relative doc path to read in full."),
                ("list", "boolean", "List all available documentation pages."),
                (
                    "max_results",
                    "integer",
                    "Maximum matching lines. Default: 60.",
                ),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let Some(docs) = ctx.docs_dir.clone() else {
            return ToolResult::err(
                "no Bialet docs directory configured; pass --bialet-docs <dir> or set MASSE_BIALET_DOCS",
            );
        };
        if !docs.is_dir() {
            return ToolResult::err(format!("docs directory not found: {}", docs.display()));
        }

        if opt_bool(&args, "list", false) {
            let mut files = Vec::new();
            collect_docs(&docs, &mut files).await;
            files.sort();
            let mut out = String::new();
            for f in &files {
                let rel = f.strip_prefix(&docs).unwrap_or(f);
                out.push_str(&rel.display().to_string());
                out.push('\n');
            }
            return ToolResult::ok(if out.is_empty() {
                "(no docs found)".into()
            } else {
                out
            });
        }

        if let Some(rel) = opt_str(&args, "file") {
            let path = match crate::path::resolve_existing_within(&docs, &rel) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(format!("cannot open doc '{rel}': {e}")),
            };
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => return ToolResult::err(format!("cannot read doc: {e}")),
            };
            let (text, _) = truncate_lines(&content, 1000);
            return ToolResult::ok(text);
        }

        let topic = match opt_str(&args, "topic") {
            Some(t) => t,
            None => {
                return ToolResult::err("provide `topic`, `file`, or list=true");
            }
        };
        let needle = topic.to_lowercase();
        let max = opt_u64(&args, "max_results").unwrap_or(60) as usize;
        let mut files = Vec::new();
        collect_docs(&docs, &mut files).await;
        files.sort();

        let mut out = String::new();
        let mut hits = 0usize;
        for file in files {
            let content = match tokio::fs::read_to_string(&file).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let rel = file
                .strip_prefix(&docs)
                .unwrap_or(&file)
                .display()
                .to_string();
            for (i, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    out.push_str(&format!("{rel}:{}: {}\n", i + 1, line.trim()));
                    hits += 1;
                    if hits >= max {
                        out.push_str(&format!("[masse] truncated at {max} matches\n"));
                        return ToolResult::ok(out);
                    }
                }
            }
        }
        if out.is_empty() {
            ToolResult::ok(format!("no documentation matched `{topic}`"))
        } else {
            ToolResult::ok(out)
        }
    }
}

async fn collect_docs(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "md" | "rst" | "txt") {
                    out.push(path);
                }
            }
        }
    }
}
