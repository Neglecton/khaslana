use super::*;
use khaslana::{BranchKind, RemoteInfo, RepositorySnapshot};

fn snapshot() -> RepositorySnapshot {
    RepositorySnapshot {
        branches: vec![
            BranchInfo {
                name: "main".to_string(),
                kind: BranchKind::Local,
                is_head: true,
                upstream: Some("origin/trunk".to_string()),
                ahead: None,
                behind: None,
            },
            BranchInfo {
                name: "origin/trunk".to_string(),
                kind: BranchKind::Remote,
                is_head: false,
                upstream: None,
                ahead: None,
                behind: None,
            },
            BranchInfo {
                name: "origin/feature/a".to_string(),
                kind: BranchKind::Remote,
                is_head: false,
                upstream: None,
                ahead: None,
                behind: None,
            },
            BranchInfo {
                name: "upstream/main".to_string(),
                kind: BranchKind::Remote,
                is_head: false,
                upstream: None,
                ahead: None,
                behind: None,
            },
        ],
        remotes: vec![
            RemoteInfo {
                name: "origin".to_string(),
                url: "https://example.com/repo.git".to_string(),
                credential_record_id: None,
            },
            RemoteInfo {
                name: "upstream".to_string(),
                url: "https://example.com/upstream.git".to_string(),
                credential_record_id: None,
            },
        ],
        ..RepositorySnapshot::default()
    }
}

#[test]
fn default_remote_branch_uses_matching_upstream() {
    let snapshot = snapshot();
    let local = current_local_branch(&snapshot).unwrap();

    assert_eq!(default_remote_branch_for(local, "origin"), "trunk");
    assert_eq!(default_remote_branch_for(local, "upstream"), "main");
}

#[test]
fn remote_branch_list_filters_by_remote() {
    let snapshot = snapshot();

    assert_eq!(
        remote_branch_names(&snapshot, "origin"),
        vec!["feature/a", "trunk"]
    );
    assert_eq!(remote_branch_names(&snapshot, "upstream"), vec!["main"]);
    assert!(remote_branch_exists(&snapshot, "origin", "feature/a"));
    assert!(!remote_branch_exists(&snapshot, "origin", "main"));
}

#[test]
fn dialog_defaults_fall_back_to_current_remote_and_upstream() {
    let snapshot = snapshot();
    let defaults = remote_branch_dialog_defaults(&snapshot, Some("origin".to_string())).unwrap();

    assert_eq!(defaults.local_branch, "main");
    assert_eq!(defaults.remote, "origin");
    assert_eq!(defaults.remote_branch, "trunk");
}
