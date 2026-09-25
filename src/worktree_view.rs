use gpui::{
    Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, Window, div, prelude::*,
    px, uniform_list,
};
use khaslana::DiffScope;

use crate::{
    CHANGE_ROW_HEIGHT, DiffHeaderTarget, EncodingMenuTarget, FieldId, RepositoryView, ResizeTarget,
    ScrollbarMode, change_state_badge, diff_scope_id, diff_scope_label, merge_view,
    placeholder_row, scrollable_frame_when, scrollable_uniform_frame,
    ui::{
        components::{
            EmptyState, PanelBadge, PanelHeaderSurface, PlaceholderAlign, floating_panel,
            list_row_surface, panel_empty_row, panel_empty_row_aligned, panel_section_header,
        },
        icons::ToolbarIcon,
        theme::{self as ui_theme, rgb},
    },
};

/// 提交条上「提交到 &lt;分支&gt;」按钮里分支名的最长字符数：超出按字符截断并加省略号。
/// 窄窗与长分支名都不能把主按钮挤出提交条（验收矩阵：长分支名不覆盖主操作）。
const COMMIT_BRANCH_LABEL_MAX_CHARS: usize = 16;

/// 提交条上「提交到 &lt;分支&gt;」的文案。
///
/// 分支名来自 `snapshot.head`（短名）；HEAD 脱离分支（detached）或尚未打开仓库时
/// 退化为「提交到当前分支」，不猜分支名。这个文案同时是按钮的 ElementId 条目键，
/// 因此分支名必须随仓库切换而变化（同一页面上的提交按钮只有这一个）。
fn commit_branch_button_label(head: Option<&str>) -> String {
    let Some(head) = head.map(str::trim).filter(|head| !head.is_empty()) else {
        return "提交到当前分支".to_string();
    };
    let mut chars = head.chars();
    let truncated: String = chars.by_ref().take(COMMIT_BRANCH_LABEL_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("提交到 {truncated}…")
    } else {
        format!("提交到 {truncated}")
    }
}

/// 左侧变更分区的高度规则。加载和有内容的分区才占用剩余空间；空分区保持一行提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeSectionHeight {
    Compact,
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChangeSectionsLayout {
    /// 是否渲染「已暂存变更」分区。无暂存内容时整段不出现，未暂存列表独占左列
    /// （重构计划 §5：无暂存时只显示未暂存列表，有暂存时才显示两个列表）。
    pub(crate) show_staged: bool,
    pub(crate) staged: ChangeSectionHeight,
    pub(crate) unstaged: ChangeSectionHeight,
}

/// 左列分区的可见性与高度规则。
///
/// `show_staged` 只在「暂存区确实有内容」或「暂存区正在加载」时为真——后者是为了让
/// 「暂存区加载中…」先占住位置，避免加载完成瞬间布局跳动。加载结束仍为空则整段消失。
pub(crate) const fn change_sections_layout(
    staged_count: usize,
    staged_loading: bool,
    unstaged_count: usize,
    unstaged_loading: bool,
) -> ChangeSectionsLayout {
    let show_staged = staged_count > 0 || staged_loading;
    ChangeSectionsLayout {
        show_staged,
        staged: if show_staged {
            ChangeSectionHeight::Fill
        } else {
            ChangeSectionHeight::Compact
        },
        unstaged: if unstaged_loading || unstaged_count > 0 {
            ChangeSectionHeight::Fill
        } else {
            ChangeSectionHeight::Compact
        },
    }
}

/// 未暂存分区的空态文案：一侧空 ≠ 全部干净（视觉规范 §2：
/// 「干净工作区」与「没有可显示内容」是两种状态，分别设计文案）。
pub(crate) fn unstaged_empty_text(loading: bool, peer_has_content: bool) -> &'static str {
    if loading {
        return "修改区加载中...";
    }
    if peer_has_content {
        // 对侧有暂存内容：工作区没有新的改动，但流程没结束。
        "未暂存变更已全部暂存"
    } else {
        // 两边都空：工作区干净（专属文案，与「暂无…」区分）。
        "工作区干净，没有待提交的改动"
    }
}

