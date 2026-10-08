# masse

`masse` is an AI coding harness written in Rust. It runs on **Linux and macOS**
only and offers two front ends over the **same persisted session**:

- a **TUI** built with `ratatui`, started with `masse --tui`;
- an **HTTP/WebSocket server** built with `axum`, started with `masse --serve`.

The single binary chooses one mode from `--tui` or `--serve`. Either way, the
whole conversation lives in one file — `session.csv` — so you can start a turn
in the terminal and keep following the same session from a browser.

```
masse --tui                 # terminal UI
masse --serve --port 7001   # http://127.0.0.1:7001 (+ ws://…/ws)
```

## Philosophy: atomic tools, no shell

Every tool does exactly one thing. **There is no generic shell tool.** Tools
that need an external binary invoke it *directly* with an argument array through
`tokio::process::Command`; `sh -c` is never used and arguments are never
concatenated into a shell string. File and directory operations are native
`tokio::fs` calls. Output filtering and truncation happen in Rust, never through
shell pipes. External binaries always return a structured result:

```
{ stdout, stderr, exit_code }
```

Every path argument is validated against the project working directory; a path
that escapes the root is rejected. External commands have a 30 s timeout by
default, configurable per call (`timeout_secs`) or globally (`--timeout`).

## Tools

| Group | Tools |
| --- | --- |
| Files (native) | `read_file`, `write_file`, `edit_file`, `delete_file` |
| Directories (native) | `create_dir`, `delete_dir`, `list_dir` |
| Search (CLI) | `grep` (invokes `grep`), `glob` (invokes `find`) |
| Git (CLI) | `git_status`, `git_diff`, `git_log`, `git_commit`, `git_branch`, `git_stage` |
| SQLite (CLI) | `sqlite_query`, `sqlite_execute` (invoke `sqlite3`) |
| Bialet (CLI) | `bialet_run`, `bialet_validate`, `bialet_test`, `bialet_server_start`, `bialet_server_stop`, `bialet_log_read`, `bialet_docs` |
| Web (CLI) | `curl` |
| Skills (dynamic) | one tool per `skills/<name>/SKILL.md` |

Bialet tools map to the framework CLI:

| Tool | Command |
| --- | --- |
| `bialet_run` | `bialet -r <code>` |
| `bialet_validate` | `bialet -t <file>` |
| `bialet_test` | `bialet -T [dir] [-q]` |
| `bialet_server_start` | `bialet -p <port> <path>` (background, logs to `.tmp/output.log`) |
| `bialet_server_stop` | kills the tracked background process |
| `bialet_log_read` | native read of `.tmp/output.log` (or a given file) |
| `bialet_docs` | search/read the local Bialet/Wren documentation |

Background processes started by `bialet_server_start` live **in the runtime
memory** of masse, never on disk. Restarting masse forgets them; `--tui` and
`--serve` both stop every tracked process on exit.

## Session persistence

One file, **`session.csv`**, one event per line. No `history/` directory and no
`state.json`: the state is reconstructed by reading the CSV, and file history is
Git's job. The format is RFC 4180.

```
timestamp,event_type,role,tool,args,status,output,exit_code
2026-10-08T12:00:00.000Z,session_start,system,,,,,
2026-10-08T12:00:05.120Z,user_message,user,,,,Build me a guestbook, 
2026-10-08T12:00:06.900Z,tool_call,assistant,write_file,path=index.wren|content=...,,,
2026-10-08T12:00:07.010Z,tool_result,tool,write_file,path=index.wren|content=...,ok,wrote 812 bytes,
```

- `event_type` ∈ `{session_start, user_message, assistant_message, tool_call, tool_result, session_end}`.
- `status` ∈ `{ok, error}`.
- `args` is a `k=v` map joined with `|`, values percent-encoded.
- Cells that do not apply are left empty.
- Fields containing a comma, a quote or a newline are quoted; internal quotes
  are doubled. Tool output may contain newlines and is quoted accordingly.

The default path is `<workspace>/session.csv`; override it with `--session`.

## AGENTS.md integration

