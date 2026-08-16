use crate::ui::theme::rgb;
use gpui::{
    Context, CursorStyle, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, div, point, prelude::*, px, uniform_list,
};
use khaslana::{CommitFileChange, CommitInfo, CommitRefInfo, CommitRefKind};

use crate::{
    CHANGE_ROW_HEIGHT, DiffHeaderTarget, EncodingMenuTarget, RepositoryView, ResizeTarget,
    ScrollbarMode, author_avatar, change_state_badge, column_splitter_accepts_mouse_events,
    column_splitter_should_clear_resize, commit_time_label, history_scope_button, placeholder_row,
    scrollable_frame_when, scrollable_uniform_frame, section_header, section_header_action,
    ui::{
        components::{metric_badge, tooltip_text},
        theme as ui_theme,
    },
};

// 提交记录图形单元覆盖完整行高，保证相邻行的轨道连续；列宽由 history_graph_width 状态提供，可拖拽调整。
const HISTORY_GRAPH_ROW_HEIGHT: f32 = 36.0;
// 提交行只直接展示少量引用，剩余引用通过 +n 的悬浮提示查看，避免挤压提交摘要。
const MAX_COMMIT_REF_LABELS: usize = 3;
const GRAPH_LANE_START: f32 = 12.0;
const GRAPH_LANE_SPACING: f32 = 14.0;
// 图形列右侧的拖拽分割条宽度，行内流式排布，自动与图形列对齐。
const GRAPH_SPLITTER_WIDTH: f32 = 6.0;
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitGraphRow {
    lane: usize,
    lanes: Vec<usize>,
    connectors: Vec<usize>,
    connected_from_top: bool,
}

