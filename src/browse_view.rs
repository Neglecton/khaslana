// 分支浏览模式 UI 模块：左侧文件树浏览器 + 右侧只读内容/差异视图。
// 实现 RepositoryView 的 render_browse_view 及相关渲染与展平逻辑。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ui::theme::rgb;
use gpui::{
    ClickEvent, Context, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, div, prelude::*, px, uniform_list,
};
use khaslana::{BrowseEntry, BrowseEntryKind, BrowseListMode};

/// 内容视图每行高度（px），与 `browse_content_line` 中的 `h(px(18.0))` 一致。
pub(crate) const BROWSE_ROW_HEIGHT: f32 = 18.0;

/// Focus Workbench 内容画布的模式标签，保持分支比较的全文语义明确可见。
pub(crate) fn browse_canvas_mode_label(
    is_compare: bool,
    view_mode: BrowseViewMode,
) -> &'static str {
    match view_mode {
        BrowseViewMode::Content if is_compare => "目标分支全文",
        BrowseViewMode::Content => "文件内容",
        BrowseViewMode::Diff => "与当前分支差异",
    }
}

/// 文件树层级使用统一的紧凑缩进尺度，避免页面布局散落魔法数。
pub(crate) const fn browse_tree_indent(depth: usize) -> f32 {
    ui_theme::SPACE_3 * depth as f32
}

use crate::{
    BrowseViewMode, CHANGE_ROW_HEIGHT, EncodingMenuTarget, RepositoryView, ResizeTarget,
    diff_encoding_label, encoding_info_label,
    ui::{
        components::{command_group, empty_state, list_row_surface, page_header, segmented_button},
        theme as ui_theme,
    },
    ui_helpers::{ScrollbarMode, placeholder_row, scrollable_uniform_frame},
};

/// 展平后的可见文件树行，用于虚拟列表渲染。
#[derive(Clone, Debug)]
pub(crate) struct VisibleBrowseRow {
    pub entry: BrowseEntry,
    pub depth: usize,
}

/// 纯函数：把已加载的目录条目 + 展开集合展平为可见行序列。
///
/// 从根目录开始递归：对每个目录条目，如果已展开且其子树已加载，则递归展开其子项。
/// depth 从 0 开始，每深入一层 +1。
pub(crate) fn flatten_browse_tree(
    entries_by_dir: &HashMap<PathBuf, Vec<BrowseEntry>>,
    expanded: &HashSet<PathBuf>,
) -> Vec<VisibleBrowseRow> {
    fn recurse(
        dir: &Path,
        depth: usize,
        entries_by_dir: &HashMap<PathBuf, Vec<BrowseEntry>>,
        expanded: &HashSet<PathBuf>,
        out: &mut Vec<VisibleBrowseRow>,
    ) {
        let key = if dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            dir.to_path_buf()
        };
        let Some(entries) = entries_by_dir.get(&key) else {
            return;
        };
        for entry in entries {
            out.push(VisibleBrowseRow {
                entry: entry.clone(),
                depth,
            });
            // 目录已展开且子树已加载时递归
            if entry.kind == BrowseEntryKind::Directory && expanded.contains(Path::new(&entry.path))
            {
                recurse(
                    Path::new(&entry.path),
                    depth + 1,
                    entries_by_dir,
                    expanded,
                    out,
                );
            }
        }
    }
    let mut rows = Vec::new();
    recurse(Path::new(""), 0, entries_by_dir, expanded, &mut rows);
    rows
}

/// 在内容行中找出显示宽度最大的一行索引，用作 `uniform_list` 的
/// `with_width_from_item` 测量基准，确保长行也能驱动水平滚动条。
pub(crate) fn widest_browse_line_index(lines: &[String]) -> Option<usize> {
    (0..lines.len())
        .map(|index| (index, crate::display_columns(&lines[index])))
        .max_by_key(|&(_, columns)| columns)
        .map(|(index, _)| index)
}

