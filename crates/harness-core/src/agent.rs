//! The agent runtime: the tool-calling loop and the actor that owns it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use harness_session::{encode_json_args, Event, SessionStore, Status as SessionStatus};
use harness_tools::cli::{cap_bytes, run as run_cmd, CmdSpec};
use harness_tools::{ToolContext, ToolRegistry, ToolResult, ToolStatus};
use tokio::sync::{broadcast, mpsc};

use crate::message::ChatMessage;
use crate::provider::{ChatRequest, Provider};
use crate::usage::{context_window_for, pricing_for, ModelPricing, TokenUsage, UsageReport};

/// Tunables for a single agent.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_iterations: usize,
    /// How many times a transient provider error is retried per model call.
    pub max_retries: usize,
    /// Base delay for the retry backoff, in milliseconds.
    pub retry_delay_ms: u64,
    /// Commit the workspace after every turn (and `git init` if needed).
    pub auto_commit: bool,
    /// After committing, `git push origin HEAD` so the remote deploys. Requires
    /// `auto_commit`.
    pub auto_push: bool,
    pub system_prompt: String,
    /// Override the built-in price for prompt tokens, in USD per million.
    pub price_input: Option<f64>,
    /// Override the built-in price for completion tokens, in USD per million.
    pub price_output: Option<f64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            model: String::new(),
            temperature: 0.2,
            max_iterations: 120,
            max_retries: 3,
            retry_delay_ms: 800,
            auto_commit: false,
            auto_push: false,
            system_prompt: String::new(),
            price_input: None,
            price_output: None,
        }
    }
}

/// Exponential backoff, capped so a turn never stalls for long.
fn retry_delay(base_ms: u64, attempt: u32) -> u64 {
    let factor = 1u64 << attempt.saturating_sub(1).min(6);
    base_ms.saturating_mul(factor).min(15_000)
}

/// First line of `text`, trimmed and truncated to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        line.chars().take(max).collect()
    }
}

/// Tools whose `path` argument can point at a `.wren` file we just changed.
const MUTATING_FILE_TOOLS: [&str; 3] = ["write_file", "edit_file", "delete_file"];

/// True when a tool call changed a `.wren` file, judged from its arguments.
fn mutates_wren(name: &str, args: &serde_json::Value) -> bool {
    if !MUTATING_FILE_TOOLS.contains(&name) {
        return false;
    }
    args.get("path")
        .and_then(|v| v.as_str())
        .map(|p| p.to_lowercase().ends_with(".wren"))
        .unwrap_or(false)
}

/// Does this assistant message look like a question the user must answer?
/// Used to avoid nagging about tests when the model stopped to ask something.
fn looks_like_question(text: &str) -> bool {
    text.contains('?') || text.contains('¿')
}

/// One-shot nudge injected into the conversation when the model tries to finish
/// after changing Wren code without running the test suite.
const VERIFY_BEFORE_FINISH: &str = "Before you finish: you changed Wren files but have not run `bialet_test` (or an equivalent end-to-end check) since the last change. \
In this harness a behaviour change is not done until it is verified. Add or update an acceptance test under `_tests/` that encodes what the user asked for \
(for a form: POST the values, then GET the page and assert they were stored), run `bialet_test`, and iterate until it is green. Then finish again. \
If no test applies here, say why in one line and finish again.";

/// One-shot nudge injected before the test nudge when the model changed Wren
/// files without checking that they still compile.
const VALIDATE_BEFORE_FINISH: &str = "Before you go further: you wrote or edited a `.wren` file but have not run `bialet_validate` on it since. \
A `.wren` file that does not compile takes the whole app down, so syntax is checked first. Run `bialet_validate` on the file you just changed, fix any error it reports, and only then continue.";

/// Injected when the same tool keeps failing the same way: repeating it cannot
/// help, so the model must change approach instead of burning the iteration cap.
const STUCK_NUDGE: &str = "You have now called the same tool several times in a row and it keeps failing in the same way, so repeating it cannot help. \
Stop and change approach: read the failure carefully, try a different tool (for example `bialet_docs` to look up the correct API, `grep`/`read_file` to inspect the real code, or `bialet_validate` to check syntax), \
or state in one line exactly what is blocking you. Do not issue the same call again.";

/// How many same-signature failures of one tool trigger STUCK_NUDGE.
const STUCK_THRESHOLD: u32 = 4;

/// After this many identical failures the turn is aborted: two nudges have
/// already been ignored, so more iterations cannot help.
const STUCK_STOP_THRESHOLD: u32 = 8;

/// Hard ceiling for one tool-call argument payload. Larger payloads are
/// rejected before dispatch: they are usually a sign the model is about to
/// repeat a whole file instead of editing it.
const MAX_TOOL_ARGS_BYTES: usize = 64 * 1024;

/// Ceiling for any single tool output before it enters the transcript. A test
/// that prints a whole table can otherwise overflow the context window.
const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;

/// The same call shape (arguments with numbers normalized) this many times in
/// one turn means the model is probing, not working.
const CALL_SHAPE_NUDGE_THRESHOLD: u32 = 5;

/// Abort when one call shape reaches this count, even if the results differed.
const CALL_SHAPE_STOP_THRESHOLD: u32 = 10;

/// Consecutive non-mutating calls before the "write something" nudge and the
/// hard stop. Successful file writes reset the counter.
const PROGRESS_NUDGE_THRESHOLD: u32 = 24;
const PROGRESS_STOP_THRESHOLD: u32 = 48;

const INVALID_ARGS_NUDGE: &str = "Your previous tool call did not contain valid JSON arguments. \
This usually means the payload was cut off because it was too long: nothing ran. Re-issue it with a smaller payload — \
write the file in two steps (`write_file` with the first half, then `edit_file` to append), or run a shorter snippet. \
Do not send the same oversized call again.";

const CALL_SHAPE_NUDGE: &str = "You have now issued the same call many times with only cosmetic changes (numbers differ, shape does not). \
The result will not change. Stop probing: read the relevant page with `bialet_docs`, then write the file that needs writing. \
If you are stuck, say in one line what is blocking you instead of repeating the call.";

const PROGRESS_NUDGE: &str = "You have made many tool calls without creating or editing a single file. \
Stop exploring and build: write `_app/migration.wren`, `_app/domain.wren`, `_app/template.wren` and the route files now. \
If an API is unclear, consult `bialet_docs` once and write the simplest version; the validator and the tests will tell you what is wrong.";

const BIALET_RUN_NUDGE: &str = "You are using `bialet_run` as a framework REPL. It has no request, session or BIALET_* tables, so it cannot build or verify the app. \
Stop probing: write `_app/migration.wren`, `_app/domain.wren`, `_app/template.wren` and the route files, then use `bialet_test` and the server.";

