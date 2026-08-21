use super::*;

fn branch(name: &str, kind: BranchKind, upstream: Option<&str>) -> BranchInfo {
    BranchInfo {
        name: name.to_string(),
        kind,
        is_head: false,
        upstream: upstream.map(str::to_string),
        ahead: None,
        behind: None,
    }
}

#[test]
fn sidebar_branch_search_is_case_insensitive() {
    let branches = vec![
        branch("Feature/Login", BranchKind::Local, None),
        branch("bugfix/logout", BranchKind::Local, None),
    ];

    assert!(sidebar_branch_matches_query(&branches[0], "feature"));
    assert!(!sidebar_branch_matches_query(&branches[1], "feature"));
}

#[test]
fn sidebar_branch_search_keeps_unicode_case_insensitive() {
    let branches = vec![branch("功能/登录", BranchKind::Local, None)];

    assert!(sidebar_branch_matches_query(&branches[0], "功能"));
    assert!(!sidebar_branch_matches_query(&branches[0], "发布"));
}

#[test]
fn sidebar_branch_search_keeps_local_and_remote_groups_separate() {
    let branches = vec![
        branch("feature/a", BranchKind::Local, None),
        branch("origin/feature/a", BranchKind::Remote, None),
    ];

    let items = sidebar_navigation_items(
        &branches,
        0,
        0,
        0,
        SidebarSectionState {
            remote_branches: true,
            ..SidebarSectionState::default()
        },
        false,
        "feature",
        false,
        "feature",
        false,
    );
    assert_eq!(
        items,
        vec![
            SidebarNavItem::SectionHeader(SidebarSection::LocalBranches),
            SidebarNavItem::Branch(0),
            SidebarNavItem::SectionHeader(SidebarSection::Remotes),
            SidebarNavItem::SectionHeader(SidebarSection::RemoteBranches),
            SidebarNavItem::Branch(1),
            SidebarNavItem::SectionHeader(SidebarSection::Tags),
        ]
    );
}

#[test]
fn sidebar_remote_branch_search_matches_full_or_partial_name() {
    let branches = vec![
        branch("origin/feature/a", BranchKind::Remote, None),
        branch("upstream/release", BranchKind::Remote, None),
    ];

    assert!(sidebar_branch_matches_query(&branches[0], "origin/feature"));
    assert!(!sidebar_branch_matches_query(
        &branches[1],
        "origin/feature"
    ));
    assert!(sidebar_branch_matches_query(&branches[1], "release"));
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
fn sidebar_uniform_slot_fits_branch_filter_input_and_padding() {
    assert_eq!(SIDEBAR_NAV_ITEM_HEIGHT, 36.0);
    assert!(
        SIDEBAR_NAV_ITEM_HEIGHT
            >= SIDEBAR_BRANCH_FILTER_INPUT_MIN_HEIGHT
                + SIDEBAR_BRANCH_FILTER_VERTICAL_PADDING * 2.0
    );
}

#[test]
fn sidebar_local_branch_search_matches_upstream() {
    let branches = vec![
        branch("main", BranchKind::Local, Some("origin/trunk")),
        branch("feature/a", BranchKind::Local, Some("origin/feature/a")),
    ];

    assert!(sidebar_branch_matches_query(&branches[0], "origin/trunk"));
    assert!(!sidebar_branch_matches_query(&branches[1], "origin/trunk"));
}

#[test]
fn sidebar_navigation_model_keeps_twenty_thousand_remote_branches_as_indices() {
    let branches = (0..20_000)
        .map(|index| branch(&format!("origin/feature/{index}"), BranchKind::Remote, None))
        .collect::<Vec<_>>();
    let items = sidebar_navigation_items(
        &branches,
        0,
        0,
        0,
        SidebarSectionState {
            remote_branches: true,
            ..SidebarSectionState::default()
        },
        false,
        "",
        false,
        "",
        false,
    );

    assert_eq!(items.len(), 20_004);
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item, SidebarNavItem::Branch(_)))
            .count(),
        20_000
    );
    assert_eq!(items[3], SidebarNavItem::Branch(0));
    assert_eq!(items[20_002], SidebarNavItem::Branch(19_999));
    // 该纯模型没有 `AnyElement`，实际元素只会由 uniform_list 的可视 range 回调创建。
    assert!(items.iter().all(|item| matches!(
        item,
        SidebarNavItem::SectionHeader(_)
            | SidebarNavItem::BranchFilter(_)
            | SidebarNavItem::Branch(_)
            | SidebarNavItem::Remote(_)
            | SidebarNavItem::Tag(_)
            | SidebarNavItem::Stash(_)
            | SidebarNavItem::EmptyLocalBranches
            | SidebarNavItem::EmptyRemoteBranches
            | SidebarNavItem::LoadingRemotes
            | SidebarNavItem::LoadingRemoteBranches
    )));
}

