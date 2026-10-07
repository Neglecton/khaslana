use super::*;
use crate::git::test_support::git_test_support::{init_repo, write_file};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn options() -> PipelineOptions {
    PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}))
}

#[test]
fn coverage_records_partial_unsupported_large_and_excluded_files() {
    let (repo_dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(
        repo_dir.path(),
        "src/good.rs",
        "fn good() { external.unknown(); }\n",
    );
    write_file(
        repo_dir.path(),
        "src/broken.rs",
        "fn valid() {}\nfn broken( {\n",
    );
    write_file(repo_dir.path(), "notes.txt", "notes");
    write_file(
        repo_dir.path(),
        "large.rs",
        &"x".repeat(PARSE_MAX_BYTES as usize + 1),
    );
    write_file(repo_dir.path(), "build.o", "generated");
    let db = data.path().join("index.db");
    run_index(repo_dir.path(), &db, true, &mut options()).unwrap();
    let report = check_coverage(&db, Some(repo_dir.path()), &[], &[".".into()], 0, 100).unwrap();
    let rows = report["results"].as_array().unwrap();
    let row = |path: &str| rows.iter().find(|row| row["path"] == path).unwrap();
    assert_eq!(row("src/good.rs")["status"], "indexed");
    assert_eq!(row("src/good.rs")["unresolved_calls"], 1);
    assert_eq!(row("src/broken.rs")["status"], "partial");
    assert!(
        !row("src/broken.rs")["error_ranges"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(row("notes.txt")["status"], "unsupported");
    assert_eq!(row("large.rs")["status"], "excluded");
    assert_eq!(row("build.o")["status"], "excluded");
    assert!(
        check_coverage(
            &db,
            Some(repo_dir.path()),
            &["../outside.rs".into()],
            &[],
            0,
            20
        )
        .is_err()
    );
    write_file(repo_dir.path(), "src/good.rs", "fn changed() {}\n");
    let report = check_coverage(
        &db,
        Some(repo_dir.path()),
        &["src/good.rs".into()],
        &[],
        0,
        20,
    )
    .unwrap();
    assert_eq!(report["results"][0]["freshness"], "metadata_changed");
    let DetailOutcome::Found(detail) = symbol_detail(&db, Some(repo_dir.path()), "good").unwrap()
    else {
        panic!("旧定义应仍存在");
    };
    assert!(
        detail.source.is_none(),
        "不能拿当前文件的旧行号当作该定义源码"
    );
}

#[test]
fn read_failure_is_retried_even_with_unchanged_metadata() {
    let (repo_dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let path = repo_dir.path().join("src/lib.rs");
    write_file(repo_dir.path(), "src/lib.rs", "fn recovered() {}\n");
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let removed = path.clone();
    let db = data.path().join("index.db");
    let mut removal_options = PipelineOptions::new(
        Arc::new(AtomicBool::new(false)),
        Box::new(move |progress| {
            if progress.phase == IndexPhase::Parse && progress.done == 0 {
                let _ = std::fs::remove_file(&removed);
            }
        }),
    );
    run_index(repo_dir.path(), &db, true, &mut removal_options).unwrap();
    let report = check_coverage(
        &db,
        Some(repo_dir.path()),
        &["src/lib.rs".into()],
        &[],
        0,
        10,
    )
    .unwrap();
    assert_eq!(report["results"][0]["status"], "read_failed");
    std::fs::write(&path, "fn recovered() {}\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(matches!(
        run_index(repo_dir.path(), &db, false, &mut options()).unwrap(),
        RunOutcome::Completed(_)
    ));
    assert_eq!(search_symbols(&db, "recovered", 10).unwrap().len(), 1);
}

#[test]
fn qualified_and_self_calls_use_container_identity_and_unknown_receivers_remain_unresolved() {
    let (repo_dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(
        repo_dir.path(),
        "src/lib.rs",
        "struct A; struct B;\nimpl A { fn save(&self) {} fn run(&self) { self.save(); B::save(); save(); remote.save(); } }\nimpl B { fn save() {} }\nfn save() {}\nfn run_module() { crate::service::Service::open(); }\n",
    );
    write_file(
        repo_dir.path(),
        "src/service.rs",
        "struct Service; impl Service { fn open() {} }\n",
    );
    let db = data.path().join("index.db");
    run_index(repo_dir.path(), &db, true, &mut options()).unwrap();
    let TraceOutcome::Found(trace) =
        trace_calls(&db, "run", TraceDirection::Outbound, 1, 20).unwrap()
    else {
        panic!("应有调用关系");
    };
    assert_eq!(trace.callees_total, 3);
    assert!(
        trace
            .callees
            .iter()
            .any(|callee| callee.qualified_name.ends_with(".A.save"))
    );
    assert!(
        trace
            .callees
            .iter()
            .any(|callee| callee.qualified_name.ends_with(".B.save"))
    );
    let report = check_coverage(&db, None, &["src/lib.rs".into()], &[], 0, 10).unwrap();
    assert_eq!(report["results"][0]["unresolved_calls"], 1);
    let TraceOutcome::Found(trace) =
        trace_calls(&db, "run_module", TraceDirection::Outbound, 1, 20).unwrap()
    else {
        panic!("应有模块限定调用");
    };
    assert_eq!(trace.callees_total, 1);
    assert!(trace.callees[0].qualified_name.ends_with(".Service.open"));
}

#[test]
fn cache_invalidates_on_generation_and_write_phase_cancellation_preserves_old_index() {
    let (repo_dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    write_file(repo_dir.path(), "src/lib.rs", "fn original() {}\n");
    let db = data.path().join("index.db");
    run_index(repo_dir.path(), &db, true, &mut options()).unwrap();
    let old = super::cache::load_index(&db).unwrap();
    let generation = index_generation(&db).unwrap();
    write_file(repo_dir.path(), "src/lib.rs", "fn changed_name() {}\n");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let mut cancel_at_write = PipelineOptions::new(
        cancel,
        Box::new(move |progress| {
            if progress.phase == IndexPhase::Write {
                flag.store(true, Ordering::Relaxed);
            }
        }),
    );
    assert!(matches!(
        run_index(repo_dir.path(), &db, false, &mut cancel_at_write).unwrap(),
        RunOutcome::Cancelled
    ));
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "original", 10).unwrap().len(), 1);
    run_index(repo_dir.path(), &db, false, &mut options()).unwrap();
    let new = super::cache::load_index(&db).unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(old.nodes.iter().any(|node| node.name == "original"));
    assert!(new.nodes.iter().any(|node| node.name == "changed_name"));
    assert_eq!(
        super::coverage::source_freshness(
            old.file_hashes.get("src/lib.rs"),
            repo_dir.path(),
            "src/lib.rs"
        ),
        "metadata_changed"
    );
    assert_ne!(index_generation(&db).unwrap(), generation);
}

#[test]
fn repository_jobs_queue_and_panic_cleanup_does_not_poison_retries() {
    let jobs = super::jobs::IndexCoordinator::default();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    assert!(jobs.schedule("first".into(), true, move |_| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    }));
    started_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(jobs.schedule("second".into(), false, |_| Ok(())));
    assert_eq!(jobs.status("second")["phase"], "queued");
    assert!(!jobs.schedule("first".into(), true, |_| Ok(())));
    release_tx.send(()).unwrap();
    jobs.wait("first");
    jobs.wait("second");
    assert!(jobs.schedule("panic".into(), true, |_| panic!("test panic")));
    jobs.wait("panic");
    assert_eq!(jobs.status("panic")["active"], false);
    assert_eq!(jobs.status("panic")["last_error"], "test panic");
    assert!(
        !jobs.schedule("panic".into(), false, |_| Ok(())),
        "失败后应退避"
    );
    assert!(
        jobs.schedule("panic".into(), true, |_| Ok(())),
        "显式刷新可覆盖退避"
    );
    jobs.wait("panic");
    assert_eq!(jobs.status("panic")["failures"], 0);
    jobs.stop();
    assert!(!jobs.schedule("stopped".into(), true, |_| Ok(())));
}

#[test]
fn ignore_and_exclusion_changes_remove_stale_symbols_and_update_coverage() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/keep.rs", "fn keep() {}\n");
    write_file(dir.path(), "src/ignored.rs", "fn ignored() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    write_file(dir.path(), ".gitignore", "src/ignored.rs\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert!(search_symbols(&db, "ignored", 10).unwrap().is_empty());
    write_file(dir.path(), "new.o", "ignored artifact");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert!(search_symbols(&db, "ignored", 10).unwrap().is_empty());
    let report = check_coverage(&db, None, &["new.o".into()], &[], 0, 10).unwrap();
    assert_eq!(report["results"][0]["status"], "excluded");
    let generation = index_generation(&db).unwrap();
    std::fs::remove_file(dir.path().join("new.o")).unwrap();
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    assert_ne!(
        index_generation(&db).unwrap(),
        generation,
        "只改变发现覆盖信息也应更新"
    );
    let report = check_coverage(&db, None, &["new.o".into()], &[], 0, 10).unwrap();
    assert_eq!(report["results"][0]["status"], "not_indexed");
}

#[test]
fn incremental_resolution_updates_unchanged_callers_when_definitions_change() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "src/caller.rs", "fn caller() { target(); }\n");
    write_file(dir.path(), "src/one.rs", "fn unrelated() {}\n");
    write_file(dir.path(), "src/two.rs", "fn another() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    write_file(dir.path(), "src/one.rs", "fn target() {}\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    let TraceOutcome::Found(trace) =
        trace_calls(&db, "caller", TraceDirection::Outbound, 1, 20).unwrap()
    else {
        panic!("应找到调用方");
    };
    assert_eq!(trace.callees_total, 1);
    let coverage = check_coverage(&db, None, &["src/caller.rs".into()], &[], 0, 10).unwrap();
    assert_eq!(coverage["results"][0]["unresolved_calls"], 0);
    write_file(dir.path(), "src/two.rs", "fn target() {}\n");
    run_index(dir.path(), &db, false, &mut options()).unwrap();
    let TraceOutcome::Found(trace) =
        trace_calls(&db, "caller", TraceDirection::Outbound, 1, 20).unwrap()
    else {
        panic!("应找到调用方");
    };
    assert_eq!(trace.callees_total, 0, "全局同名变为歧义后不能保留旧猜测");
    let coverage = check_coverage(&db, None, &["src/caller.rs".into()], &[], 0, 10).unwrap();
    assert_eq!(coverage["results"][0]["unresolved_calls"], 1);
}

#[test]
fn graph_remaps_surviving_edges_after_node_removal() {
    let mut graph = GraphBuffer::new();
    let gone = graph.upsert_node(
        NodeLabel::File,
        "gone",
        "gone",
        "gone.rs",
        0,
        0,
        "{}".into(),
    );
    let caller = graph.upsert_node(
        NodeLabel::Function,
        "caller",
        "caller",
        "caller.rs",
        1,
        1,
        "{}".into(),
    );
    let target = graph.upsert_node(
        NodeLabel::Function,
        "target",
        "target",
        "target.rs",
        1,
        1,
        "{}".into(),
    );
    graph.add_edge(gone, caller, EdgeType::Calls, "{}".into());
    graph.add_edge(caller, target, EdgeType::Calls, "{}".into());
    graph.purge_files(&["gone.rs".into()].into_iter().collect());
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.get(graph.edges[0].source).name, "caller");
    assert_eq!(graph.get(graph.edges[0].target).name, "target");
}
