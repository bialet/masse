# masse

`masse` is an AI coding harness written in Rust, purpose-built for building and
maintaining web applications with **Bialet** (Wren + SQLite) rather than acting
as a general-purpose shell agent. It runs on **Linux and macOS** only and offers
two front ends over the **same persisted session**:

- a **TUI** built with `ratatui`, started with `masse --tui`;
- an **HTTP/WebSocket server** built with `axum`, started with `masse --serve`.

The single binary chooses one mode from `--tui` or `--serve`. Either way, the
whole conversation lives in one file — `.masse/session.csv` — so you can start a
turn in the terminal and keep following the same session from a browser.

```
masse                       # terminal UI (default)
masse --tui                 # terminal UI (explicit)
masse --serve --port 7001   # http://127.0.0.1:7001 (+ ws://…/ws)
```

If no front-end flag is given, `masse` starts the TUI.

## Output modes

masse is built for people who are **not** developers. By default it runs in
**user mode**: you see your own messages and, from the assistant, only the
question it asks when it needs an answer and the plain-language explanation
when it finishes. Tool calls, results, retries and the assistant's
step-by-step narration are hidden — in both front ends.

Assistant messages are **Markdown**, and both front ends render it — headings,
lists, fenced code, blockquotes, tables and inline emphasis, code and links —
instead of printing the raw source. `harness-core` owns the parser; the TUI
draws styled terminal lines and the server pre-renders the HTML the page
injects.

Pass `--debug` for the full developer log: every tool call, every result,
every intermediate message, plus provider/model/tools/session in the status
bar. The mode is fixed for the run and the TUI and server respect it. Whatever
the mode, the complete transcript is always written to `.masse/session.csv`.

## Definition of done

masse treats behaviour as unverified until a test says otherwise. After any
change to a `.wren` file, if the model tries to finish or stop without a
passing `bialet_test` since that change, the harness nudges it once to write an
acceptance test, run it, and iterate to green — then it may finish. The nudge
is one-shot (never a deadlock) and only fires for `.wren` changes, so docs and
data edits are not held up. A stop that looks like a question is always allowed
through.

Failing external commands — a red `bialet_validate`, `bialet_test`, `sqlite3`,
and so on — are reported with `status: error`, not `ok`, so the model and the
session log get a real red signal instead of prose to interpret.

## Safety nets

Two guards keep a turn from spiralling:

- **Repeated calls are cut off.** Three identical consecutive tool calls (same
  name and arguments) cannot produce a new result, so the third is skipped with
  a tool result that tells the model to change approach.
- **Servers are killed by process group.** A Bialet server forks a worker;
  killing only the parent left that worker holding the port (a leaked server
  looks like "port already in use"). Background servers are spawned as their
  own process group and the whole group is signalled on stop, and
  `bialet_server_start` takes a free port when the requested one is busy.

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
| Git (CLI) | `git_status`, `git_diff`, `git_log`, `git_commit`, `git_branch`, `git_stage`, `git_push` |
| SQLite (CLI) | `sqlite_query` (read-only), `sqlite_execute` (INSERT/UPDATE/DELETE), `sqlite_schema` (CREATE/ALTER/DROP) — all invoke `sqlite3` |
| Bialet (CLI) | `bialet_run`, `bialet_validate`, `bialet_test`, `bialet_server_start`, `bialet_server_stop`, `bialet_log_read`, `bialet_docs` |
| Web (CLI) | `curl` (local Bialet server only: loopback + the port opened by `bialet_server_start`; GET, or POST via `data` + `header`) |
| Control | `finish` (the model calls it to declare the task done) |
| Skills (dynamic) | one tool per `skills/<name>/SKILL.md` |

Bialet tools map to the framework CLI:

| Tool | Command |
| --- | --- |
| `bialet_run` | `bialet -r <code>` |
| `bialet_validate` | `bialet -t <file>` |
| `bialet_test` | `bialet -T [dir] [-q]` |
| `bialet_server_start` | `bialet -p <port> <path>` (background, logs to `.masse/log.txt`; falls back to a free port if the requested one is busy) |
| `bialet_server_stop` | kills the tracked background process |
| `bialet_log_read` | native read of `.masse/log.txt` (or a given file) |
| `bialet_docs` | search/read the local Bialet/Wren docs, plus bundled `sqlite.md`, `html.md`, `css.md`, `javascript.md` (local files win on name collisions) |

Background processes started by `bialet_server_start` live **in the runtime
memory** of masse, never on disk. Restarting masse forgets them; `--tui` and
`--serve` both stop every tracked process on exit.

## Session persistence

One file, **`.masse/session.csv`**, **exactly one event per physical line**. No
`history/` directory and no `state.json`: the state is reconstructed by reading
the CSV, and file history is Git's job. The format is RFC 4180 for separators
and quoting, with newlines escaped so records never span multiple lines.

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
- Commas and double quotes are quoted RFC 4180 style; **newlines (and
  backslashes) are escaped** as `\n`, `\r`, `\\`, so a field never contains a
  real line break and every event stays on one line.