const PROGRESS_FINISH_NUDGE: &str = "You have already written files, so stop probing and close the turn. \
Run `bialet_test` once, delete any `debug*.wren` / `*probe*.wren` files you created, and call `finish` with a short summary. \
If something is still missing, say exactly what and finish with `success=false`. Do not keep running one-off snippets.";

const FINISH_AFTER_BUILD: &str = "You built files but the turn ended without calling `finish`. \
Run `bialet_test` one last time if you changed anything, delete debug/probe files, and call `finish` with a short summary. \
If the task is incomplete or you need a decision, call `finish` with `success=false` and say what is missing.";

const FINISH_WITHOUT_ARTIFACT: &str = "You are about to finish with `success=true`, but this turn did not create or edit any file and the project has no `.wren` file. \
That means the task is not built. If you still have steps to carry out, do them now. If you need an answer from the user, call `finish` again with `success=false` and put the question in the summary. \
Do not report success for work that does not exist.";

const QUESTION_IS_NOT_DONE: &str = "A question is not a finished task. If you need the user to decide something, call `finish` with `success=false` and put the question in the summary. \
If you can pick a sensible default, do that, build the app, and mention the assumption in the final summary.";

/// Shape of a call with digits erased, so `pad.count == 7993` and
/// `pad.count == 7992` count as the same probe. This is what lets the loop
/// guards see a spiral where every argument is numerically different.
fn normalized_call_signature(name: &str, arguments: &str) -> String {
    let mut out = String::with_capacity(name.len() + arguments.len() + 1);
    out.push_str(name);
    out.push('\u{1}');
    let mut in_digits = false;
    for ch in arguments.chars() {
        if ch.is_ascii_digit() {
            // Collapse the whole run: 7993 and 10000 are the same probe, and so
            // are `probe1.txt` and `probe10.txt`.
            if !in_digits {
                out.push('#');
                in_digits = true;
            }
        } else {
            in_digits = false;
            out.push(ch);
        }
    }
    out
}

/// Tools worth counting for the call-shape guard. These are the ones that
/// write code or probe the language; repeating one of them with only a changed
/// constant is not progress. File reads and tests are excluded: they are
/// cheap, finite, and legitimately repeat after the code changes.
fn counts_toward_shape_limit(name: &str) -> bool {
    matches!(
        name,
        "bialet_run" | "write_file" | "edit_file" | "delete_file"
    )
}

/// True when the tool call changes the project tree. A failed write does not
/// count: the point is progress, not intent.
fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file" | "edit_file" | "delete_file" | "create_dir"
    )
}

/// Does the workspace already contain a Wren application? Used to avoid
/// nagging a legitimate "explain this project" turn.
fn workspace_has_wren(root: &Path) -> bool {
    fn walk(dir: &Path, depth: usize) -> bool {
        if depth > 3 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }
                if walk(&path, depth + 1) {
                    return true;
                }
            } else if name.ends_with(".wren") {
                return true;
            }
        }
        false
    }
    walk(root, 0)
}

