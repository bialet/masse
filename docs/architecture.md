# Architecture

masse is a Cargo workspace. Each crate has a single responsibility and can be
compiled on its own.

```
harness-cli ── wires everything, owns main()
   ├── harness-tui      (ratatui front end)
   ├── harness-server   (axum front end)
   ├── harness-core     (agent runtime)
   │      ├── harness-tools    (tools + path/process plumbing)
   │      └── harness-session  (session.csv)
   └── harness-tools
```

## Data flow

```
user ──► front end ──► AgentHandle (mpsc command) ──► Agent task
                                                        │
                                          provider.complete(ChatRequest)
                                                        │
                                        tool_calls ──► ToolRegistry.call
                                                        │
                          AgentEvent ──► broadcast ──► front end (TUI / WS)
                                                        │
                                              append ──► session.csv
```

### The actor

`harness_core::AgentHandle` owns a single `Agent` in one Tokio task. It exposes
a `Send + Sync` handle:

- commands arrive over `tokio::sync::mpsc` (`user_message`, `shutdown`);
- progress is published over `tokio::sync::broadcast` as `AgentEvent`.

This keeps the conversation single-threaded (no locking around the message
history) while letting any number of subscribers render it. The TUI and the
WebSocket each subscribe to the same broadcast channel.

Each turn emits exactly one terminal event (`Finished`, `TurnFinished` or
`Error`), the provider has connect/request timeouts, and a panic inside the
agent is caught and surfaced as an error, so a front end never gets stuck. The
actor also accepts `Cancel`, which drops the in-flight turn (used by the TUI's
Esc key).

### Retries and completion

Transient provider failures are classified (`ProviderError::Retryable`) and
retried up to `max_retries` with exponential backoff; permanent failures fail
immediately. The model ends a task explicitly by calling the `finish` tool,
which sets a `Terminate` flag on its `ToolResult`; the agent stops and emits
`AgentEvent::Finished`. If the model just stops, the agent emits
`TurnFinished` instead.

A task in `harness-cli` also subscribes to the same broadcast and appends every
event (including errors and retries) to `.masse/masse.log`, a durable
human-readable log; the server exposes it at `GET /api/log`.

If the model keeps calling tools, the turn stops at `max_iterations`; instead of
failing, the agent makes one final completion request with no tools so the model
still produces an answer, and emits a `Notice`.

### Automatic commits

When `auto_commit` is enabled (the CLI enables it by default), `Agent::send`
prepares the workspace once — `git init` when there is no repository, plus a
local identity if none is set — and after each turn runs `git add -A` and
`git commit`. A clean tree produces no commit; the result is emitted as a
`Notice`.

### Providers

`harness_core::Provider` is one async method:

```rust
async fn complete(&self, request: ChatRequest) -> Result<ChatResponse>;
```

`OpenAiProvider` speaks the OpenAI `chat/completions` protocol over `reqwest`
with rustls, so any compatible endpoint works. `MockProvider` is deterministic
and used when no API key is configured.

### Rehydration

On startup the agent reads `session.csv` and rebuilds the message list with
`rebuild_messages`. Because the CSV has no OpenAI `tool_call_id`, tool results
are replayed as user-role notes. Live turns keep full fidelity; only the
reconstructed history is lossy, by design (one file, no JSON state).

## Project context

masse is Bialet-first. At startup `ProjectContext::load` reads `AGENTS.md`,
discovers `skills/*/SKILL.md`, locates the Bialet docs, and looks for a live
Bialet framework prompt (following Sphinx `literalinclude` wrappers). The
system prompt is then assembled in `prompt.rs` from the base role, the tool
reference, the Bialet guide (live copy if found, otherwise the copy embedded in
the binary at `assets/bialet_prompt.md`), `AGENTS.md`, the skills index and a
docs pointer.

## Tools

`harness_tools::Tool` is the extension point:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;   // JSON Schema
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolResult;
}
```

### Adding a tool

1. Implement `Tool` in a module under `harness-tools/src/`.
2. Register it in `harness_tools::builtin_registry`.
3. If it runs an external binary, build a `CmdSpec` and call `ctx.run`. Never
   use a shell; pass arguments as an array.
4. If it touches the filesystem, resolve paths with `ctx.resolve` /
   `ctx.resolve_existing` so they stay inside the project root.

`ToolResult` carries a `status`, the text handed to the model, and an optional
exit code, which the agent writes to the `tool_result` row.

### Security model

- No generic shell tool exists; there is nothing to escape from.
- Arguments are never concatenated into a command string.
- Every path is normalized and checked against the canonical project root; a
  resolved symlink that escapes the root is rejected.
- Subprocesses are killed after a timeout (30 s default) and their output is
  captured with bounded line counts in Rust, not via shell pipes.
- Background processes are tracked in memory only and killed on shutdown.

## Front ends

- `harness-tui` renders the transcript with `ratatui`, forwards typed messages,
  and turns `AgentEvent`s into styled lines.
- `harness-server` exposes the chat UI, a JSON API and a WebSocket. On connect
  it replays the parsed session, then streams events.

Both stop every tracked background process and shut the actor down on exit.