The default path is `<workspace>/.masse/session.csv`; override it with
`--session`.

On startup masse also ensures the workspace `.gitignore` ignores its runtime
state: it adds `.masse/` and `_db.sqlite*` when they are missing (idempotent,
best-effort).

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

## Automatic commits

After every turn masse commits the workspace, so each step of the work is a
checkpoint. If the project is not a Git repository yet, it runs `git init`
first (and sets a local `user.name` / `user.email` only when none is
configured, so the commit succeeds).

- The commit message is `masse: <first line of your request>`.
- When a turn changes nothing, no commit is made.
- Runtime state (`.masse/`, `_db.sqlite*`) is gitignored, so it is never
  committed.
- The event stream gets a `notice` with the short hash, visible in the TUI and
  in `.masse/masse.log`.

Disable it with `--no-auto-commit`.

With **`--auto-push`** (env `MASSE_AUTO_PUSH`), after each commit masse also runs
`git push origin HEAD`, so pushing the branch deploys on the remote (for
example, an IZAR app pushing `test` builds the test environment). Requires
auto-commit; a no-op push is harmless.

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
| `--session` | — | `<workspace>/.masse/session.csv` | Session file |
| `--tui` | — | **default** | Terminal UI |
| `--serve` | — | — | HTTP/WebSocket server instead of the TUI |
| `--host` / `--port` | — | `127.0.0.1` / `7001` | Server bind |
| `--model` | `MASSE_MODEL` | config file, then `deepseek-chat` | Model id |
| `--base-url` | `MASSE_BASE_URL` | config file, then `https://api.deepseek.com/v1` | OpenAI-compatible base URL |
| `--provider` | `MASSE_PROVIDER` | config file's `provider` key | Named provider profile (its `base_url`/`model`/`api_key`) |
| `--api-key` | `MASSE_API_KEY` | config file | API key (absent ⇒ mock provider) |
| `--api-key-file` | `MASSE_API_KEY_FILE` | — | Read the key from a file; beats `--api-key` |
| `--config-file` | `MASSE_CONFIG_FILE` | `<user config dir>/masse/config` | Configuration file |
| `--mock` | — | — | Force the offline mock provider |
| `--bialet-bin` | `MASSE_BIALET_BIN` | `bialet` | Bialet binary |
| `--bialet-docs` | `MASSE_BIALET_DOCS` | auto-detected | Docs directory for `bialet_docs` |
| `--timeout` | — | `30` | Default external-command timeout (s) |
| `--request-timeout` | — | `120` | Per-request provider timeout (s) |
| `--prompt-file` | `MASSE_PROMPT_FILE` | — | Extra initial prompt appended on top of the Bialet guide |
| `--max-iterations` | — | `50` | Tool-call iterations per turn; on reaching the cap the agent asks for a final answer without tools |
| `--no-auto-commit` | — | — | Disable the automatic git commit after every turn |
| `--auto-push` | `MASSE_AUTO_PUSH` | — | After each auto-commit, run `git push origin HEAD` |
| `--debug` | `MASSE_DEBUG` | — | Developer mode: show the full log (default is user mode) |
| `--max-retries` | — | `3` | Retries per model call for transient errors (`0` disables) |
| `--retry-delay-ms` | — | `800` | Base delay for the retry backoff |
| `--temperature` | — | `0.2` | Sampling temperature |
| `--print-config` | — | — | Print resolved config and exit |

The provider speaks the OpenAI `chat/completions` protocol, so DeepSeek, OpenAI,
Groq, OpenRouter, Ollama, vLLM and similar work by setting `--base-url` and
`--api-key`. With no key, or with `--mock`, the offline mock provider is used.

### Persistent configuration

Settings live in a flat `KEY=VALUE` file — `~/Library/Application Support/masse/config`
on macOS, `~/.config/masse/config` on Linux — created with mode `0600`. It lives
**outside the workspace on purpose**: the harness's file tools are confined to
the workspace, so a key stored here is not reachable by the model and does not
end up in `session.csv` or `.masse/masse.log`.

```bash
masse config path                 # where the file is (and whether it exists)
masse config set-api-key          # prompt; the key is hidden as you type and
                                  # never reaches your shell history
masse config set provider deepseek
masse config set deepseek.model deepseek-reasoner
masse config show                 # the file, with api_key values masked
masse config unset deepseek.model
masse config set deepseek.api_key sk-…   # target one profile explicitly
```

Two levels of keys:

```text
provider = deepseek        # the profile used by default
model = deepseek-chat      # top-level defaults, shared by every profile
deepseek.api_key = sk-…    # per-profile values; any profile name works
openai.base_url = https://api.openai.com/v1
```

Precedence, lowest to highest:

