//! RepositoryView 的仓库加载、远端、分支、标签与提交级操作。

use crate::*;

impl RepositoryView {
    pub(crate) fn open_repo(&mut self, path: PathBuf) {
        if Repository::open(&path).is_err() {
            self.status = "打开仓库失败".to_string();
            self.last_error = Some("该目录不是 Git 仓库".to_string());
            return;
        }
        // 记录最近打开时间，供仓库切换下拉排序。
        let _ = self.storage.upsert_recent_repo(&path);
        let tab_id = self.ensure_tab_for_path(path.clone());
        self.queue_repository_load(
            tab_id,
            path,
            "正在打开仓库",
            "仓库已打开",
            LoadPriority::User,
        );
    }

    pub(crate) fn clone_repo(&mut self) {
        let url = self.clone_url.value.trim().to_string();
        let path_text = self.clone_path.value.trim().to_string();
        if url.is_empty() || path_text.is_empty() {
            self.last_error = Some("需要填写远程仓库 URL 和克隆到父文件夹".into());
            return;
        }
        if infer_clone_directory_name(&url).is_none() {
            self.last_error = Some("无法从远程仓库 URL 推导仓库文件夹名".into());
            return;
        };
        let Some(path) = infer_clone_target_path(&url, &path_text) else {
            self.last_error = Some("需要填写远程仓库 URL 和克隆到父文件夹".into());
            return;
        };
        if path.exists() {
            self.last_error = Some("目标仓库文件夹已存在".into());
            return;
        }
        let key = normalize_repo_path(&path);
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.path_key().as_deref() == Some(key.as_str()))
        {
            let previous_mode = self.current_tab_main_mode();
            self.active_tab = Some(tab.id);
            self.inherit_main_mode(previous_mode);
            self.last_error = Some("该仓库已经打开".into());
            self.save_session();
            return;
        }

