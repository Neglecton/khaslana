use std::collections::BTreeSet;

use crate::ui::theme::rgb;
use gpui::{
    Context, IntoElement, ListSizingBehavior, PathBuilder, div, point, prelude::*, px, uniform_list,
};
use khaslana::{CommitFileChange, CommitInfo, CommitRefInfo, CommitRefKind};

use crate::{
    CHANGE_ROW_HEIGHT, DiffHeaderTarget, EncodingMenuTarget, RepositoryView, ResizeTarget,
    ScrollbarMode, author_avatar, change_state_color, commit_time_label, history_scope_button,
    placeholder_row, scrollable_uniform_frame, section_header, section_header_action,
    ui::{
        components::{metric_badge, tooltip_text},
        theme as ui_theme,
    },
};

// 提交记录采用紧凑列宽；图形单元仍覆盖完整行高，保证相邻行的轨道连续。
const HISTORY_GRAPH_WIDTH: f32 = 88.0;
const HISTORY_GRAPH_ROW_HEIGHT: f32 = 36.0;
// 提交行只直接展示少量引用，剩余引用通过 +n 的悬浮提示查看，避免挤压提交摘要。
const MAX_COMMIT_REF_LABELS: usize = 3;
const GRAPH_LANE_START: f32 = 12.0;
const GRAPH_LANE_SPACING: f32 = 14.0;
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitGraphRow {
    lane: usize,
    lanes: Vec<usize>,
    connectors: Vec<usize>,
    connected_from_top: bool,
}

impl RepositoryView {
    pub(crate) fn render_history_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.render_commit_files(cx))
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
        let content = div()
            .id("commit-history-list")
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
            .child(render_commit_graph_cell(graph))
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
            .flex_none()
            .w(px(self.history_files_width))
            .min_w(px(self.history_files_width))
            .min_h(px(0.0))
            .h_full()
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
        let path_label = file
            .old_path
            .as_ref()
            .filter(|old_path| old_path.as_str() != file.path.as_str())
            .map(|old_path| format!("{old_path} -> {}", file.path))
            .unwrap_or_else(|| file.path.clone());
        let state = file.status.label();
        let state_color = change_state_color(&file.status);

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
            .child(
                div()
                    .flex_none()
                    .w(px(24.0))
                    .text_size(px(11.0))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(state_color))
                    .child(state),
            )
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
    let loaded_oids = commits
        .iter()
        .map(|commit| commit.oid.as_str())
        .collect::<BTreeSet<_>>();
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
            lanes[lane] = Some(first_parent.clone());
            connectors.push(lane);
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

        for lane in lanes.iter_mut() {
            if let Some(oid) = lane.as_ref()
                && !loaded_oids.contains(oid.as_str())
            {
                *lane = None;
            }
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

fn render_commit_graph_cell(graph: CommitGraphRow) -> impl IntoElement {
    let max_lane = graph
        .lanes
        .iter()
        .chain(graph.connectors.iter())
        .copied()
        .max()
        .unwrap_or(graph.lane)
        .min(5);

    div()
        .relative()
        .flex_none()
        .w(px(HISTORY_GRAPH_WIDTH))
        .h_full()
        .overflow_hidden()
        .child(
            gpui::canvas(
                |_, _, _| graph,
                |bounds, graph, window, _cx| {
                    let top_y = bounds.origin.y;
                    let bottom_y = bounds.origin.y + bounds.size.height;
                    let center_y = bounds.origin.y + px(HISTORY_GRAPH_ROW_HEIGHT / 2.0);
                    let current_lane = graph.lane.min(5);
                    let current_x = bounds.origin.x + px(graph_x(current_lane));

                    // 当前提交的轨道分段绘制，分支尖端的圆点上方不再出现悬空线段。
                    for lane in graph
                        .lanes
                        .iter()
                        .copied()
                        .filter(|lane| *lane <= 5 && *lane != current_lane)
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

                    for target in graph.connectors.iter().copied().filter(|lane| *lane <= 5) {
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
        .when(max_lane >= 5, |this| {
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
            author: "测试作者".to_string(),
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
}
