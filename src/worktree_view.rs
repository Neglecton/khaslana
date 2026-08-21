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
        components::{app_panel, list_row_surface},
        icons::ToolbarIcon,
        theme::{self as ui_theme, rgb},
    },
};

/// 左侧变更分区的高度规则。加载和有内容的分区才占用剩余空间；空分区保持一行提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeSectionHeight {
    Compact,
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChangeSectionsLayout {
    pub(crate) staged: ChangeSectionHeight,
    pub(crate) unstaged: ChangeSectionHeight,
}

pub(crate) const fn change_sections_layout(
    staged_count: usize,
    staged_loading: bool,
    unstaged_count: usize,
    unstaged_loading: bool,
) -> ChangeSectionsLayout {
    ChangeSectionsLayout {
        staged: if staged_loading || staged_count > 0 {
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

impl RepositoryView {
    pub(crate) fn render_worktree_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let conflict_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.conflicts.len());

        app_panel()
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
                    .child(self.render_changes(window, cx))
                    .child(self.render_column_splitter(ResizeTarget::Changes, cx))
                    .child(self.render_diff(cx)),
            )
    }

    fn render_worktree_conflict_banner(&self, conflict_count: usize) -> impl IntoElement {
        div()
            .flex_none()
            .px(px(ui_theme::SPACE_4))
            .py(px(ui_theme::SPACE_2))
            .border_b_1()
            .border_color(rgb(ui_theme::FEEDBACK_WARNING_BORDER))
            .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
            .text_size(px(ui_theme::TYPE_BODY))
            .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
            .child(format!(
                "存在 {conflict_count} 个冲突文件，请在冲突工作台中处理"
            ))
    }

    fn render_changes(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let staged_count = self.change_indexes.staged.len();
        let unstaged_count = self.change_indexes.unstaged.len();
        let has_staged = staged_count > 0;
        let has_unstaged = unstaged_count > 0;
        let layout = change_sections_layout(
            staged_count,
            self.loading.staged(),
            unstaged_count,
            self.loading.unstaged(),
        );

        app_panel()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.changes_width))
            .min_w(px(self.changes_width))
            .min_h(px(0.0))
            .border_r_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.render_conflict_section(cx))
                    .child(self.render_virtual_change_section(
                        "已暂存变更",
                        "staged-change-list",
                        "暂存区加载中...",
                        self.loading.staged(),
                        staged_count,
                        DiffScope::Staged,
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
                        cx,
                    ))
                    .child(self.render_virtual_change_section(
                        "未暂存变更",
                        "unstaged-change-list",
                        "修改区加载中...",
                        self.loading.unstaged(),
                        unstaged_count,
                        DiffScope::Unstaged,
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
                        cx,
                    )),
            )
            .child(self.render_commit_box(window, cx))
    }

    fn render_virtual_change_section(
        &self,
        title: &'static str,
        id: &'static str,
        loading_text: &'static str,
        loading: bool,
        count: usize,
        scope: DiffScope,
        actions: Vec<gpui::AnyElement>,
        height: ChangeSectionHeight,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_staged = scope == DiffScope::Staged;

        // 设计图：
        // 未暂存变更标题 + 计数 badge（SECONDARY bg / SECONDARY_FOREGROUND fg）
        // 已暂存变更标题 + 计数 badge（PRIMARY bg / PRIMARY_FOREGROUND fg）
        let badge_bg = if is_staged {
            ui_theme::PRIMARY
        } else {
            ui_theme::SECONDARY
        };
        let badge_fg = if is_staged {
            ui_theme::PRIMARY_FOREGROUND
        } else {
            ui_theme::SECONDARY_FOREGROUND
        };
        let count_badge = if count > 0 {
            Some(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(ui_theme::RADIUS_PILL))
                    .bg(rgb(badge_bg))
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(badge_fg))
                    .child(count.to_string())
                    .into_any_element(),
            )
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .when(height == ChangeSectionHeight::Fill, |this| {
                this.flex_1().min_h(px(0.0))
            })
            .when(height == ChangeSectionHeight::Compact, |this| {
                this.flex_none()
            })
            .when(is_staged, |this| {
                this.border_t_1().border_color(rgb(ui_theme::BORDER))
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(title),
                            )
                            .when_some(count_badge, |this, badge| this.child(badge)),
                    )
                    .child(div().flex().items_center().gap(px(4.0)).children(actions)),
            )
            .child({
                let handle = self.uniform_scroll_handle(id);
                let list_handle = handle.clone();
                let scope_for_list = scope.clone();
                let empty_text = if loading {
                    loading_text
                } else if is_staged {
                    "暂无已暂存变更"
                } else {
                    "暂无未暂存变更"
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
                                            .map(|_| placeholder_row(empty_text).into_any_element())
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
            vec![placeholder_row(loading_text).into_any_element()]
        } else {
            rows
        };
        let badge_bg = if is_staged {
            ui_theme::PRIMARY
        } else {
            ui_theme::SECONDARY
        };
        let badge_fg = if is_staged {
            ui_theme::PRIMARY_FOREGROUND
        } else {
            ui_theme::SECONDARY_FOREGROUND
        };
        let count_badge = if count > 0 {
            Some(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(ui_theme::RADIUS_PILL))
                    .bg(rgb(badge_bg))
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(badge_fg))
                    .child(count.to_string())
                    .into_any_element(),
            )
        } else {
            None
        };

        // 冲突工作台仍使用自定义行；保留普通滚动容器，避免改变既有交互。
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .when(is_staged, |this| {
                this.border_t_1().border_color(rgb(ui_theme::BORDER))
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(title),
                            )
                            .when_some(count_badge, |this, badge| this.child(badge)),
                    )
                    .child(div().flex().items_center().gap(px(4.0)).children(actions)),
            )
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

        // 设计图：已暂存行 bg ACCENT，未暂存行无背景
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
            this.bg(rgb(ui_theme::ACCENT))
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
                .text_color(rgb(ui_theme::FOREGROUND))
                .truncate()
                .child(change.path),
        )
        // 设计图：行内图标按钮 20×20
        .child(self.change_row_icon_button(
            row_action_label,
            row_action_icon,
            ui_theme::MUTED_FOREGROUND,
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

        div()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            .min_h(px(260.0))
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

    fn render_commit_box(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_commit = self.repo_path.is_some() && !self.busy;
        let merge_in_progress = self.merge_in_progress();
        let conflict_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.conflicts.len());
        let can_primary_commit = if merge_in_progress {
            merge_view::merge_can_finish(
                merge_in_progress,
                conflict_count,
                self.busy,
                &self.commit_message.value,
            )
        } else {
            can_commit
        };
        let can_commit_and_push =
            can_commit && !merge_in_progress && self.current_remote().is_some();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            // 修补上次提交开关：位于提交信息输入框上方、靠右；
            // 合并进行中不提供（合并提交用“完成合并”路径）。
            .when(!merge_in_progress && self.repo_path.is_some(), |this| {
                this.child(div().flex().justify_end().child(self.toggle_row(
                    "commit-amend-toggle",
                    "修补上次提交",
                    self.amend_mode,
                    |this, _window, _cx| {
                        this.amend_mode = !this.amend_mode;
                        if this.amend_mode {
                            // 开启时输入框为空则预填 HEAD 的完整提交信息，
                            // 方便只改信息或补文件。
                            if this.commit_message.value.trim().is_empty() {
                                this.prefill_amend_message();
                            }
                        } else if let Some(prefill) = this.amend_prefill.take() {
                            // 关闭时清除由开关预填且未被用户修改的内容；
                            // 用户已编辑则保留，避免误删输入。
                            if this.commit_message.value == prefill {
                                this.commit_message.clear();
                            }
                        }
                    },
                    cx,
                )))
            })
            .child(self.input(FieldId::CommitMessage, false, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(self.render_ai_commit_button(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(self.primary_button(
                                // 修补模式下主按钮变为“修补提交”，以当前暂存区重写 HEAD。
                                if !merge_in_progress && self.amend_mode {
                                    "修补提交"
                                } else {
                                    merge_view::merge_commit_button_label(merge_in_progress)
                                },
                                can_primary_commit,
                                |this, _, _| {
                                    if this.amend_mode && !this.merge_in_progress() {
                                        this.amend();
                                    } else {
                                        this.commit();
                                    }
                                },
                                cx,
                            ))
                            .when(merge_in_progress, |this| {
                                this.child(self.danger_button(
                                    "中止合并",
                                    !self.busy,
                                    |this, _, _| this.open_abort_merge_confirm_dialog(),
                                    cx,
                                ))
                            })
                            .when(!merge_in_progress, |this| {
                                this.child(self.primary_button(
                                    // 修补模式下变为“修补提交并推送”。
                                    if self.amend_mode {
                                        "修补提交并推送"
                                    } else {
                                        "提交并推送"
                                    },
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
                            }),
                    ),
            )
    }
}

#[cfg(test)]
#[path = "tests/worktree_view.rs"]
mod tests;