/// Files the model created while probing and usually forgets to delete.
/// Leaving them behind makes later runs slower and buries the real failures.
fn debug_probe_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > 3 || out.len() >= 20 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if matches!(name.as_str(), ".git" | ".masse" | "node_modules" | "target") {
                    continue;
                }
                walk(&path, root, depth + 1, out);
                continue;
            }
            let lower = name.to_lowercase();
            if lower.ends_with(".wren") && (lower.contains("debug") || lower.contains("probe")) {
                if let Ok(relative) = path.strip_prefix(root) {
                    out.push(relative.display().to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

fn probe_files_message(files: &[String]) -> String {
    let shown = files.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
    let extra = files.len().saturating_sub(8);
    let tail = if extra > 0 {
        format!(" (+{extra} more)")
    } else {
        String::new()
    };
    format!(
        "Before finishing, delete the temporary probe/debug files you created: {shown}{tail}. \
         They are not part of the app; only the final app files should remain. Then call `finish` again."
    )
}

fn invalid_arguments_message(tool: &str, bytes: usize) -> String {
    format!(
        "The arguments for `{tool}` were not valid JSON ({bytes} bytes received). \
         Nothing was executed. This usually means the completion was cut off because the payload was too long. \
         Retry with a smaller payload: split the file into `write_file` + `edit_file`, or run a shorter snippet. \
         Do not resend the same oversized call."
    )
}

/// Turn a known Wren/Bialet runtime error into the one-line fix. The model
/// often sees the raw error in a server log but does not connect it to the
/// language rule; this nudge does that at the moment the error appears.
fn wren_trap_hint(output: &str) -> Option<&'static str> {
    if output.contains("Internal Server Error") || output.contains("500 Internal Server Error") {
        return Some(
            "A 500 is an unhandled exception. Read the server log with `bialet_log_read` and fix \
             the stack trace before changing code or tests; the message names the class/property \
             that is missing.",
        );
    }
    if output.contains("Argument must be a string") {
        return Some(
            "A parser received null (`Num.fromString(null)`, `Date.new(null)`, ...). Guard the \
             field or stringify with a default: `Num.fromString(\"%(value || 0)\") || 0`. \
             Form fields are null when absent.",
        );
    }
    if output.contains("Expect ':' after map key") {
        return Some(
            "Inside `{{ }}` a `{` or an HTML block is parsed as a map literal. Compute the \
             HTML in a Wren variable first (`var extra = cond ? <div>...</div> : \"\"`) and \
             interpolate `{{ extra }}`.",
        );
    }
    if output.contains("Expect '(' after '%'")
        || output.contains("Expect end of string interpolation")
    {
        return Some(
            "A literal `%` in a Wren string must be `\\x25` (write \"100\\x25\"), because `%(` \
             starts interpolation. In an HTML template write `%` directly (or `&#37;`).",
        );
    }
    if output.contains("does not implement 'toString()'") {
        return Some(
            "`toString` is a getter: write `val.toString`, never `val.toString()`. \
             The canonical setter is `val.toString.trim()`.",
        );
    }
    if output.contains("does not implement 'trim'") {
        return Some("`trim` is a method: write `val.toString.trim()`, not `val.toString.trim`.");
    }
    if output.contains("=(_)") {
        return Some(
            "Missing property setter: `obj.field = x` needs an explicit \
             `field=(value) { _field = value }` in the class. Wren does not synthesize it.",
        );
    }
    if output.contains("String does not implement '<") {
        return Some(
            "Strings only support `==` / `!=`. Parse to `Date` or `Num` before using `<` or `>`.",
        );
    }
    if output.contains("Query does not implement") {
        return Some(
            "Query methods take at most three positional arguments; pass one List for four or \
             more: `.query([a,b,c,d])`, `.fetch([a,b,c,d])`, `.toNum([a,b,c,d])`.",
        );
    }
    if output.contains("Map does not implement 'get") {
        return Some("`Map` has no `get`: use `map[\"key\"]` and `map.containsKey(\"key\")`.");
    }
    if output.contains("List does not implement 'reverse'") {
        return Some("`List` has no `reverse`; build a new list with a `for` loop.");
    }
    if output.contains("MapSequence does not implement '[_]'") {
        return Some(
            "A Query result is a MapSequence, not a List: do not index it with `[0]`. \
             Iterate it with `for`, or materialize it with `.fetch`/`.to(Class)` and then index the List.",
        );
    }
    if output.contains("Null does not implement") {
        return Some(
            "A method returned null. In Wren a method produces a value only with an explicit \
             `return`; add it to every multiline method that should return a query/list. \
             Also guard a null row before calling `.isEmpty`, `.values` or `.format` on it.",
        );
    }
    None
}

/// Pick the most useful line of a failed tool output: the assertion or the
/// first compiler/runtime error, not the `exit_code` header. Used in the
/// change-of-approach nudge so the model can fix the exact code path.
fn failure_hint(output: &str) -> String {
    for line in output.lines() {
        let text = line.trim();
        if text.contains("Expected")
            || text.contains('✗')
            || text.starts_with("Query Error")
            || text.starts_with("Runtime Error")
            || text.starts_with("Compilation Error")
            || text.starts_with("Error:")
        {
            return text.chars().take(240).collect();
        }
    }
    first_line(output, 160)
}

/// Normalize a tool failure into a signature that ignores volatile details
/// (timestamps, line numbers, row ids, random ids) so a stuck loop is visible
/// even when the model varies the arguments slightly between attempts.
fn failure_signature(output: &str) -> String {
    let mut sig = String::new();
    let interesting = [
        "error",
        "✗",
        "expected",
        "failed",
        "does not implement",
        "panic",
        "cannot",
        "not found",
        "invalid",
    ];
    for line in output.lines() {
        let low = line.to_lowercase();
        if !interesting.iter().any(|m| low.contains(m)) {
            continue;
        }
        for token in low.split_whitespace() {
            // Drop long random identifiers (session ids, hashes, uuids): they
            // differ on every run and would hide the repeated failure.
            let alnum = token.chars().filter(|c| c.is_ascii_alphanumeric()).count();
            let has_digit = token.chars().any(|c| c.is_ascii_digit());
            if alnum >= 12 && has_digit {
                continue;
            }
            let cleaned: String = token.chars().filter(|c| c.is_alphabetic()).collect();
            if !cleaned.is_empty() {
                sig.push_str(&cleaned);
                sig.push(' ');
            }
        }
        if sig.len() >= 200 {
            break;
        }
    }
    if sig.trim().is_empty() {
        // Nothing matched the markers: fall back to the old letters-only pass
        // so every failure still has a stable signature.
        let mut last_space = true;
        for ch in output.chars().flat_map(|c| c.to_lowercase()) {
            if ch.is_alphabetic() {
                sig.push(ch);
                last_space = false;
            } else if !last_space {
                sig.push(' ');
                last_space = true;
            }
            if sig.len() >= 200 {
                break;
            }
        }
    }
    sig.trim().to_string()
}

/// Which verification step (if any) is still owed before the turn may end.
/// Syntax comes before the test run: a file that does not compile cannot be
/// exercised, so check it first.
fn pending_verification(
    wren_unvalidated: bool,
    syntax_nudged: bool,
    wren_changed: bool,
    verify_nudged: bool,
) -> Option<&'static str> {
    if wren_unvalidated && !syntax_nudged {
        Some(VALIDATE_BEFORE_FINISH)
    } else if wren_changed && !verify_nudged {
        Some(VERIFY_BEFORE_FINISH)
    } else {
        None
    }
}

/// How a turn ended.
enum TurnOutcome {
    /// The model stopped calling tools on its own.
    Stopped(String),
    /// The `finish` tool was called.
    Finished { summary: String, success: bool },
}

impl TurnOutcome {
    fn text(&self) -> &str {
        match self {
            TurnOutcome::Stopped(text) => text,
            TurnOutcome::Finished { summary, .. } => summary,
        }
    }
}

/// Streamed progress emitted while a turn runs.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    UserMessage(String),
    AssistantMessage(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        status: String,
        output: String,
        exit_code: Option<i32>,
    },
    /// A transient provider error is being retried.
    Retrying {
        attempt: u32,
        max: u32,
        reason: String,
    },
    /// Informational note (e.g. the iteration cap was reached).
    Notice(String),
    /// Tokens and estimated cost after a turn, plus how full the context window
    /// is. Emitted once per turn, just before the terminal event.
    Usage(UsageReport),
    /// The agent declared the task finished via the `finish` tool.
    Finished {
        summary: String,
        success: bool,
    },
    Error(String),
    /// The model stopped on its own without calling `finish`.
    TurnFinished,
}

/// Owns the conversation state and drives the provider/tool loop.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    ctx: ToolContext,
    session: Arc<SessionStore>,
    config: AgentConfig,
    messages: Vec<ChatMessage>,
    /// Whether the git repo was prepared for auto-commit: `None` = not tried,
    /// `Some(true)` = ready, `Some(false)` = setup failed.
    repo_ready: Option<bool>,
    /// Tokens accumulated over every completion in this process.
    usage_total: TokenUsage,
    /// Prompt tokens of the most recent completion (context size), and the
    /// tokens of the current turn. Reset per turn for the latter.
    context_tokens: u64,
    turn_usage: TokenUsage,
}

impl Agent {
    /// Build an agent and rehydrate the conversation from the session file.
    pub async fn new(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        ctx: ToolContext,
        session: Arc<SessionStore>,
        config: AgentConfig,
    ) -> Result<Self> {
        let events = session.read_all().await?;
        if events.is_empty() {
            session.append(&Event::session_start()).await?;
        }
        let mut messages = rebuild_messages(&events, &config.system_prompt);
        if messages.is_empty() {
            messages.push(ChatMessage::system(config.system_prompt.clone()));
        }
        Ok(Agent {
            provider,
            tools,
            ctx,
            session,
            config,
            messages,
            repo_ready: None,
            usage_total: TokenUsage::default(),
            context_tokens: 0,
            turn_usage: TokenUsage::default(),
        })
    }

    /// Run a git command in the workspace root.
    async fn git(&self, args: &[&str]) -> harness_tools::cli::CmdOutput {
        let spec = CmdSpec::new("git").args(args.iter().map(|a| a.to_string()));
        run_cmd(&spec, &self.ctx.root, Duration::from_secs(30)).await
    }

