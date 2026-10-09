use super::*;
use crate::git::test_support::git_test_support::{init_repo, write_file};

fn call(server: &McpServer, tool: &str, args: Value) -> Value {
    serde_json::from_str(&server.handle_message(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args }
    }).to_string()).unwrap()).unwrap()
}

#[test]
fn automatic_checks_queue_multiple_repositories_and_refresh_edits() {
    let (first, _repo, _service) = init_repo();
    let (second, _repo2, _service2) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(first.path(), "src/lib.rs", "fn before() {}\n");
    write_file(second.path(), "src/lib.rs", "fn other_repo() {}\n");
    let mut server = McpServer::for_multi_test(data.path().into());
    server.auto_refresh = true;
    let one = McpServer::context_from_root(data.path(), first.path()).unwrap();
    let two = McpServer::context_from_root(data.path(), second.path()).unwrap();
    server.ensure_index_background(&one);
    server.ensure_index_background(&two);
    server.coordinator.wait(&one.repo_key);
    server.coordinator.wait(&two.repo_key);
    assert_eq!(read_index_stats(&two.db_path).unwrap().unwrap().symbols, 1);
    let old = super::super::index_generation(&one.db_path)
        .unwrap()
        .unwrap();
    write_file(first.path(), "src/lib.rs", "fn after_edit() {}\n");
    server.coordinator.expire_delay(&one.repo_key);
    call(
        &server,
        "index_status",
        json!({ "repo": first.path().to_str().unwrap() }),
    );
    server.coordinator.wait(&one.repo_key);
    let result = call(
        &server,
        "search_graph",
        json!({ "repo": first.path().to_str().unwrap(), "query": "after_edit" }),
    );
    assert_ne!(result["result"]["isError"], true);
    assert_eq!(result["result"]["structuredContent"]["total"], 1);
    let obsolete = call(
        &server,
        "search_graph",
        json!({ "repo": first.path().to_str().unwrap(), "query": "after_edit", "generation": old }),
    );
    assert_eq!(obsolete["result"]["isError"], true);
    let coverage = call(
        &server,
        "check_index_coverage",
        json!({ "repo": first.path().to_str().unwrap(), "paths": ["src/lib.rs"], "generation": old }),
    );
    assert_eq!(coverage["result"]["isError"], true);
}

#[test]
fn failed_automatic_build_reports_error_and_retries_after_backoff() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/lib.rs", "fn recovered() {}\n");
    let db = data.path().join("index.db");
    std::fs::create_dir(&db).unwrap();
    let mut server = McpServer::for_test(dir.path(), db.clone());
    server.auto_refresh = true;
    call(&server, "index_status", json!({}));
    server.coordinator.wait("");
    let failed = call(&server, "index_status", json!({}));
    assert_eq!(failed["result"]["isError"], true);
    let failed: Value =
        serde_json::from_str(failed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(failed["refresh"]["failures"], 1);
    assert!(failed["refresh"]["last_error"].is_string());
    std::fs::remove_dir(&db).unwrap();
    server.coordinator.expire_delay("");
    call(&server, "index_status", json!({}));
    server.coordinator.wait("");
    let ready = call(&server, "search_graph", json!({ "query": "recovered" }));
    assert_eq!(ready["result"]["structuredContent"]["total"], 1);
    assert_eq!(server.coordinator.status("")["failures"], 0);
}

#[test]
fn refresh_returns_pending_without_blocking_ping_or_duplicating_the_active_job() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let server = std::sync::Arc::new(McpServer::for_test(dir.path(), data.path().join("index.db")));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    server.coordinator.schedule("".into(), true, move |_| { release_rx.recv().unwrap(); Ok(()) });
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::sync::Arc::clone(&server);
    let thread = std::thread::spawn(move || {
        result_tx.send(call(&worker, "refresh_index", json!({}))).unwrap();
    });
    let result = result_rx.recv_timeout(std::time::Duration::from_secs(2));
    // 先释放假任务，断言失败也不遗留占用共享池的线程。
    release_tx.send(()).unwrap();
    thread.join().unwrap();
    let result = result.expect("refresh_index 不能无限等待当前任务");
    assert_eq!(result["result"]["structuredContent"]["status"], "indexing");
    assert_eq!(result["result"]["structuredContent"]["scheduled"], false);
    let ping = server.handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&ping).unwrap()["result"], json!({}));
    server.coordinator.wait("");
}

#[test]
fn full_refresh_during_another_task_explicitly_reports_not_scheduled() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let server = McpServer::for_test(dir.path(), data.path().join("index.db"));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    server.coordinator.schedule("".into(), true, move |_| {
        release_rx.recv_timeout(std::time::Duration::from_secs(2)).ok(); Ok(())
    });
    let result = call(&server, "refresh_index", json!({"mode":"full"}));
    release_tx.send(()).ok();
    server.coordinator.wait("");
    assert_eq!(result["result"]["isError"], true);
    let payload: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["scheduled"], false);
}