1. config file (`provider` selects the profile; `<profile>.<key>` beats the top-level key)
2. `MASSE_*` environment variables
3. command-line flags

Within one level a key **file** (`--api-key-file` / `MASSE_API_KEY_FILE`) beats a
bare key value. `--api-key-file` is the option for containers and systemd, where
a mounted secret is safer than a value in argv (visible via `ps`) or in the
environment (inherited by every child process, including `bialet` and `curl`).

`--print-config` always masks the key and reports where it came from:

```
provider       : openai-compatible [profile deepseek]
config file    : /Users/you/.config/masse/config (loaded)
model          : deepseek-reasoner
api key        : sk-t…3456 (from config file [deepseek])
```

## Build & run

Requires a Rust toolchain (`rustup`).

```bash
cargo build --release
target/release/masse --print-config -C /path/to/project
target/release/masse --tui -C /path/to/project
target/release/masse --serve --port 7001 -C /path/to/project
```

### Install

Build the release binary and put `masse` on your `PATH` (`~/.cargo/bin`):

```bash
cargo install --path crates/harness-cli --locked
```

`--locked` installs exactly the versions in `Cargo.lock`. Use `--force` to
reinstall after a change; `cargo install --list` shows what is installed and
`cargo uninstall masse` removes it.

Run the tests:

```bash
cargo test --workspace
```

## What goes into the system prompt

masse is **Bialet-first**: the Bialet framework guide is part of the system
prompt by default, so it writes idiomatic `.wren` from the first response.
Composition (`harness-core/src/prompt.rs`):

1. base instructions: masse's role, the atomic-tool philosophy, and Bialet
   essentials;
2. the tool reference;
3. the Bialet framework guide — a live copy if one is found, otherwise the copy
   bundled in the binary (`assets/bialet_prompt.md`, embedded at build time);
4. the repository `AGENTS.md` (chat messages take priority over it);
5. the list of discovered skills;
6. a pointer to the local docs;
7. the contents of `--prompt-file`, when given.

The live guide is searched in this order:

- in the workspace: `BIALET_PROMPT.md`, `prompt-guide.md`, `.masse/prompt.md`;
- in the detected docs directory: `prompt-guide.md`, `prompt.md`, `ai-prompt.md`.

A file that only contains a Sphinx `literalinclude` — like Bialet's
`docs/ai-prompt.md` — is followed to its target, so it resolves to
`prompt-guide.md` automatically. If nothing is found, the bundled copy is used,
so masse always knows Bialet, even offline. `--prompt-file` adds an extra prompt
on top (for project-specific or overriding instructions).

## Long turns, retries and completion

A turn always ends with exactly one terminal event (`Finished`, `TurnFinished`
or `Error`), so a front end can never get stuck on "thinking". On top of that:

- **Bounded retries**: transient provider failures (timeouts, connection
  resets, HTTP 429/5xx) are retried up to `--max-retries` times with an
  exponential backoff starting at `--retry-delay-ms`, then the turn fails
  cleanly. Permanent failures (bad request, auth, malformed response) are not
  retried. A `retrying` event is emitted while this happens.
- the provider also has a 10 s connect timeout and a `--request-timeout`
  (default 120 s);
- external commands are killed after their timeout (30 s default);
- a panic in the agent is caught and reported as an error;
- a per-turn **iteration cap** (`--max-iterations`, default 50) prevents runaway
  tool loops. When it is reached the agent does not fail: it makes one more call
  with **tools disabled** so the model produces a final answer (a `notice` event
  marks this); the turn only errors if that wrap-up call itself fails;
- **every event — including errors and retries — is appended to
  `.masse/masse.log`**, a human-readable runtime log, so there is always a
  place to see what failed. The server also exposes it at `GET /api/log`;
- in the TUI, **Esc** cancels the in-flight turn, and **Ctrl-C** quits.

### Knowing when the task is finished

Completion is explicit: the model calls the **`finish`** tool with a short
summary (and `success`) once the request is satisfied. The harness then stops
the turn and reports it everywhere:

- **TUI**: a green `✔ finished: <summary>` line and status `done` (red `failed`
  when `success=false`);
- **server**: a WebSocket event `{"type":"event","event":{"kind":"finished",…}}`,
  and `/api/session` exposes `"finished": true|false`;
- **`session.csv`**: the call is recorded as `tool_call` / `tool_result` rows
  with `tool=finish`.

`TurnFinished` is the fallback for when the model simply stops calling tools
without calling `finish`.

## HTTP API

| Route | Method | Description |
| --- | --- | --- |
| `/` | GET | Minimal chat UI (WebSocket-backed) |
| `/api/health` | GET | Provider, model, tool count, root |
| `/api/tools` | GET | Registered tools |
| `/api/session` | GET | Parsed session events (and a `finished` flag) |
| `/api/log` | GET | Tail of `.masse/masse.log` (`?lines=N`) |
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
