// 提交图谱页（MainMode::CommitGraph）：拓扑专注型的独立页面。
//
// 职责分工：主历史页负责「解剖单个提交」（四象限检查器），本页负责
// 「看提交之间的关系」——全宽泳道列表 + 分支动向高亮（全谱系/仅领先 HEAD
// 两档）+ 合并提交淡化 + 搜索过滤；底部轻量详情卡提供「在提交记录页查看」
// 跳转（跳转后主页面四象限直接就位），返回本页时工具行开关、搜索词与
// 滚动位置全部保留（专用模式切换不重置状态 + 持久滚动句柄注册表）。
//
// 泳道算法与画布渲染自 history_view.rs 迁入：主历史页已去掉泳道列，
// 本模块是泳道唯一的使用方。

use std::sync::Arc;

use gpui::{
    Context, CursorStyle, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, div, point, prelude::*, px, uniform_list,
};
use khaslana::{BranchInfo, BranchKind, CommitInfo};

use crate::{
    RepositoryView, ResizeTarget, ScrollbarMode, column_splitter_accepts_mouse_events,
    column_splitter_should_clear_resize, commit_time_label, history_scope_button,
    history_view::{
        author_label, commit_ref_labels, commit_row_content, committer_note, parents_note,
    },
    menu_separator, placeholder_row, scrollable_frame_when, scrollable_uniform_frame,
    section_header_action,
    sidebar_view::sidebar_branch_matches_normalized_query,
    ui::{
        components::{
            command_group, glass_menu, list_row_surface, page_header, segmented_button,
            tooltip_text,
        },
        theme::{self as ui_theme, rgb},
    },
};

/// 图谱页提交行高：与主历史页 48px 两行结构保持一致（泳道跨行连续）。
pub(crate) const COMMIT_GRAPH_ROW_HEIGHT: f32 = 48.0;
/// 图谱页行内引用标签上限：比主历史页（1 个）宽松，完整列表在详情卡全量展示。
pub(crate) const GRAPH_REF_LABEL_CAP: usize = 3;
const GRAPH_LANE_START: f32 = 12.0;
const GRAPH_LANE_SPACING: f32 = 14.0;
// 图形列右侧的拖拽分割条宽度，行内流式排布，自动与图形列对齐。
const GRAPH_SPLITTER_WIDTH: f32 = 6.0;
/// 淡化行的泳道线透明度（高亮谱系外的行 / 被淡化开关命中的合并提交）。
const GRAPH_DIM_ALPHA: f32 = 0.35;
/// 详情卡展开高度：完整提交信息内部滚动，不与泳道列表争抢空间。
const COMMIT_GRAPH_DETAILS_HEIGHT: f32 = 200.0;
const COMMIT_GRAPH_LIST_SCROLL_ID: &str = "commit-graph-list";
const COMMIT_GRAPH_BRANCH_MENU_SCROLL_ID: &str = "commit-graph-branch-menu-scroll";

/// 泳道行数据：某一行提交的轨道几何（自 history_view.rs 迁入，语义不变）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CommitGraphRow {
    pub(crate) lane: usize,
    pub(crate) lanes: Vec<usize>,
    pub(crate) connectors: Vec<usize>,
    pub(crate) connected_from_top: bool,
}

/// 图谱行淡化判定（纯函数）：
/// - 高亮谱系激活时：命中谱系的行保持正常（其中的合并提交正是分支吸收
///   其他线索的位置，不淡化），谱系外的行一律淡化（合并与否不影响）；
/// - 未激活高亮时：「淡化合并提交」开关命中多父提交才淡化。
pub(crate) fn graph_row_dimmed(in_trace: Option<bool>, is_merge: bool, dim_merges: bool) -> bool {
    match in_trace {
        Some(_) => in_trace == Some(false),
        None => dim_merges && is_merge,
    }
}

