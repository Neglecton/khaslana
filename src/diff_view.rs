//! 共享差异视图：工作区 / 历史 / 贮藏 / 浏览四个入口共用同一套差异渲染。
//!
//! 本模块承担两类职责：
//!
//! 1. **全文/紧凑切换**（`full_file_toggle_button` / `toggle_full_file_view`）。
//!    全文视图本质上是把 diff 的上下文行数拉满（`FULL_FILE_CONTEXT_LINES`），
//!    libgit2 会把整份文件作为上下文输出，改动行依旧按 Added/Removed 高亮。
//!    这意味着全文视图能复用现有的差异渲染、编码检测、虚拟列表与横向滚动能力。
//!    当文件体积超过 `FULL_FILE_MAX_BYTES` 时，GitService 层在分配逐行 String 之前
//!    就会返回 `FULL_FILE_TOO_LARGE_MESSAGE` 错误，UI 据此自动回退到紧凑差异。
//!
//! 2. **共享差异渲染**（`diff_section_header` / `render_virtual_diff` /
//!    `render_diff_row` / `render_encoding_dropdown`）。M5 按重构计划 §5 从
//!    `repository_ui.rs` 原样搬来：数据模型、虚拟列表、宽度测量、语法高亮槽位、
//!    部分暂存交互全部保持不变，只是把归属收到差异模块，让 `repository_ui.rs`
//!    回到「与具体页面无关的通用 UI」职责。**搬运过程不改 patch 算法。**

use gpui::{
    Context, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior, MouseButton,
    MouseDownEvent, div, prelude::*, px, uniform_list,
};

use crate::ui::theme::rgb;
use crate::{
    DiffEncodingChoice, DiffHeaderTarget, DiffLineKind, DiffRenderRow, DiffScope,
    ENCODING_MENU_WIDTH, EncodingMenuTarget, FileDiff, MainMode, RepositoryView, ScrollbarMode,
    SharedSyntaxSpans, binary_diff_placeholder, cached_widest_diff_row_index, diff_encoding_label,
    diff_header_toggle, diff_hunk_action_button, diff_line, diff_render_model_for,
    display_diff_line_kind, glass_menu, menu_separator, scrollable_uniform_frame,
    ui::{self, components::panel_section_header, theme as ui_theme},
};
use std::sync::Arc;

