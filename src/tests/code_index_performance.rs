use super::*;
use crate::git::test_support::git_test_support::{init_repo, write_file};
use rusqlite::Connection;
use std::sync::{Arc, atomic::AtomicBool};

fn options() -> PipelineOptions {
    PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}))
}

#[test]
fn incremental_store_keeps_unaffected_rows_and_removes_old_search_tokens() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/stable.rs", "fn stable_target() {}\nfn stable_caller() { stable_target(); }\n");
    write_file(dir.path(), "src/target.rs", "/// obsolete documentation\nfn old_target() {}\n");
    write_file(dir.path(), "src/caller.rs", "fn caller() { old_target(); }\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let conn = Connection::open(&db).unwrap();
    let stable_id: i64 = conn.query_row("SELECT id FROM nodes WHERE name='stable_target'", [], |row| row.get(0)).unwrap();
    conn.execute_batch("CREATE TRIGGER protect_nodes_delete BEFORE DELETE ON nodes WHEN OLD.file_path='src/stable.rs' BEGIN SELECT RAISE(ABORT,'untouched node deleted'); END;
        CREATE TRIGGER protect_nodes_update BEFORE UPDATE ON nodes WHEN OLD.file_path='src/stable.rs' BEGIN SELECT RAISE(ABORT,'untouched node updated'); END;
        CREATE TRIGGER protect_edges_delete BEFORE DELETE ON edges WHEN OLD.source_id IN (SELECT id FROM nodes WHERE file_path='src/stable.rs') BEGIN SELECT RAISE(ABORT,'untouched edge deleted'); END;
        CREATE TRIGGER protect_edges_update BEFORE UPDATE ON edges WHEN OLD.source_id IN (SELECT id FROM nodes WHERE file_path='src/stable.rs') BEGIN SELECT RAISE(ABORT,'untouched edge updated'); END;").unwrap();
    write_file(dir.path(), "src/target.rs", "/// replacement documentation\nfn new_target_longer() {}\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_eq!(conn.query_row("SELECT id FROM nodes WHERE name='stable_target'", [], |row| row.get::<_, i64>(0)).unwrap(), stable_id);
    assert!(search_symbols(&db, "old_target", 10).unwrap().iter().all(|hit| hit.name != "old_target"));
    assert!(search_symbols(&db, "obsolete", 10).unwrap().is_empty());
    assert_eq!(search_symbols(&db, "replacement", 10).unwrap().len(), 1);
    let TraceOutcome::Found(trace) = trace_calls(&db, "caller", TraceDirection::Outbound, 1, 10).unwrap() else { panic!("调用方应保留"); };
    assert_eq!(trace.callees_total, 0);
    let oversized_properties: i64 = conn.query_row("SELECT count(*) FROM nodes WHERE label='File' AND json_type(properties,'$.call_records') IS NOT NULL", [], |row| row.get(0)).unwrap();
    assert_eq!(oversized_properties, 0);
    let records: i64 = conn.query_row("SELECT count(*) FROM call_records WHERE typeof(data)='blob'", [], |row| row.get(0)).unwrap();
    assert_eq!(records, 3);
}

#[test]
fn incremental_failure_rolls_back_nodes_fts_calls_hashes_and_generation() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/lib.rs", "fn original() { original(); }\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let conn = Connection::open(&db).unwrap();
    let generation = index_generation(&db).unwrap();
    let calls: Vec<u8> = conn.query_row("SELECT data FROM call_records WHERE file_path='src/lib.rs'", [], |row| row.get(0)).unwrap();
    let hashes = CodeIndexStore::open(&db).unwrap().load_file_hashes().unwrap();
    conn.execute_batch("CREATE TRIGGER reject_new BEFORE INSERT ON nodes WHEN NEW.name='blocked_target' BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    write_file(dir.path(), "src/lib.rs", "fn blocked_target() {}\n");
    assert!(run_index(dir.path(), &db, false, &mut options()).is_err());
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "original", 10).unwrap().len(), 1);
    assert!(search_symbols(&db, "blocked_target", 10).unwrap().is_empty());
    assert_eq!(conn.query_row("SELECT data FROM call_records WHERE file_path='src/lib.rs'", [], |row| row.get::<_, Vec<u8>>(0)).unwrap(), calls);
    let after = CodeIndexStore::open(&db).unwrap().load_file_hashes().unwrap();
    assert_eq!((hashes[0].mtime_ns, hashes[0].size), (after[0].mtime_ns, after[0].size));
}

