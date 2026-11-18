//! Derive a Bialet friction log from session events.
//!
//! The log answers one question: where did the model actually trip while
//! building a Bialet app? It is *derived* from `session.csv` on every write,
//! never appended to, so it can be regenerated at any time and can never drift
//! from the record. That keeps `session.csv` the only source of truth.
//!
//! Only Bialet tool failures are collected (`bialet_validate`, `bialet_test`,
//! `bialet_docs`, `bialet_run`, the server tools and the log reader). Errors
//! from unrelated tools — curl, sqlite, git — belong to the project, not to the
//! framework, and would drown the signal.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use uuid::Uuid;

use crate::event::{decode_args, Event, EventType, Status};

/// Tools whose failures say something about Bialet itself.
pub const BIALET_TOOLS: [&str; 7] = [
    "bialet_run",
    "bialet_validate",
    "bialet_test",
    "bialet_server_start",
    "bialet_server_stop",
    "bialet_log_read",
    "bialet_docs",
];

/// Tools whose `path` argument means a file was written or edited.
const MUTATING_FILE_TOOLS: [&str; 3] = ["write_file", "edit_file", "delete_file"];

/// How much of an error excerpt survives into the report.
const ERROR_MAX_CHARS: usize = 700;
/// Content lines kept before the char cap. Wrapper lines do not count.
const ERROR_MAX_LINES: usize = 6;
/// Leading lines kept for context, e.g. the migration log before a test run.
const EXCERPT_HEAD_LINES: usize = 2;
const CONTEXT_MAX_CHARS: usize = 120;
const HEADING_MAX_CHARS: usize = 100;

/// Terms that mark a real diagnostic rather than harness boilerplate.
///
/// The heading, the excerpt and the signature all use this list, so the line
/// that names an entry is also the line that keeps it distinct from another
/// failure and the line that survives the excerpt budget.
const DIAGNOSTIC_MARKERS: [&str; 17] = [
    "compilation error",
    "runtime error",
    "stack error",
    "error at",
    "✗",
    "fail",
    "does not implement",
    "panic",
    "security error",
    "syntax errors found",
    "no documentation matched",
    "exited immediately",
    "log file not found",
    "cannot",
    "not found",
    "invalid",
    "expected",
];

/// The kind of friction, used only to group the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Syntax,
    Tests,
    Runtime,
    Docs,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Syntax => "syntax",
            Category::Tests => "tests",
            Category::Runtime => "runtime",
            Category::Docs => "docs",
            Category::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "syntax" | "validate" | "validation" => Some(Category::Syntax),
            "test" | "tests" => Some(Category::Tests),
            "runtime" | "run" | "server" => Some(Category::Runtime),
            "docs" | "doc" | "documentation" => Some(Category::Docs),
            "other" => Some(Category::Other),
            _ => None,
        }
    }

    fn of(tool: &str) -> Self {
        match tool {
            "bialet_validate" => Category::Syntax,
            "bialet_test" => Category::Tests,
            "bialet_docs" => Category::Docs,
            "bialet_run" | "bialet_server_start" | "bialet_server_stop" | "bialet_log_read" => {
                Category::Runtime
            }
            _ => Category::Other,
        }
    }

    /// Render order: the most actionable categories first.
    fn rank(self) -> u8 {
        match self {
            Category::Syntax => 0,
            Category::Tests => 1,
            Category::Runtime => 2,
            Category::Docs => 3,
            Category::Other => 4,
        }
    }
}

/// One failing call, before identical failures are collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub tool: String,
    pub category: Category,
    pub error: String,
    pub context: String,
    pub resolution: Option<Resolution>,
    pub session: String,
    pub timestamp: String,
}

/// What eventually unblocked a failure, when the log shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The tool that finally succeeded, always the same tool as the failure.
    pub tool: String,
    /// Tool results between the failure and that success, including it.
    pub calls: usize,
    pub seconds: u64,
    /// Files written or edited in between, in order.
    pub edits: Vec<String>,
}

/// One distinct failure, collapsed across sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tool: String,
    pub category: Category,
    pub signature: String,
    pub error: String,
    pub occurrences: usize,
    pub sessions: usize,
    pub first_seen: String,
    pub last_seen: String,
    pub context: String,
    pub resolution: Option<Resolution>,
}

/// Everything derived from a set of session logs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub entries: Vec<Entry>,
    /// Distinct sessions that contributed at least one failure.
    pub sessions: usize,
    /// Sessions that were read, even if they had no friction.
    pub scanned: usize,
    pub occurrences: usize,
    /// Model ids recorded in `session_start` args, newest logs first.
    pub models: Vec<String>,
    pub generated: String,
}

