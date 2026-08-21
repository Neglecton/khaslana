use super::*;

#[test]
fn blame_columns_keep_the_required_three_column_density() {
    assert_eq!(
        blame_columns_layout(),
        (BLAME_GUTTER_WIDTH, BLAME_LINENO_WIDTH, BLAME_ROW_HEIGHT)
    );
    assert_eq!(BLAME_GUTTER_WIDTH, 300.0);
    assert_eq!(BLAME_LINENO_WIDTH, 48.0);
    assert_eq!(BLAME_ROW_HEIGHT, 18.0);
}

#[test]
fn committed_blame_lines_use_sunken_gutter_and_syntax() {
    let visual = blame_line_visual_rule(false);
    assert_eq!(visual.row_background, ui_theme::SURFACE_BASE);
    assert_eq!(visual.gutter_background, ui_theme::SURFACE_SUNKEN);
    assert_eq!(visual.content_foreground, ui_theme::CONTENT_PRIMARY);
    assert!(visual.shows_syntax);
}

#[test]
fn uncommitted_blame_lines_use_warning_surface_without_syntax() {
    let visual = blame_line_visual_rule(true);
    assert_eq!(visual.row_background, ui_theme::FEEDBACK_WARNING_BG);
    assert_eq!(visual.gutter_background, ui_theme::FEEDBACK_WARNING_BG);
    assert_eq!(visual.content_foreground, ui_theme::FEEDBACK_WARNING_TEXT);
    assert!(!visual.shows_syntax);
}
