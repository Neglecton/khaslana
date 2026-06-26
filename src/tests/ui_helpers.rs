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