impl Report {
    /// Scan and deduplicate every session in `sessions`.
    ///
    /// `since` keeps occurrences first seen on or after that date; `category`
    /// keeps one category. Both filter before merging so the counts in the
    /// header describe what the report actually shows.
    pub fn build(
        sessions: &[(String, Vec<Event>)],
        root: &Path,
        since: Option<NaiveDate>,
        category: Option<Category>,
    ) -> Self {
        let mut groups: HashMap<String, Group> = HashMap::new();
        let mut contributing: BTreeSet<&str> = BTreeSet::new();
        let mut models: Vec<String> = Vec::new();

        for (id, events) in sessions {
            for model in models_in(events) {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
            for occurrence in scan_session(id, events, root) {
                if category.is_some_and(|c| c != occurrence.category) {
                    continue;
                }
                if since.is_some_and(|date| {
                    date_of(&occurrence.timestamp).is_some_and(|seen| seen < date)
                }) {
                    continue;
                }
                contributing.insert(id.as_str());
                let key = dedup_key(&occurrence.tool, &occurrence.error);
                let group = groups.entry(key).or_insert_with(|| Group::new(&occurrence));
                group.add(&occurrence);
            }
        }

        let mut entries: Vec<Entry> = groups.into_values().map(Group::finish).collect();
        entries.sort_by(|a, b| {
            a.category
                .rank()
                .cmp(&b.category.rank())
                .then_with(|| b.occurrences.cmp(&a.occurrences))
                .then_with(|| b.last_seen.cmp(&a.last_seen))
                .then_with(|| a.signature.cmp(&b.signature))
        });

        Report {
            occurrences: entries.iter().map(|e| e.occurrences).sum(),
            sessions: contributing.len(),
            scanned: sessions.len(),
            entries,
            models,
            generated: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }

    /// The human-readable log, ready to write to `friction.md` or export.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# Bialet friction log\n\n");

        let model = if self.models.is_empty() {
            "model unknown".to_string()
        } else {
            format!("model {}", self.models.join(", "))
        };
        out.push_str(&format!(
            "Generated by masse {} · {model} · {}\n\n",
            env!("CARGO_PKG_VERSION"),
            self.generated
        ));

        if self.entries.is_empty() {
            out.push_str(&format!(
                "No Bialet friction recorded in {}.\n",
                plural(self.scanned, "session")
            ));
            return out;
        }

        out.push_str(&format!(
            "{} · {} · {}\n",
            plural(self.sessions, "session"),
            plural(self.occurrences, "occurrence"),
            plural(self.entries.len(), "distinct signature"),
        ));

        let mut current: Option<Category> = None;
        for entry in &self.entries {
            if current != Some(entry.category) {
                current = Some(entry.category);
                out.push_str(&format!("\n## {}\n", entry.category.as_str()));
            }
            out.push('\n');
            out.push_str(&format!("### {}\n", heading(&entry.error)));
            out.push_str(&format!(
                "`{}` · {} · first {} · last {}\n",
                entry.tool,
                plural(entry.occurrences, "occurrence"),
                entry.first_seen,
                entry.last_seen,
            ));
            if !entry.context.is_empty() {
                out.push_str(&format!("- doing: \"{}\"\n", entry.context));
            }
            match &entry.resolution {
                Some(r) => out.push_str(&format!("- unblocked by: {}\n", describe(r))),
                None => out.push_str("- unblocked by: — (unresolved)\n"),
            }
            let body: Vec<&str> = entry.error.lines().skip(1).collect();
            if body.iter().any(|line| !line.trim().is_empty()) {
                out.push_str("\n```\n");
                out.push_str(body.join("\n").trim());
                out.push_str("\n```\n");
            }
        }
        out
    }
}

/// Write the report atomically: a reader never sees a half-written file.
pub async fn write_report(path: &Path, report: &Report) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    // A unique temp name lets the turn-end and shutdown writers race safely:
    // both produce valid content and the last rename wins.
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    tokio::fs::write(&tmp, report.render()).await?;
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

/// Collect the failures in one session, in log order.
pub fn scan_session(id: &str, events: &[Event], root: &Path) -> Vec<Occurrence> {
    let mut out = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if !is_friction(event) {
            continue;
        }
        out.push(Occurrence {
            tool: event.tool.clone(),
            category: Category::of(&event.tool),
            error: excerpt(&event.output, root),
            context: context_before(events, index),
            resolution: resolution_after(events, index),
            session: id.to_string(),
            timestamp: event.timestamp.clone(),
        });
    }
    out
}

