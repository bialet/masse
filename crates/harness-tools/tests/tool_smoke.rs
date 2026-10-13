//! Smoke tests that exercise the real external binaries through the registry.

use std::path::PathBuf;
use std::process::Command;

use harness_tools::{builtin_registry, ToolContext};
use serde_json::json;

fn have(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("masse-tools-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[tokio::test]
async fn native_file_tools_roundtrip() {
    let root = temp_root("fs");
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg
        .call(
            "write_file",
            &ctx,
            json!({"path": "sub/a.txt", "content": "one\ntwo\nthree\n"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let r = reg
        .call(
            "edit_file",
            &ctx,
            json!({"path": "sub/a.txt", "old_string": "two", "new_string": "TWO"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let r = reg
        .call("read_file", &ctx, json!({"path": "sub/a.txt"}))
        .await;
    assert!(r.output.contains("TWO"));

    let r = reg.call("list_dir", &ctx, json!({"path": "sub"})).await;
    assert!(r.output.contains("a.txt"));

    // Escaping the root must fail.
    let r = reg
        .call("read_file", &ctx, json!({"path": "../../etc/passwd"}))
        .await;
    assert!(r.is_error());

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn grep_and_glob_find_files() {
    if !have("grep") || !have("find") {
        return;
    }
    let root = temp_root("search");
    std::fs::write(root.join("hello.txt"), "hello world\n").unwrap();
    std::fs::write(root.join("other.md"), "nothing here\n").unwrap();
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg.call("grep", &ctx, json!({"pattern": "hello"})).await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("hello.txt"), "{}", r.output);

    let r = reg
        .call("glob", &ctx, json!({"pattern": "*.txt", "type": "f"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("hello.txt"), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn git_tools_flow() {
    if !have("git") {
        return;
    }
    let root = temp_root("git");
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap()
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    std::fs::write(root.join("f.txt"), "content\n").unwrap();

    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg.call("git_status", &ctx, json!({})).await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("f.txt"), "{}", r.output);

    let r = reg
        .call("git_stage", &ctx, json!({"paths": ["f.txt"]}))
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let r = reg
        .call("git_commit", &ctx, json!({"message": "add f"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let r = reg.call("git_log", &ctx, json!({"limit": 5})).await;
    assert!(r.output.contains("add f"), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn sqlite_tools_roundtrip() {
    if !have("sqlite3") {
        return;
    }
    let root = temp_root("sqlite");
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    // Structure goes through sqlite_schema.
    let r = reg
        .call(
            "sqlite_schema",
            &ctx,
            json!({"db": "test.db", "sql": "CREATE TABLE t(a TEXT);"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);

    // Rows go through sqlite_execute.
    let r = reg
        .call(
            "sqlite_execute",
            &ctx,
            json!({"db": "test.db", "sql": "INSERT INTO t VALUES ('x');"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let r = reg
        .call(
            "sqlite_query",
            &ctx,
            json!({"db": "test.db", "sql": "SELECT a FROM t;"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains('x'), "{}", r.output);

    // The guards: DDL is refused by execute, DML by schema.
    let r = reg
        .call(
            "sqlite_execute",
            &ctx,
            json!({"db": "test.db", "sql": "DROP TABLE t;"}),
        )
        .await;
    assert!(r.is_error(), "DDL must be refused by execute: {}", r.output);

    let r = reg
        .call(
            "sqlite_schema",
            &ctx,
            json!({"db": "test.db", "sql": "DELETE FROM t;"}),
        )
        .await;
    assert!(r.is_error(), "DML must be refused by schema: {}", r.output);

    // The read-only tool refuses writes at the connection.
    let r = reg
        .call(
            "sqlite_query",
            &ctx,
            json!({"db": "test.db", "sql": "INSERT INTO t VALUES ('y');"}),
        )
        .await;
    assert!(
        r.is_error(),
        "writes must fail on the read-only tool: {}",
        r.output
    );

    let _ = std::fs::remove_dir_all(&root);
}

fn docs_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("MASSE_BIALET_DOCS") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let candidate = PathBuf::from("/Users/albo/work/bialet/bialet/docs");
    candidate.is_dir().then_some(candidate)
}

#[tokio::test]
async fn curl_refuses_external_and_unstarted_ports() {
    let root = temp_root("curl");
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg
        .call("curl", &ctx, json!({"url": "https://example.com/"}))
        .await;
    assert!(r.is_error(), "external host must be refused: {}", r.output);

    let r = reg
        .call("curl", &ctx, json!({"url": "http://127.0.0.1:7001/"}))
        .await;
    assert!(
        r.is_error(),
        "a loopback port with no server must be refused: {}",
        r.output
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn bialet_docs_serves_bundled_pages() {
    // No docs dir configured: the bundled reference must still answer.
    let root = temp_root("docs-embedded");
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg.call("bialet_docs", &ctx, json!({"list": true})).await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("sqlite.md"), "{}", r.output);

    let r = reg
        .call("bialet_docs", &ctx, json!({"file": "css.md"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("box-sizing"), "{}", r.output);

    let r = reg
        .call("bialet_docs", &ctx, json!({"topic": "CREATE TABLE"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("CREATE TABLE"), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn bialet_run_and_validate() {
    if !have("bialet") {
        return;
    }
    let root = temp_root("bialet");
    let ctx = ToolContext::new(&root);
    let reg = builtin_registry();

    let r = reg
        .call(
            "bialet_run",
            &ctx,
            json!({"code": "System.print(\"masse-ok\")"}),
        )
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(r.output.contains("masse-ok"), "{}", r.output);

    std::fs::write(root.join("ok.wren"), "System.print(\"hi\")\n").unwrap();
    let r = reg
        .call("bialet_validate", &ctx, json!({"file": "ok.wren"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn bialet_docs_search() {
    let Some(docs) = docs_dir() else {
        return;
    };
    let root = temp_root("docs");
    let mut ctx = ToolContext::new(&root);
    ctx.docs_dir = Some(docs);
    let reg = builtin_registry();

    let r = reg.call("bialet_docs", &ctx, json!({"list": true})).await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(
        r.output.contains("usage.md") || r.output.contains(".md"),
        "{}",
        r.output
    );

    let r = reg
        .call("bialet_docs", &ctx, json!({"topic": "Request.post"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(!r.output.trim().is_empty(), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}

/// Locate a bialet binary: env override, then PATH, then the sibling build.
fn bialet_binary() -> Option<String> {
    if let Ok(bin) = std::env::var("MASSE_BIALET_BIN") {
        if PathBuf::from(&bin).exists() || have(&bin) {
            return Some(bin);
        }
    }
    if have("bialet") {
        return Some("bialet".into());
    }
    let sibling = PathBuf::from("/Users/albo/work/bialet/bialet/build/bialet");
    sibling.exists().then(|| sibling.display().to_string())
}

#[tokio::test]
async fn bialet_validate_quiet_and_server_lifecycle() {
    let Some(bin) = bialet_binary() else {
        return;
    };
    let root = temp_root("bialet2");
    std::fs::write(root.join("ok.wren"), "System.print(\"hi\")\n").unwrap();
    let mut ctx = ToolContext::new(&root);
    ctx.bialet_bin = bin;
    let reg = builtin_registry();

    // Quiet validation of a valid file succeeds.
    let r = reg
        .call("bialet_validate", &ctx, json!({"file": "ok.wren"}))
        .await;
    assert!(!r.is_error(), "{}", r.output);

    // Start a server: the log must land in .masse/log.txt.
    let port = 7300 + (std::process::id() % 400) as u64;
    let r = reg
        .call("bialet_server_start", &ctx, json!({"port": port}))
        .await;
    assert!(!r.is_error(), "{}", r.output);
    assert!(root.join(".masse/log.txt").exists(), "log file missing");

    let r = reg.call("bialet_log_read", &ctx, json!({})).await;
    assert!(!r.is_error(), "{}", r.output);

    // A second server on a busy port must not fail: it moves to a free port and
    // says so. Hold the port open so the situation is deterministic.
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let busy_port = busy.local_addr().unwrap().port() as u64;
    let r = reg
        .call(
            "bialet_server_start",
            &ctx,
            json!({"port": busy_port, "name": "dup"}),
        )
        .await;
    assert!(!r.is_error(), "busy port should fall back: {}", r.output);
    assert!(
        r.output.contains("was busy"),
        "the result must mention the busy port: {}",
        r.output
    );

    let r = reg
        .call("bialet_server_stop", &ctx, json!({"all": true}))
        .await;
    assert!(!r.is_error(), "{}", r.output);

    let _ = std::fs::remove_dir_all(&root);
}