impl RepositoryView {
    pub(crate) fn render_history_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selection = self.history_selected_commit.is_some();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
            .child(self.render_commit_history(cx))
            .child(self.render_column_splitter(ResizeTarget::HistoryTop, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    // 左列（与提交文件列表同宽）：上半为提交详情（默认与文件列表
                    // 对半分，可拖拽改绝对高度），下半为文件列表；无选中提交时
                    // 详情区整体不渲染，左列仅剩文件列表。
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(px(self.history_files_width))
                            .min_w(px(self.history_files_width))
                            .min_h(px(0.0))
                            // 1px 透明标记：每帧记录左列顶部窗口坐标，供对半分
                            // 模式下拖拽起始时推导详情区实际高度（见
                            // start_resize_column）。
                            .child({
                                let top_hint = self.history_details_top_hint.clone();
                                gpui::canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, _| {
                                        top_hint.set(f32::from(bounds.origin.y));
                                    },
                                )
                                .w_full()
                                .h(px(1.0))
                            })
                            .when(has_selection, |this| {
                                this.child(self.render_commit_details(cx)).child(
                                    self.render_column_splitter(ResizeTarget::HistoryDetails, cx),
                                )
                            })
                            .child(self.render_commit_files(cx)),
                    )
                    .child(self.render_column_splitter(ResizeTarget::HistoryFiles, cx))
                    .child(self.render_history_diff(cx)),
            )
    }

    fn render_commit_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = if self.history_commits.is_empty() {
            1
        } else if self.history_refreshing {
            // 刷新期间 has_more 可能过时，不渲染"加载更多"行
            self.history_commits.len()
        } else {
            self.history_commits.len() + usize::from(self.history_has_more)
        };
        let content_present = !self.history_commits.is_empty();
        let handle = self.uniform_scroll_handle("commit-history-list");
        let list_handle = handle.clone();
        // 拖拽提交图列宽时挂载窗口级鼠标事件承载层；无命中区，不拦截列表点击。
        // 过滤模式下图形列隐藏，无需承载层。
        let graph_resize_overlay = (self.resize_state(ResizeTarget::HistoryGraph).is_some()
            && self.history_file_filter.is_none())
        .then(|| self.history_graph_resize_overlay(cx).into_any_element());
        let content = div()
            .id("commit-history-list")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    "commit-history-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                if this.history_commits.is_empty() {
                                    return placeholder_row(if this.history_loading.commits {
                                        "提交记录加载中..."
                                    } else if this.repo_path.is_some() {
                                        "暂无提交记录"
                                    } else {
                                        "请先打开一个仓库"
                                    })
                                    .into_any_element();
                                }
                                if index == this.history_commits.len() {
                                    // 刷新期间隐藏"加载更多"按钮
                                    if this.history_refreshing {
                                        return placeholder_row("").into_any_element();
                                    }
                                    return div()
                                        .flex_none()
                                        .w_full()
                                        .min_w(px(0.0))
                                        .h(px(HISTORY_GRAPH_ROW_HEIGHT))
                                        .items_center()
                                        .py_1()
                                        .child(this.button(
                                            if this.history_loading.commits {
                                                "加载中..."
                                            } else {
                                                "加载更多"
                                            },
                                            !this.history_loading.commits,
                                            |this, _, _| this.load_more_history(),
                                            cx,
                                        ))
                                        .into_any_element();
                                }
                                let Some(commit) = this.history_commits.get(index).cloned() else {
                                    return placeholder_row("").into_any_element();
                                };
                                let graph = this
                                    .history_graph_rows
                                    .get(index)
                                    .cloned()
                                    .unwrap_or_default();
                                this.commit_row(commit, graph, cx).into_any_element()
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
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .min_w(px(0.0))
            .h(px(self.history_top_height))
            .min_h(px(180.0))
            .w_full()
            .child(section_header_action(
                format!("提交记录（{}）", self.history_scope.label()),
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(history_scope_button(
                            "当前分支",
                            self.history_scope == khaslana::HistoryScope::CurrentBranch,
                            |this| this.set_history_scope(khaslana::HistoryScope::CurrentBranch),
                            cx,
                        ))
                        .child(history_scope_button(
                            "所有分支",
                            self.history_scope == khaslana::HistoryScope::AllRefs,
                            |this| this.set_history_scope(khaslana::HistoryScope::AllRefs),
                            cx,
                        ))
                        // 文件路径过滤 chip：点击清除过滤，悬浮提示完整路径
                        .children(
                            self.history_file_filter
                                .as_deref()
                                .map(|path| history_file_filter_chip(path, cx)),
                        )
                        .into_any_element(),
                ),
            ))
            .child(scrollable_uniform_frame(
                "commit-history-list",
                ScrollbarMode::Vertical,
                content,
                handle,
                content_present,
                cx,
            ))
            .children(graph_resize_overlay)
    }

    /// 提交图列右侧的行内拖拽分割条：流式排布自动与图形列对齐，吞掉点击避免误选提交。
    fn render_history_graph_splitter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.resize_state(ResizeTarget::HistoryGraph).is_some();
        // 弹窗或弹层菜单打开时不显示拖拽光标、不响应，与列分割线行为一致
        let interactive = column_splitter_accepts_mouse_events(
            self.active_dialog.is_some(),
            self.any_popup_menu_open(),
        );
        div()
            .flex_none()
            .relative()
            .w(px(GRAPH_SPLITTER_WIDTH))
            .h_full()
            .when(interactive, |this| this.cursor(CursorStyle::ResizeColumn))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    // 阻止冒泡到提交行的 on_click，避免拖拽分割条时误选中提交。
                    cx.stop_propagation();
                    if !column_splitter_accepts_mouse_events(
                        this.active_dialog.is_some(),
                        this.any_popup_menu_open(),
                    ) {
                        this.finish_resize_column(ResizeTarget::HistoryGraph);
                        cx.notify();
                        return;
                    }
                    if event.click_count >= 2 {
                        this.reset_resize_target(ResizeTarget::HistoryGraph);
                    } else {
                        this.start_resize_column(ResizeTarget::HistoryGraph, event);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(px(1.0))
                    .bg(if active {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    }),
            )
    }

    /// 拖拽提交图列宽期间的窗口级鼠标事件承载层：无命中区，不拦截列表点击。
    fn history_graph_resize_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _cx| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, _, cx| {
                        let (resizing, active_dialog, popup_open) = {
                            let view = entity.read(cx);
                            (
                                view.resize_state(ResizeTarget::HistoryGraph).is_some(),
                                view.active_dialog.is_some(),
                                view.any_popup_menu_open(),
                            )
                        };
                        if column_splitter_should_clear_resize(
                            active_dialog || popup_open,
                            resizing,
                        ) {
                            entity.update(cx, |this, cx| {
                                this.finish_resize_column(ResizeTarget::HistoryGraph);
                                cx.notify();
                            });
                            return;
                        }
                        if !resizing
                            || !event.dragging()
                            || !column_splitter_accepts_mouse_events(active_dialog, popup_open)
                        {
                            return;
                        }
                        entity.update(cx, |this, cx| {
                            this.update_resize_column(ResizeTarget::HistoryGraph, event);
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    let (resizing, active_dialog, popup_open) = {
                        let view = entity.read(cx);
                        (
                            view.resize_state(ResizeTarget::HistoryGraph).is_some(),
                            view.active_dialog.is_some(),
                            view.any_popup_menu_open(),
                        )
                    };
                    if !resizing {
                        return;
                    }
                    if !column_splitter_accepts_mouse_events(active_dialog, popup_open)
                        && !column_splitter_should_clear_resize(
                            active_dialog || popup_open,
                            resizing,
                        )
                    {
                        return;
                    }
                    entity.update(cx, |this, cx| {
                        this.finish_resize_column(ResizeTarget::HistoryGraph);
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
    }

    fn commit_row(
        &self,
        commit: CommitInfo,
        graph: CommitGraphRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.history_selected_commit.as_deref() == Some(commit.oid.as_str());
        let oid = commit.oid.clone();
        let right_click_oid = commit.oid.clone();
        let right_click_short_oid = commit.short_oid.clone();
        let right_click_summary = commit.summary.clone();
        let right_click_parent_count = commit.parents.len();
        let author = commit.author.clone();
        let time = commit_time_label(commit.time);
        let row_short_oid = commit.short_oid.clone();
        let ref_labels = commit_ref_labels(&commit.refs, &row_short_oid);
        let hidden_refs = commit
            .refs
            .iter()
            .skip(MAX_COMMIT_REF_LABELS)
            .cloned()
            .collect::<Vec<_>>();
        let hidden_ref_count = hidden_refs.len();
        let unpushed = self
            .branch_sync_status
            .as_ref()
            .is_some_and(|status| status.unpushed_oids.iter().any(|oid| oid == &commit.oid));

        div()
            .id(format!("commit-{row_short_oid}"))
            .relative()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_1()
            .pr_2()
            .h(px(HISTORY_GRAPH_ROW_HEIGHT))
            .rounded_sm()
            .cursor_pointer()
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else if unpushed {
                rgb(ui_theme::COLOR_WARNING)
            } else {
                rgb(ui_theme::CARD)
            })
            .border_1()
            .border_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else if unpushed {
                rgb(ui_theme::COLOR_WARNING)
            } else {
                rgb(ui_theme::BORDER)
            })
            .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
            .when(unpushed, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(8.0))
                        .bottom(px(8.0))
                        .flex_none()
                        .w(px(3.0))
                        .rounded_sm()
                        .bg(rgb(ui_theme::COLOR_WARNING)),
                )
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_history_commit(oid.clone());
                cx.notify();
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _window, cx| {
                    this.open_commit_context_menu(
                        right_click_oid.clone(),
                        right_click_short_oid.clone(),
                        right_click_summary.clone(),
                        right_click_parent_count,
                        event,
                        _window,
                    );
                    cx.notify();
                }),
            )
            // 过滤模式下隐藏提交图形列（含列宽分割条）：过滤后中间提交缺失，
            // 泳道线会断裂，隐藏最干净。
            .when(self.history_file_filter.is_none(), |this| {
                this.child(render_commit_graph_cell(graph, self.history_graph_width))
                    .child(self.render_history_graph_splitter(cx))
            })
            .child(
                div()
                    .flex_none()
                    .w(px(68.0))
                    .px_1()
                    .py(px(2.0))
                    .rounded_sm()
                    .bg(rgb(ui_theme::ACCENT))
                    .font_family("Consolas, monospace")
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .text_align(gpui::TextAlign::Center)
                    .child(row_short_oid.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(if selected {
                                rgb(ui_theme::FOREGROUND)
                            } else {
                                rgb(ui_theme::FOREGROUND)
                            })
                            .truncate()
                            .child(commit.summary),
                    )
                    .children(ref_labels)
                    .when(hidden_ref_count > 0, |this| {
                        this.child(commit_ref_overflow_label(&row_short_oid, hidden_refs))
                    })
                    .when(unpushed, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .px_1()
                                .py(px(1.0))
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(ui_theme::COLOR_WARNING))
                                .bg(rgb(ui_theme::COLOR_WARNING))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                                .child("未推送"),
                        )
                    }),
            )
            .child(author_avatar(&author))
            .child(
                div()
                    .flex_none()
                    .w(px(100.0))
                    .text_size(px(11.0))
                    .text_color(if selected {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .truncate()
                    .child(author),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(128.0))
                    .text_size(px(11.0))
                    .text_color(if selected {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .child(time),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(16.0))
                    .text_color(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .child(">"),
            )
    }

    /// 提交详情区（历史页左列上半部）：展示选中提交的完整提交信息、
    /// 作者/提交者、时间、完整 SHA 与父提交关系；可折叠，高度可拖拽。
    fn render_commit_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(commit) = self
            .history_selected_commit
            .as_deref()
            .and_then(|oid| self.history_commits.iter().find(|info| info.oid == oid))
            .cloned()
        else {
            return section_header("提交详情").into_any_element();
        };

        let collapsed = self.history_details_collapsed;
        let toggle_label: &'static str = if collapsed { "展开" } else { "收起" };
        let header_title = if collapsed {
            format!("提交详情 · {}", commit.summary)
        } else {
            "提交详情".to_string()
        };
        let header = section_header_action(
            header_title,
            Some(
                history_scope_button(
                    toggle_label,
                    false,
                    |this| {
                        this.history_details_collapsed = !this.history_details_collapsed;
                    },
                    cx,
                )
                .into_any_element(),
            ),
        );

        if collapsed {
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .child(header)
                .into_any_element();
        }

        // 完整提交信息（去首尾空白；与摘要相同则不重复展示）。
        let message_body = commit.message.trim();
        let body_text = (message_body != commit.summary).then(|| message_body.to_string());

        let oid_for_copy = commit.oid.clone();
        let message_for_copy = message_body.to_string();
        let mut meta_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_3()
            .gap_y_1()
            .text_size(px(11.0))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(
                div()
                    .font_family("Consolas, monospace")
                    .child(commit.oid.clone()),
            )
            .child(
                div()
                    .id("history-details-copy-sha")
                    .flex_none()
                    .px_2()
                    .py(px(1.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .child("复制 SHA")
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.copy_commit_sha(oid_for_copy.clone(), cx);
                    })),
            )
            .child(
                div()
                    .id("history-details-copy-message")
                    .flex_none()
                    .px_2()
                    .py(px(1.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .child("复制信息")
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            message_for_copy.clone(),
                        ));
                        this.status = "已复制提交信息".into();
                        this.last_error = None;
                        this.notify_success(this.status.clone(), cx);
                    })),
            )
            .child(div().child(format!("作者 {}", author_label(&commit))))
            .child(div().child(format!("提交时间 {}", commit_time_label(commit.time))))
            .child(div().child(parents_note(&commit.parents)));
        if let Some(committer) = committer_note(&commit) {
            meta_row = meta_row.child(div().child(format!("提交者 {committer}")));
        }

        let handle = self.scroll_handle("history-details-scroll");
        let scroll_content = div()
            .id("history-details-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .px_3()
            .py_2()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(commit.summary.clone()),
                    )
                    .children(commit_ref_labels(&commit.refs, &commit.short_oid)),
            )
            .children(body_text.map(|text| {
                div()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(text)
            }))
            .child(meta_row);

        // 内容区套用统一的自绘滚动条容器（与其他滚动区域一致的视觉与拖拽交互）。
        let content = scrollable_frame_when(
            "history-details-scroll",
            ScrollbarMode::Vertical,
            scroll_content.into_any_element(),
            handle,
            true,
            cx,
        );

        match self.history_details_height {
            // 手动拖拽过的绝对高度。
            Some(height) => div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(height))
                .child(header)
                .child(content)
                .into_any_element(),
            // 默认：与文件列表上下对半分（双方各占 flex_1）。
            None => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(header)
                .child(content)
                .into_any_element(),
        }
    }

    fn render_commit_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.history_files.len().max(1);
        let content_present = !self.history_files.is_empty();
        let handle = self.uniform_scroll_handle("commit-file-list");
        let list_handle = handle.clone();
        let content = div()
            .id("commit-file-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    "commit-file-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                if this.history_files.is_empty() {
                                    return placeholder_row(if this.history_loading.files {
                                        "提交文件加载中..."
                                    } else if this.history_selected_commit.is_some() {
                                        "该提交没有文件变更"
                                    } else {
                                        "请选择一个提交"
                                    })
                                    .into_any_element();
                                }
                                this.history_files
                                    .get(index)
                                    .cloned()
                                    .map(|file| this.commit_file_row(file, cx).into_any_element())
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
            // 位于左列 flex_col 中（上方为提交详情区）：占余高；宽度由父容器约束。
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(section_header("提交文件"))
            .child(scrollable_uniform_frame(
                "commit-file-list",
                ScrollbarMode::Vertical,
                content,
                handle,
                content_present,
                cx,
            ))
    }

    fn commit_file_row(&self, file: CommitFileChange, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.history_selected_file.as_deref() == Some(file.path.as_str());
        let path = file.path.clone();
        let right_click_path = file.path.clone();
        let path_label = file
            .old_path
            .as_ref()
            .filter(|old_path| old_path.as_str() != file.path.as_str())
            .map(|old_path| format!("{old_path} -> {}", file.path))
            .unwrap_or_else(|| file.path.clone());

        div()
            .id(format!("commit-file-{}", file.path))
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_1()
            .h(px(CHANGE_ROW_HEIGHT))
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else {
                rgb(ui_theme::CARD)
            })
            .border_1()
            .border_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::BORDER)
            })
            .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_history_file(path.clone());
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.select_history_file(right_click_path.clone());
                    this.open_file_path_context_menu(right_click_path.clone(), event, window);
                    cx.notify();
                }),
            )
            .child(change_state_badge(Some(&file.status)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(path_label),
            )
    }

    fn render_history_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 全文视图模式下标题前缀"全文："，提示当前展示整份文件
        let prefix = if self.full_file_view { "全文：" } else { "" };
        let title = self
            .history_selected_file
            .as_ref()
            .map(|path| format!("{prefix}提交差异：{path}"))
            .unwrap_or_else(|| "提交差异".to_string());
        let empty_message = if self.history_loading.diff {
            "提交差异加载中..."
        } else {
            "请选择一个提交文件查看差异"
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            .h_full()
            .child(self.diff_section_header(title, EncodingMenuTarget::History, cx))
            .child(self.render_virtual_diff(
                "history-diff-scroll",
                self.history_diff.clone(),
                self.history_diff_headers_expanded,
                DiffHeaderTarget::History,
                empty_message.to_string(),
                cx,
            ))
            .child(self.render_encoding_dropdown(EncodingMenuTarget::History, cx))
    }
}

