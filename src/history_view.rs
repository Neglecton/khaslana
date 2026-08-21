use crate::ui::theme::rgb;
use gpui::{
    Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, div, prelude::*, px,
    uniform_list,
};
use khaslana::{CommitFileChange, CommitInfo, CommitRefInfo, CommitRefKind};

use crate::{
    CHANGE_ROW_HEIGHT, DEFAULT_HISTORY_DETAILS_HEIGHT, DiffHeaderTarget, EncodingMenuTarget,
    RepositoryView, ResizeTarget, ScrollbarMode, author_avatar, change_state_badge,
    commit_time_label, history_scope_button, placeholder_row, scrollable_frame_when,
    scrollable_uniform_frame, section_header, section_header_action,
    ui::{
        components::{list_row_surface, metric_badge, tooltip_text},
        theme as ui_theme,
    },
};

// History 提交项含摘要/ref 与作者/avatar 两层信息，使用专用 48px 行高。
const HISTORY_COMMIT_ROW_HEIGHT: f32 = 48.0;
// 主历史页导航列较窄：行内最多展示 1 个引用标签（HEAD/首个本地分支优先），
// 其余收进「+n」徽标悬浮查看，避免标签挤压提交摘要；完整拓扑在图谱页查看。
const MAX_COMMIT_REF_LABELS: usize = 1;
// 检查器下部的文件导航保持窄而稳定，差异视图始终取得剩余空间。
const HISTORY_INSPECTOR_COLLAPSED_DETAILS_HEIGHT: f32 = 32.0;

/// History Inspector 的纯布局策略。提交导航与检查器内「提交文件 | 差异」分栏
/// 均为可拖拽宽度（分别由 `HistoryFiles` / `HistoryInspectorFiles` 分割条驱动）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct HistoryInspectorLayout {
    navigator_width: f32,
    details_height: f32,
    file_list_width: f32,
}

fn history_inspector_layout(
    navigator_width: f32,
    inspector_files_width: f32,
    details_height: Option<f32>,
    details_collapsed: bool,
) -> HistoryInspectorLayout {
    HistoryInspectorLayout {
        navigator_width,
        details_height: if details_collapsed {
            HISTORY_INSPECTOR_COLLAPSED_DETAILS_HEIGHT
        } else {
            details_height.unwrap_or(DEFAULT_HISTORY_DETAILS_HEIGHT)
        },
        file_list_width: inspector_files_width.clamp(
            crate::MIN_HISTORY_INSPECTOR_FILES_WIDTH,
            crate::MAX_HISTORY_INSPECTOR_FILES_WIDTH,
        ),
    }
}

impl RepositoryView {
    pub(crate) fn render_history_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = history_inspector_layout(
            self.history_files_width,
            self.history_inspector_files_width,
            self.history_details_height,
            self.history_details_collapsed,
        );

