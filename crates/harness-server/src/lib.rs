//! The HTTP/WebSocket front end (axum).
//!
//! The server exposes a tiny chat UI plus a JSON API and a WebSocket that
//! streams [`harness_core::AgentEvent`]s. It shares the exact same
//! `session.csv` as the TUI, so a session started in one front end can be
//! followed from the other.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use harness_core::{AgentEvent, AgentHandle};
use harness_session::{Event as SessionEvent, SessionStore};
use harness_tools::ToolRegistry;
use serde_json::{json, Value as JsonValue};
use tokio::sync::broadcast::error::RecvError;

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    pub handle: AgentHandle,
    pub session: Arc<SessionStore>,
    pub tools: Arc<ToolRegistry>,
    pub provider_name: String,
    pub model: String,
    pub root: String,
    /// Path to the human-readable runtime log, if logging is enabled.
    pub log_path: Option<PathBuf>,
    /// Developer mode: the UI shows the full log. Off by default.
    pub debug: bool,
}

/// Bind `addr` and serve until the process is stopped.
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/tools", get(list_tools))
        .route("/api/session", get(session_events))
        .route("/api/log", get(log_tail))
        .route("/api/message", post(post_message))
        .route("/ws", get(ws_handler))
        .with_state(state);

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
    let events = state.session.read_all().await.unwrap_or_default();
    // The most recent `finish` tool result tells whether the task completed.
    let finished = events
        .iter()
        .rev()
        .find(|e| e.event_type == "tool_result" && e.tool == "finish")
        .map(|e| e.status == "ok");
    Json(json!({
        "session": state.session.path().display().to_string(),
        "finished": finished,
        "events": events.iter().map(session_event_json).collect::<Vec<_>>(),
    }))
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
    let ok = state.handle.send_user(text);
    if ok {
        (
            axum::http::StatusCode::ACCEPTED,
            Json(json!({ "status": "queued" })),
        )
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "agent actor stopped" })),
        )
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.handle.subscribe();

    // Replay history first so a late client sees the whole session.
    if let Ok(history) = state.session.read_all().await {
        let payload = json!({
            "type": "history",
            "session": state.session.path().display().to_string(),
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
    json!({
        "timestamp": ev.timestamp,
        "event_type": ev.event_type,
        "role": ev.role,
        "tool": ev.tool,
        "args": ev.args,
        "status": ev.status,
        "output": ev.output,
        "exit_code": ev.exit_code,
    })
}

fn agent_event_json(ev: &AgentEvent) -> JsonValue {
    match ev {
        AgentEvent::UserMessage(text) => json!({ "kind": "user_message", "text": text }),
        AgentEvent::AssistantMessage(text) => {
            json!({ "kind": "assistant_message", "text": text })
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
        AgentEvent::Finished { summary, success } => json!({
            "kind": "finished",
            "summary": summary,
            "success": success,
        }),
        AgentEvent::Error(message) => json!({ "kind": "error", "message": message }),
        AgentEvent::TurnFinished => json!({ "kind": "turn_finished" }),
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
  footer { display: flex; gap: 8px; padding: 10px 14px; border-top: 1px solid #2a2a33; }
  input { flex: 1; background: #1a1a22; border: 1px solid #2a2a33; color: #e6e6e6;
          padding: 8px 10px; border-radius: 6px; }
  button { background: #2d6cdf; color: white; border: 0; padding: 8px 16px; border-radius: 6px; cursor: pointer; }
  button:disabled { background: #333; cursor: default; }
</style>
</head>
<body>
<header>masse · <span id="status">connecting…</span> <span id="mode"></span></header>
<div id="log"></div>
<footer>
  <input id="msg" placeholder="Ask masse to do something…" autocomplete="off">
  <button id="send">Send</button>
</footer>
<script>
const logEl = document.getElementById('log');
const statusEl = document.getElementById('status');
const inputEl = document.getElementById('msg');
const sendEl = document.getElementById('send');
let ws;

function append(cls, text) {
  const div = document.createElement('div');
  div.className = cls;
  div.textContent = text;
  logEl.appendChild(div);
  logEl.scrollTop = logEl.scrollHeight;
}

const DEBUG = document.documentElement.dataset.masseMode === 'debug';
document.getElementById('mode').textContent = DEBUG ? '· debug' : '';

// In user mode (the default) the assistant's last message of a turn is held
// back and shown only when the turn ends, so step-by-step narration and tool
// noise never appear. In debug mode everything is rendered as it arrives.
let pending = null;

function flushPending() {
  if (pending !== null) { append('assistant', 'masse: ' + pending); pending = null; }
}

function renderSessionEvent(ev) {
  if (ev.event_type === 'user_message') append('user', 'you: ' + ev.output);
  else if (ev.event_type === 'assistant_message' && ev.output.trim())
    append('assistant', 'masse: ' + ev.output);
  else if (ev.event_type === 'tool_call') append('tool', '  ⚙ ' + ev.tool + ' ' + ev.args);
  else if (ev.event_type === 'tool_result')
    append('result', '  ' + (ev.status === 'ok' ? '✔' : '✘') + ' ' + ev.tool + '\n' + ev.output);
}

function renderHistoryEvent(ev) {
  if (DEBUG) { renderSessionEvent(ev); return; }
  if (ev.event_type === 'user_message') {
    flushPending();
    append('user', 'you: ' + ev.output);
  } else if (ev.event_type === 'assistant_message' && ev.output.trim()) {
    pending = ev.output;
  }
}

function renderLiveEvent(ev) {
  if (ev.kind === 'assistant_message') {
    if (DEBUG) append('assistant', 'masse: ' + ev.text);
    else if (ev.text.trim()) pending = ev.text;
  } else if (ev.kind === 'user_message') {
    flushPending();
  } else if (ev.kind === 'tool_call') {
    if (DEBUG) append('tool', '  ⚙ ' + ev.name + ' ' + ev.args);
  } else if (ev.kind === 'tool_result') {
    if (DEBUG) append('result', '  ' + (ev.status === 'ok' ? '✔' : '✘') + ' ' + ev.name + '\n' + ev.output);
  } else if (ev.kind === 'retrying') {
    if (DEBUG) append('tool', '  ↻ retry ' + ev.attempt + '/' + ev.max + ' (' + ev.reason + ')');
  } else if (ev.kind === 'notice') {
    if (DEBUG) append('result', '  • ' + ev.message);
  } else if (ev.kind === 'finished') {
    if (DEBUG) append(ev.success ? 'assistant' : 'error', (ev.success ? '✔ finished: ' : '✘ failed: ') + ev.summary);
    else flushPending();
  } else if (ev.kind === 'turn_finished') {
    flushPending();
  } else if (ev.kind === 'error') {
    pending = null;
    append('error', 'error: ' + ev.message);
  }
}

function connect() {
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
  ws.onopen = () => { statusEl.textContent = 'connected'; };
  ws.onclose = () => { statusEl.textContent = 'disconnected — reload to retry'; };
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.type === 'history') { msg.events.forEach(renderHistoryEvent); flushPending(); }
    else if (msg.type === 'event') renderLiveEvent(msg.event);
  };
}

function send() {
  const text = inputEl.value.trim();
  if (!text || !ws || ws.readyState !== WebSocket.OPEN) return;
  ws.send(JSON.stringify({ type: 'user_message', text }));
  inputEl.value = '';
}
sendEl.onclick = send;
inputEl.addEventListener('keydown', (e) => { if (e.key === 'Enter') send(); });
connect();
</script>
</body>
</html>
"#;
