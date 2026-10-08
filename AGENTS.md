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

## Rules

- Tools are atomic. Never introduce a generic shell tool.
- Never use `sh -c`; pass arguments as arrays to `tokio::process::Command`.
- Use `tokio::fs` for file operations.
- Validate every path against the project root before use (`harness_tools::path`).
- Keep session persistence append-only and JSON-free.
- `event_type ∈ {session_start, user_message, assistant_message, tool_call,
  tool_result, session_end}` and `status ∈ {ok, error}`.
- Prefer small, focused changes and keep the crates compiling independently.

## Style

- `rustfmt` defaults; 4-space indent.
- Snake_case functions, CamelCase types.
- Document public items briefly; explain *why*, not *what*.
- Tests live next to the code (`#[cfg(test)] mod tests`).
