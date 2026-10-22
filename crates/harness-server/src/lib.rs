//! The HTTP/WebSocket front end (axum).
//!
//! The server exposes a tiny chat UI plus a JSON API and a WebSocket that
//! streams [`harness_core::AgentEvent`]s. It shares the exact same
//! `session.csv` as the TUI, so a session started in one front end can be
//! followed from the other.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use harness_core::markdown;
use harness_core::{AgentEvent, AgentHandle};
use harness_session::{Event as SessionEvent, SessionManager};
use harness_tools::ToolRegistry;
use serde_json::{json, Value as JsonValue};
use tokio::sync::broadcast::error::RecvError;

/// Live turn state exposed by `/api/session`.
///
/// `session.csv` only records the conversation; it has no event for "the model
/// stopped on its own". External runners need that, or they wait forever after
/// a turn that did not call `finish`.
#[derive(Clone, Debug)]
pub struct TurnState {
    pub running: bool,
    /// `none`, `finished`, `stopped`, `error` or `cancelled`.
    pub outcome: &'static str,
    pub success: Option<bool>,
}

impl Default for TurnState {
    fn default() -> Self {
        TurnState {
            running: false,
            outcome: "none",
            success: None,
        }
    }
}

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    pub handle: AgentHandle,
    pub sessions: Arc<SessionManager>,
    pub tools: Arc<ToolRegistry>,
    pub provider_name: String,
    pub model: String,
    pub root: String,
    /// Path to the human-readable runtime log, if logging is enabled.
    pub log_path: Option<PathBuf>,
    /// Whether the UI shows the full log (the default). When false, the UI runs
    /// in user mode: each turn collapses to the question the model asks or the
    /// final summary.
    pub debug: bool,
    /// Whether a turn is running and how the last one ended.
    pub turn: Arc<std::sync::Mutex<TurnState>>,
}

/// Bind `addr` and serve until the process is stopped.
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/tools", get(list_tools))
        .route("/api/session", get(session_events))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/activate", post(activate_session))
        .route("/api/sessions/rename", post(rename_session))
        .route("/api/log", get(log_tail))
        .route("/api/message", post(post_message))
        .route(
            "/api/upload",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    spawn_turn_tracker(state.handle.clone(), state.turn.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing_line(format!("masse server listening on http://{addr}"));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing_line("shutdown signal received");
        })
        .await?;
    Ok(())
}

fn tracing_line(line: impl AsRef<str>) {
    println!("[masse] {}", line.as_ref());
}

