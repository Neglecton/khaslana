use super::*;

fn branch(name: &str, kind: BranchKind, upstream: Option<&str>) -> BranchInfo {
    BranchInfo {
        name: name.to_string(),
        kind,
        is_head: false,
        upstream: upstream.map(str::to_string),
    }
}

fn branch_names(branches: Vec<BranchInfo>) -> Vec<String> {
    branches.into_iter().map(|branch| branch.name).collect()
}

#[test]
fn sidebar_branch_search_empty_query_returns_only_requested_kind() {
    let branches = vec![
        branch("main", BranchKind::Local, None),
        branch("feature/a", BranchKind::Local, None),
        branch("origin/main", BranchKind::Remote, None),
    ];

    assert_eq!(
        branch_names(filter_sidebar_branches(&branches, BranchKind::Local, "")),
        vec!["main", "feature/a"]
    );
    assert_eq!(
        branch_names(filter_sidebar_branches(&branches, BranchKind::Remote, "")),
        vec!["origin/main"]
    );
}

#[test]
fn sidebar_branch_search_is_case_insensitive() {
    let branches = vec![
        branch("Feature/Login", BranchKind::Local, None),
        branch("bugfix/logout", BranchKind::Local, None),
    ];

    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Local,
            "feature",
        )),
        vec!["Feature/Login"]
    );
}

#[test]
fn sidebar_branch_search_keeps_local_and_remote_groups_separate() {
    let branches = vec![
        branch("feature/a", BranchKind::Local, None),
        branch("origin/feature/a", BranchKind::Remote, None),
    ];

    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Local,
            "feature",
        )),
        vec!["feature/a"]
    );
    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Remote,
            "feature",
        )),
        vec!["origin/feature/a"]
    );
}

#[test]
fn sidebar_remote_branch_search_matches_full_or_partial_name() {
    let branches = vec![
        branch("origin/feature/a", BranchKind::Remote, None),
        branch("upstream/release", BranchKind::Remote, None),
    ];

    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Remote,
            "origin/feature",
        )),
        vec!["origin/feature/a"]
    );
    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Remote,
            "release",
        )),
        vec!["upstream/release"]
    );
}

#[test]
fn sidebar_branch_action_button_ids_keep_actions_distinct() {
    assert_ne!(
        sidebar_branch_search_button_id(SidebarSection::LocalBranches),
        sidebar_branch_search_button_id(SidebarSection::RemoteBranches),
    );
    assert_ne!(
        SIDEBAR_LOCAL_BRANCH_CREATE_ID,
        sidebar_branch_search_button_id(SidebarSection::LocalBranches),
    );
}

#[test]
fn sidebar_local_branch_search_matches_upstream() {
    let branches = vec![
        branch("main", BranchKind::Local, Some("origin/trunk")),
        branch("feature/a", BranchKind::Local, Some("origin/feature/a")),
    ];

    assert_eq!(
        branch_names(filter_sidebar_branches(
            &branches,
            BranchKind::Local,
            "origin/trunk",
        )),
        vec!["main"]
    );
}