/// 按子串过滤已加载提交（大小写不敏感；匹配摘要/作者/短 SHA）。
/// 返回命中提交在原列表中的索引（保持顺序）；空查询返回全量索引。
/// 只过滤已加载页，配合「加载更多」逐步扩大搜索范围。
pub(crate) fn filter_graph_commits(commits: &[CommitInfo], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return (0..commits.len()).collect();
    }
    commits
        .iter()
        .enumerate()
        .filter(|(_, commit)| {
            commit.summary.to_lowercase().contains(&needle)
                || commit.author.to_lowercase().contains(&needle)
                || commit.short_oid.to_lowercase().contains(&needle)
        })
        .map(|(index, _)| index)
        .collect()
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
pub(crate) fn graph_max_lane(width: f32) -> usize {
    let usable = width - GRAPH_LANE_START - 9.0;
    if usable < 0.0 {
        0
    } else {
        (usable / GRAPH_LANE_SPACING).floor() as usize
    }
}

fn graph_x(lane: usize) -> f32 {
    GRAPH_LANE_START + GRAPH_LANE_SPACING * lane as f32
}

fn graph_color(lane: usize) -> gpui::Rgba {
    rgb(ui_theme::HISTORY_GRAPH_COLORS[lane % ui_theme::HISTORY_GRAPH_COLORS.len()])
}

/// 淡化泳道色：解析为当前主题色后叠加透明度。打包为 RRGGBBAA 再走
/// 主题感知 rgba 入口（字面颜色原样透传），不能把 packed 值传给 rgb()。
fn graph_dim_color(lane: usize) -> gpui::Rgba {
    let resolved = ui_theme::resolve_color(
        ui_theme::HISTORY_GRAPH_COLORS[lane % ui_theme::HISTORY_GRAPH_COLORS.len()],
    );
    let alpha = (GRAPH_DIM_ALPHA.clamp(0.0, 1.0) * 255.0).round() as u32;
    ui_theme::rgba((resolved << 8) | alpha)
}

fn render_commit_graph_cell(graph: CommitGraphRow, width: f32, dimmed: bool) -> impl IntoElement {
    // 可见泳道上限随列宽动态变化；超出可见范围的轨道不绘制，并以省略号提示。
    let visible_max = graph_max_lane(width);
    let overflow = graph
        .lanes
        .iter()
        .chain(graph.connectors.iter())
        .copied()
        .chain(std::iter::once(graph.lane))
        .any(|lane| lane > visible_max);
    let line_color = move |lane: usize| {
        if dimmed {
            graph_dim_color(lane)
        } else {
            graph_color(lane)
        }
    };

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
                    let center_y = bounds.origin.y + bounds.size.height / 2.0;
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
                        paint_graph_line(window, x, top_y, x, bottom_y, line_color(lane));
                    }

                    if graph.connected_from_top {
                        paint_graph_line(
                            window,
                            current_x,
                            top_y,
                            current_x,
                            center_y,
                            line_color(current_lane),
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
                            line_color(target),
                        );
                    }

                    paint_graph_dot(window, current_x, center_y, line_color(current_lane));
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
                    .top(px(0.0))
                    .right(px(4.0))
                    .bottom(px(0.0))
                    .flex()
                    .items_center()
                    .text_size(px(10.0))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("..."),
            )
        })
}

fn paint_graph_line(
    window: &mut gpui::Window,
    x1: gpui::Pixels,
    y1: gpui::Pixels,
    x2: gpui::Pixels,
    y2: gpui::Pixels,
    color: gpui::Rgba,
) {
    let mut builder = PathBuilder::stroke(px(2.0));
    builder.move_to(point(x1, y1));
    builder.line_to(point(x2, y2));
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_graph_dot(window: &mut gpui::Window, x: gpui::Pixels, y: gpui::Pixels, color: gpui::Rgba) {
    let outer = px(5.0);
    let inner = px(4.0);
    paint_graph_circle(window, x, y, outer, rgb(ui_theme::CARD));
    paint_graph_circle(window, x, y, inner, color);
}

fn paint_graph_circle(
    window: &mut gpui::Window,
    x: gpui::Pixels,
    y: gpui::Pixels,
    radius: gpui::Pixels,
    color: gpui::Rgba,
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
        window.paint_path(path, color);
    }
}

