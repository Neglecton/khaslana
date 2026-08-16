use std::ops::{Deref, DerefMut, Range};

use gpui::{
    App, Bounds, Context, Element, ElementId, ElementInputHandler, FocusHandle, GlobalElementId,
    IntoElement, LayoutId, PaintQuad, Pixels, ShapedLine, SharedString, Style, TextRun, Window,
    fill, point, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::theme as ui_theme;
use crate::ui::theme::{rgb, rgba};
use crate::{FieldId, RepositoryView, multiline_scroll_handle_id};

/// 多行输入单行高度（px）。
pub(crate) const MULTILINE_LINE_HEIGHT: f32 = 18.0;
/// 多行输入最小可视行数：同时是提交信息框的固定可视高度
///（内容超过该行数后滚动而非继续撑高）。
pub(crate) const MULTILINE_MIN_LINES: usize = 5;

/// 多行输入光标跟随滚动的决策（纯函数，便于单测）。
///
/// - 跟随键（光标字节, 内容长度）与上次相同 → 视为用户手动滚动，不回弹
///   也不刷新键（键变化才代表光标移动或内容改变）；
/// - 键变化但光标行仍在可视区域内 → 不滚动；
/// - 键变化且光标行越出可视区域 → 滚动到恰好可见（向下滚动底对齐光标行、
///   向上滚动顶对齐）。
///
/// 返回 (新滚动顶部的内容坐标 px（None 表示本次不滚动）, 新跟随键)。
/// 调用方无论是否滚动都应写入新跟随键：光标在可视区内移动后用户再手动
/// 滚走，残留旧键会让下一次 prepaint 误判为光标移动而回弹。
pub(crate) fn multiline_caret_follow_decision(
    last_key: Option<(usize, usize)>,
    key: (usize, usize),
    caret_line: usize,
    container_height: f32,
    visible_top: f32,
) -> (Option<f32>, Option<(usize, usize)>) {
    if last_key == Some(key) {
        return (None, last_key);
    }
    let caret_top = MULTILINE_LINE_HEIGHT * caret_line as f32;
    let caret_bottom = caret_top + MULTILINE_LINE_HEIGHT;
    let scroll_top = if caret_bottom > visible_top + container_height {
        Some(caret_bottom - container_height)
    } else if caret_top < visible_top {
        Some(caret_top)
    } else {
        None
    };
    (scroll_top, Some(key))
}

#[derive(Clone, Debug)]
pub(crate) struct TextLineLayout {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line: ShapedLine,
    pub(crate) bounds: Bounds<Pixels>,
}

#[derive(Clone, Debug)]
pub(crate) struct TextEditState {
    pub(crate) value: String,
    pub(crate) secret: bool,
    pub(crate) caret: usize,
    pub(crate) selection_anchor: Option<usize>,
    pub(crate) marked_range: Option<Range<usize>>,
    pub(crate) last_layout: Option<ShapedLine>,
    pub(crate) last_bounds: Option<Bounds<Pixels>>,
    pub(crate) last_multiline_layout: Vec<TextLineLayout>,
    pub(crate) is_selecting: bool,
    /// 上一次 prepaint 计算出的视觉行数（含自动换行），供下次 request_layout 复用。
    pub(crate) last_wrapped_line_count: usize,
    /// 上一次光标跟随滚动对应的（光标字节，内容长度）。
    /// 仅当键变化（光标移动或内容改变）时才重新跟随滚动；
    /// 键不变说明是用户手动滚动，不把视口拉回光标处。
    pub(crate) last_caret_follow: Option<(usize, usize)>,
}

impl TextEditState {
    pub(crate) fn new() -> Self {
        Self {
            value: String::new(),
            secret: false,
            caret: 0,
            selection_anchor: None,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            last_multiline_layout: Vec::new(),
            is_selecting: false,
            last_wrapped_line_count: MULTILINE_MIN_LINES,
            last_caret_follow: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str, secret: bool) -> Self {
        Self {
            value: value.to_string(),
            secret,
            caret: value.len(),
            selection_anchor: None,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            last_multiline_layout: Vec::new(),
            is_selecting: false,
            last_wrapped_line_count: MULTILINE_MIN_LINES,
            last_caret_follow: None,
        }
    }

    pub(crate) fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.caret = self.value.len();
        self.selection_anchor = None;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.last_multiline_layout.clear();
        self.is_selecting = false;
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.caret = 0;
        self.selection_anchor = None;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.last_multiline_layout.clear();
        self.is_selecting = false;
    }

    pub(crate) fn display_text(&self) -> String {
        if self.secret {
            "*".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }

    pub(crate) fn display_byte_for_value_byte(&self, value_byte: usize) -> usize {
        if self.secret {
            self.value[..value_byte].chars().count()
        } else {
            value_byte
        }
    }

    fn value_byte_for_display_byte(&self, display_byte: usize) -> usize {
        if !self.secret {
            return clamp_to_char_boundary(&self.value, display_byte);
        }
        if display_byte == 0 {
            return 0;
        }
        self.value
            .char_indices()
            .nth(display_byte)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(crate) fn selected_range(&self) -> Option<Range<usize>> {
        let anchor = self.selection_anchor?;
        if anchor == self.caret {
            None
        } else if anchor < self.caret {
            Some(anchor..self.caret)
        } else {
            Some(self.caret..anchor)
        }
    }

    pub(crate) fn input_range(&self) -> Range<usize> {
        self.selected_range().unwrap_or(self.caret..self.caret)
    }

    pub(crate) fn selection_reversed(&self) -> bool {
        self.selection_anchor
            .is_some_and(|anchor| self.caret < anchor)
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.selected_range()
            .map(|range| self.value[range].to_string())
    }

    pub(crate) fn copyable_selected_text(&self) -> Option<String> {
        (!self.secret).then(|| self.selected_text()).flatten()
    }

    pub(crate) fn select_all(&mut self) {
        self.caret = self.value.len();
        self.selection_anchor = Some(0);
        self.marked_range = None;
    }

    pub(crate) fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selected_range() else {
            return false;
        };
        let start = range.start;
        self.value.replace_range(range, "");
        self.caret = start;
        self.selection_anchor = None;
        self.marked_range = None;
        true
    }

    pub(crate) fn insert_text(&mut self, text: &str, multiline: bool) {
        self.delete_selection();
        let text = normalize_inserted_text(text, multiline);
        self.value.insert_str(self.caret, &text);
        self.caret += text.len();
        self.selection_anchor = None;
        self.marked_range = None;
    }

    pub(crate) fn delete_backward(&mut self) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let previous = self.previous_grapheme_boundary(self.caret);
        self.value.replace_range(previous..self.caret, "");
        self.caret = previous;
    }

    pub(crate) fn delete_forward(&mut self) {
        if self.delete_selection() || self.caret >= self.value.len() {
            return;
        }
        let next = self.next_grapheme_boundary(self.caret);
        self.value.replace_range(self.caret..next, "");
    }

    pub(crate) fn move_caret_to(&mut self, position: usize, extend_selection: bool) {
        let position = clamp_to_char_boundary(&self.value, position);
        if extend_selection {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.caret);
            }
        } else {
            self.selection_anchor = None;
        }
        self.caret = position;
        self.marked_range = None;
    }

    pub(crate) fn move_to(&mut self, position: usize) {
        self.move_caret_to(position, false);
    }

    pub(crate) fn select_to(&mut self, position: usize) {
        self.move_caret_to(position, true);
    }

    pub(crate) fn move_left(&mut self, extend_selection: bool) {
        if !extend_selection && let Some(range) = self.selected_range() {
            self.move_caret_to(range.start, false);
            return;
        }
        let previous = self.previous_grapheme_boundary(self.caret);
        self.move_caret_to(previous, extend_selection);
    }

    pub(crate) fn move_right(&mut self, extend_selection: bool) {
        if !extend_selection && let Some(range) = self.selected_range() {
            self.move_caret_to(range.end, false);
            return;
        }
        let next = self.next_grapheme_boundary(self.caret);
        self.move_caret_to(next, extend_selection);
    }

    pub(crate) fn move_to_line_start(&mut self, extend_selection: bool) {
        let start = self
            .line_layout_for_caret()
            .map(|line| line.start)
            .unwrap_or(0);
        self.move_caret_to(start, extend_selection);
    }

    pub(crate) fn move_to_line_end(&mut self, extend_selection: bool) {
        let end = self
            .line_layout_for_caret()
            .map(|line| line.end)
            .unwrap_or(self.value.len());
        self.move_caret_to(end, extend_selection);
    }

    pub(crate) fn move_vertical(&mut self, direction: i32, extend_selection: bool) {
        if self.last_multiline_layout.is_empty() {
            return;
        }
        let Some(current_index) = self.line_index_for_caret() else {
            return;
        };
        let target_index = if direction < 0 {
            current_index.saturating_sub(1)
        } else {
            (current_index + 1).min(self.last_multiline_layout.len().saturating_sub(1))
        };
        if target_index == current_index {
            return;
        }
        let current = &self.last_multiline_layout[current_index];
        let target = &self.last_multiline_layout[target_index];
        let local_caret = self.caret.saturating_sub(current.start);
        let x = current
            .line
            .x_for_index(local_caret.min(current.end - current.start));
        let local_target = target.line.closest_index_for_x(x);
        self.move_caret_to(
            clamp_to_char_boundary(
                &self.value,
                target.start + local_target.min(target.end - target.start),
            ),
            extend_selection,
        );
    }

    fn previous_grapheme_boundary(&self, offset: usize) -> usize {
        self.value
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_grapheme_boundary(&self, offset: usize) -> usize {
        self.value
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.value.len())
    }

    pub(crate) fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.value.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    pub(crate) fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.value.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    pub(crate) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(crate) fn replace_text_in_utf16_range_with_mode(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        multiline: bool,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.input_range());
        let text = normalize_inserted_text(text, multiline);
        self.value.replace_range(range.clone(), &text);
        self.caret = range.start + text.len();
        self.selection_anchor = None;
        self.marked_range = None;
    }

    pub(crate) fn replace_and_mark_text_in_utf16_range_with_mode(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_range_utf16: Option<Range<usize>>,
        multiline: bool,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.input_range());
        let text = normalize_inserted_text(text, multiline);
        let selected_range = selected_range_utf16
            .as_ref()
            .map(|range| {
                offset_from_utf16_in_text(&text, range.start)
                    ..offset_from_utf16_in_text(&text, range.end)
            })
            .unwrap_or_else(|| text.len()..text.len());
        self.value.replace_range(range.clone(), &text);
        if text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + text.len());
        }
        self.caret = range.start + selected_range.end;
        self.selection_anchor = Some(range.start + selected_range.start);
        if self.selection_anchor == Some(self.caret) {
            self.selection_anchor = None;
        }
    }

    pub(crate) fn text_for_utf16_range(&self, range_utf16: &Range<usize>) -> String {
        let range = self.range_from_utf16(range_utf16);
        if self.secret {
            "*".repeat(self.value[range].chars().count())
        } else {
            self.value[range].to_string()
        }
    }

    pub(crate) fn index_for_mouse_position(&self, position: gpui::Point<Pixels>) -> usize {
        if !self.last_multiline_layout.is_empty() {
            return self.multiline_index_for_mouse_position(position);
        }
        if self.value.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.value.len();
        }
        let display_byte = line.closest_index_for_x(position.x - bounds.left());
        self.value_byte_for_display_byte(display_byte)
    }

    pub(crate) fn bounds_for_utf16_range(
        &self,
        range_utf16: &Range<usize>,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        if !self.last_multiline_layout.is_empty() {
            return self.multiline_bounds_for_utf16_range(range_utf16);
        }
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(range_utf16);
        let display_range = self.display_byte_for_value_byte(range.start)
            ..self.display_byte_for_value_byte(range.end);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(display_range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(display_range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn multiline_index_for_mouse_position(&self, position: gpui::Point<Pixels>) -> usize {
        if self.value.is_empty() {
            return 0;
        }
        let Some(first) = self.last_multiline_layout.first() else {
            return 0;
        };
        if position.y < first.bounds.top() {
            return 0;
        }
        let line = self
            .last_multiline_layout
            .iter()
            .find(|line| position.y <= line.bounds.bottom())
            .or_else(|| self.last_multiline_layout.last())
            .unwrap();
        let local = line
            .line
            .closest_index_for_x(position.x - line.bounds.left())
            .min(line.end - line.start);
        clamp_to_char_boundary(&self.value, line.start + local)
    }

    fn multiline_bounds_for_utf16_range(
        &self,
        range_utf16: &Range<usize>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(range_utf16);
        let start_line = self.line_layout_for_offset(range.start)?;
        let end_line = self.line_layout_for_offset(range.end)?;
        if start_line.start == end_line.start {
            let start = range.start.saturating_sub(start_line.start);
            let end = range.end.saturating_sub(start_line.start);
            return Some(Bounds::from_corners(
                point(
                    start_line.bounds.left() + start_line.line.x_for_index(start),
                    start_line.bounds.top(),
                ),
                point(
                    start_line.bounds.left() + start_line.line.x_for_index(end),
                    start_line.bounds.bottom(),
                ),
            ));
        }
        Some(Bounds::from_corners(
            point(
                start_line.bounds.left()
                    + start_line
                        .line
                        .x_for_index(range.start.saturating_sub(start_line.start)),
                start_line.bounds.top(),
            ),
            point(end_line.bounds.right(), end_line.bounds.bottom()),
        ))
    }

    fn line_index_for_caret(&self) -> Option<usize> {
        self.last_multiline_layout
            .iter()
            .position(|line| self.caret >= line.start && self.caret <= line.end)
            .or_else(|| self.last_multiline_layout.len().checked_sub(1))
    }

    fn line_layout_for_caret(&self) -> Option<&TextLineLayout> {
        let index = self.line_index_for_caret()?;
        self.last_multiline_layout.get(index)
    }

    fn line_layout_for_offset(&self, offset: usize) -> Option<&TextLineLayout> {
        self.last_multiline_layout
            .iter()
            .find(|line| offset >= line.start && offset <= line.end)
            .or_else(|| self.last_multiline_layout.last())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TextFieldState {
    pub(crate) focus: FocusHandle,
    pub(crate) placeholder: SharedString,
    edit: TextEditState,
}

impl TextFieldState {
    pub(crate) fn new(
        cx: &mut Context<RepositoryView>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            placeholder: placeholder.into(),
            edit: TextEditState::new(),
        }
    }

    pub(crate) fn with_value(mut self, value: impl Into<String>) -> Self {
        self.edit.set_value(value);
        self
    }

    pub(crate) fn secret(mut self) -> Self {
        self.edit.secret = true;
        self
    }
}

impl Deref for TextFieldState {
    type Target = TextEditState;

    fn deref(&self) -> &Self::Target {
        &self.edit
    }
}

impl DerefMut for TextFieldState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.edit
    }
}

fn clamp_to_char_boundary(value: &str, mut position: usize) -> usize {
    position = position.min(value.len());
    while position > 0 && !value.is_char_boundary(position) {
        position -= 1;
    }
    position
}

fn offset_from_utf16_in_text(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for ch in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }
    utf8_offset
}

