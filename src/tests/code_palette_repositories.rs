use super::super::catalog::filter_entries;
use super::*;

fn branch(name: &str, kind: BranchKind) -> BranchInfo {
    BranchInfo {
        name: name.into(),
        kind,
        is_head: false,
        upstream: None,
        ahead: None,
        behind: None,
    }
}

#[test]
fn saved_paths_exclude_open_repositories_and_duplicate_windows_paths() {
    let paths = unopened_repository_paths(
        [PathBuf::from("D:/repos/open")],
        [
            PathBuf::from("d:/repos/OPEN"),
            PathBuf::from("D:/repos/closed"),
            PathBuf::from("d:/repos/CLOSED"),
        ],
    );
    assert_eq!(paths, [PathBuf::from("D:/repos/closed")]);
}

#[test]
fn results_from_old_sessions_or_unregistered_paths_do_not_finish_current_scan() {
    let path = PathBuf::from("D:/repos/saved");
    let mut catalog = RepositorySearchCatalog::new(2, vec![path.clone()]);
    assert!(!catalog.apply(1, &path, Ok(vec![branch("old", BranchKind::Local)])));
    assert!(!catalog.apply(2, Path::new("D:/repos/other"), Ok(Vec::new())));
    assert_eq!(catalog.progress_label(0), "0 个结果 · 正在读取 1 个仓库");
    assert!(catalog.apply(2, &path, Ok(vec![branch("main", BranchKind::Local)])));
    assert_eq!(catalog.progress_label(1), "1 个结果");
}

#[test]
fn closing_a_search_cancels_its_scan_without_cancelling_the_next_session() {
    let catalog = RepositorySearchCatalog::new(1, Vec::new());
    let old_cancelled = catalog.cancelled.clone();
    drop(catalog);
    let next = RepositorySearchCatalog::new(2, Vec::new());
    assert!(old_cancelled.load(Ordering::Relaxed));
    assert!(!next.cancelled.load(Ordering::Relaxed));
}

#[test]
fn same_named_branches_in_closed_repositories_keep_distinct_paths_and_kinds() {
    let first = SavedRepositorySearch {
        path: PathBuf::from("D:/repos/first"),
        branches: Some(Ok(vec![
            branch("feature/search", BranchKind::Local),
            branch("origin/feature/search", BranchKind::Remote),
        ])),
    };
    let second = SavedRepositorySearch {
        path: PathBuf::from("D:/repos/second"),
        branches: Some(Ok(vec![branch("feature/search", BranchKind::Local)])),
    };
    let mut entries = saved_repository_entries(&first);
    entries.extend(saved_repository_entries(&second));
    assert_eq!(filter_entries(entries.clone(), "feature/search").len(), 3);
    let hits = filter_entries(entries, "first 远端 search");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].target,
        SearchTarget::SavedBranch(
            first.path.clone(),
            "origin/feature/search".into(),
            BranchKind::Remote
        )
    );
}

#[test]
fn inaccessible_saved_repository_is_reported_and_other_results_remain_available() {
    let mut catalog = RepositorySearchCatalog::new(
        1,
        vec![
            PathBuf::from("D:/repos/missing"),
            PathBuf::from("D:/repos/valid"),
        ],
    );
    catalog.apply(1, Path::new("D:/repos/missing"), Err("目录已删除".into()));
    catalog.apply(
        1,
        Path::new("D:/repos/valid"),
        Ok(vec![branch("main", BranchKind::Local)]),
    );
    let failed = saved_repository_entries(&catalog.repositories[0]);
    assert!(failed[0].disabled_reason.is_some());
    assert!(failed[0].subtitle.contains("目录已删除"));
    assert_eq!(saved_repository_entries(&catalog.repositories[1]).len(), 2);
    assert_eq!(catalog.progress_label(3), "3 个结果 · 1 个仓库不可用");
}

#[test]
fn pending_navigation_requires_exact_branch_name_and_kind_after_repository_load() {
    let pending = PendingSearchBranch {
        load_id: 4,
        name: "origin/main".into(),
        kind: BranchKind::Remote,
    };
    let mut snapshot = RepositorySnapshot {
        branches: vec![branch("origin/main", BranchKind::Local)],
        ..Default::default()
    };
    assert!(!pending.matches_branch(&snapshot));
    snapshot
        .branches
        .push(branch("origin/main", BranchKind::Remote));
    assert!(pending.matches_branch(&snapshot));
}

#[test]
fn old_load_completion_does_not_consume_a_new_navigation_request() {
    let mut pending = Some(PendingSearchBranch {
        load_id: 5,
        name: "feature".into(),
        kind: BranchKind::Local,
    });
    assert!(PendingSearchBranch::take_matching(&mut pending, 4, 5, true).is_none());
    assert!(pending.is_some());
    assert!(PendingSearchBranch::take_matching(&mut pending, 5, 5, true).is_some());
    assert!(pending.is_none());
}

#[test]
fn navigating_away_or_reloading_repository_discards_delayed_branch_navigation() {
    for (current_load_id, active) in [(4, false), (5, true)] {
        let mut pending = Some(PendingSearchBranch {
            load_id: 4,
            name: "feature".into(),
            kind: BranchKind::Local,
        });
        assert!(
            PendingSearchBranch::take_matching(&mut pending, 4, current_load_id, active).is_none()
        );
        assert!(pending.is_none());
    }
}