#[test]
fn local_queries_do_not_deserialize_unrelated_graph_rows() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/lib.rs", "fn target() {}\nfn caller() { target(); }\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let conn = Connection::open(&db).unwrap();
    // 无关行故意使用不能解码成 String 的 BLOB，整图载入会失败。
    conn.execute_batch("INSERT INTO nodes(label,name,qualified_name,file_path,properties) VALUES('Module','unrelated','unrelated','',x'ff');").unwrap();
    assert!(CodeIndexStore::open(&db).unwrap().load_graph().is_err());
    assert!(matches!(symbol_detail(&db, None, "target").unwrap(), DetailOutcome::Found(_)));
    let TraceOutcome::Found(trace) = trace_calls(&db, "target", TraceDirection::Inbound, 2, 10).unwrap() else { panic!("应找到目标"); };
    assert_eq!(trace.callers[0].name, "caller");
    let report = impacted_symbols_for_files(&db, &["src/lib.rs".into()], 2).unwrap();
    assert_eq!(report.impacted_symbols.len(), 2);
    assert_eq!(index_overview(&db).unwrap().calls, 1);
}

#[test]
fn legacy_v3_remains_queryable_until_call_records_are_rebuilt() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/lib.rs", "fn target() {}\nfn caller() { target(); }\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("DROP TABLE call_names; DROP TABLE call_records; UPDATE meta SET value='3' WHERE key IN ('schema_version','search_content_version');").unwrap();
    let generation = index_generation(&db).unwrap();
    assert!(matches!(symbol_detail(&db, None, "target").unwrap(), DetailOutcome::Found(_)));
    CodeIndexStore::open(&db).unwrap();
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "caller", 10).unwrap().len(), 1);
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_ne!(index_generation(&db).unwrap(), generation);
    assert_eq!(conn.query_row("SELECT count(*) FROM call_records", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert!(matches!(run_index(dir.path(), &db, false, &mut options()).unwrap(), RunOutcome::Unchanged));
}

#[test]
fn bulk_batches_preserve_tail_nodes_edges_and_search_entries() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    let source: String = (0..550).map(|index| format!("fn f_{index}() {{ f_{}(); }}\n", (index + 1) % 550)).collect();
    write_file(dir.path(), "src/lib.rs", &source);
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let stats = read_index_stats(&db).unwrap().unwrap();
    assert_eq!(stats.symbols, 550);
    assert_eq!(stats.calls, 550);
    assert_eq!(search_symbols(&db, "f_549", 10).unwrap()[0].name, "f_549");
    let TraceOutcome::Found(trace) = trace_calls(&db, "f_548", TraceDirection::Outbound, 3, 2).unwrap() else { panic!("应找到尾批符号"); };
    assert_eq!(trace.callees_total, 3);
    assert!(trace.has_more);
    assert_eq!(trace.callees[0].name, "f_549");
    let TraceOutcome::Found(page) = trace_calls_page(&db, "f_548", TraceDirection::Outbound, 3, 2, 2).unwrap() else { panic!("应找到下一页"); };
    assert_eq!(page.callees.len(), 1);
    assert!(!page.has_more);
}

#[test]
fn simultaneous_database_writer_is_rejected_without_changing_the_index() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/lib.rs", "fn original() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let generation = index_generation(&db).unwrap();
    let lock = super::jobs::database_lock(&db);
    let _guard = lock.lock().unwrap();
    write_file(dir.path(), "src/lib.rs", "fn replacement() {}\n");
    assert!(run_index(dir.path(), &db, false, &mut options()).is_err());
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "original", 10).unwrap().len(), 1);
}

#[test]
fn full_index_keeps_every_ancestor_of_deep_files() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/deep/nested/lib.rs", "fn leaf() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let graph = CodeIndexStore::open(&db).unwrap().load_graph().unwrap();
    let folders: Vec<_> = graph.nodes.iter().filter(|node| node.label == NodeLabel::Folder).collect();
    assert_eq!(folders.len(), 3, "中间目录没有直接文件时也必须保留");
    for folder in folders {
        let parent = folder.qualified_name.rsplit_once('.').unwrap().0;
        assert!(graph.edges.iter().any(|edge| edge.target == folder.id
            && edge.etype == EdgeType::ContainsFolder
            && graph.get(edge.source).qualified_name.as_ref() == parent));
    }
}

