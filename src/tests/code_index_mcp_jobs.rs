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
