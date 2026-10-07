use super::*;
use crate::git::test_support::git_test_support::{commit_all, init_repo, write_file};

#[test]
fn review_index_search_source_and_calls_stay_on_target_commit() {
    let (dir, repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "src/lib.rs",
        "/// 更新凭据。\nfn target() { helper(); }\nfn helper() {}\n",
    );
    let target = commit_all(&repo, "target").to_string();
    write_file(dir.path(), "src/lib.rs", "fn current_head() {}\n");
    let head = commit_all(&repo, "current");
    write_file(dir.path(), "src/lib.rs", "fn dirty_workspace() {}\n");
    let cancelled = AtomicBool::new(false);
    let mut index = ReviewIndex::for_test(dir.path().into(), target.clone(), data.path().into());
    let search: Value = serde_json::from_str(
        &index
            .dispatch(&repo, "search_symbols", r#"{"query":"凭据"}"#, &cancelled)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(search["target_commit"], target);
    assert_eq!(search["results"][0]["name"], "target");
    let name = search["results"][0]["qualified_name"].as_str().unwrap();
    let detail: Value = serde_json::from_str(
        &index
            .dispatch(
                &repo,
                "get_symbol_detail",
                &json!({ "name": name }).to_string(),
                &cancelled,
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        detail["source"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("helper();"))
    );
    assert_eq!(detail["source_freshness"], "commit_snapshot");
    let trace: Value = serde_json::from_str(
        &index
            .dispatch(
                &repo,
                "trace_path",
                &json!({ "function_name": name, "direction": "outbound" }).to_string(),
                &cancelled,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(trace["callees"][0]["name"], "helper");
    let empty: Value = serde_json::from_str(
        &index
            .dispatch(
                &repo,
                "search_symbols",
                r#"{"query":"dirty_workspace"}"#,
                &cancelled,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(repo.head().unwrap().target(), Some(head));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "fn dirty_workspace() {}\n"
    );
    let db = index.db_path.clone().unwrap();
    let generation = code_index::index_generation(&db).unwrap();
    index
        .dispatch(
            &repo,
            "check_index_coverage",
            r#"{"paths":["src/lib.rs"]}"#,
            &cancelled,
        )
        .unwrap();
    assert_eq!(
        code_index::index_generation(&db).unwrap(),
        generation,
        "相同提交重复使用缓存"
    );
}

#[test]
fn cancellation_does_not_build_snapshot_and_invalid_commit_cannot_escape_cache_root() {
    let (dir, repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/lib.rs", "fn target() {}\n");
    let commit = commit_all(&repo, "target").to_string();
    let mut index = ReviewIndex::for_test(dir.path().into(), commit, data.path().into());
    let cancelled = AtomicBool::new(true);
    assert!(
        index
            .dispatch(&repo, "search_symbols", r#"{"query":"target"}"#, &cancelled)
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
    assert!(index.db_path.is_none());
    assert!(code_index::commit_index_path(data.path(), dir.path(), "../../outside").is_err());
}

#[test]
fn result_budget_preserves_json_and_usable_pagination() {
    let rows: Vec<_> = (0..30).map(|index| json!({ "name": format!("task_{index}"), "qualified_name": format!("repo.task_{index}"), "signature": "p".repeat(1500) })).collect();
    let text = bounded_result(json!({ "results": rows, "total": 30, "offset": 5 })).unwrap();
    assert!(text.chars().count() <= super::super::review_agent::MAX_TOOL_RESULT_CHARS);
    let page: Value = serde_json::from_str(&text).unwrap();
    let returned = page["results"].as_array().unwrap().len();
    assert!(returned > 0 && returned < 30);
    assert_eq!(page["next_offset"].as_u64().unwrap(), (5 + returned) as u64);
    assert_eq!(page["has_more"], true);
}
