//! End-to-end test of the agent loop: a scripted provider asks the agent to
//! call `write_file`, then finishes. Verifies tool dispatch and CSV logging.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use harness_core::{Agent, AgentConfig, ChatRequest, ChatResponse, Provider};
use harness_session::SessionStore;
use harness_tools::{builtin_registry, ToolContext};

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