/// 提交条次级动作（「提交到 &lt;分支&gt;」/「修补提交」）的启用守卫。
///
/// 计划 §5：禁用条件以真实业务守卫为准，**不能**把「暂存区非空」套进这里——
/// amend 修补允许空暂存（只改信息）。普通提交从不因「没有暂存文件」被禁用，
/// 否则只想更新提交信息时会失去入口。合并中改用 `merge_can_finish` 的结论
/// （它自己检查 busy/冲突/信息，这里直接透传其布尔结果）。
pub(crate) const fn commit_action_enabled(
    repo_open: bool,
    busy: bool,
    merge_in_progress: bool,
    merge_can_finish: bool,
) -> bool {
    if merge_in_progress {
        merge_can_finish
    } else {
        repo_open && !busy
    }
}

/// 提交条主动作（「提交并推送」）的启用守卫：普通提交的前两个条件之外，
/// 还要求仓库配置了远端（`has_remote`），合并中完全不提供推送入口。
pub(crate) const fn commit_and_push_enabled(
    repo_open: bool,
    busy: bool,
    merge_in_progress: bool,
    has_remote: bool,
) -> bool {
    !merge_in_progress && repo_open && !busy && has_remote
}

impl RepositoryView {
    pub(crate) fn render_worktree_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 「没有仓库」是页面级空态：不渲染空列表与禁用的提交条，
        // 给出可执行入口（视觉规范 §2：无仓库与干净工作区分别设计）。
        if self.repo_path.is_none() {
            return floating_panel()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(
                    EmptyState::new("还没有打开的仓库")
                        .detail("从本地磁盘打开现有仓库，或克隆一个远端仓库开始工作")
                        .fill()
                        .action(
                            self.toolbar_button(
                                "打开仓库…",
                                ToolbarIcon::Open,
                                true,
                                |this, _window, _cx| {
                                    this.browse_open();
                                },
                                cx,
                            )
                            .into_any_element(),
                        )
                        .action(
                            self.toolbar_button(
                                "克隆仓库…",
                                ToolbarIcon::Clone,
                                true,
                                |this, window, cx| {
                                    this.open_clone_dialog(window, cx);
                                },
                                cx,
                            )
                            .into_any_element(),
                        )
                        .build(),
                )
                .into_any_element();
        }

