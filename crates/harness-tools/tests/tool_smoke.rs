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

    let r = reg
        .call(
            "sqlite_execute",
            &ctx,
            json!({"db": "test.db", "sql": "CREATE TABLE t(a TEXT); INSERT INTO t VALUES ('x');"}),
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
