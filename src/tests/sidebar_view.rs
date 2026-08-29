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
    // 本地/远端分支是两个独立的分组列表，过滤结果各自落在自己的条目模型里。
    let branches = vec![
        branch("feature/a", BranchKind::Local, None),
        branch("origin/feature/a", BranchKind::Remote, None),
    ];

    assert_eq!(
        sidebar_local_branch_entries(&branches, "feature"),
        vec![SidebarNavItem::Branch(0)]
    );
    assert_eq!(
        sidebar_remote_branch_entries(&branches, "feature", false),
        vec![SidebarNavItem::Branch(1)]
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
fn sidebar_branch_entries_emit_placeholders_only_when_filter_misses() {
    let branches = vec![branch("main", BranchKind::Local, None)];
    assert_eq!(
        sidebar_local_branch_entries(&branches, "release"),
        vec![SidebarNavItem::EmptyLocalBranches]
    );
    // 无过滤词的空分组不产生占位项（渲染层只显示钉住的标题行）。
    assert!(sidebar_local_branch_entries(&[], "").is_empty());
    assert_eq!(
        sidebar_remote_branch_entries(&[], "anything", false),
        vec![SidebarNavItem::EmptyRemoteBranches]
    );
}

#[test]
fn sidebar_navigation_model_keeps_twenty_thousand_remote_branches_as_indices() {
    let branches = (0..20_000)
        .map(|index| branch(&format!("origin/feature/{index}"), BranchKind::Remote, None))
        .collect::<Vec<_>>();
    let items = sidebar_remote_branch_entries(&branches, "", false);

    assert_eq!(items.len(), 20_000);
    assert_eq!(items[0], SidebarNavItem::Branch(0));
    assert_eq!(items[19_999], SidebarNavItem::Branch(19_999));
    // 该纯模型没有 `AnyElement`，实际元素只会由 uniform_list 的可视 range 回调创建。
    assert!(items.iter().all(|item| matches!(
        item,
        SidebarNavItem::Branch(_)
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
fn sidebar_remote_entries_show_loading_or_indices() {
    assert_eq!(
        sidebar_remote_entries(2, true),
        vec![SidebarNavItem::LoadingRemotes]
    );
    assert_eq!(
        sidebar_remote_entries(2, false),
        vec![SidebarNavItem::Remote(0), SidebarNavItem::Remote(1)]
    );
    // 远端分支加载中且尚无任何远端分支时显示加载占位；已有数据则照常过滤展示。
    assert_eq!(
        sidebar_remote_branch_entries(&[], "", true),
        vec![SidebarNavItem::LoadingRemoteBranches]
    );
    let branches = vec![branch("origin/main", BranchKind::Remote, None)];
    assert_eq!(
        sidebar_remote_branch_entries(&branches, "", true),
        vec![SidebarNavItem::Branch(0)]
    );
}

#[test]
fn sidebar_local_branches_section_can_collapse_to_header_only() {
    // 本地分支与其它分组一样可折叠；默认展开仍是它（SidebarSectionState::default）。
    // 折叠分组只渲染钉住的标题行，不渲染条目列表。
    let collapsed = SidebarSectionState {
        local_branches: false,
        ..SidebarSectionState::default()
    };
    assert!(!sidebar_section_should_render_rows(
        SidebarSection::LocalBranches,
        collapsed,
        false
    ));
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
fn sidebar_sections_pin_headers_with_per_section_lists() {
    // 钉住标题架构：每个分组使用互不相同的滚动 id（各自独立滚动区），
    // 条目少的分组按内容定高、条目多的分组平分剩余空间。
    let ids = [
        sidebar_section_scroll_id(SidebarSection::LocalBranches),
        sidebar_section_scroll_id(SidebarSection::Remotes),
        sidebar_section_scroll_id(SidebarSection::RemoteBranches),
        sidebar_section_scroll_id(SidebarSection::Tags),
        sidebar_section_scroll_id(SidebarSection::Stashes),
    ];
    for (index, id) in ids.iter().enumerate() {
        assert!(ids.iter().skip(index + 1).all(|other| other != id));
    }

    assert_eq!(sidebar_section_height(0), SidebarSectionHeight::Content(0));
    assert_eq!(
        sidebar_section_height(SIDEBAR_SECTION_CONTENT_ROW_LIMIT),
        SidebarSectionHeight::Content(SIDEBAR_SECTION_CONTENT_ROW_LIMIT)
    );
    assert_eq!(
        sidebar_section_height(SIDEBAR_SECTION_CONTENT_ROW_LIMIT + 1),
        SidebarSectionHeight::Fill
    );

    // 展开的分组才渲染条目列表（默认仅本地分支展开）。
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