#[test]
fn sidebar_navigation_model_respects_section_expansion_search_and_stash_visibility() {
    let branches = vec![
        branch("main", BranchKind::Local, Some("origin/main")),
        branch("origin/main", BranchKind::Remote, None),
        branch("origin/feature", BranchKind::Remote, None),
    ];
    let collapsed = sidebar_navigation_items(
        &branches,
        1,
        1,
        1,
        SidebarSectionState::default(),
        true,
        "origin/main",
        true,
        "feature",
        false,
    );
    assert_eq!(
        collapsed,
        vec![
            SidebarNavItem::SectionHeader(SidebarSection::LocalBranches),
            SidebarNavItem::BranchFilter(SidebarSection::LocalBranches),
            SidebarNavItem::Branch(0),
            SidebarNavItem::SectionHeader(SidebarSection::Remotes),
            SidebarNavItem::SectionHeader(SidebarSection::RemoteBranches),
            SidebarNavItem::SectionHeader(SidebarSection::Tags),
            SidebarNavItem::SectionHeader(SidebarSection::Stashes),
        ]
    );

    let expanded = sidebar_navigation_items(
        &branches,
        1,
        1,
        1,
        SidebarSectionState {
            remotes: true,
            remote_branches: true,
            tags: true,
            stashes: true,
            ..SidebarSectionState::default()
        },
        true,
        "origin/main",
        true,
        "feature",
        false,
    );
    assert_eq!(
        expanded,
        vec![
            SidebarNavItem::SectionHeader(SidebarSection::LocalBranches),
            SidebarNavItem::BranchFilter(SidebarSection::LocalBranches),
            SidebarNavItem::Branch(0),
            SidebarNavItem::SectionHeader(SidebarSection::Remotes),
            SidebarNavItem::Remote(0),
            SidebarNavItem::SectionHeader(SidebarSection::RemoteBranches),
            SidebarNavItem::BranchFilter(SidebarSection::RemoteBranches),
            SidebarNavItem::Branch(2),
            SidebarNavItem::SectionHeader(SidebarSection::Tags),
            SidebarNavItem::Tag(0),
            SidebarNavItem::SectionHeader(SidebarSection::Stashes),
            SidebarNavItem::Stash(0),
        ]
    );
}

#[test]
fn sidebar_local_branches_section_can_collapse_to_header_only() {
    // 本地分支与其它分组一样可折叠；默认展开仍是它（SidebarSectionState::default）。
    let branches = vec![branch("main", BranchKind::Local, Some("origin/main"))];
    let collapsed = sidebar_navigation_items(
        &branches,
        0,
        0,
        0,
        SidebarSectionState {
            local_branches: false,
            ..SidebarSectionState::default()
        },
        false,
        "",
        false,
        "",
        false,
    );
    assert_eq!(
        collapsed,
        vec![
            SidebarNavItem::SectionHeader(SidebarSection::LocalBranches),
            SidebarNavItem::SectionHeader(SidebarSection::Remotes),
            SidebarNavItem::SectionHeader(SidebarSection::RemoteBranches),
            SidebarNavItem::SectionHeader(SidebarSection::Tags),
        ]
    );
    assert!(SidebarSectionState::default().is_expanded(SidebarSection::LocalBranches));
    assert!(!SidebarSectionState::default().is_expanded(SidebarSection::Remotes));
}

#[test]
fn sidebar_remote_manage_disabled_reason_matches_state() {
    assert_eq!(
        sidebar_remote_manage_disabled_reason(false, false),
        Some("请先打开仓库")
    );
    assert_eq!(
        sidebar_remote_manage_disabled_reason(true, true),
        Some("当前操作进行中，请稍候")
    );
    assert_eq!(sidebar_remote_manage_disabled_reason(true, false), None);
}

#[test]
fn sidebar_sections_keep_one_continuous_scroll_strategy() {
    let state = SidebarSectionState::default();

    assert!(sidebar_section_should_render_rows(
        SidebarSection::LocalBranches,
        state,
        false
    ));
    assert!(!sidebar_section_should_render_rows(
        SidebarSection::Remotes,
        state,
        false
    ));
    assert!(!sidebar_section_should_render_rows(
        SidebarSection::Stashes,
        state,
        false
    ));
}

#[test]
fn sidebar_stash_section_appears_only_when_data_exists() {
    let state = SidebarSectionState::default();

    assert!(!sidebar_section_is_visible(SidebarSection::Stashes, false));
    assert!(sidebar_section_is_visible(SidebarSection::Stashes, true));
    assert!(sidebar_section_should_render_rows(
        SidebarSection::Stashes,
        SidebarSectionState {
            stashes: true,
            ..state
        },
        true
    ));
}
