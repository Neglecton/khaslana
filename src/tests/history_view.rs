use super::*;

fn test_commit(oid: &str, parents: &[&str]) -> CommitInfo {
    CommitInfo {
        oid: oid.to_string(),
        short_oid: oid.to_string(),
        summary: oid.to_string(),
        message: oid.to_string(),
        author: "测试作者".to_string(),
        author_email: Some("test@example.invalid".to_string()),
        committer: "测试作者".to_string(),
        committer_email: Some("test@example.invalid".to_string()),
        time: 0,
        parents: parents.iter().map(|parent| (*parent).to_string()).collect(),
        refs: Vec::new(),
    }
}

#[test]
fn unmerged_branch_tips_do_not_connect_from_top() {
    let commits = vec![
        test_commit("feature-tip", &["base"]),
        test_commit("main-tip", &["base"]),
        test_commit("base", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    assert!(!rows[0].connected_from_top);
    assert!(!rows[1].connected_from_top);
    assert!(rows[2].connected_from_top);
}

// 分叉的两个分支 tip 汇合到同一父提交时，后到的 tip 并入父提交已有泳道，
// 自身泳道释放——否则父提交行之后会残留幽灵竖线贯穿到列表末尾。
#[test]
fn fork_rejoining_parent_releases_lane() {
    let commits = vec![
        test_commit("main-tip", &["base"]),
        test_commit("feature-tip", &["base"]),
        test_commit("base", &["root"]),
        test_commit("root", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    // feature-tip 行并入 base 所在泳道 0，自身泳道在行内仍可见（画圆点）。
    assert!(rows[1].lanes.contains(&1));
    assert_eq!(rows[1].connectors, vec![0]);
    // base 行及之后：幽灵泳道不应残留。
    assert_eq!(rows[2].lanes, vec![0]);
    assert_eq!(rows[3].lanes, vec![0]);
}

// 合并提交的第二父提交尚未分页加载时，其泳道不应被剪掉：引入行画斜线但不画悬空顶部竖线，
// 下一行该泳道作为贯穿竖线接续，保证线条连续。
#[test]
fn unloaded_parent_lane_stays_continuous() {
    let commits = vec![
        test_commit("merge", &["base", "missing"]),
        test_commit("base", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    assert!(rows[0].connectors.contains(&1));
    assert!(!rows[0].lanes.contains(&1));
    assert!(rows[1].lanes.contains(&1));
}

// 可见泳道上限随列宽增长，过窄时回退到 0。
#[test]
fn graph_max_lane_scales_with_width() {
    assert_eq!(graph_max_lane(20.0), 0);
    assert_eq!(graph_max_lane(64.0), 3);
    assert_eq!(graph_max_lane(96.0), 5);
    assert_eq!(graph_max_lane(480.0), 32);
}

// 提交者与作者相同时不产生展示文本（避免详情区噪音）。
#[test]
fn committer_note_only_when_differs_from_author() {
    let mut commit = test_commit("abcd1234", &[]);
    commit.committer = "测试作者".to_string();
    assert_eq!(committer_note(&commit), None);

    commit.committer = "变基机器人".to_string();
    commit.committer_email = Some("bot@example.invalid".to_string());
    assert_eq!(
        committer_note(&commit),
        Some("变基机器人 <bot@example.invalid>".to_string())
    );
}

#[test]
fn parents_note_covers_root_merge_and_octopus() {
    assert_eq!(parents_note(&[]), "根提交（无父提交）");
    assert_eq!(
        parents_note(&["aaaabbbbccccddddeeeeffff00001111".to_string()]),
        "父提交 aaaabbbb"
    );
    assert_eq!(
        parents_note(&[
            "aaaabbbbccccddddeeeeffff00001111".to_string(),
            "11112222333344445555666677778888".to_string()
        ]),
        "父提交 aaaabbbb / 11112222（合并提交）"
    );
    let octopus = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert_eq!(parents_note(&octopus), "父提交 3 个（章鱼合并）");
}

#[test]
fn author_label_includes_email_when_present() {
    let mut commit = test_commit("abcd1234", &[]);
    assert_eq!(author_label(&commit), "测试作者 <test@example.invalid>");

    commit.author_email = None;
    assert_eq!(author_label(&commit), "测试作者");
}

#[test]
fn history_commit_rows_fit_two_line_metadata_and_badges() {
    assert_eq!(HISTORY_COMMIT_ROW_HEIGHT, 48.0);
    assert!(HISTORY_COMMIT_ROW_HEIGHT > ui_theme::ROW_HEIGHT_REGULAR);
}

#[test]
fn history_inspector_layout_keeps_navigator_and_diff_space_stable() {
    let layout = history_inspector_layout(372.0, 300.0, None, false);

    assert_eq!(layout.navigator_width, 372.0);
    assert_eq!(layout.details_height, DEFAULT_HISTORY_DETAILS_HEIGHT);
    assert_eq!(layout.file_list_width, 300.0);
}

#[test]
fn history_inspector_layout_preserves_manual_details_or_collapses_it() {
    let expanded = history_inspector_layout(320.0, 280.0, Some(276.0), false);
    assert_eq!(expanded.details_height, 276.0);

    let collapsed = history_inspector_layout(320.0, 280.0, Some(276.0), true);
    assert_eq!(
        collapsed.details_height,
        HISTORY_INSPECTOR_COLLAPSED_DETAILS_HEIGHT
    );
    assert_eq!(collapsed.file_list_width, 280.0);
}

#[test]
fn history_inspector_layout_clamps_manual_file_list_width() {
    // 拖拽越界时由布局层钳制到可拖动范围，宽度状态不会写出极端值。
    let too_wide = history_inspector_layout(320.0, 5000.0, None, false);
    assert_eq!(
        too_wide.file_list_width,
        crate::MAX_HISTORY_INSPECTOR_FILES_WIDTH
    );
    let too_narrow = history_inspector_layout(320.0, 8.0, None, false);
    assert_eq!(
        too_narrow.file_list_width,
        crate::MIN_HISTORY_INSPECTOR_FILES_WIDTH
    );
}
