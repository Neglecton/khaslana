use super::*;

#[test]
fn stash_file_row_visual_rule_is_flat_and_token_driven() {
    let idle = stash_file_row_visual_rule(false);
    assert_eq!(idle.background, ui_theme::SURFACE_BASE);
    assert_eq!(idle.text, ui_theme::CONTENT_PRIMARY);
    assert!(!idle.selected);

    let selected = stash_file_row_visual_rule(true);
    assert_eq!(selected.background, ui_theme::STATE_SELECTION);
    assert_eq!(selected.text, ui_theme::CONTENT_PRIMARY);
    assert!(selected.selected);
}

#[test]
fn stash_preview_state_presence_tracks_selected_stash_only() {
    let mut state = StashPreviewState::default();
    assert!(!state.is_showing());

    state.stash_oid = Some("deadbeef".to_string());
    assert!(state.is_showing());

    state.clear();
    assert!(!state.is_showing());
    assert!(state.files.is_empty());
    assert!(state.selected_file.is_none());
}