pub(crate) fn commit_graph_rows(commits: &[CommitInfo]) -> Vec<CommitGraphRow> {
    // 不再按“已加载窗口”剪枝泳道：revwalk 为完整父提交遍历，未分页到的父提交最终必被加载，
    // 剪枝只会让合并第二父等泳道在引入行悬空、并在跨页加载时改变上方行的泳道分配，造成断线与抖动。
    // 保留泳道后，每行状态仅依赖该行及之前的提交，线条跨行连续且跨页前缀稳定。
    let mut lanes = Vec::<Option<String>>::new();
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let existing_lane = lanes
            .iter()
            .position(|oid| oid.as_deref() == Some(commit.oid.as_str()));
        let connected_from_top = existing_lane.is_some();
        let lane = existing_lane.unwrap_or_else(|| {
            if let Some(index) = lanes.iter().position(Option::is_none) {
                lanes[index] = Some(commit.oid.clone());
                index
            } else {
                lanes.push(Some(commit.oid.clone()));
                lanes.len() - 1
            }
        });
        let lanes_before = active_lane_indices(&lanes, lane);
        let mut connectors = Vec::new();

        if let Some(first_parent) = commit.parents.first() {
            // 分叉汇合：first parent 已占据其它泳道时，当前泳道并入该泳道并
            // 释放。若无条件写入当前泳道，parent 会同时占据两条泳道——父提交
            // 行只清理第一条匹配，另一条成为幽灵泳道，贯穿竖线画到列表末尾。
            if let Some(existing) = lanes
                .iter()
                .position(|oid| oid.as_deref() == Some(first_parent.as_str()))
            {
                connectors.push(existing);
                lanes[lane] = None;
            } else {
                lanes[lane] = Some(first_parent.clone());
                connectors.push(lane);
            }
        } else {
            lanes[lane] = None;
        }

        for parent in commit.parents.iter().skip(1) {
            let parent_lane = lanes
                .iter()
                .position(|oid| oid.as_deref() == Some(parent.as_str()))
                .unwrap_or_else(|| {
                    if let Some(index) = lanes.iter().position(Option::is_none) {
                        lanes[index] = Some(parent.clone());
                        index
                    } else {
                        lanes.push(Some(parent.clone()));
                        lanes.len() - 1
                    }
                });
            connectors.push(parent_lane);
        }

        connectors.sort_unstable();
        connectors.dedup();
        rows.push(CommitGraphRow {
            lane,
            lanes: lanes_before,
            connectors,
            connected_from_top,
        });
    }

    rows
}