        // Focus Workbench 的 History Inspector：提交导航全高固定在左，右侧检查器
        // 将提交概览与文件/差异拆成稳定的两层，避免原先上下三区互相挤压。
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
            .child(self.render_commit_history(layout.navigator_width, cx))
            .child(self.render_column_splitter(ResizeTarget::HistoryFiles, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    // 透明标记记录检查器顶端；保留 HistoryDetails 的默认对半分
                    // 推导和现有拖拽状态模型，不改异步或选择状态。
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
                    .child(self.render_commit_details(layout.details_height, cx))
                    .when(!self.history_details_collapsed, |this| {
                        this.child(self.render_column_splitter(ResizeTarget::HistoryDetails, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(self.render_commit_files(layout.file_list_width, cx))
                            // 检查器内「提交文件 | 差异」分栏同样可拖拽（双击复位默认窄栏）。
                            .child(
                                self.render_column_splitter(
                                    ResizeTarget::HistoryInspectorFiles,
                                    cx,
                                ),
                            )
                            .child(self.render_history_diff(cx)),
                    ),
            )
    }

    fn render_commit_history(
        &self,
        navigator_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                                        .h(px(HISTORY_COMMIT_ROW_HEIGHT))
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
                                this.commit_row(commit, cx).into_any_element()
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
            .w(px(navigator_width))
            .min_w(px(navigator_width))
            .h_full()
            .min_h(px(0.0))
            .child(section_header_action(
                format!("提交记录（{}）", self.history_scope.label()),
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        // 「图谱」入口：跳转到提交图谱页（泳道拓扑、分支动向高亮、搜索）；
                        // 也是从提交记录页返回图谱页（跳转后状态无损保留）的通道。
                        .child(history_scope_button(
                            "图谱",
                            false,
                            |this| this.open_commit_graph(),
                            cx,
                        ))
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
    }

    fn commit_row(&self, commit: CommitInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.history_selected_commit.as_deref() == Some(commit.oid.as_str());
        let oid = commit.oid.clone();
        let right_click_oid = commit.oid.clone();
        let right_click_short_oid = commit.short_oid.clone();
        let right_click_summary = commit.summary.clone();
        let right_click_parent_count = commit.parents.len();
        let row_short_oid = commit.short_oid.clone();
        let unpushed = self
            .branch_sync_status
            .as_ref()
            .is_some_and(|status| status.unpushed_oids.iter().any(|oid| oid == &commit.oid));

        // 提交导航使用统一的平面列表面：选中态由淡主色背景与 2px 指示条表达，
        // 不再为每一行绘制卡片边框。主历史页不画泳道（完整拓扑在图谱页）。
        list_row_surface(format!("commit-{row_short_oid}"), selected)
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_1()
            // 左内边距与选中指示条（左缘 2px）及未推送竖条错开；泳道移除后
            // 文字不再有泳道格子垫底，必须显式留出这段距离。
            .pl(px(ui_theme::SPACE_3))
            .pr_2()
            .h(px(HISTORY_COMMIT_ROW_HEIGHT))
            .cursor_pointer()
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
                        .bg(rgb(ui_theme::FEEDBACK_WARNING_BORDER)),
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
            .child(commit_row_content(
                &commit,
                MAX_COMMIT_REF_LABELS,
                unpushed,
                false,
            ))
    }

    /// 检查器顶部的提交概览与详情。完整信息留在可滚动区域，保证文件和差异始终可见。
    fn render_commit_details(
        &self,
        details_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(commit) = self
            .history_selected_commit
            .as_deref()
            .and_then(|oid| self.history_commits.iter().find(|info| info.oid == oid))
            .cloned()
        else {
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(details_height))
                .min_h(px(HISTORY_INSPECTOR_COLLAPSED_DETAILS_HEIGHT))
                .child(section_header("提交详情"))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("选择一个提交以检查详情、文件与差异"),
                )
                .into_any_element();
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
                        // 折叠状态是持久偏好，切换后立即落库（重启恢复）。
                        this.save_layout_preferences();
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
                .h(px(details_height))
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
                    .children(commit_ref_labels(
                        &commit.refs,
                        &commit.short_oid,
                        crate::commit_graph_view::GRAPH_REF_LABEL_CAP,
                    )),
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

        // 保留 HistoryDetails 的既有绝对高度/双击复位语义；默认高度由 Inspector
        // 策略提供，避免右侧详情区随窗口无限扩张。
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(details_height))
            .min_h(px(HISTORY_INSPECTOR_COLLAPSED_DETAILS_HEIGHT))
            .child(header)
            .child(content)
            .into_any_element()
    }

    fn render_commit_files(
        &self,
        file_list_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .flex_none()
            .w(px(file_list_width))
            .min_w(px(file_list_width))
            .min_h(px(0.0))
            .border_r_1()
            .border_color(rgb(ui_theme::BORDER))
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
            .cursor_pointer()
            .overflow_hidden()
            // 文件导航采用平面选中态，不再为每行绘制卡片边框。
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else {
                rgb(ui_theme::CARD)
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

/// 提交行两行内容（摘要 + 引用标签 / 作者 + 时间 + 短 SHA + 未推送徽标）。
/// 主历史页与图谱页共用；差异由调用方决定：行内引用标签数量上限
/// （主页面 1 个、图谱页 3 个）与淡化透明度（图谱页高亮谱系外的行）。
pub(crate) fn commit_row_content(
    commit: &CommitInfo,
    badge_cap: usize,
    unpushed: bool,
    dimmed: bool,
) -> impl IntoElement {
    let row_short_oid = commit.short_oid.clone();
    let ref_labels = commit_ref_labels(&commit.refs, &row_short_oid, badge_cap);
    let hidden_refs = commit
        .refs
        .iter()
        .skip(badge_cap)
        .cloned()
        .collect::<Vec<_>>();
    let hidden_ref_count = hidden_refs.len();
    let author = commit.author.clone();
    let time = commit_time_label(commit.time);

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(1.0))
        .when(dimmed, |this| this.opacity(0.55))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(ui_theme::FOREGROUND))
                        .truncate()
                        .child(commit.summary.clone()),
                )
                .children(ref_labels)
                .when(hidden_ref_count > 0, |this| {
                    this.child(commit_ref_overflow_label(&row_short_oid, hidden_refs))
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .min_w(px(0.0))
                .text_size(px(10.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(author_avatar(&author))
                .child(div().max_w(px(76.0)).truncate().child(author))
                .child(div().flex_none().child(time))
                .child(
                    div()
                        .flex_none()
                        .font_family("Consolas, monospace")
                        .text_color(rgb(ui_theme::PRIMARY))
                        .child(row_short_oid.clone()),
                )
                .when(unpushed, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
                            .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                            .child("未推送"),
                    )
                }),
        )
}

/// 历史页文件路径过滤 chip：显示「文件：<basename>」+ ×，点击清除过滤；
/// 样式复用 history_scope_button（选中态配色），悬浮提示完整路径。
pub(crate) fn history_file_filter_chip(
    path: &str,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
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

/// 作者展示文本：`名 <邮箱>`，无邮箱时仅名称。主历史页详情区与图谱页详情卡共用。
pub(crate) fn author_label(commit: &CommitInfo) -> String {
    match &commit.author_email {
        Some(email) => format!("{} <{}>", commit.author, email),
        None => commit.author.clone(),
    }
}

/// 提交者展示文本：仅当提交者与作者不同（rebase/cherry-pick 等）才有展示价值。
pub(crate) fn committer_note(commit: &CommitInfo) -> Option<String> {
    (commit.committer != commit.author).then(|| match &commit.committer_email {
        Some(email) => format!("{} <{}>", commit.committer, email),
        None => commit.committer.clone(),
    })
}

/// 父提交展示文本：根提交、普通提交、合并提交（双父）、章鱼合并（>2 父）。
pub(crate) fn parents_note(parents: &[String]) -> String {
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

pub(crate) fn commit_ref_labels(
    refs: &[CommitRefInfo],
    row_short_oid: &str,
    cap: usize,
) -> Vec<gpui::AnyElement> {
    refs.iter()
        .take(cap)
        .cloned()
        .enumerate()
        .map(|(index, reference)| {
            commit_ref_label(row_short_oid, index, reference).into_any_element()
        })
        .collect()
}

/// 单个引用标签徽标（图谱页详情卡全量展示时也复用）。
pub(crate) fn commit_ref_label(
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
#[path = "tests/history_view.rs"]
mod tests;