/// 分支高亮下拉的分组过滤（纯函数）：按查询词把 snapshot.branches 拆成
/// 本地/远端两组（匹配规则与侧边栏分支搜索一致：名字或 upstream 子串、
/// ASCII 无分配 + Unicode 回退；空查询返回全量）。远端条目名含 `origin/` 前缀。
pub(crate) fn commit_graph_branch_menu_groups<'a>(
    branches: &'a [BranchInfo],
    query: &str,
) -> (Vec<&'a BranchInfo>, Vec<&'a BranchInfo>) {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for branch in branches {
        if !sidebar_branch_matches_normalized_query(branch, query) {
            continue;
        }
        match branch.kind {
            BranchKind::Local => local.push(branch),
            BranchKind::Remote => remote.push(branch),
        }
    }
    (local, remote)
}

/// 下拉内的分组标题（本地分支 / 远端分支）。
fn commit_graph_branch_group_label(label: &'static str) -> gpui::AnyElement {
    div()
        .id(format!("commit-graph-branch-group-{label}"))
        .px_3()
        .pt(px(6.0))
        .pb(px(2.0))
        .text_size(px(10.0))
        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
        .child(label)
        .into_any_element()
}

/// 真分段控件的选项：无独立边框（外框由分组容器提供），选中项主色底 + 加粗，
/// 未选中项仅 hover 反馈。与相邻选项间由容器插入 1px 分隔线。
fn scope_segment_option(
    id: &'static str,
    label: &'static str,
    selected: bool,
    action: impl Fn(&mut RepositoryView) + 'static,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .min_h(px(28.0))
        .px_3()
        .text_size(px(12.0))
        .font_weight(if selected {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(rgb(if selected {
            ui_theme::PRIMARY
        } else {
            ui_theme::MUTED_FOREGROUND
        }))
        .bg(rgb(if selected {
            ui_theme::ACCENT
        } else {
            ui_theme::CARD
        }))
        .cursor_pointer()
        .when(!selected, |this| {
            this.hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            action(this);
            cx.notify();
        }))
        .child(label)
}

impl RepositoryView {
    pub(crate) fn render_commit_graph_view(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let search_query = self.commit_graph_search.value.trim().to_string();
        let search_active = !search_query.is_empty();
        // 搜索或文件过滤激活时隐藏泳道列（过滤后中间提交缺失，泳道线会断裂）。
        let graph_visible = !search_active && self.history_file_filter.is_none();

        div()
            .relative()
            .id("commit-graph-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
            .child(page_header("提交图谱", Some("分支拓扑与动向追踪")).child(
                command_group().child(self.button(
                    "关闭",
                    true,
                    |this, _window, _cx| {
                        this.close_commit_graph();
                    },
                    cx,
                )),
            ))
            .child(self.render_commit_graph_toolbar(window, cx))
            .child(self.render_commit_graph_list(search_query, cx))
            .child(self.render_commit_graph_details_card(cx))
            .when(self.commit_graph.branch_menu_open, |this| {
                this.child(self.render_commit_graph_branch_menu(window, cx))
            })
            .when(
                self.resize_state(ResizeTarget::HistoryGraph).is_some() && graph_visible,
                |this| this.child(self.history_graph_resize_overlay(cx)),
            )
    }

    /// 单行工具行（用户要求不浪费纵向空间）：分支动向追踪组（高亮下拉、仅领先
    /// HEAD、淡化合并提交）+ 列表范围（互斥分段控件）+ 文件过滤 chip + 搜索框。
    /// 高亮下拉放行首，弹出菜单锚定左缘即对齐触发器。
    fn render_commit_graph_toolbar(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let highlight_branch = self.commit_graph.highlight_branch.clone();
        let highlight_active = highlight_branch.is_some();
        let trigger_label = match &highlight_branch {
            Some(branch) => format!("高亮：{branch} ▾"),
            None => "分支高亮 ▾".to_string(),
        };
        let trigger_tooltip = if self.commit_graph.trace_loading {
            "正在计算分支谱系...".to_string()
        } else {
            "选择要追踪动向的本地分支；谱系内提交保持高亮，其余淡化".to_string()
        };
        let truncated_hint = self
            .commit_graph
            .trace
            .as_ref()
            .filter(|trace| trace.truncated)
            .map(|_| format!("已高亮前 {} 个提交", khaslana::git::COMMIT_TRACE_OID_LIMIT));

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px(px(ui_theme::SPACE_3))
            .py(px(ui_theme::SPACE_2))
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(
                div()
                    .id("commit-graph-scope-group")
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(28.0))
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .overflow_hidden()
                    .bg(rgb(ui_theme::CARD))
                    // 真分段控件：共享外框与圆角，内部选项无独立边框、
                    // 以 1px 分隔线区隔，选中项主色底——同一时刻只有一个
                    // 选中态（history_scope 单值），互斥且是一个视觉整体。
                    .child(scope_segment_option(
                        "commit-graph-scope-current",
                        "当前分支",
                        self.history_scope == khaslana::HistoryScope::CurrentBranch,
                        |this| this.set_history_scope(khaslana::HistoryScope::CurrentBranch),
                        cx,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .w(px(1.0))
                            .h_full()
                            .bg(rgb(ui_theme::BORDER)),
                    )
                    .child(scope_segment_option(
                        "commit-graph-scope-all",
                        "所有分支",
                        self.history_scope == khaslana::HistoryScope::AllRefs,
                        |this| this.set_history_scope(khaslana::HistoryScope::AllRefs),
                        cx,
                    )),
            )
            // 分支高亮下拉触发器（动态标签，无法走静态标签 helper）
            .child(
                div()
                    .id("commit-graph-branch-trigger")
                    .flex_none()
                    .flex()
                    .items_center()
                    .min_h(px(28.0))
                    .px_2()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(if highlight_active {
                        ui_theme::ACCENT
                    } else {
                        ui_theme::BORDER
                    }))
                    .bg(rgb(if highlight_active {
                        ui_theme::ACCENT
                    } else {
                        ui_theme::CARD
                    }))
                    .text_size(px(12.0))
                    .text_color(rgb(if highlight_active {
                        ui_theme::PRIMARY
                    } else {
                        ui_theme::MUTED_FOREGROUND
                    }))
                    .max_w(px(220.0))
                    .min_w(px(0.0))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.toggle_commit_graph_branch_menu(window);
                            cx.notify();
                        }),
                    )
                    .tooltip(move |_window, cx| tooltip_text(trigger_tooltip.clone(), cx))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(trigger_label),
                    ),
            )
            // 仅领先 HEAD：未选择分支高亮时禁用，悬浮说明启用前提
            .child(
                segmented_button(
                    "commit-graph-ahead-only".to_string(),
                    highlight_active && self.commit_graph.highlight_ahead_only,
                    highlight_active,
                )
                .when(!highlight_active, |this| {
                    this.tooltip(move |_window, cx| tooltip_text("先选择分支高亮后可用", cx))
                })
                .when(highlight_active, |this| {
                    this.on_click(cx.listener(|this, _event, _window, cx| {
                        this.toggle_commit_graph_ahead_only();
                        cx.notify();
                    }))
                })
                .child("仅领先 HEAD"),
            )
            .child(
                segmented_button(
                    "commit-graph-dim-merges".to_string(),
                    self.commit_graph.dim_merges,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_commit_graph_dim_merges();
                    cx.notify();
                }))
                .child("淡化合并提交"),
            )
            .when_some(truncated_hint, |this, hint| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(hint),
                )
            })
            .when(self.commit_graph.trace_loading, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("谱系计算中..."),
                )
            })
            .child(div().flex_1().min_w(px(0.0)))
            // 文件路径过滤 chip（与主历史页共用同一份过滤状态）
            .children(
                self.history_file_filter
                    .as_deref()
                    .map(|path| crate::history_view::history_file_filter_chip(path, cx)),
            )
            .child(div().flex_none().w(px(220.0)).child(self.input(
                crate::FieldId::CommitGraphSearch,
                true,
                window,
                cx,
            )))
    }

    /// 分支高亮下拉菜单（glass_menu，锚定在工具行下方）。
    /// 分支高亮下拉：顶部搜索框（打开即聚焦）+ 本地/远端分组列表 + 底部「关闭高亮」。
    fn render_commit_graph_branch_menu(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let branches = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.branches.as_slice())
            .unwrap_or(&[]);
        let query = self.commit_graph_branch_search.value.trim().to_lowercase();
        let (local_branches, remote_branches) = commit_graph_branch_menu_groups(branches, &query);
        let highlight_branch = self.commit_graph.highlight_branch.clone();
        let remote_loading =
            self.loading.remote() && remote_branches.is_empty() && query.is_empty();
        let scroll_handle = self.scroll_handle(COMMIT_GRAPH_BRANCH_MENU_SCROLL_ID);

        // 先物化条目再挂载：闭包内同时借用 self 与 cx 会引发逃逸借用。
        let mut list_children: Vec<gpui::AnyElement> = Vec::new();
        if !local_branches.is_empty() || !remote_branches.is_empty() || !remote_loading {
            list_children.push(commit_graph_branch_group_label("本地分支"));
            for branch in &local_branches {
                let selected = highlight_branch.as_deref() == Some(branch.name.as_str());
                list_children.push(self.commit_graph_branch_menu_item(
                    &branch.name,
                    selected,
                    false,
                    cx,
                ));
            }
            if !remote_branches.is_empty() || remote_loading {
                list_children.push(commit_graph_branch_group_label("远端分支"));
                for branch in &remote_branches {
                    let selected = highlight_branch.as_deref() == Some(branch.name.as_str());
                    list_children.push(self.commit_graph_branch_menu_item(
                        &branch.name,
                        selected,
                        true,
                        cx,
                    ));
                }
                if remote_loading {
                    list_children.push(
                        div()
                            .id("commit-graph-branch-remote-loading")
                            .px_3()
                            .py_1()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("远端分支加载中...")
                            .into_any_element(),
                    );
                }
            }
        }
        if list_children.is_empty() {
            list_children.push(
                div()
                    .id("commit-graph-branch-empty")
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("没有匹配的分支")
                    .into_any_element(),
            );
        }

        glass_menu()
            .absolute()
            // 锚定在工具行（单行）触发器正下方：页头 40 + 工具行 44。
            .top(px(84.0))
            .left(px(ui_theme::SPACE_3))
            .w(px(260.0))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            // 搜索框：打开菜单即聚焦，输入即过滤（本地/远端两组共用）。
            .child(
                div()
                    .id("commit-graph-branch-search-row")
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .child(div().flex_1().min_w(px(0.0)).child(self.input(
                        crate::FieldId::CommitGraphBranchSearch,
                        true,
                        window,
                        cx,
                    ))),
            )
            .child(menu_separator())
            .child(
                div()
                    .id(COMMIT_GRAPH_BRANCH_MENU_SCROLL_ID)
                    .max_h(px(320.0))
                    .overflow_y_scroll()
                    .track_scroll(&scroll_handle)
                    .children(list_children),
            )
            .child(menu_separator())
            // 「关闭高亮」固定底部，不受搜索过滤影响。
            .child(self.commit_graph_branch_menu_item(
                "off",
                self.commit_graph.highlight_branch.is_none(),
                false,
                cx,
            ))
            .into_any_element()
    }

    fn commit_graph_branch_menu_item(
        &self,
        branch: &str,
        selected: bool,
        remote: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // "off" 是关闭高亮的哨兵值，与任何真实分支名（含 "/"）不冲突。
        let is_off = branch == "off";
        let label = if is_off {
            "关闭高亮".to_string()
        } else if selected {
            format!("✓ {branch}")
        } else {
            branch.to_string()
        };
        let id = if is_off {
            "commit-graph-branch-off".to_string()
        } else if remote {
            format!("commit-graph-branch-remote-{branch}")
        } else {
            format!("commit-graph-branch-local-{branch}")
        };
        let branch_for_click = (!is_off).then(|| branch.to_string());
        div()
            .id(id)
            .px_3()
            .py_1()
            .text_size(px(12.0))
            // 远端分支名（origin/…）用次要色与本地分支区分（选中仍以主色突出）。
            .text_color(rgb(if selected {
                ui_theme::PRIMARY
            } else if remote {
                ui_theme::CONTENT_SECONDARY
            } else {
                ui_theme::FOREGROUND
            }))
            .bg(rgb(if selected {
                ui_theme::PRIMARY_SUBTLE
            } else {
                ui_theme::CARD
            }))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.set_commit_graph_highlight(branch_for_click.clone());
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    fn render_commit_graph_list(
        &self,
        search_query: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 搜索只过滤已加载页；索引映射保留原顺序，泳道数据按原索引对齐。
        let indices = Arc::new(filter_graph_commits(&self.history_commits, &search_query));
        let search_active = !search_query.trim().is_empty();
        let row_count = if indices.is_empty() {
            1
        } else if self.history_refreshing {
            indices.len()
        } else {
            indices.len() + usize::from(self.history_has_more)
        };
        let content_present = !indices.is_empty();
        let handle = self.uniform_scroll_handle(COMMIT_GRAPH_LIST_SCROLL_ID);
        let list_handle = handle.clone();
        let indices_for_rows = Arc::clone(&indices);

        let content = div()
            .id(COMMIT_GRAPH_LIST_SCROLL_ID)
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
                    COMMIT_GRAPH_LIST_SCROLL_ID,
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|row| {
                                if indices_for_rows.is_empty() {
                                    let placeholder = if this.history_loading.commits {
                                        "提交记录加载中..."
                                    } else if this.repo_path.is_none() {
                                        "请先打开一个仓库"
                                    } else {
                                        "没有匹配的提交"
                                    };
                                    return placeholder_row(placeholder).into_any_element();
                                }
                                if row == indices_for_rows.len() {
                                    if this.history_refreshing {
                                        return placeholder_row("").into_any_element();
                                    }
                                    return div()
                                        .flex_none()
                                        .w_full()
                                        .min_w(px(0.0))
                                        .h(px(COMMIT_GRAPH_ROW_HEIGHT))
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
                                let Some(&commit_index) = indices_for_rows.get(row) else {
                                    return placeholder_row("").into_any_element();
                                };
                                let Some(commit) = this.history_commits.get(commit_index).cloned()
                                else {
                                    return placeholder_row("").into_any_element();
                                };
                                // 搜索或文件过滤激活时泳道隐藏（断线规避）。
                                let graph_visible =
                                    !search_active && this.history_file_filter.is_none();
                                let graph = graph_visible.then(|| {
                                    this.history_graph_rows
                                        .get(commit_index)
                                        .cloned()
                                        .unwrap_or_default()
                                });
                                let in_trace = this
                                    .commit_graph
                                    .trace
                                    .as_ref()
                                    .map(|trace| trace.oids.contains(commit.oid.as_str()));
                                let dimmed = graph_row_dimmed(
                                    in_trace,
                                    commit.parents.len() > 1,
                                    this.commit_graph.dim_merges,
                                );
                                this.commit_graph_list_row(commit, graph, dimmed, cx)
                                    .into_any_element()
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

        scrollable_uniform_frame(
            COMMIT_GRAPH_LIST_SCROLL_ID,
            ScrollbarMode::Vertical,
            content,
            handle,
            content_present,
            cx,
        )
    }

    /// 图谱页提交行：泳道格（含列宽分割条）+ 两行内容；淡化行内容降不透明度。
    fn commit_graph_list_row(
        &self,
        commit: CommitInfo,
        graph: Option<CommitGraphRow>,
        dimmed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

        list_row_surface(format!("commit-graph-{row_short_oid}"), selected)
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_1()
            // 左内边距与选中指示条（左缘 2px）错开；泳道隐藏（搜索/文件过滤）
            // 时行首没有泳道格子，同样需要这段留白。
            .pl(px(ui_theme::SPACE_3))
            .pr_2()
            .h(px(COMMIT_GRAPH_ROW_HEIGHT))
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
                // 选中与主历史页共享：文件/差异随即预加载，跳转后四象限直接就位。
                this.select_history_commit(oid.clone());
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
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
            .when_some(graph, |this, graph| {
                this.child(render_commit_graph_cell(
                    graph,
                    self.history_graph_width,
                    dimmed,
                ))
                .child(self.render_history_graph_splitter(cx))
            })
            .child(commit_row_content(
                &commit,
                GRAPH_REF_LABEL_CAP,
                unpushed,
                dimmed,
            ))
    }

    /// 底部轻量详情卡：完整信息 + 全量标签 + 「在提交记录页查看」跳转。
    /// 深入检查（文件列表/差异）留在主历史页四象限，本页不复制检查器。
    fn render_commit_graph_details_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let commit = self
            .history_selected_commit
            .as_deref()
            .and_then(|oid| self.history_commits.iter().find(|info| info.oid == oid))
            .cloned();
        let collapsed = self.commit_graph.details_collapsed;

        let Some(commit) = commit else {
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(COMMIT_GRAPH_DETAILS_HEIGHT))
                .child(section_header_action("提交详情", None))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("点击泳道行选中提交，查看完整信息"),
                )
                .into_any_element();
        };

        let header_title = if collapsed {
            format!("提交详情 · {}", commit.summary)
        } else {
            "提交详情".to_string()
        };
        let toggle_label: &'static str = if collapsed { "展开" } else { "收起" };
        let header = section_header_action(
            header_title,
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(history_scope_button(
                        "在提交记录页查看",
                        false,
                        |this| this.close_commit_graph(),
                        cx,
                    ))
                    .child(history_scope_button(
                        toggle_label,
                        false,
                        |this| {
                            this.commit_graph.details_collapsed =
                                !this.commit_graph.details_collapsed;
                        },
                        cx,
                    ))
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
                    .id("commit-graph-copy-sha")
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
                    .id("commit-graph-copy-message")
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

        let handle = self.scroll_handle("commit-graph-details-scroll");
        let scroll_content = div()
            .id("commit-graph-details-scroll")
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
                    .flex_wrap()
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
                    // 详情卡全量展示引用标签（不受行内 3 个上限约束）。
                    .children(commit_ref_labels(
                        &commit.refs,
                        &commit.short_oid,
                        commit.refs.len(),
                    ))
                    .when(commit.refs.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("无引用标签"),
                        )
                    }),
            )
            .children(body_text.map(|text| {
                div()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(text)
            }))
            .child(meta_row);

        let content = scrollable_frame_when(
            "commit-graph-details-scroll",
            ScrollbarMode::Vertical,
            scroll_content.into_any_element(),
            handle,
            true,
            cx,
        );

        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(COMMIT_GRAPH_DETAILS_HEIGHT))
            .child(header)
            .child(content)
            .into_any_element()
    }

    /// 泳道列右侧的行内拖拽分割条：流式排布自动与泳道列对齐，吞掉点击避免误选提交。
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

    /// 拖拽泳道列宽期间的窗口级鼠标事件承载层：无命中区，不拦截列表点击。
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
}

#[cfg(test)]
#[path = "tests/commit_graph_view.rs"]
mod tests;
