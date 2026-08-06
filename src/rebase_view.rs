//! 变基（Rebase）UI 模块。
//!
//! 变基通过 `with_repo` 后台执行，冲突走现有冲突工作台。
//! 当 `snapshot.rebase_in_progress` 为真时，在工作区顶部显示变基状态条，
//! 提供"继续变基 / 跳过此提交 / 中止"三个动作。

use gpui::{Context, IntoElement, div, prelude::*, px};
use khaslana::{BranchName, GitError, RebaseOutcome};

use crate::ui::theme::rgb;
use crate::{RepositoryView, ui::theme as ui_theme};

impl RepositoryView {
    /// 把当前分支变基到指定分支之上。由侧边栏右键菜单调用。
    pub(crate) fn rebase_branch(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("开始变基") {
            return;
        }
        self.with_repo_blocking("变基完成", move |service, repo| {
            match service.rebase_branch(repo, &BranchName::new(name))? {
                RebaseOutcome::Completed(snapshot) => Ok(snapshot),
                RebaseOutcome::Conflicts { snapshot, .. } => {
                    // 通过返回冲突错误让 with_repo 自动展示冲突工作台
                    Err(GitError::Conflicts(snapshot.conflicts))
                }
            }
        });
    }

    /// 变基遇到冲突后，用户已解决所有冲突，继续重放下一个提交。
    pub(crate) fn rebase_continue(&mut self) {
        self.with_repo_blocking("变基完成", move |service, repo| {
            match service.rebase_continue(repo)? {
                RebaseOutcome::Completed(snapshot) => Ok(snapshot),
                RebaseOutcome::Conflicts { snapshot, .. } => {
                    Err(GitError::Conflicts(snapshot.conflicts))
                }
            }
        });
    }

    /// 跳过当前冲突提交。
    pub(crate) fn rebase_skip(&mut self) {
        self.with_repo_blocking("变基完成", move |service, repo| {
            match service.rebase_skip(repo)? {
                RebaseOutcome::Completed(snapshot) => Ok(snapshot),
                RebaseOutcome::Conflicts { snapshot, .. } => {
                    Err(GitError::Conflicts(snapshot.conflicts))
                }
            }
        });
    }

    /// 中止变基，回到变基前状态。
    pub(crate) fn rebase_abort(&mut self) {
        self.with_repo_blocking("变基已中止", move |service, repo| {
            service.rebase_abort(repo)
        });
    }

    /// 渲染变基状态条。当 `snapshot.rebase_in_progress` 为真时显示，
    /// 提供"继续变基"、"跳过此提交"和"中止"按钮。
    pub(crate) fn render_rebase_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let snapshot = self.snapshot.as_ref()?;
        if !snapshot.rebase_in_progress {
            return None;
        }

        let has_conflicts = !snapshot.conflicts.is_empty();
        let busy = self.busy;
        // 继续变基仅在无冲突且非忙时可用
        let can_continue = !has_conflicts && !busy;
        let can_skip = !busy;
        let can_abort = !busy;

        let message = if has_conflicts {
            "变基进行中 · 存在冲突待解决"
        } else {
            "变基进行中"
        };

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
                        .child(message),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.button(
                            "继续变基",
                            can_continue,
                            |this, _, _| {
                                this.rebase_continue();
                            },
                            cx,
                        ))
                        .when(has_conflicts, |this| {
                            this.child(self.button(
                                "跳过此提交",
                                can_skip,
                                |this, _, _| this.rebase_skip(),
                                cx,
                            ))
                        })
                        .child(self.button(
                            "中止",
                            can_abort,
                            |this, _, _| {
                                this.rebase_abort();
                            },
                            cx,
                        )),
                ),
        )
    }
}