/// A failure worth keeping: a Bialet tool that errored, or a docs lookup that
/// found nothing (which still returns `ok`).
fn is_friction(event: &Event) -> bool {
    if event.kind() != Some(EventType::ToolResult) {
        return false;
    }
    if !BIALET_TOOLS.contains(&event.tool.as_str()) {
        return false;
    }
    // Stopping a server that is not running is harness state, not Bialet.
    if event.tool == "bialet_server_stop"
        && event.output.to_lowercase().contains("no running process")
    {
        return false;
    }
    match event.status_kind() {
        Some(Status::Error) => true,
        Some(Status::Ok) => {
            event.tool == "bialet_docs"
                && event
                    .output
                    .trim_start()
                    .to_lowercase()
                    .starts_with("no documentation matched")
        }
        None => false,
    }
}

/// The user message that set up the work the model was doing when it failed.
fn context_before(events: &[Event], index: usize) -> String {
    events[..index]
        .iter()
        .rev()
        .find(|e| e.kind() == Some(EventType::UserMessage))
        .map(|e| truncate(&collapse(&e.output), CONTEXT_MAX_CHARS))
        .unwrap_or_default()
}

/// The next success of the same tool *on the same target*, plus the edits made
/// on the way there.
///
/// A success on a different target is not a resolution: validating another file
/// says nothing about the broken one, and the log would credit an unrelated
/// call. Tools with no target argument keep the older same-tool rule.
fn resolution_after(events: &[Event], index: usize) -> Option<Resolution> {
    let tool = &events[index].tool;
    let target = target_of(tool, &events[index].args);
    let success = events[index + 1..].iter().position(|e| {
        e.kind() == Some(EventType::ToolResult)
            && &e.tool == tool
            && e.status_kind() == Some(Status::Ok)
            && match &target {
                Some(want) => target_of(tool, &e.args).as_deref() == Some(want.as_str()),
                None => true,
            }
    })? + index
        + 1;

    let calls = events[index + 1..=success]
        .iter()
        .filter(|e| e.kind() == Some(EventType::ToolResult))
        .count();
    let seconds = seconds_between(&events[index].timestamp, &events[success].timestamp);

    let mut edits: Vec<String> = Vec::new();
    for event in &events[index + 1..success] {
        if event.kind() != Some(EventType::ToolCall)
            || !MUTATING_FILE_TOOLS.contains(&event.tool.as_str())
        {
            continue;
        }
        let path = decode_args(&event.args)
            .into_iter()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value);
        if let Some(path) = path {
            if !edits.contains(&path) {
                edits.push(path);
            }
        }
    }

    Some(Resolution {
        tool: tool.clone(),
        calls,
        seconds,
        edits,
    })
}

/// The argument that names what a tool checked, when there is exactly one.
///
/// Only `bialet_validate` has a single target today (`file=`); the rest either
/// have none or take several arguments that all describe the same run.
fn target_of(tool: &str, args: &str) -> Option<String> {
    let key = match tool {
        "bialet_validate" => "file",
        _ => return None,
    };
    decode_args(args)
        .into_iter()
        .find(|(name, value)| name == key && !value.trim().is_empty())
        .map(|(_, value)| value)
}

fn models_in(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind() == Some(EventType::SessionStart))
        .filter_map(|e| {
            decode_args(&e.args)
                .into_iter()
                .find(|(key, _)| key == "model")
                .map(|(_, value)| value)
        })
        .filter(|value| !value.trim().is_empty())
        .collect()
}

