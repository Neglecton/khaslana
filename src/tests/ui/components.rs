use super::*;

#[test]
fn flat_list_row_rule_only_marks_selected_rows() {
    let idle = list_row_visual_rule(false);
    assert_eq!(idle.background, theme::SURFACE_BASE);
    assert!(!idle.shows_selection_indicator);

    let selected = list_row_visual_rule(true);
    assert_eq!(selected.background, theme::PRIMARY_SUBTLE);
    assert!(selected.shows_selection_indicator);
}

#[test]
fn focusable_icon_button_activation_keys_are_limited_to_enter_and_space() {
    assert!(icon_button_key_activates("enter"));
    assert!(icon_button_key_activates("space"));
    assert!(!icon_button_key_activates("escape"));
    assert!(!icon_button_key_activates("a"));
}
