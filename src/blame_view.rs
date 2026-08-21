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
    ui::{
        components::{command_group, page_header, tooltip_text},
        theme as ui_theme,
    },
    ui_helpers::{ScrollbarMode, placeholder_row, scrollable_uniform_frame},
};

/// 追溯视图每行高度（px），与 browse_content_line 的 18px 一致。
pub(crate) const BLAME_ROW_HEIGHT: f32 = 18.0;
/// 注释栏宽度：哈希 + 作者 + 日期 + 摘要四个固定/弹性分列。
const BLAME_GUTTER_WIDTH: f32 = 300.0;
/// 注释栏内哈希列宽（8 位短 oid，等宽字体）。
const BLAME_GUTTER_HASH_WIDTH: f32 = 56.0;
/// 注释栏内作者列宽（truncate）。
const BLAME_GUTTER_AUTHOR_WIDTH: f32 = 72.0;
/// 注释栏内日期列宽（含年份的 yyyy-mm-dd）。
const BLAME_GUTTER_DATE_WIDTH: f32 = 64.0;
/// 行号列宽度（右对齐 + 两侧内边距，容纳 5 位行号）。
const BLAME_LINENO_WIDTH: f32 = 48.0;

/// 追溯行的视觉层级由状态决定，而不是由完整边框或卡片堆叠表达。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlameLineVisualRule {
    row_background: u32,
    gutter_background: u32,
    content_foreground: u32,
    shows_syntax: bool,
}

const fn blame_line_visual_rule(is_uncommitted: bool) -> BlameLineVisualRule {
    if is_uncommitted {
        BlameLineVisualRule {
            row_background: ui_theme::FEEDBACK_WARNING_BG,
            gutter_background: ui_theme::FEEDBACK_WARNING_BG,
            content_foreground: ui_theme::FEEDBACK_WARNING_TEXT,
            shows_syntax: false,
        }
    } else {
        BlameLineVisualRule {
            row_background: ui_theme::SURFACE_BASE,
            gutter_background: ui_theme::SURFACE_SUNKEN,
            content_foreground: ui_theme::CONTENT_PRIMARY,
            shows_syntax: true,
        }
    }
}

/// 三列追溯布局的固定尺度，避免后续视觉调整意外改变既有信息密度。
const fn blame_columns_layout() -> (f32, f32, f32) {
    (BLAME_GUTTER_WIDTH, BLAME_LINENO_WIDTH, BLAME_ROW_HEIGHT)
}

#[cfg(test)]
#[path = "tests/blame_view.rs"]
mod tests;

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
            .bg(rgb(ui_theme::SURFACE_CANVAS))
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

        page_header("文件追溯", Some("提交归属与工作区变更")).child(
            command_group()
                .child(
                    div()
                        .id("blame-header-path")
                        .max_w(px(360.0))
                        .min_w(px(0.0))
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .truncate()
                        .tooltip(move |_window, cx| tooltip_text(tooltip_path.clone(), cx))
                        .child(path),
                )
                .when(self.blame.loading, |this| {
                    this.child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child("加载中..."),
                    )
                })
                .child(
                    div()
                        .id("blame-encoding")
                        .relative()
                        .flex_none()
                        .min_h(px(ui_theme::CONTROL_HEIGHT_COMPACT))
                        .px(px(ui_theme::SPACE_2))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .border_1()
                        .border_color(rgb(ui_theme::BORDER_MUTED))
                        .bg(rgb(ui_theme::SURFACE_RAISED))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .text_size(px(ui_theme::TYPE_META))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
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
                .child(
                    div()
                        .id("blame-close")
                        .flex_none()
                        .min_h(px(ui_theme::CONTROL_HEIGHT_COMPACT))
                        .px(px(ui_theme::SPACE_2))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .border_1()
                        .border_color(rgb(ui_theme::BORDER_MUTED))
                        .bg(rgb(ui_theme::SURFACE_RAISED))
                        .text_size(px(ui_theme::TYPE_BODY))
                        .text_color(rgb(if self.busy {
                            ui_theme::CONTENT_TERTIARY
                        } else {
                            ui_theme::CONTENT_PRIMARY
                        }))
                        .when(!self.busy, |this| {
                            this.cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                        })
                        .when(self.busy, |this| this.cursor_not_allowed().opacity(0.6))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            if !this.busy {
                                this.close_blame();
                                cx.notify();
                            }
                        }))
                        .child("关闭"),
                ),
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
            .bg(rgb(ui_theme::SURFACE_BASE))
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
        let visual = blame_line_visual_rule(is_uncommitted);
        let syntax_spans = if visual.shows_syntax {
            crate::ui_helpers::syntax_spans_for_line(&self.blame.syntax, index)
        } else {
            None
        };

        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .h(px(blame_columns_layout().2))
            // 未提交行整行以警告色打底，与已提交行明显区分；提交行使用基础 surface。
            .bg(rgb(visual.row_background))
            // 列 1：注释栏（微灰底侧栏）
            .child(self.blame_gutter(hunk, is_hunk_first, is_uncommitted, index))
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
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                    .text_color(rgb(visual.content_foreground))
                    .child(crate::ui_helpers::syntax_styled_text(&text, syntax_spans)),
            )
    }

    /// 注释栏：微灰底侧栏，块首行按「哈希 | 作者 | 日期 | 摘要」固定宽度
    /// 分列对齐（跨行纵向整齐），连续同块行留空；未提交块首行显示
    /// 「未提交」徽标。哈希用主色等宽字体点缀，其余弱化文字。
    /// 悬浮注释栏显示完整提交信息（作者/摘要可能被列宽截断）。
    fn blame_gutter(
        &self,
        hunk: Option<&khaslana::BlameHunkInfo>,
        is_hunk_first: bool,
        is_uncommitted: bool,
        index: usize,
    ) -> impl IntoElement {
        let commit = if is_hunk_first && !is_uncommitted {
            hunk.and_then(|hunk| hunk.commit.as_ref())
        } else {
            None
        };
        // 完整信息（含精确到秒的时间与未截断的作者/摘要）
        let commit_tooltip = commit.map(|commit| {
            format!(
                "{} {} {} {}",
                commit.short_oid,
                commit.author,
                crate::ui_helpers::commit_time_label(commit.time),
                commit.summary
            )
        });
        let visual = blame_line_visual_rule(is_uncommitted);

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .w(px(BLAME_GUTTER_WIDTH))
            .h(px(BLAME_ROW_HEIGHT))
            .pr(px(8.0))
            .overflow_hidden()
            // 已提交行的注释栏铺微灰底，与代码区分区；未提交行沿用整行
            // 警告底，确保警告行不会被侧栏底色覆盖。
            .bg(rgb(visual.gutter_background))
            .id(format!("blame-gutter-{index}"))
            // 悬浮显示完整提交信息（作者/摘要被截断时的兜底查看入口）
            .when_some(commit_tooltip, |this, tooltip| {
                this.tooltip(move |_window, cx| tooltip_text(tooltip.clone(), cx))
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
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .truncate()
                        .child(commit.author.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(BLAME_GUTTER_DATE_WIDTH))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(blame_date_label(commit.time)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                        .border_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                        .text_size(px(10.0))
                        .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                        .child("未提交"),
                )
            })
    }
}

/// 追溯注释栏的紧凑日期（含年份的 yyyy-mm-dd，本地时区）。
fn blame_date_label(seconds: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "----".to_string())
}