/// Follow the broadcast stream and mirror the turn lifecycle into shared state.
/// The front end and any HTTP polling client can then tell the difference
/// between "still thinking", "finished with finish" and "stopped without it".
fn spawn_turn_tracker(handle: AgentHandle, turn: Arc<std::sync::Mutex<TurnState>>) {
    let mut rx = handle.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => apply_turn_event(&turn, &event),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

fn apply_turn_event(turn: &Arc<std::sync::Mutex<TurnState>>, event: &AgentEvent) {
    let next = match event {
        AgentEvent::UserMessage(_) => Some(TurnState {
            running: true,
            outcome: "none",
            success: None,
        }),
        AgentEvent::Finished { success, .. } => Some(TurnState {
            running: false,
            outcome: "finished",
            success: Some(*success),
        }),
        AgentEvent::TurnFinished => Some(TurnState {
            running: false,
            outcome: "stopped",
            success: None,
        }),
        AgentEvent::Error(message) => Some(TurnState {
            running: false,
            outcome: if message.contains("cancel") {
                "cancelled"
            } else {
                "error"
            },
            success: Some(false),
        }),
        _ => None,
    };
    if let (Some(next), Ok(mut state)) = (next, turn.lock()) {
        *state = next;
    }
}

async fn index(State(state): State<AppState>) -> Html<String> {
    let mode = if state.debug { "debug" } else { "user" };
    Html(INDEX_HTML.replace("__MASSE_MODE__", mode))
}

async fn health(State(state): State<AppState>) -> Json<JsonValue> {
    Json(json!({
        "status": "ok",
        "provider": state.provider_name,
        "model": state.model,
        "tools": state.tools.len(),
        "root": state.root,
        "mode": if state.debug { "debug" } else { "user" },
    }))
}

async fn list_tools(State(state): State<AppState>) -> Json<JsonValue> {
    let tools: Vec<JsonValue> = state
        .tools
        .tools()
        .iter()
        .map(|t| {
            json!({
                "name": t.name(),
                "description": t.description(),
                "requires_approval": t.requires_approval(),
            })
        })
        .collect();
    Json(json!({ "tools": tools }))
}

async fn session_events(State(state): State<AppState>) -> Json<JsonValue> {
    let store = state.sessions.active_store().await;
    let events = store.read_all().await.unwrap_or_default();
    let info = state.sessions.active_info().await.ok();
    // The most recent `finish` tool result tells whether the task completed.
    let finished = events
        .iter()
        .rev()
        .find(|e| e.event_type == "tool_result" && e.tool == "finish")
        .map(|e| e.status == "ok");
    let turn = state.turn.lock().map(|t| t.clone()).unwrap_or_default();
    Json(json!({
        "session": store.path().display().to_string(),
        "id": state.sessions.active_id().await,
        "title": info.map(|i| i.title).unwrap_or_default(),
        "finished": finished,
        "turn": {
            "state": if turn.running { "running" } else { "idle" },
            "outcome": turn.outcome,
            "success": turn.success,
        },
        "events": events.iter().map(session_event_json).collect::<Vec<_>>(),
    }))
}

fn session_info_json(info: &harness_session::SessionInfo) -> JsonValue {
    json!({
        "id": info.id,
        "title": info.title,
        "created": info.created,
        "updated": info.updated,
        "events": info.events,
        "path": info.path.display().to_string(),
    })
}

async fn list_sessions(State(state): State<AppState>) -> Json<JsonValue> {
    let sessions = state.sessions.list().await;
    Json(json!({
        "active": state.sessions.active_id().await,
        "dir": state.sessions.dir().display().to_string(),
        "sessions": sessions.iter().map(session_info_json).collect::<Vec<_>>(),
    }))
}

/// Create a fresh session and switch the agent to it.
fn turn_is_running(state: &AppState) -> bool {
    state.turn.lock().map(|turn| turn.running).unwrap_or(false)
}

async fn create_session(State(state): State<AppState>, body: Option<Json<JsonValue>>) -> Response {
    if turn_is_running(&state) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "finish the current turn before switching sessions" })),
        )
            .into_response();
    }
    let title = body
        .as_ref()
        .and_then(|Json(value)| value.get("title"))
        .and_then(|value| value.as_str());
    match state.sessions.create(title).await {
        Ok(info) => {
            if !state.handle.switch_session(info.id.clone()) {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "agent actor stopped" })),
                )
                    .into_response();
            }
            (
                StatusCode::CREATED,
                Json(json!({ "session": session_info_json(&info) })),
            )
                .into_response()
        }
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{error:#}") })),
        )
            .into_response(),
    }
}

/// Switch to an existing session. The WebSocket announces the new transcript.
async fn activate_session(State(state): State<AppState>, Json(body): Json<JsonValue>) -> Response {
    if turn_is_running(&state) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "finish the current turn before switching sessions" })),
        )
            .into_response();
    }
    let Some(id) = body.get("id").and_then(|value| value.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing `id`" })),
        )
            .into_response();
    };
    if !state.handle.switch_session(id) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "agent actor stopped" })),
        )
            .into_response();
    }
    (StatusCode::ACCEPTED, Json(json!({ "status": "switching" }))).into_response()
}