    /// Make sure the workspace is a git repo able to commit: `git init` when
    /// there is no repository, and a local identity when none is configured.
    pub async fn ensure_git_repo(&self) -> Result<()> {
        // Use the enclosing repository if there is one; otherwise make the
        // workspace its own repo.
        let inside = self.git(&["rev-parse", "--is-inside-work-tree"]).await;
        if !inside.success() || inside.stdout.trim() != "true" {
            let out = self.git(&["init", "-q"]).await;
            if !out.success() {
                return Err(anyhow::anyhow!(
                    "git init failed in {}: {}",
                    self.ctx.root.display(),
                    out.to_agent_text()
                ));
            }
        }
        let email = self.git(&["config", "user.email"]).await;
        if email.stdout.trim().is_empty() {
            let _ = self.git(&["config", "user.email", "masse@localhost"]).await;
            let _ = self.git(&["config", "user.name", "masse"]).await;
        }
        Ok(())
    }

    /// Commit the workspace after a turn. Returns a short report when something
    /// was committed, or `None` when the tree was already clean.
    pub async fn commit_turn(&self, label: &str) -> Result<Option<String>> {
        let status = self.git(&["status", "--porcelain"]).await;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "git status failed: {}",
                status.to_agent_text()
            ));
        }
        if status.stdout.trim().is_empty() {
            return Ok(None);
        }
        let add = self.git(&["add", "-A"]).await;
        if !add.success() {
            return Err(anyhow::anyhow!("git add failed: {}", add.to_agent_text()));
        }
        let message = format!("masse: {}", first_line(label, 72));
        let commit = self.git(&["commit", "-q", "-m", &message]).await;
        if !commit.success() {
            return Err(anyhow::anyhow!(
                "git commit failed: {}",
                commit.to_agent_text()
            ));
        }
        let hash = self.git(&["rev-parse", "--short", "HEAD"]).await;
        let short = hash.stdout.trim().to_string();
        Ok(Some(format!("{short} {message}")))
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn session(&self) -> &Arc<SessionStore> {
        &self.session
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Fold one completion's usage into the running totals. Context size is the
    /// prompt of the most recent call, not a sum: it is how full the window is.
    fn record_usage(&mut self, usage: TokenUsage) {
        if !usage.is_empty() {
            self.usage_total.add(usage);
            self.turn_usage.add(usage);
            self.context_tokens = usage.prompt_tokens;
        }
    }

    /// The price in effect for the current model: the command-line override where
    /// given, layered over the built-in table. `None` when neither applies.
    fn pricing(&self) -> Option<ModelPricing> {
        let table = pricing_for(&self.config.model);
        if self.config.price_input.is_none() && self.config.price_output.is_none() {
            return table;
        }
        Some(ModelPricing {
            input_per_million: self
                .config
                .price_input
                .or_else(|| table.map(|p| p.input_per_million))
                .unwrap_or(0.0),
            output_per_million: self
                .config
                .price_output
                .or_else(|| table.map(|p| p.output_per_million))
                .unwrap_or(0.0),
        })
    }

    /// Snapshot of this turn's and the session's usage for the front ends.
    fn usage_report(&self) -> UsageReport {
        let price = self.pricing();
        UsageReport {
            turn: self.turn_usage,
            total: self.usage_total,
            turn_cost_usd: price.map(|p| p.cost(self.turn_usage)),
            total_cost_usd: price.map(|p| p.cost(self.usage_total)),
            context_tokens: self.context_tokens,
            context_window: context_window_for(&self.config.model),
        }
    }

    /// Run one user turn to completion.
    ///
    /// Exactly one terminal event (`Finished`, `TurnFinished` or `Error`) is
    /// emitted on every path, so a front end can never get stuck waiting.
    pub async fn send(
        &mut self,
        text: &str,
        sink: Option<&broadcast::Sender<AgentEvent>>,
    ) -> Result<String> {
        // Prepare the repo once, so auto-commit can work.
        if self.config.auto_commit && self.repo_ready.is_none() {
            match self.ensure_git_repo().await {
                Ok(()) => self.repo_ready = Some(true),
                Err(e) => {
                    self.repo_ready = Some(false);
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("auto-commit disabled: {e}")));
                    }
                }
            }
        }

        let result = self.run_turn(text, sink).await;
        if let Some(s) = sink {
            // Report tokens and cost before the terminal event, so the front end
            // has the numbers when it settles the turn.
            let _ = s.send(AgentEvent::Usage(self.usage_report()));
            match &result {
                Ok(TurnOutcome::Stopped(_)) => {
                    let _ = s.send(AgentEvent::TurnFinished);
                }
                Ok(TurnOutcome::Finished { summary, success }) => {
                    let _ = s.send(AgentEvent::Finished {
                        summary: summary.clone(),
                        success: *success,
                    });
                }
                Err(e) => {
                    let _ = s.send(AgentEvent::Error(e.to_string()));
                }
            }
        }

        // Commit whatever the turn changed.
        if self.config.auto_commit && self.repo_ready == Some(true) {
            match self.commit_turn(text).await {
                Ok(Some(report)) => {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("committed {report}")));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!("auto-commit failed: {e}")));
                    }
                }
            }

            // Push so the deploy hook fires. A no-op push is harmless.
            if self.config.auto_push {
                let push = self.git(&["push", "origin", "HEAD"]).await;
                if !push.success() {
                    if let Some(s) = sink {
                        let _ = s.send(AgentEvent::Notice(format!(
                            "auto-push failed: {}",
                            push.to_agent_text()
                        )));
                    }
                }
            }
        }

        result.map(|outcome| outcome.text().to_string())
    }

    async fn run_turn(
        &mut self,
        text: &str,
        sink: Option<&broadcast::Sender<AgentEvent>>,
    ) -> Result<TurnOutcome> {
        macro_rules! emit {
            ($ev:expr) => {
                if let Some(s) = sink {
                    let _ = s.send($ev);
                }
            };
        }

        emit!(AgentEvent::UserMessage(text.to_string()));
        self.session.append(&Event::user_message(text)).await?;
        self.messages.push(ChatMessage::user(text));
        self.turn_usage = TokenUsage::default();

        // Definition-of-done tracking: did we change Wren code since the last
        // green test run, and have we already nudged once this turn?
        let mut wren_changed = false;
        let mut wren_unvalidated = false;
        let mut verify_nudged = false;
        let mut syntax_nudged = false;
        // Repeated-call detection: the last call signature and how many times in
        // a row it has been issued. Digits are normalized away so a loop that
        // only changes a number still looks like the same call.
        let mut last_call: Option<String> = None;
        let mut repeat_count: u32 = 0;
        // Per-turn call-shape counts and failure counts. These are deliberately
        // not consecutive: alternating edit/test or probe/probe cycles must trip
        // them too.
        let mut call_shapes: HashMap<String, u32> = HashMap::new();
        let mut nudged_shapes: HashSet<String> = HashSet::new();
        let mut failure_counts: HashMap<String, u32> = HashMap::new();
        let mut nudged_failures: HashSet<String> = HashSet::new();
        // Progress tracking: a turn that never writes a file cannot finish the
        // task, however many docs it reads.
        let mut calls_without_mutation: u32 = 0;
        let mut progress_nudged = false;
        let mut progress_finish_nudged = false;
        let mut mutated_this_turn = false;
        let mut finish_nudged = false;
        let mut cleanup_nudged = false;
        let mut invalid_args_nudged = false;
        // A growing `bialet_run` snippet (6 lines, 7, 8...) evades the
        // call-shape guard because the arguments really are different. The
        // count of consecutive runs catches that spiral.
        let mut consecutive_bialet_runs: u32 = 0;
        let mut bialet_run_nudged = false;
        // Known Wren/Bialet errors already explained this turn.
        let mut trap_nudged: HashSet<&'static str> = HashSet::new();

        for _ in 0..self.config.max_iterations {
            let request = ChatRequest {
                model: self.config.model.clone(),
                messages: self.messages.clone(),
                tools: self.tools.openai_schemas(),
                temperature: self.config.temperature,
            };
            // Retry only transient errors, a bounded number of times; a
            // permanent error (bad request, auth, malformed response) fails
            // immediately.
            let mut attempt: u32 = 0;
            let response = loop {
                match self.provider.complete(request.clone()).await {
                    Ok(response) => break response,
                    Err(e) => {
                        if e.is_retryable() && attempt < self.config.max_retries as u32 {
                            attempt += 1;
                            let delay = retry_delay(self.config.retry_delay_ms, attempt);
                            emit!(AgentEvent::Retrying {
                                attempt,
                                max: self.config.max_retries as u32,
                                reason: e.to_string(),
                            });
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            continue;
                        }
                        return Err(anyhow::anyhow!(e.to_string()));
                    }
                }
            };

            self.record_usage(response.usage);

            if !response.content.trim().is_empty() {
                emit!(AgentEvent::AssistantMessage(response.content.clone()));
                self.session
                    .append(&Event::assistant_message(&response.content))
                    .await?;
            }

            let calls = response.tool_calls.clone();
            self.messages.push(ChatMessage::assistant_with_calls(
                response.content.clone(),
                calls.clone(),
            ));

            if calls.is_empty() {
                // The model stopped on its own. If it just changed Wren code and
                // is not asking the user a question, make it check syntax and
                // verify before it can finish.
                if !looks_like_question(&response.content) {
                    if let Some(nudge) = pending_verification(
                        wren_unvalidated,
                        syntax_nudged,
                        wren_changed,
                        verify_nudged,
                    ) {
                        if nudge == VALIDATE_BEFORE_FINISH {
                            syntax_nudged = true;
                            emit!(AgentEvent::Notice(
                                "checking syntax before finishing".into()
                            ));
                        } else {
                            verify_nudged = true;
                            emit!(AgentEvent::Notice(
                                "verifying the change before finishing".into()
                            ));
                        }
                        self.messages.push(ChatMessage::user(nudge));
                        continue;
                    }
                }
                // The model built an app but went quiet without `finish`. Ask
                // once for the explicit outcome; without this the runner
                // reports `turn_finished` even when the app is complete. Do not
                // nudge a plain question: the user needs to see it.
                if !finish_nudged
                    && mutated_this_turn
                    && workspace_has_wren(&self.ctx.root)
                    && !looks_like_question(&response.content)
                {
                    finish_nudged = true;
                    emit!(AgentEvent::Notice(
                        "turn ended without finish after writing Wren files; asking for a final summary"
                            .into()
                    ));
                    self.messages.push(ChatMessage::user(FINISH_AFTER_BUILD));
                    continue;
                }
                return Ok(TurnOutcome::Stopped(response.content));
            }

            let mut nudge_after_batch: Option<String> = None;
            for call in calls {
                // Break tool-call loops: the exact same call three times in a
                // row cannot produce a new result. The *shape* guard below is
                // the one that normalizes numbers, so a probe that only changes
                // a constant is still caught.
                let call_signature = format!("{}\u{1}{}", call.name, call.arguments);
                if last_call.as_deref() == Some(call_signature.as_str()) {
                    repeat_count += 1;
                } else {
                    repeat_count = 1;
                    last_call = Some(call_signature);
                }
                let shape_signature = normalized_call_signature(&call.name, &call.arguments);
                let shape_count = if counts_toward_shape_limit(&call.name) {
                    let count = call_shapes.entry(shape_signature.clone()).or_insert(0);
                    *count += 1;
                    *count
                } else {
                    0
                };

                // A truncated completion produces invalid JSON. Do not turn that
                // into a misleading "missing required argument": nothing ran.
                let parsed = serde_json::from_str::<serde_json::Value>(&call.arguments).ok();
                let arguments_ok = parsed.is_some();
                let args_value = parsed.unwrap_or_else(|| serde_json::json!({}));
                let args_encoded = if arguments_ok {
                    encode_json_args(&args_value)
                } else {
                    format!("_invalid_json=true&raw_len={}", call.arguments.len())
                };
                let changed_wren = arguments_ok && mutates_wren(&call.name, &args_value);

                emit!(AgentEvent::ToolCall {
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                });
                self.session
                    .append(&Event::tool_call(&call.name, &args_encoded))
                    .await?;

                let result = if !arguments_ok {
                    if !invalid_args_nudged {
                        invalid_args_nudged = true;
                        nudge_after_batch.get_or_insert_with(|| INVALID_ARGS_NUDGE.to_string());
                    }
                    ToolResult::err(invalid_arguments_message(&call.name, call.arguments.len()))
                } else if call.arguments.len() > MAX_TOOL_ARGS_BYTES {
                    ToolResult::err(format!(
                        "The arguments for `{}` are {} bytes, above the {MAX_TOOL_ARGS_BYTES}-byte limit. \
                         Nothing was executed. Split the work into smaller calls (`write_file` for the first part, then `edit_file`).",
                        call.name,
                        call.arguments.len()
                    ))
                } else if repeat_count >= 3 {
                    emit!(AgentEvent::Notice(format!(
                        "skipping `{}`: same call repeated {repeat_count} times",
                        call.name
                    )));
                    ToolResult::err(format!(
                        "skipped by the harness: `{}` was already called {repeat_count} times in a row with the same arguments, so the result cannot change. Do not repeat it — change your approach, or explain to the user what is blocking you.",
                        call.name
                    ))
                } else {
                    self.tools.call(&call.name, &self.ctx, args_value).await
                };
                let mut result = result;
                if result.output.len() > MAX_TOOL_OUTPUT_BYTES {
                    result.output = cap_bytes(&result.output, MAX_TOOL_OUTPUT_BYTES);
                }
                let failure_hint = if result.status == ToolStatus::Ok {
                    String::new()
                } else {
                    failure_hint(&result.output)
                };
                if call.name == "bialet_run" {
                    consecutive_bialet_runs += 1;
                } else if result.status == ToolStatus::Ok
                    && (is_mutating_tool(&call.name) || call.name == "bialet_test")
                {
                    consecutive_bialet_runs = 0;
                }
                if let Some(hint) = wren_trap_hint(&result.output) {
                    if trap_nudged.insert(hint) {
                        nudge_after_batch.get_or_insert_with(|| {
                            format!("{hint}\nThe output was:\n{failure_hint}")
                        });
                        emit!(AgentEvent::Notice(
                            "detected a known Wren/Bialet trap; adding a targeted hint".into()
                        ));
                    }
                }
                let terminate = result.terminate.clone();
                let session_status = if result.status == ToolStatus::Ok {
                    SessionStatus::Ok
                } else {
                    SessionStatus::Error
                };
                self.session
                    .append(&Event::tool_result(
                        &call.name,
                        &args_encoded,
                        session_status,
                        &result.output,
                        result.exit_code,
                    ))
                    .await?;

                emit!(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    status: result.status.as_str().to_string(),
                    output: result.output.clone(),
                    exit_code: result.exit_code,
                });

                // Compute the failure signature before `result.output` is moved
                // into the transcript below.
                let fail_sig = if result.status == ToolStatus::Ok {
                    None
                } else {
                    Some(format!(
                        "{}\u{1}{}",
                        call.name,
                        failure_signature(&result.output)
                    ))
                };

                // Progress: only a successful file mutation counts as work.
                if result.status == ToolStatus::Ok && is_mutating_tool(&call.name) {
                    mutated_this_turn = true;
                    calls_without_mutation = 0;
                } else {
                    calls_without_mutation += 1;
                }
                if arguments_ok {
                    invalid_args_nudged = false;
                }

                self.messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    call.name.clone(),
                    result.output,
                ));

                if changed_wren {
                    wren_changed = true;
                    wren_unvalidated = true;
                }
                if call.name == "bialet_validate" && result.status == ToolStatus::Ok {
                    wren_unvalidated = false;
                }
                if call.name == "bialet_test" && result.status == ToolStatus::Ok {
                    wren_changed = false;
                    wren_unvalidated = false;
                }

                // Stuck-loop detection: the same tool failing the same way over
                // and over cannot make progress. The count is per turn and does
                // not reset on unrelated successes, so edit/test cycles trip it.
                if let Some(sig) = fail_sig {
                    let count = {
                        let c = failure_counts.entry(sig.clone()).or_insert(0);
                        *c += 1;
                        *c
                    };
                    if count >= STUCK_STOP_THRESHOLD {
                        let reason = format!(
                            "Turn stopped: `{}` failed the same way {count} times (last error: {}). \
                             Repeating it cannot help, so the harness aborted the turn instead of burning the remaining iterations. \
                             The files written so far are kept.",
                            call.name, failure_hint
                        );
                        emit!(AgentEvent::AssistantMessage(reason.clone()));
                        self.session
                            .append(&Event::assistant_message(&reason))
                            .await?;
                        return Ok(TurnOutcome::Stopped(reason));
                    }
                    if count >= STUCK_THRESHOLD && nudged_failures.insert(sig) {
                        nudge_after_batch.get_or_insert_with(|| {
                            format!(
                                "{STUCK_NUDGE}\nThe last failure was:\n{failure_hint}\n\
                                 Change the code path that produces that error; do not rerun the same test until you have changed it."
                            )
                        });
                        emit!(AgentEvent::Notice(format!(
                            "`{}` failed the same way {count} times; asking the model to change approach",
                            call.name
                        )));
                    }
                }

                // A `bialet_run` snippet that grows line by line is a REPL
                // spiral: every argument is different, so the shape guard
                // cannot see it. Count consecutive runs instead.
                if consecutive_bialet_runs >= 24 {
                    let reason = format!(
                        "Turn stopped: {consecutive_bialet_runs} consecutive `bialet_run` calls without writing a file. \
                         `bialet_run` has no request/session and cannot build the app; the turn is being aborted so it does not burn the budget. \
                         The files written so far are kept."
                    );
                    emit!(AgentEvent::AssistantMessage(reason.clone()));
                    self.session
                        .append(&Event::assistant_message(&reason))
                        .await?;
                    return Ok(TurnOutcome::Stopped(reason));
                }
                if consecutive_bialet_runs >= 12 && !bialet_run_nudged {
                    bialet_run_nudged = true;
                    nudge_after_batch.get_or_insert_with(|| BIALET_RUN_NUDGE.to_string());
                    emit!(AgentEvent::Notice(format!(
                        "{consecutive_bialet_runs} consecutive bialet_run calls; asking the model to write files"
                    )));
                }

                // A call shape repeated with only cosmetic changes is a probe,
                // not work. Nudge once, then abort if it continues.
                if shape_count >= CALL_SHAPE_STOP_THRESHOLD {
                    let reason = format!(
                        "Turn stopped: `{}` was called {shape_count} times with the same arguments (ignoring numbers). \
                         This is a probe loop, not progress; the remaining iterations would be wasted. \
                         Read `bialet-pitfalls.md` and `bialet-recipes.md` before the next attempt.",
                        call.name
                    );
                    emit!(AgentEvent::AssistantMessage(reason.clone()));
                    self.session
                        .append(&Event::assistant_message(&reason))
                        .await?;
                    return Ok(TurnOutcome::Stopped(reason));
                }
                if shape_count >= CALL_SHAPE_NUDGE_THRESHOLD
                    && nudged_shapes.insert(shape_signature)
                {
                    nudge_after_batch.get_or_insert_with(|| CALL_SHAPE_NUDGE.to_string());
                    emit!(AgentEvent::Notice(format!(
                        "`{}` repeated the same call shape {shape_count} times; asking the model to change approach",
                        call.name
                    )));
                }

                if calls_without_mutation >= PROGRESS_STOP_THRESHOLD {
                    if mutated_this_turn && !progress_finish_nudged {
                        // The app exists; the model is over-verifying. Ask it
                        // to close the turn instead of throwing the work away.
                        progress_finish_nudged = true;
                        calls_without_mutation = 0;
                        nudge_after_batch.get_or_insert_with(|| PROGRESS_FINISH_NUDGE.to_string());
                        emit!(AgentEvent::Notice(
                            "app files exist but the model keeps probing; asking it to test once and finish"
                                .into()
                        ));
                    } else {
                        let reason = format!(
                            "Turn stopped: {calls_without_mutation} tool calls in a row without creating or editing a file. \
                             The model is stuck exploring instead of building; the files written so far are kept."
                        );
                        emit!(AgentEvent::AssistantMessage(reason.clone()));
                        self.session
                            .append(&Event::assistant_message(&reason))
                            .await?;
                        return Ok(TurnOutcome::Stopped(reason));
                    }
                }
                if calls_without_mutation >= PROGRESS_NUDGE_THRESHOLD && !progress_nudged {
                    progress_nudged = true;
                    nudge_after_batch.get_or_insert_with(|| PROGRESS_NUDGE.to_string());
                    emit!(AgentEvent::Notice(
                        "many read-only calls without a file write; asking the model to build"
                            .into()
                    ));
                }

                // The `finish` tool ends the turn with an explicit outcome, but
                // not before syntax is checked and the change is verified.
                if let Some(t) = terminate {
                    if let Some(nudge) = pending_verification(
                        wren_unvalidated,
                        syntax_nudged,
                        wren_changed,
                        verify_nudged,
                    ) {
                        if nudge == VALIDATE_BEFORE_FINISH {
                            syntax_nudged = true;
                            emit!(AgentEvent::Notice(
                                "checking syntax before finishing".into()
                            ));
                        } else {
                            verify_nudged = true;
                            emit!(AgentEvent::Notice(
                                "verifying the change before finishing".into()
                            ));
                        }
                        self.messages.push(ChatMessage::user(nudge));
                        break;
                    }
                    // A "success" that created nothing is not a success. Nudge
                    // once; the model may still insist (a legitimate Q&A turn).
                    if t.success && !finish_nudged {
                        if !mutated_this_turn && !workspace_has_wren(&self.ctx.root) {
                            finish_nudged = true;
                            nudge_after_batch
                                .get_or_insert_with(|| FINISH_WITHOUT_ARTIFACT.to_string());
                            emit!(AgentEvent::Notice(
                                "finish(success) rejected: no file was created this turn".into()
                            ));
                            break;
                        }
                        if looks_like_question(&t.summary) {
                            finish_nudged = true;
                            nudge_after_batch
                                .get_or_insert_with(|| QUESTION_IS_NOT_DONE.to_string());
                            emit!(AgentEvent::Notice(
                                "finish(success) rejected: the summary is a question".into()
                            ));
                            break;
                        }
                    }
                    // Probe files left behind make later runs slower and hide
                    // the real failures. Ask once for them to be deleted.
                    if t.success && !cleanup_nudged {
                        let probes = debug_probe_files(&self.ctx.root);
                        if !probes.is_empty() {
                            cleanup_nudged = true;
                            emit!(AgentEvent::Notice(format!(
                                "finish(success) rejected: {} probe file(s) left behind",
                                probes.len()
                            )));
                            self.messages
                                .push(ChatMessage::user(probe_files_message(&probes)));
                            break;
                        }
                    }
                    return Ok(TurnOutcome::Finished {
                        summary: t.summary,
                        success: t.success,
                    });
                }
            }

            if let Some(nudge) = nudge_after_batch {
                self.messages.push(ChatMessage::user(nudge));
                continue;
            }
        }

        // We reached the iteration cap. Rather than failing the turn, ask the
        // model once more with tools disabled so it produces a real answer.
        emit!(AgentEvent::Notice(format!(
            "tool-call iteration limit ({}) reached; requesting a final answer without tools",
            self.config.max_iterations
        )));
        let mut wrap_messages = self.messages.clone();
        wrap_messages.push(ChatMessage::user(
            "You have reached the maximum number of tool calls for this turn. \
             Do not call any more tools. Reply with a concise summary of what you \
             did and what remains to be done. Be honest about state: if the tests \
             are not green, if something is unverified, or if a step is incomplete, \
             say so plainly — do not claim the task is finished when it is not.",
        ));
        let wrap_request = ChatRequest {
            model: self.config.model.clone(),
            messages: wrap_messages,
            tools: Vec::new(),
            temperature: self.config.temperature,
        };
        match self.provider.complete(wrap_request).await {
            Ok(response) => {
                self.record_usage(response.usage);
                if !response.content.trim().is_empty() {
                    emit!(AgentEvent::AssistantMessage(response.content.clone()));
                    self.session
                        .append(&Event::assistant_message(&response.content))
                        .await?;
                    self.messages.push(ChatMessage::assistant(response.content.clone()));
                }
                Ok(TurnOutcome::Stopped(response.content))
            }
            Err(e) => Err(anyhow::anyhow!(
                "agent reached the tool-call iteration limit ({}) and the wrap-up request failed: {e}",
                self.config.max_iterations
            )),
        }
    }
}