/// Normalize a tool failure into a signature that ignores volatile details
/// (timestamps, line numbers, row ids, random ids) so the same failure collapses
/// even when the model varies the arguments slightly between attempts.
pub fn signature(output: &str) -> String {
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

struct Group {
    entry: Entry,
    sessions: BTreeSet<String>,
}

impl Group {
    fn new(occurrence: &Occurrence) -> Self {
        Group {
            entry: Entry {
                tool: occurrence.tool.clone(),
                category: occurrence.category,
                signature: signature(&occurrence.error),
                error: occurrence.error.clone(),
                occurrences: 0,
                sessions: 0,
                first_seen: String::new(),
                last_seen: String::new(),
                context: String::new(),
                resolution: None,
            },
            sessions: BTreeSet::new(),
        }
    }

    fn add(&mut self, occurrence: &Occurrence) {
        self.entry.occurrences += 1;
        self.sessions.insert(occurrence.session.clone());
        // The earliest occurrence is the representative sample, so the heading
        // and the "doing" line always agree with "first seen" even though the
        // sessions arrive newest-first.
        if self.entry.first_seen.is_empty() || occurrence.timestamp < self.entry.first_seen {
            self.entry.first_seen = occurrence.timestamp.clone();
            self.entry.error = occurrence.error.clone();
            self.entry.signature = signature(&occurrence.error);
            self.entry.context = occurrence.context.clone();
        }
        if occurrence.timestamp > self.entry.last_seen {
            self.entry.last_seen = occurrence.timestamp.clone();
        }
        // Keep the most recent resolution: it is the clearest evidence that the
        // failure is fixable and how.
        if occurrence.resolution.is_some() {
            self.entry.resolution = occurrence.resolution.clone();
        }
    }

    fn finish(mut self) -> Entry {
        self.entry.sessions = self.sessions.len();
        self.entry
    }
}

fn dedup_key(tool: &str, error: &str) -> String {
    let sig = signature(error);
    let sig = if sig.is_empty() {
        heading(error).to_lowercase()
    } else {
        sig
    };
    format!("{tool}\u{1f}{sig}")
}

/// First line that names the failure, safe to repeat as a Markdown heading.
///
/// The first non-empty line is usually `exit_code: 1` or `stdout:`, which makes
/// every entry share a heading: prefer a line that looks like a diagnostic,
/// then fall back to the first meaningful line.
fn heading(error: &str) -> String {
    let meaningful = || {
        error
            .lines()
            .map(|line| strip_log_timestamp(line.trim()).trim())
            .filter(|line| !is_wrapper_line(line))
    };
    let line = meaningful()
        .find(|line| is_diagnostic_line(line))
        .or_else(|| meaningful().next())
        .unwrap_or("(no output)");
    let line = truncate(line, HEADING_MAX_CHARS);
    // An error line that starts with `#` would otherwise become a heading of
    // its own and break the report structure.
    line.strip_prefix('#')
        .map(|rest| format!("\\#{rest}"))
        .unwrap_or(line)
}

fn excerpt(output: &str, root: &Path) -> String {
    // Wrapper lines (`exit_code:`, `stdout:`) and blank separators carry no
    // signal; drop them before spending the line budget on them.
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|line| !is_wrapper_line(line))
        .collect();
    let picked = excerpt_lines(&lines);
    let joined = cap_lines(&picked, ERROR_MAX_CHARS);
    redact(&joined, root)
}

/// Head for context plus the lines that carry the diagnostic, with `…` where
/// lines were skipped.
///
/// A long test run puts the failure after pages of boilerplate, and keeping
/// only the tail dropped the `✗ file.wren` line that tells two failures apart.
fn excerpt_lines<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    if lines.len() <= ERROR_MAX_LINES {
        return lines.to_vec();
    }
    let head = EXCERPT_HEAD_LINES.min(lines.len());
    let mut chosen: BTreeSet<usize> = (0..head).collect();
    let mut remaining = ERROR_MAX_LINES - head;
    for (index, line) in lines.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        if !is_diagnostic_line(line) {
            continue;
        }
        if !chosen.contains(&index) {
            chosen.insert(index);
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
        // A failed test prints its name and then the assertion message: keep
        // both, or the report only knows that some test failed.
        if is_failure_header(line) {
            if let Some(body) = (index + 1..lines.len()).find(|&i| {
                !chosen.contains(&i) && !is_failure_header(lines[i]) && !is_wrapper_line(lines[i])
            }) {
                chosen.insert(body);
                remaining -= 1;
            }
        }
    }
    if chosen.len() == head {
        // No marker at all: the tail is the next best guess (a summary line).
        for index in lines.len() - remaining..lines.len() {
            chosen.insert(index);
        }
    }
    assemble(lines, &chosen)
}

/// The chosen lines in their original order, with `…` for every skipped run.
fn assemble<'a>(lines: &[&'a str], chosen: &BTreeSet<usize>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    let mut previous: Option<usize> = None;
    for &index in chosen {
        let gap = match previous {
            Some(previous) => index > previous + 1,
            None => index > 0,
        };
        if gap {
            out.push("…");
        }
        out.push(lines[index]);
        previous = Some(index);
    }
    if previous.is_some_and(|index| index + 1 < lines.len()) {
        out.push("…");
    }
    out
}

/// Join whole lines up to `max_chars`; dropping a line beats cutting one in
/// half, because half a diagnostic is not a diagnostic.
fn cap_lines(lines: &[&str], max_chars: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for line in lines {
        let length = line.chars().count();
        let separator = usize::from(!out.is_empty());
        if used + separator + length > max_chars {
            if out.is_empty() {
                out = truncate(line, max_chars);
            } else if used + separator < max_chars {
                out.push('\n');
                out.push('…');
            }
            break;
        }
        if separator == 1 {
            out.push('\n');
        }
        out.push_str(line);
        used += separator + length;
    }
    out
}

/// Drop the `2026-11-18 02:12:39 ` prefix the Bialet logger writes.
fn strip_log_timestamp(line: &str) -> &str {
    let bytes = line.as_bytes();
    let digits =
        |range: std::ops::Range<usize>| range.into_iter().all(|i| bytes[i].is_ascii_digit());
    if bytes.len() > 19
        && digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b' '
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19)
        && bytes[19] == b' '
    {
        line[19..].trim_start()
    } else {
        line
    }
}