fn normalize_inserted_text(text: &str, multiline: bool) -> String {
    if multiline {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.replace(['\r', '\n'], "")
    }
}

pub(crate) struct SingleLineInputElement {
    pub(crate) field_id: FieldId,
    pub(crate) entity: gpui::Entity<RepositoryView>,
}

pub(crate) struct SingleLineInputPrepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

pub(crate) struct MultiLineInputElement {
    pub(crate) field_id: FieldId,
    pub(crate) entity: gpui::Entity<RepositoryView>,
}

pub(crate) struct MultiLineInputPrepaint {
    lines: Vec<TextLineLayout>,
    placeholder: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for SingleLineInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SingleLineInputElement {
    type RequestLayoutState = ();
    type PrepaintState = SingleLineInputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.entity.read(cx);
        let field = view.field(self.field_id);
        let display = field.display_text();
        let is_empty = field.value.is_empty();
        let style = window.text_style();
        let display_text: SharedString = if is_empty {
            field.placeholder.clone()
        } else {
            display.clone().into()
        };
        let text_color: gpui::Hsla = if is_empty {
            rgba(ui_theme::INPUT_PLACEHOLDER).into()
        } else {
            style.color
        };
        let base_run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if !is_empty {
            if let Some(marked_range) = field.marked_range.as_ref() {
                let marked_start = field.display_byte_for_value_byte(marked_range.start);
                let marked_end = field.display_byte_for_value_byte(marked_range.end);
                vec![
                    TextRun {
                        len: marked_start,
                        ..base_run.clone()
                    },
                    TextRun {
                        len: marked_end.saturating_sub(marked_start),
                        underline: Some(gpui::UnderlineStyle {
                            color: Some(base_run.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..base_run.clone()
                    },
                    TextRun {
                        len: display_text.len().saturating_sub(marked_end),
                        ..base_run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            } else {
                vec![base_run]
            }
        } else {
            vec![base_run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text.clone(), font_size, &runs, None);
        let focused = field.focus.is_focused(window);
        let selection = if !is_empty {
            field.selected_range().map(|range| {
                let start = field.display_byte_for_value_byte(range.start);
                let end = field.display_byte_for_value_byte(range.end);
                fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(start), bounds.top()),
                        point(bounds.left() + line.x_for_index(end), bounds.bottom()),
                    ),
                    rgba(ui_theme::INPUT_SELECTION),
                )
            })
        } else {
            None
        };
        let cursor = if focused && selection.is_none() {
            let caret = if is_empty {
                0
            } else {
                field.display_byte_for_value_byte(field.caret)
            };
            Some(fill(
                Bounds::new(
                    point(bounds.left() + line.x_for_index(caret), bounds.top()),
                    size(px(1.5), bounds.bottom() - bounds.top()),
                ),
                rgb(ui_theme::INPUT_CARET),
            ))
        } else {
            None
        };
        SingleLineInputPrepaint {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.entity.read(cx).field(self.field_id).focus.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().unwrap_or_default();
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
        self.entity.update(cx, |view, _cx| {
            let field = view.field_mut(self.field_id);
            field.last_layout = Some(line);
            field.last_bounds = Some(bounds);
        });
    }
}

impl IntoElement for MultiLineInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MultiLineInputElement {
    type RequestLayoutState = ();
    type PrepaintState = MultiLineInputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let field = self.entity.read(cx).field(self.field_id);
        // 视觉行数优先用上一次 prepaint 算出的自动换行行数；首次渲染时按逻辑行数估算，
        // 至少保留 MIN_LINES 的高度。换行宽度变化后 prepaint 会更新该值并触发重排。
        let logical = logical_line_ranges(&field.value)
            .len()
            .max(MULTILINE_MIN_LINES);
        let line_count = field.last_wrapped_line_count.max(logical);
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = px(MULTILINE_LINE_HEIGHT * line_count as f32).into();
        // 固定高度的滚动容器内禁止收缩：布局引擎把 min-height:auto 解析为 0，
        // 默认 flex_shrink=1 会把本元素压回容器高度，导致内容测量不溢出、
        // 滚动与滚动条失效（行仍在绘制、只是被裁掉）。
        style.flex_shrink = 0.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.entity.read(cx);
        let field = view.field(self.field_id);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = px(MULTILINE_LINE_HEIGHT);
        let focused = field.focus.is_focused(window);

        if field.value.is_empty() {
            let run = TextRun {
                len: field.placeholder.len(),
                font: style.font(),
                color: rgba(ui_theme::INPUT_PLACEHOLDER).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let placeholder =
                window
                    .text_system()
                    .shape_line(field.placeholder.clone(), font_size, &[run], None);
            let cursor = focused.then(|| {
                fill(
                    Bounds::new(bounds.origin, size(px(1.5), line_height)),
                    rgb(ui_theme::INPUT_CARET),
                )
            });
            return MultiLineInputPrepaint {
                lines: Vec::new(),
                placeholder: Some(placeholder),
                cursor,
                selections: Vec::new(),
            };
        }

        let mut lines = Vec::new();
        let mut selections = Vec::new();
        let mut cursor = None;
        // 光标所在视觉行（跟随滚动用）。
        let mut caret_visual_line: Option<usize> = None;
        let wrap_width = bounds.size.width.max(px(1.0));
        // 用 shape_text 一次性把完整文本（含 \n）按 wrap_width 做自动换行，
        // 得到每个逻辑行对应的 WrappedLine（含换行边界）。
        let value = &field.value;
        let wrapped_lines = window
            .text_system()
            .shape_text(
                SharedString::from(value.clone()),
                font_size,
                &multiline_text_runs(field, &style, &(0..value.len()), value.len()),
                Some(wrap_width),
                None,
            )
            .unwrap_or_default();

        let mut visual_line_index = 0usize;
        // shape_text 内部按 \n 拆分逻辑行；这里需要重建每个逻辑行在 value 中的字节起点。
        let logical_ranges = logical_line_ranges(value);
        for (logical_idx, wrapped_line) in wrapped_lines.iter().enumerate() {
            let logical_range = logical_ranges.get(logical_idx).cloned().unwrap_or(0..0);
            let boundaries = wrapped_line.wrap_boundaries();
            // 每个视觉行覆盖 [seg_start, seg_end) 字节区间（相对于完整 value）。
            let mut seg_starts: Vec<usize> = vec![logical_range.start];
            for boundary in boundaries {
                let glyph_index = wrapped_line.unwrapped_layout.runs[boundary.run_ix].glyphs
                    [boundary.glyph_ix]
                    .index;
                seg_starts.push(logical_range.start + glyph_index);
            }
            let seg_ends: Vec<usize> = seg_starts[1..]
                .iter()
                .copied()
                .chain([logical_range.end])
                .collect();

            for (&seg_start, &seg_end) in seg_starts.iter().zip(seg_ends.iter()) {
                let seg_text: SharedString = value[seg_start..seg_end].to_string().into();
                let seg_range = seg_start..seg_end;
                let seg_runs = multiline_text_runs(field, &style, &seg_range, seg_text.len());
                let shaped = window
                    .text_system()
                    .shape_line(seg_text, font_size, &seg_runs, None);
                let top = bounds.top() + px(MULTILINE_LINE_HEIGHT * visual_line_index as f32);
                let line_bounds = Bounds::new(
                    point(bounds.left(), top),
                    size(bounds.size.width, line_height),
                );

                if let Some(selection) = field.selected_range()
                    && let Some(overlap) = range_overlap(&selection, &seg_range)
                {
                    let start = overlap.start.saturating_sub(seg_range.start);
                    let end = overlap.end.saturating_sub(seg_range.start);
                    selections.push(fill(
                        Bounds::from_corners(
                            point(
                                line_bounds.left() + shaped.x_for_index(start),
                                line_bounds.top(),
                            ),
                            point(
                                line_bounds.left() + shaped.x_for_index(end),
                                line_bounds.bottom(),
                            ),
                        ),
                        rgba(ui_theme::INPUT_SELECTION),
                    ));
                }

                if field.selected_range().is_none()
                    && field.caret >= seg_range.start
                    && field.caret <= seg_range.end
                {
                    let local = field.caret.saturating_sub(seg_range.start);
                    // 光标跟随滚动不要求聚焦：AI 流式生成等场景下 caret 被推到
                    // 末尾时，未聚焦的输入框也能自动滚到最新内容。
                    caret_visual_line = Some(visual_line_index);
                    if focused {
                        cursor = Some(fill(
                            Bounds::new(
                                point(
                                    line_bounds.left() + shaped.x_for_index(local),
                                    line_bounds.top(),
                                ),
                                size(px(1.5), line_height),
                            ),
                            rgb(ui_theme::INPUT_CARET),
                        ));
                    }
                }

                lines.push(TextLineLayout {
                    start: seg_start,
                    end: seg_end,
                    line: shaped,
                    bounds: line_bounds,
                });
                visual_line_index += 1;
            }
        }

        // 光标跟随滚动：仅当光标移动或内容改变（跟随键变化）且光标越出
        // 可视区域时才滚动到光标可见；键不变视为用户手动滚动，不回弹
        // （不要求聚焦，AI 流式回填把 caret 推到末尾时同样跟随）。
        if let Some(caret_line) = caret_visual_line {
            let handle = self
                .entity
                .read(cx)
                .scroll_handle(multiline_scroll_handle_id(self.field_id));
            let container_height = f32::from(handle.bounds().size.height);
            if container_height > 1.0 {
                let follow_key = (field.caret, value.len());
                let (scroll_top, new_key) = multiline_caret_follow_decision(
                    field.last_caret_follow,
                    follow_key,
                    caret_line,
                    container_height,
                    -f32::from(handle.offset().y),
                );
                if let Some(top) = scroll_top {
                    let offset = handle.offset();
                    handle.set_offset(point(offset.x, px(-top)));
                }
                if new_key != field.last_caret_follow {
                    self.entity.update(cx, |view, _| {
                        view.field_mut(self.field_id).last_caret_follow = new_key;
                    });
                }
            }
        }

        MultiLineInputPrepaint {
            lines,
            placeholder: None,
            cursor,
            selections,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.entity.read(cx).field(self.field_id).focus.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        if let Some(placeholder) = prepaint.placeholder.take() {
            let _ = placeholder.paint(bounds.origin, window.line_height(), window, cx);
        }
        for line in &prepaint.lines {
            let _ = line
                .line
                .paint(line.bounds.origin, window.line_height(), window, cx);
        }
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
        self.entity.update(cx, |view, _cx| {
            let field = view.field_mut(self.field_id);
            field.last_layout = None;
            field.last_bounds = Some(bounds);
            let prev_count = field.last_wrapped_line_count;
            let new_count = prepaint.lines.len().max(MULTILINE_MIN_LINES);
            field.last_wrapped_line_count = new_count;
            field.last_multiline_layout = prepaint.lines.clone();
            // 换行行数变化后请求重排，让布局高度匹配实际内容。
            if prev_count != new_count {
                window.refresh();
            }
        });
    }
}

fn logical_line_ranges(value: &str) -> Vec<Range<usize>> {
    if value.is_empty() {
        return vec![0..0];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, ch) in value.char_indices() {
        if ch == '\n' {
            ranges.push(start..index);
            start = index + ch.len_utf8();
        }
    }
    ranges.push(start..value.len());
    ranges
}

fn range_overlap(a: &Range<usize>, b: &Range<usize>) -> Option<Range<usize>> {
    let start = a.start.max(b.start);
    let end = a.end.min(b.end);
    (start < end).then_some(start..end)
}

fn multiline_text_runs(
    field: &TextEditState,
    style: &gpui::TextStyle,
    line_range: &Range<usize>,
    line_len: usize,
) -> Vec<TextRun> {
    let base_run = TextRun {
        len: line_len,
        font: style.font(),
        color: style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let Some(marked_range) = field.marked_range.as_ref() else {
        return vec![base_run];
    };
    let Some(overlap) = range_overlap(marked_range, line_range) else {
        return vec![base_run];
    };
    let marked_start = overlap.start.saturating_sub(line_range.start);
    let marked_end = overlap.end.saturating_sub(line_range.start);
    vec![
        TextRun {
            len: marked_start,
            ..base_run.clone()
        },
        TextRun {
            len: marked_end.saturating_sub(marked_start),
            underline: Some(gpui::UnderlineStyle {
                color: Some(base_run.color),
                thickness: px(1.0),
                wavy: false,
            }),
            ..base_run.clone()
        },
        TextRun {
            len: line_len.saturating_sub(marked_end),
            ..base_run
        },
    ]
    .into_iter()
    .filter(|run| run.len > 0)
    .collect()
}

#[cfg(test)]
#[path = "tests/text_input.rs"]
mod tests;
