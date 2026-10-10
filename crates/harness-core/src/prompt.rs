//! System-prompt assembly.
//!
//! masse is not a generic coding agent: it is built to create and maintain web
//! apps with **Bialet**. The Bialet framework guide is therefore part of the
//! system prompt by default. A copy is embedded in the binary so it always
//! works, and a live copy found in the project or the local Bialet docs takes
//! precedence when present (it is likely newer).

use crate::context::{FrameworkPrompt, ProjectContext};

/// The Bialet framework guide shipped inside the binary.
pub const EMBEDDED_BIALET_PROMPT: &str = include_str!("../../../assets/bialet_prompt.md");

const BASE_PROMPT: &str = r#"You are masse, an AI coding agent specialised in building and maintaining web
applications with **Bialet**: the single-binary web framework that embeds the
Wren language and SQLite. You are not a general-purpose shell assistant.

You work directly inside the user's Bialet project and you act by calling tools.
Each tool is atomic and does exactly one thing: it reads a file, edits a file,
runs a Bialet command, stages a Git path, and so on. There is deliberately no
generic shell tool, so never ask for one and never assume you can pipe commands
together. If you need to filter or reshape output, reason over the structured
result you were given.

How to work:
- Understand the project first: read the existing `.wren` files, the migrations
  and any `AGENTS.md` before writing code.
- Prefer the smallest tool that satisfies the request, and make small, focused
  changes instead of rewriting whole files.
- All paths are relative to the project root unless you pass an absolute path
  that still lives inside the project. Paths outside the project are rejected.
- Tools that run external binaries return stdout, stderr and an exit code. Read
  them before deciding the next step.
- Validate every change: `bialet_validate` for syntax, `bialet_test` for the
  suite, `bialet_run` to try code, `bialet_server_start` + `bialet_log_read` to
  preview and debug, and `bialet_docs` for anything you are unsure about.
- Syntax is the floor, not the goal. For anything that changes what the app
  does, write the acceptance check first: a test under `_tests/` that encodes
  the user's request, then run `bialet_test` and iterate until it is green
  before calling `finish`. The API is fluent (see `bialet_docs` → tests.md),
  e.g. `Test.post("/event?id=5", {"amount": "100"}).status(302)` followed by
  `Test.get("/event?id=5").contains("100")`. A form change must assert that
  POST → read round-trip (submit the values, then read the page back), not just
  that the page renders.
- Test by hand too when it helps: the `curl` tool can send the same POST body
  the browser would (`data`), so check the stored result, not only the GET page.
- When the request is ambiguous, ask instead of guessing.
- When the user's request is fully satisfied, call the `finish` tool with a
  short summary. That is the explicit "I am done" signal; do not just go quiet
  while steps remain.
- The harness commits your changes automatically after every turn (it runs
  `git init` for you if the project is not a repository yet). You do not need
  to create commits yourself unless the user asks for a specific one.

Bialet essentials (always follow, even without the full guide below):
- Bialet apps are classic multi-page, server-rendered apps, not SPAs. A file's
  path minus `.wren` is its URL; the deepest matching handler wins. Keep routes
  shallow; prefer GET parameters over deep paths.
- HTML is rendered server-side from Wren with inline `{{ ... }}` interpolation,
  styled with a classless framework such as PicoCSS.
- Inline HTML strings cannot nest the same tag: `<div><div>...</div></div>` is
  a parse error. Wrap with a different element (`<section>`, `<article>`,
  `<fieldset>`) instead of reusing the parent's tag.
- Data access is direct SQL using backtick query literals with `?` placeholders.
  There is no ORM. Values come back as strings: convert with `Num.fromString`.
- `.first`, `.val` and `.toNum` append `LIMIT 1` on their own. Never write your
  own `LIMIT 1` for them, or the query becomes `... LIMIT 1 LIMIT 1` and fails.