        let conflict_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.conflicts.len());

        // 页面根不铺底色：左列变更列表、右列差异与提交条各自是独立的悬浮面板。
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            // 页面不再有静态头行（工作区/已暂存/未暂存）：分支名在 titlebar，
            // 计数由下方各分区标题自带，模式入口在左侧 Context Navigator。
            .when_some(self.render_merge_banner(cx), |this, banner| {
                this.child(banner)
            })
            .when_some(self.render_rebase_banner(cx), |this, banner| {
                this.child(banner)
            })
            .when(conflict_count > 0, |this| {
                this.child(self.render_worktree_conflict_banner(conflict_count))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    // 左列：变更列表占满可用高度。
                    .child(self.render_changes(cx))
                    .child(self.render_column_splitter(ResizeTarget::Changes, cx))
                    // 右列：差异占满剩余高度，提交条固定在 Diff 正下方、与代码区同宽
                    //（最新 Pencil 稿第三版：提交信息移到右侧 Diff 正下方，左侧文件列表
                    // 占满可用高度）。两段共处一列，因此差异区不能再设固定最小高度。
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(self.render_diff(cx))
                            .child(self.render_commit_box(window, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_worktree_conflict_banner(&self, conflict_count: usize) -> impl IntoElement {
        div()
            .flex_none()
            .px(px(ui_theme::SPACE_4))
            .py(px(ui_theme::SPACE_2))
            .mb(px(ui_theme::SPACE_2))
            .rounded(px(ui_theme::RADIUS_MD))
            .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
            .text_size(px(ui_theme::TYPE_BODY))
            .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
            .child(format!(
                "存在 {conflict_count} 个冲突文件，请在冲突工作台中处理"
            ))
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let staged_count = self.change_indexes.staged.len();
        let unstaged_count = self.change_indexes.unstaged.len();
        let has_staged = staged_count > 0;
        let has_unstaged = unstaged_count > 0;
        // 冲突区（若存在）占据左列顶部，其余分区不再是面板第一行。
        let has_conflicts = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.conflicts.is_empty());
        let layout = change_sections_layout(
            staged_count,
            self.loading.staged(),
            unstaged_count,
            self.loading.unstaged(),
        );

        // 左列变更列表是独立悬浮面板：与差异面板之间只隔拖拽区的空隙，
        // 圆角与投影表达「两块分开的实体」，不再共用一张大卡。
        floating_panel()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.changes_width))
            .min_w(px(self.changes_width))
            .min_h(px(0.0))
            // 右侧分隔线由紧随的列分割条（Changes）统一绘制；
            // 面板不得再自画右边框，否则出现两条平行框线。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.render_conflict_section(cx))
                    // 已暂存分区按需出现：无暂存内容时不渲染，
                    // 未暂存列表直接吃满左列剩余高度。
                    .when(layout.show_staged, |this| {
                        this.child(self.render_virtual_change_section(
                            "已暂存变更",
                            "staged-change-list",
                            "暂存区加载中...",
                            self.loading.staged(),
                            staged_count,
                            DiffScope::Staged,
                            unstaged_count > 0,
                            vec![
                                self.change_icon_button(
                                    "取消暂存全部",
                                    ToolbarIcon::Minus,
                                    has_staged && !self.busy,
                                    |this, _, _| this.unstage_all(),
                                    cx,
                                )
                                .into_any_element(),
                            ],
                            layout.staged,
                            // 无冲突区时，已暂存分区是面板第一行：标题行带顶部圆角。
                            !has_conflicts,
                            cx,
                        ))
                    })
                    .child(self.render_virtual_change_section(
                        "未暂存变更",
                        "unstaged-change-list",
                        "修改区加载中...",
                        self.loading.unstaged(),
                        unstaged_count,
                        DiffScope::Unstaged,
                        staged_count > 0,
                        vec![
                            self.change_icon_button(
                                "暂存全部",
                                ToolbarIcon::Plus,
                                has_unstaged && !self.busy,
                                |this, _, _| this.stage_all(),
                                cx,
                            )
                            .into_any_element(),
                            self.change_destructive_icon_button(
                                "丢弃全部",
                                has_unstaged && !self.busy,
                                |this, _, _| this.confirm_discard_all(),
                                cx,
                            )
                            .into_any_element(),
                        ],
                        layout.unstaged,
                        // 无冲突区且无已暂存分区时，未暂存分区才是面板第一行。
                        !has_conflicts && !layout.show_staged,
                        cx,
                    )),
            )
    }

    fn render_virtual_change_section(
        &self,
        title: &'static str,
        id: &'static str,
        loading_text: &'static str,
        loading: bool,
        count: usize,
        scope: DiffScope,
        // 对侧分区是否有内容：未暂存列表为空而对侧有暂存时，空态文案
        // 与「工作区干净」区分（一侧空 ≠ 全部干净）。
        peer_has_content: bool,
        actions: Vec<gpui::AnyElement>,
        height: ChangeSectionHeight,
        // 该分区是否是左列面板的第一行（决定标题行是否带顶部圆角）。
        top: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_staged = scope == DiffScope::Staged;

        // 设计图：未暂存变更计数徽标用 WB_ROW_HOVER 底，已暂存用 PRIMARY 底
        //（「已暂存」是即将进入提交的内容，用主色标记）。
        let badge = (count > 0).then(|| {
            if is_staged {
                PanelBadge::new(count, ui_theme::PRIMARY, ui_theme::PRIMARY_FOREGROUND)
            } else {
                PanelBadge::new(count, ui_theme::WB_ROW_HOVER, ui_theme::CONTENT_PRIMARY)
            }
        });
        let mut header = panel_section_header(title).padding_x(ui_theme::SPACE_4);
        if top {
            header = header.top_rounded();
        }
        if let Some(badge) = badge {
            header = header.badge(badge);
        }

        div()
            .flex()
            .flex_col()
            .when(height == ChangeSectionHeight::Fill, |this| {
                this.flex_1().min_h(px(0.0))
            })
            .when(height == ChangeSectionHeight::Compact, |this| {
                this.flex_none()
            })
            // 仅在前面有其他分区时留白；首个分区的标题必须贴合面板顶部。
            .when(!top, |this| this.mt(px(ui_theme::SPACE_2)))
            .child(header.actions(actions).build())
            .child({
                let handle = self.uniform_scroll_handle(id);
                let list_handle = handle.clone();
                let scope_for_list = scope.clone();
                let empty_text = if loading {
                    loading_text
                } else if is_staged {
                    "暂无已暂存变更"
                } else {
                    unstaged_empty_text(loading, peer_has_content)
                };
                let content = div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .when(height == ChangeSectionHeight::Fill, |this| {
                        this.flex_1().min_h(px(0.0))
                    })
                    .when(height == ChangeSectionHeight::Compact, |this| {
                        this.flex_none().h(px(CHANGE_ROW_HEIGHT))
                    })
                    .w_full()
                    .min_w(px(0.0))
                    .child(
                        // 上万文件时仅为当前可见范围构造行，避免每次重绘创建全部元素。
                        uniform_list(
                            id,
                            count.max(1),
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, cx| {
                                    if count == 0 {
                                        return range
                                            .map(|_| {
                                                panel_empty_row_aligned(
                                                    empty_text,
                                                    CHANGE_ROW_HEIGHT,
                                                    PlaceholderAlign::Center,
                                                    ui_theme::SPACE_4,
                                                )
                                                .into_any_element()
                                            })
                                            .collect::<Vec<_>>();
                                    }
                                    let indexes = this.change_indexes.for_scope(&scope_for_list);
                                    range
                                        .map(|row_index| {
                                            indexes
                                                .get(row_index)
                                                .and_then(|change_index| {
                                                    this.snapshot.as_ref().and_then(|snapshot| {
                                                        snapshot.changes.get(*change_index)
                                                    })
                                                })
                                                .cloned()
                                                .map(|change| {
                                                    this.change_row(
                                                        change,
                                                        scope_for_list.clone(),
                                                        cx,
                                                    )
                                                    .into_any_element()
                                                })
                                                .unwrap_or_else(|| {
                                                    placeholder_row("").into_any_element()
                                                })
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .track_scroll(&list_handle)
                        .with_sizing_behavior(ListSizingBehavior::Auto)
                        .flex_1()
                        .w_full()
                        .min_w(px(0.0))
                        .min_h(px(0.0)),
                    )
                    .into_any_element();
                scrollable_uniform_frame(
                    id,
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    count > 0,
                    cx,
                )
            })
    }

    pub(crate) fn render_change_section(
        &self,
        title: &'static str,
        id: &'static str,
        loading_text: &'static str,
        loading: bool,
        rows: Vec<gpui::AnyElement>,
        content_present: bool,
        count: usize,
        is_staged: bool,
        actions: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = if rows.is_empty() && loading {
            vec![
                panel_empty_row(loading_text, CHANGE_ROW_HEIGHT, PlaceholderAlign::Start)
                    .into_any_element(),
            ]
        } else {
            rows
        };
        let badge = (count > 0).then(|| {
            if is_staged {
                PanelBadge::new(count, ui_theme::PRIMARY, ui_theme::PRIMARY_FOREGROUND)
            } else {
                PanelBadge::new(count, ui_theme::WB_ROW_HOVER, ui_theme::CONTENT_PRIMARY)
            }
        });
        let mut header = panel_section_header(title).padding_x(ui_theme::SPACE_4);
        if let Some(badge) = badge {
            header = header.badge(badge);
        }

        // 冲突工作台仍使用自定义行；保留普通滚动容器，避免改变既有交互。
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(header.actions(actions).build())
            .child({
                let handle = self.scroll_handle(id);
                let content = div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(2.0))
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    id,
                    ScrollbarMode::Both,
                    content,
                    handle,
                    content_present,
                    cx,
                )
            })
    }

    fn change_row(
        &self,
        change: khaslana::WorktreeChange,
        scope: DiffScope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let path = change.path.clone();
        let selected = self.is_change_selected(&scope, &change.path);
        let state = match scope {
            DiffScope::Staged => change.staged.as_ref(),
            DiffScope::Unstaged => change.unstaged.as_ref(),
        };
        let is_staged = scope == DiffScope::Staged;
        // 行内图标按钮的 ElementId（页面 + 暂存态 + 路径 + 动作）；在 path 被移入
        // 各闭包与子元素之前先算好。
        let row_action_id = format!("change-row-action-{}-{}", diff_scope_id(&scope), path);

        // 行内图标按钮：plus（暂存）或 minus（取消暂存）
        let row_action_icon = if is_staged {
            ToolbarIcon::Minus
        } else {
            ToolbarIcon::Plus
        };
        let row_action_label = if is_staged {
            "取消暂存此文件"
        } else {
            "暂存此文件"
        };
        let row_action_enabled = !self.busy;
        let row_action_click: std::sync::Arc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
            if is_staged {
                std::sync::Arc::new({
                    let path = path.clone();
                    move |this: &mut Self, _window: &mut Window, _cx: &mut Context<Self>| {
                        this.unstage_paths(vec![path.clone()], "取消暂存");
                    }
                })
            } else {
                std::sync::Arc::new({
                    let path = path.clone();
                    move |this: &mut Self, _window: &mut Window, _cx: &mut Context<Self>| {
                        this.stage_paths(vec![path.clone()], "暂存");
                    }
                })
            };

        // 设计图：已暂存行 bg STATE_HOVER，未暂存行无背景
        list_row_surface(
            format!("change-{}-{}", diff_scope_id(&scope), change.path),
            selected,
        )
        .flex()
        .flex_none()
        .w_full()
        .min_w(px(0.0))
        .h(px(CHANGE_ROW_HEIGHT))
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(8.0))
        .overflow_hidden()
        .cursor_pointer()
        .when(is_staged && !selected, |this| {
            this.bg(rgb(ui_theme::STATE_HOVER))
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let path = path.clone();
                let scope = scope.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.select_change_from_mouse(path.clone(), scope.clone(), event);
                    this.change_context_menu = None;
                    cx.notify();
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.open_change_context_menu(path.clone(), scope.clone(), event, _window);
                cx.notify();
            }),
        )
        // 状态徽章：圆角填充底色 + 白色加粗字母（统一 Git 状态色）
        .child(change_state_badge(state))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                // uniform_list 行禁 truncate（MinContent 测量坍缩后省略号
                // 固化到绘制），硬裁剪替代。
                .overflow_hidden()
                .whitespace_nowrap()
                .child(change.path),
        )
        // 设计图：行内图标按钮 20×20。
        // 按钮在每个变更行里都出现，因此 ElementId 必须带条目键（scope + 路径），
        // 不能让所有行共用同一个键控状态（见 `change_row_icon_button`）。
        .child(self.change_row_icon_button(
            row_action_id,
            row_action_label,
            row_action_icon,
            ui_theme::CONTENT_SECONDARY,
            row_action_enabled,
            {
                let click = row_action_click;
                move |this, _window, cx| {
                    click(this, _window, cx);
                    cx.notify();
                }
            },
            cx,
        ))
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 全文视图模式下标题前缀"全文："，提示当前展示整份文件而非仅改动区域
        let prefix = if self.full_file_view { "全文：" } else { "" };
        let title = self
            .diff
            .as_ref()
            .map(|diff| {
                format!(
                    "{prefix}差异：{} ({})",
                    diff.path,
                    diff_scope_label(&diff.scope)
                )
            })
            .unwrap_or_else(|| "差异".to_string());

        // 差异区是与变更列表并列的独立悬浮面板；提交条浮在它下方，
        // 三段各自带圆角与投影，中间以留白分隔。
        floating_panel()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            // 右列被「差异 + 提交条」两段共享：这里只负责吃满剩余高度，
            // 不设固定最小高度，否则最小窗（860 × 520）下提交条会被挤出视口。
            .min_h(px(0.0))
            .child(self.diff_section_header(title, EncodingMenuTarget::Worktree, cx))
            .child(self.render_virtual_diff(
                "diff-scroll",
                self.diff.clone(),
                self.diff_headers_expanded,
                DiffHeaderTarget::Worktree,
                "请选择一个变更文件查看差异".to_string(),
                cx,
            ))
            .child(self.render_encoding_dropdown(EncodingMenuTarget::Worktree, cx))
    }

    /// 提交条：位于差异区正下方、与代码区同宽（最新 Pencil 稿第三版）。
    ///
    /// 结构与设计稿一致：标题行「提交信息 + AI 生成」、提交信息输入框、
    /// 底部一行「修补上次提交」与提交动作。动作按设计稿分主次：
    /// 「提交到 &lt;分支&gt;」是浅色次级按钮，「提交并推送」是蓝色主按钮；
    /// 合并进行中用「完成合并」+「中止合并」，不出现推送入口。
    fn render_commit_box(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let merge_in_progress = self.merge_in_progress();
        let conflict_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.conflicts.len());
        let can_primary_commit = commit_action_enabled(
            self.repo_path.is_some(),
            self.busy,
            merge_in_progress,
            merge_view::merge_can_finish(
                merge_in_progress,
                conflict_count,
                self.busy,
                &self.commit_message.value,
            ),
        );
        let can_commit_and_push = commit_and_push_enabled(
            self.repo_path.is_some(),
            self.busy,
            merge_in_progress,
            self.current_remote().is_some(),
        );
        // 次级动作的文案：修补模式是「修补提交」，否则是「提交到 <当前分支>」。
        let secondary_label = if self.amend_mode {
            "修补提交".to_string()
        } else {
            commit_branch_button_label(
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.head.as_deref()),
            )
        };
        // 主动作文案同样预计算：修补模式下变为「修补提交并推送」。
        let primary_label: &'static str = if self.amend_mode {
            "修补提交并推送"
        } else {
            "提交并推送"
        };

        let commit_actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(ui_theme::SPACE_2))
            // 窄窗/长分支名时不裁切按钮：动作组整体换行，主操作始终可达。
            .flex_wrap()
            .when(merge_in_progress, |this| {
                this.child(self.primary_button(
                    merge_view::merge_commit_button_label(true),
                    can_primary_commit,
                    |this, _, _| this.commit(),
                    cx,
                ))
                .child(self.danger_button(
                    "中止合并",
                    !self.busy,
                    |this, _, _| this.open_abort_merge_confirm_dialog(),
                    cx,
                ))
            })
            .when(!merge_in_progress, |this| {
                this.child(self.secondary_button_with_icon(
                    secondary_label.into(),
                    ToolbarIcon::Check,
                    can_primary_commit,
                    |this, _, _| {
                        if this.amend_mode {
                            this.amend();
                        } else {
                            this.commit();
                        }
                    },
                    cx,
                ))
                .child(self.primary_button_with_icon(
                    primary_label,
                    ToolbarIcon::ArrowUp,
                    can_commit_and_push,
                    |this, _, _| {
                        if this.amend_mode {
                            this.amend_and_push();
                        } else {
                            this.commit_and_push();
                        }
                    },
                    cx,
                ))
            });

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_2()
            // 提交条与差异面板等宽同列：只留与上方差异区的间距，不设左右外边距
            // （设计稿：提交信息框与差异框左右对齐、宽度一致）。
            .mt(px(ui_theme::SPACE_2))
            .p(px(ui_theme::SPACE_3))
            .rounded(px(ui_theme::RADIUS_MD))
            // 提交区是独立抬起的一条工作条：用底色 + 圆角 + 内边距与上方内容分层，
            // 不再用贯穿的顶部边框把两区硬切开。
            .bg(rgb(ui_theme::WB_COMMIT_BAR))
            .shadow(crate::ui::components::control_shadow())
            // 标题行的底色随卡片本身：这里已经是一条抬起的实体，不再叠分组带。
            .child(
                panel_section_header("提交信息")
                    .surface(PanelHeaderSurface::Parent)
                    .action(self.render_ai_commit_button(cx).into_any_element())
                    .build(),
            )
            .child(self.input(FieldId::CommitMessage, false, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(ui_theme::SPACE_2))
                    // 极窄列下动作组整体换行，而不是与开关互相挤压或把主操作
                    // 挤出提交条（验收矩阵：长分支名不覆盖主操作）。
                    .flex_wrap()
                    // 修补上次提交复选框：位于提交信息下方左侧；
                    // 合并进行中不提供（合并提交用“完成合并”路径）。
                    // 设计稿是复选框（方框 + 勾）而不是开关。
                    .when(!merge_in_progress && self.repo_path.is_some(), |this| {
                        this.child(self.checkbox_row(
                            "commit-amend-toggle",
                            "修补上次提交",
                            self.amend_mode,
                            |this, _cx| {
                                this.amend_mode = !this.amend_mode;
                                if this.amend_mode {
                                    // 开启时输入框为空则预填 HEAD 的完整提交信息，
                                    // 方便只改信息或补文件。
                                    if this.commit_message.value.trim().is_empty() {
                                        this.prefill_amend_message();
                                    }
                                } else if let Some(prefill) = this.amend_prefill.take() {
                                    // 关闭时清除由复选框预填且未被用户修改的内容；
                                    // 用户已编辑则保留，避免误删输入。
                                    if this.commit_message.value == prefill {
                                        this.commit_message.clear();
                                    }
                                }
                            },
                            cx,
                        ))
                    })
                    .child(commit_actions.ml_auto()),
            )
    }
}

#[cfg(test)]
#[path = "tests/worktree_view.rs"]
mod tests;