At startup masse reads `AGENTS.md` from the project root and injects it into the
system prompt as repository instructions. Instructions typed in the chat take
priority over `AGENTS.md` when they conflict. This follows the standard
`AGENTS.md` convention (Linux Foundation / agents.md).

## Skills integration

masse scans `skills/<name>/SKILL.md` at startup. Each skill's `name` and
`description` are read from the YAML frontmatter and the skill is registered **as
a tool**. When the model invokes it, the markdown body is injected into the
conversation as the tool result, following the `SKILL.md` format used by
skills.sh.

```markdown
---
name: bialet-build
description: Build Bialet web apps step by step.
---

# Bialet Development Skill
…instructions…
```

## Architecture

A Cargo workspace of focused crates:

```
crates/
  harness-core     runtime: message model, providers, agent loop, context
  harness-tools    every tool (native + CLI), path validation, process registry
  harness-session  session.csv append-only store and RFC 4180 codec
  harness-tui      ratatui terminal front end
  harness-server   axum HTTP/WebSocket front end
  harness-cli      the single `masse` binary
```

`harness-core` owns an **actor** (`AgentHandle`): the agent runs in one task and
streams `AgentEvent`s over a `tokio::sync::broadcast` channel, while the TUI and
the server subscribe. Commands (`user_message`, `shutdown`) arrive over an mpsc
channel. Both front ends therefore share the same runtime and the same session
file.

## Configuration

| Flag | Env | Default | Meaning |
| --- | --- | --- | --- |
| `-C`, `--workspace` | — | `.` | Project working directory |
| `--session` | — | `<workspace>/session.csv` | Session file |
| `--tui` / `--serve` | — | — | Front end (one required) |
| `--host` / `--port` | — | `127.0.0.1` / `7001` | Server bind |
| `--model` | `MASSE_MODEL` | `deepseek-chat` | Model id |
| `--base-url` | `MASSE_BASE_URL` | `https://api.deepseek.com/v1` | OpenAI-compatible base URL |
| `--api-key` | `MASSE_API_KEY` | — | API key (absent ⇒ mock provider) |
| `--mock` | — | — | Force the offline mock provider |
| `--bialet-bin` | `MASSE_BIALET_BIN` | `bialet` | Bialet binary |
| `--bialet-docs` | `MASSE_BIALET_DOCS` | auto-detected | Docs directory for `bialet_docs` |
| `--timeout` | — | `30` | Default external-command timeout (s) |
| `--max-iterations` | — | `24` | Tool iterations per turn |
| `--temperature` | — | `0.2` | Sampling temperature |
| `--print-config` | — | — | Print resolved config and exit |

The provider speaks the OpenAI `chat/completions` protocol, so DeepSeek, OpenAI,
Groq, OpenRouter, Ollama, vLLM and similar work by setting `--base-url` and
`--api-key`. With no key, or with `--mock`, the offline mock provider is used.

## Build & run

Requires a Rust toolchain (`rustup`).

```bash
cargo build --release
target/release/masse --print-config -C /path/to/project
target/release/masse --tui -C /path/to/project
target/release/masse --serve --port 7001 -C /path/to/project
```

Run the tests:

```bash
cargo test --workspace
```

## HTTP API

| Route | Method | Description |
| --- | --- | --- |
| `/` | GET | Minimal chat UI (WebSocket-backed) |
| `/api/health` | GET | Provider, model, tool count, root |
| `/api/tools` | GET | Registered tools |
| `/api/session` | GET | Parsed session events |
| `/api/message` | POST | `{ "text": "…" }` queues a user turn |
| `/ws` | GET | WebSocket: replays history, streams events |

WebSocket frames are JSON. Client → server:
`{"type":"user_message","text":"…"}`. Server → client:
`{"type":"history","events":[…]}` on connect, then
`{"type":"event","event":{"kind":"…"}}` as the turn progresses.

## Limitations

- Linux and macOS only; no Windows support is provided or tested.
- No generic shell tool by design. If you need something the tools do not cover,
  add a new atomic tool in `harness-tools`.
- Background processes are not persisted; they die with masse.
- The CSV has no `tool_call_id`, so replayed tool results are reconstructed as
  user-role notes. Live turns keep full fidelity.
