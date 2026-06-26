use gpui::{
    Animation, AnimationExt, Context, IntoElement, MouseButton, Window, div, prelude::*, px, rgb,
    rgba,
};
use khaslana::{BranchInfo, BranchKind, RepositorySnapshot};
use yororen_ui::animation::{constants::duration, ease_out_quint_clamped};
use yororen_ui::component::{ArrowDirection, IconName, icon, select, select_option};
use yororen_ui::theme::ActiveTheme;

use crate::{
    FieldId, RepositoryView, ScrollbarMode, scrollable_frame_intrinsic,
    ui::{
        components::{dialog_actions, toggle_box},
        theme as ui_theme,
    },
};

const REMOTE_OPERATION_DIALOG_WIDTH: f32 = 640.0;
const REMOTE_OPERATION_CONTROL_HEIGHT: f32 = 34.0;
const REMOTE_OPERATION_BRANCH_MENU_HEIGHT: f32 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteBranchOperationKind {
    Pull,
    Push,
    SetUpstream,
}

impl RemoteBranchOperationKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Pull => "拉取",
            Self::Push => "推送",
            Self::SetUpstream => "设置 upstream",
        }
    }

    pub(crate) fn confirm_label(self) -> &'static str {
        match self {
            Self::Pull => "拉取",
            Self::Push => "推送",
            Self::SetUpstream => "设置",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Pull => {
                "拉取会先获取所选远端，然后把远端分支合并到当前本地分支。远端分支必须已存在。"
            }
            Self::Push => {
                "推送会把当前本地分支推送到填写的远程分支；远端分支不存在时会自动创建，并设置为上游分支。"
            }
            Self::SetUpstream => {
                "upstream 用来计算本地分支相对远端分支的拉取和推送状态；只能选择已存在的远端分支。"
            }
        }
    }

    pub(crate) fn requires_existing_remote_branch(self) -> bool {
        matches!(self, Self::Pull | Self::SetUpstream)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RemoteBranchOperationState {
    pub(crate) local_branch: Option<String>,
    pub(crate) selected_remote: Option<String>,
    pub(crate) refreshing: bool,
    pub(crate) branch_dropdown_open: bool,
    /// 拉取时是否用变基代替合并。
    pub(crate) use_rebase: bool,
}

impl RemoteBranchOperationState {
    pub(crate) fn clear(&mut self) {
        self.local_branch = None;
        self.selected_remote = None;
        self.refreshing = false;
        self.branch_dropdown_open = false;
        self.use_rebase = false;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteBranchDialogDefaults {
    pub(crate) local_branch: String,
    pub(crate) remote: String,
    pub(crate) remote_branch: String,
}

pub(crate) fn current_local_branch(snapshot: &RepositorySnapshot) -> Option<&BranchInfo> {
    snapshot
        .branches
        .iter()
        .find(|branch| branch.kind == BranchKind::Local && branch.is_head)
}

pub(crate) fn local_branch_by_name<'a>(
    snapshot: &'a RepositorySnapshot,
    name: &str,
) -> Option<&'a BranchInfo> {
    snapshot
        .branches
        .iter()
        .find(|branch| branch.kind == BranchKind::Local && branch.name == name)
}

pub(crate) fn default_remote_branch_for(
    local_branch: &BranchInfo,
    selected_remote: &str,
) -> String {
    local_branch
        .upstream
        .as_deref()
        .and_then(|upstream| strip_remote_prefix(upstream, selected_remote))
        .map(str::to_string)
        .unwrap_or_else(|| local_branch.name.clone())
}

pub(crate) fn remote_branch_names<'a>(
    snapshot: &'a RepositorySnapshot,
    remote: &str,
) -> Vec<&'a str> {
    let mut names = snapshot
        .branches
        .iter()
        .filter(|branch| branch.kind == BranchKind::Remote)
        .filter_map(|branch| strip_remote_prefix(&branch.name, remote))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

pub(crate) fn remote_branch_exists(
    snapshot: &RepositorySnapshot,
    remote: &str,
    remote_branch: &str,
) -> bool {
    remote_branch_names(snapshot, remote)
        .into_iter()
        .any(|branch| branch == remote_branch)
}

pub(crate) fn remote_branch_dialog_defaults(
    snapshot: &RepositorySnapshot,
    current_remote: Option<String>,
) -> Result<RemoteBranchDialogDefaults, String> {
    let local_branch = current_local_branch(snapshot)
        .ok_or_else(|| "当前不是本地分支，无法拉取或推送".to_string())?;
    let remote = current_remote
        .filter(|remote| snapshot.remotes.iter().any(|info| info.name == *remote))
        .or_else(|| {
            snapshot
                .remotes
                .iter()
                .find(|remote| remote.name == "origin")
                .map(|remote| remote.name.clone())
        })
        .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()))
        .ok_or_else(|| "当前仓库没有远端".to_string())?;
    let remote_branch = default_remote_branch_for(local_branch, &remote);
    Ok(RemoteBranchDialogDefaults {
        local_branch: local_branch.name.clone(),
        remote,
        remote_branch,
    })
}

