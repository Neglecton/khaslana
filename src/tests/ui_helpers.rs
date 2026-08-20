use super::*;

#[test]
fn change_state_color_uses_semantic_status_colors() {
    // GitHub 风格 Git 状态区分色：绿增 / 蓝改 / 红删 / 亮红冲突 / 橙更名 / 灰未跟踪
    assert_eq!(change_state_color(&ChangeState::Added), ui_theme::GIT_ADDED);
    assert_eq!(
        change_state_color(&ChangeState::Modified),
        ui_theme::GIT_MODIFIED
    );
    assert_eq!(
        change_state_color(&ChangeState::Deleted),
        ui_theme::GIT_REMOVED
    );
    assert_eq!(
        change_state_color(&ChangeState::Conflicted),
        ui_theme::DESTRUCTIVE
    );
    assert_eq!(
        change_state_color(&ChangeState::Renamed),
        ui_theme::GIT_RENAMED
    );
    assert_eq!(
        change_state_color(&ChangeState::Typechange),
        ui_theme::GIT_RENAMED
    );
    assert_eq!(
        change_state_color(&ChangeState::Untracked),
        ui_theme::GIT_UNTRACKED
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

#[test]
fn avatar_palette_pairs_are_distinct() {
    // 每对 (底色, 渐变伙伴色) 两色必须不同，否则头像渐变退化成纯色
    for (base, gradient_to) in AVATAR_PALETTE {
        assert_ne!(base, gradient_to);
    }
    // 相同键稳定取同一对颜色
    assert_eq!(
        avatar_palette_colors("alice"),
        avatar_palette_colors("alice")
    );
    assert_eq!(avatar_palette_colors(""), avatar_palette_colors(""));
}