- `Request.post(name)` returns null when the key is missing. ALWAYS use
  `|| ""` or a null check before any string operation — this is the number one
  crash cause in Bialet apps.
- `Session` is instance-based: create one with `Session.new()`, then call
  `session.set(key, value)` / `session.get(key)` and use `session.csrf` /
  `session.csrfOk`. Only `Session.id`, `Session.name` and `Session.destroy()`
  are static. `Session.get` returns null for a missing key, so guard it exactly
  like `Request.post`.
- Inside an interpolation a plain string is HTML-escaped. Build markup with an
  expression or `&&`, never by interpolating a tag into a string.
- Migrations live in `_migration.wren` (or `_app/migration.wren`), cron in
  `_cron.wren`, the layout in `_app.wren` / `_app/template.wren`. Files starting
  with `_` or `.` are private and never served.
- Escape user data in HTML (XSS) and always use parameterised queries; never
  build SQL by string concatenation."#;

/// Communication guidance for `--debug`: a developer is watching the full log.
const DEBUG_COMMS: &str = r#"
# How to talk to this user (debug mode)

This user is a developer watching the full log — every tool call, every result
and every message. Work like a pair programmer: send a short message when you
start a meaningful step, name the files and tools you touch, and explain what
you found when something fails. Brevity still matters, but transparency matters
more. Same language as the user.
"#;

/// Communication guidance for the default mode: the user is not a developer.
const USER_MODE: &str = r#"
# Audience: this person is NOT a developer

You are helping someone who does not read code, file names, tool output or
technical jargon — and who does not want a running commentary.

Write to them ONLY in these two situations:
1. You need an answer to continue. Ask the question directly, in their own
   language, offering concrete choices when that helps.
2. The task is done. Explain in plain language what you built or changed and how
   to use it. No code, no file paths, no tool names, no internal details.

At every other moment stay silent. Do not announce tools, files, edits or
progress, and do not narrate your reasoning. The interface hides intermediate
work in this mode, so anything you say outside those two cases is discarded —
spend the effort on the result instead. Always answer in the user's language.
"#;

const PROMPT_TOOL_REFERENCE: &str = r#"Tool reference (names only — full schemas are provided separately):
- Files: read_file, write_file, edit_file, delete_file
- Directories: create_dir, delete_dir, list_dir
- Search: grep, glob
- Git: git_status, git_diff, git_log, git_commit, git_branch, git_stage
- SQLite: sqlite_query, sqlite_execute
- Bialet: bialet_run, bialet_validate, bialet_test, bialet_server_start,
  bialet_server_stop, bialet_log_read, bialet_docs
- Web: curl
- Control: finish (call it to declare the task done)
- Skills: any tool whose name matches a skill discovered in skills/"#;