fn active_lane_indices(lanes: &[Option<String>], current_lane: usize) -> Vec<usize> {
    let mut indices = lanes
        .iter()
        .enumerate()
        .filter_map(|(index, oid)| oid.as_ref().map(|_| index))
        .collect::<Vec<_>>();
    if !indices.contains(&current_lane) {
        indices.push(current_lane);
    }
    indices.sort_unstable();
    indices.dedup();
    indices
}

// 由图形列宽推算最右可绘制泳道索引：为圆点半径与右侧分割条预留空间，避免圆点被分割条遮挡。
fn graph_max_lane(width: f32) -> usize {
    let usable = width - GRAPH_LANE_START - 9.0;
    if usable < 0.0 {
        0
    } else {
        (usable / GRAPH_LANE_SPACING).floor() as usize
    }
}

fn render_commit_graph_cell(graph: CommitGraphRow, width: f32) -> impl IntoElement {
    // 可见泳道上限随列宽动态变化；超出可见范围的轨道不绘制，并以省略号提示。
    let visible_max = graph_max_lane(width);
    let overflow = graph
        .lanes
        .iter()
        .chain(graph.connectors.iter())
        .copied()
        .chain(std::iter::once(graph.lane))
        .any(|lane| lane > visible_max);

    div()
        .relative()
        .flex_none()
        .w(px(width))
        .h_full()
        .overflow_hidden()
        .child(
            gpui::canvas(
                |_, _, _| graph,
                move |bounds, graph, window, _cx| {
                    let top_y = bounds.origin.y;
                    let bottom_y = bounds.origin.y + bounds.size.height;
                    let center_y = bounds.origin.y + px(HISTORY_GRAPH_ROW_HEIGHT / 2.0);
                    let current_lane = graph.lane.min(visible_max);
                    let current_x = bounds.origin.x + px(graph_x(current_lane));

                    // 当前提交的轨道分段绘制，分支尖端的圆点上方不再出现悬空线段。
                    for lane in graph
                        .lanes
                        .iter()
                        .copied()
                        .filter(|lane| *lane <= visible_max && *lane != current_lane)
                    {
                        let x = bounds.origin.x + px(graph_x(lane));
                        paint_graph_line(window, x, top_y, x, bottom_y, graph_color(lane));
                    }

                    if graph.connected_from_top {
                        paint_graph_line(
                            window,
                            current_x,
                            top_y,
                            current_x,
                            center_y,
                            graph_color(current_lane),
                        );
                    }

                    for target in graph
                        .connectors
                        .iter()
                        .copied()
                        .filter(|lane| *lane <= visible_max)
                    {
                        let target_x = bounds.origin.x + px(graph_x(target));
                        paint_graph_line(
                            window,
                            current_x,
                            center_y,
                            target_x,
                            bottom_y,
                            graph_color(target),
                        );
                    }

                    paint_graph_dot(window, current_x, center_y, graph_color(current_lane));
                },
            )
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0)),
        )
        .when(overflow, |this| {
            this.child(
                div()
                    .absolute()
                    .right(px(4.0))
                    .top(px(15.0))
                    .text_size(px(10.0))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("..."),
            )
        })
}

