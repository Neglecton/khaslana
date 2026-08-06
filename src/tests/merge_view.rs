use super::{
    merge_allows_disruptive_action, merge_banner_message, merge_can_finish,
    merge_commit_button_label, merge_message_update,
};

#[test]
fn merge_banner_distinguishes_conflicts_from_ready_to_finish() {
    assert_eq!(merge_banner_message(2), "合并进行中 · 2 个冲突待解决");
    assert_eq!(
        merge_banner_message(0),
        "合并进行中 · 冲突已全部解决，请检查结果并完成合并"
    );
}

#[test]
fn merge_finish_action_requires_ready_state_and_message() {
    assert!(merge_can_finish(true, 0, false, "Merge branch 'feature'"));
    assert!(!merge_can_finish(true, 1, false, "message"));
    assert!(!merge_can_finish(true, 0, true, "message"));
    assert!(!merge_can_finish(true, 0, false, "  "));
    assert!(!merge_can_finish(false, 0, false, "message"));
    assert_eq!(merge_commit_button_label(true), "完成合并");
    assert_eq!(merge_commit_button_label(false), "提交");
}

#[test]
fn merge_state_restores_message_and_blocks_disruptive_actions() {
    assert_eq!(
        merge_message_update(false, true, Some("Merge branch 'feature'")),
        Some("Merge branch 'feature'".into())
    );
    assert_eq!(merge_message_update(true, true, Some("默认信息")), None);
    assert_eq!(merge_message_update(true, false, None), Some(String::new()));
    assert!(merge_allows_disruptive_action(false));
    assert!(!merge_allows_disruptive_action(true));
}