/// Reconstruct a provider-ready conversation from session events.
///
/// The CSV does not store OpenAI `tool_call_id`s, so tool results are replayed
/// as user-role notes. This keeps the transcript valid without losing content.
pub fn rebuild_messages(events: &[Event], system_prompt: &str) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(system_prompt)];
    let mut pending_tool: Option<String> = None;
    for ev in events {
        match ev.event_type.as_str() {
            "user_message" => messages.push(ChatMessage::user(ev.output.clone())),
            "assistant_message" => {
                if !ev.output.trim().is_empty() {
                    messages.push(ChatMessage::assistant(ev.output.clone()));
                }
            }
            "tool_call" => pending_tool = Some(ev.tool.clone()),
            "tool_result" => {
                let name = if ev.tool.is_empty() {
                    pending_tool.take().unwrap_or_default()
                } else {
                    ev.tool.clone()
                };
                let note = format!("[tool {name} → {}]\n{}", ev.status, ev.output);
                messages.push(ChatMessage::user(note));
            }
            _ => {}
        }
    }
    messages
}

/// Commands the actor accepts.
enum Command {
    User(String),
    Cancel,
    Shutdown,
}

/// A cheap, cloneable handle to a running [`Agent`].
///
/// The actor pattern keeps the single-threaded conversation invariant while
/// exposing a `Send` handle to the TUI and the HTTP server.
#[derive(Clone)]
pub struct AgentHandle {
    tx: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<AgentEvent>,
}

