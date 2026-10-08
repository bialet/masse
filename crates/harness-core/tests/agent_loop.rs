//! End-to-end test of the agent loop: a scripted provider asks the agent to
//! call `write_file`, then finishes. Verifies tool dispatch and CSV logging.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use harness_core::{
    Agent, AgentConfig, AgentEvent, AgentHandle, ChatRequest, ChatResponse, Provider,
};
use harness_session::SessionStore;
use harness_tools::{builtin_registry, ToolContext};
use tokio::sync::broadcast;

struct ScriptedProvider {
    responses: Mutex<VecDeque<ChatResponse>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }
}

#[tokio::test]
async fn agent_runs_a_tool_then_finishes() {
    let root = std::env::temp_dir().join(format!("masse-loop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![harness_core::ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"hi.txt","content":"hola"}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
    });
    responses.push_back(ChatResponse {
        content: "done".into(),
        tool_calls: vec![],
        finish_reason: Some("stop".into()),
    });

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let config = AgentConfig {
        model: "test".into(),
        system_prompt: "test system prompt".into(),
        ..Default::default()
    };

    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session.clone(),
        config,
    )
    .await
    .unwrap();

    let reply = agent.send("please create hi.txt", None).await.unwrap();
    assert_eq!(reply, "done");

    let written = std::fs::read_to_string(root.join("hi.txt")).unwrap();
    assert_eq!(written, "hola");

    let events = session.read_all().await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(kinds.contains(&"session_start"));
    assert!(kinds.contains(&"user_message"));
    assert!(kinds.contains(&"tool_call"));
    assert!(kinds.contains(&"tool_result"));

    let result = events
        .iter()
        .find(|e| e.event_type == "tool_result")
        .unwrap();
    assert_eq!(result.tool, "write_file");
    assert_eq!(result.status, "ok");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn path_escape_is_rejected() {
    let root = std::env::temp_dir().join(format!("masse-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let ctx = ToolContext::new(&root);
    assert!(ctx.resolve("../../etc/passwd").is_err());
    assert!(ctx.resolve("sub/dir/file.txt").is_ok());

    let _ = std::fs::remove_dir_all(&root);
}

/// A provider that fails immediately.
struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    fn name(&self) -> &str {
        "failing"
    }
    async fn complete(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
        anyhow::bail!("boom")
    }
}

/// A provider that never answers.
struct HangingProvider;

#[async_trait]
impl Provider for HangingProvider {
    fn name(&self) -> &str {
        "hanging"
    }
    async fn complete(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        unreachable!()
    }
}

async fn build_agent(provider: Arc<dyn Provider>, root: &std::path::Path) -> Agent {
    Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(root),
        Arc::new(SessionStore::new(root.join("session.csv"))),
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn provider_error_always_emits_a_terminal_event() {
    let root = std::env::temp_dir().join(format!("masse-err-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut agent = build_agent(Arc::new(FailingProvider), &root).await;
    let (tx, mut rx) = broadcast::channel(16);
    let result = agent.send("hi", Some(&tx)).await;
    assert!(result.is_err());

    let mut saw_error = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, AgentEvent::Error(_)) {
            saw_error = true;
        }
    }
    assert!(
        saw_error,
        "an Error event must be emitted on provider failure"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn cancel_aborts_a_hanging_turn() {
    let root = std::env::temp_dir().join(format!("masse-cancel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let agent = build_agent(Arc::new(HangingProvider), &root).await;
    let handle = AgentHandle::spawn(agent);
    let mut rx = handle.subscribe();

    assert!(handle.send_user("hi"));

    let started = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(event) = rx.recv().await {
            if matches!(event, AgentEvent::UserMessage(_)) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(started, "turn should have started");

    handle.cancel();
    let cancelled = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(event) = rx.recv().await {
            if let AgentEvent::Error(message) = event {
                if message.contains("cancel") {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(cancelled, "cancel must produce an Error event");
    handle.shutdown();

    let _ = std::fs::remove_dir_all(&root);
}