fn graph_x(lane: usize) -> f32 {
    GRAPH_LANE_START + GRAPH_LANE_SPACING * lane as f32
}

fn graph_color(lane: usize) -> u32 {
    ui_theme::HISTORY_GRAPH_COLORS[lane % ui_theme::HISTORY_GRAPH_COLORS.len()]
}

fn paint_graph_line(
    window: &mut gpui::Window,
    x1: gpui::Pixels,
    y1: gpui::Pixels,
    x2: gpui::Pixels,
    y2: gpui::Pixels,
    color: u32,
) {
    let mut builder = PathBuilder::stroke(px(2.0));
    builder.move_to(point(x1, y1));
    builder.line_to(point(x2, y2));
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}

fn paint_graph_dot(window: &mut gpui::Window, x: gpui::Pixels, y: gpui::Pixels, color: u32) {
    let outer = px(5.0);
    let inner = px(4.0);
    paint_graph_circle(window, x, y, outer, ui_theme::CARD);
    paint_graph_circle(window, x, y, inner, color);
}

fn paint_graph_circle(
    window: &mut gpui::Window,
    x: gpui::Pixels,
    y: gpui::Pixels,
    radius: gpui::Pixels,
    color: u32,
) {
    let mut builder = PathBuilder::fill();
    builder.move_to(point(x - radius, y));
    builder.arc_to(
        point(radius, radius),
        px(0.0),
        false,
        true,
        point(x + radius, y),
    );
    builder.arc_to(
        point(radius, radius),
        px(0.0),
        false,
        true,
        point(x - radius, y),
    );
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}

