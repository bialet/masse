//! Native file and directory tools (all via `tokio::fs`).

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::tool::{
    opt_bool, opt_str, opt_u64, req_str, schema_object, Tool, ToolContext, ToolResult,
};

const MAX_READ_BYTES: usize = 512 * 1024;

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file inside the project. Returns the contents, optionally sliced by line range. Use offset/limit for large files."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "path",
                    "string",
                    "Path relative to the project root (or absolute inside it).",
                ),
                (
                    "offset",
                    "integer",
                    "1-based first line to return. Default: 1.",
                ),
                (
                    "limit",
                    "integer",
                    "Maximum number of lines to return. Default: all.",
                ),
            ],
            &["path"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let path = match req_str(&args, "path").and_then(|p| ctx.resolve_existing(&p)) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let data = match tokio::fs::read(&path).await {
            Ok(d) => d,
            Err(e) => return ToolResult::err(format!("cannot read '{}': {e}", path.display())),
        };
        let truncated = data.len() > MAX_READ_BYTES;
        let slice = if truncated {
            &data[..MAX_READ_BYTES]
        } else {
            &data[..]
        };
        let text = String::from_utf8_lossy(slice);
        let lines: Vec<&str> = text.lines().collect();
        let offset = opt_u64(&args, "offset").unwrap_or(1).max(1) as usize;
        let limit = opt_u64(&args, "limit").map(|v| v as usize);
        let start = (offset - 1).min(lines.len());
        let end = match limit {
            Some(l) => (start + l).min(lines.len()),
            None => lines.len(),
        };
        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            out.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
        }
        if truncated {
            out.push_str(&format!(
                "[masse] file truncated at {MAX_READ_BYTES} bytes\n"
            ));
        }
        ToolResult::ok(out)
    }
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create or overwrite a text file. Parent directories are created by default. Prefer edit_file for targeted changes to an existing file."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("path", "string", "Destination path inside the project."),
                ("content", "string", "Full file contents to write."),
                (
                    "create_dirs",
                    "boolean",
                    "Create missing parent directories. Default: true.",
                ),
            ],
            &["path", "content"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let content = match req_str(&args, "content") {
            Ok(c) => c,
            Err(e) => return ToolResult::err(e),
        };
        let path = match ctx.resolve(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        if opt_bool(&args, "create_dirs", true) {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return ToolResult::err(format!("cannot create {}: {e}", parent.display()));
                }
            }
        }
        match tokio::fs::write(&path, content.as_bytes()).await {
            Ok(()) => ToolResult::ok(format!(
                "wrote {} bytes to {}",
                content.len(),
                ctx.display_path(&path)
            )),
            Err(e) => ToolResult::err(format!("cannot write '{}': {e}", path.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

pub struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace an exact substring in a text file. By default old_string must occur exactly once; set replace_all to replace every occurrence."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("path", "string", "File to edit."),
                ("old_string", "string", "Exact text to find."),
                ("new_string", "string", "Replacement text."),
                (
                    "replace_all",
                    "boolean",
                    "Replace every occurrence. Default: false.",
                ),
            ],
            &["path", "old_string", "new_string"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let old = match req_str(&args, "old_string") {
            Ok(s) => s,
            Err(e) => return ToolResult::err(e),
        };
        let new = match req_str(&args, "new_string") {
            Ok(s) => s,
            Err(e) => return ToolResult::err(e),
        };
        let replace_all = opt_bool(&args, "replace_all", false);
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("cannot read '{}': {e}", path.display())),
        };
        if old.is_empty() {
            return ToolResult::err("old_string must not be empty");
        }
        let count = content.matches(&old).count();
        if count == 0 {
            return ToolResult::err("old_string not found in file");
        }
        if count > 1 && !replace_all {
            return ToolResult::err(format!(
                "old_string occurs {count} times; provide more context or set replace_all=true"
            ));
        }
        let updated = if replace_all {
            content.replace(&old, &new)
        } else {
            content.replacen(&old, &new, 1)
        };
        match tokio::fs::write(&path, updated.as_bytes()).await {
            Ok(()) => ToolResult::ok(format!(
                "{} replacement(s) in {}",
                if replace_all { count } else { 1 },
                ctx.display_path(&path)
            )),
            Err(e) => ToolResult::err(format!("cannot write '{}': {e}", path.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// delete_file
// ---------------------------------------------------------------------------

pub struct DeleteFile;

#[async_trait]
impl Tool for DeleteFile {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a single file inside the project. Refuses to delete a directory."
    }

    fn schema(&self) -> Json {
        schema_object(&[("path", "string", "File to delete.")], &["path"])
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_dir() => {
                return ToolResult::err("path is a directory; use delete_dir");
            }
            Ok(_) => {}
            Err(e) => return ToolResult::err(format!("cannot stat '{}': {e}", path.display())),
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => ToolResult::ok(format!("deleted {}", ctx.display_path(&path))),
            Err(e) => ToolResult::err(format!("cannot delete '{}': {e}", path.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// create_dir
// ---------------------------------------------------------------------------

pub struct CreateDir;

#[async_trait]
impl Tool for CreateDir {
    fn name(&self) -> &str {
        "create_dir"
    }

    fn description(&self) -> &str {
        "Create a directory (and missing parents) inside the project."
    }

    fn schema(&self) -> Json {
        schema_object(&[("path", "string", "Directory to create.")], &["path"])
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let path = match ctx.resolve(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        match tokio::fs::create_dir_all(&path).await {
            Ok(()) => ToolResult::ok(format!("created {}", ctx.display_path(&path))),
            Err(e) => ToolResult::err(format!("cannot create '{}': {e}", path.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// delete_dir
// ---------------------------------------------------------------------------

pub struct DeleteDir;

#[async_trait]
impl Tool for DeleteDir {
    fn name(&self) -> &str {
        "delete_dir"
    }

    fn description(&self) -> &str {
        "Delete a directory. Non-recursive by default; set recursive=true to remove the tree. Never deletes the project root."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("path", "string", "Directory to delete."),
                (
                    "recursive",
                    "boolean",
                    "Remove contents recursively. Default: false.",
                ),
            ],
            &["path"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = match req_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        if let Err(e) = crate::path::assert_not_root(&ctx.root, &path) {
            return ToolResult::err(e);
        }
        let recursive = opt_bool(&args, "recursive", false);
        let result = if recursive {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_dir(&path).await
        };
        match result {
            Ok(()) => ToolResult::ok(format!("deleted {}", ctx.display_path(&path))),
            Err(e) => ToolResult::err(format!("cannot delete '{}': {e}", path.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// list_dir
// ---------------------------------------------------------------------------

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory. One entry per line, directories suffixed with '/'. Optionally recurse."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "path",
                    "string",
                    "Directory to list. Default: project root.",
                ),
                (
                    "recursive",
                    "boolean",
                    "Walk subdirectories. Default: false.",
                ),
                (
                    "max_entries",
                    "integer",
                    "Cap the number of entries returned. Default: 500.",
                ),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let rel = opt_str(&args, "path").unwrap_or_default();
        let path = match ctx.resolve_existing(&rel) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let max = opt_u64(&args, "max_entries").unwrap_or(500) as usize;
        let recursive = opt_bool(&args, "recursive", false);
        let mut out: Vec<String> = Vec::new();
        let mut stack = vec![path.clone()];
        let mut truncated = false;
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(e) => return ToolResult::err(format!("cannot list '{}': {e}", dir.display())),
            };
            let mut entries: Vec<(String, bool, std::path::PathBuf)> = Vec::new();
            while let Ok(Some(entry)) = rd.next_entry().await {
                let meta = entry.metadata().await.ok();
                let is_dir = meta.map(|m| m.is_dir()).unwrap_or(false);
                entries.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    is_dir,
                    entry.path(),
                ));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (_name, is_dir, full) in entries {
                let rel_display = ctx.display_path(&full);
                out.push(if is_dir {
                    format!("{rel_display}/")
                } else {
                    rel_display
                });
                if out.len() >= max {
                    truncated = true;
                    break;
                }
                if recursive && is_dir {
                    stack.push(full);
                }
            }
            if truncated {
                break;
            }
        }
        let mut text = out.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        if truncated {
            text.push_str(&format!("[masse] listing truncated at {max} entries\n"));
        }
        if text.is_empty() {
            text = "(empty directory)".to_string();
        }
        ToolResult::ok(text)
    }
}