impl RepositoryView {
    /// 全文/差异切换按钮，放在差异区域标题栏编码按钮旁。
    /// 激活（全文）态使用强调色高亮，非激活态使用弱化样式。
    pub(crate) fn full_file_toggle_button(
        &self,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.full_file_view;
        // 按钮文案表达"点击后切换到的模式"
        let label = if active { "差异" } else { "全文" };
        diff_header_tool_button(
            match target {
                EncodingMenuTarget::Worktree => "worktree-full-file-toggle",
                EncodingMenuTarget::History => "history-full-file-toggle",
                EncodingMenuTarget::Stash => "stash-full-file-toggle",
                EncodingMenuTarget::Browse => "browse-full-file-toggle",
                // 追溯视图不经 diff_section_header 渲染该按钮（视图本身即整份文件）
                EncodingMenuTarget::Blame => "blame-full-file-toggle",
            },
            label,
            active,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.toggle_full_file_view(cx);
                cx.notify();
            }),
        )
    }

    /// 切换全文视图开关并重新加载当前可见的差异。
    /// 缓存 key 包含 `full_file` 字段，因此紧凑/全文两套缓存互不污染，无需清空。
    pub(crate) fn toggle_full_file_view(&mut self, cx: &mut Context<Self>) {
        self.full_file_view = !self.full_file_view;
        self.status = if self.full_file_view {
            "已切换为全文视图".to_string()
        } else {
            "已切换为差异视图".to_string()
        };
        self.reload_visible_diffs_after_full_file_change();
        cx.notify();
    }

    /// 切换全文开关后扇出重新加载当前可见的三类差异（工作区/历史/贮藏）。
    fn reload_visible_diffs_after_full_file_change(&mut self) {
        if let Some(diff) = self.diff.clone() {
            self.load_diff(diff.path.clone(), diff.scope.clone());
        }
        if self.main_mode == MainMode::History
            && let Some(path) = self.history_selected_file.clone()
        {
            self.select_history_file_with_reload(path, true);
        }
        if self.main_mode == MainMode::Stash
            && let Some(path) = self.stash_preview.selected_file.clone()
        {
            self.select_stash_file(path, true);
        }
        if self.main_mode == MainMode::Browse
            && self.browse.view_mode == crate::BrowseViewMode::Diff
        {
            self.browse.diff = None;
            self.browse.diff_headers_expanded = false;
            self.load_browse_current();
        }
    }

    /// 全文视图加载因文件过大失败时，自动回退到紧凑差异视图。
    /// 在错误事件处理闭包末尾调用，检测 `last_error` 是否为全文过大错误。
    pub(crate) fn revert_full_file_if_too_large_error(&mut self) {
        let is_too_large = self
            .last_error
            .as_deref()
            .is_some_and(|err| err.contains(khaslana::FULL_FILE_TOO_LARGE_MESSAGE));
        if is_too_large && self.full_file_view {
            self.full_file_view = false;
            self.last_error = None;
            self.status = "文件过大，已回退到差异视图".to_string();
            self.reload_visible_diffs_after_full_file_change();
        }
    }
    pub(crate) fn render_encoding_dropdown(
        &self,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.encoding_menu_target != Some(target) {
            return div().into_any_element();
        }
        let current = self.current_diff_encoding_choice();
        let title = match target {
            EncodingMenuTarget::Worktree => "工作区差异编码",
            EncodingMenuTarget::History => "提交差异编码",
            EncodingMenuTarget::Stash => "贮藏差异编码",
            EncodingMenuTarget::Browse => "浏览编码",
            EncodingMenuTarget::Blame => "追溯编码",
        };

        glass_menu()
            .absolute()
            .top(px(38.0))
            .right(px(12.0))
            .w(px(ENCODING_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(title),
            )
            .child(menu_separator())
            .child(self.encoding_menu_item(DiffEncodingChoice::Auto, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Utf8, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Gb18030, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Big5, current, cx))
            .into_any_element()
    }

    fn encoding_menu_item(
        &self,
        choice: DiffEncodingChoice,
        current: DiffEncodingChoice,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = choice == current;
        let label = if selected {
            format!("✓ {}", choice.label())
        } else {
            format!("  {}", choice.label())
        };
        div()
            .id(format!("context-menu-encoding-{}", choice.label()))
            .px_3()
            .py_1()
            .text_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::CONTENT_PRIMARY)
            })
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::WB_PANEL)
            })
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.choose_diff_encoding(choice);
                cx.notify();
            }))
            .child(label)
    }

    pub(crate) fn render_virtual_diff(
        &self,
        scroll_id: &'static str,
        diff: Option<Arc<FileDiff>>,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 二进制文件不渲染逐行 diff（也不显示「Binary files ... differ」原始行），
        // 直接显示信息占位卡片，含文件大小/新增删除信息。例外：Office 文档
        // （docx/xlsx/pptx）提取出的文本差异行——is_binary 保持 true（沿用
        // 全部二进制门控）但携带 lines，按普通行渲染文本化预览。
        if let Some(diff) = diff
            .as_deref()
            .filter(|diff| diff.is_binary && diff.lines.is_empty())
        {
            return binary_diff_placeholder(diff).into_any_element();
        }
        let model = diff_render_model_for(diff.as_deref(), headers_expanded);
        let row_count = model.row_count;
        let content_present = diff.is_some() && row_count > 0;
        // 以内容最宽的文本行作为列表水平宽度的测量基准，保证长行也能左右滚动。
        // 结果按 diff 身份缓存：大 diff（上限 2 万行）每帧重算是 O(总字符) 扫描。
        let width_measure_index = cached_widest_diff_row_index(
            diff.as_ref(),
            headers_expanded,
            &model,
            &self.widest_diff_row_cache,
        )
        .or_else(|| row_count.checked_sub(1));
        let handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = handle.clone();
        let model_for_list = model.clone();
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .font_family("Consolas")
            .text_size(px(12.0))
            // 代码内容保持平整底色（WB_DIFF_SURFACE），不跟随面板悬浮感加投影/圆角，
            // 否则每行都会像卡片。
            .bg(rgb(ui_theme::WB_DIFF_SURFACE))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let diff = diff.as_deref();
                        range
                            .map(|index| {
                                this.render_diff_row(
                                    diff,
                                    model_for_list.row_at(index),
                                    headers_expanded,
                                    header_target,
                                    &empty_message,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_width_from_item(width_measure_index)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Both,
            content,
            handle,
            content_present,
            cx,
        )
        .into_any_element()
    }

    /// 按差异视图上下文取对应槽位的语法高亮结果（带主题变体守卫）。
    fn syntax_spans_for_diff(&self, target: DiffHeaderTarget) -> Option<&SharedSyntaxSpans> {
        let spans = match target {
            DiffHeaderTarget::Worktree => &self.diff_syntax,
            DiffHeaderTarget::History => &self.history_diff_syntax,
            DiffHeaderTarget::Stash => &self.stash_preview.diff_syntax,
            DiffHeaderTarget::Browse => &self.browse.diff_syntax,
        };
        spans
            .as_deref()
            .filter(|spans| spans.dark == ui::theme::active_variant().is_dark())
    }

    fn render_diff_row(
        &self,
        diff: Option<&FileDiff>,
        row: DiffRenderRow,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 仅工作区差异视图提供部分暂存交互（历史/贮藏/浏览只读）；Office
        // 文档的文本化差异是提取合成的，不能按块/按行回写（部分暂存守卫在
        // 服务层同样拒绝），这里直接不显示按钮。
        let partial_stage_enabled =
            header_target == DiffHeaderTarget::Worktree && !diff.is_some_and(|d| d.is_binary);
        match row {
            DiffRenderRow::HeaderToggle => {
                let summary = if headers_expanded {
                    "Diff 元信息（点击折叠）"
                } else {
                    "Diff 元信息（点击展开）"
                };
                diff_header_toggle(summary, header_target, cx).into_any_element()
            }
            DiffRenderRow::DiffLine(index) => {
                let Some(line) = diff.and_then(|diff| diff.lines.get(index)) else {
                    return diff_line(DiffLineKind::Context, None, None, String::new(), None)
                        .into_any_element();
                };
                // 按差异上下文取对应槽位的语法高亮（仅全文模式计算过）。
                let syntax_spans = self
                    .syntax_spans_for_diff(header_target)
                    .and_then(|spans| spans.lines.get(index).map(Vec::as_slice));
                if line.kind == DiffLineKind::Header {
                    let is_hunk_header = line.content.starts_with("@@");
                    let row_element =
                        diff_line(line.kind.clone(), None, None, line.content.clone(), None);
                    if partial_stage_enabled && is_hunk_header {
                        // hunk 分隔行右侧提供整块暂存/取消暂存按钮。
                        let is_stage = diff
                            .map(|diff| diff.scope == DiffScope::Unstaged)
                            .unwrap_or(true);
                        let label: &'static str = if is_stage {
                            "暂存此块"
                        } else {
                            "取消暂存此块"
                        };
                        let hunk_index = line.hunk_index;
                        return div()
                            .relative()
                            .child(row_element)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_1()
                                    .flex()
                                    .items_center()
                                    .child(diff_hunk_action_button(
                                        hunk_index,
                                        label,
                                        move |this| {
                                            this.apply_hunk_partial_stage(hunk_index);
                                        },
                                        cx,
                                    )),
                            )
                            .into_any_element();
                    }
                    return row_element.into_any_element();
                }
                let row_element = diff_line(
                    display_diff_line_kind(line.kind.clone(), diff.is_some_and(|d| d.untracked)),
                    line.old_lineno,
                    line.new_lineno,
                    line.content.clone(),
                    syntax_spans,
                );
                if !partial_stage_enabled {
                    return row_element.into_any_element();
                }
                let selectable = matches!(line.kind, DiffLineKind::Added | DiffLineKind::Removed);
                if !selectable {
                    return row_element.into_any_element();
                }
                // +/- 行：点击选择（Ctrl/Cmd 多选、Shift 范围）。
                // 高亮层必须放在 row_element 之后：GPUI 按子元素顺序绘制，
                // 放在前面会被行自身的不透明背景完全盖住，视觉上不可见。
                let selected = self.diff_line_selection.contains(&index);
                div()
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            let multi = event.modifiers.control || event.modifiers.platform;
                            let shift = event.modifiers.shift;
                            this.toggle_diff_line_selection(index, multi, shift);
                            cx.notify();
                        }),
                    )
                    .child(row_element)
                    .when(selected, |this| {
                        this.child(
                            // 整行半透明主题色打底：复用输入选区 token（自带 alpha，跟随主题色），
                            // 叠加在 +/- 行背景色之上仍能清晰辨认选中范围。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .bg(ui_theme::rgba(ui_theme::INPUT_SELECTION)),
                        )
                        .child(
                            // 左缘 2px 主题色实线条作为第二重视觉信号。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(2.0))
                                .bg(rgb(ui_theme::PRIMARY)),
                        )
                    })
                    .into_any_element()
            }
            DiffRenderRow::Empty => {
                let message = diff
                    .map(|diff| {
                        if diff.is_binary {
                            "二进制文件仅显示元信息"
                        } else {
                            "没有可显示的文本差异"
                        }
                    })
                    .unwrap_or(empty_message);
                diff_line(DiffLineKind::Context, None, None, message.to_string(), None)
                    .into_any_element()
            }
        }
    }

    pub(crate) fn diff_section_header(
        &self,
        title: String,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let diff = match target {
            EncodingMenuTarget::Worktree => self.diff.as_deref(),
            EncodingMenuTarget::History => self.history_diff.as_deref(),
            EncodingMenuTarget::Stash => self.stash_preview.diff.as_deref(),
            EncodingMenuTarget::Browse => self.browse.diff.as_deref(),
            // 追溯视图没有 FileDiff；该 target 不经此头部渲染
            EncodingMenuTarget::Blame => None,
        };
        let tools = div()
            .flex()
            .items_center()
            .gap_2()
            // 二进制文件没有全文/编码差异可言，隐藏这两个工具按钮
            .when(!diff.is_some_and(|diff| diff.is_binary), |this| {
                this.child(self.full_file_toggle_button(target, cx))
                    .child(self.encoding_button(diff, target, cx))
            })
            // 按行选择非空时的部分暂存入口（仅工作区差异视图）。
            .when(
                target == EncodingMenuTarget::Worktree && !self.diff_line_selection.is_empty(),
                |this| {
                    let count = self.diff_line_selection.len();
                    let is_stage = self
                        .diff
                        .as_ref()
                        .map(|diff| diff.scope == DiffScope::Unstaged)
                        .unwrap_or(true);
                    let label = if is_stage {
                        format!("暂存选中行({count})")
                    } else {
                        format!("取消暂存选中行({count})")
                    };
                    this.child(
                        diff_header_tool_button("stage-selected-diff-lines-button", label, true)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.apply_selected_partial_stage();
                                cx.notify();
                            })),
                    )
                },
            )
            // 工作区差异的「追溯」入口：打开该文件的追溯视图
            //（规格严格复用「全文/编码」工具按钮；二进制文件不提供）。
            .when(
                target == EncodingMenuTarget::Worktree
                    && self.diff.as_ref().is_some_and(|diff| !diff.is_binary),
                |this| {
                    let path = self
                        .diff
                        .as_ref()
                        .map(|diff| diff.path.clone())
                        .unwrap_or_default();
                    this.child(
                        diff_header_tool_button("worktree-diff-blame-button", "追溯", true)
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.open_blame_file(path.clone());
                                cx.notify();
                            })),
                    )
                },
            );

        panel_section_header(title)
            .accent_title()
            .padding_x(ui_theme::SPACE_4)
            .action(tools.into_any_element())
            .build()
    }

    fn encoding_button(
        &self,
        diff: Option<&FileDiff>,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let requested = self.current_diff_encoding_choice();
        let label = diff
            .map(diff_encoding_label)
            .unwrap_or_else(|| format!("编码：{}", requested.label()));
        diff_header_tool_button(
            match target {
                EncodingMenuTarget::Worktree => "worktree-diff-encoding",
                EncodingMenuTarget::History => "history-diff-encoding",
                EncodingMenuTarget::Stash => "stash-diff-encoding",
                EncodingMenuTarget::Browse => "browse-encoding",
                EncodingMenuTarget::Blame => "blame-encoding",
            },
            label,
            false,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.toggle_encoding_menu(target);
                cx.notify();
            }),
        )
    }
}

/// 差异区标题栏的紧凑工具按钮（全文 / 编码 / 暂存选中行 / 追溯共用）。
///
/// 尺寸规格 px 8 / py 2 / RADIUS_XS / 11px 必须与差异区标题栏高度相容，
/// 否则会撑高标题行。激活态用强调色文字，非激活态弱化；两者共用同一底色，
/// 状态差异只走文字色，切换时不引起块状明度跳变。
fn diff_header_tool_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(ui_theme::RADIUS_XS))
        .bg(rgb(ui_theme::STATE_HOVER))
        .text_color(rgb(if active {
            ui_theme::PRIMARY
        } else {
            ui_theme::CONTENT_SECONDARY
        }))
        .text_size(px(11.0))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
        .child(label.into())
}
