// 操作调度模块：后台 Git 任务的派发与生命周期管理。
//
// 这里集中了 RepositoryView 的「打开仓库 → 设置 busy/遮罩 → 后台线程跑 Git →
// 通过 UiEvent 回 UI」的调度骨架，从 main.rs 抽出以降低主文件体积。
// 方法仍挂在 `impl RepositoryView` 上，通过 `self.<method>()` 跨文件调用。
//
// 纯位置搬移，未改变任何方法体逻辑。两个零调用的死代码包装器
// （with_repo_keep_dialog_blocking / with_repo_keep_dialog_owned_blocking）
// 在搬移时一并删除。

use std::time::Instant;

use git2::Repository;

use khaslana::{GitService, RepositorySnapshot};

use crate::{
    OperationBlocker, OperationKind, RepoTabId, RepositoryLoading, RepositoryView, TaskKind,
    UiEvent, conflicts, send_ui_event,
};

impl RepositoryView {
    pub(crate) fn with_repo<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_with_blocker(label, OperationBlocker::None, f);
    }

    pub(crate) fn with_repo_blocking<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_with_blocker(label, OperationBlocker::Modal, f);
    }

    fn with_repo_with_blocker<F>(&mut self, label: &'static str, blocker: OperationBlocker, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let service = self.service_for_tab(tab_id);
        let snapshot_service = service.clone();
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            started_message_for_label(label),
            blocker,
            move || {
                let mut repo = Repository::open(path)?;
                match f(service, &mut repo) {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: label.to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err(err) => {
                        let snapshot = snapshot_service.snapshot_after_operation(&mut repo).ok();
                        if let Some(snapshot) = snapshot
                            && !snapshot.conflicts.is_empty()
                        {
                            return Ok(UiEvent::OperationFinished {
                                tab_id: Some(tab_id),
                                message: conflicts::conflict_status_message(
                                    label,
                                    snapshot.conflicts.len(),
                                ),
                                snapshot: Some(snapshot),
                                diff: None,
                            });
                        }
                        Err(err)
                    }
                }
            },
        );
    }

    pub(crate) fn with_repo_keep_dialog<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label.to_string(), OperationBlocker::None, f)
    }

    pub(crate) fn with_repo_keep_dialog_blocking<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label.to_string(), OperationBlocker::Modal, f)
    }

    pub(crate) fn with_repo_keep_dialog_owned_blocking<F>(&mut self, label: String, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label, OperationBlocker::Modal, f)
    }

    fn with_repo_keep_dialog_owned_with_blocker<F>(
        &mut self,
        label: String,
        blocker: OperationBlocker,
        f: F,
    ) where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        if self.busy {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let service = self.service_for_tab(tab_id);
        let started = started_message_for_label_text(&label);
        self.apply_status_event(Some(tab_id), |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = blocker;
            this.operation_blocker_started = if blocker.blocks_interaction() {
                Some(Instant::now())
            } else {
                None
            };
            this.operation_kind = OperationKind::from_message(&started);
            this.status = started.clone();
            this.last_error = None;
        });
        let tx = self.tx.clone();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id: Some(tab_id),
                message: started,
            },
        );
        self.tasks.spawn(TaskKind::Long, move || {
            match Repository::open(path)
                .map_err(khaslana::GitError::from)
                .and_then(|mut repo| f(service, &mut repo))
            {
                Ok(snapshot) => send_ui_event(
                    &tx,
                    UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: label,
                        snapshot: Some(snapshot),
                        diff: None,
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    pub(crate) fn spawn_operation_for_tab<F>(
        &mut self,
        tab_id: Option<RepoTabId>,
        started: &'static str,
        f: F,
    ) where
        F: FnOnce() -> khaslana::Result<UiEvent> + Send + 'static,
    {
        self.spawn_operation_for_tab_with_blocker(tab_id, started, OperationBlocker::None, f);
    }

    pub(crate) fn spawn_operation_for_tab_with_blocker<F>(
        &mut self,
        tab_id: Option<RepoTabId>,
        started: &'static str,
        blocker: OperationBlocker,
        f: F,
    ) where
        F: FnOnce() -> khaslana::Result<UiEvent> + Send + 'static,
    {
        if let Some(tab_id) = tab_id
            && self.tab(tab_id).is_none()
        {
            return;
        }
        let busy = tab_id
            .and_then(|id| self.tab(id).map(|tab| tab.busy))
            .unwrap_or(self.busy);
        if busy {
            self.apply_status_event(tab_id, |this| {
                this.last_error = Some("已有操作正在运行".into());
            });
            return;
        }
        self.close_popups();
        self.apply_status_event(tab_id, |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = blocker;
            this.operation_blocker_started = if blocker.blocks_interaction() {
                Some(Instant::now())
            } else {
                None
            };
            this.operation_kind = OperationKind::from_message(started);
            this.status = started.to_string();
            this.last_error = None;
        });
        let tx = self.tx.clone();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id,
                message: started.to_string(),
            },
        );
        self.tasks.spawn(TaskKind::Long, move || match f() {
            Ok(event) => {
                send_ui_event(&tx, event);
            }
            Err(err) => {
                send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id,
                        error: err.to_string(),
                    },
                );
            }
        });
    }
}

fn started_message_for_label(label: &'static str) -> &'static str {
    match label {
        "拉取远程引用完成" => "正在拉取远程引用",
        "拉取完成" => "正在拉取",
        "推送完成" => "正在推送",
        "提交并推送完成" => "正在提交并推送",
        "远端分支已拉取到本地" => "正在拉取远端分支",
        "克隆完成" => "正在克隆仓库",
        "已刷新" => "正在刷新仓库",
        "合并完成" => "正在合并分支",
        "变基完成" => "正在变基分支",
        "变基已中止" => "正在中止变基",
        "变基拉取完成" => "正在变基拉取",
        "切换分支完成" => "正在切换分支",
        "提交完成" => "正在提交",
        "分支已创建" => "正在创建分支",
        "分支已重命名" => "正在重命名分支",
        "分支已删除" => "正在删除分支",
        "检出标签完成" => "正在检出标签",
        "应用贮藏完成" => "正在应用贮藏",
        "弹出贮藏完成" => "正在弹出贮藏",
        "分支已重置" => "正在重置分支",
        "回滚提交完成" => "正在回滚提交",
        "远端已更新" => "正在更新远端",
        "远端已新增" => "正在新增远端",
        "远端已删除" => "正在删除远端",
        "远端已刷新" => "正在刷新远端",
        "冲突已标记为解决" => "正在标记冲突解决",
        "子模块已同步记录版本" => "正在同步子模块记录版本",
        "子模块已更新到远端最新" => "正在更新子模块到远端最新",
        _ => label,
    }
}

fn started_message_for_label_text(label: &str) -> String {
    if label.starts_with("子模块 ") && label.ends_with(" 已更新到远端最新") {
        return "正在更新子模块到远端最新".to_string();
    }
    match label {
        "子模块已同步记录版本" => "正在同步子模块记录版本".to_string(),
        "子模块已更新到远端最新" => "正在更新子模块到远端最新".to_string(),
        _ => label.to_string(),
    }
}
