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

## Rules

- Tools are atomic. Never introduce a generic shell tool.
- Never use `sh -c`; pass arguments as arrays to `tokio::process::Command`.
- Use `tokio::fs` for file operations.
- Validate every path against the project root before use (`harness_tools::path`).
- Keep session persistence append-only and JSON-free.
- `event_type ∈ {session_start, user_message, assistant_message, tool_call,
  tool_result, session_end}` and `status ∈ {ok, error}`.
- A `.wren` change is not done until `bialet_test` is green. The agent loop
  nudges once when the model tries to finish or stop after a `.wren` edit
  without a passing test (`mutates_wren` + `VERIFY_BEFORE_FINISH` in
  `harness-core/src/agent.rs`); keep it one-shot and question-aware.
- Keep `ToolResult::from_cmd` honest: a non-zero exit is an error, not `ok`.
- Break tool-call loops: three identical consecutive calls are skipped with a
  nudge (`last_call`/`repeat_count` in `harness-core/src/agent.rs`).
- Background servers are spawned with `process_group(0)` and killed by group
  (`harness-tools/src/process.rs`); Bialet forks a worker that survives a plain
  child kill and would keep the port.
- Prefer small, focused changes and keep the crates compiling independently.

## Style

- `rustfmt` defaults; 4-space indent.
- Snake_case functions, CamelCase types.
- Document public items briefly; explain *why*, not *what*.
- Tests live next to the code (`#[cfg(test)] mod tests`).
