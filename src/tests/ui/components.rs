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
