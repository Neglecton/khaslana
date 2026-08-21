use super::*;
use khaslana::{ConflictBlockResolution, ConflictBlockStatus, ConflictDraftStatus};

#[test]
fn conflict_workbench_uses_flat_semantic_surface_roles() {
    assert_eq!(
        conflict_workbench_surfaces(),
        ConflictWorkbenchSurfaces {
            canvas: ui_theme::SURFACE_CANVAS,
            rail: ui_theme::RAIL_SURFACE,
            header: ui_theme::TITLEBAR_SURFACE,
            content: ui_theme::SURFACE_BASE,
            border: ui_theme::BORDER_MUTED,
        }
    );
}

#[test]
fn conflict_paths_follow_snapshot_conflict_order() {
    let mut snapshot = RepositorySnapshot::default();
    snapshot.conflicts = vec!["a.txt".to_string(), "dir/b.txt".to_string()];

    assert_eq!(
        conflict_paths(Some(&snapshot)),
        vec!["a.txt".to_string(), "dir/b.txt".to_string()]
    );
    assert!(conflict_paths(None).is_empty());
}

#[test]
fn conflict_status_message_names_operation_and_resolution_area() {
    assert_eq!(
        conflict_status_message("合并操作已完成", 2),
        "合并产生冲突，请在工作区使用 IDEA 或进入“冲突处理”解决（2 个文件）"
    );
    assert_eq!(
        conflict_status_message("正在同步", 1),
        "操作产生冲突，请在左侧“冲突”区域解决（1 个文件）"
    );
}

#[test]
fn result_pane_uses_draft_ranges_for_line_ownership() {
    let view = ConflictFileView {
        path: "file.txt".into(),
        kind: ConflictFileKind::Text,
        draft: "before\nresult\nafter\n".into(),
        ours_text: "before\nours\nafter\n".into(),
        theirs_text: "before\ntheirs\nafter\n".into(),
        blocks: vec![ConflictBlock {
            base: Some("before\nbase\nafter\n".into()),
            ours: "ours\n".into(),
            theirs: "theirs\n".into(),
            start: 7,
            end: 14,
            ours_start: 7,
            ours_end: 12,
            theirs_start: 7,
            theirs_end: 14,
            status: ConflictBlockStatus::Unresolved,
            has_manual_edits: false,
        }],
        draft_status: ConflictDraftStatus::Dirty,
        fallback_reason: None,
    };

    let owners = conflict_document_line_owners(
        &view.draft,
        ConflictDocumentPane::Result,
        &view,
        view.draft.lines().count(),
    );

    assert_eq!(owners, vec![None, Some(0), None]);
}

#[test]
fn conflict_document_line_model_preserves_empty_and_trailing_lines() {
    let view = ConflictFileView {
        path: "file.txt".into(),
        kind: ConflictFileKind::Text,
        draft: "before\n\nresult\n".into(),
        ours_text: "before\n\nours\n".into(),
        theirs_text: "before\n\ntheirs\n".into(),
        blocks: vec![ConflictBlock {
            base: Some("before\n\nbase\n".into()),
            ours: "ours\n".into(),
            theirs: "theirs\n".into(),
            start: 8,
            end: 15,
            ours_start: 8,
            ours_end: 13,
            theirs_start: 8,
            theirs_end: 15,
            status: ConflictBlockStatus::Unresolved,
            has_manual_edits: false,
        }],
        draft_status: ConflictDraftStatus::Dirty,
        fallback_reason: None,
    };

    let model = ConflictDocumentLineModel::new(&view.draft, ConflictDocumentPane::Result, &view);

    assert_eq!(model.line_count(), 4);
    assert_eq!(model.line_text(0), "before");
    assert_eq!(model.line_text(1), "");
    assert_eq!(model.line_text(2), "result");
    assert_eq!(model.line_text(3), "");
    assert_eq!(model.owner_at(2), Some(0));
}

#[test]
fn conflict_plain_line_model_preserves_empty_and_trailing_lines() {
    let model = ConflictPlainLineModel::new("base\n\nend\n");

    assert_eq!(model.line_count(), 4);
    assert_eq!(model.line_text(0), "base");
    assert_eq!(model.line_text(1), "");
    assert_eq!(model.line_text(2), "end");
    assert_eq!(model.line_text(3), "");
}

#[test]
fn conflict_line_colors_merged_uses_success_highlight_in_result_pane() {
    let block = ConflictBlock {
        base: None,
        ours: "ours\n".into(),
        theirs: "theirs\n".into(),
        start: 0,
        end: 5,
        ours_start: 0,
        ours_end: 5,
        theirs_start: 0,
        theirs_end: 6,
        status: ConflictBlockStatus::Merged,
        has_manual_edits: false,
    };

    // 结果区：选中绿色高亮（区别于未处理的黄色），非选中恢复普通行。
    assert_eq!(
        conflict_line_colors(ConflictDocumentPane::Result, &block, true),
        (
            ui_theme::FEEDBACK_SUCCESS_BG,
            ui_theme::FEEDBACK_SUCCESS_TEXT
        )
    );
    assert_eq!(
        conflict_line_colors(ConflictDocumentPane::Result, &block, false),
        (ui_theme::SURFACE_BASE, ui_theme::CONTENT_PRIMARY)
    );
    // 两侧栏与 Resolved 分支一致：非选中普通行、选中红色提示。
    assert_eq!(
        conflict_line_colors(ConflictDocumentPane::Ours, &block, false),
        (ui_theme::SURFACE_BASE, ui_theme::CONTENT_PRIMARY)
    );
    assert_eq!(
        conflict_line_colors(ConflictDocumentPane::Theirs, &block, true),
        (ui_theme::FEEDBACK_ERROR_BG, ui_theme::FEEDBACK_ERROR_TEXT)
    );
}