impl AgentHandle {
    pub fn spawn(mut agent: Agent) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
        let (events, _) = broadcast::channel::<AgentEvent>(2048);
        let emitter = events.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::User(text) => {
                        // Guard against panics so the actor never dies silently
                        // and leaves a front end stuck on "thinking".
                        let fut = std::panic::AssertUnwindSafe(agent.send(&text, Some(&emitter)))
                            .catch_unwind();
                        tokio::pin!(fut);
                        let mut shutdown = false;
                        loop {
                            tokio::select! {
                                outcome = &mut fut => {
                                    if outcome.is_err() {
                                        let _ = emitter.send(AgentEvent::Error(
                                            "internal error: agent task panicked; turn aborted".into(),
                                        ));
                                    }
                                    break;
                                }
                                queued = rx.recv() => {
                                    match queued {
                                        Some(Command::Cancel) => {
                                            // Dropping `fut` cancels the in-flight turn.
                                            let _ = emitter.send(AgentEvent::Error(
                                                "turn cancelled".into(),
                                            ));
                                            break;
                                        }
                                        Some(Command::Shutdown) | None => {
                                            shutdown = true;
                                            break;
                                        }
                                        // Ignore new messages while busy.
                                        Some(Command::User(_)) => {}
                                    }
                                }
                            }
                        }
                        if shutdown {
                            break;
                        }
                    }
                    Command::Cancel => {
                        let _ = emitter.send(AgentEvent::Error("nothing to cancel".into()));
                    }
                    Command::Shutdown => break,
                }
            }
        });
        AgentHandle { tx, events }
    }

    /// Queue a user message. Returns false if the actor has stopped.
    pub fn send_user(&self, text: impl Into<String>) -> bool {
        self.tx.send(Command::User(text.into())).is_ok()
    }

    /// Ask the actor to abort the in-flight turn, if any.
    pub fn cancel(&self) -> bool {
        self.tx.send(Command::Cancel).is_ok()
    }

    /// Subscribe to the event stream. Each subscriber sees every future event.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_signature_ignores_volatile_details() {
        // Timestamps, line numbers and ids differ between attempts; the failure
        // is the same one.
        let a = "2026-10-16 17:54:43 Runtime Error Could not load module 'bialet_cli/bialet/test'.";
        let b = "2026-10-16 17:55:02 Runtime Error Could not load module 'bialet_cli/bialet/test'.";
        assert_eq!(failure_signature(a), failure_signature(b));

        // A genuinely different failure must not collapse into the same bucket.
        let c = "Query does not implement 'fetch(_,_,_,_,_)'.";
        assert_ne!(failure_signature(a), failure_signature(c));
    }

    #[test]
    fn known_wren_traps_get_a_targeted_hint() {
        assert!(
            wren_trap_hint("Runtime Error Null does not implement 'isEmpty'.")
                .unwrap()
                .contains("return")
        );
        assert!(wren_trap_hint("String does not implement 'toString()'.")
            .unwrap()
            .contains("getter"));
        assert!(wren_trap_hint("String does not implement 'trim'.")
            .unwrap()
            .contains("trim()"));
        assert!(wren_trap_hint("Order does not implement 'code=(_)'.")
            .unwrap()
            .contains("setter"));
        assert!(wren_trap_hint("String does not implement '<(_)'.")
            .unwrap()
            .contains("Date"));
        assert!(wren_trap_hint("Map does not implement 'get(_)'.")
            .unwrap()
            .contains("map["));
        assert!(wren_trap_hint("Query does not implement 'query(_,_,_,_)'.")
            .unwrap()
            .contains("List"));
        assert!(
            wren_trap_hint("Compilation Error line 2 Error: Expect '(' after '%'.")
                .unwrap()
                .contains("x25")
        );
        assert!(wren_trap_hint("HTTP/1.1 500 Internal Server Error")
            .unwrap()
            .contains("log"));
        assert!(wren_trap_hint("List does not implement 'reverse'.")
            .unwrap()
            .contains("for"));
        assert!(wren_trap_hint("todo bien").is_none());
    }

    #[test]
    fn pending_verification_prefers_syntax_then_tests() {
        // Unvalidated Wren asks for the syntax check first.
        assert_eq!(
            pending_verification(true, false, true, false),
            Some(VALIDATE_BEFORE_FINISH)
        );
        // Syntax already nudged: the next step is the test run.
        assert_eq!(
            pending_verification(true, true, true, false),
            Some(VERIFY_BEFORE_FINISH)
        );
        // Both nudged: nothing owed.
        assert_eq!(pending_verification(true, true, true, true), None);
        // No Wren change: nothing to verify.
        assert_eq!(pending_verification(false, false, false, false), None);
    }

    #[test]
    fn call_shape_ignores_numbers_but_not_structure() {
        let a = normalized_call_signature("bialet_run", r#"{"code":"pad.count == 7993"}"#);
        let b = normalized_call_signature("bialet_run", r#"{"code":"pad.count == 7992"}"#);
        assert_eq!(a, b, "a changed constant is the same probe");

        let c = normalized_call_signature("bialet_run", r#"{"code":"System.print(Date.new())"}"#);
        assert_ne!(a, c, "a different snippet is a different call");
    }

    #[test]
    fn failure_signature_ignores_random_session_ids() {
        let a = "✗ lista.wren (40ms)\nExpected body to contain \"x\"\nrows=[{key: who, id: oa9jvaiLKlHSJi9YgAGzqa5uE70I3lDwpnK8cqBQ}]";
        let b = "✗ lista.wren (37ms)\nExpected body to contain \"x\"\nrows=[{key: who, id: U9668fRw8Xm4DkciG6ExGyyvmuEFwB2BTKDwTNRT}]";
        assert_eq!(
            failure_signature(a),
            failure_signature(b),
            "random ids must not hide a repeated failure"
        );
        assert!(failure_signature(a).contains("expected"));
    }

    #[test]
    fn workspace_detection_ignores_vendor_dirs() {
        let dir = std::env::temp_dir().join(format!("masse-has-wren-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules/x.wren"), "// vendor").unwrap();
        assert!(!workspace_has_wren(&dir));

        std::fs::create_dir_all(dir.join("_app")).unwrap();
        std::fs::write(dir.join("_app/domain.wren"), "class A {}").unwrap();
        assert!(workspace_has_wren(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_files_are_detected_and_reported() {
        let dir = std::env::temp_dir().join(format!("masse-probes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("_tests")).unwrap();
        std::fs::create_dir_all(dir.join("_app")).unwrap();
        std::fs::create_dir_all(dir.join(".masse")).unwrap();
        std::fs::write(dir.join("debug1.wren"), "// probe").unwrap();
        std::fs::write(dir.join("_tests/debug2.wren"), "// probe").unwrap();
        std::fs::write(dir.join("_tests/zz-debug.wren"), "// probe").unwrap();
        std::fs::write(dir.join("csrfprobe.wren"), "// probe").unwrap();
        std::fs::write(dir.join("app.wren"), "// app").unwrap();
        std::fs::write(dir.join("_app/domain.wren"), "class A {}").unwrap();
        std::fs::write(dir.join(".masse/debug3.wren"), "// runtime").unwrap();

        let found = debug_probe_files(&dir);
        assert_eq!(
            found,
            vec![
                "_tests/debug2.wren".to_string(),
                "_tests/zz-debug.wren".to_string(),
                "csrfprobe.wren".to_string(),
                "debug1.wren".to_string(),
            ],
            "only app-level probe files should be reported: {found:?}"
        );
        let message = probe_files_message(&found);
        assert!(message.contains("csrfprobe.wren"));
        assert!(message.contains("delete"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
