use super::*;
use khaslana::{ConflictBlockStatus, ConflictDraftStatus};

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
        conflict_status_message("合并完成", 2),
        "合并产生冲突，请在左侧“冲突”区域解决（2 个文件）"
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
