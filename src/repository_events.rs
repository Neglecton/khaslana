//! RepositoryView 的 UI 事件归约与状态落位。

use crate::*;

impl RepositoryView {
    pub(crate) fn handle_ui_event(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::UiTick => {
                self.progress_phase = self.progress_phase.wrapping_add(1);
                let now = Instant::now();
                let feedbacks_before = self.feedbacks.len();
                self.feedbacks.retain(|feedback| !feedback.is_expired(now));
                let feedbacks_expired = self.feedbacks.len() != feedbacks_before;
                self.sync_conflict_editor_into_state();
                self.handle_tray_action(cx);
                // 周期静默更新检查（12 小时一次）：到期才触发；fire 时读取
                // auto_check 开关（会话中切换设置即时生效）；重排下次时刻
                // 无条件执行，开关关闭也不堆积错过的检查。
                if now >= self.next_periodic_update_check {
                    self.next_periodic_update_check = now + PERIODIC_UPDATE_CHECK_INTERVAL;
                    if self.update_preferences.auto_check {
                        self.start_update_check(UpdateCheckTrigger::Periodic);
                    }
                }
                // 空闲期不做全窗口重绘（UiTick 每 420ms 一次，无条件 notify 会让
                // 应用常驻 2.4Hz 重绘并触发渲染路径里的重复计算）：只在时态内容
                // 变化时通知——底部进度线在动画（有加载中操作）、操作遮罩到延迟
                // 阈值需要显示、或有过期反馈被移除。托盘动作无需重绘。
                if self.has_active_loading()
                    || self.active_operation_blocker_message().is_some()
                    || feedbacks_expired
                {
                    cx.notify();
                }
                // UiTick 不走事件末尾的统一 notify。
                return;
            }
            UiEvent::OperationStarted { tab_id, message } => {
                self.apply_status_event(tab_id, |this| {
                    this.busy = true;
                    this.operation_kind = OperationKind::from_message(&message);
                    this.status = message;
                    this.last_error = None;
                });
            }
            UiEvent::OperationProgress { tab_id, message } => {
                self.apply_status_event(tab_id, |this| {
                    this.status = message;
                });
            }
            UiEvent::RepositoryFastLoaded {
                tab_id,
                message,
                snapshot,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        let was_merge_in_progress = this.merge_in_progress();
                        let merge_in_progress = snapshot.merge_in_progress;
                        let merge_message = snapshot.merge_message.clone();
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.loading = RepositoryLoading {
                            metadata: true,
                            status_fast: true,
                            status_full: true,
                        };
                        this.status = message;
                        this.last_error = None;
                        this.diff = None;
                        this.branch_sync_status = None;
                        this.branch_sync_loading = false;
                        this.refresh_history();
                        this.change_selection.clear();
                        this.repo_path = Some(snapshot.path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        this.snapshot = Some(snapshot);
                        this.sync_merge_message_transition(
                            was_merge_in_progress,
                            merge_in_progress,
                            merge_message,
                        );
                        this.sync_conflict_mode_with_snapshot();
                        this.scroll_local_branch_to_current();
                        // 仓库（重）加载后历史引用已变：已有历史列表时不受视图限制地后台刷新，
                        // 覆盖切换分支/拉取/推送等引用类操作；初始打开（列表为空）不预加载。
                        this.reload_history_after_change();
                        // 已启用索引的仓库：后台增量检查（mtime+size 无变化时零写入）。
                        let loaded_path = this.repo_path.clone();
                        this.maybe_auto_code_index_refresh(loaded_path.as_deref(), cx);
                    }
                });
            }
            UiEvent::RepositoryMetadataLoaded {
                tab_id,
                message,
                snapshot,
                load_id,
            } => {
                let mut sync_request = None;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.loading.metadata = false;
                        this.status = message;
                        this.merge_metadata_snapshot(snapshot);
                        this.scroll_local_branch_to_current();
                        sync_request = this.prepare_branch_sync_status_request();
                    }
                });
                if let Some((tab_id, path, remote, load_id, request_id)) = sync_request {
                    self.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
                }
            }
            UiEvent::RepositoryStatusFastLoaded {
                tab_id,
                message,
                changes,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading.status_fast = false;
                        this.status = message;
                        this.replace_changes(changes);
                    }
                });
            }
            UiEvent::RepositoryStatusFullLoaded {
                tab_id,
                message,
                changes,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading.status_full = false;
                        this.status = message;
                        this.replace_changes(changes);
                    }
                });
            }
            UiEvent::RepositoryLoadStageFailed {
                tab_id,
                error,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading = RepositoryLoading::default();
                        this.operation_kind = OperationKind::Local;
                        this.status = "仓库已打开，后台加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::RepositoryLoadFinished { tab_id, load_id } => {
                self.active_repository_loads = self.active_repository_loads.saturating_sub(1);
                if self
                    .tab(tab_id)
                    .is_some_and(|tab| tab.repository_load_id == load_id)
                {
                    self.apply_status_event(Some(tab_id), |this| {
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.operation_kind = OperationKind::Local;
                    });
                }
                self.start_queued_repository_loads();
            }
            UiEvent::OperationFinished {
                tab_id,
                message,
                snapshot,
                diff,
            } => {
                let toast_message = message.clone();
                let keeps_remote_branch_dialog = matches!(
                    self.active_dialog.as_ref(),
                    Some(DialogState::RemoteBranchOperation { .. })
                );
                let should_refresh_repository =
                    operation_requires_repository_refresh(&message) && !keeps_remote_branch_dialog;
                let affects_commit_history = operation_affects_commit_history(&message);
                let should_refresh_submodules = operation_refreshes_submodule_dialog(&message)
                    && self.active_dialog == Some(DialogState::SubmoduleManager);
                let has_snapshot = snapshot.is_some();
                let has_diff = diff.is_some();
                let mut full_status_request = None;
                let mut sync_request = None;
                let mut repository_refresh_request = None;
                self.apply_status_event(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.remote_branch_operation.refreshing = false;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = message;
                    if let Some(snapshot) = snapshot {
                        let was_merge_in_progress = this.merge_in_progress();
                        let merge_in_progress = snapshot.merge_in_progress;
                        let merge_message = snapshot.merge_message.clone();
                        this.repo_path = (!snapshot.path.as_os_str().is_empty())
                            .then(|| snapshot.path.clone())
                            .or_else(|| this.repo_path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        if !snapshot.conflicts.is_empty() {
                            this.diff = None;
                            this.diff_headers_expanded = false;
                            this.reset_uniform_scroll("diff-scroll");
                        }
                        this.snapshot = Some(snapshot);
                        this.sync_merge_message_transition(
                            was_merge_in_progress,
                            merge_in_progress,
                            merge_message,
                        );
                        this.prune_stash_preview();
                        this.prune_change_selection();
                        this.sync_conflict_mode_with_snapshot();
                        this.refresh_history();
                        this.scroll_local_branch_to_current();
                        if affects_commit_history {
                            // 创建/移动提交或 HEAD 的操作：无论当前视图都后台刷新
                            // 提交记录及其 HEAD/分支/标签徽章，不再需要人工刷新。
                            this.reload_history_after_change();
                        } else {
                            this.reload_history_if_active();
                        }
                        if let Some(tab_id) = tab_id {
                            if should_refresh_repository {
                                repository_refresh_request =
                                    this.repo_path.clone().map(|path| (tab_id, path));
                            } else {
                                full_status_request = this
                                    .repo_path
                                    .clone()
                                    .map(|path| (tab_id, path, this.repository_load_id));
                                this.loading.status_full = true;
                            }
                        }
                        if !should_refresh_repository {
                            sync_request = this.prepare_branch_sync_status_request();
                        }
                    }
                    if let Some(diff) = diff {
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::Worktree {
                                    scope: diff.scope.clone(),
                                    path: diff.path.clone(),
                                },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.diff = Some(diff);
                        this.diff_headers_expanded = false;
                        this.diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::WorktreeDiff);
                        this.reset_uniform_scroll("diff-scroll");
                    }
                });
                // 暂存/取消暂存（整文件或按块/按行）后差异面板跟随刷新。
                // 必须先于全量状态补全/分支同步请求执行：load_diff 会经
                // spawn_operation 递增 repository_load_id（diff 缓存失效机制），
                // 这些后台请求若沿用闭包内捕获的旧代际，结果到达时会因代际
                // 守卫不匹配被丢弃，变更列表将停留在不含未跟踪文件的操作快照上。
                if operation_refreshes_worktree_diff(&toast_message) {
                    self.refresh_diff_after_stage_change();
                    let op_repo = tab_id
                        .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
                        .and_then(|tab| tab.repo_path.clone());
                    self.maybe_auto_code_index_refresh(op_repo.as_deref(), cx);
                }
                if let Some((tab_id, path, _)) = full_status_request {
                    // 代际需在差异重载之后取最新值（见上），否则结果会被守卫丢弃。
                    if let Some(load_id) = self.tab(tab_id).map(|tab| tab.repository_load_id) {
                        self.load_full_status_for_tab(
                            tab_id,
                            path,
                            load_id,
                            "变更已补全".to_string(),
                        );
                    }
                }
                if let Some((tab_id, path, remote, _, request_id)) = sync_request {
                    if let Some(load_id) = self.tab(tab_id).map(|tab| tab.repository_load_id) {
                        self.load_branch_sync_status_for_tab(
                            tab_id, path, remote, load_id, request_id,
                        );
                    }
                }
                if let Some((tab_id, path)) = repository_refresh_request {
                    // 分支引用变化后重新走完整仓库加载，避免只应用操作快照时遗漏引用或状态更新。
                    self.queue_repository_load(
                        tab_id,
                        path,
                        "正在刷新分支状态",
                        "分支状态已刷新",
                        LoadPriority::Background,
                    );
                }
                if should_notify_operation_finished(&toast_message, has_snapshot, has_diff) {
                    self.notify_completion(&toast_message, cx);
                }
                if should_refresh_submodules {
                    self.load_submodules();
                }
            }
            UiEvent::DiscardChangeFinished {
                tab_id,
                message,
                snapshot,
                changes,
                load_id,
            } => {
                let toast_message = message.clone();
                let mut should_notify = false;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        should_notify = true;
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.operation_kind = OperationKind::Local;
                        this.loading = RepositoryLoading::default();
                        this.status = message;
                        this.last_error = None;
                        this.repo_path = Some(snapshot.path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        this.snapshot = Some(snapshot);
                        this.prune_stash_preview();
                        this.sync_conflict_mode_with_snapshot();
                        this.replace_changes(changes);
                        this.diff = None;
                        this.diff_headers_expanded = false;
                        this.reset_uniform_scroll("diff-scroll");
                        this.refresh_history();
                        this.scroll_local_branch_to_current();
                        this.reload_history_if_active();
                    }
                });
                if should_notify {
                    self.notify_success(toast_message, cx);
                }
            }
            UiEvent::CredentialRecordsLoaded { records, message } => {
                let toast_message = message.clone();
                self.end_global_test_busy();
                self.operation_blocker = OperationBlocker::None;
                self.operation_blocker_started = None;
                self.credential_records = records;
                self.status = message;
                self.last_error = None;
                if Self::should_toast_completion(&toast_message) {
                    self.notify_completion(&toast_message, cx);
                }
            }
            UiEvent::SshCredentialsDiscovered { request_id, result } => {
                if request_id == self.ssh_credential_discovery.request_id {
                    let key_count = result.keys.len();
                    let agent_count = result.agent_identities.len();
                    self.ssh_credential_discovery.loading = false;
                    self.ssh_credential_discovery.result = Some(result);
                    self.ssh_credential_discovery.error = None;
                    self.status =
                        format!("已发现 {key_count} 个 SSH 私钥，Agent 中有 {agent_count} 个身份");
                }
            }
            UiEvent::SshCredentialDiscoveryFailed { request_id, error } => {
                if request_id == self.ssh_credential_discovery.request_id {
                    self.ssh_credential_discovery.loading = false;
                    self.ssh_credential_discovery.error = Some(error.clone());
                    self.status = "本机 SSH 检测失败".into();
                    self.last_error = Some(error);
                }
            }
            UiEvent::OAuthLoginReady {
                request_id,
                provider,
                url,
                user_code,
            } => {
                if request_id == self.oauth_login_flow.request_id {
                    self.oauth_login_flow.user_code = user_code;
                    self.oauth_login_flow.verification_uri = Some(url.clone());
                    open_url(&url);
                    self.status = format!("请在浏览器中完成{}登录", provider.label());
                }
            }
            UiEvent::OAuthLoginSucceeded {
                request_id,
                provider,
                username,
                token,
                gitee_refresh,
            } => {
                if request_id == self.oauth_login_flow.request_id {
                    let (host_url, note) = match provider {
                        OAuthProvider::Github => ("github.com", "GitHub 登录成功，凭据已保存"),
                        OAuthProvider::Gitee => (
                            "gitee.com",
                            if gitee_refresh.is_some() {
                                "Gitee 登录成功，凭据已保存（令牌将自动续期）"
                            } else {
                                // 旧版 broker 未透传 refresh_token：保持手动续期提示。
                                "Gitee 登录成功，凭据已保存（令牌约 1 天过期，过期后重新登录）"
                            },
                        ),
                    };
                    let provider_label = provider.label();
                    self.oauth_login_flow.loading = false;
                    self.oauth_login_flow.provider = None;
                    self.oauth_login_flow.user_code = None;
                    self.oauth_login_flow.verification_uri = None;
                    self.oauth_login_flow.cancel = None;
                    // 把令牌当作 PAT 填入 HTTPS 凭据表单并直接保存，用户无需手动录入。
                    self.credential_form_mode = CredentialFormMode::Https;
                    self.credential_username.set_value(username.clone());
                    self.credential_secret.set_value(token);
                    if self.credential_display_name.value.trim().is_empty()
                        || !self.credential_display_name.value.contains(provider_label)
                    {
                        self.credential_display_name
                            .set_value(format!("{provider_label} · {username}"));
                    }
                    if !self
                        .credential_remote_url
                        .value
                        .to_ascii_lowercase()
                        .contains(host_url)
                    {
                        self.credential_remote_url
                            .set_value(format!("https://{host_url}"));
                    }
                    self.credential_scope = CredentialScope::Host;
                    self.save_credential_form();
                    // Gitee：把自动续期材料落到独立 Keyring 条目（绑定刚保存的记录）。
                    if let (OAuthProvider::Gitee, Some(record_id)) =
                        (provider, self.pending_gitee_refresh_record.take())
                        && let Some((refresh_token, expires_at)) = gitee_refresh
                    {
                        if let Err(err) = khaslana::credentials::save_gitee_refresh_payload(
                            &record_id,
                            &refresh_token,
                            expires_at,
                        ) {
                            tracing::warn!("Gitee 续期材料保存失败：{err}");
                            self.notify_warning(
                                "Gitee 令牌已保存，但自动续期材料写入失败，令牌过期后需重新登录",
                                cx,
                            );
                        }
                    } else {
                        self.pending_gitee_refresh_record = None;
                    }
                    self.notify_success(note, cx);
                }
            }
            UiEvent::GiteeTokenRefreshed { success, message } => {
                if success {
                    self.status = message;
                } else {
                    // 续期失败不阻断当前操作（旧令牌继续尝试），提示重新登录即可恢复。
                    self.notify_warning(message, cx);
                }
            }
            UiEvent::OAuthLoginFailed { request_id, error } => {
                if request_id == self.oauth_login_flow.request_id {
                    let label = self
                        .oauth_login_flow
                        .provider
                        .map(|p| p.label())
                        .unwrap_or("OAuth");
                    self.oauth_login_flow.loading = false;
                    self.oauth_login_flow.provider = None;
                    self.oauth_login_flow.user_code = None;
                    self.oauth_login_flow.verification_uri = None;
                    self.oauth_login_flow.cancel = None;
                    self.oauth_login_flow.error = Some(error.clone());
                    self.last_error = Some(error.clone());
                    self.notify_error(format!("{label} 登录失败：{error}"), cx);
                }
            }
            UiEvent::CredentialSshKeyFileSelected { path } => {
                if let Some(path) = path {
                    self.use_discovered_ssh_key(path);
                } else {
                    self.status = "已取消选择 SSH 私钥".into();
                }
            }
            UiEvent::HistoryCommitsLoaded {
                tab_id,
                commits,
                refs_cache,
                append,
                has_more,
                scope,
                path_filter,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅最新一代请求（seq 匹配）复位加载标志并应用数据；
                    // 旧一代晚到的结果直接丢弃，避免覆盖新数据。
                    // seq 匹配但 load_id 失配（操作作废了本次请求）也要复位标志，
                    // 否则 history_loading.commits 永久为 true 会吞掉后续所有历史加载。
                    if seq == this.history_load_seq {
                        this.history_loading.commits = false;
                        if this.history_commits_event_matches(
                            load_id,
                            scope,
                            path_filter.as_deref(),
                        ) {
                            this.history_refs_cache = Some(refs_cache);
                            this.history_has_more = has_more;
                            if append {
                                this.history_commits.extend(commits);
                            } else {
                                this.history_commits = commits;
                                let was_refreshing = this.history_refreshing;
                                if was_refreshing {
                                    // 刷新时保留选中提交（若仍存在于新列表）。
                                    // 选中的文件列表与差异按 oid 不可变，一并保留，
                                    // 避免出现“详情显示选中提交、文件/差异永远空占位”
                                    // 的不一致（历史上这里的清空没有配套重载）。
                                    let still_exists =
                                        this.history_selected_commit.as_ref().is_some_and(|oid| {
                                            this.history_commits
                                                .iter()
                                                .any(|c| c.oid == oid.as_str())
                                        });
                                    if !still_exists {
                                        this.history_selected_commit = None;
                                        this.history_files.clear();
                                        this.history_selected_file = None;
                                        this.history_diff = None;
                                    }
                                } else {
                                    // 非刷新（scope 切换/初始加载）：全部重置
                                    this.history_selected_commit = None;
                                    this.history_files.clear();
                                    this.history_selected_file = None;
                                    this.history_diff = None;
                                }
                                this.history_refreshing = false;
                            }
                            // 过滤模式下隐藏提交图形列：过滤后中间提交缺失，
                            // 泳道线会断裂，跳过泳道计算并让行渲染不画图形。
                            this.history_graph_rows = if this.history_file_filter.is_none() {
                                commit_graph_view::commit_graph_rows(&this.history_commits)
                            } else {
                                Vec::new()
                            };

                            if this.history_selected_commit.is_none() {
                                if let Some(commit) = this.history_commits.first() {
                                    this.select_history_commit(commit.oid.clone());
                                } else {
                                    this.status = "当前分支暂无提交记录".to_string();
                                }
                            } else {
                                this.status = "提交记录已加载".to_string();
                                // 自愈：选中保留但文件列表为空（旧状态冻结、或此前的
                                // 文件加载被仓库重载作废）时重新拉取；非空时
                                // select_history_commit 幂等跳过，不影响现有展示。
                                if let Some(oid) = this.history_selected_commit.clone() {
                                    this.select_history_commit(oid);
                                }
                            }
                        }
                    }
                });
            }
            UiEvent::HistoryFilesLoaded {
                tab_id,
                commit_oid,
                files,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.history_selected_commit.as_deref() == Some(commit_oid.as_str())
                    {
                        this.history_loading.files = false;
                        this.history_files = files;
                        this.history_selected_file = None;
                        this.history_diff = None;
                        this.history_diff_headers_expanded = false;

                        if let Some(preferred) = preferred_history_file(
                            this.history_file_filter.as_deref(),
                            &this.history_files,
                        ) {
                            this.select_history_file(preferred);
                        } else {
                            this.status = "该提交没有文件变更".to_string();
                        }
                    }
                });
            }
            UiEvent::HistoryDiffLoaded {
                tab_id,
                commit_oid,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.history_selected_commit.as_deref() == Some(commit_oid.as_str())
                        && this.history_selected_file.as_deref() == Some(path.as_str())
                    {
                        this.history_loading.diff = false;
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::History { commit_oid, path },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.history_diff = Some(diff);
                        this.history_diff_headers_expanded = false;
                        this.history_diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::HistoryDiff);
                        this.reset_uniform_scroll("history-diff-scroll");
                        this.status = "提交差异已加载".to_string();
                    }
                });
            }
            UiEvent::StashFilesLoaded {
                tab_id,
                stash_oid,
                files,
                load_id,
            } => {
                let mut first_path = None;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.stash_preview.stash_oid.as_deref() == Some(stash_oid.as_str())
                    {
                        this.stash_preview.loading_files = false;
                        this.stash_preview.files = files;
                        this.stash_preview.selected_file = None;
                        this.stash_preview.diff = None;
                        this.stash_preview.diff_headers_expanded = false;
                        first_path = this
                            .stash_preview
                            .files
                            .first()
                            .map(|file| file.path.clone());
                        if first_path.is_none() {
                            this.status = "该贮藏没有文件变更".to_string();
                        }
                    }
                });
                if let Some(path) = first_path
                    && self.active_tab == Some(tab_id)
                {
                    self.select_stash_file(path, false);
                }
            }
            UiEvent::StashDiffLoaded {
                tab_id,
                stash_oid,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.stash_preview.stash_oid.as_deref() == Some(stash_oid.as_str())
                        && this.stash_preview.selected_file.as_deref() == Some(path.as_str())
                    {
                        this.stash_preview.loading_diff = false;
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::Stash { stash_oid, path },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.stash_preview.diff = Some(diff);
                        this.stash_preview.diff_headers_expanded = false;
                        this.stash_preview.diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::StashDiff);
                        this.reset_uniform_scroll("stash-diff-scroll");
                        this.status = "贮藏差异已加载".to_string();
                    }
                });
            }
            UiEvent::HistoryLoadFailed {
                tab_id,
                error,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 失败（包括被 load_id 作废的陈旧失败）都要复位加载标志，
                    // 否则操作开始 bump load_id 后若操作失败，标志永久卡住，
                    // 后续所有历史/贮藏预览加载都会被在飞守卫静默吞掉。
                    this.history_loading = HistoryLoading::default();
                    this.history_refreshing = false;
                    this.stash_preview.loading_files = false;
                    this.stash_preview.loading_diff = false;
                    if load_id == this.repository_load_id {
                        this.status = if this.main_mode == MainMode::Stash {
                            "贮藏预览加载失败".to_string()
                        } else {
                            "提交记录加载失败".to_string()
                        };
                        this.last_error = Some(error);
                        // 全文视图过大时自动回退到紧凑差异
                        this.revert_full_file_if_too_large_error();
                    }
                });
            }
            UiEvent::CommitTraceLoaded {
                tab_id,
                branch,
                ahead_only,
                oids,
                truncated,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅最新代际（seq 匹配）应用：参数（分支/模式）变化必先递增 seq，
                    // 旧一代晚到的结果直接丢弃；仓库重载（load_id 失配）同样作废。
                    if seq != this.commit_graph.trace_seq || load_id != this.repository_load_id {
                        return;
                    }
                    this.commit_graph.trace_loading = false;
                    this.commit_graph.trace = Some(CommitTrace {
                        oids: Arc::new(oids.into_iter().collect()),
                        truncated,
                    });
                    let mode_label = if ahead_only {
                        "仅领先 HEAD"
                    } else {
                        "全谱系"
                    };
                    this.status = format!("已高亮分支 {branch}（{mode_label}）");
                });
            }
            UiEvent::CommitTraceLoadFailed {
                tab_id,
                error,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if seq != this.commit_graph.trace_seq {
                        return;
                    }
                    this.commit_graph.trace_loading = false;
                    if load_id == this.repository_load_id {
                        this.status = "分支谱系计算失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::BranchSyncStatusLoaded {
                tab_id,
                status,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && request_id == this.branch_sync_request_id
                    {
                        this.branch_sync_loading = false;
                        this.branch_sync_status = status;
                    }
                });
            }
            UiEvent::BranchSyncStatusFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && request_id == this.branch_sync_request_id
                    {
                        this.branch_sync_loading = false;
                        this.branch_sync_status = None;
                        tracing::warn!("branch sync status skipped: {error}");
                    }
                });
            }
            UiEvent::SubmodulesLoaded {
                tab_id,
                items,
                load_id,
                request_id,
            } => {
                let mut should_load_remote_statuses = false;
                self.with_tab_context(tab_id, |this| {
                    if submodule_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.items = items;
                        this.submodule_dialog.remote_statuses.clear();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.loading = false;
                        this.submodule_dialog.loaded = true;
                        this.submodule_dialog.error = None;
                        this.submodule_dialog.remote_error = None;
                        should_load_remote_statuses = !this.submodule_dialog.items.is_empty()
                            && this.active_dialog == Some(DialogState::SubmoduleManager);
                        this.status = "子模块列表已加载".to_string();
                    }
                });
                if should_load_remote_statuses {
                    let _ = self.with_tab_context(tab_id, |this| {
                        this.load_submodule_remote_statuses();
                    });
                }
            }
            UiEvent::SubmodulesLoadFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.items.clear();
                        this.submodule_dialog.remote_statuses.clear();
                        this.submodule_dialog.loading = false;
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.loaded = false;
                        this.submodule_dialog.error = Some(error.clone());
                        this.submodule_dialog.remote_error = None;
                        this.status = "子模块列表加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::SubmoduleRemoteStatusesLoaded {
                tab_id,
                statuses,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_remote_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.remote_statuses = statuses.into_iter().collect();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.remote_error = None;
                        this.status = "子模块远端状态已检查".to_string();
                    }
                });
            }
            UiEvent::SubmoduleRemoteStatusesLoadFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_remote_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.remote_statuses = this
                            .submodule_dialog
                            .items
                            .iter()
                            .map(|module| {
                                (
                                    module.name.clone(),
                                    SubmoduleRemoteSyncStatus::Unavailable(error.clone()),
                                )
                            })
                            .collect();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.remote_error = Some(error.clone());
                        this.status = "子模块远端状态检查失败".to_string();
                        tracing::warn!("submodule remote status skipped: {error}");
                    }
                });
            }
            UiEvent::BlameLoaded {
                tab_id,
                path,
                view,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // load_id 与路径双校验：仓库重载或切换追溯文件后，
                    // 旧请求的结果不落地。
                    if load_id == this.repository_load_id
                        && this.blame.path.as_deref() == Some(path.as_str())
                    {
                        this.blame.loading = false;
                        this.blame.syntax = None;
                        this.blame.view = Some(Arc::new(view));
                        this.status = "文件追溯已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::Blame);
                    }
                });
            }
            UiEvent::BlameLoadFailed {
                tab_id,
                path,
                error,
                load_id,
            } => {
                let toast_message = error.clone();
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.blame.path.as_deref() == Some(path.as_str())
                    {
                        this.blame.loading = false;
                        this.status = "文件追溯加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
                self.notify_error(toast_message, cx);
            }
            UiEvent::SyntaxHighlighted {
                tab_id,
                slot,
                anchor,
                anchor_len,
                spans,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // Arc 身份守卫：内容已被替换（或 Arc 地址被复用但行数不同）
                    // 时丢弃，避免旧高亮错配新内容。
                    let current = match slot {
                        SyntaxSlot::WorktreeDiff => this
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::HistoryDiff => this
                            .history_diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::StashDiff => this
                            .stash_preview
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::BrowseDiff => this
                            .browse
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::Blame => this
                            .blame
                            .view
                            .as_ref()
                            .map(|view| (Arc::as_ptr(view) as usize, view.lines.len())),
                        SyntaxSlot::BrowseContent => this
                            .browse
                            .content
                            .as_ref()
                            .map(|content| (Arc::as_ptr(content) as usize, content.lines.len())),
                    };
                    if current != Some((anchor, anchor_len)) {
                        return;
                    }
                    match slot {
                        SyntaxSlot::WorktreeDiff => this.diff_syntax = spans,
                        SyntaxSlot::HistoryDiff => this.history_diff_syntax = spans,
                        SyntaxSlot::StashDiff => this.stash_preview.diff_syntax = spans,
                        SyntaxSlot::BrowseDiff => this.browse.diff_syntax = spans,
                        SyntaxSlot::Blame => this.blame.syntax = spans,
                        SyntaxSlot::BrowseContent => this.browse.content_syntax = spans,
                    }
                });
            }
            UiEvent::ConflictSyntaxHighlighted {
                tab_id,
                path,
                pane,
                seq,
                spans,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if !this.conflict_workbench.files.contains_key(&path) {
                        return;
                    }
                    let Some(entry) = this.conflict_workbench.syntax.get_mut(&path) else {
                        return;
                    };
                    match pane {
                        ConflictSyntaxPane::Ours => entry.ours = spans,
                        ConflictSyntaxPane::Theirs => entry.theirs = spans,
                        // 草稿已再次变更（更新的调度在飞）时丢弃旧结果
                        ConflictSyntaxPane::Draft => {
                            if entry.draft_seq == seq {
                                entry.draft = spans;
                            }
                        }
                    }
                });
            }
            UiEvent::BrowseTargetResolved {
                tab_id,
                target,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.browse.target = Some(target);
                        match this.browse.list_mode {
                            BrowseListMode::Tree => {
                                // 浏览模式自动加载根目录树。
                                this.load_browse_tree(PathBuf::new());
                            }
                            BrowseListMode::Compare => {
                                // 比较模式只加载差异文件列表，避免遍历完整文件树。
                                this.load_browse_compare_files();
                            }
                        }
                    }
                });
            }
            UiEvent::BrowseTreeLoaded {
                tab_id,
                dir_path,
                entries,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.browse.loading_tree = false;
                        this.browse.entries_by_dir.insert(dir_path, entries);
                    }
                });
            }
            UiEvent::BrowseCompareFilesLoaded {
                tab_id,
                target_oid,
                files,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    let target_matches = this
                        .browse
                        .target
                        .as_ref()
                        .is_some_and(|target| target.commit_oid == target_oid);
                    if load_id == this.repository_load_id
                        && target_matches
                        && this.browse.list_mode == BrowseListMode::Compare
                    {
                        this.browse.compare_loading = false;
                        this.browse.compare_files = files;
                        if let Some(first) = this.browse.compare_files.first().cloned() {
                            this.status =
                                format!("已加载 {} 个差异文件", this.browse.compare_files.len());
                            this.select_browse_compare_file(first);
                        } else {
                            this.browse.selected_file = None;
                            this.browse.selected_compare_file = None;
                            this.browse.content = None;
                            this.browse.diff = None;
                            this.browse.diff_headers_expanded = false;
                            this.status = "该分支与当前分支没有差异".to_string();
                        }
                    }
                });
            }
            UiEvent::BrowseFileContentLoaded {
                tab_id,
                path,
                content,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.browse.selected_file.as_deref() == Some(std::path::Path::new(&path))
                        && this.browse.view_mode == BrowseViewMode::Content
                    {
                        this.browse.loading_content = false;
                        this.browse.content_syntax = None;
                        this.browse.content = Some(Arc::new(content));
                        this.status = "文件内容已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::BrowseContent);
                    }
                });
            }
            UiEvent::BrowseFileDiffLoaded {
                tab_id,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.browse.selected_file.as_deref() == Some(std::path::Path::new(&path))
                        && this.browse.view_mode == BrowseViewMode::Diff
                    {
                        this.browse.loading_diff = false;
                        this.browse.diff_syntax = None;
                        this.browse.diff = Some(Arc::new(diff));
                        this.browse.diff_headers_expanded = false;
                        this.status = "文件差异已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::BrowseDiff);
                    }
                });
            }
            UiEvent::OperationFailed { tab_id, error } => {
                let toast_message = error.clone();
                // 全局测试失败路径带 tab_id=None：定向复位借用的来源 tab。
                if tab_id.is_none() {
                    self.end_global_test_busy();
                }
                self.apply_status_event(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.remote_branch_operation.refreshing = false;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = "操作失败".to_string();
                    this.last_error = Some(error);
                    // 全文视图过大时自动回退到紧凑差异
                    this.revert_full_file_if_too_large_error();
                });
                self.notify_error(toast_message, cx);
            }
            UiEvent::CredentialRequested {
                tab_id,
                request,
                response_tx,
            } => {
                if tab_id.is_some_and(|tab_id| self.tab(tab_id).is_none()) {
                    if let Ok(mut response_tx) = response_tx.lock()
                        && let Some(response_tx) = response_tx.take()
                    {
                        let _ = response_tx.send(Ok(None));
                    }
                    return;
                }
                self.apply_status_event(tab_id, |this| {
                    this.status = "需要凭据".to_string();
                });
                self.notify_toast(AppToastKind::Info, "远端操作需要凭据，请在右上角填写", cx);
                let pending = PendingCredential {
                    tab_id,
                    request,
                    response_tx,
                };
                // 仅当请求被提升为当前待处理时才准备表单：排队中的请求不能
                // 重置表单——那会用旧请求的参数覆盖用户正在输入的内容。
                if self.enqueue_credential_request(pending) {
                    self.prepare_current_credential_prompt();
                }
            }
            UiEvent::ProxyTestFinished { message } => {
                let toast_message = message.clone();
                self.end_global_test_busy();
                self.operation_blocker = OperationBlocker::None;
                self.operation_blocker_started = None;
                self.status = message;
                self.last_error = None;
                self.notify_success(toast_message, cx);
            }
            UiEvent::WorkflowProgress { tab_id, entry } => {
                self.with_tab_context(tab_id, |this| {
                    this.status = entry.message.clone();
                    this.workflow_state.log.push(entry);
                });
            }
            UiEvent::CodeIndexProgress {
                repo_path,
                message,
                done,
                total,
            } => {
                self.handle_code_index_progress(repo_path, message, done, total);
                cx.notify();
            }
            UiEvent::CodeIndexFinished { repo_path, stats } => {
                self.handle_code_index_finished(repo_path, stats, cx);
            }
            UiEvent::CodeIndexFailed { repo_path, error } => {
                self.handle_code_index_failed(repo_path, error, cx);
            }
            UiEvent::CodeIndexStatsLoaded { repo_path, stats } => {
                self.handle_code_index_stats_loaded(repo_path, stats);
                cx.notify();
            }
            UiEvent::CodePaletteSearchFinished { seq, hits } => {
                self.handle_code_palette_search_finished(seq, hits);
            }
            UiEvent::CodePaletteDetailFinished { seq, detail } => {
                self.handle_code_palette_detail_finished(seq, detail);
            }
            UiEvent::WorkflowTemplatesLoaded { result } => {
                self.apply_workflow_templates(result, cx);
            }
            UiEvent::WorkflowFinished {
                tab_id,
                message,
                snapshot,
                log,
            } => {
                let toast_message = message.clone();
                let mut full_status_request = None;
                let mut sync_request = None;
                self.with_tab_context(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = message;
                    this.last_error = None;
                    this.workflow_state.log = log;
                    this.repo_path = Some(snapshot.path.clone());
                    this.sync_selected_remote(&snapshot);
                    this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                    this.snapshot = Some(snapshot);
                    this.prune_stash_preview();
                    this.sync_conflict_mode_with_snapshot();
                    this.prune_change_selection();
                    this.diff = None;
                    this.diff_headers_expanded = false;
                    this.reset_uniform_scroll("diff-scroll");
                    this.refresh_history();
                    this.scroll_local_branch_to_current();
                    // 工作流可能执行拉取/提交等操作，历史列表存在时后台刷新
                    this.reload_history_after_change();
                    full_status_request = this
                        .repo_path
                        .clone()
                        .map(|path| (tab_id, path, this.repository_load_id));
                    this.loading.status_full = true;
                    sync_request = this.prepare_branch_sync_status_request();
                });
                if let Some((tab_id, path, load_id)) = full_status_request {
                    self.load_full_status_for_tab(tab_id, path, load_id, "变更已补全".to_string());
                }
                if let Some((tab_id, path, remote, load_id, request_id)) = sync_request {
                    self.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
                }
                self.notify_completion(&toast_message, cx);
            }
            UiEvent::OpenRepositoryFolderSelected { path } => {
                if let Some(path) = path {
                    self.open_repo(path);
                } else {
                    self.status = "已取消选择仓库文件夹".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::CloneTargetFolderSelected { path } => {
                if let Some(path) = path {
                    self.clone_path.set_value(path.display().to_string());
                    self.last_error = None;
                } else {
                    self.status = "已取消选择克隆父文件夹".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::ExternalMergeExecutableSelected { path } => {
                self.external_merge_detection = None;
                if let Some(path) = path {
                    self.external_merge_intellij_path
                        .set_value(path.display().to_string());
                    self.external_merge_enabled_form = true;
                    self.status = format!("已选择 IntelliJ IDEA：{}", path.display());
                    self.last_error = None;
                } else {
                    self.status = "已取消选择 IntelliJ IDEA 程序".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::AiWorkflowTemplateGenerated { content } => {
                self.handle_ai_workflow_template_generated(content, cx);
            }
            UiEvent::AiCommitMessageGenerated { message } => {
                self.ai_commit_loading = false;
                // 思考弹窗随完成自动关闭。
                self.ai_thinking_overlay = None;
                // 兜底守卫：空结果不覆盖输入框（避免清掉用户草稿）并显式提示。
                // 正常路径已在生成任务里按空正文报错，这里防御未来回归。
                if message.trim().is_empty() {
                    self.status = "AI 未返回提交信息".into();
                    self.last_error = Some("AI 返回的提交信息为空".into());
                    self.notify_error("AI 返回的提交信息为空", cx);
                } else {
                    // 流式期间已逐段填入输入框，这里用最终结果做一次干净覆盖，
                    // 确保最终的 trim 和换行规范化。
                    self.commit_message.set_value(message);
                    self.status = "AI 已生成提交信息".into();
                    self.last_error = None;
                }
            }
            UiEvent::AiReviewGenerated {
                generation,
                review,
                saved,
            } => {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                if self.ai_review_active_generation == Some(generation) {
                    self.ai_review_loading = false;
                    self.ai_review_cancel = None;
                    self.ai_review_progress = None;
                    // live 缓冲定格为正式结果（同一数据源），清空避免重复展示。
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    self.ai_review_loaded_label = None;
                    self.ai_review = Some(Arc::new(review));
                    self.status = if saved {
                        "AI 评审已生成并保存到评审记录".into()
                    } else {
                        "AI 评审已生成（保存记录失败）".into()
                    };
                    self.last_error = None;
                } else {
                    // 后台分离任务完成：落盘已在任务线程做完，提示可去历史查看。
                    let message = if saved {
                        "后台 AI 评审完成，已保存到评审记录".to_string()
                    } else {
                        "后台 AI 评审完成（保存记录失败，历史弹窗中将看不到本次记录）".to_string()
                    };
                    self.status = message.clone();
                    self.notify_completion(&message, cx);
                }
            }
            UiEvent::AiReviewStepAdded { generation, step } => {
                // 代际守卫：UI 已分离（切目标/取消）的旧任务事件不进面板。
                if self.ai_review_active_generation == Some(generation) {
                    // 保留已完成评审的轨迹直到新评审开始覆盖（generate 时清空）。
                    // 思维链/中间正文步骤落定后，对应 live 区让位于正式时间线行。
                    match &step {
                        AiReviewStep::Reasoning { .. } => self.ai_review_live_reasoning.clear(),
                        AiReviewStep::Message { .. } => self.ai_review_live_content.clear(),
                        AiReviewStep::ToolCall { .. } => {}
                    }
                    self.ai_review_steps.push(step);
                }
            }
            UiEvent::AiReviewProgress {
                generation,
                message,
            } => {
                if self.ai_review_active_generation == Some(generation) {
                    // 新一轮开始：清上一轮的 live 思维链（正文 live 只属于末轮，
                    // 也在换轮时清空避免中轮正文残留）。
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    // 进度同时落状态栏，与其它 AI 任务一致。
                    self.status = message.clone();
                    self.ai_review_progress = Some(message);
                }
            }
            UiEvent::AiReviewDelta {
                generation,
                content_delta,
                reasoning_delta,
            } => {
                if self.ai_review_active_generation == Some(generation) {
                    if let Some(delta) = content_delta {
                        self.ai_review_live_content.push_str(&delta);
                    }
                    if let Some(delta) = reasoning_delta {
                        self.ai_review_live_reasoning.push_str(&delta);
                    }
                }
            }
            UiEvent::AiReviewFailed { generation, error } => {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                if self.ai_review_active_generation == Some(generation) {
                    self.ai_review_loading = false;
                    self.ai_review_cancel = None;
                    self.ai_review_progress = None;
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    // 失败保留已产生的轨迹（可检查模型做了什么）。
                    self.status = "AI 请求失败".into();
                    self.last_error = Some(error.clone());
                    self.notify_error(format!("AI 请求失败：{error}"), cx);
                } else {
                    // 后台分离任务失败：不打断用户，仅状态栏可见。
                    self.status = format!("后台 AI 评审失败：{error}");
                }
            }
            UiEvent::AiReviewCancelled => {
                // 取消时 UI 已即时复位，这里只做在途计数归位。
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
            }
            UiEvent::AiReviewHistoryLoaded { records } => {
                if let Some(state) = self.ai_review_history.as_mut() {
                    state.loading = false;
                    state.records = records;
                }
            }
            UiEvent::AiReviewHistoryLoadFailed { error } => {
                if let Some(state) = self.ai_review_history.as_mut() {
                    state.loading = false;
                    state.error = Some(error);
                }
            }
            // ── 代码理解（CU2-T5）：事件在 view 模块内按 (project_key, generation) 守卫 ──
            UiEvent::UnderstandingProgress {
                project_key,
                generation,
                message,
            } => {
                self.handle_understanding_progress(project_key, generation, message, cx);
            }
            UiEvent::UnderstandingDelta {
                project_key,
                generation,
                content_delta,
                reasoning_delta,
            } => {
                self.handle_understanding_delta(
                    project_key,
                    generation,
                    content_delta,
                    reasoning_delta,
                    cx,
                );
            }
            UiEvent::UnderstandingStepAdded {
                project_key,
                generation,
                step,
            } => {
                self.handle_understanding_step(project_key, generation, step, cx);
            }
            UiEvent::UnderstandingFinished {
                project_key,
                generation,
                answer,
                saved,
            } => {
                self.handle_understanding_finished(project_key, generation, answer, saved, cx);
            }
            UiEvent::UnderstandingFailed {
                project_key,
                generation,
                error,
            } => {
                self.handle_understanding_failed(project_key, generation, error, cx);
            }
            UiEvent::UnderstandingCancelled {
                project_key,
                generation,
            } => {
                self.handle_understanding_cancelled(project_key, generation, cx);
            }
            UiEvent::UnderstandingHistoryLoaded {
                project_key,
                records,
            } => {
                self.handle_understanding_history_loaded(project_key, records, cx);
            }
            UiEvent::AiConflictMergeProgress {
                path,
                segment,
                total,
            } => {
                // 分段模式下的进度提示：仅当前选中的冲突文件更新状态栏，
                // 避免生成期间切换文件后被旧任务的进度占据。整文件模式
                // 只有一段，不发送该事件。
                if self.conflict_workbench.selected_path.as_deref() == Some(path.as_str()) {
                    self.status = format!("正在生成 AI 合并建议（第 {segment}/{total} 段）");
                }
            }
            UiEvent::AiConflictMergeGenerated { path, draft } => {
                self.ai_conflict_loading = false;
                // 思考弹窗随完成自动关闭。
                self.ai_thinking_overlay = None;
                match self.conflict_workbench.files.get_mut(&path) {
                    Some(view) if view.kind == ConflictFileKind::Text => {
                        // Merged 写入：被覆盖块标记「已合并」（绿色），不再
                        // 计入未处理，也不触发手工修改横幅。
                        view.set_merged_draft(draft);
                        self.sync_conflict_editor_from_state();
                        // 草稿整体替换后重算结果区语法高亮
                        self.schedule_conflict_syntax_for_selected(&[ConflictSyntaxPane::Draft]);
                        self.status = "已填入 AI 合并建议，请检查后应用".into();
                        self.last_error = None;
                        self.notify_success("已填入 AI 合并建议，请检查后应用", cx);
                    }
                    // 生成期间冲突被解决、文件被移出列表或切换了标签页：
                    // 结果无处安放，仅状态栏说明，不弹错误。
                    _ => {
                        self.status = "AI 合并建议已返回，但该文件已不在冲突列表".into();
                    }
                }
            }
            UiEvent::AiRequestFailed { error } => {
                self.ai_commit_loading = false;
                self.ai_conflict_loading = false;
                // 思考弹窗随失败自动关闭（无论哪路业务失败）。
                self.ai_thinking_overlay = None;
                // 工作流模板编辑器的 AI 生成失败：错误写进编辑器内错误条
                // （弹窗仍开着，用户可直接改需求重试），并复位其 loading。
                self.handle_ai_workflow_template_failed(&error);
                // 评审失败走带代际的 AiReviewFailed（旧任务的失败不能
                // 误复位当前附着任务的状态）。
                // 测试连接失败时也要解锁借用的 busy，否则按钮永久禁用。
                self.end_global_test_busy();
                // 失败提示走右下角 toast + 状态栏双通道，仅靠状态栏小字极易被错过。
                self.status = "AI 请求失败".into();
                self.last_error = Some(error.clone());
                self.notify_error(format!("AI 请求失败：{error}"), cx);
            }
            UiEvent::AiThinkingDelta {
                content_delta,
                reasoning_delta,
            } => {
                // 思考弹窗已关闭（后台运行）时丢弃增量。
                let Some(overlay) = self.ai_thinking_overlay.as_mut() else {
                    return;
                };
                overlay.reasoning.push_str(&reasoning_delta);
                if let Some(delta) = content_delta {
                    overlay.content.push_str(&delta);
                }
                // 钉底跟随不在这里做：事件时机的 max_offset 是上一帧的，
                // 会恒落后一帧；改由弹窗内容末位 canvas 的 prepaint 按内容
                // 长度键门控执行（见 render_ai_thinking_overlay）。
            }
            UiEvent::AiConnectionTested { message } => {
                self.end_global_test_busy();
                self.status = message.clone();
                self.last_error = None;
                self.notify_completion(&message, cx);
            }
            // ── 更新事件 ──
            UiEvent::UpdateCheckFinished {
                manifest,
                asset,
                trigger,
            } => {
                self.update_checking = false;
                let known_version = self
                    .available_update
                    .as_ref()
                    .map(|known| known.version.clone());
                self.available_update = Some(manifest.clone());
                self.status = format!("发现新版本 v{}", manifest.version);
                self.last_error = None;
                if trigger == UpdateCheckTrigger::Periodic {
                    // 周期静默检查：不弹模态框，只弹可点击气泡直达更新设置；
                    // 同版本已提示过（点 ✕ / 超时消失 = 稍后）不再重复打扰，
                    // 设置页常驻「发现新版本」卡片仍可查。
                    if let Some(message) =
                        periodic_update_found_toast(known_version.as_deref(), &manifest.version)
                    {
                        self.notify_toast_with_action(
                            AppToastKind::Info,
                            message,
                            ToastAction::OpenUpdateSettings,
                            cx,
                        );
                    }
                } else {
                    self.active_dialog = Some(DialogState::NewVersionAvailable {
                        version: manifest.version.clone(),
                        notes: manifest.notes.clone(),
                        published_at: manifest.published_at.clone(),
                        size: asset.size,
                    });
                }
            }
            UiEvent::UpdateCheckFailed { error, trigger } => {
                self.update_checking = false;
                // 周期静默检查：无新版本 / 网络失败等完全无反馈。
                if trigger == UpdateCheckTrigger::Periodic {
                    return;
                }
                if !error.is_empty() {
                    self.update_error = Some(error.clone());
                    self.status = error.clone();
                    // 手动检查弹气泡反馈（成功发现最新=成功气泡、真失败=
                    // 错误气泡）；启动自动检查保持安静，仅状态栏与设置页错误条。
                    if let Some((kind, message)) = update_check_feedback(&error, trigger) {
                        self.notify_toast(kind, message, cx);
                    }
                }
            }
            UiEvent::UpdateDownloadProgress { downloaded, total } => {
                let mb_down = downloaded as f64 / 1_048_576.0;
                let mb_total = total as f64 / 1_048_576.0;
                self.update_download_progress =
                    Some(format!("{:.1} MB / {:.1} MB", mb_down, mb_total));
            }
            UiEvent::UpdateReadyToInstall {
                staging_dir,
                manifest,
            } => {
                self.update_downloading = false;
                self.update_download_progress = None;
                self.status = format!("更新 v{} 已准备就绪", manifest.version);
                self.last_error = None;
                self.active_dialog = Some(DialogState::ConfirmInstallUpdate {
                    version: manifest.version.clone(),
                });
                // 保存 staging_dir 以便安装
                self.available_update = Some(manifest);
                self.staging_dir_for_install = Some(staging_dir);
            }
            UiEvent::UpdateInstallFailed { error } => {
                self.update_downloading = false;
                self.update_download_progress = None;
                self.update_error = Some(error.clone());
                self.status = "更新失败".into();
                self.notify_toast(AppToastKind::Error, format!("更新失败：{error}"), cx);
            }
            UiEvent::BackgroundTaskPanicked { message } => {
                // 无法定位具体是哪个任务 panic：保守复位所有 tab 的 busy/加载
                // 标志与仓库加载槽位（序号守卫会丢弃迟到的旧结果，复位是安全的）。
                for tab in self.tabs.iter_mut() {
                    tab.busy = false;
                    tab.loading = RepositoryLoading::default();
                    tab.history_loading = HistoryLoading::default();
                }
                self.active_repository_loads = 0;
                // 全局一次性标志同样复位：AI 生成（commit/冲突合并）、思考
                // 弹窗、全局 busy 槽位、更新下载与工作流编辑器的 AI 生成
                // 标志——任何一项卡死都会永久阻断对应入口（按钮禁用 /
                // 「已有操作正在运行」）。
                self.ai_commit_loading = false;
                self.ai_conflict_loading = false;
                self.ai_thinking_overlay = None;
                self.global_busy_tab = None;
                self.update_downloading = false;
                // 代码索引任务无法区分是否 panic 来源：置空全局单任务守卫，
                // 否则卡死的守卫会永久挡掉手动/自动索引入口与设置页状态卡。
                self.code_index_task = None;
                self.code_index_progress_message.clear();
                self.reset_ai_loading_after_panic();
                self.status = "后台任务异常".into();
                self.notify_toast(
                    AppToastKind::Error,
                    format!("后台任务异常退出，已复位操作状态：{message}"),
                    cx,
                );
            }
            UiEvent::AmendPrefillLoaded { tab_id, message } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅当仍处于修补模式且输入框仍为空时填入：期间用户可能
                    // 已关闭开关或手动输入了内容。
                    if this.amend_mode
                        && this.commit_message.value.trim().is_empty()
                        && let Some(message) = message.filter(|m| !m.trim().is_empty())
                    {
                        this.amend_prefill = Some(message.clone());
                        this.commit_message.set_value(message);
                        this.commit_message.caret = 0;
                    }
                });
            }
        }
        cx.notify();
    }
}
