//! RepositoryView 的生命周期、会话、事件泵与应用级协调。

use crate::*;

impl RepositoryView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let (storage, storage_status, storage_error) = Self::open_storage();
        let credential_store = Arc::new(KeyringCredentialStore::with_storage(storage.clone()));
        let ai_settings = Self::load_ai_provider_settings(&storage);
        let remote_credential_bindings =
            Arc::new(Mutex::new(Self::load_remote_credential_bindings(&storage)));
        let proxy_settings = Self::load_proxy_settings(&storage);
        let external_merge_settings = Self::load_external_merge_settings(&storage);
        let theme_mode = Self::load_theme_mode(&storage);
        let theme_accent = Self::load_theme_accent(&storage);
        let layout_preferences = Self::load_layout_preferences(&storage);
        let proxy_custom = proxy_settings.custom.normalized();
        #[cfg(windows)]
        let (tray, tray_error) = match tray::TrayController::new() {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(error)),
        };
        Self::spawn_event_pump(rx.clone(), cx);
        Self::spawn_ui_tick(tx.clone());
        let tasks = TaskExecutor::new(tx.clone());

        Self {
            tx,
            rx,
            tasks,
            storage: storage.clone(),
            credential_store,
            remote_credential_bindings,
            credential_records: Vec::new(),
            workflow_templates: Vec::new(),
            workflow_template_dir: workflow_templates_dir(),
            workflow_editor: None,
            pending_workflow_edit: None,
            workflow_template_context_menu: None,
            diff_encoding_preferences: Self::load_diff_encoding_preferences(&storage),
            diff_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(DIFF_CACHE_CAPACITY)
                    .expect("diff cache capacity must be nonzero"),
            )),
            proxy_settings: proxy_settings.clone(),
            theme_mode,
            theme_accent,
            tabs: Vec::new(),
            active_tab: None,
            global_busy_tab: None,
            next_tab_id: 1,
            fallback_tab: {
                let mut tab = RepoTabState::new(RepoTabId(0), None);
                if let Some(status) = storage_status {
                    tab.status = status;
                    tab.last_error = storage_error;
                }
                tab
            },
            restoring_session: false,
            // 布局偏好：启动时从 layout_preferences 恢复（空库回默认常量），
            // 宽度按 MIN/MAX 钳制——防手改 DB 或常量演进导致越界布局。
            context_navigator_preferences: ContextNavigatorPreferences {
                visible: layout_preferences.navigator_visible.unwrap_or(true),
            },
            sidebar_width: layout_preferences
                .sidebar_width
                .map(|width| width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH))
                .unwrap_or(DEFAULT_SIDEBAR_WIDTH),
            changes_width: layout_preferences
                .changes_width
                .map(|width| width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH))
                .unwrap_or(DEFAULT_CHANGES_WIDTH),
            workflow_templates_width: layout_preferences
                .workflow_templates_width
                .map(|width| {
                    width.clamp(MIN_WORKFLOW_TEMPLATES_WIDTH, MAX_WORKFLOW_TEMPLATES_WIDTH)
                })
                .unwrap_or(DEFAULT_WORKFLOW_TEMPLATES_WIDTH),
            history_files_width: layout_preferences
                .history_files_width
                .map(|width| width.clamp(MIN_HISTORY_FILES_WIDTH, MAX_HISTORY_FILES_WIDTH))
                .unwrap_or(DEFAULT_HISTORY_FILES_WIDTH),
            history_inspector_files_width: layout_preferences
                .history_inspector_files_width
                .map(|width| {
                    width.clamp(
                        MIN_HISTORY_INSPECTOR_FILES_WIDTH,
                        MAX_HISTORY_INSPECTOR_FILES_WIDTH,
                    )
                })
                .unwrap_or(DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH),
            history_details_height: layout_preferences
                .history_details_height
                .map(|height| height.clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT)),
            history_details_collapsed: layout_preferences.history_details_collapsed,
            history_details_top_hint: Arc::new(Cell::new(0.0)),
            browse_tree_width: layout_preferences
                .browse_tree_width
                .map(|width| width.clamp(MIN_BROWSE_TREE_WIDTH, MAX_BROWSE_TREE_WIDTH))
                .unwrap_or(DEFAULT_BROWSE_TREE_WIDTH),
            history_graph_width: layout_preferences
                .history_graph_width
                .map(|width| width.clamp(MIN_HISTORY_GRAPH_WIDTH, MAX_HISTORY_GRAPH_WIDTH))
                .unwrap_or(DEFAULT_HISTORY_GRAPH_WIDTH),
            resizing_sidebar_width: None,
            resizing_changes_width: None,
            resizing_workflow_templates_width: None,
            resizing_history_files_width: None,
            resizing_history_inspector_files_width: None,
            resizing_history_details_height: None,
            resizing_browse_tree_width: None,
            resizing_history_graph_width: None,
            resizing_understanding_source_width: None,
            understanding_source_width: code_understanding_view::UNDERSTANDING_SOURCE_DEFAULT_WIDTH,
            scroll_handles: RefCell::new(HashMap::new()),
            uniform_scroll_handles: RefCell::new(HashMap::new()),
            widest_diff_row_cache: RefCell::new(WidestDiffRowCache::default()),
            scrollbar_drag: None,
            pending_credential: None,
            pending_credentials: VecDeque::new(),
            repository_load_queue: VecDeque::new(),
            active_repository_loads: 0,
            feedbacks: VecDeque::new(),
            next_feedback_id: 0,
            progress_phase: 0,
            active_dialog: None,
            settings_center: None,
            shortcut_bindings: Self::load_shortcut_bindings(&storage),
            workflow_shortcut_bindings: Self::load_workflow_shortcut_bindings(&storage),
            recording_shortcut: None,
            settings_center_focus: cx.focus_handle(),
            workflow_shortcut_binding_focus: cx.focus_handle(),
            dialog_before_window_close: None,
            exit_requested: false,
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            tray_error,
            branch_context_menu: None,
            remote_context_menu: None,
            change_context_menu: None,
            file_path_context_menu: None,
            credential_context_menu: None,
            tag_context_menu: None,
            stash_context_menu: None,
            commit_context_menu: None,
            encoding_menu_target: None,
            encoding_menu_closed_by_capture: None,
            commit_graph_branch_menu_closed_by_capture: false,
            repo_switcher_menu: None,
            context_navigator_overlay_open: false,
            repo_switcher_anchor: None,
            repo_switcher_recent: Vec::new(),
            repo_switcher_search: TextFieldState::new(cx, "搜索仓库"),
            commit_graph_search: TextFieldState::new(cx, "搜索提交/作者/SHA"),
            commit_graph_branch_search: TextFieldState::new(cx, "搜索分支"),
            repo_switcher_search_open: false,
            save_credential: false,
            credential_scope: CredentialScope::RemoteUrl,
            credential_form_mode: CredentialFormMode::Https,
            credential_use_ssh_agent: false,
            ssh_credential_discovery: SshCredentialDiscoveryState::default(),
            oauth_login_flow: OAuthLoginFlowState::default(),
            pending_gitee_refresh_record: None,
            clone_url: TextFieldState::new(cx, "远程仓库 URL"),
            clone_path: TextFieldState::new(cx, "克隆到父文件夹"),
            clone_recursive_submodules: default_clone_recursive_submodules(),
            branch_name: TextFieldState::new(cx, "新分支名称"),
            create_branch_checkout: true,
            branch_rename: TextFieldState::new(cx, "重命名为"),
            commit_message: TextFieldState::new(cx, "提交信息"),
            amend_mode: false,
            amend_prefill: None,
            stash_message: TextFieldState::new(cx, "贮藏说明（可选）"),
            tag_name: TextFieldState::new(cx, "标签名称，如 v1.0.0"),
            tag_message: TextFieldState::new(cx, "标签附注信息（可选）"),
            tag_annotated: true,
            tag_push_remote: None,
            stash_include_untracked: false,
            stash_keep_index: false,
            credential_username: TextFieldState::new(cx, "用户名"),
            credential_secret: TextFieldState::new(cx, "密码或 PAT").secret(),
            credential_key_path: TextFieldState::new(cx, "SSH 私钥路径"),
            credential_passphrase: TextFieldState::new(cx, "SSH 密码短语").secret(),

            credential_remote_url: TextFieldState::new(cx, "适用远端 URL"),
            credential_test_url: TextFieldState::new(cx, "测试地址"),
            credential_test_error: None,
            credential_display_name: TextFieldState::new(cx, "凭据名称（可选）"),
            conflict_editor: TextFieldState::new(cx, "冲突结果"),
            remote_name: TextFieldState::new(cx, "远端名称"),
            remote_url: TextFieldState::new(cx, "远端地址"),
            remote_credential_policy: RemoteCredentialPolicy::AutoMatch,
            remote_branch_name: TextFieldState::new(cx, "远程分支"),
            remote_branch_search: TextFieldState::new(cx, "搜索远端分支"),
            sidebar_local_branch_search: TextFieldState::new(cx, "搜索本地分支"),
            sidebar_remote_branch_search: TextFieldState::new(cx, "搜索远端分支"),
            sidebar_local_branch_search_open: false,
            sidebar_remote_branch_search_open: false,
            remote_branch_operation: RemoteBranchOperationState::default(),
            proxy_mode: proxy_settings.mode,
            external_merge_enabled_form: external_merge_settings.enabled,
            external_merge_auto_open_form: external_merge_settings.auto_open_intellij,
            external_merge_settings: external_merge_settings.clone(),
            ai_enabled_form: ai_settings.enabled,
            ai_settings,
            ai_base_url: TextFieldState::new(cx, "Base URL，例如 https://api.openai.com/v1"),
            ai_api_key: TextFieldState::new(cx, "API Key").secret(),
            ai_model: TextFieldState::new(cx, "模型名称，例如 gpt-4o-mini"),
            external_merge_intellij_path: TextFieldState::new(cx, "IntelliJ IDEA 路径（可选）")
                .with_value(external_merge_settings.normalized_intellij_path()),
            external_merge_detection: None,
            ai_commit_loading: false,
            ai_review: None,
            ai_review_loading: false,
            ai_review_steps: Vec::new(),
            ai_review_progress: None,
            ai_review_step_expanded: BTreeSet::new(),
            ai_review_live_reasoning: String::new(),
            ai_review_live_content: String::new(),
            ai_review_expanded: false,
            ai_review_next_generation: 1,
            ai_review_active_generation: None,
            ai_review_running_tasks: 0,
            ai_review_cancel: None,
            ai_review_loaded_label: None,
            ai_review_history: None,
            conflict_pane_scroll_sync: Rc::new(RefCell::new(None)),
            ai_conflict_loading: false,
            ai_thinking_overlay: None,
            ai_thinking_follow_state: std::rc::Rc::new(AiThinkingFollowState {
                last_key: std::cell::Cell::new((usize::MAX, usize::MAX)),
            }),
            code_index_task: None,
            code_index_stats: HashMap::new(),
            code_index_filter: TextFieldState::new(cx, "按名称或路径过滤"),
            code_index_list_entries: Vec::new(),
            code_index_progress_done: 0,
            code_index_progress_total: 0,
            code_index_progress_message: String::new(),
            code_search_palette: None,
            code_palette_search: TextFieldState::new(cx, "搜索符号或类型名…"),
            understanding_question: TextFieldState::new(
                cx,
                code_understanding_view::QUESTION_PLACEHOLDER,
            )
            .with_compact_multiline(),
            understanding_tasks: code_understanding_view::UnderstandingTaskRegistry::default(),
            understanding_notice: None,
            code_palette_search_seq: 0,
            code_palette_detail_seq: 0,
            code_index_enabled_cache: {
                let mut cache = std::collections::HashSet::new();
                if let Ok(prefs) = storage.load_code_index_preferences() {
                    cache.extend(
                        prefs
                            .repositories
                            .into_iter()
                            .filter(|(_, enabled)| *enabled)
                            .map(|(repo, _)| repo),
                    );
                }
                cache
            },
            // ── 更新状态 ──
            update_preferences: Self::load_update_preferences(&storage),
            update_checking: false,
            next_periodic_update_check: Instant::now() + PERIODIC_UPDATE_CHECK_INTERVAL,
            update_downloading: false,
            available_update: None,
            update_download_progress: None,
            update_error: None,
            staging_dir_for_install: None,
            proxy_http_url: TextFieldState::new(cx, "HTTP 代理 URL")
                .with_value(proxy_custom.http_proxy),
            proxy_https_url: TextFieldState::new(cx, "HTTPS 代理 URL")
                .with_value(proxy_custom.https_proxy),
            proxy_socks5_url: TextFieldState::new(cx, "SOCKS5 代理 URL")
                .with_value(proxy_custom.socks5_proxy),
        }
    }

    fn spawn_ui_tick(tx: Sender<UiEvent>) {
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(420));
                if tx.try_send(UiEvent::UiTick).is_err() {
                    break;
                }
            }
        });
    }

    pub(crate) fn new_with_session(cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(cx);
        view.restore_session();
        // 启动时自动检查更新（Startup：结果不弹气泡，仅状态栏；
        // 发现新版本仍会弹窗——既有行为不变）
        if view.update_preferences.auto_check {
            view.start_update_check(UpdateCheckTrigger::Startup);
        }
        // 老用户首次进入便携版本时，提示把数据从 C 盘迁移到程序同级目录。
        view.maybe_prompt_portable_migration();
        // exe 位于临时/聊天软件接收/下载目录时，提示移动程序到安全目录。
        view.maybe_prompt_exe_relocation();
        view
    }

    /// 检测当前是否需要提供「迁移到便携目录」入口。
    /// 仅当应用当前仍在旧目录（C 盘）运行、且未在排队待迁移时返回真；
    /// 一旦已切换到便携目录（新机器或迁移完成后），即返回假，设置中心入口随之隐藏。
    /// 不检查「不再提示」标记：即使用户曾选择保持现状，仍可从设置中心手动触发迁移。
    pub(crate) fn portable_migration_available(&self) -> bool {
        let (Some(active), Some(portable)) = (
            khaslana::default_database_path(),
            khaslana::portable_database_path(),
        ) else {
            return false;
        };
        // 当前已启用便携目录（新机器或已完成迁移）→ 无需迁移入口。
        if active == portable {
            return false;
        }
        // 已排队待迁移（下次启动搬运）→ 不重复显示。
        if khaslana::portable_pending_marker().is_some_and(|p| p.exists()) {
            return false;
        }
        // 当前激活路径（旧目录）的库确实存在 → 可迁移。
        active.exists()
    }

    /// 检测是否应提示用户把数据从旧目录（C 盘）迁移到便携目录。
    fn maybe_prompt_portable_migration(&mut self) {
        if !self.portable_migration_available() {
            return;
        }
        if self.storage.portable_migration_dismissed() {
            return;
        }
        self.active_dialog = Some(DialogState::PortableMigrationPrompt);
    }

    /// 用户确认迁移：写入待迁移标记后重启应用，下次启动在打开数据库前完成搬运。
    pub(crate) fn confirm_portable_migration(&mut self) {
        let _ = self
            .storage
            .set_meta_value("pending_portable_migration", "1");
        if let Some(marker) = khaslana::portable_pending_marker() {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, []);
        }
        self.active_dialog = None;
        Self::relaunch_app();
    }

    /// 用户保持现状：永久忽略便携迁移提示。
    pub(crate) fn dismiss_portable_migration(&mut self) {
        let _ = self.storage.mark_portable_migration_dismissed();
        self.active_dialog = None;
    }

    /// 重启应用：启动新实例后立即退出当前进程。
    fn relaunch_app() {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("khaslana.exe"));
        let exe_str = exe.to_string_lossy().to_string();
        let _ = std::process::Command::new(&exe_str).spawn();
        std::process::exit(0);
    }

    // ── 程序位置风险搬迁（exe 位于临时/聊天软件接收/下载目录） ──

    /// 当前是否应提供「移动程序到安全目录」入口。
    /// dismiss 标记只抑制启动弹窗，不影响设置中心手动入口。
    pub(crate) fn exe_relocation_available(&self) -> bool {
        if khaslana::current_exe_location_risk() == khaslana::ExeLocationRisk::Safe {
            return false;
        }
        // 已排队待搬迁 → 不重复提示。
        if khaslana::exe_relocation_pending_marker().is_some_and(|marker| marker.exists()) {
            return false;
        }
        true
    }

    /// 检测是否应提示用户把程序（连同数据）移出风险目录。
    fn maybe_prompt_exe_relocation(&mut self) {
        if !self.exe_relocation_available() {
            return;
        }
        if self.storage.exe_relocation_dismissed() {
            return;
        }
        // 便携迁移提示优先；两者都满足时本轮只弹一个，位置风险下一轮再提示。
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(DialogState::ExeRelocationPrompt);
    }

    /// 用户确认搬迁：写入待搬迁标记（固定目录下）后重启，
    /// 下次启动最早期把程序与数据搬到安全目录并从新位置运行。
    pub(crate) fn confirm_exe_relocation(&mut self) {
        self.active_dialog = None;
        let Some(target) = khaslana::exe_relocation_target_dir() else {
            self.last_error = Some("无法定位搬迁目标目录".to_string());
            return;
        };
        if let Err(err) = khaslana::request_exe_relocation(&target) {
            self.last_error = Some(err.to_string());
            return;
        }
        Self::relaunch_app();
    }

    /// 用户保持现状：永久忽略程序位置风险提示（数据本身已受解析规则保护）。
    pub(crate) fn dismiss_exe_relocation(&mut self) {
        let _ = self.storage.mark_exe_relocation_dismissed();
        self.active_dialog = None;
    }

    pub(crate) fn scroll_handle(&self, id: &'static str) -> ScrollHandle {
        let scoped_id = self
            .active_tab
            .map(|tab_id| format!("tab-{}:{id}", tab_id.0))
            .unwrap_or_else(|| format!("global:{id}"));
        self.scroll_handles
            .borrow_mut()
            .entry(scoped_id)
            .or_default()
            .clone()
    }

    pub(crate) fn scroll_local_branch_to_current(&self) {
        // 分组标题钉住后，本地分支是独立虚拟列表：在过滤后的条目模型里定位
        // HEAD 行号，交给该列表的滚动目标机制。分组折叠时不渲染列表，跳过定位。
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if !self
            .sidebar_sections
            .is_expanded(SidebarSection::LocalBranches)
        {
            return;
        }
        let Some(head_branch_index) = snapshot
            .branches
            .iter()
            .position(|branch| branch.kind == BranchKind::Local && branch.is_head)
        else {
            return;
        };
        let local_query = if self.sidebar_local_branch_search_open {
            self.sidebar_local_branch_search.value.trim()
        } else {
            ""
        };
        let entries = sidebar_view::sidebar_local_branch_entries(&snapshot.branches, local_query);
        let Some(row) = entries.iter().position(|item| {
            matches!(item, sidebar_view::SidebarNavItem::Branch(index) if *index == head_branch_index)
        }) else {
            return;
        };
        self.uniform_scroll_handle(sidebar_view::sidebar_section_scroll_id(
            SidebarSection::LocalBranches,
        ))
        .scroll_to_item(row, ScrollStrategy::Center);
    }

    pub(crate) fn uniform_scroll_handle(&self, id: &'static str) -> UniformListScrollHandle {
        let scoped_id = self
            .active_tab
            .map(|tab_id| format!("tab-{}:{id}", tab_id.0))
            .unwrap_or_else(|| format!("global:{id}"));
        self.uniform_scroll_handles
            .borrow_mut()
            .entry(scoped_id)
            .or_insert_with(UniformListScrollHandle::new)
            .clone()
    }

    pub(crate) fn reset_uniform_scroll(&self, id: &'static str) {
        let handle = self.uniform_scroll_handle(id);
        handle
            .0
            .borrow_mut()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    pub(crate) fn active_tab_id(&self) -> Option<RepoTabId> {
        self.active_tab
    }

    pub(crate) fn active_tab(&self) -> Option<&RepoTabState> {
        let id = self.active_tab?;
        self.tabs.iter().find(|tab| tab.id == id)
    }

    pub(crate) fn tab_mut(&mut self, tab_id: RepoTabId) -> Option<&mut RepoTabState> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    pub(crate) fn tab(&self, tab_id: RepoTabId) -> Option<&RepoTabState> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
    }

    pub(crate) fn ensure_tab_for_path(&mut self, path: PathBuf) -> RepoTabId {
        // 记录切换前的主模式：打开/切换仓库保持「当前区域」不变
        //（主模式跟随；专用页面绑定 per-repo 状态不继承）。
        let previous_mode = self.current_tab_main_mode();
        let key = normalize_repo_path(&path);
        let existing_id = self
            .tabs
            .iter()
            .find(|tab| tab.path_key().as_deref() == Some(key.as_str()))
            .map(|tab| tab.id);
        if let Some(id) = existing_id {
            if let Some(tab) = self.tab_mut(id) {
                tab.last_active_at = now_epoch_secs();
            }
            self.active_tab = Some(id);
            self.inherit_main_mode(previous_mode);
            self.save_session();
            return id;
        }

        let id = RepoTabId(self.next_tab_id);
        self.next_tab_id = self.next_tab_id.wrapping_add(1).max(1);
        self.tabs.push(RepoTabState::new(id, Some(path)));
        self.active_tab = Some(id);
        self.inherit_main_mode(previous_mode);
        self.save_session();
        id
    }

    pub(crate) fn activate_tab(&mut self, tab_id: RepoTabId) {
        if self.active_tab == Some(tab_id) || self.tab(tab_id).is_none() {
            return;
        }
        let previous_mode = self.current_tab_main_mode();
        if self.active_dialog == Some(DialogState::SubmoduleManager) {
            self.close_dialog();
        }
        self.close_popups();
        if let Some(active) = self.active_tab
            && let Some(tab) = self.tab_mut(active)
        {
            tab.release_large_diff_caches();
            tab.submodule_dialog.invalidate();
        }
        self.active_tab = Some(tab_id);
        if let Some(tab) = self.tab_mut(tab_id) {
            tab.last_active_at = now_epoch_secs();
            // 记录最近打开时间，供仓库切换下拉排序。
            if let Some(path) = tab.repo_path.clone() {
                let _ = self.storage.upsert_recent_repo(&path);
            }
        }
        // 切换仓库保持当前区域：主模式（工作区/提交记录/工作流/图谱）跟随切换
        // 带过去；专用页面（追溯/浏览/冲突等）不继承，落回目标 tab 自身模式。
        self.inherit_main_mode(previous_mode);
        self.ensure_history_loaded();
        self.sync_conflict_mode_with_snapshot();
        self.save_session();
        // 切换仓库后自动本地刷新
        self.refresh();
    }

    /// 当前激活 tab 的主模式（无激活 tab 时 None）。
    pub(crate) fn current_tab_main_mode(&self) -> Option<MainMode> {
        self.active_tab
            .and_then(|id| self.tab(id))
            .map(|tab| tab.main_mode)
    }

    /// 主模式继承：切换/打开/克隆仓库时把切换前所在的主页面写到新激活的 tab。
    /// 专用模式（Conflict/Stash/Browse/Blame）绑定 per-repo 状态，不继承。
    /// 必须在 `active_tab` 已指向目标 tab 之后调用（经 Deref 写入该 tab）。
    pub(crate) fn inherit_main_mode(&mut self, previous: Option<MainMode>) {
        if let Some(mode) = inheritable_main_mode(previous) {
            self.main_mode = mode;
            if mode == MainMode::CodeUnderstanding {
                // 与 `set_main_mode` 对齐：切换/打开仓库后继承理解页时，
                // 必须补齐新仓库的索引统计投影与本地完成历史，否则页头与
                // 左侧导航器会停在上一仓库（或空）的状态上。
                self.ensure_understanding_index_stats();
                self.ensure_understanding_history_loaded();
            }
        }
    }

    pub(crate) fn close_tab(&mut self, tab_id: RepoTabId) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if self.active_tab == Some(tab_id)
            && self.active_dialog == Some(DialogState::SubmoduleManager)
        {
            self.close_dialog();
        }
        self.close_popups();
        self.tabs.remove(index);
        // 清理该 tab 的滚动句柄：句柄按 `tab-{id}:` 前缀入表，不随 tab 关闭
        // 移除会长期缓慢积累（频繁开合仓库时无上限）。
        {
            let prefix = format!("tab-{}:", tab_id.0);
            self.scroll_handles
                .borrow_mut()
                .retain(|key, _| !key.starts_with(&prefix));
            self.uniform_scroll_handles
                .borrow_mut()
                .retain(|key, _| !key.starts_with(&prefix));
        }
        let mut retained = VecDeque::new();
        while let Some(pending) = self.pending_credentials.pop_front() {
            if pending.tab_id == Some(tab_id) {
                send_credential_response(&pending, Ok(None));
            } else {
                retained.push_back(pending);
            }
        }
        self.pending_credentials = retained;
        self.repository_load_queue
            .retain(|request| request.tab_id != tab_id);
        if self
            .pending_credential
            .as_ref()
            .and_then(|pending| pending.tab_id)
            == Some(tab_id)
        {
            if let Some(pending) = self.pending_credential.as_ref() {
                send_credential_response(pending, Ok(None));
            }
            self.show_next_credential_request();
        }
        if self.active_tab == Some(tab_id) {
            self.active_tab = self
                .tabs
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.tabs.get(prev)))
                .map(|tab| tab.id);
        }
        self.save_session();
    }

    fn open_storage() -> (Arc<khaslana::AppStorage>, Option<String>, Option<String>) {
        match khaslana::AppStorage::open_default() {
            Ok(storage) => (Arc::new(storage), None, None),
            Err(first_err) => {
                tracing::warn!("local config database open failed, recreating: {first_err}");
                match khaslana::AppStorage::recreate_default_after_failure() {
                    Ok(storage) => (
                        Arc::new(storage),
                        Some("本地配置数据库已重建".to_string()),
                        Some(format!("原数据库打开失败，已创建空数据库：{first_err}")),
                    ),
                    Err(second_err) => {
                        tracing::warn!(
                            "local config database recreate failed, using memory database: {second_err}"
                        );
                        let storage =
                            khaslana::AppStorage::open_in_memory().unwrap_or_else(|err| {
                                panic!("无法创建临时配置数据库：{err}");
                            });
                        (
                            Arc::new(storage),
                            Some("正在使用临时配置数据库".to_string()),
                            Some(format!("本地配置数据库不可用：{second_err}")),
                        )
                    }
                }
            }
        }
    }

    fn load_session_state(&self) -> Option<SessionState> {
        match self.storage.load_session_state() {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!("session load skipped: {err}");
                None
            }
        }
    }

    fn load_diff_encoding_preferences(storage: &khaslana::AppStorage) -> DiffEncodingPreferences {
        storage
            .load_diff_encoding_preferences()
            .inspect_err(|err| tracing::warn!("diff encoding preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_remote_credential_bindings(storage: &khaslana::AppStorage) -> RemoteCredentialBindings {
        storage
            .load_remote_credential_bindings()
            .inspect_err(|err| tracing::warn!("remote credential bindings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_proxy_settings(storage: &khaslana::AppStorage) -> NetworkProxySettings {
        storage
            .load_proxy_settings()
            .inspect_err(|err| tracing::warn!("network proxy settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_ai_provider_settings(storage: &khaslana::AppStorage) -> AiProviderSettings {
        storage
            .load_ai_provider_settings()
            .inspect_err(|err| tracing::warn!("ai provider settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_external_merge_settings(storage: &khaslana::AppStorage) -> ExternalMergeSettings {
        storage
            .load_external_merge_settings()
            .inspect_err(|err| tracing::warn!("external merge settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_update_preferences(storage: &khaslana::AppStorage) -> UpdatePreferences {
        storage
            .load_update_preferences()
            .inspect_err(|err| tracing::warn!("update preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    /// 加载快捷键绑定；存储为空或出错时回退全部默认值。
    pub(crate) fn load_shortcut_bindings(storage: &khaslana::AppStorage) -> ShortcutBindings {
        let stored = storage
            .load_shortcut_bindings()
            .inspect_err(|err| tracing::warn!("shortcut bindings load skipped: {err}"))
            .unwrap_or_default();
        // 合并：存储的绑定优先，缺失的动作用默认值补齐，保证新动作一定有快捷键。
        let mut result = default_shortcut_bindings();
        for (id, keystroke) in stored.bindings {
            if ShortcutAction::from_id(&id).is_some() {
                result.bindings.insert(id, keystroke);
            }
        }
        result
    }

    /// 保存当前快捷键绑定到数据库。
    pub(crate) fn save_shortcut_bindings(&self) {
        if let Err(err) = self.storage.save_shortcut_bindings(&self.shortcut_bindings) {
            tracing::warn!("shortcut bindings write skipped: {err}");
        }
    }

    /// 加载工作流快捷键绑定；存储为空或出错时回退空表。加载时做防御剪枝
    /// （撞静态键/同键重复/坏键位丢弃），有变化则把剪枝结果写回存储。
    pub(crate) fn load_workflow_shortcut_bindings(
        storage: &khaslana::AppStorage,
    ) -> khaslana::WorkflowShortcutBindings {
        let stored = storage
            .load_workflow_shortcut_bindings()
            .inspect_err(|err| tracing::warn!("workflow shortcut bindings load skipped: {err}"))
            .unwrap_or_default();
        let app_bindings = Self::load_shortcut_bindings(storage);
        let (pruned, changed) = prune_workflow_shortcut_bindings(&stored, &app_bindings);
        if changed {
            tracing::warn!("workflow shortcut bindings pruned on load");
            let _ = storage.save_workflow_shortcut_bindings(&pruned);
        }
        pruned
    }

    /// 保存工作流快捷键绑定到数据库（整体覆盖）。
    pub(crate) fn save_workflow_shortcut_bindings(&self) {
        if let Err(err) = self
            .storage
            .save_workflow_shortcut_bindings(&self.workflow_shortcut_bindings)
        {
            tracing::warn!("workflow shortcut bindings write skipped: {err}");
        }
    }

    /// 保存工作流绑定并全量重注册键位（写入后的统一出口，保证绑定即刻生效）。
    pub(crate) fn persist_workflow_shortcut_bindings(&mut self, cx: &mut Context<Self>) {
        self.save_workflow_shortcut_bindings();
        crate::register_all_key_bindings(
            &mut cx.deref_mut(),
            &self.shortcut_bindings,
            &self.workflow_shortcut_bindings,
            false,
        );
        cx.notify();
    }

    /// 加载布局偏好；存储为空或出错时回退全部默认值。
    fn load_layout_preferences(storage: &khaslana::AppStorage) -> khaslana::LayoutPreferences {
        storage
            .load_layout_preferences()
            .inspect_err(|err| tracing::warn!("layout preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    /// 保存布局偏好（导航器展开 + 全部分割线位置）。
    ///
    /// 仅在离散用户动作后调用（拖拽结束/双击复位/导航器开合/详情卡折叠），
    /// UI 线程同步写：单行 <200B 的本地 SQLite 写无感知卡顿，同步保证操作
    /// 顺序落库且「改完即关应用」不丢最后一次修改（与主题/快捷键保存同模式）。
    pub(crate) fn save_layout_preferences(&self) {
        let preferences = khaslana::LayoutPreferences {
            navigator_visible: Some(self.context_navigator_preferences.visible),
            sidebar_width: Some(self.sidebar_width),
            changes_width: Some(self.changes_width),
            workflow_templates_width: Some(self.workflow_templates_width),
            history_files_width: Some(self.history_files_width),
            history_inspector_files_width: Some(self.history_inspector_files_width),
            history_graph_width: Some(self.history_graph_width),
            browse_tree_width: Some(self.browse_tree_width),
            history_details_height: self.history_details_height,
            history_details_collapsed: self.history_details_collapsed,
        };
        if let Err(err) = self.storage.save_layout_preferences(&preferences) {
            tracing::warn!("layout preferences write skipped: {err}");
        }
    }

    fn save_diff_encoding_preferences(&self) {
        if let Err(err) = self
            .storage
            .save_diff_encoding_preferences(&self.diff_encoding_preferences)
        {
            tracing::warn!("diff encoding preferences write skipped: {err}");
        }
    }

    pub(crate) fn save_remote_credential_bindings(&self) {
        let Ok(bindings) = self.remote_credential_bindings.lock() else {
            tracing::warn!("remote credential bindings state read skipped");
            return;
        };
        if let Err(err) = self.storage.save_remote_credential_bindings(&bindings) {
            tracing::warn!("remote credential bindings write skipped: {err}");
        }
    }

    pub(crate) fn save_ai_provider_settings(&self) {
        if let Err(err) = self.storage.save_ai_provider_settings(&self.ai_settings) {
            tracing::warn!("ai provider settings write skipped: {err}");
        }
    }

    pub(crate) fn save_external_merge_settings(&self) {
        if let Err(err) = self
            .storage
            .save_external_merge_settings(&self.external_merge_settings)
        {
            tracing::warn!("external merge settings write skipped: {err}");
        }
    }

    pub(crate) fn save_proxy_settings(&self) {
        if let Err(err) = self.storage.save_proxy_settings(&self.proxy_settings) {
            tracing::warn!("network proxy settings write skipped: {err}");
        }
    }

    pub(crate) fn diff_encoding_choice_for_path(&self, path: &Path) -> DiffEncodingChoice {
        self.diff_encoding_preferences
            .repositories
            .get(&normalize_repo_path(path))
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn current_diff_encoding_choice(&self) -> DiffEncodingChoice {
        self.repo_path
            .as_ref()
            .map(|path| self.diff_encoding_choice_for_path(path))
            .unwrap_or_default()
    }

    pub(crate) fn set_current_diff_encoding(&mut self, encoding: DiffEncodingChoice) {
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let key = normalize_repo_path(&repo_path);
        if encoding == DiffEncodingChoice::Auto {
            self.diff_encoding_preferences.repositories.remove(&key);
        } else {
            self.diff_encoding_preferences
                .repositories
                .insert(key, encoding);
        }
        self.save_diff_encoding_preferences();
        self.status = format!("差异编码已切换为 {}", encoding.label());
        self.reload_visible_diffs_after_encoding_change();
    }

    fn reload_visible_diffs_after_encoding_change(&mut self) {
        self.diff_cache.borrow_mut().clear();
        if let Some(diff) = self.diff.clone() {
            self.load_diff(diff.path.clone(), diff.scope.clone());
        }
        if self.main_mode == MainMode::History
            && let Some(path) = self.history_selected_file.clone()
        {
            self.select_history_file_with_reload(path, true);
        }
        if self.main_mode == MainMode::Stash
            && let Some(path) = self.stash_preview.selected_file.clone()
        {
            self.select_stash_file(path, true);
        }
        if self.main_mode == MainMode::Browse {
            self.reload_browse_on_encoding_change();
        }
        if self.main_mode == MainMode::Blame {
            self.reload_blame_on_encoding_change();
        }
    }

    pub(crate) fn save_session(&self) {
        if self.restoring_session {
            return;
        }
        let repo_paths = dedupe_repo_paths(
            self.tabs
                .iter()
                .filter_map(|tab| tab.repo_path.clone())
                .collect::<Vec<_>>(),
        );
        let active_repo_path = self
            .active_tab()
            .and_then(|tab| tab.repo_path.as_ref())
            .cloned();
        let state = SessionState {
            repo_paths,
            active_repo_path,
        };
        if let Err(err) = self.storage.save_session_state(&state) {
            tracing::warn!("session write skipped: {err}");
        }
    }

    fn restore_session(&mut self) {
        let Some(session) = self.load_session_state() else {
            return;
        };
        self.restoring_session = true;
        let mut restored = Vec::new();
        let mut failed = 0usize;
        let mut seen = BTreeSet::new();

        for path in session.repo_paths {
            let key = normalize_repo_path(&path);
            if !seen.insert(key) {
                continue;
            }
            if !path.exists() || Repository::open(&path).is_err() {
                failed += 1;
                continue;
            }
            restored.push(path);
        }

        if restored.is_empty() {
            if failed > 0 {
                self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
                self.fallback_tab.status = "会话恢复失败".to_string();
            }
            self.restoring_session = false;
            self.save_session();
            return;
        }

        let active_key = session
            .active_repo_path
            .as_ref()
            .map(|path| normalize_repo_path(path));
        let mut active = None;
        for path in restored {
            let id = self.ensure_tab_for_path(path.clone());
            if active_key.as_deref() == Some(normalize_repo_path(&path).as_str()) {
                active = Some(id);
            }
        }
        if let Some(active) = active.or(self.active_tab) {
            self.active_tab = Some(active);
        }
        if failed > 0 {
            self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
        }

        let tabs = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        for tab_id in tabs {
            if let Some(path) = self.tab(tab_id).and_then(|tab| tab.repo_path.clone()) {
                self.queue_repository_load(
                    tab_id,
                    path,
                    "正在恢复仓库",
                    "仓库已恢复",
                    LoadPriority::Background,
                );
            }
        }
        self.restoring_session = false;
        self.save_session();
    }

    pub(crate) fn active_tab_state(&self) -> &RepoTabState {
        self.active_tab().unwrap_or_else(|| &self.fallback_tab)
    }

    pub(crate) fn active_tab_state_mut(&mut self) -> &mut RepoTabState {
        let id = self.active_tab;
        if let Some(id) = id
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            return &mut self.tabs[index];
        }
        &mut self.fallback_tab
    }

    pub(crate) fn service_for_tab(&self, tab_id: RepoTabId) -> GitService {
        GitService::new(
            Arc::new(TabCredentialProvider::new(
                self.credential_store.clone(),
                self.storage.clone(),
                self.remote_credential_bindings.clone(),
                self.tx.clone(),
                tab_id,
                self.proxy_settings.clone(),
            )),
            Arc::new(TabProgress {
                tx: self.tx.clone(),
                tab_id,
            }),
        )
        .with_proxy_settings(self.proxy_settings.clone())
    }

    pub(crate) fn with_tab_context<R>(
        &mut self,
        tab_id: RepoTabId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        self.tab(tab_id)?;
        let previous = self.active_tab;
        self.active_tab = Some(tab_id);
        let result = f(self);
        self.active_tab = previous
            .filter(|id| self.tab(*id).is_some())
            .or_else(|| self.tabs.first().map(|tab| tab.id));
        Some(result)
    }

    pub(crate) fn apply_status_event(
        &mut self,
        tab_id: Option<RepoTabId>,
        f: impl FnOnce(&mut Self),
    ) {
        if let Some(tab_id) = tab_id {
            let _ = self.with_tab_context(tab_id, f);
        } else {
            f(self);
        }
    }

    /// 全局测试类操作开始借用 busy：记录发起时的活动 tab 并置 busy。
    pub(crate) fn begin_global_test_busy(&mut self, status: &str) {
        self.global_busy_tab = self.active_tab;
        self.busy = true;
        self.status = status.into();
        self.last_error = None;
    }

    /// 复位全局测试借用的 tab busy；tab 已在测试期间关闭时静默忽略。
    pub(crate) fn end_global_test_busy(&mut self) {
        if let Some(tab_id) = self.global_busy_tab.take() {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.busy = false;
            }
        }
    }

    pub(crate) fn disabled_reason(
        &self,
        enabled: bool,
        fallback: &'static str,
    ) -> Option<&'static str> {
        if enabled {
            None
        } else if self.busy {
            Some("当前操作运行中")
        } else if self.repo_path.is_none() {
            Some("请先打开仓库")
        } else {
            Some(fallback)
        }
    }

    /// 入队凭据请求。返回 `true` 表示该请求被提升为当前待处理（此前没有
    /// pending），`false` 表示已排队（当前表单保持不动，用户输入不丢失）。
    pub(crate) fn enqueue_credential_request(&mut self, pending: PendingCredential) -> bool {
        if pending
            .tab_id
            .is_some_and(|tab_id| self.tab(tab_id).is_none())
        {
            return false;
        }
        if self.pending_credential.is_none() {
            self.pending_credential = Some(pending);
            true
        } else {
            self.pending_credentials.push_back(pending);
            false
        }
    }

    pub(crate) fn show_next_credential_request(&mut self) {
        self.pending_credential = None;
        while let Some(pending) = self.pending_credentials.pop_front() {
            if pending
                .tab_id
                .is_none_or(|tab_id| self.tab(tab_id).is_some())
            {
                self.pending_credential = Some(pending);
                self.prepare_current_credential_prompt();
                break;
            }
        }
    }

    pub(crate) fn prepare_current_credential_prompt(&mut self) {
        let Some(pending) = self.pending_credential.as_ref() else {
            return;
        };
        let request = pending.request.clone();
        self.save_credential = true;
        self.credential_scope = CredentialScope::RemoteUrl;
        self.credential_form_mode = credential_form_mode_for_request(&request);
        self.credential_use_ssh_agent = false;
        self.credential_username.set_value(
            request
                .username_from_url
                .clone()
                .unwrap_or_else(|| "git".to_string()),
        );
        self.credential_secret.clear();
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        self.credential_display_name.clear();
        self.credential_remote_url.set_value(request.url);
    }

    fn spawn_event_pump(rx: Receiver<UiEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |weak: WeakEntity<RepositoryView>, cx| {
            while let Ok(event) = rx.recv().await {
                if weak
                    .update(cx, |this, cx| {
                        this.handle_ui_event(event, cx);
                        this.drain_pending_events(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                let _ = cx.refresh();
            }
        })
        .detach();
    }

    pub(crate) fn drain_pending_events(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = self.rx.try_recv() {
            self.handle_ui_event(event, cx);
        }
    }

    pub(crate) fn merge_metadata_snapshot(&mut self, snapshot: RepositorySnapshot) {
        let mut merged = self.snapshot.take().unwrap_or_default();
        let was_merge_in_progress = merged.merge_in_progress;
        let merge_in_progress = snapshot.merge_in_progress;
        let merge_message = snapshot.merge_message.clone();
        merged.path = snapshot.path;
        merged.head = snapshot.head;
        merged.branches = snapshot.branches;
        merged.remotes = snapshot.remotes;
        merged.tags = snapshot.tags;
        merged.stashes = snapshot.stashes;
        merged.conflicts = snapshot.conflicts;
        merged.merge_in_progress = merge_in_progress;
        merged.merge_message = snapshot.merge_message;
        merged.rebase_in_progress = snapshot.rebase_in_progress;
        self.repo_path = Some(merged.path.clone());
        self.sync_selected_remote(&merged);
        self.change_indexes = ChangeListIndexes::rebuild(&merged.changes);
        self.snapshot = Some(merged);
        self.sync_merge_message_transition(was_merge_in_progress, merge_in_progress, merge_message);
        self.sync_conflict_mode_with_snapshot();
    }

    pub(crate) fn replace_changes(&mut self, changes: Vec<khaslana::WorktreeChange>) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.changes = changes;
            self.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
        } else {
            self.change_indexes = ChangeListIndexes::default();
        }
        self.prune_change_selection();
    }

    pub(crate) fn sync_conflict_mode_with_snapshot(&mut self) {
        let conflict_paths = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.conflicts.clone())
            .unwrap_or_default();
        let auto_open_conflict_mode = !self.merge_in_progress();
        let tab = self.active_tab_state_mut();
        sync_conflict_state_from_paths(
            &mut tab.main_mode,
            &mut tab.conflict_workbench,
            &conflict_paths,
            auto_open_conflict_mode,
        );

        if conflict_paths.is_empty() {
            self.conflict_editor.clear();
            return;
        }
        self.ensure_conflict_views_loaded();
        self.sync_conflict_editor_from_state();
        self.maybe_auto_open_external_merge_for_selected_conflict();
    }

    pub(crate) fn ensure_conflict_views_loaded(&mut self) {
        let paths = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.conflicts.clone())
            .unwrap_or_default();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        for path in paths {
            if self.conflict_workbench.files.contains_key(&path) {
                continue;
            }
            match Repository::open(&repo_path)
                .map_err(khaslana::GitError::from)
                .and_then(|repo| service.conflict_file_view(&repo, Path::new(&path)))
            {
                Ok(view) => {
                    self.conflict_workbench.files.insert(path, view);
                }
                Err(err) => {
                    self.last_error = Some(err.to_string());
                }
            }
        }
    }

    pub(crate) fn sync_conflict_editor_from_state(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.conflict_editor.clear();
            return;
        };
        let Some((kind, draft)) = self
            .conflict_workbench
            .files
            .get(&path)
            .map(|view| (view.kind, view.draft.clone()))
        else {
            self.conflict_editor.clear();
            return;
        };
        if kind != ConflictFileKind::Text {
            self.conflict_editor.clear();
            return;
        }
        if !conflict_editor_should_store_draft(kind) {
            self.conflict_editor.clear();
            self.scroll_conflict_panes_to_selected_block(
                &draft,
                self.selected_conflict_block_start(),
            );
            return;
        }
        if self.conflict_editor.value != draft {
            self.conflict_editor.set_value(draft);
        }
        self.highlight_selected_conflict_block();
    }

    fn selected_conflict_block_start(&self) -> usize {
        let Some(path) = self.conflict_workbench.selected_path.as_ref() else {
            return 0;
        };
        self.conflict_workbench
            .files
            .get(path)
            .and_then(|view| {
                view.blocks
                    .get(
                        self.conflict_workbench
                            .selected_block
                            .min(view.blocks.len().saturating_sub(1)),
                    )
                    .map(|block| block.start)
            })
            .unwrap_or(0)
    }

    fn highlight_selected_conflict_block(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        let Some(block) = view
            .blocks
            .get(
                self.conflict_workbench
                    .selected_block
                    .min(view.blocks.len().saturating_sub(1)),
            )
            .cloned()
        else {
            return;
        };
        let draft = view.draft.clone();
        if conflict_editor_should_store_draft(view.kind) {
            self.conflict_editor.move_caret_to(block.start, false);
            self.conflict_editor.move_caret_to(block.end, true);
        }
        self.scroll_conflict_panes_to_selected_block(&draft, block.start);
    }

    fn scroll_conflict_panes_to_selected_block(&self, text: &str, offset: usize) {
        let line_index = line_index_for_byte_offset(text, offset);
        for handle_id in conflict_workbench_scroll_handle_ids() {
            self.uniform_scroll_handle(handle_id)
                .scroll_to_item_strict_with_offset(line_index, ScrollStrategy::Top, 4);
        }
    }

    pub(crate) fn sync_conflict_editor_into_state(&mut self) {
        if !conflict_result_pane_uses_editor() {
            return;
        }
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let new_value = self.conflict_editor.value.clone();
        let Some(view) = self.conflict_workbench.files.get_mut(&path) else {
            return;
        };
        if view.kind == ConflictFileKind::Text && view.draft != new_value {
            let max_index = view.blocks.len().saturating_sub(1);
            view.set_draft(new_value);
            self.conflict_workbench.selected_block =
                self.conflict_workbench.selected_block.min(max_index);
        }
    }

    pub(crate) fn select_conflict_file(&mut self, path: String) {
        self.sync_conflict_editor_into_state();
        self.conflict_workbench.selected_path = Some(path.clone());
        self.conflict_workbench.selected_block = 0;
        self.conflict_workbench.show_base = false;
        self.conflict_workbench.clear_pending_resolve();
        // 换文件后三栏 offset 全变，清掉同步滚动的上帧记录避免误判源栏。
        self.conflict_pane_scroll_sync.borrow_mut().take();
        self.ensure_conflict_views_loaded();
        if self.conflict_workbench.files.contains_key(&path) {
            self.sync_conflict_editor_from_state();
            self.maybe_auto_open_external_merge_for_selected_conflict();
            // 切换冲突文件后为新的选中文件补算三栏语法高亮
            let panes = [
                ConflictSyntaxPane::Ours,
                ConflictSyntaxPane::Theirs,
                ConflictSyntaxPane::Draft,
            ];
            self.schedule_conflict_syntax_for_selected(&panes);
        }
    }

    fn select_conflict_block(&mut self, index: usize) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        if view.blocks.is_empty() {
            self.conflict_workbench.selected_block = 0;
        } else {
            self.conflict_workbench.selected_block = index.min(view.blocks.len() - 1);
        }
        self.conflict_workbench.clear_pending_resolve();
        self.sync_conflict_editor_from_state();
    }

    pub(crate) fn step_conflict_block(&mut self, delta: isize) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        if view.blocks.is_empty() {
            return;
        }
        let current = self.conflict_workbench.selected_block as isize;
        let target = (current + delta).clamp(0, view.blocks.len() as isize - 1) as usize;
        self.select_conflict_block(target);
    }

    pub(crate) fn apply_selected_conflict_resolution(
        &mut self,
        resolution: ConflictBlockResolution,
    ) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let selected_block = self.conflict_workbench.selected_block;
        if let Some(view) = self.conflict_workbench.files.get_mut(&path) {
            view.apply_block_resolution(selected_block, resolution);
        }
        self.sync_conflict_editor_from_state();
        // 草稿文本已变，重算结果区语法高亮（seq 守卫丢弃乱序旧结果）
        self.schedule_conflict_syntax_for_selected(&[ConflictSyntaxPane::Draft]);
    }

    pub(crate) fn ignore_selected_conflict_block(&mut self) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let selected_block = self.conflict_workbench.selected_block;
        if let Some(view) = self.conflict_workbench.files.get_mut(&path) {
            view.ignore_block(selected_block);
        }
        self.conflict_workbench.clear_pending_resolve();
        self.sync_conflict_editor_from_state();
    }

    pub(crate) fn apply_selected_conflict_draft(&mut self, resolve: bool) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        let Some(view) = self.conflict_workbench.files.get_mut(&path) else {
            self.last_error = Some("冲突文件详情尚未加载".into());
            return;
        };
        let unresolved_count = view.unresolved_block_count();
        let draft = view.draft.clone();
        if !resolve {
            view.mark_applied();
        } else if self
            .conflict_workbench
            .request_resolve_confirmation(path.clone(), unresolved_count)
        {
            self.active_dialog = Some(DialogState::ConfirmConflictResolve);
            return;
        }
        self.apply_conflict_draft_operation(path, draft, resolve);
    }

    pub(crate) fn confirm_pending_conflict_resolve(&mut self) {
        self.sync_conflict_editor_into_state();
        let Some(pending) = self.conflict_workbench.pending_resolve.clone() else {
            self.active_dialog = None;
            return;
        };
        let Some(draft) = self
            .conflict_workbench
            .files
            .get(&pending.path)
            .map(|view| view.draft.clone())
        else {
            self.conflict_workbench.clear_pending_resolve();
            self.active_dialog = None;
            self.last_error = Some("冲突文件详情尚未加载".into());
            return;
        };
        self.conflict_workbench.clear_pending_resolve();
        self.active_dialog = None;
        self.apply_conflict_draft_operation(pending.path, draft, true);
    }

    pub(crate) fn cancel_pending_conflict_resolve(&mut self) {
        self.conflict_workbench.clear_pending_resolve();
        self.active_dialog = None;
    }

    fn apply_conflict_draft_operation(&mut self, path: String, draft: String, resolve: bool) {
        let path_for_op = path.clone();
        let label = if resolve {
            "冲突结果已应用并标记解决"
        } else {
            "冲突草稿已应用到工作区"
        };
        self.with_repo(label, move |service, repo| {
            if resolve {
                service.apply_conflict_draft_and_resolve(repo, Path::new(&path_for_op), &draft)
            } else {
                service.apply_conflict_draft(repo, Path::new(&path_for_op), &draft)
            }
        });
    }

    pub(crate) fn prune_change_selection(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.change_selection.clear();
            return;
        };
        let staged = snapshot
            .changes
            .iter()
            .filter(|change| change.staged.is_some())
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>();
        let unstaged = snapshot
            .changes
            .iter()
            .filter(|change| change.unstaged.is_some())
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>();
        self.change_selection
            .staged
            .retain(|path| staged.contains(path));
        self.change_selection
            .unstaged
            .retain(|path| unstaged.contains(path));
        if self
            .change_selection
            .staged_anchor
            .as_ref()
            .is_some_and(|path| !staged.contains(path))
        {
            self.change_selection.staged_anchor = None;
        }
        if self
            .change_selection
            .unstaged_anchor
            .as_ref()
            .is_some_and(|path| !unstaged.contains(path))
        {
            self.change_selection.unstaged_anchor = None;
        }
    }

    pub(crate) fn submit_focused_field(&mut self, field: FieldId) {
        if matches!(field, FieldId::CommitMessage) {
            self.commit();
        } else if matches!(field, FieldId::ConflictEditor) {
            self.apply_selected_conflict_draft(false);
        } else if matches!(field, FieldId::CloneUrl | FieldId::ClonePath) {
            if self.active_dialog == Some(DialogState::CloneRepo) {
                self.clone_repo();
            }
        } else if matches!(field, FieldId::BranchName) {
            if self.active_dialog == Some(DialogState::CreateBranch) {
                self.create_branch();
            }
        } else if matches!(field, FieldId::BranchRename) {
            if let Some(DialogState::RenameBranch { branch }) = self.active_dialog.clone() {
                self.rename_branch(branch);
            }
        } else if matches!(field, FieldId::StashMessage) {
            if self.active_dialog == Some(DialogState::StashForm) {
                self.save_stash();
            }
        } else if matches!(field, FieldId::TagName) {
            if matches!(self.active_dialog, Some(DialogState::TagForm { .. })) {
                self.create_tag();
            }
        } else if matches!(field, FieldId::RemoteName | FieldId::RemoteUrl) {
            if let Some(DialogState::RemoteForm { editing }) = self.active_dialog.clone() {
                self.save_remote(editing);
            }
        } else if matches!(field, FieldId::RemoteBranchName) {
            if let Some(DialogState::RemoteBranchOperation { kind }) = self.active_dialog.clone() {
                self.confirm_remote_branch_operation(kind);
            }
        } else if matches!(field, FieldId::RemoteBranchSearch) {
            self.remote_branch_operation.branch_dropdown_open = false;
        } else if matches!(
            field,
            FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
        ) {
            if self.settings_center == Some(SettingsCategory::Proxy) {
                self.save_network_proxy_settings();
            }
        } else if matches!(field, FieldId::ExternalMergeIntellijPath) {
            if self.settings_center == Some(SettingsCategory::ExternalMerge) {
                self.save_external_merge_settings_from_form_and_resume();
            }
        } else if matches!(
            field,
            FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
        ) {
            if self.settings_center == Some(SettingsCategory::Ai) {
                self.save_ai_provider_settings_from_form();
            }
        } else if matches!(field, FieldId::CredentialTestUrl) {
            if matches!(self.active_dialog, Some(DialogState::TestCredential { .. })) {
                self.confirm_test_credential();
            }
        } else if matches!(
            field,
            FieldId::CredentialSecret
                | FieldId::CredentialPassphrase
                | FieldId::CredentialUsername
                | FieldId::CredentialKeyPath
                | FieldId::CredentialRemoteUrl
                | FieldId::CredentialDisplayName
        ) {
            if matches!(self.active_dialog, Some(DialogState::CredentialForm { .. })) {
                self.save_credential_form();
            } else {
                self.use_credentials();
            }
        }
    }

    pub(crate) fn notify_text_field_changed(&mut self, field: FieldId) {
        // 全局符号搜索面板：输入即查（seq 守卫防乱序）。
        if field == FieldId::CodePaletteSearch {
            self.on_code_palette_input_changed();
        }
        if matches!(field, FieldId::WorkflowInput(_)) {
            self.workflow_input_changed();
        }
        // 编辑器文本框值变化即同步回纯数据层（预览/保存校验都读数据层）
        if let FieldId::WorkflowEditor(editor_id) = field {
            self.workflow_editor_field_changed(editor_id);
        }
    }

    pub(crate) fn focused_text_field(&self, window: &Window, cx: &App) -> Option<FieldId> {
        let field = self.focused_field(window, cx)?;
        if self.active_operation_blocker_message().is_some()
            && !self.operation_blocker_allows_text_field(field)
        {
            return None;
        }
        Some(field)
    }

    pub(crate) fn operation_blocker_allows_text_field(&self, field: FieldId) -> bool {
        self.pending_credential.is_some()
            && matches!(
                field,
                FieldId::CredentialUsername
                    | FieldId::CredentialSecret
                    | FieldId::CredentialKeyPath
                    | FieldId::CredentialPassphrase
            )
    }

    pub(crate) fn text_backspace(
        &mut self,
        _: &TextBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_backward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_delete(
        &mut self,
        _: &TextDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_forward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_left(&mut self, _: &TextLeft, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(false);
            cx.notify();
        }
    }

    pub(crate) fn text_right(
        &mut self,
        _: &TextRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(false);
            cx.notify();
        }
    }

    pub(crate) fn text_up(&mut self, _: &TextUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_vertical(-1, false);
                cx.notify();
            }
        }
    }

    pub(crate) fn text_down(&mut self, _: &TextDown, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_vertical(1, false);
                cx.notify();
            }
        }
    }

    pub(crate) fn text_select_left(
        &mut self,
        _: &TextSelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_right(
        &mut self,
        _: &TextSelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_up(
        &mut self,
        _: &TextSelectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(-1, true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_down(
        &mut self,
        _: &TextSelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(1, true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_all(
        &mut self,
        _: &TextSelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).select_all();
            cx.notify();
        }
    }

    pub(crate) fn text_home(&mut self, _: &TextHome, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_start(false);
            } else {
                self.field_mut(field).move_caret_to(0, false);
            }
            cx.notify();
        }
    }

    pub(crate) fn text_end(&mut self, _: &TextEnd, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_end(false);
            } else {
                let end = self.field(field).value.len();
                self.field_mut(field).move_caret_to(end, false);
            }
            cx.notify();
        }
    }

    pub(crate) fn text_paste(
        &mut self,
        _: &TextPaste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.field_mut(field).replace_text_in_utf16_range_with_mode(
                None,
                &text,
                Self::is_multiline_field(field),
            );
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_copy(&mut self, _: &TextCopy, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn text_cut(&mut self, _: &TextCut, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.field_mut(field).delete_selection();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_submit(
        &mut self,
        _: &TextSubmit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if field == FieldId::CommitMessage {
                self.commit();
                cx.notify();
            } else if field == FieldId::ConflictEditor {
                self.apply_selected_conflict_draft(false);
                cx.notify();
            } else if matches!(
                field,
                FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
            ) && self.settings_center == Some(SettingsCategory::Proxy)
            {
                self.save_network_proxy_settings();
                cx.notify();
            } else if matches!(
                field,
                FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
            ) && self.settings_center == Some(SettingsCategory::Ai)
            {
                self.save_ai_provider_settings_from_form();
                cx.notify();
            } else if field == FieldId::ExternalMergeIntellijPath
                && self.settings_center == Some(SettingsCategory::ExternalMerge)
            {
                self.save_external_merge_settings_from_form();
                cx.notify();
            }
        }
    }

    fn focused_field(&self, window: &Window, _cx: &App) -> Option<FieldId> {
        DEDICATED_FIELDS
            .iter()
            .find_map(|(id, access)| access(self).focus.is_focused(window).then_some(*id))
            .or_else(|| self.focused_workflow_input(window))
            .or_else(|| self.workflow_editor_focused_field(window))
    }

    pub(crate) fn field(&self, id: FieldId) -> &TextFieldState {
        match id {
            FieldId::WorkflowInput(index) => self.workflow_input_field(index),
            // 编辑器字段经独立寻址（渲染前 ensure 已初始化；弹窗关闭瞬间
            // 的在途渲染回落到静态字段兜底）。
            FieldId::WorkflowEditor(editor_id) => self
                .workflow_editor_field_ref(editor_id)
                .unwrap_or(&self.branch_name),
            // 经 DEDICATED_FIELDS 单一注册表查找：与 focused_field 共用一份
            // 清单，漏注册会在此处 panic（首次渲染即暴露）而非静默丢输入。
            _ => DEDICATED_FIELDS
                .iter()
                .find_map(|(field_id, access)| (*field_id == id).then(|| access(self)))
                .expect("FieldId 未注册到 DEDICATED_FIELDS"),
        }
    }

    pub(crate) fn field_mut(&mut self, id: FieldId) -> &mut TextFieldState {
        match id {
            FieldId::CloneUrl => &mut self.clone_url,
            FieldId::ClonePath => &mut self.clone_path,
            FieldId::BranchName => &mut self.branch_name,
            FieldId::BranchRename => &mut self.branch_rename,
            FieldId::RemoteName => &mut self.remote_name,
            FieldId::RemoteUrl => &mut self.remote_url,
            FieldId::CommitMessage => &mut self.commit_message,
            FieldId::StashMessage => &mut self.stash_message,
            FieldId::TagName => &mut self.tag_name,
            FieldId::TagMessage => &mut self.tag_message,
            FieldId::CredentialUsername => &mut self.credential_username,
            FieldId::CredentialSecret => &mut self.credential_secret,
            FieldId::CredentialKeyPath => &mut self.credential_key_path,
            FieldId::CredentialPassphrase => &mut self.credential_passphrase,
            FieldId::CredentialRemoteUrl => &mut self.credential_remote_url,
            FieldId::CredentialTestUrl => &mut self.credential_test_url,
            FieldId::CredentialDisplayName => &mut self.credential_display_name,
            FieldId::ConflictEditor => &mut self.conflict_editor,
            FieldId::RemoteBranchName => &mut self.remote_branch_name,
            FieldId::RemoteBranchSearch => &mut self.remote_branch_search,
            FieldId::RepoSwitcherSearch => &mut self.repo_switcher_search,
            FieldId::CodeIndexFilter => &mut self.code_index_filter,
            FieldId::CodePaletteSearch => &mut self.code_palette_search,
            FieldId::CodeUnderstandingQuestion => &mut self.understanding_question,
            FieldId::CommitGraphSearch => &mut self.commit_graph_search,
            FieldId::CommitGraphBranchSearch => &mut self.commit_graph_branch_search,
            FieldId::SidebarLocalBranchSearch => &mut self.sidebar_local_branch_search,
            FieldId::SidebarRemoteBranchSearch => &mut self.sidebar_remote_branch_search,
            FieldId::ProxyHttpUrl => &mut self.proxy_http_url,
            FieldId::ProxyHttpsUrl => &mut self.proxy_https_url,
            FieldId::ProxySocks5Url => &mut self.proxy_socks5_url,
            FieldId::AiBaseUrl => &mut self.ai_base_url,
            FieldId::AiApiKey => &mut self.ai_api_key,
            FieldId::AiModel => &mut self.ai_model,
            FieldId::ExternalMergeIntellijPath => &mut self.external_merge_intellij_path,
            FieldId::WorkflowInput(index) => self.workflow_input_field_mut(index),
            // 编辑器字段惰性初始化需要 Context（focus_handle）；
            // 编辑器未打开而字段仍被寻址（弹窗关闭瞬间的在途事件）时
            // 退回一个无关紧要的静态字段，保证返回值满足借用契约。
            FieldId::WorkflowEditor(editor_id) => {
                return workflow_editor_field_or_fallback(self, editor_id);
            }
        }
    }

    pub(crate) fn browse_open(&mut self) {
        self.status = "正在选择仓库文件夹".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new().pick_folder();
            send_ui_event(&tx, UiEvent::OpenRepositoryFolderSelected { path });
        });
    }

    pub(crate) fn browse_clone_target(&mut self) {
        self.status = "正在选择克隆父文件夹".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new().pick_folder();
            send_ui_event(&tx, UiEvent::CloneTargetFolderSelected { path });
        });
    }

    pub(crate) fn open_clone_dialog(&mut self, window: &mut Window) {
        self.close_popups();
        self.clone_url.clear();
        self.clone_path.clear();
        self.clone_recursive_submodules = default_clone_recursive_submodules();
        self.active_dialog = Some(DialogState::CloneRepo);
        self.last_error = None;
        window.focus(&self.clone_url.focus);
    }

    pub(crate) fn open_create_branch_dialog(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.branch_name.clear();
        self.create_branch_checkout = true;
        self.active_dialog = Some(DialogState::CreateBranch);
        self.last_error = None;
    }

    pub(crate) fn open_rename_branch_dialog(&mut self, branch: String) {
        self.close_popups();
        self.branch_rename.set_value(branch.clone());
        self.active_dialog = Some(DialogState::RenameBranch { branch });
        self.last_error = None;
    }

    pub(crate) fn close_popups(&mut self) {
        // 全局符号搜索面板与其他弹层互斥：任何新弹层打开前都要关掉它。
        self.code_search_palette = None;
        self.active_dialog = None;
        self.ai_review_history = None;
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.credential_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.workflow_template_context_menu = None;
        self.encoding_menu_target = None;
        self.encoding_menu_closed_by_capture = None;
        self.commit_graph_branch_menu_closed_by_capture = false;
        self.active_tab_state_mut().commit_graph.branch_menu_open = false;
        self.commit_graph_branch_search.clear();
        self.context_navigator_overlay_open = false;
        self.close_repo_switcher();
        // 工作流编辑器的下拉（类型/守卫）在弹窗关闭时一并收起，防陈旧态。
        self.workflow_editor_close_menus();
    }

    /// 切换仓库切换下拉的展开/收起；展开时菜单固定在触发器按钮正下方（按记录的锚点定位）。
    pub(crate) fn toggle_repo_switcher(&mut self, window: &Window) {
        if self.repo_switcher_menu.is_some() {
            self.close_repo_switcher();
            return;
        }
        // 仓库切换与窄窗导航覆盖层互斥。
        self.context_navigator_overlay_open = false;
        // 展开时同步加载最近仓库列表（SQLite 本地查询 < 1ms），渲染时纯读缓存。
        self.repo_switcher_recent = self.storage.load_recent_repos().unwrap_or_default();
        let viewport_size = window.viewport_size();
        // 锚点由触发器 paint 时记录；首帧尚未记录时回退到视口左上角。
        let (x, y) = self
            .repo_switcher_anchor
            .map(|anchor| {
                repo_switcher_menu_origin(
                    &anchor,
                    f32::from(viewport_size.width),
                    f32::from(viewport_size.height),
                )
            })
            .unwrap_or((MENU_VIEWPORT_MARGIN, MENU_VIEWPORT_MARGIN));
        self.repo_switcher_menu = Some(RepoSwitcherMenu { x, y });
        // 搜索默认收起为「搜索仓库」按钮；清掉上一次的搜索词。
        self.repo_switcher_search_open = false;
        self.repo_switcher_search.clear();
    }

    pub(crate) fn close_repo_switcher(&mut self) {
        self.repo_switcher_menu = None;
        self.repo_switcher_search_open = false;
        self.repo_switcher_search.clear();
    }

    /// 是否有任一弹出菜单（仓库切换下拉、各类右键菜单、编码菜单）或窄窗导航覆盖层打开。
    /// 弹层没有全屏遮罩，期间分栏分割线等底层交互应暂停，避免抢走弹层边缘的点击。
    pub(crate) fn any_popup_menu_open(&self) -> bool {
        self.context_navigator_overlay_open
            || self.repo_switcher_menu.is_some()
            || self.branch_context_menu.is_some()
            || self.remote_context_menu.is_some()
            || self.change_context_menu.is_some()
            || self.file_path_context_menu.is_some()
            || self.credential_context_menu.is_some()
            || self.tag_context_menu.is_some()
            || self.stash_context_menu.is_some()
            || self.commit_graph.branch_menu_open
            || self.commit_context_menu.is_some()
            || self.workflow_template_context_menu.is_some()
            || self.encoding_menu_target.is_some()
            || self.workflow_editor_menu_open()
    }

    pub(crate) fn toggle_sidebar_section(&mut self, section: SidebarSection) {
        self.close_popups();
        self.sidebar_sections.toggle(section);
    }

    pub(crate) fn close_dialog(&mut self) {
        if self.active_dialog == Some(DialogState::ConfirmWindowClose) {
            self.cancel_window_close();
            return;
        }
        // 如果没有 active_dialog 但设置中心打开，关闭设置中心。
        if self.active_dialog.is_none() && self.settings_center.is_some() {
            self.close_settings_center();
            return;
        }
        let closing_submodule_manager = self.active_dialog == Some(DialogState::SubmoduleManager);
        self.active_dialog = None;
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        self.credential_context_menu = None;
        if closing_submodule_manager {
            self.submodule_dialog.invalidate();
        }
        self.last_error = None;
    }

    fn request_window_close(&mut self) {
        if self.active_dialog == Some(DialogState::ConfirmWindowClose) {
            return;
        }
        // 关闭确认临时覆盖已有弹窗；用户取消时恢复，避免丢失尚未保存的表单。
        let previous_dialog = self.active_dialog.take();
        self.close_popups();
        self.dialog_before_window_close = previous_dialog;
        self.active_dialog = Some(DialogState::ConfirmWindowClose);
        self.last_error = None;
    }

    pub(crate) fn cancel_window_close(&mut self) {
        self.active_dialog = self.dialog_before_window_close.take();
        self.last_error = None;
    }

    pub(crate) fn should_close_window(&mut self) -> bool {
        if self.exit_requested {
            return true;
        }
        self.request_window_close();
        false
    }

    pub(crate) fn exit_application(&mut self, cx: &mut Context<Self>) {
        self.exit_requested = true;
        self.dialog_before_window_close = None;
        cx.quit();
    }

    pub(crate) fn minimize_to_tray(&mut self, window: &Window, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let result = self
                .tray
                .as_mut()
                .ok_or_else(|| {
                    self.tray_error
                        .clone()
                        .unwrap_or_else(|| "系统托盘不可用".to_string())
                })
                .and_then(|tray| tray.hide_window(window));
            match result {
                Ok(()) => {
                    self.active_dialog = None;
                    self.dialog_before_window_close = None;
                    self.last_error = None;
                }
                Err(error) => {
                    self.last_error = Some(error);
                    self.notify_error("无法缩小到系统托盘", cx);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            self.last_error = Some("当前平台暂不支持缩小到系统托盘".to_string());
            self.notify_error("无法缩小到系统托盘", cx);
        }
    }

    #[cfg(windows)]
    pub(crate) fn attach_window_to_tray(&mut self, window: &Window) {
        let result = self.tray.as_mut().map(|tray| tray.attach_window(window));
        if let Some(Err(error)) = result {
            self.tray = None;
            self.tray_error = Some(error);
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn attach_window_to_tray(&mut self, _window: &Window) {}

    pub(crate) fn handle_tray_action(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let action = self.tray.as_ref().and_then(|tray| tray.next_action());
            match action {
                Some(tray::TrayAction::Show) => {
                    if let Some(tray) = self.tray.as_ref() {
                        tray.show_window();
                    }
                }
                Some(tray::TrayAction::Exit) => self.exit_application(cx),
                None => {}
            }
        }
        #[cfg(not(windows))]
        let _ = cx;
    }

    pub(crate) fn close_credential_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.credential_context_menu.is_some() {
            self.credential_context_menu = None;
            cx.notify();
        }
    }

    /// 打开设置中心，默认显示凭据管理分类。
    pub(crate) fn open_settings_center(&mut self) {
        self.close_popups();
        self.settings_center = Some(SettingsCategory::Credentials);
        self.reload_credential_records("凭据列表已加载");
    }

    /// 通知气泡「点击查看更新」的直达入口：打开设置中心并定位到更新设置页。
    pub(crate) fn open_update_settings_center(&mut self) {
        self.close_popups();
        self.settings_center = Some(SettingsCategory::Update);
    }

    /// 切换设置中心的分类。
    pub(crate) fn select_settings_category(&mut self, category: SettingsCategory) {
        self.settings_center = Some(category);
        match category {
            SettingsCategory::Credentials => {
                self.reload_credential_records("凭据列表已加载");
            }
            SettingsCategory::Proxy => {
                self.reset_proxy_form_from_settings();
            }
            SettingsCategory::Ai => {
                self.reset_ai_form_from_settings();
            }
            SettingsCategory::ExternalMerge => {
                self.reset_external_merge_form_from_settings();
            }
            SettingsCategory::CodeIndex => {
                self.refresh_code_index_stats();
            }
            SettingsCategory::Theme
            | SettingsCategory::Update
            | SettingsCategory::Shortcuts
            | SettingsCategory::About => {}
        }
    }

    /// 关闭设置中心。
    pub(crate) fn close_settings_center(&mut self) {
        self.settings_center = None;
        // 关闭设置中心时清掉可能残留的「保存并继续」待处理冲突路径，
        // 等价于原外部合并「取消」按钮的清理职责。
        external_merge_view::clear_pending_external_merge_path();
    }

    /// 设置页保存后按 `last_error` 提示成功/失败（各 save 失败置 Some、成功置 None）。
    /// 成功不关闭页面，失败把具体错误带进 toast。
    pub(crate) fn notify_settings_save(
        &mut self,
        success_msg: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        if let Some(err) = self.last_error.clone() {
            self.notify_error(format!("保存失败：{err}"), cx);
        } else {
            self.notify_success(success_msg, cx);
        }
    }

    pub(crate) fn open_credential_manager(&mut self) {
        self.open_settings_center();
        self.select_settings_category(SettingsCategory::Credentials);
    }

    pub(crate) fn reset_proxy_form_from_settings(&mut self) {
        let custom = self.proxy_settings.custom.normalized();
        self.proxy_mode = self.proxy_settings.mode;
        self.proxy_http_url.set_value(custom.http_proxy);
        self.proxy_https_url.set_value(custom.https_proxy);
        self.proxy_socks5_url.set_value(custom.socks5_proxy);
    }

    pub(crate) fn proxy_form_settings(&self) -> NetworkProxySettings {
        NetworkProxySettings {
            mode: self.proxy_mode,
            custom: CustomProxySettings {
                http_proxy: self.proxy_http_url.value.trim().to_string(),
                https_proxy: self.proxy_https_url.value.trim().to_string(),
                socks5_proxy: self.proxy_socks5_url.value.trim().to_string(),
            },
        }
    }

    pub(crate) fn set_proxy_mode(&mut self, mode: NetworkProxyMode) {
        self.proxy_mode = mode;
        self.last_error = None;
    }

    pub(crate) fn save_network_proxy_settings(&mut self) {
        let settings = self.proxy_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.proxy_settings = settings;
        self.save_proxy_settings();
        self.status = "代理设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn open_external_merge_settings(&mut self) {
        self.open_settings_center();
        self.select_settings_category(SettingsCategory::ExternalMerge);
        self.status = "合并工具设置已打开".into();
        self.last_error = None;
    }

    pub(crate) fn reset_external_merge_form_from_settings(&mut self) {
        self.external_merge_enabled_form = self.external_merge_settings.enabled;
        self.external_merge_auto_open_form = self.external_merge_settings.auto_open_intellij;
        self.external_merge_intellij_path
            .set_value(self.external_merge_settings.normalized_intellij_path());
        self.external_merge_detection = None;
    }

    pub(crate) fn external_merge_form_settings(&self) -> ExternalMergeSettings {
        ExternalMergeSettings {
            enabled: self.external_merge_enabled_form,
            auto_open_intellij: self.external_merge_auto_open_form,
            intellij_path: self.external_merge_intellij_path.value.trim().to_string(),
        }
    }

    pub(crate) fn set_external_merge_enabled_form(&mut self, enabled: bool) {
        self.external_merge_enabled_form = enabled;
        if !enabled {
            self.external_merge_auto_open_form = false;
        }
        self.last_error = None;
    }

    pub(crate) fn set_external_merge_auto_open_form(&mut self, enabled: bool) {
        self.external_merge_auto_open_form = enabled;
        if enabled {
            self.external_merge_enabled_form = true;
        }
        self.last_error = None;
    }

    pub(crate) fn save_external_merge_settings_from_form(&mut self) {
        self.external_merge_settings = self.external_merge_form_settings();
        self.save_external_merge_settings();
        self.status = "合并工具设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn test_external_merge_settings_from_form(&mut self) {
        let settings = self.external_merge_form_settings();
        match khaslana::external_merge::resolve_intellij_idea_command_with_settings(&settings) {
            Ok(path) => {
                self.external_merge_detection = Some((settings.clone(), true));
                self.external_merge_settings = settings;
                self.save_external_merge_settings();
                self.status = format!("已找到 IntelliJ IDEA：{}", path.display());
                self.last_error = None;
            }
            Err(err) => {
                self.external_merge_detection = Some((settings, false));
                self.status = "IntelliJ IDEA 检测失败".into();
                self.last_error = Some(err.to_string());
            }
        }
    }

    // ── 更新方法 ──────────────────────────────────────────────────────────

    pub(crate) fn start_update_check(&mut self, trigger: UpdateCheckTrigger) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        // 周期静默检查不写任何可见状态（无新版本/失败就静默）；
        // 启动/手动检查维持状态栏反馈与错误复位。
        if trigger != UpdateCheckTrigger::Periodic {
            self.update_error = None;
            self.status = "检查更新中".into();
        }

        let tx = self.tx.clone();
        let preferences = self.update_preferences.clone();
        let proxy_settings = self.proxy_settings.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let sources = update::manifest_sources_for(&preferences);
            match update::check_for_update(&sources, &preferences, &proxy_settings) {
                Ok(UpdateCheckResult::UpdateAvailable { manifest, asset }) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFinished {
                            manifest: Arc::new(manifest),
                            asset,
                            trigger,
                        },
                    );
                }
                Ok(UpdateCheckResult::UpToDate) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: "当前已是最新版本".into(),
                            trigger,
                        },
                    );
                }
                Ok(UpdateCheckResult::SkippedVersion) => {
                    // 用户跳过了此版本，静默忽略
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: String::new(),
                            trigger,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: err.to_string(),
                            trigger,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn start_update_download(&mut self) {
        let Some(manifest) = self.available_update.clone() else {
            return;
        };
        let asset = manifest.platforms.get("windows-x86_64").cloned();
        let Some(asset) = asset else {
            self.update_error = Some("缺少下载信息".into());
            return;
        };
        self.update_downloading = true;
        self.update_download_progress = None;
        self.update_error = None;
        self.status = "下载更新中".into();

        let tx = self.tx.clone();
        let config_dir = khaslana::default_database_path()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let proxy_settings = self.proxy_settings.clone();

        self.tasks.spawn(TaskKind::Long, move || {
            // 进度回调：发送 UpdateDownloadProgress 事件
            let on_progress = |downloaded: u64, total: u64| {
                let _ = tx.try_send(UiEvent::UpdateDownloadProgress { downloaded, total });
            };

            // 下载
            match update::download_update(&asset, &config_dir, &proxy_settings, Some(&on_progress))
            {
                Ok((zip_path, computed_sha256)) => {
                    // SHA-256 校验
                    if computed_sha256 != asset.sha256 {
                        send_ui_event(
                            &tx,
                            UiEvent::UpdateInstallFailed {
                                error: "更新包 SHA-256 校验失败，文件可能被篡改".into(),
                            },
                        );
                        return;
                    }
                    // 解压 staging
                    let version = manifest.version.clone();
                    match update::prepare_staging(&zip_path, &version, &config_dir) {
                        Ok(staging_dir) => {
                            send_ui_event(
                                &tx,
                                UiEvent::UpdateReadyToInstall {
                                    staging_dir,
                                    manifest,
                                },
                            );
                        }
                        Err(err) => {
                            send_ui_event(
                                &tx,
                                UiEvent::UpdateInstallFailed {
                                    error: format!("更新包解压失败：{err}"),
                                },
                            );
                        }
                    }
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateInstallFailed {
                            error: format!("更新包下载失败：{err}"),
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn install_update(
        &mut self,
        staging_dir: &Path,
        _version: &str,
        cx: &mut Context<Self>,
    ) {
        // 检查写入权限
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("khaslana.exe"));
        let exe_dir = current_exe
            .parent()
            .unwrap_or_else(|| Path::as_ref(Path::new(".")));

        // 尝试在 exe 目录创建临时文件来验证写入权限
        let test_file = exe_dir.join(".khaslana_update_test");
        let writable = fs::File::create(&test_file).is_ok();
        let _ = fs::remove_file(&test_file);

        if !writable {
            let version = self
                .available_update
                .as_ref()
                .map(|m| m.version.clone())
                .unwrap_or_default();
            self.active_dialog = Some(DialogState::UpdateNoWritePermission { version });
            return;
        }

        // 构造 updater 命令
        let new_exe = staging_dir.join("khaslana.exe");
        let new_updater = staging_dir.join("khaslana_updater.exe");
        let config_dir = khaslana::default_database_path()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let backup_dir = config_dir.join("updates").join("backup");
        let pid = std::process::id();

        // 先转为字符串，避免 Command::new move PathBuf 后无法引用
        let new_exe_str = new_exe.to_string_lossy().to_string();
        let new_updater_str = new_updater.to_string_lossy().to_string();
        let current_exe_str = current_exe.to_string_lossy().to_string();
        let backup_dir_str = backup_dir.to_string_lossy().to_string();
        let pid_str = pid.to_string();

        if let Err(err) = Command::new(&new_updater_str)
            .args([
                "--pid",
                &pid_str,
                "--target-exe",
                &current_exe_str,
                "--new-exe",
                &new_exe_str,
                "--new-updater",
                &new_updater_str,
                "--backup-dir",
                &backup_dir_str,
                "--restart",
            ])
            .spawn()
        {
            // updater 启动失败（杀软拦截、staging 目录被清理等）：留在当前
            // 版本并提示，不退出应用——静默退出会让用户误以为更新已安装。
            self.active_dialog = None;
            self.update_error = Some(format!("更新器启动失败：{err}"));
            self.status = "更新失败".into();
            self.notify_toast(
                AppToastKind::Error,
                format!("更新器启动失败：{err}，已保留当前版本"),
                cx,
            );
            return;
        }

        std::process::exit(0);
    }

    pub(crate) fn skip_version(&mut self, version: &str) {
        self.update_preferences.skipped_version = Some(version.to_string());
        self.save_update_preferences();
        self.active_dialog = None;
    }

    pub(crate) fn clear_skipped_version(&mut self) {
        self.update_preferences.skipped_version = None;
        self.save_update_preferences();
        self.status = "已清除跳过版本".into();
    }

    pub(crate) fn save_update_preferences(&self) {
        if let Err(err) = self
            .storage
            .save_update_preferences(&self.update_preferences)
        {
            tracing::warn!("update preferences write skipped: {err}");
        }
    }

    pub(crate) fn reset_ai_form_from_settings(&mut self) {
        self.ai_enabled_form = self.ai_settings.enabled;
        self.ai_base_url
            .set_value(self.ai_settings.base_url.clone());
        self.ai_api_key.set_value(self.ai_settings.api_key.clone());
        self.ai_model.set_value(self.ai_settings.model.clone());
    }

    pub(crate) fn ai_form_settings(&self) -> AiProviderSettings {
        let mut settings = self.ai_settings.clone();
        settings.enabled = self.ai_enabled_form;
        settings.base_url = self.ai_base_url.value.trim().to_string();
        settings.api_key = self.ai_api_key.value.trim().to_string();
        settings.model = self.ai_model.value.trim().to_string();
        settings
    }

    pub(crate) fn set_ai_enabled_form(&mut self, enabled: bool) {
        self.ai_enabled_form = enabled;
        self.last_error = None;
    }

    pub(crate) fn save_ai_provider_settings_from_form(&mut self) {
        let settings = self.ai_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.ai_settings = settings;
        self.save_ai_provider_settings();
        self.status = "AI 设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn test_network_proxy_settings(&mut self) {
        if self.busy || self.global_busy_tab.is_some() {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端，无法测试代理".into());
            return;
        };
        let settings = self.proxy_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }

        self.proxy_settings = settings;
        self.save_proxy_settings();
        self.begin_global_test_busy("正在测试代理连接");
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let result = (|| -> khaslana::Result<()> {
                let repo = Repository::open(repo_path)?;
                service.test_proxy(&repo, &RemoteName::new(remote))?;
                Ok(())
            })();
            match result {
                Ok(()) => send_ui_event(
                    &tx,
                    UiEvent::ProxyTestFinished {
                        message: "代理测试通过".into(),
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: None,
                        error: err.to_string(),
                    },
                ),
            }
        });
    }
}
