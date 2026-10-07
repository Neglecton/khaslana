use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

fn options() -> PipelineOptions {
    PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}))
}

fn fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    for (path, source) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let db = temp.path().join("index.db");
    assert!(matches!(
        run_index(&root, &db, true, &mut options()).unwrap(),
        RunOutcome::Completed(_)
    ));
    (temp, root, db)
}

#[test]
fn exact_type_name_outranks_partial_function_and_structural_noise() {
    let (_temp, _root, db) = fixture(&[(
        "src/service.rs",
        "struct Service {}\nfn service_service_service() {}\n",
    )]);
    let hits = search_symbols(&db, "Service", 20).unwrap();
    assert_eq!(hits[0].name, "Service");
    assert_eq!(hits[0].label, "Struct");
    assert!(
        hits.iter()
            .all(|hit| hit.label != "File" && hit.label != "Folder")
    );
    let (_, total) = search_symbols_with_options(
        &db,
        &SearchOptions {
            query: Some("service"),
            label: Some("File"),
            limit: 20,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(total, 1);
}

#[test]
fn declaration_docs_and_signature_are_searchable_but_function_body_is_not() {
    let (_temp, _root, db) = fixture(&[
        (
            "src/auth.rs",
            "/// Rotate credentials before expiry.\npub fn renew(token: &CredentialToken) { let hidden_body_marker = 1; }\n",
        ),
        ("src/other.rs", "fn rotate_metrics() {}\n"),
    ]);
    let hits = search_symbols(&db, "credentials expiry", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "renew");
    assert!(hits[0].signature.contains("CredentialToken"));
    assert!(hits[0].docstring.contains("Rotate credentials"));
    assert_eq!(
        search_symbols(&db, "CredentialToken", 20).unwrap()[0].name,
        "renew"
    );
    assert!(
        search_symbols(&db, "hidden_body_marker", 20)
            .unwrap()
            .is_empty()
    );
    let detail = symbol_detail(&db, None, &hits[0].qualified_name).unwrap();
    let DetailOutcome::Found(detail) = detail else {
        panic!("应找到定义");
    };
    assert!(detail.docstring.contains("expiry"));
}

#[test]
fn docs_follow_wrappers_and_python_docstrings_without_leaking_file_comments() {
    let mut extractor = Extractor::new();
    let rust = extractor
        .extract(
            LangId::Rust,
            b"//! File overview\n/// Renew credentials.\n#[inline]\npub fn renew() {}\n",
        )
        .unwrap()
        .unwrap();
    assert!(rust.defs[0].docstring.contains("Renew credentials"));
    assert!(!rust.defs[0].docstring.contains("overview"));
    let ts = extractor.extract(LangId::TypeScript, b"/** Refresh access token. */\nexport const renew = (value: string) => { return value; };\n").unwrap().unwrap();
    let renew = ts.defs.iter().find(|def| def.name == "renew").unwrap();
    assert!(renew.docstring.contains("Refresh access token"));
    assert!(!renew.signature.contains("return"));
    let py = extractor.extract(LangId::Python, b"@decorator\ndef renew(value):\n    \"\"\"Refresh access token.\"\"\"\n    return value\n").unwrap().unwrap();
    assert!(py.defs[0].docstring.contains("Refresh access token"));
    assert!(!py.defs[0].signature.contains("return"));
    let trailing = extractor
        .extract(
            LangId::Rust,
            b"fn previous() {} // Previous explanation.\nfn next() {}\n",
        )
        .unwrap()
        .unwrap();
    assert!(
        trailing
            .defs
            .iter()
            .find(|def| def.name == "next")
            .unwrap()
            .docstring
            .is_empty()
    );
}

#[test]
fn filters_and_pagination_share_a_stable_complete_match_set() {
    let (_temp, _root, db) = fixture(&[
        (
            "src/auth/a.rs",
            "fn refresh_access() {}\nfn refresh_session() {}\n",
        ),
        ("src/auth/b.rs", "fn refresh_credentials() {}\n"),
        ("src/cache.rs", "fn refresh_cache() {}\n"),
    ]);
    let mut request = SearchOptions {
        query: Some("refresh"),
        name_pattern: Some("^refresh_(access|session|credentials)$"),
        file_pattern: Some("src\\auth\\*"),
        label: Some("Function"),
        limit: 1,
        ..Default::default()
    };
    let mut names = Vec::new();
    for offset in 0..3 {
        request.offset = offset;
        let (hits, total) = search_symbols_with_options(&db, &request).unwrap();
        assert_eq!(total, 3);
        assert_eq!(hits.len(), 1);
        names.push(hits[0].qualified_name.clone());
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 3);
    request.offset = 20;
    let (hits, total) = search_symbols_with_options(&db, &request).unwrap();
    assert!(hits.is_empty());
    assert_eq!(total, 3);
    request.query = None;
    request.offset = 0;
    assert_eq!(search_symbols_with_options(&db, &request).unwrap().1, 3);
    request.name_pattern = Some("[");
    assert!(
        search_symbols_with_options(&db, &request)
            .unwrap_err()
            .to_string()
            .contains("正则")
    );
}

#[test]
fn natural_keywords_fall_back_only_when_full_match_is_empty() {
    let (_temp, _root, db) = fixture(&[
        ("src/auth.rs", "/// Rotate credentials.\nfn renew() {}\n"),
        ("src/cache.rs", "fn rotate_cache() {}\n"),
    ]);
    let hits = search_symbols(&db, "rotate credentials", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "renew");
    assert!(
        search_symbols(&db, "credentials unavailable_word", 20)
            .unwrap()
            .iter()
            .any(|hit| hit.name == "renew")
    );
    assert!(search_symbols(&db, "[]\"***", 20).unwrap().is_empty());
}

#[test]
fn chinese_words_match_inside_continuous_documentation_as_phrases() {
    let (_temp, _root, db) = fixture(&[
        (
            "src/auth.rs",
            "/// 立即更新访问凭据，避免过期。\nfn renew() {}\n",
        ),
        (
            "src/other.rs",
            "/// 凭证信息，依据配置执行。\nfn other() {}\n",
        ),
    ]);
    let hits = search_symbols(&db, "凭据", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "renew");
    assert_eq!(
        search_symbols(&db, "访问凭据", 20).unwrap()[0].name,
        "renew"
    );
}

fn downgrade_to_v2(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch("DROP TABLE nodes_fts;
        CREATE VIRTUAL TABLE nodes_fts USING fts5(name, qualified_name, label, file_path, content='');
        INSERT INTO nodes_fts(rowid, name, qualified_name, label, file_path)
            SELECT id, name, qualified_name, label, file_path FROM nodes;
        UPDATE meta SET value = '2' WHERE key = 'schema_version';
        DELETE FROM meta WHERE key = 'search_content_version';").unwrap();
}

#[test]
fn old_index_remains_readable_and_migration_preserves_graph_and_hashes() {
    let (_temp, root, db) = fixture(&[("src/auth.rs", "/// Rotate credentials.\nfn renew() {}\n")]);
    let before = read_index_stats(&db).unwrap().unwrap();
    downgrade_to_v2(&db);
    // MCP 只读查询不得触发写入，也不能在升级前失去搜索能力。
    assert_eq!(search_symbols(&db, "renew", 20).unwrap()[0].name, "renew");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "2");
    drop(conn);
    let store = CodeIndexStore::open(&db).unwrap();
    let after = store.read_stats().unwrap().unwrap();
    assert_eq!(before.nodes, after.nodes);
    assert_eq!(before.edges, after.edges);
    assert_eq!(before.repo_path, after.repo_path);
    assert_eq!(store.load_file_hashes().unwrap().len(), before.files);
    assert!(!store.has_search_metadata());
    drop(store);
    assert!(matches!(
        run_index(&root, &db, false, &mut options()).unwrap(),
        RunOutcome::Completed(_)
    ));
    assert_eq!(
        search_symbols(&db, "credentials", 20).unwrap()[0].name,
        "renew"
    );
    assert!(matches!(
        run_index(&root, &db, false, &mut options()).unwrap(),
        RunOutcome::Unchanged
    ));
}

#[test]
fn failed_migration_rolls_back_without_destroying_old_index() {
    let (_temp, _root, db) = fixture(&[("src/auth.rs", "fn renew() {}\n")]);
    downgrade_to_v2(&db);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_migration BEFORE UPDATE ON meta BEGIN SELECT RAISE(ABORT, 'blocked'); END;").unwrap();
    assert!(CodeIndexStore::open(&db).is_err());
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "2");
    assert_eq!(search_symbols(&db, "renew", 20).unwrap()[0].name, "renew");
    conn.execute_batch("DROP TRIGGER reject_migration;")
        .unwrap();
    assert!(CodeIndexStore::open(&db).is_ok());
}

#[test]
fn migration_keeps_fts_aligned_with_non_contiguous_node_ids() {
    let (_temp, _root, db) = fixture(&[("src/auth.rs", "fn renew() {}\n")]);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
        UPDATE nodes SET id = id * 10;
        UPDATE edges SET source_id = source_id * 10, target_id = target_id * 10;
        PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    drop(conn);
    downgrade_to_v2(&db);
    let store = CodeIndexStore::open(&db).unwrap();
    assert_eq!(store.search_symbols("renew", 20).unwrap()[0].name, "renew");
}

#[test]
fn tracing_reports_full_totals_and_allows_paging_past_the_old_cap() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("index.db");
    let mut graph = GraphBuffer::new();
    let target = graph.add_symbol(
        NodeLabel::Function,
        "target",
        "repo.target".into(),
        "src/lib.rs",
        1,
        1,
        "{}".into(),
    );
    for index in 0..125 {
        let name = format!("caller_{index:03}");
        let caller = graph.add_symbol(
            NodeLabel::Function,
            &name,
            format!("repo.{name}"),
            "src/lib.rs",
            1,
            1,
            "{}".into(),
        );
        graph.add_edge(caller, target, EdgeType::Calls, "{}".into());
    }
    CodeIndexStore::open(&db)
        .unwrap()
        .replace_all(&graph, &[], &CodeIndexMeta::default())
        .unwrap();
    let TraceOutcome::Found(first) =
        trace_calls_page(&db, "target", TraceDirection::Inbound, 1, 100, 0).unwrap()
    else {
        panic!("应找到调用方");
    };
    assert_eq!(first.callers.len(), 100);
    assert_eq!(first.callers_total, 125);
    assert!(first.has_more);
    let TraceOutcome::Found(second) =
        trace_calls_page(&db, "target", TraceDirection::Inbound, 1, 100, 100).unwrap()
    else {
        panic!("应找到调用方");
    };
    assert_eq!(second.callers.len(), 25);
    assert_eq!(second.callers_total, 125);
    assert!(!second.has_more);
    assert!(first.callers.iter().all(|first| {
        second
            .callers
            .iter()
            .all(|second| first.qualified_name != second.qualified_name)
    }));
}

