---
name: masse-guide
description: |-
  Explain how the masse harness itself works. Use when the user asks what tools
  masse has, how the session file is structured, how to run the TUI or the
  server, or how skills and AGENTS.md are loaded.
---

# masse self-reference

masse is an AI coding harness in Rust. It has two front ends over one session:

- `masse --tui` opens a ratatui terminal UI.
- `masse --serve --port 7001` starts an axum HTTP/WebSocket server.

## Session

Everything is stored in a single append-only `session.csv` (RFC 4180):

```
timestamp,event_type,role,tool,args,status,output,exit_code
```

`event_type` is one of `session_start`, `user_message`, `assistant_message`,
`tool_call`, `tool_result`, `session_end`. `status` is `ok` or `error`. `args`
is `k=v` pairs joined with `|`, values percent-encoded.

## Tools

Files: `read_file`, `write_file`, `edit_file`, `delete_file`.
Directories: `create_dir`, `delete_dir`, `list_dir`.
Search: `grep`, `glob`.
Git: `git_status`, `git_diff`, `git_log`, `git_commit`, `git_branch`, `git_stage`.
SQLite: `sqlite_query`, `sqlite_execute`.
Bialet: `bialet_run`, `bialet_validate`, `bialet_test`, `bialet_server_start`,
`bialet_server_stop`, `bialet_log_read`, `bialet_docs`.
Web: `curl`.
Control: `finish` — call it to declare the task done (stops the turn).

There is no generic shell tool: every tool is atomic and external binaries are
invoked with argument arrays.
