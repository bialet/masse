# masse Development Guide

masse is an AI coding harness in Rust with two front ends (TUI and
HTTP/WebSocket server) sharing one `session.csv`.

## Layout

- `crates/harness-core` — agent loop, providers, message model, project context
- `crates/harness-tools` — every tool plus path validation and the process registry
- `crates/harness-session` — RFC 4180 `session.csv` store
- `crates/harness-tui` — ratatui front end
- `crates/harness-server` — axum front end
- `crates/harness-cli` — the `masse` binary

## Commands

```bash
cargo check --workspace
cargo test --workspace
cargo build --release
cargo clippy --workspace --all-targets
cargo fmt --all
```

## Output modes

The default front-end output is **user mode**: the model may only emit a
question or the final summary, and the front ends hide tool traffic and
intermediate narration. `--debug` shows everything. Keep that invariant when
adding events: if an event is only useful to a developer, the front ends must
not surface it in user mode. The shared logic lives in
`harness_core::present::UserModeFilter` (TUI) and in the embedded page's script
(server).

## Documentation

- `assets/bialet-docs/` is a vendored copy of the **complete** Bialet manual.
  Sync it, never edit it by hand:

  ```bash
  bash scripts/sync-bialet-docs.sh [path/to/bialet/docs]
  ```

- `assets/docs/` holds the curated Wren core references and
  `bialet-recipes.md` (verified date/money/CSV/`Util`/`Config`/test recipes).
- `crates/harness-tools/build.rs` walks both folders and generates the
  manifest, so **adding a page is enough**; there is no `include_str!` list to
  update. A missing page cannot silently stay out of the binary.
- Keep `the_prompt_only_links_bundled_pages` green: every `*.md` link in
  `assets/bialet_prompt.md` must resolve to an embedded page.

## Rules

- Tools are atomic. Never introduce a generic shell tool.
- Never use `sh -c`; pass arguments as arrays to `tokio::process::Command`.
- Use `tokio::fs` for file operations.
- Validate every path against the project root before use (`harness_tools::path`).
- Keep session persistence append-only and JSON-free.
- `event_type ∈ {session_start, user_message, assistant_message, tool_call,
  tool_result, session_end}` and `status ∈ {ok, error}`.
- A `.wren` change is not done until syntax is checked and `bialet_test` is
  green. The agent loop nudges once for syntax (`VALIDATE_BEFORE_FINISH`) and
  once for the test run (`VERIFY_BEFORE_FINISH`) when the model tries to finish
  or stop after a `.wren` edit (`mutates_wren` + `pending_verification` in
  `harness-core/src/agent.rs`); keep each nudge one-shot and question-aware.
- Keep `ToolResult::from_cmd` honest: a non-zero exit is an error, not `ok`.
- Break tool-call loops twice over (`harness-core/src/agent.rs`): three
  identical consecutive calls are skipped (`last_call`/`repeat_count`), and the
  same tool failing the same way (`STUCK_THRESHOLD`, `failure_signature`, which
  drops digits so timestamps/ids don't hide a repeat) earns a change-of-approach
  nudge. Both must reset so a fresh budget follows.
- Background servers are spawned with `process_group(0)` and killed by group
  (`harness-tools/src/process.rs`); Bialet forks a worker that survives a plain
  child kill and would keep the port.
- Prefer small, focused changes and keep the crates compiling independently.

## Style

- `rustfmt` defaults; 4-space indent.
- Snake_case functions, CamelCase types.
- Document public items briefly; explain *why*, not *what*.
- Tests live next to the code (`#[cfg(test)] mod tests`).
