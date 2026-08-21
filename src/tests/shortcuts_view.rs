use super::*;

#[test]
fn format_keystroke_ctrl_shift_combo() {
    assert_eq!(format_keystroke("ctrl-shift-f"), "Ctrl+Shift+F");
    assert_eq!(format_keystroke("ctrl-shift-l"), "Ctrl+Shift+L");
}

#[test]
fn format_keystroke_single_key() {
    assert_eq!(format_keystroke("f5"), "F5");
}

#[test]
fn format_keystroke_ctrl_comma() {
    assert_eq!(format_keystroke("ctrl-,"), "Ctrl+,");
}

#[test]
fn format_keystroke_ctrl_number() {
    assert_eq!(format_keystroke("ctrl-1"), "Ctrl+1");
}

#[test]
fn find_conflict_detects_other_action() {
    // 默认绑定中 refresh=f5, fetch=ctrl-shift-f。
    let bindings = crate::default_shortcut_bindings();
    // 给 refresh 绑定 ctrl-shift-f（fetch 的默认）应冲突到 fetch。
    let conflict =
        crate::find_shortcut_conflict(&bindings, ShortcutAction::Refresh, "ctrl-shift-f");
    assert_eq!(conflict, Some(ShortcutAction::Fetch));
}

#[test]
fn find_conflict_no_self_conflict() {
    let bindings = crate::default_shortcut_bindings();
    // 一个动作的当前绑定不与自己冲突。
    let conflict = crate::find_shortcut_conflict(&bindings, ShortcutAction::Refresh, "f5");
    assert_eq!(conflict, None);
}

#[test]
fn recording_visual_state_only_marks_the_active_action() {
    assert!(shortcut_row_is_recording(
        Some(ShortcutAction::Refresh),
        ShortcutAction::Refresh
    ));
    assert!(!shortcut_row_is_recording(
        Some(ShortcutAction::Refresh),
        ShortcutAction::Fetch
    ));
    assert!(!shortcut_row_is_recording(None, ShortcutAction::Refresh));
}

#[test]
fn shortcut_controls_accept_enter_and_space_only() {
    assert!(shortcut_button_key_activates("enter"));
    assert!(shortcut_button_key_activates("space"));
    assert!(!shortcut_button_key_activates("escape"));
}

#[test]
fn reset_default_is_disabled_during_recording_with_reason() {
    assert!(shortcut_reset_enabled(None));
    assert!(!shortcut_reset_enabled(Some(ShortcutAction::Refresh)));
    assert_eq!(
        shortcut_reset_disabled_reason(Some(ShortcutAction::Refresh)),
        Some("请先结束快捷键录制")
    );
    assert_eq!(shortcut_reset_disabled_reason(None), None);
}
