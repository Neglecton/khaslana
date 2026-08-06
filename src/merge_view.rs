//! IDEA 风格的普通合并状态条、完成和中止交互。

use gpui::{Context, IntoElement, div, prelude::*, px};
use khaslana::CommitMessage;

use crate::ui::components::dialog_actions;
use crate::ui::theme::rgb;
use crate::{DialogState, RepositoryView, ui::theme as ui_theme};

pub(crate) fn merge_banner_message(conflict_count: usize) -> String {
    if conflict_count > 0 {
        format!("合并进行中 · {conflict_count} 个冲突待解决")
    } else {
        "合并进行中 · 冲突已全部解决，请检查结果并完成合并".into()
    }
}

pub(crate) fn merge_can_finish(
    merge_in_progress: bool,
    conflict_count: usize,
    busy: bool,
    message: &str,
) -> bool {
    merge_in_progress && conflict_count == 0 && !busy && !message.trim().is_empty()
}

pub(crate) fn merge_commit_button_label(merge_in_progress: bool) -> &'static str {
    if merge_in_progress {
        "完成合并"
    } else {
        "提交"
    }
}

pub(crate) fn merge_allows_disruptive_action(merge_in_progress: bool) -> bool {
    !merge_in_progress
}

fn merge_message_update(
    was_in_progress: bool,
    merge_in_progress: bool,
    merge_message: Option<&str>,
) -> Option<String> {
    if !was_in_progress && merge_in_progress {
        merge_message.map(str::to_string)
    } else if was_in_progress && !merge_in_progress {
        Some(String::new())
    } else {
        None
    }
}

impl RepositoryView {
    pub(crate) fn merge_in_progress(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.merge_in_progress)
    }

    /// 合并会话期间阻止启动会改变 HEAD 或覆盖工作区的其他操作。
    pub(crate) fn ensure_no_merge_in_progress(&mut self, action: &str) -> bool {
        if merge_allows_disruptive_action(self.merge_in_progress()) {
            return true;
        }
        self.last_error = Some(format!(
            "合并正在进行，不能{action}；请先完成合并或中止合并"
        ));
        false
    }

    pub(crate) fn sync_merge_message_transition(
        &mut self,
        was_in_progress: bool,
        merge_in_progress: bool,
        merge_message: Option<String>,
    ) {
        if let Some(message) = merge_message_update(
            was_in_progress,
            merge_in_progress,
            merge_message.as_deref(),
        ) {
            if merge_in_progress {
                self.commit_message.set_value(message);
            } else {
                self.commit_message.clear();
                self.scroll_handle("commit-message-input-scroll")
                    .set_offset(gpui::point(px(0.0), px(0.0)));
            }
        }
    }

    pub(crate) fn finish_merge(&mut self) {
        let message = self.commit_message.value.trim().to_string();
        if message.is_empty() {
            self.last_error = Some("需要填写合并提交信息".into());
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.conflicts.is_empty())
        {
            self.last_error = Some("仍有冲突文件未解决，不能完成合并".into());
            return;
        }
        self.with_repo_blocking("合并已完成", move |service, repo| {
            service.finish_merge(repo, &CommitMessage::new(message))
        });
    }

    pub(crate) fn open_abort_merge_confirm_dialog(&mut self) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmAbortMerge);
        self.last_error = None;
    }

    pub(crate) fn confirm_abort_merge(&mut self) {
        self.close_dialog();
        self.with_repo_blocking("合并已中止", move |service, repo| {
            service.abort_merge(repo)
        });
    }

    pub(crate) fn render_merge_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let snapshot = self.snapshot.as_ref()?;
        if !snapshot.merge_in_progress {
            return None;
        }

        let conflict_count = snapshot.conflicts.len();
        let can_finish = merge_can_finish(
            snapshot.merge_in_progress,
            conflict_count,
            self.busy,
            &self.commit_message.value,
        );
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(ui_theme::BORDER))
                .bg(rgb(ui_theme::COLOR_WARNING))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .child(merge_banner_message(conflict_count)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(conflict_count == 0, |this| {
                            this.child(self.primary_button(
                                "完成合并",
                                can_finish,
                                |this, _, _| this.finish_merge(),
                                cx,
                            ))
                        })
                        .child(self.button(
                            "中止合并",
                            !self.busy,
                            |this, _, _| this.open_abort_merge_confirm_dialog(),
                            cx,
                        )),
                ),
        )
    }

    pub(crate) fn render_confirm_abort_merge_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("中止合并", cx)
            .w(px(520.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("确定要中止当前合并吗？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_ERROR_FOREGROUND))
                    .child("本次合并产生的内容以及已经完成的冲突解决结果都会被丢弃。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "中止合并",
                        !self.busy,
                        |this, _, _| this.confirm_abort_merge(),
                        cx,
                    )),
            )
    }
}

#[cfg(test)]
#[path = "tests/merge_view.rs"]
mod tests;
