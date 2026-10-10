//! Git tools. All invoke the `git` binary with argument arrays.

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::CmdSpec;
use crate::tool::{
    opt_bool, opt_str, opt_str_array, opt_u64, req_str, schema_object, Tool, ToolContext,
    ToolResult,
};

fn git(args: Vec<String>) -> CmdSpec {
    CmdSpec::new("git").args(args)
}

async fn run_git(ctx: &ToolContext, args: Vec<String>, timeout_secs: Option<u64>) -> ToolResult {
    let spec = git(args);
    let out = ctx.run(&spec, ctx.timeout(timeout_secs)).await;
    ToolResult::from_cmd(&out)
}

// ---------------------------------------------------------------------------
// git_status
// ---------------------------------------------------------------------------

pub struct GitStatus;

#[async_trait]
impl Tool for GitStatus {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the working tree status in porcelain format."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[("timeout_secs", "integer", "Override the default timeout.")],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let mut a = vec!["status".into(), "--porcelain=v1".into(), "-b".into()];
        if let Some(p) = opt_str(&args, "path") {
            a.push("--".into());
            a.push(p);
        }
        run_git(ctx, a, opt_u64(&args, "timeout_secs")).await
    }
}

// ---------------------------------------------------------------------------
// git_diff
// ---------------------------------------------------------------------------

pub struct GitDiff;

#[async_trait]
impl Tool for GitDiff {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show unstaged changes (or staged changes with staged=true). Optionally limit to a path."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "staged",
                    "boolean",
                    "Show staged changes instead of the working tree. Default: false.",
                ),
                ("path", "string", "Limit the diff to this path."),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let mut a = vec!["diff".to_string()];
        if opt_bool(&args, "staged", false) {
            a.push("--staged".into());
        }
        a.push("--no-color".into());
        if let Some(p) = opt_str(&args, "path") {
            a.push("--".into());
            a.push(p);
        }
        run_git(ctx, a, opt_u64(&args, "timeout_secs")).await
    }
}

// ---------------------------------------------------------------------------
// git_log
// ---------------------------------------------------------------------------

pub struct GitLog;

#[async_trait]
impl Tool for GitLog {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show recent commits in oneline format."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "limit",
                    "integer",
                    "Number of commits to show. Default: 20.",
                ),
                ("path", "string", "Limit history to this path."),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let n = opt_u64(&args, "limit").unwrap_or(20);
        let mut a = vec![
            "log".into(),
            "--oneline".into(),
            "--no-color".into(),
            "-n".into(),
            n.to_string(),
        ];
        if let Some(p) = opt_str(&args, "path") {
            a.push("--".into());
            a.push(p);
        }
        run_git(ctx, a, opt_u64(&args, "timeout_secs")).await
    }
}

// ---------------------------------------------------------------------------
// git_commit
// ---------------------------------------------------------------------------

pub struct GitCommit;

#[async_trait]
impl Tool for GitCommit {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a commit with the given message. Set all=true to stage tracked changes first (git commit -a)."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("message", "string", "Commit message."),
                (
                    "all",
                    "boolean",
                    "Also stage modified tracked files. Default: false.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["message"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let message = match req_str(&args, "message") {
            Ok(m) => m,
            Err(e) => return ToolResult::err(e),
        };
        let mut a = vec!["commit".to_string()];
        if opt_bool(&args, "all", false) {
            a.push("-a".into());
        }
        a.push("-m".into());
        a.push(message);
        run_git(ctx, a, opt_u64(&args, "timeout_secs")).await
    }
}

// ---------------------------------------------------------------------------
// git_branch
// ---------------------------------------------------------------------------

pub struct GitBranch;

#[async_trait]
impl Tool for GitBranch {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn description(&self) -> &str {
        "Inspect or modify branches. action: list (default), current, create, delete. For create/delete, pass name."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "action",
                    "string",
                    "list | current | create | delete. Default: list.",
                ),
                ("name", "string", "Branch name for create/delete."),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let action = opt_str(&args, "action").unwrap_or_else(|| "list".into());
        let timeout = opt_u64(&args, "timeout_secs");
        match action.as_str() {
            "list" => run_git(ctx, vec!["branch".into(), "--no-color".into()], timeout).await,
            "current" => {
                run_git(ctx, vec!["branch".into(), "--show-current".into()], timeout).await
            }
            "create" => match opt_str(&args, "name") {
                Some(name) => run_git(ctx, vec!["branch".into(), "--".into(), name], timeout).await,
                None => ToolResult::err("git_branch action=create requires `name`"),
            },
            "delete" => match opt_str(&args, "name") {
                Some(name) => run_git(ctx, vec!["branch".into(), "-d".into(), name], timeout).await,
                None => ToolResult::err("git_branch action=delete requires `name`"),
            },
            other => ToolResult::err(format!("unknown git_branch action `{other}`")),
        }
    }
}

// ---------------------------------------------------------------------------
// git_stage
// ---------------------------------------------------------------------------

pub struct GitStage;

#[async_trait]
impl Tool for GitStage {
    fn name(&self) -> &str {
        "git_stage"
    }

    fn description(&self) -> &str {
        "Stage paths (git add). Set all=true to stage everything, or list explicit paths."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "paths",
                    "array",
                    "Paths to stage, relative to the project root.",
                ),
                (
                    "all",
                    "boolean",
                    "Stage all changes including deletions. Default: false.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let timeout = opt_u64(&args, "timeout_secs");
        let mut a = vec!["add".to_string()];
        if opt_bool(&args, "all", false) {
            a.push("-A".into());
            return run_git(ctx, a, timeout).await;
        }
        let paths = opt_str_array(&args, "paths");
        if paths.is_empty() {
            return ToolResult::err("provide `paths` or set all=true");
        }
        a.push("--".into());
        for p in paths {
            // Validate that each path stays inside the project.
            if let Err(e) = ctx.resolve(&p) {
                return ToolResult::err(e);
            }
            a.push(p);
        }
        run_git(ctx, a, timeout).await
    }
}

// ---------------------------------------------------------------------------
// git_push
// ---------------------------------------------------------------------------

pub struct GitPush;

#[async_trait]
impl Tool for GitPush {
    fn name(&self) -> &str {
        "git_push"
    }

    fn description(&self) -> &str {
        "Push the current branch to a remote (git push). Pushing to origin \
         deploys: the remote's post-receive hook builds and releases the app."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("remote", "string", "Remote name. Default: origin."),
                (
                    "branch",
                    "string",
                    "Branch to push. Default: the current branch (HEAD).",
                ),
                (
                    "force",
                    "boolean",
                    "Push with --force-with-lease. Default: false.",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &[],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let remote = opt_str(&args, "remote").unwrap_or_else(|| "origin".into());
        if remote.starts_with('-') || remote.contains(char::is_whitespace) {
            return ToolResult::err("invalid remote name");
        }
        let mut a = vec!["push".to_string()];
        if opt_bool(&args, "force", false) {
            a.push("--force-with-lease".into());
        }
        a.push(remote);
        match opt_str(&args, "branch") {
            Some(b) => {
                if b.starts_with('-') || b.contains(char::is_whitespace) {
                    return ToolResult::err("invalid branch name");
                }
                a.push(b);
            }
            None => a.push("HEAD".into()),
        }
        run_git(ctx, a, opt_u64(&args, "timeout_secs")).await
    }
}
