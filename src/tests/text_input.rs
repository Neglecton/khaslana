use super::TextEditState;

#[test]
fn text_field_edits_at_utf8_char_boundaries() {
    let mut field = TextEditState::for_test("ab你cd", false);

    field.move_caret_to(3, false);
    assert_eq!(field.caret, 2);
    field.insert_text("X", false);
    assert_eq!(field.value, "abX你cd");

    field.delete_backward();
    assert_eq!(field.value, "ab你cd");
    assert_eq!(field.caret, 2);

    field.move_caret_to("ab你".len(), false);
    field.delete_backward();
    assert_eq!(field.value, "abcd");
    assert_eq!(field.caret, 2);

    field.set_value("ab你cd");
    field.move_caret_to(2, false);
    field.delete_forward();
    assert_eq!(field.value, "abcd");
    assert_eq!(field.caret, 2);
}

#[test]
fn text_field_selection_replace_and_navigation_work() {
    let mut field = TextEditState::for_test("abcdef", false);

    field.move_caret_to(2, false);
    field.move_caret_to(5, true);
    assert_eq!(field.selected_text().as_deref(), Some("cde"));

    field.insert_text("X", false);
    assert_eq!(field.value, "abXf");
    assert_eq!(field.caret, 3);
    assert_eq!(field.selected_range(), None);

    field.select_all();
    assert_eq!(field.selected_text().as_deref(), Some("abXf"));
    field.move_left(false);
    assert_eq!(field.caret, 0);
    assert_eq!(field.selected_range(), None);

    field.move_right(false);
    assert_eq!(field.caret, 1);
}

#[test]
fn text_field_single_line_paste_strips_newlines() {
    let mut single_line = TextEditState::for_test("ab", false);
    single_line.move_caret_to(1, false);
    single_line.insert_text("x\ny\r\nz", false);
    assert_eq!(single_line.value, "axyzb");

    let mut multiline = TextEditState::for_test("ab", false);
    multiline.move_caret_to(1, false);
    multiline.insert_text("x\ny", true);
    assert_eq!(multiline.value, "ax\nyb");
}

#[test]
fn text_field_secret_display_masks_and_blocks_copyable_text() {
    let mut field = TextEditState::for_test("密码12", true);

    assert_eq!(field.display_text(), "****");
    assert_eq!(field.display_byte_for_value_byte("密码".len()), 2);

    field.select_all();
    assert_eq!(field.selected_text().as_deref(), Some("密码12"));
    assert_eq!(field.copyable_selected_text(), None);

    field.clear();
    assert!(field.value.is_empty());
    assert_eq!(field.caret, 0);
    assert_eq!(field.selected_range(), None);
}

#[test]
fn text_field_utf16_ranges_round_trip() {
    let field = TextEditState::for_test("a你😀b", false);
    let range = "a你".len().."a你😀".len();

    assert_eq!(field.range_to_utf16(&range), 2..4);
    assert_eq!(field.range_from_utf16(&(2..4)), range);
    assert_eq!(field.text_for_utf16_range(&(1..4)), "你😀");
}

#[test]
fn text_field_grapheme_navigation_keeps_emoji_together() {
    let mut field = TextEditState::for_test("a👨‍👩‍👧‍👦b", false);
    field.move_caret_to(field.value.len(), false);

    field.move_left(false);
    assert_eq!(field.caret, "a👨‍👩‍👧‍👦".len());
    field.move_left(false);
    assert_eq!(field.caret, 1);
    field.delete_forward();
    assert_eq!(field.value, "ab");
}

#[test]
fn text_field_platform_replacement_strips_newlines() {
    let mut field = TextEditState::for_test("ab", false);
    field.move_caret_to(1, false);

    field.replace_text_in_utf16_range_with_mode(None, "x\ny\r\nz", false);
    assert_eq!(field.value, "axyzb");
    assert_eq!(field.caret, 4);
}

#[test]
fn text_field_platform_replacement_can_keep_multiline_text() {
    let mut field = TextEditState::for_test("ab", false);
    field.move_caret_to(1, false);

    field.replace_text_in_utf16_range_with_mode(None, "x\ny\r\nz", true);
    assert_eq!(field.value, "ax\ny\nzb");
    assert_eq!(field.caret, "ax\ny\nz".len());
}

#[test]
fn text_field_marked_text_replacement_updates_selection() {
    let mut field = TextEditState::for_test("ab", false);
    field.move_caret_to(1, false);

    field.replace_and_mark_text_in_utf16_range_with_mode(None, "你", Some(1..1), false);
    assert_eq!(field.value, "a你b");
    assert_eq!(field.marked_range, Some(1.."a你".len()));
    assert_eq!(field.caret, "a你".len());
    assert_eq!(field.selected_range(), None);
}

#[test]
fn text_field_secret_utf16_text_is_masked() {
    let field = TextEditState::for_test("密码12", true);

    assert_eq!(field.text_for_utf16_range(&(0..4)), "****");
}
