use std::collections::BTreeMap;

use git2::Repository;
use gpui::{Context, CursorStyle, IntoElement, MouseButton, div, prelude::*, px, rgb, rgba};
use khaslana::{SubmoduleInfo, SubmoduleRemoteSyncStatus, SubmoduleState};

use crate::{
    DialogState, RepositoryView, ScrollbarMode, UiEvent, placeholder_row, scrollable_frame_when,
    send_ui_event, tasks::TaskKind, ui::theme as ui_theme,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubmoduleDialogState {
    pub(crate) items: Vec<SubmoduleInfo>,
    pub(crate) remote_statuses: BTreeMap<String, SubmoduleRemoteSyncStatus>,
    pub(crate) loading: bool,
    pub(crate) remote_loading: bool,
    pub(crate) loaded: bool,
    pub(crate) request_id: u64,
    pub(crate) remote_request_id: u64,
    pub(crate) error: Option<String>,
    pub(crate) remote_error: Option<String>,
}

impl SubmoduleDialogState {
    pub(crate) fn invalidate(&mut self) {
        self.items.clear();
        self.remote_statuses.clear();
        self.loading = false;
        self.remote_loading = false;
        self.loaded = false;
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.remote_request_id = self.remote_request_id.wrapping_add(1).max(1);
        self.error = None;
        self.remote_error = None;
    }
}

pub(crate) fn submodule_request_matches(
    state: &SubmoduleDialogState,
    repository_load_id: u64,
    load_id: u64,
    request_id: u64,
) -> bool {
    load_id == repository_load_id && request_id == state.request_id
}

pub(crate) fn submodule_remote_request_matches(
    state: &SubmoduleDialogState,
    repository_load_id: u64,
    load_id: u64,
    request_id: u64,
) -> bool {
    load_id == repository_load_id && request_id == state.remote_request_id
}

pub(crate) fn operation_refreshes_submodule_dialog(message: &str) -> bool {
    message == "子模块已同步记录版本"
        || message == "子模块已更新到远端最新"
        || (message.starts_with("子模块 ") && message.ends_with(" 已更新到远端最新"))
}

impl RepositoryView {
    pub(crate) fn open_submodule_manager(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.active_dialog = Some(DialogState::SubmoduleManager);
        self.status = "子模块已打开".into();
        self.last_error = None;
        if !self.submodule_dialog.loaded && !self.submodule_dialog.loading {
            self.load_submodules();
        }
    }

    pub(crate) fn load_submodules(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        let request_id = {
            let state = &mut self.submodule_dialog;
            state.request_id = state.request_id.wrapping_add(1).max(1);
            state.remote_request_id = state.remote_request_id.wrapping_add(1).max(1);
            state.loading = true;
            state.remote_loading = false;
            state.remote_statuses.clear();
            state.error = None;
            state.remote_error = None;
            state.request_id
        };
        self.status = "正在加载子模块列表".into();

        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<Vec<SubmoduleInfo>> {
                let repo = Repository::open(repo_path)?;
                service.submodules(&repo)
            })();
            match result {
                Ok(items) => send_ui_event(
                    &tx,
                    UiEvent::SubmodulesLoaded {
                        tab_id,
                        items,
                        load_id,
                        request_id,
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::SubmodulesLoadFailed {
                        tab_id,
                        error: err.to_string(),
                        load_id,
                        request_id,
                    },
                ),
            }
        });
    }

    pub(crate) fn load_submodule_remote_statuses(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        if !self.submodule_dialog.loaded || self.submodule_dialog.items.is_empty() {
            return;
        }

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        let request_id = {
            let state = &mut self.submodule_dialog;
            state.remote_request_id = state.remote_request_id.wrapping_add(1).max(1);
            state.remote_loading = true;
            state.remote_error = None;
            state.remote_statuses = state
                .items
                .iter()
                .map(|module| (module.name.clone(), SubmoduleRemoteSyncStatus::Checking))
                .collect();
            state.remote_request_id
        };
        self.status = "正在检查子模块远端状态".into();

        self.tasks.spawn(TaskKind::Long, move || {
            let result = (|| -> khaslana::Result<Vec<(String, SubmoduleRemoteSyncStatus)>> {
                let repo = Repository::open(repo_path)?;
                service.submodule_remote_sync_statuses(&repo)
            })();
            match result {
                Ok(statuses) => send_ui_event(
                    &tx,
                    UiEvent::SubmoduleRemoteStatusesLoaded {
                        tab_id,
                        statuses,
                        load_id,
                        request_id,
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::SubmoduleRemoteStatusesLoadFailed {
                        tab_id,
                        error: err.to_string(),
                        load_id,
                        request_id,
                    },
                ),
            }
        });
    }

    pub(crate) fn update_submodules(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.with_repo_keep_dialog_blocking(
            "子模块已同步记录版本",
            move |service, repo| service.update_submodules(repo),
        );
    }

    pub(crate) fn update_submodules_to_remote_latest(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.with_repo_keep_dialog_blocking(
            "子模块已更新到远端最新",
            move |service, repo| service.update_submodules_to_remote_latest(repo),
        );
    }

    pub(crate) fn update_submodule_to_remote_latest(&mut self, name: String) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        let label = format!("子模块 {name} 已更新到远端最新");
        self.with_repo_keep_dialog_owned_blocking(label, move |service, repo| {
            service.update_submodule_to_remote_latest(repo, &name)
        });
    }

    pub(crate) fn render_submodule_manager_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = &self.submodule_dialog;
        let rows = if state.loading && !state.loaded {
            vec![placeholder_row("正在加载子模块列表...").into_any_element()]
        } else if let Some(error) = state.error.as_ref() {
            vec![self.submodule_dialog_placeholder(format!("子模块列表加载失败：{error}"))]
        } else if state.loaded && state.items.is_empty() {
            vec![placeholder_row("当前仓库没有子模块").into_any_element()]
        } else {
            state
                .items
                .iter()
                .map(|module| self.submodule_dialog_row(module, cx).into_any_element())
                .collect::<Vec<_>>()
        };
        let can_update = state.loaded && !state.items.is_empty() && !self.busy && !state.loading;

        div()
            .id("dialog-子模块")
            .w(px(940.0))
            .max_h(px(640.0))
            .p_4()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .cursor(CursorStyle::Arrow)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("子模块"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.button(
                                "刷新",
                                !self.busy && !state.loading,
                                |this, _, _| this.load_submodules(),
                                cx,
                            ))
                            .child(self.button(
                                "同步记录版本",
                                can_update,
                                |this, _, _| this.update_submodules(),
                                cx,
                            ))
                            .child(self.primary_button(
                                "更新到远端最新",
                                can_update,
                                |this, _, _| this.update_submodules_to_remote_latest(),
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(if state.remote_loading {
                        "列表仅在打开弹窗时读取；正在后台检查子模块相对远端分支的超前/落后状态。"
                    } else {
                        "列表仅在打开弹窗时读取；同步记录版本会检出父仓库记录的提交，更新到远端最新会修改父仓库子模块指针，需要后续提交该变更。"
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h(px(0.0))
                    .max_h(px(430.0))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .rounded_sm()
                    .child(self.submodule_dialog_header())
                    .child({
                        let handle = self.scroll_handle("submodule-manager-list");
                        let content = div()
                            .id("submodule-manager-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_0()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&handle)
                            .children(rows)
                            .into_any_element();
                        scrollable_frame_when(
                            "submodule-manager-list",
                            ScrollbarMode::Vertical,
                            content,
                            handle,
                            !state.items.is_empty(),
                            cx,
                        )
                    }),
            )
            .child(div().flex().justify_end().child(self.button(
                "关闭",
                !self.busy,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }

    fn submodule_dialog_placeholder(&self, text: String) -> gpui::AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .px_3()
            .py_4()
            .text_size(px(12.0))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(text)
            .into_any_element()
    }

    fn submodule_dialog_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .justify_center()
                    .child("路径"),
            )
            .child(div().flex_none().flex().justify_center().child("状态"))
            .child(
                div()
                    .flex_none()
                    .w(px(86.0))
                    .flex()
                    .justify_center()
                    .child("目标"),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(86.0))
                    .flex()
                    .justify_center()
                    .child("当前"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .justify_center()
                    .child("URL"),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(92.0))
                    .flex()
                    .justify_center()
                    .child("操作"),
            )
    }

    fn submodule_dialog_row(
        &self,
        module: &SubmoduleInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target = module.index_id.as_deref().map(short_oid).unwrap_or("-");
        let current = module.workdir_id.as_deref().map(short_oid).unwrap_or("-");
        let url = module.url.as_deref().unwrap_or("未配置 URL");
        let module_name = module.name.clone();
        let remote_status = self.submodule_dialog.remote_statuses.get(&module.name);
        let (status_label, status_tone) = submodule_status_display(module, remote_status);
        div()
            .id(format!("submodule-manager-row-{}", module.path.display()))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(module.path.display().to_string()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(module.name.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .child(submodule_status_pill(status_label, status_tone)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(86.0))
                    .truncate()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(target.to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(86.0))
                    .truncate()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(current.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(url.to_string()),
            )
            .child(div().flex_none().w(px(92.0)).child(self.button(
                "更新最新",
                !self.busy,
                move |this, _, _| this.update_submodule_to_remote_latest(module_name.clone()),
                cx,
            )))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubmoduleStatusTone {
    Ready,
    Info,
    Warning,
    Danger,
    Muted,
}

pub(crate) fn submodule_status_display(
    module: &SubmoduleInfo,
    remote_status: Option<&SubmoduleRemoteSyncStatus>,
) -> (String, SubmoduleStatusTone) {
    if let Some(label) = submodule_local_issue_label(&module.status) {
        return (label.to_string(), SubmoduleStatusTone::Warning);
    }

    match remote_status {
        Some(SubmoduleRemoteSyncStatus::UpToDate) => {
            ("远端同步".to_string(), SubmoduleStatusTone::Ready)
        }
        Some(SubmoduleRemoteSyncStatus::Behind(behind)) => {
            (format!("落后 {behind}"), SubmoduleStatusTone::Info)
        }
        Some(SubmoduleRemoteSyncStatus::Ahead(ahead)) => {
            (format!("超前 {ahead}"), SubmoduleStatusTone::Warning)
        }
        Some(SubmoduleRemoteSyncStatus::Diverged { ahead, behind }) => (
            format!("分叉 {ahead}/{behind}"),
            SubmoduleStatusTone::Danger,
        ),
        Some(SubmoduleRemoteSyncStatus::Unavailable(_)) => {
            ("远端未知".to_string(), SubmoduleStatusTone::Muted)
        }
        Some(SubmoduleRemoteSyncStatus::Unknown | SubmoduleRemoteSyncStatus::Checking) | None => {
            let ready = module.status.is_ready();
            (
                module.status.label().to_string(),
                if ready {
                    SubmoduleStatusTone::Ready
                } else {
                    SubmoduleStatusTone::Warning
                },
            )
        }
    }
}

fn submodule_local_issue_label(status: &SubmoduleState) -> Option<&'static str> {
    if !status.initialized {
        Some("未初始化")
    } else if !status.checked_out {
        Some("未检出")
    } else if status.workdir_modified {
        Some("有改动")
    } else if status.workdir_untracked {
        Some("有未跟踪文件")
    } else {
        None
    }
}

fn submodule_status_pill(label: String, tone: SubmoduleStatusTone) -> impl IntoElement {
    let (bg, border, text) = match tone {
        SubmoduleStatusTone::Ready => (
            ui_theme::COLOR_SUCCESS,
            ui_theme::FEEDBACK_SUCCESS_BORDER,
            ui_theme::FEEDBACK_SUCCESS_TEXT,
        ),
        SubmoduleStatusTone::Info => (
            ui_theme::ACCENT,
            ui_theme::FEEDBACK_INFO_BORDER,
            ui_theme::PRIMARY,
        ),
        SubmoduleStatusTone::Warning => (
            ui_theme::COLOR_WARNING,
            ui_theme::FEEDBACK_WARNING_BORDER,
            ui_theme::COLOR_WARNING_FOREGROUND,
        ),
        SubmoduleStatusTone::Danger => (
            ui_theme::COLOR_ERROR,
            ui_theme::DESTRUCTIVE,
            ui_theme::COLOR_ERROR_FOREGROUND,
        ),
        SubmoduleStatusTone::Muted => (
            ui_theme::ACCENT,
            ui_theme::BORDER,
            ui_theme::MUTED_FOREGROUND,
        ),
    };
    div()
        .flex_none()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .child(label)
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

#[cfg(test)]
#[path = "tests/submodule_view.rs"]
mod tests;
