use std::{cell::RefCell, ops::Range, path::Path, rc::Rc, sync::Arc};

use crate::ui::theme::rgb;
use gpui::{
    Context, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior, MouseButton,
    MouseDownEvent, PathBuilder, UniformListScrollHandle, Window, canvas, div, point, prelude::*,
    px, uniform_list,
};
use khaslana::{
    ConflictBlock, ConflictBlockResolution, ConflictBlockStatus, ConflictFileKind,
    ConflictFileView, ConflictResolutionSide, RepositorySnapshot,
};

use crate::{
    MainMode, RepositoryView,
    ui::{
        components::{app_panel, empty_state},
        icons::ToolbarIcon,
        theme as ui_theme,
    },
    ui_helpers::{ScrollbarMode, scrollable_uniform_frame},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictDocumentPane {
    Ours,
    Result,
    Theirs,
}

pub(crate) fn conflict_status_message(label: &str, count: usize) -> String {
    if label.starts_with("合并") {
        return format!("合并产生冲突，请在工作区使用 IDEA 或进入“冲突处理”解决（{count} 个文件）");
    }
    let operation = label.strip_suffix("完成").unwrap_or("操作");
    format!("{operation}产生冲突，请在左侧“冲突”区域解决（{count} 个文件）")
}

fn conflict_paths(snapshot: Option<&RepositorySnapshot>) -> Vec<String> {
    snapshot
        .map(|snapshot| snapshot.conflicts.clone())
        .unwrap_or_default()
}

fn conflict_document_line_owners(
    content: &str,
    pane: ConflictDocumentPane,
    view: &ConflictFileView,
    line_count: usize,
) -> Vec<Option<usize>> {
    let mut owners = vec![None; line_count.max(1)];
    for (index, block) in view.blocks.iter().enumerate() {
        let (start, end) = conflict_document_byte_range(block, pane);
        let range = conflict_byte_range_to_lines(content, start, end);
        for line_index in range {
            if let Some(owner) = owners.get_mut(line_index) {
                *owner = Some(index);
            }
        }
    }
    owners
}

fn conflict_document_byte_range(
    block: &ConflictBlock,
    pane: ConflictDocumentPane,
) -> (usize, usize) {
    match pane {
        ConflictDocumentPane::Ours => (block.ours_start, block.ours_end),
        ConflictDocumentPane::Result => (block.start, block.end),
        ConflictDocumentPane::Theirs => (block.theirs_start, block.theirs_end),
    }
}

fn conflict_byte_range_to_lines(content: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let start_line = content[..start.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let mut end_line = content[..end.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    if end_line == start_line {
        end_line += 1;
    }
    start_line..end_line
}

// ── 三栏连线（IDEA 式采用指示）──────────────────────────────

/// 冲突栏单行高度兜底值（行渲染 `.min_h(px(18.0))`）。
const CONFLICT_CONNECTOR_FALLBACK_ROW_HEIGHT: f32 = 18.0;

/// 单个冲突块在三栏中的行区间（构建时预计算，paint 闭包只做坐标换算）。
#[derive(Clone)]
struct ConflictConnectorAnchor {
    ours_lines: Range<usize>,
    result_lines: Range<usize>,
    theirs_lines: Range<usize>,
    selected: bool,
}

/// 连线 overlay 的 paint 数据：三栏滚动句柄、各栏总行数与块锚点。
struct ConflictConnectorData {
    ours_handle: UniformListScrollHandle,
    result_handle: UniformListScrollHandle,
    theirs_handle: UniformListScrollHandle,
    ours_line_count: usize,
    result_line_count: usize,
    theirs_line_count: usize,
    anchors: Vec<ConflictConnectorAnchor>,
    /// 三栏 offset 的上帧记录（同步滚动源判定用），跨帧持久于
    /// `RepositoryView.conflict_pane_scroll_sync`。
    scroll_state: Rc<RefCell<Option<[f32; 3]>>>,
}

/// uniform_list 的总行数：与 `conflict_document_line_ranges` 的区间数一致
///（空文本占 1 行、行尾换行产生尾空行），是行高换算的分母。
fn uniform_list_line_count(content: &str) -> usize {
    if content.is_empty() {
        1
    } else {
        content.bytes().filter(|byte| *byte == b'\n').count() + 1
    }
}

/// 块区域在某栏内的内容坐标 y 段（窗口坐标，含滚动 offset；
/// offset.y 向下滚动为负）。
fn conflict_block_y_range(
    viewport_top: f32,
    offset_y: f32,
    row_height: f32,
    lines: &Range<usize>,
) -> (f32, f32) {
    let top = viewport_top + offset_y + row_height * lines.start as f32;
    (top, top + row_height * (lines.end - lines.start) as f32)
}

/// 把块区域 y 段裁剪到视口可见范围并返回中点锚点 y；
/// 整段滚出视口（不可见）返回 `None`。
fn conflict_connector_anchor_y(
    viewport_top: f32,
    viewport_bottom: f32,
    block_top: f32,
    block_bottom: f32,
) -> Option<f32> {
    let visible_top = block_top.max(viewport_top);
    let visible_bottom = block_bottom.min(viewport_bottom);
    (visible_bottom > visible_top).then_some((visible_top + visible_bottom) / 2.0)
}

/// 某一栏的可视口几何（窗口坐标）、行高与滚动信息。
struct ConflictPaneViewport {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    offset_x: f32,
    offset_y: f32,
    /// 可滚动的最大竖直偏移（正值；offset.y 取值范围 [-max, 0]）。
    max_offset_y: f32,
    row_height: f32,
    handle: UniformListScrollHandle,
}

/// 从 uniform_list 滚动句柄读取视口 bounds、滚动 offset 与行高。
/// 首帧尚未布局（bounds 高度非正）返回 `None`，该帧跳过连线绘制。
fn conflict_pane_viewport(
    handle: &UniformListScrollHandle,
    line_count: usize,
) -> Option<ConflictPaneViewport> {
    let state = handle.0.borrow();
    let bounds = state.base_handle.bounds();
    let height = f32::from(bounds.size.height);
    if !(height > 1.0) {
        return None;
    }
    // ItemSize.contents = 行高 × 总行数（uniform_list prepaint 写入），
    // 换算回单行高度；异常值兜底到行渲染的 min_h。
    let row_height = state
        .last_item_size
        .map(|size| f32::from(size.contents.height) / line_count.max(1) as f32)
        .filter(|height| (4.0..=100.0).contains(height))
        .unwrap_or(CONFLICT_CONNECTOR_FALLBACK_ROW_HEIGHT);
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let offset = state.base_handle.offset();
    Some(ConflictPaneViewport {
        left,
        right: left + f32::from(bounds.size.width),
        top,
        bottom: top + height,
        offset_x: f32::from(offset.x),
        offset_y: f32::from(offset.y),
        max_offset_y: f32::from(state.base_handle.max_offset().height).max(0.0),
        row_height,
        handle: handle.clone(),
    })
}

/// 同步滚动的源栏判定：与上帧比较，恰好一栏 offset 变化超过阈值时
/// 返回该栏（用户滚轮/拖动滚动条只改一栏）；多栏同时变化（程序化
/// 三栏联动 scrollToItem）或全部未变返回 `None`，不做同步。
fn conflict_scroll_sync_source(current: [f32; 3], prev: [f32; 3]) -> Option<usize> {
    let mut source = None;
    for index in 0..3 {
        if (current[index] - prev[index]).abs() > 0.5 {
            if source.is_some() {
                return None;
            }
            source = Some(index);
        }
    }
    source
}

/// 一次性读取三栏视口；任一栏尚未布局返回 `None`。
fn conflict_pane_viewports(data: &ConflictConnectorData) -> Option<[ConflictPaneViewport; 3]> {
    Some([
        conflict_pane_viewport(&data.ours_handle, data.ours_line_count)?,
        conflict_pane_viewport(&data.result_handle, data.result_line_count)?,
        conflict_pane_viewport(&data.theirs_handle, data.theirs_line_count)?,
    ])
}

/// 三栏同步滚动：把源栏的竖直 offset 应用到其余两栏，各自钳制到自身
/// 可滚动范围（短栏不越界，避免下一帧被布局钳回引发来回弹跳）。
/// 横向偏移各自保留（行宽差异大，横向无对应关系）。
/// 返回是否实际应用了同步（调用方据此请求补一帧）。
fn sync_conflict_pane_scrolling(
    panes: &mut [ConflictPaneViewport; 3],
    prev: &mut Option<[f32; 3]>,
) -> bool {
    let current = [panes[0].offset_y, panes[1].offset_y, panes[2].offset_y];
    let mut applied = false;
    let next = match prev.as_ref() {
        Some(prev_offsets) => match conflict_scroll_sync_source(current, *prev_offsets) {
            Some(source) => {
                let target = current[source];
                let mut updated = current;
                for other in 0..3 {
                    if other == source {
                        continue;
                    }
                    let clamped = target.max(-panes[other].max_offset_y).min(0.0);
                    if (clamped - current[other]).abs() > 0.5 {
                        panes[other]
                            .handle
                            .0
                            .borrow()
                            .base_handle
                            .set_offset(point(px(panes[other].offset_x), px(clamped)));
                        panes[other].offset_y = clamped;
                        updated[other] = clamped;
                        applied = true;
                    }
                }
                updated
            }
            None => current,
        },
        None => current,
    };
    *prev = Some(next);
    applied
}

/// 画一条 S 形三次贝塞尔连线：两端水平出发/到达（IDEA 合并工具风格）。
fn paint_conflict_connector(
    window: &mut Window,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: u32,
    width: f32,
) {
    let mid_x = (x1 + x2) / 2.0;
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(point(px(x1), px(y1)));
    builder.cubic_bezier_to(
        point(px(x2), px(y2)),
        point(px(mid_x), px(y1)),
        point(px(mid_x), px(y2)),
    );
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}

/// 绘制全部连线（滚动同步在 canvas prepaint 中先行完成）：ours 右缘 →
/// 结果区左缘、theirs 左缘 → 结果区右缘。块区域在两侧栏或结果区整段
/// 不可见时跳过该条（落点看不见的连线只会增加噪音）；部分可见裁剪到
/// 可视段中点。三栏视口顶部不对齐（ours/theirs 有操作按钮行），
/// 连线斜向是预期语义。
fn paint_conflict_connectors(data: ConflictConnectorData, window: &mut Window) {
    let Some(panes) = conflict_pane_viewports(&data) else {
        return;
    };
    let (ours, result, theirs) = (&panes[0], &panes[1], &panes[2]);

    let anchor_y = |pane: &ConflictPaneViewport, lines: &Range<usize>| {
        let (top, bottom) = conflict_block_y_range(pane.top, pane.offset_y, pane.row_height, lines);
        conflict_connector_anchor_y(pane.top, pane.bottom, top, bottom)
    };

    for anchor in &data.anchors {
        // 非选中块用 MUTED_FOREGROUND 实色（BORDER 与背景融为一体），
        // 选中块主题色加粗。
        let (color, width) = if anchor.selected {
            (ui_theme::ACCENT, 2.5)
        } else {
            (ui_theme::MUTED_FOREGROUND, 1.5)
        };
        if let (Some(from_y), Some(to_y)) = (
            anchor_y(ours, &anchor.ours_lines),
            anchor_y(result, &anchor.result_lines),
        ) {
            paint_conflict_connector(window, ours.right, from_y, result.left, to_y, color, width);
        }
        if let (Some(from_y), Some(to_y)) = (
            anchor_y(theirs, &anchor.theirs_lines),
            anchor_y(result, &anchor.result_lines),
        ) {
            paint_conflict_connector(
                window,
                theirs.left,
                from_y,
                result.right,
                to_y,
                color,
                width,
            );
        }
    }
}

#[derive(Clone, Debug)]
struct ConflictDocumentLineModel {
    content: Arc<str>,
    ranges: Arc<[Range<usize>]>,
    owners: Arc<[Option<usize>]>,
}

#[derive(Clone, Debug)]
struct ConflictPlainLineModel {
    content: Arc<str>,
    ranges: Arc<[Range<usize>]>,
}

impl ConflictPlainLineModel {
    fn new(content: &str) -> Self {
        Self {
            content: Arc::from(content),
            ranges: Arc::from(conflict_document_line_ranges(content)),
        }
    }

    fn line_count(&self) -> usize {
        self.ranges.len().max(1)
    }

    fn line_text(&self, index: usize) -> &str {
        let Some(range) = self.ranges.get(index) else {
            return "";
        };
        &self.content[range.clone()]
    }
}

impl ConflictDocumentLineModel {
    fn new(content: &str, pane: ConflictDocumentPane, view: &ConflictFileView) -> Self {
        let ranges = conflict_document_line_ranges(content);
        let owners = conflict_document_line_owners(content, pane, view, ranges.len());
        Self {
            content: Arc::from(content),
            ranges: Arc::from(ranges),
            owners: Arc::from(owners),
        }
    }

    fn line_count(&self) -> usize {
        self.ranges.len().max(1)
    }

    fn line_text(&self, index: usize) -> &str {
        let Some(range) = self.ranges.get(index) else {
            return "";
        };
        &self.content[range.clone()]
    }

    fn owner_at(&self, index: usize) -> Option<usize> {
        self.owners.get(index).copied().flatten()
    }
}

fn conflict_document_line_ranges(content: &str) -> Vec<Range<usize>> {
    if content.is_empty() {
        return vec![0..0];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            ranges.push(start..index);
            start = index + ch.len_utf8();
        }
    }
    ranges.push(start..content.len());
    ranges
}

fn conflict_line_colors(
    pane: ConflictDocumentPane,
    block: &ConflictBlock,
    active: bool,
) -> (u32, u32) {
    match block.status {
        ConflictBlockStatus::Ignored => {
            if active {
                (ui_theme::ACCENT, ui_theme::MUTED_FOREGROUND)
            } else {
                (ui_theme::CARD, ui_theme::MUTED_FOREGROUND)
            }
        }
        ConflictBlockStatus::Resolved(_) => match pane {
            ConflictDocumentPane::Ours | ConflictDocumentPane::Theirs => {
                if active {
                    (ui_theme::COLOR_ERROR, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
            ConflictDocumentPane::Result => {
                if active {
                    (ui_theme::ACCENT, ui_theme::PRIMARY)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
        },
        // AI 合并块：选中时结果区用绿色高亮（区别于未处理的黄色），
        // 直观表达「这段已经合并完成」。
        ConflictBlockStatus::Merged => match pane {
            ConflictDocumentPane::Ours | ConflictDocumentPane::Theirs => {
                if active {
                    (ui_theme::COLOR_ERROR, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
            ConflictDocumentPane::Result => {
                if active {
                    (ui_theme::COLOR_SUCCESS, ui_theme::COLOR_SUCCESS_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
        },
        ConflictBlockStatus::Unresolved => match pane {
            ConflictDocumentPane::Ours | ConflictDocumentPane::Theirs => {
                if active {
                    (ui_theme::COLOR_ERROR, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::COLOR_WARNING, ui_theme::FOREGROUND)
                }
            }
            ConflictDocumentPane::Result => {
                if active {
                    (ui_theme::COLOR_WARNING, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
        },
    }
}

impl RepositoryView {
    pub(crate) fn render_conflict_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts = conflict_paths(self.snapshot.as_ref());
        let conflict_rows = conflicts
            .iter()
            .cloned()
            .map(|path| self.conflict_row(path, cx).into_any_element())
            .collect::<Vec<_>>();
        let has_conflicts = !conflict_rows.is_empty();
        let conflict_count = conflicts.len();

        div().when(has_conflicts, |this| {
            this.child(self.render_conflict_summary(conflicts.len()))
                .child(self.render_change_section(
                    "冲突",
                    "conflict-list",
                    "",
                    false,
                    conflict_rows,
                    true,
                    conflict_count,
                    false,
                    Vec::new(),
                    cx,
                ))
                .child(div().flex_none().h(px(1.0)).bg(rgb(ui_theme::BORDER)))
        })
    }

    pub(crate) fn render_conflict_workbench(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .when_some(self.render_merge_banner(cx), |this, banner| {
                this.child(banner)
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(self.render_conflict_file_rail(cx))
                    .child(self.render_column_splitter(crate::ResizeTarget::Changes, cx))
                    .child(self.render_conflict_detail(window, cx)),
            )
    }

    fn render_conflict_file_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts = conflict_paths(self.snapshot.as_ref());
        let selected_path = self.conflict_workbench.selected_path.as_deref();
        let file_rows = conflicts
            .iter()
            .cloned()
            .map(|path| {
                self.conflict_file_row(path.clone(), selected_path == Some(path.as_str()), cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        app_panel()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.changes_width))
            .min_w(px(self.changes_width))
            .h_full()
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::COLOR_WARNING))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                    .child(format!("存在 {} 个冲突文件", conflicts.len())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .children(file_rows),
            )
    }

    fn conflict_file_row(
        &self,
        path: String,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = self.conflict_workbench.files.get(&path);
        let badge = match view.map(|view| view.kind) {
            Some(ConflictFileKind::Text) => "文本",
            Some(ConflictFileKind::Binary) => "二进制",
            Some(ConflictFileKind::Unsupported) => "回退",
            None => "加载中",
        };
        let unresolved = view
            .map(ConflictFileView::unresolved_block_count)
            .unwrap_or_default();
        let dirty = view
            .map(|view| view.draft_status)
            .is_some_and(|status| matches!(status, khaslana::ConflictDraftStatus::Dirty));
        let applied = view
            .map(|view| view.draft_status)
            .is_some_and(|status| matches!(status, khaslana::ConflictDraftStatus::Applied));
        let path_for_select = path.clone();

        crate::ui::components::list_row_surface(format!("conflict-workbench-{path}"), selected)
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                    window.focus(&this.conflict_editor.focus);
                    this.main_mode = MainMode::Conflict;
                    this.select_conflict_file(path_for_select.clone());
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(path),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_1()
                            .py(px(2.0))
                            .rounded_sm()
                            .bg(rgb(ui_theme::ACCENT))
                            .text_size(px(10.0))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(badge),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("未处理 {unresolved}"))
                    .when(dirty, |this| this.child("草稿已修改"))
                    .when(applied, |this| this.child("已应用")),
            )
    }

    fn render_conflict_detail(&self, _window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_path = self.conflict_workbench.selected_path.clone();
        let selected_view = selected_path
            .as_ref()
            .and_then(|path| self.conflict_workbench.files.get(path));
        let title = selected_path
            .clone()
            .unwrap_or_else(|| "请选择一个冲突文件".to_string());

        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(self.render_conflict_header(title, selected_view, cx))
            .child(match selected_view {
                Some(view) if view.kind == ConflictFileKind::Text => self
                    .render_text_conflict_detail(view, cx)
                    .into_any_element(),
                Some(view) => self
                    .render_fallback_conflict_detail(view, cx)
                    .into_any_element(),
                None => empty_state(
                    Some(ToolbarIcon::Search),
                    "请选择一个冲突文件",
                    None::<&'static str>,
                )
                .into_any_element(),
            })
    }

    fn render_conflict_header(
        &self,
        title: String,
        view: Option<&ConflictFileView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let block_count = view.map(|view| view.blocks.len()).unwrap_or_default();
        let selected_block = self
            .conflict_workbench
            .selected_block
            .min(block_count.saturating_sub(1));
        let progress = if block_count == 0 {
            "无文本冲突块".to_string()
        } else {
            format!(
                "块 {}/{}，未处理 {}",
                selected_block + 1,
                block_count,
                view.map(ConflictFileView::unresolved_block_count)
                    .unwrap_or_default()
            )
        };
        let unresolved = view
            .map(ConflictFileView::unresolved_block_count)
            .unwrap_or_default();
        let ignored = view
            .map(ConflictFileView::ignored_block_count)
            .unwrap_or_default();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(progress),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_wrap()
                    .child(self.button(
                        "上一个块",
                        block_count > 0,
                        |this, _, _| this.step_conflict_block(-1),
                        cx,
                    ))
                    .child(self.button(
                        "下一个块",
                        block_count > 0,
                        |this, _, _| this.step_conflict_block(1),
                        cx,
                    ))
                    .when(unresolved > 0, |this| {
                        this.child(self.conflict_count_badge(
                            format!("未处理 {unresolved}"),
                            ui_theme::COLOR_WARNING,
                            ui_theme::COLOR_WARNING_FOREGROUND,
                        ))
                    })
                    .when(ignored > 0, |this| {
                        this.child(self.conflict_count_badge(
                            format!("已忽略 {ignored}"),
                            ui_theme::ACCENT,
                            ui_theme::PRIMARY,
                        ))
                    })
                    .child(self.button(
                        "忽略该块",
                        block_count > 0 && !self.busy,
                        |this, _, _| this.ignore_selected_conflict_block(),
                        cx,
                    ))
                    .child(self.button(
                        if self.conflict_workbench.show_base {
                            "隐藏 Base"
                        } else {
                            "显示 Base"
                        },
                        block_count > 0,
                        |this, _, _| {
                            this.conflict_workbench.show_base = !this.conflict_workbench.show_base
                        },
                        cx,
                    ))
                    .child(self.button(
                        if self.ai_conflict_loading {
                            "AI 生成中..."
                        } else if !self.ai_settings.is_usable() {
                            // 按钮不支持 tooltip（app_button 返回不透明元素），
                            // 未配置时直接在文案中标注原因。
                            "AI 合并建议（未配置）"
                        } else {
                            "AI 合并建议"
                        },
                        view.is_some_and(|view| view.kind == ConflictFileKind::Text)
                            && self.ai_conflict_merge_button_enabled(),
                        |this, _, _| this.generate_ai_conflict_merge(),
                        cx,
                    ))
                    .child(self.button(
                        "应用到工作区",
                        view.is_some_and(|view| view.kind == ConflictFileKind::Text) && !self.busy,
                        |this, _, _| this.apply_selected_conflict_draft(false),
                        cx,
                    ))
                    .child(self.button(
                        if self.external_merge_settings.enabled {
                            "用 IntelliJ IDEA 解决"
                        } else {
                            "配置 IDEA 并解决"
                        },
                        view.is_some() && !self.busy,
                        |this, _, _| this.resolve_selected_conflict_with_intellij_idea(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "应用并标记已解决",
                        view.is_some_and(|view| view.kind == ConflictFileKind::Text) && !self.busy,
                        |this, _, _| this.apply_selected_conflict_draft(true),
                        cx,
                    )),
            )
    }

    fn render_text_conflict_detail(
        &self,
        view: &ConflictFileView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected_block = self
            .conflict_workbench
            .selected_block
            .min(view.blocks.len().saturating_sub(1));
        let block = view.blocks.get(selected_block);
        let warning =
            (view.has_manual_blocks() || view.requires_resolution_confirmation()).then(|| {
                format!(
                    "仍有 {} 个代码块未处理；直接解决时会先弹出确认。",
                    view.unresolved_block_count()
                )
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .when_some(warning, |this, warning| {
                this.child(
                    div()
                        .mx_3()
                        .mt_3()
                        .px_3()
                        .py_2()
                        .rounded_sm()
                        .bg(rgb(ui_theme::COLOR_WARNING))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .child(warning),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .gap_2()
                    .p_3()
                    .child(self.render_conflict_document_pane(
                        "当前版本",
                        "conflict-ours-scroll",
                        crate::CONFLICT_OURS_SCROLL_HANDLE_ID,
                        &view.ours_text,
                        ConflictDocumentPane::Ours,
                        view,
                        selected_block,
                        vec![
                            self.button(
                                "接受当前",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::Ours,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                            self.button(
                                "接受两边（当前在前）",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::BothOursFirst,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                        ],
                        cx,
                    ))
                    .child(self.render_conflict_document_pane(
                        "结果区",
                        "conflict-result-scroll",
                        crate::CONFLICT_RESULT_SCROLL_HANDLE_ID,
                        &view.draft,
                        ConflictDocumentPane::Result,
                        view,
                        selected_block,
                        Vec::new(),
                        cx,
                    ))
                    .child(self.render_conflict_document_pane(
                        "传入版本",
                        "conflict-theirs-scroll",
                        crate::CONFLICT_THEIRS_SCROLL_HANDLE_ID,
                        &view.theirs_text,
                        ConflictDocumentPane::Theirs,
                        view,
                        selected_block,
                        vec![
                            self.button(
                                "接受传入",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::Theirs,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                            self.button(
                                "接受两边（传入在前）",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::BothTheirsFirst,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                        ],
                        cx,
                    ))
                    // 连线 overlay 作为最后一个子元素绘制在三栏之上
                    //（GPUI 子元素按声明顺序绘制）。
                    .child(self.render_conflict_connectors(view, selected_block)),
            )
            .when(
                self.conflict_workbench.show_base
                    && block.is_some_and(|block| block.base.as_ref().is_some()),
                |this| {
                    this.child(
                        app_panel()
                            .flex_none()
                            .mx_3()
                            .mb_3()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(rgb(ui_theme::BORDER))
                                    .bg(rgb(ui_theme::CARD))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(rgb(ui_theme::FOREGROUND))
                                            .child("Base"),
                                    )
                                    .when_some(block, |this, block| {
                                        this.child(self.conflict_block_status_badge(
                                            block.status,
                                            block.has_manual_edits,
                                        ))
                                    }),
                            )
                            .child(self.render_conflict_plain_text(
                                "conflict-base-scroll",
                                block.and_then(|block| block.base.as_deref()).unwrap_or(""),
                                cx,
                            )),
                    )
                },
            )
    }

    fn render_fallback_conflict_detail(
        &self,
        view: &ConflictFileView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return div().into_any_element();
        };
        let path_for_ours = path.clone();
        let path_for_theirs = path.clone();
        let path_for_mark = path.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .gap_3()
            .p_4()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .bg(rgb(ui_theme::COLOR_WARNING))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                    .child(
                        view.fallback_reason
                            .clone()
                            .unwrap_or_else(|| "该冲突暂不支持可视化文本编辑".into()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.button(
                        "当前版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_ours.clone(),
                                ConflictResolutionSide::Ours,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "传入版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_theirs.clone(),
                                ConflictResolutionSide::Theirs,
                            )
                        },
                        cx,
                    ))
                    .child(self.primary_button(
                        "标记解决",
                        !self.busy,
                        move |this, _, _| this.mark_conflict_resolved(path_for_mark.clone()),
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// 三栏连线 overlay：从「当前版本/传入版本」两侧的冲突块区域画
    /// S 形曲线指向结果区对应块区域，指示采用后内容落点（IDEA 风格）。
    /// 纯绘制 canvas，不注册鼠标事件、不拦截交互；作为三栏行容器的
    /// 最后一个子元素绘制在最上层，每帧按各栏滚动 offset 重绘。
    fn render_conflict_connectors(
        &self,
        view: &ConflictFileView,
        selected_block: usize,
    ) -> impl IntoElement {
        let anchors = view
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| ConflictConnectorAnchor {
                ours_lines: conflict_byte_range_to_lines(
                    &view.ours_text,
                    block.ours_start,
                    block.ours_end,
                ),
                result_lines: conflict_byte_range_to_lines(&view.draft, block.start, block.end),
                theirs_lines: conflict_byte_range_to_lines(
                    &view.theirs_text,
                    block.theirs_start,
                    block.theirs_end,
                ),
                selected: index == selected_block,
            })
            .collect();
        let data = ConflictConnectorData {
            ours_handle: self.uniform_scroll_handle(crate::CONFLICT_OURS_SCROLL_HANDLE_ID),
            result_handle: self.uniform_scroll_handle(crate::CONFLICT_RESULT_SCROLL_HANDLE_ID),
            theirs_handle: self.uniform_scroll_handle(crate::CONFLICT_THEIRS_SCROLL_HANDLE_ID),
            ours_line_count: uniform_list_line_count(&view.ours_text),
            result_line_count: uniform_list_line_count(&view.draft),
            theirs_line_count: uniform_list_line_count(&view.theirs_text),
            anchors,
            scroll_state: self.conflict_pane_scroll_sync.clone(),
        };
        canvas(
            move |_, _, cx| {
                // 同步滚动放在 prepaint：paint 期 set_offset 只写值且
                // window.refresh() 在绘制期是 no-op，本帧 paint 读不到新
                // 值；prepaint 期写入同帧生效。实际应用了同步时经
                // refresh_windows 请求补一帧，滚动停止后其余两栏不差
                // 最后一拍（下一帧无变化即收敛，不会循环刷新）。
                if let Some(mut panes) = conflict_pane_viewports(&data) {
                    let mut state = data.scroll_state.borrow_mut();
                    if sync_conflict_pane_scrolling(&mut panes, &mut state) {
                        drop(state);
                        cx.refresh_windows();
                    }
                }
                data
            },
            move |_, data, window, _| paint_conflict_connectors(data, window),
        )
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
    }

    fn render_conflict_document_pane(
        &self,
        title: &'static str,
        scroll_id: &'static str,
        handle_id: &'static str,
        content: &str,
        pane: ConflictDocumentPane,
        view: &ConflictFileView,
        selected_block: usize,
        actions: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_block = view.blocks.get(selected_block);
        let has_actions = !actions.is_empty();
        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(title),
                    )
                    .when_some(active_block, |this, block| {
                        this.child(
                            self.conflict_block_status_badge(block.status, block.has_manual_edits),
                        )
                    }),
            )
            .when(has_actions, move |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .children(actions),
                )
            })
            .child(self.render_conflict_document_text(
                scroll_id,
                handle_id,
                content,
                pane,
                view,
                selected_block,
                cx,
            ))
    }

    /// 取某冲突文件某分栏的语法高亮（带主题变体守卫；未计算/已失效返回 None，
    /// 回退整行块状态前景色）。
    fn conflict_syntax_spans(
        &self,
        pane: ConflictDocumentPane,
        view: &ConflictFileView,
    ) -> Option<Arc<khaslana::syntax::SyntaxSpans>> {
        let entry = self.conflict_workbench.syntax.get(&view.path)?;
        let spans = match pane {
            ConflictDocumentPane::Ours => entry.ours.as_ref(),
            ConflictDocumentPane::Result => entry.draft.as_ref(),
            ConflictDocumentPane::Theirs => entry.theirs.as_ref(),
        };
        spans
            .filter(|spans| spans.dark == ui_theme::active_variant().is_dark())
            .cloned()
    }

    fn render_conflict_document_text(
        &self,
        scroll_id: &'static str,
        handle_id: &'static str,
        content: &str,
        pane: ConflictDocumentPane,
        view: &ConflictFileView,
        selected_block: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = self.uniform_scroll_handle(handle_id);
        let list_handle = handle.clone();
        let model = Arc::new(ConflictDocumentLineModel::new(content, pane, view));
        let row_count = model.line_count();
        let model_for_list = model.clone();
        let blocks = Arc::<[ConflictBlock]>::from(view.blocks.clone());
        // 该分栏的语法高亮（带变体守卫；结果区随草稿重算）
        let syntax = self.conflict_syntax_spans(pane, view);
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |_this, range: Range<usize>, _window, _cx| {
                        range
                            .map(|line_index| {
                                let owner = model_for_list.owner_at(line_index);
                                let block = owner.and_then(|index| blocks.get(index));
                                let active = owner == Some(selected_block);
                                let (bg, fg) = block
                                    .map(|block| conflict_line_colors(pane, block, active))
                                    .unwrap_or((ui_theme::CARD, ui_theme::FOREGROUND));
                                let line = model_for_list.line_text(line_index);
                                // 块状态背景保留，语法色做前景
                                let spans = syntax
                                    .as_ref()
                                    .and_then(|spans| {
                                        spans.lines.get(line_index).map(Vec::as_slice)
                                    })
                                    .filter(|spans| !spans.is_empty());
                                div()
                                    .min_h(px(18.0))
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(bg))
                                    .text_color(rgb(fg))
                                    .child(if line.is_empty() {
                                        div().child(" ").into_any_element()
                                    } else {
                                        crate::ui_helpers::syntax_styled_text(line, spans)
                                    })
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();
        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Vertical,
            content,
            handle,
            true,
            cx,
        )
    }

    fn render_conflict_plain_text(
        &self,
        scroll_id: &'static str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = handle.clone();
        let model = Arc::new(ConflictPlainLineModel::new(content));
        let row_count = model.line_count();
        let model_for_list = model.clone();
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |_this, range: Range<usize>, _window, _cx| {
                        range
                            .map(|line_index| {
                                let line = model_for_list.line_text(line_index);
                                div()
                                    .min_h(px(18.0))
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(if line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        line.to_string()
                                    })
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();
        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Vertical,
            content,
            handle,
            true,
            cx,
        )
    }

    fn conflict_count_badge(&self, label: String, bg: u32, fg: u32) -> impl IntoElement {
        div()
            .flex_none()
            .px_2()
            .py(px(2.0))
            .rounded_sm()
            .bg(rgb(bg))
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(fg))
            .child(label)
    }

    fn conflict_block_status_badge(
        &self,
        status: ConflictBlockStatus,
        has_manual_edits: bool,
    ) -> impl IntoElement {
        let (label, bg, fg) = match status {
            ConflictBlockStatus::Ignored => {
                ("已忽略", ui_theme::ACCENT, ui_theme::MUTED_FOREGROUND)
            }
            ConflictBlockStatus::Resolved(_) => ("已处理", ui_theme::ACCENT, ui_theme::PRIMARY),
            ConflictBlockStatus::Merged => (
                "已合并",
                ui_theme::COLOR_SUCCESS,
                ui_theme::COLOR_SUCCESS_FOREGROUND,
            ),
            ConflictBlockStatus::Unresolved if has_manual_edits => (
                "手工修改",
                ui_theme::COLOR_WARNING,
                ui_theme::COLOR_WARNING_FOREGROUND,
            ),
            ConflictBlockStatus::Unresolved => (
                "未处理",
                ui_theme::COLOR_WARNING,
                ui_theme::COLOR_WARNING_FOREGROUND,
            ),
        };
        div()
            .flex_none()
            .px_2()
            .py(px(2.0))
            .rounded_sm()
            .bg(rgb(bg))
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(fg))
            .child(label)
    }

    fn resolve_conflict_with_side(&mut self, path: String, side: ConflictResolutionSide) {
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");
        let label = match side {
            ConflictResolutionSide::Ours => "已使用当前版本解决冲突",
            ConflictResolutionSide::Theirs => "已使用传入版本解决冲突",
        };
        self.with_repo(label, move |service, repo| {
            service.resolve_conflict_with_side(repo, Path::new(&path), side)
        });
    }

    fn mark_conflict_resolved(&mut self, path: String) {
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");
        self.with_repo("冲突已标记为解决", move |service, repo| {
            service.mark_conflict_resolved(repo, Path::new(&path))
        });
    }

    pub(crate) fn resolve_selected_conflict_with_intellij_idea(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        self.resolve_conflict_with_intellij_idea_path(path);
    }

    fn resolve_conflict_with_intellij_idea_path(&mut self, path: String) -> bool {
        self.request_external_merge_for_path(path)
    }

    pub(crate) fn maybe_auto_open_external_merge_for_selected_conflict(&mut self) {
        if self.busy
            || !self.external_merge_settings.enabled
            || !self.external_merge_settings.auto_open_intellij
        {
            return;
        }
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        if !self.conflict_workbench.files.contains_key(&path)
            || self
                .conflict_workbench
                .external_merge_auto_opened
                .contains(&path)
        {
            return;
        }
        if self.resolve_conflict_with_intellij_idea_path(path.clone()) {
            self.conflict_workbench
                .mark_external_merge_auto_opened(path);
        }
    }

    fn render_conflict_summary(&self, count: usize) -> impl IntoElement {
        div()
            .flex_none()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::COLOR_WARNING))
            .text_size(px(12.0))
            .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
            .child(format!("存在 {count} 个冲突文件"))
    }

    fn conflict_row(&self, path: String, cx: &mut Context<Self>) -> impl IntoElement {
        let path_for_switch = path.clone();
        let path_for_ours = path.clone();
        let path_for_theirs = path.clone();
        let path_for_mark = path.clone();

        div()
            .id(format!("conflict-{path}"))
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::COLOR_WARNING))
            .bg(rgb(ui_theme::COLOR_WARNING))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.0))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::COLOR_WARNING)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            window.focus(&this.conflict_editor.focus);
                            this.main_mode = MainMode::Conflict;
                            this.select_conflict_file(path_for_switch.clone());
                            this.change_context_menu = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(24.0))
                            .text_size(px(11.0))
                            .font_family("monospace")
                            .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                            .child("!"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.button(
                        "当前版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_ours.clone(),
                                ConflictResolutionSide::Ours,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "传入版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_theirs.clone(),
                                ConflictResolutionSide::Theirs,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "标记解决",
                        !self.busy,
                        move |this, _, _| this.mark_conflict_resolved(path_for_mark.clone()),
                        cx,
                    )),
            )
    }
}

#[cfg(test)]
#[path = "../tests/conflicts.rs"]
mod tests;
