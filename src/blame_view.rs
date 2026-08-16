// 文件追溯（blame）视图 UI 模块。
//
// 实现 RepositoryView 的 render_blame_view：IDE 风格的分组注释栏
//（短 oid + 作者 + 日期 + 摘要）+ 行号列 + 内容列，uniform_list 虚拟渲染，
// 支持双向滚动条与编码切换；未提交行显示灰色「未提交」徽标。

use crate::ui::theme::rgb;
use gpui::{
    Context, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior, MouseButton,
    MouseDownEvent, div, prelude::*, px, uniform_list,
};
use khaslana::BlameView;

use crate::{
    EncodingMenuTarget, RepositoryView,
    ui::{components::tooltip_text, theme as ui_theme},
    ui_helpers::{ScrollbarMode, placeholder_row, scrollable_uniform_frame},
};

/// 追溯视图每行高度（px），与 browse_content_line 的 18px 一致。
pub(crate) const BLAME_ROW_HEIGHT: f32 = 18.0;
/// 注释栏宽度：哈希 + 作者 + 日期 + 摘要四个固定/弹性分列。
const BLAME_GUTTER_WIDTH: f32 = 280.0;
/// 注释栏内哈希列宽（8 位短 oid，等宽字体）。
const BLAME_GUTTER_HASH_WIDTH: f32 = 56.0;
/// 注释栏内作者列宽（truncate）。
const BLAME_GUTTER_AUTHOR_WIDTH: f32 = 72.0;
/// 注释栏内日期列宽（月-日）。
const BLAME_GUTTER_DATE_WIDTH: f32 = 36.0;
/// 行号列宽度（右对齐 + 两侧内边距，容纳 5 位行号）。
const BLAME_LINENO_WIDTH: f32 = 48.0;

/// 按内容身份缓存的最宽行扫描（与 BrowseState::widest_line_cache 同一套模式）：
/// 大文件打开期间每帧重算是 O(总字符)，虚拟列表的 with_width_from_item
/// 只需要一个测量基准行，缓存后免重扫。
fn cached_widest_blame_line_index(
    view: &Option<std::sync::Arc<BlameView>>,
    cache: &std::cell::RefCell<Option<((usize, usize), Option<usize>)>>,
) -> Option<usize> {
    let key = view
        .as_ref()
        .map(|view| (std::sync::Arc::as_ptr(view) as usize, view.lines.len()));
    let mut cache = cache.borrow_mut();
    if cache.as_ref().map(|(cached, _)| *cached) != key {
        let value = view
            .as_ref()
            .map(|view| crate::browse_view::widest_browse_line_index(&view.lines))
            .flatten();
        *cache = key.map(|key| (key, value));
    }
    cache.as_ref().and_then(|(_, value)| *value)
}