/// A line `ToolResult::from_cmd` adds around the real output.
fn is_wrapper_line(line: &str) -> bool {
    let line = strip_log_timestamp(line.trim()).trim();
    line.is_empty()
        || line == "…"
        || line == "---"
        || line == "ok"
        || line.starts_with("exit_code:")
        || line.starts_with("stdout:")
        || line.starts_with("stderr:")
}

/// True when the line carries the diagnostic, not the harness preamble.
fn is_diagnostic_line(line: &str) -> bool {
    let line = strip_log_timestamp(line.trim()).to_lowercase();
    DIAGNOSTIC_MARKERS
        .iter()
        .any(|marker| line.contains(marker))
}

/// A `✗ file.wren (3ms)` or `FAIL file.wren` line: the error text is next.
fn is_failure_header(line: &str) -> bool {
    let line = strip_log_timestamp(line.trim()).trim_start();
    line.starts_with('✗') || line.split_whitespace().next() == Some("FAIL")
}

/// Replace the absolute workspace path so the report can leave the machine.
fn redact(text: &str, root: &Path) -> String {
    let root = root.display().to_string();
    if root.is_empty() || root == "." || root == "/" {
        return text.to_string();
    }
    text.replace(&root, "<workspace>")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn describe(resolution: &Resolution) -> String {
    let when = format!(
        "{} later ({})",
        plural(resolution.calls, "call"),
        duration(resolution.seconds)
    );
    if resolution.edits.is_empty() {
        return format!("`{}` succeeded {when}", resolution.tool);
    }
    let mut edits: Vec<String> = resolution
        .edits
        .iter()
        .take(3)
        .map(|path| format!("`{path}`"))
        .collect();
    if resolution.edits.len() > 3 {
        edits.push(format!("+{} more", resolution.edits.len() - 3));
    }
    format!("edits to {}, {when}", edits.join(", "))
}

fn duration(seconds: u64) -> String {
    match seconds {
        0..=1 => format!("{seconds}s"),
        2..=59 => format!("{seconds}s"),
        60..=3599 => {
            let (minutes, rest) = (seconds / 60, seconds % 60);
            if rest == 0 {
                format!("{minutes}m")
            } else {
                format!("{minutes}m {rest}s")
            }
        }
        _ => {
            let (hours, rest) = (seconds / 3600, (seconds % 3600) / 60);
            if rest == 0 {
                format!("{hours}h")
            } else {
                format!("{hours}h {rest}m")
            }
        }
    }
}

fn seconds_between(start: &str, end: &str) -> u64 {
    let parse = |value: &str| DateTime::parse_from_rfc3339(value).ok();
    match (parse(start), parse(end)) {
        (Some(a), Some(b)) => (b - a).num_seconds().max(0) as u64,
        _ => 0,
    }
}

fn date_of(timestamp: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(timestamp.get(..10)?, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Vec<Event> {
        vec![
            Event::session_start_with("deepseek-chat", "0.1.0"),
            Event::user_message("add a contact form"),
        ]
    }

    fn result(tool: &str, status: Status, output: &str) -> Event {
        Event::tool_result(tool, "", status, output, Some(1))
    }

    fn path() -> &'static Path {
        Path::new("/tmp/example")
    }

    #[test]
    fn a_syntax_failure_is_captured_with_its_context() {
        let mut log = events();
        log.push(result(
            "bialet_validate",
            Status::Error,
            "app/views/index.wren:12: expected '}' after block",
        ));

        let found = scan_session("s1", &log, path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, Category::Syntax);
        assert_eq!(found[0].context, "add a contact form");
    }

    #[test]
    fn unrelated_tool_errors_are_ignored() {
        let mut log = events();
        log.push(result("sqlite_execute", Status::Error, "no such table"));
        log.push(result("curl", Status::Error, "connection refused"));
        assert!(scan_session("s1", &log, path()).is_empty());
    }

    #[test]
    fn a_stopped_server_that_was_not_running_is_not_friction() {
        let mut log = events();
        log.push(result(
            "bialet_server_stop",
            Status::Error,
            "no running process named `app`",
        ));
        assert!(scan_session("s1", &log, path()).is_empty());
    }

    #[test]
    fn a_docs_miss_is_friction_even_though_the_tool_succeeded() {
        let mut log = events();
        log.push(result(
            "bialet_docs",
            Status::Ok,
            "no documentation matched `websockets`",
        ));
        let found = scan_session("s1", &log, path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, Category::Docs);
    }

    #[test]
    fn repeated_failures_collapse_across_sessions() {
        let mut first = events();
        first.push(result(
            "bialet_validate",
            Status::Error,
            "app/views/index.wren:12: expected '}' after block",
        ));
        let mut second = events();
        second.push(result(
            "bialet_validate",
            Status::Error,
            "app/views/other.wren:31: expected '}' after block",
        ));

        let report = Report::build(
            &[("s1".into(), first), ("s2".into(), second)],
            path(),
            None,
            None,
        );
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].occurrences, 2);
        assert_eq!(report.entries[0].sessions, 2);
        assert_eq!(report.sessions, 2);
    }

    #[test]
    fn a_later_success_records_the_edits_that_fixed_it() {
        let mut log = events();
        log.push(result(
            "bialet_validate",
            Status::Error,
            "app/views/index.wren:12: expected '}'",
        ));
        log.push(Event::tool_call(
            "edit_file",
            &crate::event::encode_args(&[("path", "app/views/index.wren".into())]),
        ));
        log.push(result("edit_file", Status::Ok, "ok"));
        log.push(result("bialet_validate", Status::Ok, "syntax ok"));

        let found = scan_session("s1", &log, path());
        let resolution = found[0].resolution.as_ref().expect("resolved");
        assert_eq!(resolution.calls, 2, "the edit and the green validate");
        assert_eq!(resolution.edits, vec!["app/views/index.wren"]);
    }

    #[test]
    fn a_trailing_failure_stays_unresolved() {
        let mut log = events();
        log.push(result(
            "bialet_test",
            Status::Error,
            "✗ form.wren\nExpected body to contain \"Ada\"",
        ));
        let found = scan_session("s1", &log, path());
        assert!(found[0].resolution.is_none());
    }

    #[test]
    fn the_report_renders_categories_counts_and_resolution() {
        let mut log = events();
        log.push(result(
            "bialet_validate",
            Status::Error,
            "app/views/index.wren:12: expected '}'",
        ));
        log.push(Event::tool_call(
            "edit_file",
            &crate::event::encode_args(&[("path", "app/views/index.wren".into())]),
        ));
        log.push(result("edit_file", Status::Ok, "ok"));
        log.push(result("bialet_validate", Status::Ok, "syntax ok"));
        log.push(result(
            "bialet_test",
            Status::Error,
            "✗ form.wren\nExpected body to contain \"Ada\"",
        ));

        let report = Report::build(&[("s1".into(), log)], path(), None, None);
        let text = report.render();
        assert!(text.contains("# Bialet friction log"));
        assert!(text.contains("model deepseek-chat"));
        assert!(text.contains("## syntax"));
        assert!(text.contains("## tests"));
        assert!(text.contains("unblocked by: edits to `app/views/index.wren`"));
        assert!(text.contains("(unresolved)"));
    }

    #[test]
    fn an_empty_report_names_the_sessions_it_scanned() {
        let report = Report::build(&[("s1".into(), events())], path(), None, None);
        let text = report.render();
        assert!(
            text.contains("No Bialet friction recorded in 1 session."),
            "{text}"
        );
    }

    #[test]
    fn the_earliest_occurrence_is_the_representative_sample() {
        let mut old = events();
        let mut first = result("bialet_validate", Status::Error, "a.wren:1: expected '}'");
        first.timestamp = "2026-11-01T10:00:00.000Z".into();
        old.push(first);

        let mut new = events();
        let mut second = result("bialet_validate", Status::Error, "a.wren:9: expected '}'");
        second.timestamp = "2026-11-02T10:00:00.000Z".into();
        new.push(second);

        // Sessions arrive newest-first, but the sample must match "first seen".
        let report = Report::build(
            &[("new".into(), new), ("old".into(), old)],
            path(),
            None,
            None,
        );
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].error, "a.wren:1: expected '}'");
        assert_eq!(report.entries[0].first_seen, "2026-11-01T10:00:00.000Z");
    }

    #[test]
    fn the_category_filter_drops_everything_else() {
        let mut log = events();
        log.push(result("bialet_validate", Status::Error, "expected '}'"));
        log.push(result(
            "bialet_test",
            Status::Error,
            "✗ form.wren\nExpected body to contain \"Ada\"",
        ));
        let report = Report::build(&[("s1".into(), log)], path(), None, Some(Category::Tests));
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].category, Category::Tests);
        assert_eq!(report.occurrences, 1);
    }

    #[test]
    fn the_workspace_path_is_redacted() {
        let mut log = events();
        log.push(result(
            "bialet_run",
            Status::Error,
            "cannot open /tmp/example/app/main.wren",
        ));
        let found = scan_session("s1", &log, Path::new("/tmp/example"));
        assert!(found[0].error.contains("<workspace>"), "{}", found[0].error);
        assert!(!found[0].error.contains("/tmp/example"));
    }

    #[test]
    fn signature_ignores_volatile_details() {
        // Timestamps, line numbers and ids differ between attempts; the failure
        // is the same one.
        let a = "2026-10-16 17:54:43 Runtime Error Could not load module 'bialet_cli/bialet/test'.";
        let b = "2026-10-16 17:55:02 Runtime Error Could not load module 'bialet_cli/bialet/test'.";
        assert_eq!(signature(a), signature(b));

        // A genuinely different failure must not collapse into the same bucket.
        let c = "Query does not implement 'fetch(_,_,_,_,_)'.";
        assert_ne!(signature(a), signature(c));
    }

    #[test]
    fn signature_ignores_random_session_ids() {
        let a = "✗ lista.wren (40ms)\nExpected body to contain \"x\"\nrows=[{key: who, id: oa9jvaiLKlHSJi9YgAGzqa5uE70I3lDwpnK8cqBQ}]";
        let b = "✗ lista.wren (37ms)\nExpected body to contain \"x\"\nrows=[{key: who, id: U9668fRw8Xm4DkciG6ExGyyvmuEFwB2BTKDwTNRT}]";
        assert_eq!(
            signature(a),
            signature(b),
            "random ids must not hide a repeated failure"
        );
        assert!(signature(a).contains("expected"));
    }

    /// A long test run puts the failure after the migration boilerplate. Taking
    /// only the first lines made every `bialet_test` failure look identical.
    #[test]
    fn excerpt_keeps_the_failure_at_the_end() {
        let migrations = (1..=20)
            .map(|i| format!("2026-11-15 04:48:18 Log Migration applied - migration {i}\n"))
            .collect::<String>();
        let output = format!(
            "{migrations}2026-11-15 04:48:18 Migration start\nRunning tests in ./_tests...\n\n1 of 3 tests failed in 147ms\nFAIL cuadro.wren: expected 200\n"
        );

        let text = excerpt(&output, Path::new("/tmp/example"));
        assert!(text.contains("1 of 3 tests failed"), "got: {text}");
        assert!(text.contains("FAIL cuadro.wren"), "got: {text}");
        assert!(
            text.contains("Migration applied"),
            "the head stays for context: {text}"
        );
        assert!(text.contains('…'), "the skip must be visible: {text}");
    }

    const CIVIL: &str = "Error at 'Civil': Module variable is already defined.";

    /// A synthetic `bialet_test` run, shaped like the real output: migration
    /// noise, passing tests, the failures, then the summary.
    fn test_run(failures: &[(&str, &str)], millis: u32) -> String {
        let mut out = String::from(
            "exit_code: 1\nstdout:\n\
             2026-11-18 02:14:42 Log Migration applied - Create tasks table\n\
             2026-11-18 02:14:42 Migration start \n\
             Running tests in ./_tests...\n\n\
             \x20 ✓ tarea.wren (19ms)\n\
             \x20 ✓ acciones.wren (30ms)\n",
        );
        for (file, message) in failures {
            out.push_str(&format!("  ✗ {file} ({millis}ms)\n      {message}\n"));
        }
        out.push_str(
            "\nSummary:\n\nTotal Tests: 4\nPassed Tests: 2\nFailed Tests: 2\n\
             Skipped Tests: 0\nElapsed Time: 51ms\n",
        );
        out
    }

    #[test]
    fn headings_name_the_diagnostic_not_the_wrapper() {
        let validate = "exit_code: 1\nstderr:\n\
             2026-11-18 02:12:39 Compilation Error _app/domain.wren line 105 \
             Error at 'isValid': Expect '{' to begin method body.\n\
             ✗ Syntax errors found in: _app/domain.wren\n";
        let tests = test_run(&[("prioridad.wren", CIVIL)], 1);

        let validate_heading = heading(validate);
        assert!(
            validate_heading.starts_with("Compilation Error"),
            "got: {validate_heading}"
        );
        assert!(
            !validate_heading.contains("2026-11-18"),
            "the log timestamp is noise: {validate_heading}"
        );

        let tests_heading = heading(&tests);
        assert_eq!(tests_heading, "✗ prioridad.wren (1ms)");
        assert_ne!(
            validate_heading, tests_heading,
            "two different failures need two different headings"
        );
    }

    #[test]
    fn heading_skips_wrappers_and_falls_back_to_the_first_real_line() {
        assert_eq!(
            heading("exit_code: 1\nstdout:\n…\n---\nno marker here\n"),
            "no marker here"
        );
        assert_eq!(heading("exit_code: 1\nstdout:\nstderr:\n"), "(no output)");
    }

    #[test]
    fn excerpt_keeps_each_failing_test_and_its_error() {
        let output = test_run(&[("prioridad.wren", CIVIL), ("calendario.wren", CIVIL)], 1);
        let text = excerpt(&output, path());

        assert!(text.contains("✗ prioridad.wren"), "got: {text}");
        assert!(text.contains("✗ calendario.wren"), "got: {text}");
        assert!(text.contains(CIVIL), "got: {text}");
        assert!(!text.contains("exit_code"), "the wrapper is noise: {text}");
        assert!(
            text.contains("Migration applied"),
            "the head stays for context: {text}"
        );
        assert!(text.contains('…'), "the skip must be visible: {text}");
    }

    #[test]
    fn excerpt_drops_whole_lines_at_the_char_cap() {
        let long = |label: &str| format!("{label} Error at 'x': {}", "detail ".repeat(60));
        let output = format!(
            "exit_code: 1\nstdout:\nLog Migration applied\n{}\n{}\n",
            long("first"),
            long("second")
        );

        let text = excerpt(&output, path());
        assert!(
            text.chars().count() <= ERROR_MAX_CHARS,
            "the excerpt exceeded the cap: {}",
            text.chars().count()
        );
        assert!(text.contains("first Error at"), "got: {text}");
        assert!(
            !text.contains("second Error at"),
            "an over-budget line is dropped whole, not cut in half: {text}"
        );
    }

    #[test]
    fn distinct_failing_tests_do_not_share_a_signature() {
        let a = signature(&excerpt(&test_run(&[("prioridad.wren", CIVIL)], 1), path()));
        let b = signature(&excerpt(
            &test_run(&[("calendario.wren", CIVIL)], 7),
            path(),
        ));

        assert!(a.contains("prioridadwren"), "the file must survive: {a}");
        assert!(a.contains("civil"), "the error text must survive: {a}");
        assert_ne!(a, b, "different failing files need different signatures");

        // The same failure with another timing is the same failure.
        let again = signature(&excerpt(
            &test_run(&[("prioridad.wren", CIVIL)], 33),
            path(),
        ));
        assert_eq!(a, again);
    }

    #[test]
    fn different_failing_tests_stay_separate_in_the_report() {
        let run = |file: &str| {
            let mut log = events();
            log.push(result(
                "bialet_test",
                Status::Error,
                &test_run(&[(file, CIVIL)], 1),
            ));
            log
        };
        let report = Report::build(
            &[
                ("s1".into(), run("prioridad.wren")),
                ("s2".into(), run("calendario.wren")),
            ],
            path(),
            None,
            None,
        );
        assert_eq!(report.entries.len(), 2);
        let text = report.render();
        assert!(text.contains("### ✗ prioridad.wren"), "got: {text}");
        assert!(text.contains("### ✗ calendario.wren"), "got: {text}");
    }

    fn validate_event(file: &str, status: Status, output: &str) -> Event {
        Event::tool_result(
            "bialet_validate",
            &crate::event::encode_args(&[("file", file.to_string())]),
            status,
            output,
            Some(if status == Status::Ok { 0 } else { 1 }),
        )
    }

    #[test]
    fn a_validate_of_another_file_does_not_resolve_the_failure() {
        let mut log = events();
        log.push(validate_event(
            "_app/domain.wren",
            Status::Error,
            "Compilation Error _app/domain.wren line 105 Error at 'isValid': Expect '{' to begin method body.",
        ));
        log.push(validate_event("_app/template.wren", Status::Ok, "ok"));

        let found = scan_session("s1", &log, path());
        assert_eq!(found.len(), 1);
        assert!(
            found[0].resolution.is_none(),
            "a green check of another file is not a fix: {:?}",
            found[0].resolution
        );
    }

    #[test]
    fn a_validate_of_the_same_file_still_resolves_after_an_edit() {
        let mut log = events();
        log.push(validate_event(
            "_app/domain.wren",
            Status::Error,
            "Compilation Error _app/domain.wren line 105 Error at 'isValid': Expect '{' to begin method body.",
        ));
        // Same tool, other file: skipped, not mistaken for the fix.
        log.push(validate_event("_app/template.wren", Status::Ok, "ok"));
        log.push(Event::tool_call(
            "edit_file",
            &crate::event::encode_args(&[("path", "_app/domain.wren".to_string())]),
        ));
        log.push(result("edit_file", Status::Ok, "ok"));
        log.push(validate_event("_app/domain.wren", Status::Ok, "syntax ok"));

        let found = scan_session("s1", &log, path());
        let resolution = found[0].resolution.as_ref().expect("resolved");
        assert_eq!(
            resolution.calls, 3,
            "the other file, the edit and the green check"
        );
        assert_eq!(resolution.edits, vec!["_app/domain.wren"]);
    }
}