        let tab_id = self.ensure_tab_for_path(path.clone());
        let service = self.service_for_tab(tab_id);
        let options = khaslana::CloneOptions {
            recursive_submodules: self.clone_recursive_submodules,
        };
        self.spawn_operation_for_tab(Some(tab_id), "正在克隆仓库", move || {
            service
                .clone_repo_with_options(&url, &RepoPath::new(path), options)
                .map(|snapshot| UiEvent::OperationFinished {
                    tab_id: Some(tab_id),
                    message: "克隆完成".to_string(),
                    snapshot: Some(snapshot),
                    diff: None,
                })
        });
    }

    pub(crate) fn refresh(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.queue_repository_load(tab_id, path, "正在刷新仓库", "已刷新", LoadPriority::User);
    }

    pub(crate) fn queue_repository_load(
        &mut self,
        tab_id: RepoTabId,
        path: PathBuf,
        started: &'static str,
        finished: &'static str,
        priority: LoadPriority,
    ) {
        let load_id = {
            let Some(tab) = self.tab_mut(tab_id) else {
                return;
            };
            let load_id = tab.repository_load_id.wrapping_add(1);
            tab.repository_load_id = load_id;
            tab.repo_path = Some(path.clone());
            tab.busy = true;
            tab.operation_blocker = OperationBlocker::None;
            tab.operation_blocker_started = None;
            tab.operation_kind = OperationKind::from_message(started);
            tab.loading = RepositoryLoading::default();
            tab.branch_sync_status = None;
            tab.branch_sync_loading = false;
            tab.branch_sync_request_id = tab.branch_sync_request_id.wrapping_add(1).max(1);
            tab.submodule_dialog.invalidate();
            tab.status = started.to_string();
            tab.last_error = None;
            load_id
        };
        self.close_popups();
        self.save_session();
        self.repository_load_queue
            .retain(|request| request.tab_id != tab_id);
        let request = RepositoryLoadRequest {
            tab_id,
            path,
            started,
            finished,
        };
        if priority == LoadPriority::User {
            self.repository_load_queue.push_front(request);
        } else {
            self.repository_load_queue.push_back(request);
        }
        self.start_queued_repository_loads();
        if let Some(tab) = self.tab(tab_id)
            && tab.repository_load_id == load_id
            && self
                .repository_load_queue
                .iter()
                .any(|request| request.tab_id == tab_id)
        {
            self.apply_status_event(Some(tab_id), |this| {
                this.status = "等待加载仓库".to_string();
            });
        }
    }

    pub(crate) fn start_queued_repository_loads(&mut self) {
        while self.active_repository_loads < MAX_CONCURRENT_REPO_LOADS {
            let Some(request) = self.repository_load_queue.pop_front() else {
                break;
            };
            if self.tab(request.tab_id).is_none() {
                continue;
            }
            self.active_repository_loads += 1;
            self.spawn_repository_load(request);
        }
    }

    fn spawn_repository_load(&mut self, request: RepositoryLoadRequest) {
        let tab_id = request.tab_id;
        let path = request.path;
        let started = request.started;
        let finished = request.finished;
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self
            .tab(tab_id)
            .map(|tab| tab.repository_load_id)
            .unwrap_or_default();
        let load_started = Instant::now();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id: Some(tab_id),
                message: started.to_string(),
            },
        );
        self.tasks.spawn(TaskKind::Short, move || {
            let stage_started = Instant::now();
            let repo_path = RepoPath::new(path);
            let fast = match service.open_fast(&repo_path) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::OperationFailed {
                            tab_id: Some(tab_id),
                            error: err.to_string(),
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            };
            perf_log(
                "repo.open_fast",
                stage_started,
                format!("tab={} branches={}", tab_id.0, fast.branches.len()),
            );
            send_ui_event(
                &tx,
                UiEvent::RepositoryFastLoaded {
                    tab_id,
                    message: "本地分支已加载，正在加载仓库详情".to_string(),
                    snapshot: fast,
                    load_id,
                },
            );

            let mut repo = match Repository::open(&repo_path.0) {
                Ok(repo) => repo,
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            };

            let stage_started = Instant::now();
            match service.snapshot_metadata(&mut repo) {
                Ok(snapshot) => {
                    perf_log(
                        "repo.metadata",
                        stage_started,
                        format!(
                            "tab={} branches={} remotes={} tags={} stashes={} conflicts={}",
                            tab_id.0,
                            snapshot.branches.len(),
                            snapshot.remotes.len(),
                            snapshot.tags.len(),
                            snapshot.stashes.len(),
                            snapshot.conflicts.len()
                        ),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryMetadataLoaded {
                            tab_id,
                            message: "仓库信息已加载".to_string(),
                            snapshot,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            }

            let stage_started = Instant::now();
            match service.status_fast(&repo) {
                Ok(changes) => {
                    perf_log(
                        "repo.status_fast",
                        stage_started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFastLoaded {
                            tab_id,
                            message: "快速变更已加载，正在补全未跟踪文件".to_string(),
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            }

            let stage_started = Instant::now();
            match service.status_full(&repo) {
                Ok(changes) => {
                    perf_log(
                        "repo.status_full",
                        stage_started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFullLoaded {
                            tab_id,
                            message: finished.to_string(),
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
            perf_log(
                "repo.load_total",
                load_started,
                format!("tab={} load_id={}", tab_id.0, load_id),
            );
            send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
        });
    }

    pub(crate) fn load_full_status_for_tab(
        &self,
        tab_id: RepoTabId,
        path: PathBuf,
        load_id: u64,
        message: String,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<Vec<khaslana::WorktreeChange>> {
                let repo = Repository::open(path)?;
                service.status_full(&repo)
            })();
            match result {
                Ok(changes) => {
                    perf_log(
                        "repo.status_full.operation",
                        started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFullLoaded {
                            tab_id,
                            message,
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn prepare_branch_sync_status_request(
        &mut self,
    ) -> Option<(RepoTabId, PathBuf, String, u64, u64)> {
        let tab_id = self.active_tab_id()?;
        let path = self.repo_path.clone()?;
        let Some(remote) = self.current_remote() else {
            self.branch_sync_status = None;
            self.branch_sync_loading = false;
            self.branch_sync_request_id = self.branch_sync_request_id.wrapping_add(1).max(1);
            return None;
        };
        let load_id = self.repository_load_id;
        self.branch_sync_request_id = self.branch_sync_request_id.wrapping_add(1).max(1);
        self.branch_sync_loading = true;
        Some((tab_id, path, remote, load_id, self.branch_sync_request_id))
    }

    pub(crate) fn load_branch_sync_status_for_tab(
        &self,
        tab_id: RepoTabId,
        path: PathBuf,
        remote: String,
        load_id: u64,
        request_id: u64,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<Option<BranchSyncStatus>> {
                let repo = Repository::open(path)?;
                service.branch_sync_status(&repo, &RemoteName::new(remote))
            })();
            match result {
                Ok(status) => {
                    perf_log(
                        "branch.sync_status",
                        started,
                        format!(
                            "tab={} ahead={} behind={}",
                            tab_id.0,
                            status.as_ref().map(|status| status.ahead).unwrap_or(0),
                            status.as_ref().map(|status| status.behind).unwrap_or(0)
                        ),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::BranchSyncStatusLoaded {
                            tab_id,
                            status,
                            load_id,
                            request_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::BranchSyncStatusFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                            request_id,
                        },
                    );
                }
            }
        });
    }

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

    fn with_repo_keep_dialog<F>(&mut self, label: &'static str, f: F)
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

    pub(crate) fn current_remote(&self) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        self.selected_remote
            .as_ref()
            .filter(|remote| snapshot.remotes.iter().any(|info| info.name == **remote))
            .cloned()
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name.as_str() == "origin")
                    .map(|remote| remote.name.clone())
            })
            .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()))
    }

    pub(crate) fn sync_selected_remote(&mut self, snapshot: &RepositorySnapshot) {
        if snapshot.remotes.is_empty() {
            self.selected_remote = None;
            return;
        }

        if self
            .selected_remote
            .as_ref()
            .is_some_and(|remote| snapshot.remotes.iter().any(|info| info.name == *remote))
        {
            return;
        }

        self.selected_remote = snapshot
            .remotes
            .iter()
            .find(|remote| remote.name.as_str() == "origin")
            .map(|remote| remote.name.clone())
            .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()));
    }

    pub(crate) fn fetch(&mut self) {
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        self.with_repo("拉取远程引用完成", move |service, repo| {
            service.fetch(repo, &RemoteName::new(remote))
        });
    }

    pub(crate) fn refresh_remote(&mut self, remote: String) {
        self.remote_context_menu = None;
        self.selected_remote = Some(remote.clone());
        self.with_repo("远端已刷新", move |service, repo| {
            service.refresh(repo, Some(&RemoteName::new(remote)))
        });
    }

    pub(crate) fn open_remote_branch_operation(&mut self, kind: RemoteBranchOperationKind) {
        if matches!(
            kind,
            RemoteBranchOperationKind::Pull | RemoteBranchOperationKind::Push
        ) && !self.ensure_no_merge_in_progress("拉取或推送")
        {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let defaults = match remote_branch_dialog_defaults(snapshot, self.current_remote()) {
            Ok(defaults) => defaults,
            Err(message) => {
                self.last_error = Some(message);
                return;
            }
        };
        self.close_popups();
        self.remote_branch_operation.clear();
        self.remote_branch_operation.local_branch = Some(defaults.local_branch);
        self.remote_branch_operation.selected_remote = Some(defaults.remote);
        self.remote_branch_name.set_value(defaults.remote_branch);
        self.remote_branch_search.clear();
        self.active_dialog = Some(DialogState::RemoteBranchOperation { kind });
        self.last_error = None;
    }

    pub(crate) fn open_set_branch_upstream_dialog(&mut self, branch: String) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(local_branch) = local_branch_by_name(snapshot, &branch) else {
            self.last_error = Some(format!("本地分支不存在：{branch}"));
            return;
        };
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let remote_branch = default_remote_branch_for(local_branch, &remote);
        self.close_popups();
        self.remote_branch_operation.clear();
        self.remote_branch_operation.local_branch = Some(branch);
        self.remote_branch_operation.selected_remote = Some(remote);
        self.remote_branch_name.set_value(remote_branch);
        self.remote_branch_search.clear();
        self.active_dialog = Some(DialogState::RemoteBranchOperation {
            kind: RemoteBranchOperationKind::SetUpstream,
        });
        self.last_error = None;
    }

    pub(crate) fn select_remote_branch_operation_remote(&mut self, remote: String) {
        self.remote_branch_operation.selected_remote = Some(remote.clone());
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        let default_branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.remote_branch_operation
                    .local_branch
                    .as_deref()
                    .and_then(|name| local_branch_by_name(snapshot, name))
                    .or_else(|| remote_branch_operation::current_local_branch(snapshot))
            })
            .map(|local_branch| default_remote_branch_for(local_branch, &remote));
        if let Some(default_branch) = default_branch {
            self.remote_branch_name.set_value(default_branch);
        }
        self.last_error = None;
    }

    pub(crate) fn refresh_remote_branch_operation(&mut self) {
        let Some(remote) = self.remote_branch_operation.selected_remote.clone() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_operation.refreshing = true;
        self.with_repo_keep_dialog("拉取远程引用完成", move |service, repo| {
            service.fetch(repo, &RemoteName::new(remote))
        });
    }

    pub(crate) fn confirm_remote_branch_operation(&mut self, kind: RemoteBranchOperationKind) {
        if matches!(
            kind,
            RemoteBranchOperationKind::Pull | RemoteBranchOperationKind::Push
        ) && !self.ensure_no_merge_in_progress("拉取或推送")
        {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(local_branch) = self
            .remote_branch_operation
            .local_branch
            .as_deref()
            .and_then(|name| local_branch_by_name(snapshot, name))
            .or_else(|| remote_branch_operation::current_local_branch(snapshot))
            .map(|branch| branch.name.clone())
        else {
            self.last_error = Some("当前不是本地分支，无法拉取、推送或设置 upstream".into());
            return;
        };
        let Some(remote) = self.remote_branch_operation.selected_remote.clone() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let remote_branch = self.remote_branch_name.value.trim().to_string();
        if remote_branch.is_empty() {
            self.last_error = Some("需要填写远程分支".into());
            return;
        }
        if kind.requires_existing_remote_branch()
            && !remote_branch_exists(snapshot, &remote, &remote_branch)
        {
            self.last_error = Some("远端分支不存在，请点击刷新或选择已有分支".into());
            return;
        }

        let use_rebase = self.remote_branch_operation.use_rebase;
        self.active_dialog = None;
        self.remote_branch_operation.refreshing = false;
        self.remote_branch_operation.branch_dropdown_open = false;
        match kind {
            RemoteBranchOperationKind::Pull => {
                if use_rebase {
                    // 用变基代替合并
                    self.with_repo_blocking("变基拉取完成", move |service, repo| {
                        service.pull_branch_rebase(
                            repo,
                            &RemoteName::new(remote),
                            &BranchName::new(remote_branch),
                        )
                    });
                } else {
                    self.with_repo_blocking("拉取完成", move |service, repo| {
                        service.pull_branch(
                            repo,
                            &RemoteName::new(remote),
                            &BranchName::new(remote_branch),
                        )
                    });
                }
            }
            RemoteBranchOperationKind::Push => {
                self.with_repo_blocking("推送完成", move |service, repo| {
                    service.push_branch_to(
                        repo,
                        &RemoteName::new(remote),
                        &BranchName::new(local_branch),
                        &BranchName::new(remote_branch),
                        true,
                    )
                });
            }
            RemoteBranchOperationKind::SetUpstream => {
                self.with_repo("upstream 已设置", move |service, repo| {
                    service.set_branch_upstream(
                        repo,
                        &BranchName::new(local_branch),
                        &RemoteName::new(remote),
                        &BranchName::new(remote_branch),
                    )
                });
            }
        }
    }

    pub(crate) fn checkout(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("切换分支") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("切换分支完成", move |service, repo| {
            service.checkout_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn create_branch(&mut self) {
        let name = self.branch_name.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("需要填写分支名称".into());
            return;
        }
        let checkout = self.create_branch_checkout;
        self.with_repo("分支已创建", move |service, repo| {
            service.create_branch_from(repo, &BranchName::new(name), None, checkout)
        });
    }

    pub(crate) fn rename_branch(&mut self, old: String) {
        let new = self.branch_rename.value.trim().to_string();
        if new.is_empty() {
            self.last_error = Some("需要填写新的分支名称".into());
            return;
        }
        self.with_repo("分支已重命名", move |service, repo| {
            service.rename_branch(repo, &BranchName::new(old), &BranchName::new(new))
        });
    }

    pub(crate) fn delete_branch(&mut self, name: String) {
        self.with_repo("分支已删除", move |service, repo| {
            service.delete_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn merge_branch(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("再次合并") {
            return;
        }
        self.with_repo_blocking("合并操作已完成", move |service, repo| {
            service.merge_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_remote_branch(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("切换远端分支") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("远端分支已拉取到本地", move |service, repo| {
            service.checkout_remote_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_tag(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("检出标签") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("检出标签完成", move |service, repo| {
            service.checkout_tag(repo, &TagName::new(name))
        });
    }

    // ── 标签管理 ──────────────────────────────────────────────

    /// 打开创建标签对话框；`target_oid` 为 None 时目标为 HEAD。
    pub(crate) fn open_tag_form_dialog(
        &mut self,
        target_oid: Option<String>,
        target_summary: String,
    ) {
        self.close_popups();
        self.tag_name.clear();
        self.tag_message.clear();
        self.tag_annotated = true;
        self.active_dialog = Some(DialogState::TagForm {
            target_oid,
            target_summary,
        });
        self.last_error = None;
    }

    pub(crate) fn create_tag(&mut self) {
        let target_oid = match self.active_dialog.clone() {
            Some(DialogState::TagForm { target_oid, .. }) => target_oid,
            _ => return,
        };
        let name = self.tag_name.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("请填写标签名称".into());
            return;
        }
        let message = if self.tag_annotated {
            Some(self.tag_message.value.trim().to_string())
        } else {
            None
        };
        self.tag_name.clear();
        self.tag_message.clear();
        self.with_repo("标签已创建", move |service, repo| {
            service.create_tag(
                repo,
                &TagName::new(name.clone()),
                target_oid.as_deref(),
                message.as_deref(),
            )
        });
    }

    pub(crate) fn open_tag_push_dialog(&mut self, tag: String) {
        self.close_popups();
        self.tag_push_remote = self.current_remote();
        // 下拉展开态不能跨弹窗会话残留：clear/close_popups 都不覆盖这个字段，
        // 不重置的话上次展开的下拉会在重开弹窗时保持展开。
        self.active_dialog = Some(DialogState::TagPush { tag });
        self.last_error = None;
    }

    pub(crate) fn push_tag(&mut self, tag: String) {
        let Some(remote) = self
            .tag_push_remote
            .clone()
            .filter(|remote| !remote.trim().is_empty())
            .or_else(|| self.current_remote())
        else {
            self.last_error = Some("当前仓库没有远端，无法推送标签".into());
            return;
        };
        self.with_repo_blocking("标签已推送", move |service, repo| {
            service.push_tag(
                repo,
                &RemoteName::new(remote.clone()),
                &TagName::new(tag.clone()),
            )
        });
    }

    pub(crate) fn open_delete_tag_confirm(&mut self, tag: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmDeleteTag { tag });
        self.last_error = None;
    }

    pub(crate) fn delete_tag(&mut self, tag: String) {
        self.with_repo("标签已删除", move |service, repo| {
            service.delete_tag(repo, &TagName::new(tag.clone()))
        });
    }

    pub(crate) fn open_delete_remote_tag_confirm(&mut self, remote: String, tag: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmDeleteRemoteTag { remote, tag });
        self.last_error = None;
    }

    pub(crate) fn delete_remote_tag(&mut self, remote: String, tag: String) {
        self.with_repo_blocking("远端标签已删除", move |service, repo| {
            service.delete_remote_tag(
                repo,
                &RemoteName::new(remote.clone()),
                &TagName::new(tag.clone()),
            )
        });
    }

    pub(crate) fn apply_stash(&mut self, index: usize) {
        if !self.ensure_no_merge_in_progress("应用贮藏") {
            return;
        }
        self.with_repo_blocking("应用贮藏完成", move |service, repo| {
            service.apply_stash(repo, index)
        });
    }

    pub(crate) fn pop_stash(&mut self, index: usize) {
        if !self.ensure_no_merge_in_progress("弹出贮藏") {
            return;
        }
        self.with_repo_blocking("弹出贮藏完成", move |service, repo| {
            service.pop_stash(repo, index)
        });
    }

    pub(crate) fn open_reset_confirm_dialog(
        &mut self,
        oid: String,
        summary: String,
        mode: ResetMode,
    ) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmReset { oid, summary, mode });
        self.last_error = None;
    }

    pub(crate) fn open_revert_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmRevert { oid, summary });
        self.last_error = None;
    }

    pub(crate) fn open_revert_merge_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmRevertMerge { oid, summary });
        self.last_error = None;
    }

    pub(crate) fn open_uncommit_to_staged_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmUncommitToStaged { oid, summary });
        self.last_error = None;
    }

    pub(crate) fn open_discard_change_confirm_dialog(
        &mut self,
        paths: Vec<String>,
        scope: DiffScope,
        target: DiscardTarget,
    ) {
        if paths.is_empty() {
            self.last_error = Some("没有可回滚的文件".into());
            self.change_context_menu = None;
            return;
        }
        self.close_popups();
        match target {
            DiscardTarget::Single => {
                if let Some(path) = paths.first() {
                    self.select_only_change(path.clone(), scope.clone(), false);
                }
            }
            DiscardTarget::Selected | DiscardTarget::All => {
                self.clear_opposite_change_selection(&scope);
            }
        }
        self.active_dialog = Some(DialogState::ConfirmDiscardChange {
            scope,
            target,
            paths,
        });
        self.last_error = None;
    }

    /// 丢弃全部未暂存变更 — 先弹出确认弹窗
    pub(crate) fn confirm_discard_all(&mut self) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            let paths = self
                .change_indexes
                .unstaged
                .iter()
                .filter_map(|i| snapshot.changes.get(*i))
                .map(|c| c.path.clone())
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                self.open_discard_change_confirm_dialog(
                    paths,
                    DiffScope::Unstaged,
                    DiscardTarget::All,
                );
            }
        }
    }

    pub(crate) fn reset_to_commit(&mut self, oid: String, mode: ResetMode) {
        if !self.ensure_no_merge_in_progress("重置提交") {
            return;
        }
        self.with_repo_blocking("分支已重置", move |service, repo| {
            service.reset_to_commit(repo, &oid, mode)
        });
    }

    pub(crate) fn revert_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("回滚提交") {
            return;
        }
        self.with_repo_blocking("回滚提交完成", move |service, repo| {
            service.revert_commit(repo, &oid)
        });
    }

    pub(crate) fn revert_merge_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("撤销合并提交") {
            return;
        }
        self.with_repo_blocking("撤销合并完成", move |service, repo| {
            service.revert_merge_commit(repo, &oid)
        });
    }

    pub(crate) fn uncommit_to_staged(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("还原提交到暂存区") {
            return;
        }
        self.with_repo_blocking("提交已还原到暂存区", move |service, repo| {
            service.uncommit_to_staged(repo, &oid)
        });
    }

    pub(crate) fn discard_change(
        &mut self,
        paths: Vec<String>,
        scope: DiffScope,
        target: DiscardTarget,
    ) {
        if !self.ensure_no_merge_in_progress("回滚工作区更改") {
            return;
        }
        let message = match scope {
            DiffScope::Staged => match target {
                DiscardTarget::Single => "已回滚文件全部更改",
                DiscardTarget::Selected => "已回滚选定文件全部更改",
                DiscardTarget::All => "已回滚暂存区全部更改",
            },
            DiffScope::Unstaged => match target {
                DiscardTarget::Single => "已回滚未暂存更改",
                DiscardTarget::Selected => "已回滚选定未暂存更改",
                DiscardTarget::All => "已回滚修改区全部更改",
            },
        };
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.close_dialog();
        self.change_context_menu = None;
        let path_set = paths.iter().cloned().collect::<BTreeSet<_>>();
        self.change_selection
            .selected_mut(&scope)
            .retain(|path| !path_set.contains(path));
        self.clear_change_anchor_if_empty(&scope);
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");

        let service = self.service_for_tab(tab_id);
        let load_id = {
            let Some(tab) = self.tab_mut(tab_id) else {
                return;
            };
            tab.repository_load_id = tab.repository_load_id.wrapping_add(1);
            tab.repository_load_id
        };
        self.spawn_operation_without_load_bump_with_blocker(
            Some(tab_id),
            "正在回滚文件更改",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(repo_path)?;
                let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
                let path_refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                let snapshot = match scope {
                    DiffScope::Staged => service.discard_all_paths(&mut repo, path_refs)?,
                    DiffScope::Unstaged => service.discard_unstaged_paths(&mut repo, path_refs)?,
                };
                let changes = service.status_full(&repo)?;
                Ok(UiEvent::DiscardChangeFinished {
                    tab_id,
                    message: message.to_string(),
                    snapshot,
                    changes,
                    load_id,
                })
            },
        );
    }

    fn spawn_operation_without_load_bump_with_blocker<F>(
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
        self.tasks.spawn(TaskKind::Short, move || match f() {
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

    pub(crate) fn copy_commit_sha(&mut self, oid: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(oid));
        self.commit_context_menu = None;
        self.status = "已复制提交 SHA".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn copy_file_absolute_path(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        let absolute_path = repository_file_absolute_path(repo_path, &path);
        cx.write_to_clipboard(ClipboardItem::new_string(
            absolute_path.to_string_lossy().into_owned(),
        ));
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.status = "已复制文件绝对路径".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn open_file_parent_directory(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        let absolute_path = repository_file_absolute_path(repo_path, &path);
        let Some(parent) = absolute_path.parent() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("无法确定文件所在目录", cx);
            return;
        };
        if !parent.is_dir() {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning(format!("文件所在目录不存在：{}", parent.display()), cx);
            return;
        }
        match system::open_directory(parent) {
            Ok(()) => {
                self.status = "已打开文件所在目录".into();
                self.last_error = None;
                self.notify_success(self.status.clone(), cx);
            }
            Err(err) => {
                let message = format!("打开文件所在目录失败：{err}");
                self.last_error = Some(message.clone());
                self.notify_error(message, cx);
            }
        }
        self.change_context_menu = None;
        self.file_path_context_menu = None;
    }

    /// 在系统资源管理器中打开当前仓库根目录（快捷键入口）。
    pub(crate) fn open_repo_in_explorer(&mut self, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref().map(PathBuf::from) else {
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        self.close_popups();
        match system::open_directory(&repo_path) {
            Ok(()) => {
                self.status = "已在资源管理器中打开仓库".into();
                self.last_error = None;
                self.notify_success(self.status.clone(), cx);
            }
            Err(err) => {
                let message = format!("打开仓库目录失败：{err}");
                self.last_error = Some(message.clone());
                self.notify_error(message, cx);
            }
        }
    }

    /// 以默认浏览器打开当前远端的 URL（快捷键入口）。
    pub(crate) fn open_remote_in_browser(&mut self, cx: &mut Context<Self>) {
        let Some(remote_name) = self.current_remote() else {
            self.notify_warning("当前仓库没有远端", cx);
            return;
        };
        // 先把 url 提取为 owned String，避免后续 self 操作与不可变借用冲突。
        let url = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .remotes
                .iter()
                .find(|r| r.name == remote_name)
                .map(|r| r.url.clone())
        });
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            self.notify_warning("远端 URL 为空或未找到", cx);
            return;
        };
        self.close_popups();
        open_url(&url);
        self.status = format!("已在浏览器中打开 {url}");
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn copy_branch_name(&mut self, branch: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(branch));
        self.branch_context_menu = None;
        self.status = "已复制分支名称".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn copy_remote_checkout_command(&mut self, branch: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(format!(
            "git checkout --track {branch}"
        )));
        self.branch_context_menu = None;
        self.status = "已复制 checkout 命令".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }
}