impl RepositoryView {
    pub(crate) fn render_blame_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
            .child(self.render_blame_header(cx))
            .child(self.render_blame_body(cx))
            // 编码选择下拉菜单（复用 diff 编码选择）
            .child(self.render_encoding_dropdown(EncodingMenuTarget::Blame, cx))
    }

    /// 顶部信息栏：「文件追溯」标题 + 路径 + 关闭按钮 + 编码按钮。
    fn render_blame_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self
            .blame
            .path
            .clone()
            .unwrap_or_else(|| "未选择文件".to_string());
        let tooltip_path = path.clone();
        let encoding_label = self
            .blame
            .view
            .as_ref()
            .map(|view| view.encoding.label())
            .unwrap_or_else(|| self.current_diff_encoding_choice().label());

        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child("文件追溯"),
                    )
                    .child(
                        div()
                            .id("blame-header-path")
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .truncate()
                            .tooltip(move |_window, cx| tooltip_text(tooltip_path.clone(), cx))
                            .child(path),
                    )
                    .when(self.blame.loading, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("加载中..."),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("blame-encoding")
                            .relative()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .bg(rgb(ui_theme::CARD))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .text_size(px(11.0))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.toggle_encoding_menu(EncodingMenuTarget::Blame);
                                    cx.notify();
                                }),
                            )
                            .child(format!("编码：{encoding_label}")),
                    )
                    .child(self.button("关闭", !self.busy, |this, _, _| this.close_blame(), cx)),
            )
    }

    /// 主体：虚拟列表渲染注释栏 + 行号 + 内容，双向滚动。
    fn render_blame_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.blame.view.clone();
        let row_count = view
            .as_ref()
            .map(|view| view.lines.len().max(1))
            .unwrap_or(1);
        let content_present = view.is_some();
        let handle = self.uniform_scroll_handle("blame-scroll");
        let list_handle = handle.clone();

        let inner_content = div()
            .id("blame-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    "blame-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, _cx| {
                        let view = this.blame.view.clone();
                        range
                            .map(|index| {
                                let Some(view) = view.as_ref() else {
                                    return placeholder_row(if this.blame.loading {
                                        "正在加载文件追溯..."
                                    } else {
                                        "请选择一个文件查看追溯"
                                    })
                                    .into_any_element();
                                };
                                let line = view.lines.get(index).cloned().unwrap_or_default();
                                this.blame_line(view, index, line).into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_width_from_item(cached_widest_blame_line_index(
                    &view,
                    &self.blame.widest_line_cache,
                ))
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        scrollable_uniform_frame(
            "blame-scroll",
            ScrollbarMode::Both,
            inner_content,
            handle,
            content_present,
            cx,
        )
    }

    /// 渲染追溯视图的一行。
    ///
    /// 三列布局：注释栏 | 行号 | 内容。不用分割线（避免整页呈表格感），
    /// 注释栏以微灰底色形成「注释侧栏 | 代码区」的 IDE 分区；分组信息由
    /// 「仅块首行有注释」天然传达。未提交行以警告色打底整行区分，
    /// 内容文字用警告前景色且不做语法高亮（与已提交行的彩色代码区分）。
    fn blame_line(&self, view: &BlameView, index: usize, text: String) -> impl IntoElement {
        let hunk = view
            .line_hunk
            .get(index)
            .and_then(|hunk_index| view.hunks.get(*hunk_index));
        let is_hunk_first = hunk.is_some_and(|hunk| hunk.start_line == index + 1);
        let is_uncommitted = hunk.is_some_and(|hunk| hunk.commit.is_none());
        let syntax_spans = if is_uncommitted {
            None
        } else {
            crate::ui_helpers::syntax_spans_for_line(&self.blame.syntax, index)
        };

        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .h(px(BLAME_ROW_HEIGHT))
            // 未提交行整行以警告色打底，与已提交行明显区分
            .when(is_uncommitted, |this| this.bg(rgb(ui_theme::COLOR_WARNING)))
            // 列 1：注释栏（微灰底侧栏）
            .child(self.blame_gutter(hunk, is_hunk_first, is_uncommitted))
            // 列 2：行号（右对齐 + 内边距，与内容拉开距离）
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .w(px(BLAME_LINENO_WIDTH))
                    .h(px(BLAME_ROW_HEIGHT))
                    .pr(px(8.0))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child((index + 1).to_string()),
            )
            // 列 3：内容（左内边距与行号隔开；已提交行带语法高亮）
            .child(
                div()
                    .flex_none()
                    .h(px(BLAME_ROW_HEIGHT))
                    .line_height(px(BLAME_ROW_HEIGHT))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pl(px(8.0))
                    .text_color(rgb(if is_uncommitted {
                        ui_theme::COLOR_WARNING_FOREGROUND
                    } else {
                        ui_theme::FOREGROUND
                    }))
                    .child(crate::ui_helpers::syntax_styled_text(&text, syntax_spans)),
            )
    }

    /// 注释栏：微灰底侧栏，块首行按「哈希 | 作者 | 日期 | 摘要」固定宽度
    /// 分列对齐（跨行纵向整齐），连续同块行留空；未提交块首行显示
    /// 「未提交」徽标。哈希用主色等宽字体点缀，其余弱化文字。
    fn blame_gutter(
        &self,
        hunk: Option<&khaslana::BlameHunkInfo>,
        is_hunk_first: bool,
        is_uncommitted: bool,
    ) -> impl IntoElement {
        let commit = if is_hunk_first && !is_uncommitted {
            hunk.and_then(|hunk| hunk.commit.as_ref())
        } else {
            None
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .w(px(BLAME_GUTTER_WIDTH))
            .h(px(BLAME_ROW_HEIGHT))
            .pr(px(8.0))
            .overflow_hidden()
            // 已提交行的注释栏铺微灰底，与代码区分区；未提交行保持整行
            // 警告底（此处不再铺灰底，避免盖掉行背景）。
            .when(!is_uncommitted, |this| {
                this.bg(rgb(ui_theme::DIFF_HEADER_BG))
            })
            .when_some(commit, |this, commit| {
                this.child(
                    div()
                        .flex_none()
                        .w(px(BLAME_GUTTER_HASH_WIDTH))
                        .font_family("Consolas, monospace")
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::PRIMARY))
                        .truncate()
                        .child(commit.short_oid.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(BLAME_GUTTER_AUTHOR_WIDTH))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .truncate()
                        .child(commit.author.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(BLAME_GUTTER_DATE_WIDTH))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(blame_date_label(commit.time)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .truncate()
                        .child(commit.summary.clone()),
                )
            })
            .when(is_hunk_first && is_uncommitted, |this| {
                this.child(
                    div()
                        .flex_none()
                        .ml(px(4.0))
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .text_size(px(10.0))
                        .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .child("未提交"),
                )
            })
    }
}

/// 追溯注释栏的紧凑日期（月-日，本地时区）。
fn blame_date_label(seconds: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "--".to_string())
}
