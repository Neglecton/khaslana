use super::{MULTILINE_LINE_HEIGHT, TextEditState, multiline_caret_follow_decision};

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

#[test]
fn multiline_caret_follow_scrolls_only_when_caret_moves_or_content_changes() {
    // 可视高度按 5 行（MULTILINE_MIN_LINES）计算，共 6 行内容。
    let container = MULTILINE_LINE_HEIGHT * 5.0;
    // 光标在第 6 行（索引 5），视口显示 1..6 行：光标可见，不滚动但刷新键
    let visible_top = MULTILINE_LINE_HEIGHT;
    let key = (10, 20);
    let (scroll, new_key) =
        multiline_caret_follow_decision(Some((9, 20)), key, 5, container, visible_top);
    assert_eq!(scroll, None);
    assert_eq!(new_key, Some(key));

    // 同一帧后用户手动上滚到 0..5 行，光标（第 6 行）出界：键未变 → 不回弹
    let (scroll, new_key) = multiline_caret_follow_decision(Some(key), key, 5, container, 0.0);
    assert_eq!(scroll, None);
    assert_eq!(new_key, Some(key));

    // 光标随后移动（键变化）：向下滚动到恰好看见光标行（底对齐）
    let next_key = (12, 21);
    let (scroll, new_key) = multiline_caret_follow_decision(Some(key), next_key, 5, container, 0.0);
    let expect = MULTILINE_LINE_HEIGHT * 6.0 - container;
    assert_eq!(scroll, Some(expect));
    assert_eq!(new_key, Some(next_key));

    // 光标移回首行、视口仍在底部：向上滚动顶对齐光标行
    let (scroll, _) = multiline_caret_follow_decision(
        Some(next_key),
        (0, 21),
        0,
        container,
        MULTILINE_LINE_HEIGHT,
    );
    assert_eq!(scroll, Some(0.0));

    // 首次渲染（无历史键）同样跟随
    let (scroll, new_key) = multiline_caret_follow_decision(None, key, 5, container, 0.0);
    assert_eq!(scroll, Some(MULTILINE_LINE_HEIGHT * 6.0 - container));
    assert_eq!(new_key, Some(key));
}
