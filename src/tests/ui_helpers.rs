use super::*;

#[test]
fn change_state_color_uses_semantic_status_colors() {
    assert_eq!(change_state_color(&ChangeState::Added), ui_theme::GIT_ADDED);
    assert_eq!(
        change_state_color(&ChangeState::Modified),
        ui_theme::COLOR_WARNING_FOREGROUND
    );
    assert_eq!(
        change_state_color(&ChangeState::Deleted),
        ui_theme::DESTRUCTIVE
    );
    assert_eq!(
        change_state_color(&ChangeState::Conflicted),
        ui_theme::DESTRUCTIVE
    );
    assert_eq!(
        change_state_color(&ChangeState::Untracked),
        ui_theme::MUTED_FOREGROUND
    );
}

#[test]
fn repo_initials_multi_segment() {
    // 多段名取前两段首字母。
    assert_eq!(repo_initials("mc-manager"), "MM");
    assert_eq!(repo_initials("opencl-ffm"), "OF");
    assert_eq!(repo_initials("a/b/c"), "AB");
}

#[test]
fn repo_initials_single_word() {
    // 单词名：首字母 + 首个内部大写字母；纯小写仅首字母。
    assert_eq!(repo_initials("EasyTier"), "ET");
    assert_eq!(repo_initials("qqBot"), "QB");
    assert_eq!(repo_initials("khaslana"), "K");
    assert_eq!(repo_initials("optical"), "O");
}

#[test]
fn repo_initials_empty() {
    assert_eq!(repo_initials(""), "?");
    assert_eq!(repo_initials("---"), "?");
}