fn strip_remote_prefix<'a>(name: &'a str, remote: &str) -> Option<&'a str> {
    name.strip_prefix(remote)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
}

impl RepositoryView {
    pub(crate) fn render_remote_branch_operation_dialog(
        &self,
        kind: RemoteBranchOperationKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.snapshot.as_ref();
        let remotes = snapshot
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        let local_branch = snapshot
            .and_then(|snapshot| {
                self.remote_branch_operation
                    .local_branch
                    .as_deref()
                    .and_then(|name| local_branch_by_name(snapshot, name))
                    .or_else(|| current_local_branch(snapshot))
            })
            .map(|branch| branch.name.clone());
        let local_branch_label = local_branch.as_deref().unwrap_or("无本地分支").to_string();
        let selected_remote = self
            .remote_branch_operation
            .selected_remote
            .clone()
            .or_else(|| self.current_remote())
            .unwrap_or_default();
        let remote_branch = self.remote_branch_name.value.trim().to_string();
        let branch_exists = snapshot.is_some_and(|snapshot| {
            remote_branch_exists(snapshot, &selected_remote, &remote_branch)
        });
        let remote_exists = remotes.iter().any(|remote| remote.name == selected_remote);
        let can_confirm = !self.busy
            && !self.remote_branch_operation.refreshing
            && remote_exists
            && local_branch.is_some()
            && !remote_branch.is_empty()
            && (!kind.requires_existing_remote_branch() || branch_exists);
        let missing_branch_hint = if kind.requires_existing_remote_branch()
            && !remote_branch.is_empty()
            && !branch_exists
        {
            Some("远端分支不存在，请点击刷新或选择已有分支")
        } else {
            None
        };

        self.dialog_panel(kind.title(), cx)
            .w(px(REMOTE_OPERATION_DIALOG_WIDTH))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("当前本地分支"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .truncate()
                                    .child(local_branch_label),
                            ),
                    )
                    .child(self.button(
                        "刷新",
                        !self.busy && !selected_remote.is_empty(),
                        |this, _, _| this.refresh_remote_branch_operation(),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(kind.help()),
            )
            .child(self.remote_selector(remotes, selected_remote.clone(), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("远程分支"),
                    )
                    .child(self.remote_branch_editable_selector(
                        selected_remote.clone(),
                        window,
                        cx,
                    ))
                    .when_some(missing_branch_hint, |this, hint| {
                        this.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::DESTRUCTIVE))
                                .child(hint),
                        )
                    }),
            )
            // 拉取时显示"用变基代替合并"勾选框
            .when(kind == RemoteBranchOperationKind::Pull, |this| {
                let checked = self.remote_branch_operation.use_rebase;
                this.child(
                    div()
                        .id("pull-use-rebase-toggle")
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|this| this.opacity(0.8))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.remote_branch_operation.use_rebase =
                                    !this.remote_branch_operation.use_rebase;
                                cx.notify();
                            }),
                        )
                        .child(toggle_box(checked))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("用变基代替合并"),
                        ),
                )
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        kind.confirm_label(),
                        can_confirm,
                        move |this, _, _| this.confirm_remote_branch_operation(kind),
                        cx,
                    )),
            )
    }

    fn remote_selector(
        &self,
        remotes: Vec<khaslana::RemoteInfo>,
        selected_remote: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected_url = remotes
            .iter()
            .find(|remote| remote.name == selected_remote)
            .map(|remote| remote.url.clone());
        let options = remotes
            .iter()
            .map(|remote| {
                select_option()
                    .value(remote.name.clone())
                    .label(remote.name.clone())
            })
            .collect::<Vec<_>>();
        let entity = cx.entity();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("远端"),
            )
            .child(
                div().w_full().text_size(px(12.0)).child(
                    select("remote-branch-operation-remote-select")
                        .w_full()
                        .h(px(REMOTE_OPERATION_CONTROL_HEIGHT))
                        .options(options)
                        .placeholder("选择远端")
                        .value(selected_remote)
                        .disabled(remotes.is_empty() || self.busy)
                        .height(px(REMOTE_OPERATION_CONTROL_HEIGHT).into())
                        .menu_width(px(REMOTE_OPERATION_DIALOG_WIDTH - 32.0))
                        .on_change(move |value, _window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.select_remote_branch_operation_remote(value);
                                cx.notify();
                            });
                        }),
                ),
            )
            .when_some(selected_url, |this, url| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .truncate()
                        .child(url),
                )
            })
    }

    fn remote_branch_editable_selector(
        &self,
        selected_remote: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let branches = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                remote_branch_names(snapshot, &selected_remote)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let dropdown_open = self.remote_branch_operation.branch_dropdown_open;
        let disabled = self.busy || self.remote_branch_operation.refreshing;

        div()
            .relative()
            .w_full()
            .child(self.input(FieldId::RemoteBranchName, false, window, cx))
            .child(
                div()
                    .absolute()
                    .right(px(1.0))
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .w(px(34.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_l_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::INPUT_BG))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .when(!disabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    })
                    .when(disabled, |this| this.opacity(0.55).cursor_not_allowed())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            if disabled {
                                return;
                            }
                            let next_open = !this.remote_branch_operation.branch_dropdown_open;
                            this.remote_branch_operation.branch_dropdown_open = next_open;
                            this.remote_branch_search.clear();
                            if next_open {
                                window.focus(&this.remote_branch_search.focus);
                            } else {
                                window.focus(&this.remote_branch_name.focus);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(
                        icon(IconName::Arrow(ArrowDirection::Down))
                            .size(px(12.0))
                            .color(rgb(ui_theme::MUTED_FOREGROUND)),
                    ),
            )
            .when(dropdown_open, |this| {
                let menu = self.remote_branch_dropdown_menu(selected_remote, branches, window, cx);
                this.child(gpui::deferred(menu).with_priority(100))
            })
    }

    fn remote_branch_dropdown_menu(
        &self,
        selected_remote: String,
        branches: Vec<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected_branch = self.remote_branch_name.value.trim().to_string();
        let search = self.remote_branch_search.value.trim().to_lowercase();
        let current_remote = selected_remote.clone();
        let branches = branches
            .into_iter()
            .filter(|branch| {
                if search.is_empty() {
                    true
                } else {
                    let full_name = format!("{selected_remote}/{branch}").to_lowercase();
                    branch.to_lowercase().contains(&search) || full_name.contains(&search)
                }
            })
            .collect::<Vec<_>>();
        let content_present = !branches.is_empty();
        let handle = self.scroll_handle("remote-branch-operation-branch-list");
        let entity = cx.entity();
        let close_entity = entity.clone();
        let theme = cx.theme().clone();
        let menu_bg = theme.surface.raised;
        let menu_border = theme.border.default;
        let row_fg = theme.content.primary;
        let row_hover_bg = theme.surface.hover;
        let row_selected_bg = theme.action.primary.bg.alpha(0.10);
        let check_color = theme.action.primary.bg;

        let content = div()
            .id("remote-branch-operation-branch-list")
            .flex()
            .flex_col()
            .max_h(px(REMOTE_OPERATION_BRANCH_MENU_HEIGHT))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .py_1()
            .when(!content_present, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("暂无远端分支，请点击刷新获取"),
                )
            })
            .children(
                branches
                    .into_iter()
                    .map(move |branch| {
                        let value = branch.clone();
                        let label = format!("{current_remote}/{branch}");
                        let selected = selected_branch == branch;
                        let entity = entity.clone();

                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(row_fg)
                            .when(selected, |this| this.bg(row_selected_bg))
                            .hover(move |this| this.bg(row_hover_bg))
                            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.remote_branch_name.set_value(value.clone());
                                    this.remote_branch_operation.branch_dropdown_open = false;
                                    window.focus(&this.remote_branch_name.focus);
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            })
                            .child(div().min_w(px(0.0)).truncate().child(label))
                            .when(selected, |this| {
                                this.child(icon(IconName::Check).size(px(12.0)).color(check_color))
                            })
                    })
                    .collect::<Vec<_>>(),
            )
            .into_any_element();

        div()
            .absolute()
            .top(px(REMOTE_OPERATION_CONTROL_HEIGHT + 6.0))
            .left_0()
            .right_0()
            .rounded_md()
            .border_1()
            .border_color(menu_border)
            .bg(menu_bg)
            .shadow_md()
            .occlude()
            .on_mouse_down_out(move |_event, _window, cx| {
                let _ = close_entity.update(cx, |this, cx| {
                    this.remote_branch_operation.branch_dropdown_open = false;
                    this.remote_branch_search.clear();
                    cx.notify();
                });
            })
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .child(self.input(FieldId::RemoteBranchSearch, false, window, cx)),
            )
            .child(
                div()
                    .max_h(px(REMOTE_OPERATION_BRANCH_MENU_HEIGHT))
                    .when(content_present, |this| {
                        this.h(px(REMOTE_OPERATION_BRANCH_MENU_HEIGHT))
                    })
                    .when(!content_present, |this| this.flex_none())
                    .child(scrollable_frame_intrinsic(
                        "remote-branch-operation-branch-list",
                        ScrollbarMode::Vertical,
                        content,
                        handle,
                        cx,
                    )),
            )
            .with_animation(
                "remote-branch-operation-branch-menu",
                Animation::new(duration::MENU_OPEN).with_easing(ease_out_quint_clamped),
                |this, value| this.opacity(value).mt(px(6.0 - 6.0 * value)),
            )
    }
}

#[cfg(test)]
#[path = "tests/remote_branch_operation.rs"]
mod tests;