#[test]
fn incremental_updates_replaces_documentation_and_removes_deleted_definitions() {
    let (_temp, root, db) = fixture(&[("src/auth.rs", "/// Rotate credentials.\nfn renew() {}\n")]);
    std::fs::write(
        root.join("src/auth.rs"),
        "/// Invalidate expired sessions immediately.\nfn revoke() {}\n",
    )
    .unwrap();
    assert!(matches!(
        run_index(&root, &db, false, &mut options()).unwrap(),
        RunOutcome::Completed(_)
    ));
    assert!(search_symbols(&db, "credentials", 20).unwrap().is_empty());
    assert_eq!(
        search_symbols(&db, "expired sessions", 20).unwrap()[0].name,
        "revoke"
    );
}

#[test]
#[ignore = "真实仓库检索验收：cargo test --lib real_repository_documentation_search -- --ignored --nocapture"]
fn real_repository_documentation_search() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("index.db");
    let started = std::time::Instant::now();
    let outcome = run_index(root, &db, true, &mut options()).unwrap();
    println!("真实仓库索引：{outcome:?}，耗时 {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let hits = search_symbols(&db, "自动刷新", 20).unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.name == "run_incremental_if_stale"),
        "{hits:?}"
    );
    println!(
        "中文文档查询命中 {} 个定义，耗时 {:?}",
        hits.len(),
        started.elapsed()
    );
    let store = CodeIndexStore::open(&db).unwrap();
    let old_matches: i64 = store.conn.query_row(
        "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH '{name qualified_name label file_path} : \"自 动 刷 新\"'",
        [], |row| row.get(0)).unwrap();
    assert_eq!(
        old_matches, 0,
        "此用例应验证名称/路径检索原先找不到的业务描述"
    );
    assert_eq!(
        search_symbols(&db, "run_incremental_if_stale", 1).unwrap()[0].name,
        "run_incremental_if_stale"
    );
    let symbol = &hits.iter().find(|hit| hit.name == "run_incremental_if_stale").unwrap().qualified_name;
    let started = std::time::Instant::now();
    let cold = trace_calls_page(&db, symbol, TraceDirection::Both, 2, 20, 0).unwrap();
    let cold_time = started.elapsed();
    assert!(matches!(cold, TraceOutcome::Found(_)));
    let started = std::time::Instant::now();
    for _ in 0..20 { trace_calls_page(&db, symbol, TraceDirection::Both, 2, 20, 0).unwrap(); }
    println!("调用查询首次 {:?}，缓存后平均 {:?}", cold_time, started.elapsed() / 20);
    assert!(matches!(run_index(root, &db, false, &mut options()).unwrap(), RunOutcome::Unchanged));
}