/// 历史页文件路径过滤 chip：显示「文件：<basename>」+ ×，点击清除过滤；
/// 样式复用 history_scope_button（选中态配色），悬浮提示完整路径。
fn history_file_filter_chip(path: &str, cx: &mut Context<RepositoryView>) -> impl IntoElement {
    let label = format!("文件：{}", file_filter_label(path));
    let tooltip = path.to_string();
    div()
        .id("history-file-filter-chip")
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .max_w(px(220.0))
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(ui_theme::ACCENT))
        .bg(rgb(ui_theme::ACCENT))
        .text_size(px(11.0))
        .text_color(rgb(ui_theme::PRIMARY))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.set_history_file_filter(None);
            cx.notify();
        }))
        .tooltip(move |_window, cx| tooltip_text(tooltip.clone(), cx))
        .child(div().min_w(px(0.0)).truncate().child(label))
        .child(div().flex_none().child("×"))
}

/// 过滤 chip 的短标签：优先 basename，过长时截断加省略号。
fn file_filter_label(path: &str) -> String {
    let basename = path.rsplit('/').next().unwrap_or(path);
    if basename.chars().count() > 20 {
        let truncated: String = basename.chars().take(18).collect();
        format!("{truncated}…")
    } else {
        basename.to_string()
    }
}

/// 作者展示文本：`名 <邮箱>`，无邮箱时仅名称。
fn author_label(commit: &CommitInfo) -> String {
    match &commit.author_email {
        Some(email) => format!("{} <{}>", commit.author, email),
        None => commit.author.clone(),
    }
}