async fn rename_session(State(state): State<AppState>, Json(body): Json<JsonValue>) -> Response {
    let (Some(id), Some(title)) = (
        body.get("id").and_then(|value| value.as_str()),
        body.get("title").and_then(|value| value.as_str()),
    ) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "`id` and `title` are required" })),
        )
            .into_response();
    };
    match state.sessions.rename(id, title).await {
        Ok(info) => Json(json!({ "session": session_info_json(&info) })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{error:#}") })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct LogQuery {
    lines: Option<usize>,
}

/// Tail the human-readable runtime log (`.masse/masse.log`), where errors and
/// retries are recorded.
async fn log_tail(State(state): State<AppState>, Query(q): Query<LogQuery>) -> Json<JsonValue> {
    let Some(path) = &state.log_path else {
        return Json(json!({ "error": "no runtime log configured" }));
    };
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let n = q.lines.unwrap_or(400).max(1);
    let lines: Vec<&str> = content.lines().collect();
    let log = if lines.len() > n {
        lines[lines.len() - n..].join("\n")
    } else {
        content
    };
    Json(json!({
        "path": path.display().to_string(),
        "log": log,
    }))
}

async fn post_message(
    State(state): State<AppState>,
    Json(body): Json<JsonValue>,
) -> impl IntoResponse {
    let text = body
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if text.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing `text`" })),
        );
    }
    if let Ok(mut turn) = state.turn.lock() {
        *turn = TurnState {
            running: true,
            outcome: "none",
            success: None,
        };
    }
    let ok = state.handle.send_user(text);
    if ok {
        (
            axum::http::StatusCode::ACCEPTED,
            Json(json!({ "status": "queued" })),
        )
    } else {
        if let Ok(mut turn) = state.turn.lock() {
            turn.running = false;
        }
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "agent actor stopped" })),
        )
    }
}

/// Largest file the browser may upload. Spreadsheets are usually small; this
/// is a guard against accidental huge bodies, not a product limit.
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct UploadQuery {
    name: Option<String>,
}

/// Accept a raw file body and store it under `.masse/uploads/`, inside the
/// project root, so the agent can read it with `read_spreadsheet` later.
async fn upload(
    State(state): State<AppState>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "empty file" })),
        )
            .into_response();
    }
    if body.len() > MAX_UPLOAD_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "file too large (max 25 MB)" })),
        )
            .into_response();
    }

    let root = std::path::Path::new(&state.root);
    let dir = match harness_tools::path::resolve_within(root, ".masse/uploads") {
        Ok(dir) => dir,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("cannot create upload dir: {e}") })),
        )
            .into_response();
    }

    let safe = sanitize_filename(q.name.as_deref().unwrap_or("upload.bin"));
    let dest = dir.join(format!("{}-{safe}", uuid::Uuid::new_v4().simple()));
    if let Err(e) = tokio::fs::write(&dest, &body).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("cannot save upload: {e}") })),
        )
            .into_response();
    }

    let rel = dest
        .strip_prefix(root)
        .unwrap_or(&dest)
        .display()
        .to_string();
    (
        StatusCode::OK,
        Json(json!({ "path": rel, "bytes": body.len() })),
    )
        .into_response()
}