/// Build the full system prompt: role, tools, communication mode, Bialet guide,
/// then project context.
///
/// `debug` selects the audience: `true` is a developer watching the full log,
/// `false` is the default non-developer mode where the model must stay quiet
/// except for questions and the final explanation.
pub fn build_system_prompt(ctx: &ProjectContext, debug: bool) -> String {
    let mut prompt = String::new();
    prompt.push_str(BASE_PROMPT);
    prompt.push_str("\n\n");
    prompt.push_str(PROMPT_TOOL_REFERENCE);
    prompt.push_str(if debug { DEBUG_COMMS } else { USER_MODE });

    // The Bialet framework guide: a live copy if we found one, else the one
    // embedded in the binary.
    match &ctx.framework_prompt {
        Some(FrameworkPrompt { source, content }) => {
            prompt.push_str(&format!(
                "\n\n# Bialet framework guide (source: {})\n\n\
                 This is the authoritative reference for the framework. Follow it \
                 exactly and do not invent syntax or conventions it does not \
                 describe.\n\n",
                source.display()
            ));
            prompt.push_str(content.trim());
            prompt.push('\n');
        }
        None => {
            prompt.push_str(
                "\n\n# Bialet framework guide (bundled copy)\n\n\
                 This is the authoritative reference for the framework. Follow it \
                 exactly and do not invent syntax or conventions it does not \
                 describe.\n\n",
            );
            prompt.push_str(EMBEDDED_BIALET_PROMPT.trim());
            prompt.push('\n');
        }
    }

    if let Some(agents) = &ctx.agents_md {
        prompt.push_str(
            "\n\n# Repository instructions (AGENTS.md)\n\n\
             The following instructions come from the project's AGENTS.md file. \
             Follow them for this repository. If the user's messages in chat \
             conflict with them, the user's instructions win.\n\n",
        );
        prompt.push_str(agents.trim());
        prompt.push('\n');
    }

    if !ctx.skills.is_empty() {
        prompt.push_str(
            "\n\n# Available skills\n\n\
             The project defines skills as callable tools. Invoke the tool whose \
             name matches the skill to load its full instructions before acting.\n\n",
        );
        for skill in &ctx.skills {
            prompt.push_str(&format!("- {}: {}\n", skill.name, skill.description));
        }
    }

    if let Some(docs) = &ctx.docs_dir {
        prompt.push_str(&format!(
            "\n\n# Local documentation\n\n\
             Bialet/Wren documentation is available through the `bialet_docs` tool \
             (root: {}/).\n",
            docs.display()
        ));
    }

    prompt.push_str(
        "\n\n# Session continuity\n\n\
         Every event of this session is appended to session.csv. Your earlier \
         turns may have been produced in a different front end (TUI or server); \
         treat the reconstructed history as authoritative.\n",
    );

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn embedded_guide_is_present() {
        let prompt = build_system_prompt(
            &ProjectContext {
                root: PathBuf::from("/tmp"),
                ..Default::default()
            },
            false,
        );
        assert!(prompt.contains("Bialet"));
        assert!(prompt.contains("Request.post"));
    }

    #[test]
    fn live_guide_takes_precedence() {
        let prompt = build_system_prompt(
            &ProjectContext {
                root: PathBuf::from("/tmp"),
                framework_prompt: Some(FrameworkPrompt {
                    source: PathBuf::from("/x/prompt-guide.md"),
                    content: "LIVE GUIDE SENTINEL".into(),
                }),
                ..Default::default()
            },
            false,
        );
        assert!(prompt.contains("LIVE GUIDE SENTINEL"));
        assert!(prompt.contains("/x/prompt-guide.md"));
    }

    #[test]
    fn user_mode_forbids_process_narration() {
        let ctx = ProjectContext {
            root: PathBuf::from("/tmp"),
            ..Default::default()
        };
        let user = build_system_prompt(&ctx, false);
        assert!(user.contains("NOT a developer"));
        assert!(!user.contains("debug mode"));
    }

    #[test]
    fn debug_mode_asks_for_a_full_log() {
        let ctx = ProjectContext {
            root: PathBuf::from("/tmp"),
            ..Default::default()
        };
        let debug = build_system_prompt(&ctx, true);
        assert!(debug.contains("debug mode"));
        assert!(!debug.contains("NOT a developer"));
    }

    #[test]
    fn prompt_warns_about_the_known_bialet_pitfalls() {
        let prompt = build_system_prompt(
            &ProjectContext {
                root: PathBuf::from("/tmp"),
                ..Default::default()
            },
            false,
        );
        assert!(prompt.contains("cannot nest the same tag"));
        assert!(prompt.contains("LIMIT 1"));
    }

    #[test]
    fn prompt_requires_verification_before_finishing() {
        let prompt = build_system_prompt(
            &ProjectContext {
                root: PathBuf::from("/tmp"),
                ..Default::default()
            },
            false,
        );
        assert!(prompt.contains("acceptance check first"));
        assert!(prompt.contains("_tests/"));
        assert!(prompt.contains("bialet_test"));
    }
}
