//! End-to-end test of the agent loop: a scripted provider asks the agent to
//! call `write_file`, then finishes. Verifies tool dispatch and CSV logging.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use harness_core::{
    Agent, AgentConfig, AgentEvent, AgentHandle, ChatRequest, ChatResponse, Provider,
    ProviderError, ProviderResult, ToolCall,
};
use harness_session::{SessionManager, SessionStore};
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

    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
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
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: "done".into(),
        tool_calls: vec![],
        finish_reason: Some("stop".into()),
        usage: Default::default(),
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
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        Err(ProviderError::Permanent("boom".into()))
    }
}

/// A provider that never answers.
struct HangingProvider;

#[async_trait]
impl Provider for HangingProvider {
    fn name(&self) -> &str {
        "hanging"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        unreachable!()
    }
}

/// A provider that fails transiently a few times, then succeeds.
struct RetryThenOk {
    failures: usize,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for RetryThenOk {
    fn name(&self) -> &str {
        "retry"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.failures {
            Err(ProviderError::Retryable(format!("transient {n}")))
        } else {
            Ok(ChatResponse {
                content: "ok".into(),
                ..Default::default()
            })
        }
    }
}

/// A provider that asks to call the `finish` tool.
struct FinishProvider;

#[async_trait]
impl Provider for FinishProvider {
    fn name(&self) -> &str {
        "finish"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        Ok(ChatResponse {
            tool_calls: vec![ToolCall {
                id: "f1".into(),
                name: "finish".into(),
                arguments: r#"{"summary":"all done","success":true}"#.into(),
            }],
            ..Default::default()
        })
    }
}

/// A provider that always asks for a tool, to exercise the iteration cap.
struct LoopingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for LoopingProvider {
    fn name(&self) -> &str {
        "looping"
    }
    async fn complete(&self, request: ChatRequest) -> ProviderResult<ChatResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.tools.is_empty() {
            // The wrap-up call: no tools offered, so answer in text.
            Ok(ChatResponse {
                content: "wrapped up".into(),
                ..Default::default()
            })
        } else {
            Ok(ChatResponse {
                tool_calls: vec![ToolCall {
                    id: "t".into(),
                    name: "list_dir".into(),
                    arguments: "{}".into(),
                }],
                ..Default::default()
            })
        }
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
    let sessions = SessionManager::load(&root, None).await.unwrap();
    let handle = AgentHandle::spawn(agent, sessions);
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

#[tokio::test]
async fn retries_transient_errors_then_succeeds() {
    let root = std::env::temp_dir().join(format!("masse-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(RetryThenOk {
        failures: 2,
        calls: calls.clone(),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session,
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            max_retries: 5,
            retry_delay_ms: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (tx, mut rx) = broadcast::channel(64);
    let reply = agent.send("hi", Some(&tx)).await.unwrap();
    assert_eq!(reply, "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "two retries then success");

    let mut saw_retry = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, AgentEvent::Retrying { .. }) {
            saw_retry = true;
        }
    }
    assert!(saw_retry, "a Retrying event must be emitted");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn finish_tool_ends_the_turn() {
    let root = std::env::temp_dir().join(format!("masse-finish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut agent = build_agent(Arc::new(FinishProvider), &root).await;
    let (tx, mut rx) = broadcast::channel(64);
    let reply = agent.send("do it", Some(&tx)).await.unwrap();
    assert_eq!(reply, "all done");

    let mut finished = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Finished { summary, success } = event {
            finished = Some((summary, success));
        }
    }
    assert_eq!(finished, Some(("all done".to_string(), true)));

    let events = agent.session().read_all().await.unwrap();
    assert!(events
        .iter()
        .any(|e| e.event_type == "tool_result" && e.tool == "finish"));

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn iteration_cap_wraps_up_instead_of_failing() {
    let root = std::env::temp_dir().join(format!("masse-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(LoopingProvider {
        calls: calls.clone(),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session,
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            max_iterations: 3,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (tx, mut rx) = broadcast::channel(128);
    let reply = agent.send("go", Some(&tx)).await.unwrap();
    assert_eq!(reply, "wrapped up");
    assert!(
        calls.load(Ordering::SeqCst) >= 4,
        "3 capped iterations plus one wrap-up call"
    );

    let mut saw_notice = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, AgentEvent::Notice(_)) {
            saw_notice = true;
        }
    }
    assert!(
        saw_notice,
        "a Notice must be emitted when the cap is reached"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_wren_change_is_nudged_before_the_model_can_stop() {
    let root = std::env::temp_dir().join(format!("masse-verify-stop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"app.wren","content":"// app"}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    // The model tries to stop with a summary right after editing.
    responses.push_back(ChatResponse {
        content: "listo, ya está".into(),
        tool_calls: vec![],
        finish_reason: Some("stop".into()),
        usage: Default::default(),
    });
    // After the syntax nudge it tries to finish; after the test nudge it is
    // finally allowed.
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c2".into(),
            name: "finish".into(),
            arguments: r#"{"summary":"primero","success":true}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c3".into(),
            name: "finish".into(),
            arguments: r#"{"summary":"verificado","success":true}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let mut agent = build_agent(provider, &root).await;
    let (tx, mut rx) = broadcast::channel(64);

    let reply = agent.send("cambiá app.wren", Some(&tx)).await.unwrap();
    assert_eq!(
        reply, "verificado",
        "the turn must not end until syntax is checked and the change is verified"
    );

    let mut saw_syntax = false;
    let mut saw_verify = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Notice(message) = event {
            if message.contains("checking syntax") {
                saw_syntax = true;
            }
            if message.contains("verifying") {
                saw_verify = true;
            }
        }
    }
    assert!(saw_syntax, "a syntax-check Notice must be emitted");
    assert!(saw_verify, "a verification Notice must be emitted");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_question_after_a_wren_change_is_not_nudged() {
    let root = std::env::temp_dir().join(format!("masse-verify-q-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"app.wren","content":"// app"}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: "¿Preferís A o B?".into(),
        tool_calls: vec![],
        finish_reason: Some("stop".into()),
        usage: Default::default(),
    });

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let mut agent = build_agent(provider, &root).await;

    let reply = agent.send("cambiá app.wren", None).await.unwrap();
    assert_eq!(reply, "¿Preferís A o B?");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn finish_after_a_wren_change_is_held_for_syntax_then_tests() {
    let root = std::env::temp_dir().join(format!("masse-verify-finish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"app.wren","content":"// app"}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    // Three finishes: syntax nudge, then test nudge, then allowed.
    for (id, summary) in [("c2", "primero"), ("c3", "segundo"), ("c4", "tercero")] {
        responses.push_back(ChatResponse {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: "finish".into(),
                arguments: format!(r#"{{"summary":"{summary}","success":true}}"#),
            }],
            finish_reason: Some("tool_calls".into()),
            usage: Default::default(),
        });
    }

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let mut agent = build_agent(provider, &root).await;

    let reply = agent.send("cambiá app.wren", None).await.unwrap();
    assert_eq!(
        reply, "tercero",
        "the early finishes are held back; the one after both nudges is allowed"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn identical_repeated_calls_are_skipped() {
    let root = std::env::temp_dir().join(format!("masse-repeat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(RepeatingProvider {
        calls: calls.clone(),
    });
    let mut agent = build_agent(provider, &root).await;
    let (tx, mut rx) = broadcast::channel(256);

    let reply = agent.send("go", Some(&tx)).await.unwrap();
    assert_eq!(reply, "stopped");

    let events = agent.session().read_all().await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "tool_result" && e.output.contains("skipped by the harness")),
        "the third identical call must be skipped"
    );

    let mut saw_notice = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Notice(message) = event {
            if message.contains("skipping") {
                saw_notice = true;
            }
        }
    }
    assert!(saw_notice, "a Notice must be emitted when a loop is broken");

    let _ = std::fs::remove_dir_all(&root);
}

/// A provider that issues the same tool call over and over, then stops.
struct RepeatingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for RepeatingProvider {
    fn name(&self) -> &str {
        "repeating"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < 4 {
            Ok(ChatResponse {
                tool_calls: vec![ToolCall {
                    id: format!("c{n}"),
                    name: "list_dir".into(),
                    arguments: "{}".into(),
                }],
                ..Default::default()
            })
        } else {
            Ok(ChatResponse {
                content: "stopped".into(),
                ..Default::default()
            })
        }
    }
}

#[tokio::test]
async fn auto_commit_commits_after_a_turn() {
    let git_ok = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !git_ok {
        return;
    }

    let root = std::env::temp_dir().join(format!("masse-autocommit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![harness_core::ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"f.txt","content":"hi"}"#.into(),
        }],
        finish_reason: None,
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: "done".into(),
        tool_calls: vec![],
        finish_reason: None,
        usage: Default::default(),
    });
    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session,
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            auto_commit: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let reply = agent.send("create f.txt", None).await.unwrap();
    assert_eq!(reply, "done");
    assert!(root.join(".git").exists(), "git repo should be created");
    let log = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(log.status.success());
    assert!(
        !log.stdout.is_empty(),
        "the turn's changes should have been committed"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A provider whose read_file calls fail the same way but with different
/// arguments, mimicking a model that keeps retrying with cosmetic changes.
struct StuckFailingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for StuckFailingProvider {
    fn name(&self) -> &str {
        "stuck"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < 4 {
            Ok(ChatResponse {
                tool_calls: vec![ToolCall {
                    id: format!("c{n}"),
                    name: "read_file".into(),
                    arguments: format!(r#"{{"path":"missing{n}.txt"}}"#),
                }],
                ..Default::default()
            })
        } else {
            Ok(ChatResponse {
                content: "stopped".into(),
                ..Default::default()
            })
        }
    }
}

#[tokio::test]
async fn repeated_failures_with_varying_args_nudge_a_change_of_approach() {
    let root = std::env::temp_dir().join(format!("masse-stuck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(StuckFailingProvider {
        calls: calls.clone(),
    });
    let mut agent = build_agent(provider, &root).await;
    let (tx, mut rx) = broadcast::channel(64);

    let reply = agent.send("go", Some(&tx)).await.unwrap();
    assert_eq!(reply, "stopped");

    let mut saw_notice = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Notice(message) = event {
            if message.contains("change approach") {
                saw_notice = true;
            }
        }
    }
    assert!(
        saw_notice,
        "the same failure repeating with differing args must trigger a Notice"
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.content.contains("keeps failing in the same way")),
        "a change-of-approach nudge must reach the conversation"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn invalid_json_arguments_are_reported_honestly() {
    let root = std::env::temp_dir().join(format!("masse-badjson-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            // A completion cut off mid-string: invalid JSON.
            arguments: r#"{"path":"hi.txt","content":"hola"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_2".into(),
            name: "finish".into(),
            arguments: r#"{"summary":"blocked","success":false}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session.clone(),
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let reply = agent.send("build it", None).await.unwrap();
    assert_eq!(reply, "blocked");
    assert!(
        !root.join("hi.txt").exists(),
        "an invalid call must not execute"
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.content.contains("not valid JSON")),
        "the model must be told its arguments were invalid"
    );
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.content.contains("missing required")),
        "a truncated payload must not masquerade as a missing argument"
    );

    let events = session.read_all().await.unwrap();
    let call = events
        .iter()
        .find(|e| e.event_type == "tool_call")
        .expect("the call is logged");
    assert!(call.args.contains("_invalid_json=true"), "{}", call.args);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn finish_success_without_artifacts_is_rejected_once() {
    let root = std::env::temp_dir().join(format!("masse-noartifact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "finish".into(),
            arguments: r#"{"summary":"listo","success":true}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    responses.push_back(ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_2".into(),
            name: "finish".into(),
            arguments: r#"{"summary":"necesito una decisión","success":false}"#.into(),
        }],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
    });
    let mut agent = build_agent(provider, &root).await;
    let reply = agent.send("build it", None).await.unwrap();

    assert_eq!(reply, "necesito una decisión");
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.content.contains("did not create or edit any file")),
        "the false success must be challenged"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_probe_loop_is_aborted_instead_of_burning_the_budget() {
    let root = std::env::temp_dir().join(format!("masse-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(ProbeLoopProvider {
        calls: calls.clone(),
    });
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let session2 = session.clone();
    let mut agent = Agent::new(
        provider,
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session,
        AgentConfig {
            model: "test".into(),
            system_prompt: "test".into(),
            max_iterations: 40,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let text = agent
        .send("build it", None)
        .await
        .expect("the probe loop ends with a stopped outcome, not an Err");
    let events = session2.read_all().await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "tool_call")
            .count(),
        10,
        "the guard should stop at the tenth call"
    );
    assert!(
        text.contains("probe loop") || text.contains("same arguments"),
        "unexpected stop reason: {text}"
    );
    assert!(
        calls.load(Ordering::SeqCst) <= 12,
        "the loop should stop early, got {} model calls",
        calls.load(Ordering::SeqCst)
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Writes `probe1.txt`, `probe2.txt`, ... on every call: same shape, changed
/// constant, which is exactly the loop seen in the failed runs.
struct ProbeLoopProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for ProbeLoopProvider {
    fn name(&self) -> &str {
        "probe-loop"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ChatResponse {
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "write_file".into(),
                arguments: format!(r#"{{"path":"probe{n}.txt","content":"x"}}"#),
            }],
            ..Default::default()
        })
    }
}

/// Reports a fixed usage block so the accounting, pricing and context paths can
/// be asserted end to end.
struct UsageProvider;

#[async_trait]
impl Provider for UsageProvider {
    fn name(&self) -> &str {
        "usage"
    }
    async fn complete(&self, _request: ChatRequest) -> ProviderResult<ChatResponse> {
        Ok(ChatResponse {
            content: "listo".into(),
            usage: harness_core::TokenUsage {
                prompt_tokens: 64_000,
                completion_tokens: 1_000_000,
                total_tokens: 1_064_000,
            },
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn usage_event_reports_tokens_cost_and_context() {
    let root = std::env::temp_dir().join(format!("masse-usage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    let config = AgentConfig {
        model: "deepseek-chat".into(),
        system_prompt: "s".into(),
        ..Default::default()
    };
    let mut agent = Agent::new(
        Arc::new(UsageProvider),
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session,
        config,
    )
    .await
    .unwrap();

    let (tx, mut rx) = broadcast::channel(64);
    agent.send("hola", Some(&tx)).await.unwrap();
    drop(tx);

    let mut usage = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Usage(report) = event {
            usage = Some(report);
        }
    }
    let report = usage.expect("a Usage event must be emitted");
    assert_eq!(report.turn.normalized_total(), 1_064_000);
    assert_eq!(report.total.normalized_total(), 1_064_000);
    // deepseek-chat: 0.064M in at $0.27 + 1.0M out at $1.10.
    let expected = 0.064 * 0.27 + 1.0 * 1.10;
    assert!(
        (report.turn_cost_usd.unwrap() - expected).abs() < 1e-9,
        "cost was {:?}",
        report.turn_cost_usd
    );
    assert_eq!(report.context_tokens, 64_000);
    assert_eq!(report.context_window, Some(128_000));
    assert_eq!(report.context_percent(), Some(50.0));

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_full_context_is_compacted_without_losing_the_raw_log() {
    let root = std::env::temp_dir().join(format!(
        "masse-compact-loop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // Pre-fill the log so the resumed context is over the threshold (1% of
    // deepseek-chat's 128k window).
    let session = Arc::new(SessionStore::new(root.join("session.csv")));
    for i in 0..8 {
        session
            .append(&harness_session::Event::user_message(&format!(
                "request {i} {}",
                "detail ".repeat(120)
            )))
            .await
            .unwrap();
        session
            .append(&harness_session::Event::assistant_message(&format!(
                "answer {i} {}",
                "result ".repeat(120)
            )))
            .await
            .unwrap();
    }
    let before = session.read_all().await.unwrap().len();

    let mut responses = VecDeque::new();
    // First call is the summarizer, second is the turn itself.
    responses.push_back(ChatResponse {
        content: "Summary: the user asked for eight things and all were done.".into(),
        ..Default::default()
    });
    responses.push_back(ChatResponse {
        content: "done".into(),
        ..Default::default()
    });

    let agent = Agent::new(
        Arc::new(ScriptedProvider {
            responses: Mutex::new(responses),
        }),
        Arc::new(builtin_registry()),
        ToolContext::new(&root),
        session.clone(),
        AgentConfig {
            model: "deepseek-chat".into(),
            system_prompt: "s".into(),
            compact_at_percent: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut agent = agent;
    let reply = agent.send("one more", None).await.unwrap();
    assert_eq!(reply, "done");

    let events = session.read_all().await.unwrap();
    // The raw conversation is still there: nothing was deleted.
    assert!(events.len() >= before + 3);
    let compact = events
        .iter()
        .position(|e| e.event_type == "context_compact")
        .expect("a context_compact event must be written");
    let last_user = events
        .iter()
        .rposition(|e| e.event_type == "user_message")
        .unwrap();
    assert!(
        compact < last_user,
        "the summary must precede the new message"
    );
    assert!(events[compact].output.contains("eight things"));
    assert_eq!(events[compact].args, "method=model|covered_events=16");

    // Replaying the log starts from the summary instead of all 16 events.
    let replay = harness_core::rebuild_messages(&events, "s");
    assert!(replay[1].content.contains("eight things"));
    assert!(!replay.iter().any(|m| m.content.contains("request 0")));

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn switching_to_a_fresh_session_emits_an_empty_transcript() {
    let root = std::env::temp_dir().join(format!(
        "masse-switch-fresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let sessions = SessionManager::load(&root, None).await.unwrap();
    let agent = build_agent(Arc::new(FailingProvider), &root).await;
    let handle = AgentHandle::spawn(agent, sessions.clone());
    let mut rx = handle.subscribe();

    let fresh = sessions.create(None).await.unwrap();
    assert!(!fresh.path.exists(), "the new session has no log yet");
    assert!(handle.switch_session(fresh.id.clone()));

    let events = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(AgentEvent::SessionChanged { id, events, .. }) = rx.recv().await {
                if id == fresh.id {
                    return Some(events);
                }
            }
        }
    })
    .await
    .expect("the actor must answer the switch")
    .expect("the switch must not fail");

    assert!(events.is_empty(), "a new session starts with an empty view");
    handle.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