/// 按内容身份缓存的最宽行扫描（见 `BrowseState::widest_line_cache`）。
/// 大文件打开期间每帧重算是 O(总字符)，拖动行选区等高频重绘会明显卡顿。
fn cached_widest_browse_line_index(
    content: &Option<std::sync::Arc<khaslana::BrowseFileContent>>,
    cache: &std::cell::RefCell<Option<((usize, usize), Option<usize>)>>,
) -> Option<usize> {
    let key = content.as_ref().map(|content| {
        (
            std::sync::Arc::as_ptr(content) as usize,
            content.lines.len(),
        )
    });
    let mut cache = cache.borrow_mut();
    if cache.as_ref().map(|(cached, _)| *cached) != key {
        let value = content
            .as_ref()
            .map(|content| widest_browse_line_index(&content.lines))
            .flatten();
        *cache = key.map(|key| (key, value));
    }
    cache.as_ref().and_then(|(_, value)| *value)
}

impl RepositoryView {
    pub(crate) fn render_browse_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::SURFACE_CANVAS))
            .child(match self.browse.list_mode {
                BrowseListMode::Tree => self.render_browse_file_tree(cx).into_any_element(),
                BrowseListMode::Compare => self.render_browse_compare_files(cx).into_any_element(),
            })
            .child(self.render_column_splitter(ResizeTarget::BrowseFiles, cx))
            .child(self.render_browse_content_area(cx))
    }

    /// 渲染左侧文件树浏览器。
    fn render_browse_file_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_display = self
            .browse
            .target
            .as_ref()
            .map(|target| target.display_name.clone())
            .unwrap_or_else(|| "加载中...".to_string());
        let short_oid = self
            .browse
            .target
            .as_ref()
            .map(|target| {
                target
                    .commit_oid
                    .get(..7)
                    .unwrap_or(&target.commit_oid)
                    .to_string()
            })
            .unwrap_or_default();

        // 同一帧的行数和可见项必须共享快照，避免处理器在滚动期间重建整棵树。
        let rows = Arc::new(flatten_browse_tree(
            &self.browse.entries_by_dir,
            &self.browse.expanded,
        ));
        let row_count = rows.len().max(1);
        let has_target = self.browse.target.is_some();
        let content_present = !rows.is_empty();
        let handle = self.uniform_scroll_handle("browse-tree-scroll");
        let list_handle = handle.clone();
        let rows_snapshot = Arc::clone(&rows);

        let content = div()
            .id("browse-tree-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .px(px(ui_theme::SPACE_2))
            .pb(px(ui_theme::SPACE_2))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                uniform_list(
                    "browse-tree-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                if rows_snapshot.is_empty() {
                                    return empty_state(
                                        "文件树",
                                        if !has_target {
                                            "正在解析引用..."
                                        } else if this.browse.loading_tree {
                                            "正在加载文件树..."
                                        } else {
                                            "仓库为空"
                                        },
                                    )
                                    .into_any_element();
                                }
                                rows_snapshot
                                    .get(index)
                                    .cloned()
                                    .map(|row| this.browse_tree_row(row, cx).into_any_element())
                                    .unwrap_or_else(|| placeholder_row("").into_any_element())
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.browse_tree_width))
            .min_w(px(self.browse_tree_width))
            .min_h(px(0.0))
            .h_full()
            // 右侧分隔线由紧随的列分割条（BrowseFiles）统一绘制，
            // 面板不自画右边框，避免出现两条平行框线。
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(page_header("分支浏览", Some("目标引用的只读文件树")))
            // 目标引用与退出操作保持在平面命令行，不额外堆叠卡片。
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(ui_theme::SPACE_2))
                    .px(px(ui_theme::SPACE_4))
                    .py(px(ui_theme::SPACE_2))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ui_theme::SPACE_1))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(ui_theme::TYPE_BODY))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .truncate()
                                    .child(target_display),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(ui_theme::TYPE_META))
                                    .font_family("Consolas, monospace")
                                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                    .child(short_oid),
                            ),
                    )
                    .child(command_group().child(self.button(
                        "关闭",
                        !self.busy,
                        |this, _, _| this.close_browse(),
                        cx,
                    ))),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(ui_theme::SPACE_4))
                    .py(px(ui_theme::SPACE_2))
                    .text_size(px(ui_theme::TYPE_META))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("文件树"),
            )
            .child(scrollable_uniform_frame(
                "browse-tree-scroll",
                ScrollbarMode::Vertical,
                content,
                handle,
                content_present,
                cx,
            ))
    }

    /// 渲染文件树的一行。
    fn browse_tree_row(&self, row: VisibleBrowseRow, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = row.entry;
        let depth = row.depth;
        let indent = px(browse_tree_indent(depth));
        let is_dir = entry.kind == BrowseEntryKind::Directory;
        let is_expanded = self.browse.expanded.contains(Path::new(&entry.path));
        let is_selected = self
            .browse
            .selected_file
            .as_deref()
            .map(|selected| selected == Path::new(&entry.path))
            .unwrap_or(false);
        let is_submodule = entry.kind == BrowseEntryKind::Submodule;

        // 目录可点击展开/折叠；文件可点击选中
        let path_for_click = PathBuf::from(&entry.path);

        let caret = if is_dir {
            if is_expanded { "▼" } else { "▶" }
        } else {
            ""
        };

        let icon = match entry.kind {
            BrowseEntryKind::Directory => {
                if is_expanded {
                    "📂"
                } else {
                    "📁"
                }
            }
            BrowseEntryKind::File => "📄",
            BrowseEntryKind::Submodule => "📦",
        };

        let name_color = if is_submodule {
            ui_theme::CONTENT_TERTIARY
        } else {
            ui_theme::CONTENT_PRIMARY
        };

        list_row_surface(format!("browse-row-{}", entry.path), is_selected)
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap(px(ui_theme::SPACE_1))
            .h(px(CHANGE_ROW_HEIGHT))
            .pl(indent)
            .pr(px(ui_theme::SPACE_2))
            .cursor_pointer()
            .overflow_hidden()
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                if is_dir {
                    this.toggle_browse_dir(path_for_click.clone());
                } else {
                    this.select_browse_file(path_for_click.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .text_size(px(ui_theme::TYPE_META))
                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                    .child(caret),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(18.0))
                    .text_size(px(13.0))
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(name_color))
                    .truncate()
                    .child(entry.name),
            )
    }

    /// 渲染右侧内容区域（内容/差异切换 + 视图）。
    fn render_browse_content_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let show_ai_review = self.browse.list_mode == BrowseListMode::Compare;
        // AI 评审展开时占满右侧区域（替换内容/差异视图），收起时回底部
        // 单行条；生成中也可收起（进度显示在底部条）。
        let ai_review_full = show_ai_review && self.ai_review_expanded;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            .h_full()
            .bg(rgb(ui_theme::SURFACE_CANVAS))
            .child(self.render_browse_content_header(cx))
            .child(if ai_review_full {
                self.render_ai_review_panel(cx).into_any_element()
            } else {
                match self.browse.view_mode {
                    BrowseViewMode::Content => {
                        self.render_browse_content_view(cx).into_any_element()
                    }
                    BrowseViewMode::Diff => self.render_browse_diff_view(cx).into_any_element(),
                }
            })
            .when(show_ai_review && !ai_review_full, |this| {
                this.child(self.render_ai_review_panel(cx))
            })
            // 评审历史弹窗（覆盖层）
            .when(self.ai_review_history.is_some(), |this| {
                this.child(self.render_ai_review_history(cx))
            })
            // 编码选择下拉菜单
            .child(self.render_encoding_dropdown(EncodingMenuTarget::Browse, cx))
    }

    /// 右侧顶部栏：模式切换 + 编码按钮。
    fn render_browse_content_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_path = self
            .browse
            .selected_file
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "未选择文件".to_string());

        let mode_label = browse_canvas_mode_label(
            self.browse.list_mode == BrowseListMode::Compare,
            self.browse.view_mode,
        );

        page_header("内容画布", Some("所选文件的只读内容或差异"))
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .truncate()
                    .child(format!("{mode_label}: {selected_path}")),
            )
            .child(
                command_group()
                    // 内容/差异切换
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(ui_theme::SPACE_1))
                            .child(self.browse_mode_segment(BrowseViewMode::Content, "内容", cx))
                            .child(self.browse_mode_segment(BrowseViewMode::Diff, "差异", cx)),
                    )
                    // 编码按钮：两种模式都显示
                    .child(self.browse_encoding_button(cx)),
            )
    }

    /// 模式切换的分段按钮。
    fn browse_mode_segment(
        &self,
        mode: BrowseViewMode,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.browse.view_mode == mode;
        segmented_button(format!("browse-mode-{mode:?}"), selected, !self.busy)
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    this.set_browse_view_mode(mode);
                    cx.notify();
                }),
            )
    }

    /// 浏览模式的编码按钮：内容模式取 BrowseFileContent.encoding，差异模式取 FileDiff.encoding。
    /// 点击后弹出编码选择下拉菜单（复用 EncodingMenuTarget::Browse），选择后重新加载当前文件。
    fn browse_encoding_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.browse_encoding_label();
        div()
            .id("browse-encoding")
            .relative()
            .flex_none()
            .px(px(ui_theme::SPACE_2))
            .py(px(ui_theme::SPACE_1))
            .rounded(px(ui_theme::RADIUS_XS))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
            .text_size(px(ui_theme::TYPE_META))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.toggle_encoding_menu(EncodingMenuTarget::Browse);
                    cx.notify();
                }),
            )
            .child(label)
    }

    /// 根据当前模式生成编码标签文本。
    fn browse_encoding_label(&self) -> String {
        match self.browse.view_mode {
            BrowseViewMode::Content => {
                if let Some(content) = self.browse.content.as_ref() {
                    encoding_info_label(&content.encoding)
                } else {
                    format!("编码：{}", self.current_diff_encoding_choice().label())
                }
            }
            BrowseViewMode::Diff => {
                if let Some(diff) = self.browse.diff.as_ref() {
                    diff_encoding_label(diff)
                } else {
                    format!("编码：{}", self.current_diff_encoding_choice().label())
                }
            }
        }
    }

    /// 只读内容视图：虚拟列表渲染文件行。
    fn render_browse_content_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.browse.content.clone();
        let loading = self.browse.loading_content;

        let row_count = if let Some(content) = content.as_ref() {
            if content.is_binary {
                1
            } else {
                content.lines.len().max(1)
            }
        } else {
            1
        };
        let content_present = content.is_some();
        let handle = self.uniform_scroll_handle("browse-content-scroll");
        let list_handle = handle.clone();
        let entity = cx.entity();

        let inner_content = div()
            .id("browse-content-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p(px(ui_theme::SPACE_3))
            .font_family("Consolas, monospace")
            .text_size(px(ui_theme::TYPE_BODY))
            .bg(rgb(ui_theme::SURFACE_BASE))
            // 内容区行选择纯鼠标（拖选）；不设键盘上下文/焦点——键盘复制/全选
            // 仅保留在文本框内（键盘白名单见 AGENTS.md §8）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let line_count = this
                        .browse
                        .content
                        .as_ref()
                        .map(|c| if c.is_binary { 0 } else { c.lines.len() })
                        .unwrap_or(0);
                    if line_count == 0 {
                        return;
                    }
                    let row = this.browse_row_for_mouse_y(event.position.y, line_count);
                    this.browse.sel_start = Some(row);
                    this.browse.sel_end = Some(row);
                    this.browse.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !this.browse.selecting {
                        return;
                    }
                    // 按住拖出内容区后释放鼠标时，mouse_up 不会经过本元素，
                    // selecting 会卡在 true：此后纯 hover 也持续改选区。
                    // 依赖事件自带的按键状态复位，与滚动条拖拽同一套模式。
                    if !event.dragging() {
                        this.browse.selecting = false;
                        cx.notify();
                        return;
                    }
                    let line_count = this
                        .browse
                        .content
                        .as_ref()
                        .map(|c| if c.is_binary { 0 } else { c.lines.len() })
                        .unwrap_or(0);
                    if line_count == 0 {
                        return;
                    }
                    let row = this.browse_row_for_mouse_y(event.position.y, line_count);
                    if this.browse.sel_end != Some(row) {
                        this.browse.sel_end = Some(row);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    if this.browse.selecting {
                        this.browse.selecting = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                uniform_list(
                    "browse-content-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, _cx| {
                        let content = this.browse.content.clone();
                        range
                            .map(|index| {
                                let Some(content) = content.as_ref() else {
                                    let detail = if this.browse.loading_content {
                                        "正在加载文件内容..."
                                    } else if this
                                        .browse
                                        .selected_compare_file
                                        .as_ref()
                                        .is_some_and(|file| {
                                            file.status == khaslana::ChangeState::Deleted
                                        })
                                    {
                                        "目标分支中不存在该文件，请切换到差异视图查看删除内容"
                                    } else {
                                        "请选择一个文件查看内容"
                                    };
                                    return empty_state("内容画布", detail).into_any_element();
                                };
                                if content.is_binary {
                                    return empty_state(
                                        "无法预览二进制文件",
                                        "请切换到差异视图查看文件变更信息",
                                    )
                                    .into_any_element();
                                }
                                let line = content.lines.get(index).cloned().unwrap_or_default();
                                this.browse_content_line(index, line).into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_width_from_item(cached_widest_browse_line_index(
                    &content,
                    &self.browse.widest_line_cache,
                ))
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        let _ = (loading, entity);
        scrollable_uniform_frame(
            "browse-content-scroll",
            ScrollbarMode::Both,
            inner_content,
            handle,
            content_present,
            cx,
        )
    }

    /// 渲染一行只读文件内容（带行号 + 语法高亮）。
    fn browse_content_line(&self, index: usize, text: String) -> impl IntoElement {
        let selected = self.browse.is_row_selected(index);
        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_start()
            .gap_2()
            .h(px(BROWSE_ROW_HEIGHT))
            .when(selected, |this| this.bg(rgb(ui_theme::STATE_SELECTION)))
            .child(
                div()
                    .flex_none()
                    .w(px(40.0))
                    .text_size(px(ui_theme::TYPE_META))
                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                    .text_align(gpui::TextAlign::Right)
                    .child((index + 1).to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(BROWSE_ROW_HEIGHT))
                    .line_height(px(BROWSE_ROW_HEIGHT))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(crate::ui_helpers::syntax_styled_text(
                        &text,
                        crate::ui_helpers::syntax_spans_for_line(
                            &self.browse.content_syntax,
                            index,
                        ),
                    )),
            )
    }

    /// 差异视图：复用现有 diff 渲染。
    fn render_browse_diff_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty_message = if self.browse.loading_diff {
            "文件差异加载中..."
        } else {
            "请选择一个文件查看与当前分支的差异"
        };

        self.render_virtual_diff(
            "browse-diff-scroll",
            self.browse.diff.clone(),
            self.browse.diff_headers_expanded,
            crate::DiffHeaderTarget::Browse,
            empty_message.to_string(),
            cx,
        )
    }
}

#[cfg(test)]
#[path = "tests/browse_view.rs"]
mod tests;
