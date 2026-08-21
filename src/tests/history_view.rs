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

// 泳道算法测试（commit_graph_rows / graph_max_lane 等）已随实现迁移到
// src/tests/commit_graph_view.rs。

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

// 主历史页导航列较窄：行内引用标签上限收紧到 1（HEAD/首个本地分支优先），
// 其余收进「+n」徽标；完整标签展示交给图谱页（上限 3）与详情卡（全量）。
#[test]
fn main_history_rows_cap_inline_ref_labels_to_one() {
    assert_eq!(MAX_COMMIT_REF_LABELS, 1);
    assert_eq!(crate::commit_graph_view::GRAPH_REF_LABEL_CAP, 3);
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