/// 提交者展示文本：仅当提交者与作者不同（rebase/cherry-pick 等）才有展示价值。
fn committer_note(commit: &CommitInfo) -> Option<String> {
    (commit.committer != commit.author).then(|| match &commit.committer_email {
        Some(email) => format!("{} <{}>", commit.committer, email),
        None => commit.committer.clone(),
    })
}

/// 父提交展示文本：根提交、普通提交、合并提交（双父）、章鱼合并（>2 父）。
fn parents_note(parents: &[String]) -> String {
    let short = |oid: &str| oid.chars().take(8).collect::<String>();
    match parents.len() {
        0 => "根提交（无父提交）".to_string(),
        1 => format!("父提交 {}", short(&parents[0])),
        2 => format!(
            "父提交 {} / {}（合并提交）",
            short(&parents[0]),
            short(&parents[1])
        ),
        count => format!("父提交 {count} 个（章鱼合并）"),
    }
}

fn commit_ref_labels(refs: &[CommitRefInfo], row_short_oid: &str) -> Vec<gpui::AnyElement> {
    refs.iter()
        .take(MAX_COMMIT_REF_LABELS)
        .cloned()
        .enumerate()
        .map(|(index, reference)| {
            commit_ref_label(row_short_oid, index, reference).into_any_element()
        })
        .collect()
}

fn commit_ref_label(
    row_short_oid: &str,
    index: usize,
    reference: CommitRefInfo,
) -> impl IntoElement {
    let tooltip = commit_ref_tooltip(&reference);
    let (bg, border, fg, label) = match reference.kind {
        CommitRefKind::LocalBranch => (
            ui_theme::REF_LOCAL_BG,
            ui_theme::REF_LOCAL_BORDER,
            ui_theme::REF_LOCAL_TEXT,
            reference.name,
        ),
        CommitRefKind::RemoteBranch => (
            ui_theme::REF_REMOTE_BG,
            ui_theme::REF_REMOTE_BORDER,
            ui_theme::REF_REMOTE_TEXT,
            reference.name,
        ),
        CommitRefKind::Tag => (
            ui_theme::REF_TAG_BG,
            ui_theme::REF_TAG_BORDER,
            ui_theme::REF_TAG_TEXT,
            reference.name,
        ),
        CommitRefKind::Head => (
            ui_theme::REF_HEAD_BG,
            ui_theme::REF_HEAD_BG,
            ui_theme::REF_HEAD_TEXT,
            reference.name,
        ),
    };

    div()
        .id(format!("commit-ref-{row_short_oid}-{index}"))
        .flex_none()
        .max_w(px(120.0))
        .px_1()
        .py(px(1.0))
        .rounded_sm()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_size(px(10.0))
        .text_color(rgb(fg))
        .truncate()
        .tooltip(move |_window, cx| tooltip_text(tooltip.clone(), cx))
        .child(label)
}

fn commit_ref_overflow_label(
    row_short_oid: &str,
    hidden_refs: Vec<CommitRefInfo>,
) -> impl IntoElement {
    let count = hidden_refs.len();
    let tooltip = hidden_commit_refs_tooltip(&hidden_refs);
    metric_badge(format!("+{count}"), ui_theme::PRIMARY)
        .id(format!("commit-ref-overflow-{row_short_oid}"))
        .tooltip(move |_window, cx| tooltip_text(tooltip.clone(), cx))
}

fn commit_ref_tooltip(reference: &CommitRefInfo) -> String {
    format!(
        "{}：{}",
        commit_ref_kind_label(&reference.kind),
        reference.name
    )
}

fn hidden_commit_refs_tooltip(refs: &[CommitRefInfo]) -> String {
    let mut heads = Vec::new();
    let mut local_branches = Vec::new();
    let mut remote_branches = Vec::new();
    let mut tags = Vec::new();

    for reference in refs {
        match reference.kind {
            CommitRefKind::Head => heads.push(reference.name.clone()),
            CommitRefKind::LocalBranch => local_branches.push(reference.name.clone()),
            CommitRefKind::RemoteBranch => remote_branches.push(reference.name.clone()),
            CommitRefKind::Tag => tags.push(reference.name.clone()),
        }
    }

    let mut parts = Vec::new();
    push_commit_ref_group(&mut parts, "HEAD", heads);
    push_commit_ref_group(&mut parts, "本地分支", local_branches);
    push_commit_ref_group(&mut parts, "远端分支", remote_branches);
    push_commit_ref_group(&mut parts, "标签", tags);

    format!("隐藏引用（{} 个）：{}", refs.len(), parts.join("；"))
}

fn push_commit_ref_group(parts: &mut Vec<String>, label: &'static str, names: Vec<String>) {
    if !names.is_empty() {
        parts.push(format!("{label}：{}", names.join("、")));
    }
}