#[test]
fn incremental_structure_matches_full_rebuild_after_move_and_branch_switch() {
    let (dir, repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    let full_db = data.path().join("full.db");
    write_file(dir.path(), "old/deep/leaf.rs", "fn leaf() {}\n");
    write_file(dir.path(), "stable.rs", "fn stable() {}\n");
    let oid = crate::git::test_support::git_test_support::commit_all(&repo, "initial");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    repo.branch("next", &repo.find_commit(oid).unwrap(), false).unwrap();
    repo.set_head("refs/heads/next").unwrap();
    std::fs::remove_file(dir.path().join("old/deep/leaf.rs")).unwrap();
    write_file(dir.path(), "new/nested/leaf.rs", "fn leaf() {}\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_eq!(read_index_stats(&db).unwrap().unwrap().mode, "incremental");
    run_index(dir.path(), &full_db, true, &mut options()).unwrap();
    let relationships = |path: &std::path::Path| {
        let graph = CodeIndexStore::open(path).unwrap().load_graph().unwrap();
        let mut nodes: Vec<_> = graph.nodes.iter().map(|node| (node.label.as_str(), node.qualified_name.to_string())).collect();
        let mut edges: Vec<_> = graph.edges.iter().map(|edge| (graph.get(edge.source).qualified_name.to_string(),
            graph.get(edge.target).qualified_name.to_string(), edge.etype.as_str())).collect();
        nodes.sort();
        edges.sort();
        (nodes, edges)
    };
    assert_eq!(relationships(&db), relationships(&full_db));
}

#[test]
fn branch_only_refresh_updates_metadata_without_reparsing_symbols() {
    let (dir, repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/lib.rs", "fn stable() {}\n");
    let oid = crate::git::test_support::git_test_support::commit_all(&repo, "initial");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let generation = index_generation(&db).unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER protect_file_delete BEFORE DELETE ON nodes WHEN OLD.file_path<>'' BEGIN SELECT RAISE(ABORT,'unchanged file deleted'); END;
        CREATE TRIGGER protect_file_update BEFORE UPDATE ON nodes WHEN OLD.file_path<>'' BEGIN SELECT RAISE(ABORT,'unchanged file updated'); END;").unwrap();
    repo.branch("next", &repo.find_commit(oid).unwrap(), false).unwrap();
    repo.set_head("refs/heads/next").unwrap();
    assert!(matches!(run_index(dir.path(), &db, false, &mut options()).unwrap(), RunOutcome::Completed(_)));
    assert_eq!(read_index_stats(&db).unwrap().unwrap().branch, "next");
    assert_ne!(index_generation(&db).unwrap(), generation);
    let generation = index_generation(&db).unwrap();
    assert!(matches!(run_index(dir.path(), &db, false, &mut options()).unwrap(), RunOutcome::Unchanged));
    assert_eq!(index_generation(&db).unwrap(), generation);
}

#[test]
fn missing_repository_root_preserves_existing_index() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "lib.rs", "fn original() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let generation = index_generation(&db).unwrap();
    assert!(run_index(&dir.path().join("missing"), &db, false, &mut options()).is_err());
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "original", 10).unwrap().len(), 1);
}

#[test]
fn growing_file_is_excluded_before_parsing_and_retried_after_shrinking() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "lib.rs", "fn small() {}\n");
    let path = dir.path().join("lib.rs");
    let mut growing = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(move |progress| {
        if progress.phase == IndexPhase::Parse && progress.done == 0 {
            let mut source = "fn oversized() {}\n//".to_string();
            source.push_str(&"x".repeat(PARSE_MAX_BYTES as usize));
            std::fs::write(&path, source).unwrap();
        }
    }));
    run_index(dir.path(), &db, true, &mut growing).unwrap();
    assert!(search_symbols(&db, "oversized", 10).unwrap().is_empty());
    let report = check_coverage(&db, None, &["lib.rs".into()], &[], 0, 10).unwrap();
    assert_eq!(report["results"][0]["status"], "excluded");
    assert_eq!(report["results"][0]["reason"], "超过单文件解析上限");
    write_file(dir.path(), "lib.rs", "fn recovered_after_growth() {}\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_eq!(search_symbols(&db, "recovered_after_growth", 10).unwrap().len(), 1);
}

#[test]
fn old_content_version_rebuilds_once_without_changing_schema() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/deep/lib.rs", "fn target() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute("UPDATE meta SET value='4' WHERE key='search_content_version'", []).unwrap();
    let generation = index_generation(&db).unwrap();
    assert_eq!(search_symbols(&db, "target", 10).unwrap().len(), 1);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let mut cancel_upgrade = PipelineOptions::new(cancel, Box::new(move |progress| {
        if progress.phase == IndexPhase::Write { flag.store(true, std::sync::atomic::Ordering::Relaxed); }
    }));
    assert!(matches!(run_index(dir.path(), &db, false, &mut cancel_upgrade).unwrap(), RunOutcome::Cancelled));
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(conn.query_row("SELECT value FROM meta WHERE key='search_content_version'", [], |row| row.get::<_, String>(0)).unwrap(), "4");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_ne!(index_generation(&db).unwrap(), generation);
    assert_eq!(read_index_stats(&db).unwrap().unwrap().mode, "full");
    assert_eq!(conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| row.get::<_, String>(0)).unwrap(), "4");
    assert!(matches!(run_index(dir.path(), &db, false, &mut options()).unwrap(), RunOutcome::Unchanged));
}