#[test]
fn conflict_error_background_always_uses_error_text_in_active_side_panes() {
    for status in [
        ConflictBlockStatus::Unresolved,
        ConflictBlockStatus::Resolved(ConflictBlockResolution::Ours),
        ConflictBlockStatus::Merged,
    ] {
        let block = ConflictBlock {
            base: None,
            ours: "ours\n".into(),
            theirs: "theirs\n".into(),
            start: 0,
            end: 5,
            ours_start: 0,
            ours_end: 5,
            theirs_start: 0,
            theirs_end: 6,
            status,
            has_manual_edits: false,
        };
        for pane in [ConflictDocumentPane::Ours, ConflictDocumentPane::Theirs] {
            assert_eq!(
                conflict_line_colors(pane, &block, true),
                (ui_theme::FEEDBACK_ERROR_BG, ui_theme::FEEDBACK_ERROR_TEXT),
                "active side pane must pair error background with error text for {status:?}"
            );
            let inactive_colors = if matches!(status, ConflictBlockStatus::Unresolved) {
                (ui_theme::FEEDBACK_WARNING_BG, ui_theme::CONTENT_PRIMARY)
            } else {
                (ui_theme::SURFACE_BASE, ui_theme::CONTENT_PRIMARY)
            };
            assert_eq!(
                conflict_line_colors(pane, &block, false),
                inactive_colors,
                "inactive side pane must preserve its existing neutral/warning treatment for {status:?}"
            );
        }
    }
}

#[test]
fn uniform_list_line_count_matches_line_range_count() {
    assert_eq!(uniform_list_line_count(""), 1);
    assert_eq!(uniform_list_line_count("abc"), 1);
    assert_eq!(uniform_list_line_count("a\nb"), 2);
    // 行尾换行产生一个尾空行，多算 1 行（与行区间切分一致）。
    assert_eq!(uniform_list_line_count("a\nb\n"), 3);
    for content in ["", "abc", "a\nb", "a\nb\n", "a\n\nb"] {
        assert_eq!(
            uniform_list_line_count(content),
            conflict_document_line_ranges(content).len(),
            "mismatch for {content:?}"
        );
    }
}

#[test]
fn conflict_block_y_range_applies_viewport_and_scroll_offset() {
    // 视口顶 100，向下滚 2 行（offset.y = -36），行高 18：
    // 第 5 行（索引 5）顶部 = 100 - 36 + 90 = 154。
    assert_eq!(
        conflict_block_y_range(100.0, -36.0, 18.0, &(5..6)),
        (154.0, 172.0)
    );
    // 多行块高度按行数累计。
    assert_eq!(
        conflict_block_y_range(100.0, 0.0, 18.0, &(2..5)),
        (136.0, 190.0)
    );
}

#[test]
fn conflict_connector_anchor_y_clamps_and_skips_invisible_blocks() {
    // 完全可见：取区域中点。
    assert_eq!(
        conflict_connector_anchor_y(0.0, 100.0, 20.0, 40.0),
        Some(30.0)
    );
    // 顶部被视口裁剪：钳到可视段后取中点。
    assert_eq!(
        conflict_connector_anchor_y(10.0, 100.0, 0.0, 40.0),
        Some(25.0)
    );
    // 底部被视口裁剪。
    assert_eq!(
        conflict_connector_anchor_y(0.0, 30.0, 20.0, 40.0),
        Some(25.0)
    );
    // 整段在视口外：不画线。
    assert_eq!(conflict_connector_anchor_y(100.0, 200.0, 10.0, 40.0), None);
    assert_eq!(conflict_connector_anchor_y(0.0, 10.0, 20.0, 40.0), None);
    // 零高度区域（防御）：不画线。
    assert_eq!(conflict_connector_anchor_y(0.0, 100.0, 30.0, 30.0), None);
}

#[test]
fn conflict_scroll_sync_source_picks_single_changed_pane() {
    let prev = [-100.0, -100.0, -100.0];
    // 恰好一栏变化（用户滚动该栏）：返回它作为源。
    assert_eq!(
        conflict_scroll_sync_source([-180.0, -100.0, -100.0], prev),
        Some(0)
    );
    assert_eq!(
        conflict_scroll_sync_source([-100.0, -100.0, -40.0], prev),
        Some(2)
    );
    // 全部未变（含亚像素抖动）：不同步。
    assert_eq!(
        conflict_scroll_sync_source([-100.0, -100.0, -99.7], prev),
        None
    );
    // 多栏同时变化（程序化三栏联动）：不同步。
    assert_eq!(
        conflict_scroll_sync_source([-180.0, -180.0, -100.0], prev),
        None
    );
    assert_eq!(
        conflict_scroll_sync_source([-180.0, -60.0, -40.0], prev),
        None
    );
}