fn commit_ref_kind_label(kind: &CommitRefKind) -> &'static str {
    match kind {
        CommitRefKind::LocalBranch => "本地分支",
        CommitRefKind::RemoteBranch => "远端分支",
        CommitRefKind::Tag => "标签",
        CommitRefKind::Head => "HEAD",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_commit(oid: &str, parents: &[&str]) -> CommitInfo {
        CommitInfo {
            oid: oid.to_string(),
            short_oid: oid.to_string(),
            summary: oid.to_string(),
            message: oid.to_string(),
            author: "测试作者".to_string(),
            author_email: Some("test@example.invalid".to_string()),
            committer: "测试作者".to_string(),
            committer_email: Some("test@example.invalid".to_string()),
            time: 0,
            parents: parents.iter().map(|parent| (*parent).to_string()).collect(),
            refs: Vec::new(),
        }
    }

    #[test]
    fn unmerged_branch_tips_do_not_connect_from_top() {
        let commits = vec![
            test_commit("feature-tip", &["base"]),
            test_commit("main-tip", &["base"]),
            test_commit("base", &[]),
        ];

        let rows = commit_graph_rows(&commits);

        assert!(!rows[0].connected_from_top);
        assert!(!rows[1].connected_from_top);
        assert!(rows[2].connected_from_top);
    }

    // 分叉的两个分支 tip 汇合到同一父提交时，后到的 tip 并入父提交已有泳道，
    // 自身泳道释放——否则父提交行之后会残留幽灵竖线贯穿到列表末尾。
    #[test]
    fn fork_rejoining_parent_releases_lane() {
        let commits = vec![
            test_commit("main-tip", &["base"]),
            test_commit("feature-tip", &["base"]),
            test_commit("base", &["root"]),
            test_commit("root", &[]),
        ];

        let rows = commit_graph_rows(&commits);

        // feature-tip 行并入 base 所在泳道 0，自身泳道在行内仍可见（画圆点）。
        assert!(rows[1].lanes.contains(&1));
        assert_eq!(rows[1].connectors, vec![0]);
        // base 行及之后：幽灵泳道不应残留。
        assert_eq!(rows[2].lanes, vec![0]);
        assert_eq!(rows[3].lanes, vec![0]);
    }

    // 合并提交的第二父提交尚未分页加载时，其泳道不应被剪掉：引入行画斜线但不画悬空顶部竖线，
    // 下一行该泳道作为贯穿竖线接续，保证线条连续。
    #[test]
    fn unloaded_parent_lane_stays_continuous() {
        let commits = vec![
            test_commit("merge", &["base", "missing"]),
            test_commit("base", &[]),
        ];

        let rows = commit_graph_rows(&commits);

        assert!(rows[0].connectors.contains(&1));
        assert!(!rows[0].lanes.contains(&1));
        assert!(rows[1].lanes.contains(&1));
    }

    // 可见泳道上限随列宽增长，过窄时回退到 0。
    #[test]
    fn graph_max_lane_scales_with_width() {
        assert_eq!(graph_max_lane(20.0), 0);
        assert_eq!(graph_max_lane(64.0), 3);
        assert_eq!(graph_max_lane(96.0), 5);
        assert_eq!(graph_max_lane(480.0), 32);
    }

    // 提交者与作者相同时不产生展示文本（避免详情区噪音）。
    #[test]
    fn committer_note_only_when_differs_from_author() {
        let mut commit = test_commit("abcd1234", &[]);
        commit.committer = "测试作者".to_string();
        assert_eq!(committer_note(&commit), None);

        commit.committer = "变基机器人".to_string();
        commit.committer_email = Some("bot@example.invalid".to_string());
        assert_eq!(
            committer_note(&commit),
            Some("变基机器人 <bot@example.invalid>".to_string())
        );
    }

    #[test]
    fn parents_note_covers_root_merge_and_octopus() {
        assert_eq!(parents_note(&[]), "根提交（无父提交）");
        assert_eq!(
            parents_note(&["aaaabbbbccccddddeeeeffff00001111".to_string()]),
            "父提交 aaaabbbb"
        );
        assert_eq!(
            parents_note(&[
                "aaaabbbbccccddddeeeeffff00001111".to_string(),
                "11112222333344445555666677778888".to_string()
            ]),
            "父提交 aaaabbbb / 11112222（合并提交）"
        );
        let octopus = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(parents_note(&octopus), "父提交 3 个（章鱼合并）");
    }

    #[test]
    fn author_label_includes_email_when_present() {
        let mut commit = test_commit("abcd1234", &[]);
        assert_eq!(author_label(&commit), "测试作者 <test@example.invalid>");

        commit.author_email = None;
        assert_eq!(author_label(&commit), "测试作者");
    }
}