/// Keep only a safe basename: no directories, no separators, bounded length.
fn sanitize_filename(name: &str) -> String {
    let base = std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload.bin");
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(120);
    if out.trim_matches('.').is_empty() {
        "upload.bin".to_string()
    } else {
        out
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.handle.subscribe();

    // Ping periodically so proxies that drop idle WebSockets (Cloudflare closes
    // them after ~100s of silence) keep the connection open. Browsers answer
    // Ping frames with a Pong automatically, so this also flushes dead peers.
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await;

    // Replay history first so a late client sees the whole session.
    let store = state.sessions.active_store().await;
    if let Ok(history) = store.read_all().await {
        let payload = json!({
            "type": "history",
            "session": store.path().display().to_string(),
            "id": state.sessions.active_id().await,
            "events": history.iter().map(session_event_json).collect::<Vec<_>>(),
        });
        if sender
            .send(Message::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(user) = parse_client_message(text.as_str()) {
                            state.handle.send_user(user);
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sender.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(ev) => {
                        let payload = json!({ "type": "event", "event": agent_event_json(&ev) });
                        if sender.send(Message::Text(payload.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
}

fn parse_client_message(text: &str) -> Option<String> {
    let value: JsonValue = serde_json::from_str(text).ok()?;
    if value.get("type").and_then(|v| v.as_str()) == Some("user_message") {
        value
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

fn session_event_json(ev: &SessionEvent) -> JsonValue {
    // Assistant messages are Markdown; pre-render them so the browser only has
    // to inject the result. Other event types carry an empty string.
    let html = if ev.event_type == "assistant_message" {
        markdown::to_html(&ev.output)
    } else {
        String::new()
    };
    json!({
        "timestamp": ev.timestamp,
        "event_type": ev.event_type,
        "role": ev.role,
        "tool": ev.tool,
        "args": ev.args,
        "status": ev.status,
        "output": ev.output,
        "html": html,
        "exit_code": ev.exit_code,
    })
}

fn agent_event_json(ev: &AgentEvent) -> JsonValue {
    match ev {
        AgentEvent::UserMessage(text) => json!({ "kind": "user_message", "text": text }),
        AgentEvent::AssistantMessage(text) => {
            json!({ "kind": "assistant_message", "text": text, "html": markdown::to_html(text) })
        }
        AgentEvent::ToolCall { name, args } => {
            json!({ "kind": "tool_call", "name": name, "args": args })
        }
        AgentEvent::ToolResult {
            name,
            status,
            output,
            exit_code,
        } => json!({
            "kind": "tool_result",
            "name": name,
            "status": status,
            "output": output,
            "exit_code": exit_code,
        }),
        AgentEvent::Retrying {
            attempt,
            max,
            reason,
        } => json!({
            "kind": "retrying",
            "attempt": attempt,
            "max": max,
            "reason": reason,
        }),
        AgentEvent::Notice(message) => json!({ "kind": "notice", "message": message }),
        AgentEvent::Usage(report) => json!({
            "kind": "usage",
            "turn_tokens": report.turn.normalized_total(),
            "total_tokens": report.total.normalized_total(),
            "prompt_tokens": report.total.prompt_tokens,
            "completion_tokens": report.total.completion_tokens,
            "turn_cost_usd": report.turn_cost_usd,
            "total_cost_usd": report.total_cost_usd,
            "context_tokens": report.context_tokens,
            "context_window": report.context_window,
            "context_percent": report.context_percent(),
        }),
        AgentEvent::Finished { summary, success } => json!({
            "kind": "finished",
            "summary": summary,
            "success": success,
        }),
        AgentEvent::Error(message) => json!({ "kind": "error", "message": message }),
        AgentEvent::TurnFinished => json!({ "kind": "turn_finished" }),
        AgentEvent::SessionChanged {
            id,
            title,
            path,
            events,
        } => json!({
            "kind": "session_changed",
            "id": id,
            "title": title,
            "path": path,
            "events": events.iter().map(session_event_json).collect::<Vec<_>>(),
        }),
    }
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en" data-masse-mode="__MASSE_MODE__">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>masse</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0; font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
         background: #101014; color: #e6e6e6; display: flex; flex-direction: column; height: 100vh; }
  header { padding: 8px 14px; border-bottom: 1px solid #2a2a33; color: #9aa0a6; }
  #log { flex: 1; overflow-y: auto; padding: 12px 14px; white-space: pre-wrap; }
  #log .user { color: #7fd1ff; }
  #log .assistant { color: #8be28b; }
  #log .tool { color: #d6b45a; }
  #log .error { color: #ff6b6b; }
  #log .result { color: #9aa0a6; }
  /* Rendered Markdown: block layout instead of the log's pre-wrap. */
  #log .assistant .who { font-weight: 600; margin-bottom: 2px; }
  #log .md { white-space: normal; color: #e6e6e6; }
  #log .md > :first-child { margin-top: 0; }
  #log .md > :last-child { margin-bottom: 0; }
  #log .md p { margin: 0.4em 0; }
  #log .md h1, #log .md h2, #log .md h3,
  #log .md h4, #log .md h5, #log .md h6 { color: #e6e6e6; line-height: 1.3; margin: 0.6em 0 0.3em; }
  #log .md h1 { font-size: 1.3em; } #log .md h2 { font-size: 1.15em; }
  #log .md code { background: #1a1a22; padding: 1px 5px; border-radius: 4px; color: #d6b45a; }
  #log .md pre { background: #1a1a22; padding: 8px 10px; border-radius: 6px; overflow-x: auto; }
  #log .md pre code { background: none; padding: 0; color: #e6e6e6; }
  #log .md ul, #log .md ol { margin: 0.4em 0; padding-left: 1.5em; }
  #log .md li { margin: 0.1em 0; }
  #log .md blockquote { border-left: 3px solid #2a2a33; margin: 0.4em 0; padding: 0 0 0 10px; color: #9aa0a6; }
  #log .md a { color: #7fd1ff; }
  #log .md hr { border: 0; border-top: 1px solid #2a2a33; margin: 0.6em 0; }
  #log .md table { border-collapse: collapse; margin: 0.4em 0; }
  #log .md th, #log .md td { border: 1px solid #2a2a33; padding: 2px 8px; }
  #log .md img { max-width: 100%; }
  #spinner { color: #d6b45a; }
  #log .working { color: #d6b45a; animation: pulse 1.2s ease-in-out infinite; }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.45; } }
  footer { display: flex; gap: 8px; padding: 10px 14px; border-top: 1px solid #2a2a33; }
  input { flex: 1; background: #1a1a22; border: 1px solid #2a2a33; color: #e6e6e6;
          padding: 8px 10px; border-radius: 6px; }
  button { background: #2d6cdf; color: white; border: 0; padding: 8px 16px; border-radius: 6px; cursor: pointer; }
  button:disabled { background: #333; cursor: default; }
  header select { background: #1a1a22; border: 1px solid #2a2a33; color: #e6e6e6;
                  padding: 2px 6px; border-radius: 5px; max-width: 34ch; }
  header button { padding: 2px 8px; font-size: 12px; }
</style>
</head>
<body>
<header>masse ·
  <select id="sessions" title="Session"></select>
  <button id="newSession" type="button" title="New session">+ new</button>
  <span id="spinner" hidden>⠋</span> <span id="status">connecting…</span> <span id="mode"></span> <span id="usage"></span></header>
<div id="log"></div>
<footer>
  <button id="attach" type="button" title="Attach a spreadsheet">Attach</button>
  <input id="file" type="file" accept=".xls,.xlsx,.xlsb,.ods" hidden>
  <input id="msg" placeholder="Ask masse to do something…" autocomplete="off">
  <button id="send">Send</button>
</footer>
<script>
const logEl = document.getElementById('log');
const statusEl = document.getElementById('status');
const usageEl = document.getElementById('usage');
const inputEl = document.getElementById('msg');
const sendEl = document.getElementById('send');
const spinnerEl = document.getElementById('spinner');
const sessionsEl = document.getElementById('sessions');
const newSessionEl = document.getElementById('newSession');
let ws;
let reconnectDelay = 1000;
let queued = '';
let workingEl = null;
let workingTimer = null;
let workingFrame = 0;
let turnHadOutput = false;
const SPIN = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];

// Keep the "working" indicator pinned to the bottom as new rows arrive.
function pinWorking() {
  if (workingEl) logEl.appendChild(workingEl);
}

function append(cls, text) {
  const div = document.createElement('div');
  div.className = cls;
  div.textContent = text;
  logEl.appendChild(div);
  pinWorking();
  logEl.scrollTop = logEl.scrollHeight;
}

function appendHtml(cls, html) {
  const div = document.createElement('div');
  div.className = cls;
  div.innerHTML = html;
  logEl.appendChild(div);
  pinWorking();
  logEl.scrollTop = logEl.scrollHeight;
}

// The server renders assistant Markdown to HTML, so the browser only injects
// the result. The label sits above the rendered block.
function renderAssistant(html) {
  appendHtml('assistant', '<div class="who">masse</div><div class="md">' + html + '</div>');
}

const DEBUG = document.documentElement.dataset.masseMode === 'debug';
document.getElementById('mode').textContent = DEBUG ? '' : '· user mode';

// In user mode the assistant's last message of a turn is held back and shown
// only when the turn ends, so step-by-step narration and tool noise never
// appear. In debug mode (the default) everything is rendered as it arrives.
let pending = null;

// `pending` holds the rendered HTML of the assistant's last message.
function flushPending() {
  if (pending !== null) { renderAssistant(pending); pending = null; turnHadOutput = true; }
}

// A turn can spend a long time running tools and still produce no text to show
// (user mode hides tool traffic and step-by-step narration). The header spinner
// plus a log line make it obvious that masse is still working.
function setWorking(on) {
  sendEl.disabled = on;
  if (on) {
    if (workingTimer === null) {
      spinnerEl.hidden = false;
      workingTimer = setInterval(() => {
        workingFrame = (workingFrame + 1) % SPIN.length;
        spinnerEl.textContent = SPIN[workingFrame];
      }, 100);
    }
    if (!workingEl) {
      workingEl = document.createElement('div');
      workingEl.className = 'working';
      workingEl.textContent = 'masse está trabajando…';
      logEl.appendChild(workingEl);
    }
  } else {
    if (workingTimer !== null) { clearInterval(workingTimer); workingTimer = null; }
    spinnerEl.hidden = true;
    if (workingEl) { workingEl.remove(); workingEl = null; }
  }
  logEl.scrollTop = logEl.scrollHeight;
}

function renderSessionEvent(ev) {
  if (ev.event_type === 'user_message') append('user', 'you: ' + ev.output);
  else if (ev.event_type === 'assistant_message' && ev.output.trim())
    renderAssistant(ev.html);
  else if (ev.event_type === 'tool_call') append('tool', '  ⚙ ' + ev.tool + ' ' + ev.args);
  else if (ev.event_type === 'tool_result')
    append('result', '  ' + (ev.status === 'ok' ? '✔' : '✘') + ' ' + ev.tool + '\n' + ev.output);
  else if (ev.event_type === 'context_compact')
    append('result', '  ── earlier history compacted (full log kept) ──');
}

function renderHistoryEvent(ev) {
  if (DEBUG) { renderSessionEvent(ev); return; }
  if (ev.event_type === 'user_message') {
    flushPending();
    append('user', 'you: ' + ev.output);
  } else if (ev.event_type === 'assistant_message' && ev.output.trim()) {
    pending = ev.html;
  }
}

function renderLiveEvent(ev) {
  if (ev.kind === 'session_changed') {
    logEl.innerHTML = '';
    pending = null;
    turnHadOutput = false;
    setWorking(false);
    (ev.events || []).forEach(renderHistoryEvent);
    flushPending();
    loadSessions();
    statusEl.textContent = 'connected';
    return;
  }
  if (ev.kind === 'assistant_message') {
    if (DEBUG) renderAssistant(ev.html);
    else if (ev.text.trim()) pending = ev.html;
  } else if (ev.kind === 'user_message') {
    flushPending();
    if (!DEBUG) append('user', 'you: ' + ev.text);
    turnHadOutput = false;
    setWorking(true);
  } else if (ev.kind === 'tool_call') {
    if (DEBUG) append('tool', '  ⚙ ' + ev.name + ' ' + ev.args);
  } else if (ev.kind === 'tool_result') {
    if (DEBUG) append('result', '  ' + (ev.status === 'ok' ? '✔' : '✘') + ' ' + ev.name + '\n' + ev.output);
  } else if (ev.kind === 'retrying') {
    if (DEBUG) append('tool', '  ↻ retry ' + ev.attempt + '/' + ev.max + ' (' + ev.reason + ')');
  } else if (ev.kind === 'notice') {
    if (DEBUG) append('result', '  • ' + ev.message);
  } else if (ev.kind === 'usage') {
    renderUsage(ev);
  } else if (ev.kind === 'finished') {
    setWorking(false);
    if (DEBUG) {
      append(ev.success ? 'assistant' : 'error', (ev.success ? '✔ finished: ' : '✘ failed: ') + ev.summary);
    } else {
      flushPending();
      if (!turnHadOutput) {
        if (ev.success) append('result', '✔ listo');
        else append('error', '✘ ' + (ev.summary || 'no se pudo completar'));
      }
    }
  } else if (ev.kind === 'turn_finished') {
    setWorking(false);
    flushPending();
    if (!DEBUG && !turnHadOutput) append('result', '✔ listo');
  } else if (ev.kind === 'error') {
    setWorking(false);
    pending = null;
    append('error', 'error: ' + ev.message);
  }
}

// Token/cost/context summary in the header, shown in both modes.
function fmtTokens(n) {
  if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}
function fmtCost(usd) {
  if (usd == null) return '—';
  if (usd > 0 && usd < 0.0001) return '<$0.0001';
  return '$' + usd.toFixed(4);
}
function renderUsage(ev) {
  const parts = ['tok ' + fmtTokens(ev.total_tokens)];
  if (ev.total_cost_usd != null) {
    parts.push('turn ' + fmtCost(ev.turn_cost_usd));
    parts.push('total ' + fmtCost(ev.total_cost_usd));
  }
  if (ev.context_percent != null) {
    parts.push('ctx ' + fmtTokens(ev.context_tokens) + '/' + fmtTokens(ev.context_window) +
               ' (' + ev.context_percent.toFixed(0) + '%)');
  }
  usageEl.textContent = '· ' + parts.join(' · ');
}

// --- Sessions -------------------------------------------------------------
// The dropdown is the whole session UI: pick one to resume it, or create a
// fresh one. The server broadcasts `session_changed` with the new transcript.
async function loadSessions() {
  try {
    const res = await fetch('/api/sessions');
    if (!res.ok) return;
    const data = await res.json();
    sessionsEl.innerHTML = '';
    (data.sessions || []).forEach((s) => {
      const option = document.createElement('option');
      option.value = s.id;
      option.textContent = (s.title || s.id) + ' · ' + s.id;
      if (s.id === data.active) option.selected = true;
      sessionsEl.appendChild(option);
    });
    if (!sessionsEl.options.length) {
      const option = document.createElement('option');
      option.value = data.active || '';
      option.textContent = 'new session';
      sessionsEl.appendChild(option);
    }
  } catch (_) {}
}

sessionsEl.onchange = async () => {
  statusEl.textContent = 'switching…';
  await fetch('/api/sessions/activate', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: sessionsEl.value }),
  }).catch(() => {});
};

newSessionEl.onclick = async () => {
  statusEl.textContent = 'creating…';
  await fetch('/api/sessions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: '{}',
  }).catch(() => {});
};

function connect() {
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
  ws.onopen = () => {
    statusEl.textContent = 'connected';
    reconnectDelay = 1000;
    // The server replays the whole session on every connect, so start clean.
    logEl.innerHTML = '';
    pending = null;
    turnHadOutput = false;
    setWorking(false);
    loadSessions();
    if (queued !== null && queued !== '') {
      const text = queued;
      queued = '';
      ws.send(JSON.stringify({ type: 'user_message', text }));
    }
  };
  ws.onclose = () => {
    statusEl.textContent = 'disconnected — reconnecting…';
    setWorking(false);
    setTimeout(connect, reconnectDelay);
    reconnectDelay = Math.min(reconnectDelay * 2, 10000);
  };
  ws.onerror = () => { try { ws.close(); } catch (_) {} };
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.type === 'history') { msg.events.forEach(renderHistoryEvent); flushPending(); }
    else if (msg.type === 'event') renderLiveEvent(msg.event);
  };
}

function send() {
  const text = inputEl.value.trim();
  if (!text) return;
  inputEl.value = '';
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    // Not connected yet: hold the message and deliver it once we reconnect.
    statusEl.textContent = 'reconnecting…';
    queued = queued ? queued + '\n' + text : text;
    return;
  }
  ws.send(JSON.stringify({ type: 'user_message', text }));
  setWorking(true);
}
sendEl.onclick = send;
inputEl.addEventListener('keydown', (e) => { if (e.key === 'Enter') send(); });

// Attach: upload the file into the project (`.masse/uploads/`) and leave a
// path reference in the composer. The agent reads it with read_spreadsheet.
const fileEl = document.getElementById('file');
const attachEl = document.getElementById('attach');
attachEl.onclick = () => fileEl.click();
fileEl.onchange = async () => {
  const file = fileEl.files[0];
  if (!file) return;
  statusEl.textContent = 'uploading…';
  try {
    const res = await fetch('/api/upload?name=' + encodeURIComponent(file.name), {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: file,
    });
    const info = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(info.error || res.statusText);
    const ref = '[attached file: ' + info.path + '] ';
    inputEl.value = inputEl.value ? inputEl.value + ' ' + ref : ref;
    inputEl.focus();
    statusEl.textContent = 'connected';
  } catch (e) {
    statusEl.textContent = 'upload failed: ' + e.message;
  } finally {
    fileEl.value = '';
  }
};

connect();
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_events_expose_tokens_cost_and_context() {
        let value = agent_event_json(&AgentEvent::Usage(harness_core::UsageReport {
            total: harness_core::TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 20,
                total_tokens: 120,
            },
            context_tokens: 64,
            context_window: Some(128),
            turn_cost_usd: Some(0.001),
            total_cost_usd: Some(0.002),
            ..Default::default()
        }));
        assert_eq!(value["kind"], "usage");
        assert_eq!(value["total_tokens"], 120);
        assert_eq!(value["turn_cost_usd"], 0.001);
        assert_eq!(value["context_percent"], 50.0);
    }

    #[test]
    fn assistant_events_carry_rendered_html() {
        let value = agent_event_json(&AgentEvent::AssistantMessage("**hi**".into()));
        assert_eq!(value["kind"], "assistant_message");
        assert_eq!(value["text"], "**hi**");
        assert_eq!(value["html"], "<p><strong>hi</strong></p>");
    }

    #[test]
    fn assistant_session_events_carry_rendered_html() {
        let event = SessionEvent {
            event_type: "assistant_message".into(),
            output: "# Title".into(),
            ..Default::default()
        };
        let value = session_event_json(&event);
        assert_eq!(value["html"], "<h1>Title</h1>");
    }

    #[test]
    fn other_events_have_no_html() {
        let event = SessionEvent {
            event_type: "user_message".into(),
            output: "hi".into(),
            ..Default::default()
        };
        assert_eq!(session_event_json(&event)["html"], "");
    }

    #[test]
    fn turn_state_tracks_every_terminal_outcome() {
        let turn = Arc::new(std::sync::Mutex::new(TurnState::default()));

        apply_turn_event(&turn, &AgentEvent::UserMessage("go".into()));
        {
            let state = turn.lock().unwrap();
            assert!(state.running);
            assert_eq!(state.outcome, "none");
        }

        apply_turn_event(
            &turn,
            &AgentEvent::Finished {
                summary: "done".into(),
                success: true,
            },
        );
        {
            let state = turn.lock().unwrap();
            assert!(!state.running);
            assert_eq!(state.outcome, "finished");
            assert_eq!(state.success, Some(true));
        }

        apply_turn_event(&turn, &AgentEvent::UserMessage("again".into()));
        apply_turn_event(&turn, &AgentEvent::TurnFinished);
        {
            let state = turn.lock().unwrap();
            assert!(!state.running);
            assert_eq!(state.outcome, "stopped", "a turn without finish is visible");
        }

        apply_turn_event(&turn, &AgentEvent::UserMessage("again".into()));
        apply_turn_event(&turn, &AgentEvent::Error("turn cancelled".into()));
        {
            let state = turn.lock().unwrap();
            assert_eq!(state.outcome, "cancelled");
        }
    }

    #[test]
    fn page_ships_the_markdown_renderer() {
        assert!(INDEX_HTML.contains("renderAssistant"));
        assert!(INDEX_HTML.contains(".md"));
    }

    #[test]
    fn page_ships_the_working_indicator() {
        assert!(INDEX_HTML.contains("id=\"spinner\""));
        assert!(INDEX_HTML.contains("setWorking"));
        assert!(INDEX_HTML.contains("trabajando"));
    }

    #[test]
    fn page_ships_the_session_picker() {
        assert!(INDEX_HTML.contains("id=\"sessions\""));
        assert!(INDEX_HTML.contains("newSession"));
        assert!(INDEX_HTML.contains("session_changed"));
        assert!(INDEX_HTML.contains("/api/sessions/activate"));
    }

    #[test]
    fn session_changed_events_carry_the_new_transcript() {
        let event = AgentEvent::SessionChanged {
            id: "20261022-120000-0000".into(),
            title: "guestbook".into(),
            path: "/tmp/guestbook.csv".into(),
            events: vec![SessionEvent {
                event_type: "user_message".into(),
                output: "hola".into(),
                ..Default::default()
            }],
        };
        let value = agent_event_json(&event);
        assert_eq!(value["kind"], "session_changed");
        assert_eq!(value["id"], "20261022-120000-0000");
        assert_eq!(value["title"], "guestbook");
        assert_eq!(value["events"][0]["output"], "hola");
    }
}
