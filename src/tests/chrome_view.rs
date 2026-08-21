use super::*;

#[test]
fn layout_policy_preserves_titlebar_at_required_bands() {
    let narrow = shell_layout_policy(1119.0);
    assert_eq!(narrow.band, LayoutBand::Narrow);
    assert!(!narrow.show_context_navigator);

    let standard = shell_layout_policy(1120.0);
    assert_eq!(standard.band, LayoutBand::Standard);
    assert!(standard.show_context_navigator);
    assert!(shell_layout_policy(1439.0).show_context_navigator);

    let comfortable = shell_layout_policy(1440.0);
    assert_eq!(comfortable.band, LayoutBand::Comfortable);
    assert!(comfortable.show_context_navigator);
}

#[test]
fn minimum_window_size_keeps_native_controls_reachable() {
    assert!(MIN_WINDOW_WIDTH >= 3.0 * 44.0);
    assert!(MIN_WINDOW_HEIGHT >= theme::TITLEBAR_HEIGHT + STATUS_BAR_HEIGHT);
    assert_eq!(
        shell_content_height(MIN_WINDOW_HEIGHT),
        MIN_WINDOW_HEIGHT - theme::TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT
    );
    assert_eq!(shell_content_height(40.0), 0.0);
}

#[test]
fn ultra_narrow_width_still_uses_narrow_policy() {
    let policy = shell_layout_policy(MIN_WINDOW_WIDTH);
    assert_eq!(policy.band, LayoutBand::Narrow);
    assert!(!policy.show_context_navigator);
}

#[test]
fn context_navigator_uses_docked_and_overlay_presentations() {
    let standard = shell_layout_policy(1120.0);
    assert_eq!(
        context_navigator_presentation(standard, MainMode::Worktree, true, false),
        ContextNavigatorPresentation::Docked
    );
    assert_eq!(
        context_navigator_presentation(
            shell_layout_policy(1119.0),
            MainMode::Worktree,
            true,
            false
        ),
        ContextNavigatorPresentation::Hidden
    );
    assert_eq!(
        context_navigator_presentation(shell_layout_policy(1119.0), MainMode::Worktree, true, true),
        ContextNavigatorPresentation::Overlay
    );
    assert_eq!(
        context_navigator_presentation(standard, MainMode::History, false, false),
        ContextNavigatorPresentation::Hidden
    );
}

#[test]
fn context_navigator_rejects_specialized_modes() {
    assert!(context_navigator_supported_mode(MainMode::History));
    assert!(!context_navigator_supported_mode(MainMode::Conflict));
    assert!(!context_navigator_supported_mode(MainMode::Stash));
    assert!(!context_navigator_supported_mode(MainMode::Browse));
    assert!(!context_navigator_supported_mode(MainMode::Blame));
    // 提交图谱页同为专用模式：Navigator 隐藏（收起窄条），模式图标仍是返回入口。
    assert!(!context_navigator_supported_mode(MainMode::CommitGraph));
    assert_eq!(
        context_navigator_presentation(shell_layout_policy(1119.0), MainMode::Conflict, true, true),
        ContextNavigatorPresentation::Hidden
    );
}
