//! RepositoryView 的凭据、仓库、分支、历史与工作区动作。

use crate::*;

impl RepositoryView {
    pub(crate) fn toggle_encoding_menu(&mut self, target: EncodingMenuTarget) {
        if self.encoding_menu_closed_by_capture == Some(target) {
            self.encoding_menu_closed_by_capture = None;
            self.encoding_menu_target = None;
            return;
        }
        self.encoding_menu_closed_by_capture = None;
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.active_dialog = None;
        self.commit_graph.branch_menu_open = false;
        self.commit_graph_branch_search.clear();
        self.encoding_menu_target = if self.encoding_menu_target == Some(target) {
            None
        } else {
            Some(target)
        };
    }

    pub(crate) fn choose_diff_encoding(&mut self, encoding: DiffEncodingChoice) {
        self.encoding_menu_target = None;
        self.encoding_menu_closed_by_capture = None;
        self.set_current_diff_encoding(encoding);
    }

    pub(crate) fn change_paths(&self, scope: DiffScope) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .changes
                    .iter()
                    .filter(|change| match scope {
                        DiffScope::Staged => change.staged.is_some(),
                        DiffScope::Unstaged => change.unstaged.is_some(),
                    })
                    .map(|change| change.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn selected_change_paths(&self, scope: DiffScope) -> Vec<String> {
        self.change_selection
            .selected(&scope)
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) fn is_change_selected(&self, scope: &DiffScope, path: &str) -> bool {
        self.change_selection.selected(scope).contains(path)
    }

    pub(crate) fn has_local_branch_for_remote(&self, remote_branch: &str) -> bool {
        let Some((_, local_name)) = remote_branch.split_once('/') else {
            return false;
        };
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.branches.iter().any(|branch| {
                branch.kind == BranchKind::Local && branch.name.as_str() == local_name
            })
        })
    }

    pub(crate) fn clear_opposite_change_selection(&mut self, scope: &DiffScope) {
        match scope {
            DiffScope::Staged => {
                self.change_selection.unstaged.clear();
                self.change_selection.unstaged_anchor = None;
            }
            DiffScope::Unstaged => {
                self.change_selection.staged.clear();
                self.change_selection.staged_anchor = None;
            }
        }
    }

    pub(crate) fn clear_change_anchor_if_empty(&mut self, scope: &DiffScope) {
        if !self.change_selection.selected(scope).is_empty() {
            return;
        }
        match scope {
            DiffScope::Staged => self.change_selection.staged_anchor = None,
            DiffScope::Unstaged => self.change_selection.unstaged_anchor = None,
        }
    }

    pub(crate) fn select_only_change(&mut self, path: String, scope: DiffScope, load_diff: bool) {
        self.change_selection.clear();
        self.change_selection
            .selected_mut(&scope)
            .insert(path.clone());
        self.change_selection.set_anchor(&scope, path.clone());
        if load_diff {
            self.load_diff(path, scope);
        }
    }

    pub(crate) fn select_change_from_mouse(
        &mut self,
        path: String,
        scope: DiffScope,
        event: &MouseDownEvent,
    ) {
        self.clear_opposite_change_selection(&scope);
        let multi = event.modifiers.control || event.modifiers.platform;
        if event.modifiers.shift {
            self.select_change_range(path.clone(), scope.clone());
        } else if multi {
            let selected = self.change_selection.selected_mut(&scope);
            if selected.contains(&path) {
                selected.remove(&path);
                self.clear_change_anchor_if_empty(&scope);
            } else {
                selected.insert(path.clone());
                self.change_selection.set_anchor(&scope, path.clone());
                self.load_diff(path.clone(), scope.clone());
            }
        } else if self.is_change_selected(&scope, &path) {
            self.change_selection.selected_mut(&scope).remove(&path);
            self.clear_change_anchor_if_empty(&scope);
        } else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
        }
    }

    fn select_change_range(&mut self, path: String, scope: DiffScope) {
        self.clear_opposite_change_selection(&scope);
        let paths = self.change_paths(scope.clone());
        let Some(current_index) = paths.iter().position(|candidate| candidate == &path) else {
            return;
        };
        let Some(anchor) = self.change_selection.anchor(&scope).cloned() else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
            return;
        };
        let Some(anchor_index) = paths.iter().position(|candidate| candidate == &anchor) else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
            return;
        };
        let (start, end) = if anchor_index <= current_index {
            (anchor_index, current_index)
        } else {
            (current_index, anchor_index)
        };
        let selected = self.change_selection.selected_mut(&scope);
        selected.clear();
        selected.extend(paths[start..=end].iter().cloned());
        self.load_diff(path, scope);
    }

    fn ensure_change_context_selection(&mut self, path: String, scope: DiffScope) {
        self.clear_opposite_change_selection(&scope);
        if !self.is_change_selected(&scope, &path) {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
        }
    }

    pub(crate) fn open_change_context_menu(
        &mut self,
        path: String,
        scope: DiffScope,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.ensure_change_context_selection(path.clone(), scope.clone());
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.file_path_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let menu_height = if scope == DiffScope::Staged {
            STAGED_CHANGE_MENU_HEIGHT
        } else {
            CHANGE_MENU_HEIGHT
        };
        let (x, y) = clamped_menu_position(event, window, CHANGE_MENU_WIDTH, menu_height);
        self.change_context_menu = Some(ChangeContextMenu { path, scope, x, y });
    }

    pub(crate) fn open_file_path_context_menu(
        &mut self,
        path: String,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.credential_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let (x, y) =
            clamped_menu_position(event, window, FILE_PATH_MENU_WIDTH, FILE_PATH_MENU_HEIGHT);
        self.file_path_context_menu = Some(FilePathContextMenu { path, x, y });
    }

    pub(crate) fn mouse_down_inside_context_menu(&self, event: &MouseDownEvent) -> bool {
        let x: f32 = event.position.x.into();
        let y: f32 = event.position.y.into();
        self.branch_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, BRANCH_MENU_WIDTH, BRANCH_MENU_HEIGHT)
        }) || self.remote_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, REMOTE_MENU_WIDTH, REMOTE_MENU_HEIGHT)
        }) || self.change_context_menu.as_ref().is_some_and(|menu| {
            let height = if menu.scope == DiffScope::Staged {
                STAGED_CHANGE_MENU_HEIGHT
            } else {
                CHANGE_MENU_HEIGHT
            };
            point_in_menu(x, y, menu.x, menu.y, CHANGE_MENU_WIDTH, height)
        }) || self.file_path_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(
                x,
                y,
                menu.x,
                menu.y,
                FILE_PATH_MENU_WIDTH,
                FILE_PATH_MENU_HEIGHT,
            )
        }) || self.credential_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(
                x,
                y,
                menu.x,
                menu.y,
                CREDENTIAL_MENU_WIDTH,
                CREDENTIAL_MENU_HEIGHT,
            )
        }) || self.tag_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, TAG_MENU_WIDTH, TAG_MENU_HEIGHT)
        }) || self.stash_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, STASH_MENU_WIDTH, STASH_MENU_HEIGHT)
        }) || self
            .workflow_template_context_menu
            .as_ref()
            .is_some_and(|menu| {
                point_in_menu(
                    x,
                    y,
                    menu.x,
                    menu.y,
                    WORKFLOW_TEMPLATE_MENU_WIDTH,
                    WORKFLOW_TEMPLATE_MENU_HEIGHT,
                )
            })
            || self.commit_context_menu.as_ref().is_some_and(|menu| {
                point_in_menu(x, y, menu.x, menu.y, COMMIT_MENU_WIDTH, menu.height)
            })
            || self.repo_switcher_menu.as_ref().is_some_and(|menu| {
                point_in_repo_switcher(x, y, menu, self.repo_switcher_anchor.as_ref())
            })
    }

    pub(crate) fn open_commit_context_menu(
        &mut self,
        oid: String,
        short_oid: String,
        summary: String,
        parent_count: usize,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.select_history_commit(oid.clone());
        self.branch_context_menu = None;
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let is_unpushed = self
            .branch_sync_status
            .as_ref()
            .is_some_and(|status| status.unpushed_oids.iter().any(|id| id == &oid));
        let height = if is_unpushed {
            COMMIT_UNPUSHED_MENU_HEIGHT
        } else {
            COMMIT_MENU_HEIGHT
        };
        let (x, y) = clamped_menu_position(event, window, COMMIT_MENU_WIDTH, height);
        let is_head = self
            .history_commits
            .iter()
            .find(|commit| commit.oid == oid)
            .is_some_and(|commit| {
                commit
                    .refs
                    .iter()
                    .any(|reference| reference.kind == khaslana::CommitRefKind::Head)
            });
        self.commit_context_menu = Some(CommitContextMenu {
            oid,
            short_oid,
            summary,
            parent_count,
            is_unpushed,
            is_head,
            height,
            x,
            y,
        });
    }

    pub(crate) fn start_resize_column(&mut self, target: ResizeTarget, event: &MouseDownEvent) {
        // 注意：不要在这里 close_popups——根层 capture_any_mouse_down 已对真正的
        // 菜单外部点击统一关闭弹层；菜单边缘容差区内的点击命中判定视为菜单内部
        //（菜单保持打开），此处若再关会把容差区内落在分割线上的点击重新误杀。
        // 详情区对半分模式（height = None）：点击处即分割条顶部，用左列顶部
        // 坐标推导当前实际高度并固化为绝对值，后续拖拽增量才有基准。
        if target == ResizeTarget::HistoryDetails && self.history_details_height.is_none() {
            let click_y: f32 = event.position.y.into();
            let column_top = self.history_details_top_hint.get();
            let derived = if column_top > 0.0 {
                (click_y - column_top - 4.0)
                    .clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT)
            } else {
                DEFAULT_HISTORY_DETAILS_HEIGHT
            };
            self.history_details_height = Some(derived);
        }
        let state = ResizeState {
            start_x: event.position.x.into(),
            start_y: event.position.y.into(),
            start_width: self.column_width(target),
            start_height: self.row_height(target),
        };
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width = Some(state),
            ResizeTarget::Changes => self.resizing_changes_width = Some(state),
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width = Some(state),
            ResizeTarget::HistoryFiles => self.resizing_history_files_width = Some(state),
            ResizeTarget::HistoryInspectorFiles => {
                self.resizing_history_inspector_files_width = Some(state)
            }
            ResizeTarget::HistoryDetails => self.resizing_history_details_height = Some(state),
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = Some(state),
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width = Some(state),
        }
    }

    pub(crate) fn update_resize_column(&mut self, target: ResizeTarget, event: &MouseMoveEvent) {
        let Some(resize) = self.resize_state(target) else {
            return;
        };
        let current_x: f32 = event.position.x.into();
        let delta = current_x - resize.start_x;
        match target {
            ResizeTarget::HistoryDetails => {
                let current_y: f32 = event.position.y.into();
                let delta = current_y - resize.start_y;
                let height = (resize.start_height + delta)
                    .clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT);
                self.set_row_height(target, height);
            }
            ResizeTarget::HistoryFiles => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_HISTORY_FILES_WIDTH, MAX_HISTORY_FILES_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::WorkflowTemplates => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_WORKFLOW_TEMPLATES_WIDTH, MAX_WORKFLOW_TEMPLATES_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::HistoryInspectorFiles => {
                let width = (resize.start_width + delta).clamp(
                    MIN_HISTORY_INSPECTOR_FILES_WIDTH,
                    MAX_HISTORY_INSPECTOR_FILES_WIDTH,
                );
                self.set_column_width(target, width);
            }
            ResizeTarget::BrowseFiles => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_BROWSE_TREE_WIDTH, MAX_BROWSE_TREE_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::HistoryGraph => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_HISTORY_GRAPH_WIDTH, MAX_HISTORY_GRAPH_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::Sidebar | ResizeTarget::Changes => {
                let width = (resize.start_width + delta).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
                self.set_column_width(target, width);
            }
        }
    }

    pub(crate) fn finish_resize_column(&mut self, target: ResizeTarget) {
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width = None,
            ResizeTarget::Changes => self.resizing_changes_width = None,
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width = None,
            ResizeTarget::HistoryFiles => self.resizing_history_files_width = None,
            ResizeTarget::HistoryInspectorFiles => {
                self.resizing_history_inspector_files_width = None
            }
            ResizeTarget::HistoryDetails => self.resizing_history_details_height = None,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = None,
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width = None,
        }
        // 拖拽结束：布局已定型，同步落库（重启恢复）。
        self.save_layout_preferences();
    }

    pub(crate) fn reset_resize_target(&mut self, target: ResizeTarget) {
        self.finish_resize_column(target);
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = DEFAULT_SIDEBAR_WIDTH,
            ResizeTarget::Changes => self.changes_width = DEFAULT_CHANGES_WIDTH,
            ResizeTarget::WorkflowTemplates => {
                self.workflow_templates_width = DEFAULT_WORKFLOW_TEMPLATES_WIDTH
            }
            ResizeTarget::HistoryFiles => self.history_files_width = DEFAULT_HISTORY_FILES_WIDTH,
            ResizeTarget::HistoryInspectorFiles => {
                self.history_inspector_files_width = DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH
            }
            // 双击复位：回到检查器的默认详情高度。
            ResizeTarget::HistoryDetails => self.history_details_height = None,
            ResizeTarget::BrowseFiles => self.browse_tree_width = DEFAULT_BROWSE_TREE_WIDTH,
            ResizeTarget::HistoryGraph => self.history_graph_width = DEFAULT_HISTORY_GRAPH_WIDTH,
        }
        // finish_resize_column 已保存一次；复位改写了默认值后再保存最终状态。
        self.save_layout_preferences();
    }

    fn column_width(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width,
            ResizeTarget::Changes => self.changes_width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width,
            ResizeTarget::HistoryFiles => self.history_files_width,
            ResizeTarget::HistoryInspectorFiles => self.history_inspector_files_width,
            ResizeTarget::HistoryDetails => 0.0,
            ResizeTarget::BrowseFiles => self.browse_tree_width,
            ResizeTarget::HistoryGraph => self.history_graph_width,
        }
    }

    fn set_column_width(&mut self, target: ResizeTarget, width: f32) {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = width,
            ResizeTarget::Changes => self.changes_width = width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width = width,
            ResizeTarget::HistoryFiles => self.history_files_width = width,
            ResizeTarget::HistoryInspectorFiles => self.history_inspector_files_width = width,
            ResizeTarget::HistoryDetails => {}
            ResizeTarget::BrowseFiles => self.browse_tree_width = width,
            ResizeTarget::HistoryGraph => self.history_graph_width = width,
        }
    }

    fn row_height(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::HistoryDetails => self
                .history_details_height
                .unwrap_or(DEFAULT_HISTORY_DETAILS_HEIGHT),
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::HistoryInspectorFiles
            | ResizeTarget::BrowseFiles
            | ResizeTarget::HistoryGraph => 0.0,
        }
    }

    fn set_row_height(&mut self, target: ResizeTarget, height: f32) {
        match target {
            ResizeTarget::HistoryDetails => self.history_details_height = Some(height),
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::HistoryInspectorFiles
            | ResizeTarget::BrowseFiles
            | ResizeTarget::HistoryGraph => {}
        }
    }

    pub(crate) fn resize_state(&self, target: ResizeTarget) -> Option<ResizeState> {
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width,
            ResizeTarget::Changes => self.resizing_changes_width,
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width,
            ResizeTarget::HistoryFiles => self.resizing_history_files_width,
            ResizeTarget::HistoryInspectorFiles => self.resizing_history_inspector_files_width,
            ResizeTarget::HistoryDetails => self.resizing_history_details_height,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width,
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width,
        }
    }

    pub(crate) fn toggle_diff_headers(&mut self) {
        self.diff_headers_expanded = !self.diff_headers_expanded;
        self.reset_uniform_scroll("diff-scroll");
    }

    pub(crate) fn toggle_history_diff_headers(&mut self) {
        self.history_diff_headers_expanded = !self.history_diff_headers_expanded;
        self.reset_uniform_scroll("history-diff-scroll");
    }

    pub(crate) fn toggle_browse_diff_headers(&mut self) {
        self.browse.diff_headers_expanded = !self.browse.diff_headers_expanded;
        self.reset_uniform_scroll("browse-diff-scroll");
    }

    pub(crate) fn set_main_mode(&mut self, mode: MainMode) {
        self.main_mode = mode;
        // Navigator 偏好由 tab 内的各模式独立保存，切换模式不重置用户选择。
        self.close_popups();
        if self.main_mode == MainMode::Conflict {
            self.ensure_conflict_views_loaded();
            self.sync_conflict_editor_from_state();
            // 进入冲突工作台即为选中文件补算三栏语法高亮
            let panes = [
                ConflictSyntaxPane::Ours,
                ConflictSyntaxPane::Theirs,
                ConflictSyntaxPane::Draft,
            ];
            self.schedule_conflict_syntax_for_selected(&panes);
        }
        if self.main_mode == MainMode::Workflow {
            self.refresh_workflow_templates();
        }
        if self.main_mode == MainMode::History || self.main_mode == MainMode::CommitGraph {
            self.ensure_history_loaded();
        }
    }

    /// 刷新历史时保留旧列表可见，等新数据就绪后直接替换
    pub(crate) fn refresh_history(&mut self) {
        // 保留：commits、graph_rows、has_more、refs_cache、selected_commit。
        // 选中提交的文件列表与差异按 oid 不可变，同样保留展示（与提交列表
        // 同一套 stale-while-revalidate 策略）；若新列表丢弃选中提交，
        // 由 HistoryCommitsLoaded 统一清空文件与差异。
        self.history_refreshing = true;
        self.history_loading = HistoryLoading::default();
    }

    pub(crate) fn diff_cache_key(&self, kind: DiffCacheKind, repo_path: &Path) -> DiffCacheKey {
        DiffCacheKey {
            repo_key: normalize_repo_path(repo_path),
            load_id: self.repository_load_id,
            encoding: self.diff_encoding_choice_for_path(repo_path),
            kind,
            full_file: self.full_file_view,
        }
    }

    pub(crate) fn cached_diff(&self, key: &DiffCacheKey) -> Option<Arc<FileDiff>> {
        self.diff_cache.borrow_mut().get(key).cloned()
    }

    pub(crate) fn cache_diff(&self, key: DiffCacheKey, diff: Arc<FileDiff>) {
        self.diff_cache.borrow_mut().put(key, diff);
    }

    // ===== 提交图谱页 =====

    /// 从主历史页「图谱」按钮进入图谱页。不重置任何图谱状态：
    /// 高亮分支、开关、搜索词与滚动位置跨跳转无损保留。
    pub(crate) fn open_commit_graph(&mut self) {
        self.set_main_mode(MainMode::CommitGraph);
    }

    /// 关闭图谱页返回主历史页。「在提交记录页查看」跳转复用同一出口：
    /// 选中提交与已预加载的文件/差异在四象限直接就位。
    pub(crate) fn close_commit_graph(&mut self) {
        self.set_main_mode(MainMode::History);
        self.status = "已返回提交记录页".to_string();
    }

    /// 图谱页分支高亮下拉的展开/收起（与编码菜单同一套防重开模式）。
    pub(crate) fn toggle_commit_graph_branch_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_graph_branch_menu_closed_by_capture {
            self.commit_graph_branch_menu_closed_by_capture = false;
            self.commit_graph.branch_menu_open = false;
            self.commit_graph_branch_search.clear();
            return;
        }
        // 互斥关闭其余弹层菜单
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        self.commit_graph.branch_menu_open = !self.commit_graph.branch_menu_open;
        if self.commit_graph.branch_menu_open {
            // 打开即清空上次搜索词并聚焦搜索框，输入即过滤（仓库切换下拉同款）。
            self.commit_graph_branch_search.clear();
            window.focus(&self.commit_graph_branch_search.focus, cx);
        }
    }

    /// 设置/清除分支动向高亮；Some 时后台计算谱系 OID 集合。
    pub(crate) fn set_commit_graph_highlight(&mut self, branch: Option<String>) {
        self.close_popups();
        self.commit_graph.highlight_branch = branch.clone();
        self.commit_graph.trace = None;
        match branch {
            Some(branch) => {
                self.status = format!("正在计算分支谱系：{branch}");
                self.refresh_commit_graph_trace();
            }
            None => {
                self.commit_graph.trace_loading = false;
                self.status = "已关闭分支高亮".to_string();
            }
        }
    }

    /// 切换「仅领先 HEAD」模式：重新计算谱系集合（全谱系 ↔ 增量动向）。
    pub(crate) fn toggle_commit_graph_ahead_only(&mut self) {
        if self.commit_graph.highlight_branch.is_none() {
            return;
        }
        self.commit_graph.highlight_ahead_only = !self.commit_graph.highlight_ahead_only;
        self.refresh_commit_graph_trace();
    }

    /// 切换「淡化合并提交」：纯渲染开关，无需后台计算。
    pub(crate) fn toggle_commit_graph_dim_merges(&mut self) {
        self.commit_graph.dim_merges = !self.commit_graph.dim_merges;
    }

    /// 后台计算当前高亮分支的谱系 OID 集合（Short 任务池，纯本地 revwalk）。
    fn refresh_commit_graph_trace(&mut self) {
        let Some(branch) = self.commit_graph.highlight_branch.clone() else {
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let ahead_only = self.commit_graph.highlight_ahead_only;
        let load_id = self.repository_load_id;
        self.commit_graph.trace_seq += 1;
        let seq = self.commit_graph.trace_seq;
        self.commit_graph.trace = None;
        self.commit_graph.trace_loading = true;

        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let (oids, truncated) = service.branch_commit_oids(&repo, &branch, ahead_only)?;
                Ok(UiEvent::CommitTraceLoaded {
                    tab_id,
                    branch,
                    ahead_only,
                    oids,
                    truncated,
                    load_id,
                    seq,
                })
            })();
            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::CommitTraceLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                            seq,
                        },
                    );
                }
            }
        });
    }

    // ===== 分支浏览模式 =====

    /// 从侧边栏分支右键菜单进入浏览模式。
    pub(crate) fn open_browse_branch(&mut self, branch: String, kind: BranchKind) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let ref_kind = match kind {
            BranchKind::Local => BrowseRefKind::LocalBranch,
            BranchKind::Remote => BrowseRefKind::RemoteBranch,
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Tree;
        self.main_mode = MainMode::Browse;
        self.status = format!("正在解析分支 {branch}");
        self.open_browse_resolve(repo_path, tab_id, branch, ref_kind);
    }

    /// 从侧边栏分支右键菜单进入比较模式。
    pub(crate) fn open_compare_branch(&mut self, branch: String, kind: BranchKind) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let ref_kind = match kind {
            BranchKind::Local => BrowseRefKind::LocalBranch,
            BranchKind::Remote => BrowseRefKind::RemoteBranch,
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Compare;
        self.browse.view_mode = BrowseViewMode::Diff;
        // 切换比较目标时作废旧评审。
        self.reset_ai_review_state();
        self.main_mode = MainMode::Browse;
        self.status = format!("正在准备分支比较 {branch}");
        self.open_browse_resolve(repo_path, tab_id, branch, ref_kind);
    }

    /// 从侧边栏标签右键菜单进入浏览模式。
    pub(crate) fn open_browse_tag(&mut self, tag: String) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Tree;
        self.main_mode = MainMode::Browse;
        self.status = format!("正在解析标签 {tag}");
        self.open_browse_resolve(repo_path, tab_id, tag, BrowseRefKind::Tag);
    }

    /// 后台解析目标引用为 BrowseTarget。
    fn open_browse_resolve(
        &mut self,
        repo_path: PathBuf,
        tab_id: RepoTabId,
        name: String,
        ref_kind: BrowseRefKind,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let target = service.resolve_browse_target(&repo, &name, ref_kind)?;
                Ok(UiEvent::BrowseTargetResolved {
                    tab_id,
                    target,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
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

    /// 后台加载某个目录的文件树条目。
    pub(crate) fn load_browse_tree(&mut self, dir: PathBuf) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        let prefix = if dir.as_os_str().is_empty() {
            None
        } else {
            Some(dir.clone())
        };
        self.browse.loading_tree = true;
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let entries = service.browse_tree_entries(&repo, &commit_oid, prefix.as_deref())?;
                Ok(UiEvent::BrowseTreeLoaded {
                    tab_id,
                    dir_path: dir,
                    entries,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
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

    /// 后台加载目标分支与当前 HEAD 的差异文件列表。
    pub(crate) fn load_browse_compare_files(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        self.browse.compare_loading = true;
        self.browse.compare_files.clear();
        // 重新加载差异时清空展开状态，让新比较默认全部展开。
        self.browse.compare_expanded.clear();
        self.browse.selected_file = None;
        self.browse.selected_compare_file = None;
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.status = "正在加载分支差异".to_string();

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let files = service.browse_compare_files(&repo, &commit_oid)?;
                Ok(UiEvent::BrowseCompareFilesLoaded {
                    tab_id,
                    target_oid: commit_oid,
                    files,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
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

    /// 展开/折叠比较差异文件树中的目录。
    /// 第一次操作时把默认全展开固化成显式集合，再增删目标目录，
    /// 避免其它目录意外折叠。
    pub(crate) fn toggle_compare_dir(&mut self, dir: String) {
        if self.browse.compare_expanded.is_empty() {
            self.browse.compare_expanded =
                browse_compare_view::all_compare_dirs(&self.browse.compare_files);
        }
        if self.browse.compare_expanded.contains(&dir) {
            self.browse.compare_expanded.remove(&dir);
        } else {
            self.browse.compare_expanded.insert(dir);
        }
    }

    /// 展开/折叠目录；展开时按需懒加载子树。
    pub(crate) fn toggle_browse_dir(&mut self, path: PathBuf) {
        let already_loaded = self
            .browse
            .entries_by_dir
            .contains_key(&BrowseState::dir_key(&path));
        if self.browse.expanded.contains(&path) {
            self.browse.expanded.remove(&path);
        } else {
            self.browse.expanded.insert(path.clone());
            if !already_loaded {
                self.load_browse_tree(path);
            }
        }
    }

    /// 选中文件树文件并按当前模式加载内容或差异。
    pub(crate) fn select_browse_file(&mut self, path: PathBuf) {
        if self.browse.selected_file.as_ref() == Some(&path)
            && (self.browse.content.is_some() || self.browse.diff.is_some())
        {
            return;
        }
        self.browse.selected_file = Some(path.clone());
        self.browse.selected_compare_file = None;
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    /// 选中比较模式中的差异文件，并保留旧路径/状态供 diff 与全文视图判断。
    pub(crate) fn select_browse_compare_file(&mut self, file: BrowseCompareFile) {
        let path = PathBuf::from(&file.path);
        if self.browse.selected_file.as_ref() == Some(&path)
            && self.browse.selected_compare_file.as_ref() == Some(&file)
            && (self.browse.content.is_some() || self.browse.diff.is_some())
        {
            return;
        }
        self.browse.selected_file = Some(path);
        self.browse.selected_compare_file = Some(file);
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    fn clear_browse_current_views(&mut self) {
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.clear_browse_selection();
        self.reset_uniform_scroll("browse-content-scroll");
        self.reset_uniform_scroll("browse-diff-scroll");
    }

    /// 切换内容/差异视图模式，并按需重新加载。
    pub(crate) fn set_browse_view_mode(&mut self, mode: BrowseViewMode) {
        if self.browse.view_mode == mode {
            return;
        }
        self.browse.view_mode = mode;
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    /// 根据当前选中的文件和视图模式触发后台加载。
    pub(crate) fn load_browse_current(&mut self) {
        let Some(path) = self.browse.selected_file.clone() else {
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let mode = self.browse.view_mode;
        let compare_file = self.browse.selected_compare_file.clone();
        let old_path = compare_file
            .as_ref()
            .and_then(|file| file.old_path.as_ref())
            .map(PathBuf::from);

        if mode == BrowseViewMode::Content
            && compare_file
                .as_ref()
                .is_some_and(|file| file.status == ChangeState::Deleted)
        {
            self.browse.loading_content = false;
            self.status = "目标分支中不存在该文件".to_string();
            return;
        }

        match mode {
            BrowseViewMode::Content => {
                self.browse.loading_content = true;
                self.status = "正在加载文件内容".to_string();
            }
            BrowseViewMode::Diff => {
                self.browse.loading_diff = true;
                self.status = "正在加载文件差异".to_string();
            }
        }

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                match mode {
                    BrowseViewMode::Content => {
                        let content =
                            service.browse_file_content(&repo, &commit_oid, &path, encoding)?;
                        Ok(UiEvent::BrowseFileContentLoaded {
                            tab_id,
                            path: path.to_string_lossy().to_string(),
                            content,
                            load_id,
                        })
                    }
                    BrowseViewMode::Diff => {
                        let diff = service.browse_file_diff_for_compare(
                            &repo,
                            &commit_oid,
                            &path,
                            old_path.as_deref(),
                            full_context,
                            encoding,
                        )?;
                        Ok(UiEvent::BrowseFileDiffLoaded {
                            tab_id,
                            path: path.to_string_lossy().to_string(),
                            diff,
                            load_id,
                        })
                    }
                }
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
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

    /// 关闭浏览模式，回到工作区。
    pub(crate) fn close_browse(&mut self) {
        self.main_mode = MainMode::Worktree;
        // 评审针对具体比较目标，退出浏览时整体作废，避免下次进入比较模式
        // 时残留旧评审直接占满右侧区域。
        self.reset_ai_review_state();
        self.status = "已退出分支浏览".to_string();
    }

    /// 分离当前评审展示（**不终止在途任务**）：切目标/退出浏览时调用。
    /// 后台任务继续执行，完成后落盘并 toast 提示；其后续事件因代际
    /// 不匹配被丢弃，不会污染新目标的展示状态。
    fn reset_ai_review_state(&mut self) {
        self.ai_review = None;
        self.ai_review_steps.clear();
        self.ai_review_step_expanded.clear();
        self.ai_review_progress = None;
        self.ai_review_live_reasoning.clear();
        self.ai_review_live_content.clear();
        self.ai_review_expanded = false;
        self.ai_review_loaded_label = None;
        self.ai_review_active_generation = None;
        self.ai_review_loading = false;
        // 取消标志不动：任务继续后台执行直到结束落盘。
    }

    /// 取消当前附着的评审任务：置位取消标志并分离展示；任务在轮次
    /// 边界自行退出（不落盘、不提示失败、不占并发名额的时间超过必要）。
    pub(crate) fn cancel_ai_review(&mut self) {
        if let Some(flag) = self.ai_review_cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.reset_ai_review_state();
        self.status = "已取消 AI 评审".into();
    }

    /// 打开评审历史弹窗并后台加载当前仓库的最近记录。
    pub(crate) fn open_ai_review_history(&mut self) {
        if self.ai_review_history.is_some() {
            return;
        }
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.last_error = Some("无法定位数据目录".into());
            return;
        };
        self.ai_review_history = Some(AiReviewHistoryState {
            loading: true,
            records: Vec::new(),
            error: None,
        });
        self.status = "正在加载评审记录".into();
        let tx = self.tx.clone();
        self.tasks.spawn(crate::TaskKind::Short, move || {
            let repo_path_string = repo_path.display().to_string();
            let result = khaslana::ai::list_review_records(
                &data_dir,
                &repo_path_string,
                AI_REVIEW_HISTORY_LIMIT,
            );
            match result {
                Ok(records) => {
                    crate::send_ui_event(&tx, crate::UiEvent::AiReviewHistoryLoaded { records })
                }
                Err(err) => crate::send_ui_event(
                    &tx,
                    crate::UiEvent::AiReviewHistoryLoadFailed {
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 关闭评审历史弹窗。
    pub(crate) fn close_ai_review_history(&mut self) {
        self.ai_review_history = None;
    }

    /// 把一条历史记录载入评审面板（若有生成中的附着任务先分离，其继续
    /// 后台执行并落盘）。
    pub(crate) fn open_ai_review_record(&mut self, record: AiReviewRecord) {
        self.close_ai_review_history();
        self.reset_ai_review_state();
        let date =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(record.created_at_millis as i64)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "时间未知".to_string());
        self.ai_review_loaded_label =
            Some(format!("历史 · {date} · {}", record.target_display_name));
        self.ai_review = Some(Arc::new(record.result.clone()));
        self.ai_review_steps = record.result.steps;
        self.ai_review_expanded = true;
        self.status = "已打开历史评审记录".into();
        self.last_error = None;
    }

    /// 对比视图依赖具体分支的差异；切换分支/检出后旧差异会失效，
    /// 命中对比视图时关闭它，回到工作区，让新 HEAD 的状态正常展示。
    pub(crate) fn close_browse_if_comparing(&mut self) {
        if self.main_mode == MainMode::Browse && self.browse.list_mode == BrowseListMode::Compare {
            self.close_browse();
        }
        // 追溯视图同样基于 HEAD：检出后内容失效，一并关闭。
        if self.main_mode == MainMode::Blame {
            self.close_blame();
        }
    }

    // ===== 文件追溯（blame）视图 =====

    /// 打开某文件的追溯视图：置状态、切主模式并后台加载。
    ///
    /// 历史页提交文件右键入口对 HEAD 版本追溯（v1 不支持对任意提交 blame，
    /// `BlameOptions::newest_commit` 留作后续）；工作区入口同时纳入未提交
    /// 改动（服务端经 blame_buffer 处理）。
    pub(crate) fn open_blame_file(&mut self, path: String) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let load_id = self.repository_load_id;
        self.blame.reset();
        self.blame.path = Some(path.clone());
        self.blame.loading = true;
        self.main_mode = MainMode::Blame;
        self.status = format!("正在加载文件追溯：{path}");
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let view = service.blame_file(&repo, Path::new(&path), encoding)?;
                Ok(UiEvent::BlameLoaded {
                    tab_id,
                    path: path.clone(),
                    view,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::BlameLoadFailed {
                        tab_id,
                        path,
                        error: err.to_string(),
                        load_id,
                    },
                ),
            }
        });
    }

    /// 关闭追溯视图，回到工作区（仿浏览模式）。
    pub(crate) fn close_blame(&mut self) {
        self.blame.reset();
        self.main_mode = MainMode::Worktree;
        self.status = "已退出文件追溯".to_string();
    }

    /// 编码切换时重新加载当前追溯文件。
    pub(crate) fn reload_blame_on_encoding_change(&mut self) {
        if self.main_mode != MainMode::Blame {
            return;
        }
        if let Some(path) = self.blame.path.clone() {
            self.open_blame_file(path);
        }
    }

    /// 右键菜单「查看文件历史」入口：设置历史页路径过滤并切换过去；
    /// 已在历史页时仅设置过滤器。
    pub(crate) fn view_file_history(&mut self, path: String) {
        self.close_popups();
        let in_history = self.main_mode == MainMode::History;
        self.set_history_file_filter(Some(path));
        if !in_history {
            self.set_main_mode(MainMode::History);
        }
    }

    // ===== 语法高亮调度（统一后台补算 + Arc 身份守卫） =====

    /// 为当前活动 tab 的某个槽位调度后台语法高亮。
    ///
    /// 调度只发生在「内容落位」之后（各加载事件处理分支 / 缓存命中路径），
    /// 计算是纯 CPU 任务（不碰 git），结果经 `SyntaxHighlighted` 回填；
    /// diff 槽位仅在全文模式且非二进制时调度（紧凑差异块不高亮）。
    pub(crate) fn schedule_syntax_highlight(&mut self, slot: SyntaxSlot) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let source = match slot {
            SyntaxSlot::WorktreeDiff => self.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::HistoryDiff => self.history_diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::StashDiff => self.stash_preview.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::BrowseDiff => self.browse.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::Blame => self.blame.view.clone().map(SyntaxSource::Blame),
            SyntaxSlot::BrowseContent => self.browse.content.clone().map(SyntaxSource::Content),
        };
        let Some(source) = source else {
            return;
        };
        if let SyntaxSource::Diff(diff) = &source
            && (!self.full_file_view || diff.is_binary)
        {
            return;
        }
        if let SyntaxSource::Content(content) = &source
            && content.is_binary
        {
            return;
        }
        let (anchor, anchor_len) = source.anchor();
        let dark = ui::theme::active_variant().is_dark();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let spans = match &source {
                SyntaxSource::Diff(diff) => khaslana::syntax::highlight_diff_lines(diff, dark),
                SyntaxSource::Blame(view) => {
                    khaslana::syntax::highlight(&view.path, &view.lines, dark)
                }
                SyntaxSource::Content(content) => {
                    khaslana::syntax::highlight(&content.path, &content.lines, dark)
                }
            };
            send_ui_event(
                &tx,
                UiEvent::SyntaxHighlighted {
                    tab_id,
                    slot,
                    anchor,
                    anchor_len,
                    spans: spans.map(Arc::new),
                },
            );
        });
    }

    /// 为当前选中的冲突文件调度指定分栏的语法高亮。
    ///
    /// 只计算选中文件（渲染也只看选中文件）；draft 每次调度递增 seq，
    /// 晚到的旧结果在回填时被丢弃。二进制冲突文件无文本栏，直接跳过。
    pub(crate) fn schedule_conflict_syntax_for_selected(&mut self, panes: &[ConflictSyntaxPane]) {
        let panes: Vec<ConflictSyntaxPane> = panes.to_vec();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let (ours, theirs, draft) = match self.conflict_workbench.files.get(&path) {
            Some(view) if view.kind == ConflictFileKind::Text => (
                view.ours_text.clone(),
                view.theirs_text.clone(),
                view.draft.clone(),
            ),
            _ => return,
        };
        let dark = ui::theme::active_variant().is_dark();
        for pane in panes {
            let (text, seq) = match pane {
                ConflictSyntaxPane::Ours => (ours.clone(), 0),
                ConflictSyntaxPane::Theirs => (theirs.clone(), 0),
                ConflictSyntaxPane::Draft => {
                    let entry = self
                        .conflict_workbench
                        .syntax
                        .entry(path.clone())
                        .or_default();
                    entry.draft_seq += 1;
                    (draft.clone(), entry.draft_seq)
                }
            };
            let task_path = path.clone();
            let tx = self.tx.clone();
            self.tasks.spawn(TaskKind::Short, move || {
                let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
                let spans = khaslana::syntax::highlight(&task_path, &lines, dark);
                send_ui_event(
                    &tx,
                    UiEvent::ConflictSyntaxHighlighted {
                        tab_id,
                        path: task_path,
                        pane,
                        seq,
                        spans: spans.map(Arc::new),
                    },
                );
            });
        }
    }

    /// 主题深浅切换后：清空全部语法高亮并按新变体重新调度。
    ///
    /// 只是从各槽位现存的 Arc/文本补算，不做任何 git 重载；
    /// 含非活动 tab（调度是读 Arc 的轻量任务）。
    pub(crate) fn invalidate_and_refresh_syntax_highlights(&mut self) {
        let slots_by_tab: Vec<(RepoTabId, Vec<SyntaxSlot>)> = self
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.id,
                    vec![
                        SyntaxSlot::WorktreeDiff,
                        SyntaxSlot::HistoryDiff,
                        SyntaxSlot::StashDiff,
                        SyntaxSlot::BrowseDiff,
                        SyntaxSlot::Blame,
                        SyntaxSlot::BrowseContent,
                    ],
                )
            })
            .collect();
        for (tab_id, slots) in slots_by_tab {
            self.with_tab_context(tab_id, |this| {
                this.diff_syntax = None;
                this.history_diff_syntax = None;
                this.stash_preview.diff_syntax = None;
                this.browse.diff_syntax = None;
                this.browse.content_syntax = None;
                this.blame.syntax = None;
                this.conflict_workbench.syntax.clear();
                for slot in slots {
                    this.schedule_syntax_highlight(slot);
                }
                let panes = [
                    ConflictSyntaxPane::Ours,
                    ConflictSyntaxPane::Theirs,
                    ConflictSyntaxPane::Draft,
                ];
                this.schedule_conflict_syntax_for_selected(&panes);
            });
        }
    }

    /// 将鼠标 Y 坐标映射到内容行索引（基于 uniform_list 滚动偏移与行高）。
    pub(crate) fn browse_row_for_mouse_y(&self, y: Pixels, line_count: usize) -> usize {
        let scroll = self.uniform_scroll_handle("browse-content-scroll");
        let state = scroll.0.borrow();
        let bounds = state.base_handle.bounds();
        let offset_y = f32::from(state.base_handle.offset().y);
        let row = ((f32::from(y) - f32::from(bounds.top()) - offset_y)
            / crate::browse_view::BROWSE_ROW_HEIGHT)
            .floor()
            .max(0.0) as usize;
        row.min(line_count.saturating_sub(1))
    }

    /// 清空行级选区。
    pub(crate) fn clear_browse_selection(&mut self) {
        self.browse.selecting = false;
        self.browse.sel_start = None;
        self.browse.sel_end = None;
    }

    /// 编码切换时重新加载当前浏览文件。
    pub(crate) fn reload_browse_on_encoding_change(&mut self) {
        if self.main_mode != MainMode::Browse {
            return;
        }
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.load_browse_current();
    }

    pub(crate) fn set_history_scope(&mut self, scope: HistoryScope) {
        if self.history_scope == scope {
            return;
        }
        self.history_scope = scope;
        // 过滤器是用户意图：切 scope 保留，仅显式清除（chip 的 ×）。
        self.clear_history();
        self.status = format!("提交记录范围已切换为{}", scope.label());
        self.load_history_page(false);
    }

    /// 设置/清除历史页的文件路径过滤（None 为清除）。
    ///
    /// 仿 `set_history_scope`：设字段 -> 清列表（`clear_history` 不清过滤器）
    /// -> 全量重载。切换分支、刷新等操作同样保留过滤器，per-tab 生命周期
    /// 随 tab 自然销毁。
    pub(crate) fn set_history_file_filter(&mut self, path: Option<String>) {
        if self.history_file_filter == path {
            return;
        }
        let label = path
            .as_deref()
            .map(|path| format!("提交记录已按文件 {path} 过滤"))
            .unwrap_or_else(|| "已清除文件过滤".to_string());
        self.history_file_filter = path;
        self.clear_history();
        self.status = label;
        self.load_history_page(false);
    }

    pub(crate) fn reload_history_if_active(&mut self) {
        if self.main_mode == MainMode::History {
            self.load_history_page(false);
        }
    }

    /// 历史可能已过时：正在查看历史页或已有历史列表时立即后台刷新，
    /// 不受当前视图限制；历史列表为空且不在历史页（用户从未看过历史）时
    /// 保持现状，等进入历史页时由 ensure_history_loaded 拉取，避免预加载。
    pub(crate) fn reload_history_after_change(&mut self) {
        if self.repo_path.is_some()
            && !self.history_loading.commits
            && (matches!(self.main_mode, MainMode::History | MainMode::CommitGraph)
                || !self.history_commits.is_empty())
        {
            self.load_history_page(false);
        }
    }

    pub(crate) fn ensure_history_loaded(&mut self) {
        if matches!(self.main_mode, MainMode::History | MainMode::CommitGraph)
            && self.repo_path.is_some()
            // 列表为空时首载；被标记陈旧（操作后未在后台刷新）时重新拉取
            && (self.history_commits.is_empty() || self.history_refreshing)
            && !self.history_loading.commits
        {
            self.load_history_page(false);
        }
    }

    fn load_history_page(&mut self, append: bool) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        if self.history_loading.commits {
            return;
        }

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let scope = self.history_scope;
        // 文件路径过滤：非空时分派到 file_history（只返回改动过该文件的提交）
        let path_filter = self.history_file_filter.clone();
        // 仅分页（append）复用 refs 缓存；全量刷新传 None 重建，
        // 保证切换分支、提交等操作后 HEAD/分支/标签徽章与最新仓库状态一致。
        let refs_cache = if append {
            self.history_refs_cache.clone()
        } else {
            None
        };
        let offset = if append {
            self.history_commits.len()
        } else {
            0
        };
        let load_id = self.repository_load_id;
        self.history_load_seq += 1;
        let seq = self.history_load_seq;
        self.history_loading.commits = true;
        self.status = if append {
            "正在加载更多提交记录".to_string()
        } else {
            "正在加载提交记录".to_string()
        };
        self.last_error = None;

        // 图谱页高亮激活时随全量历史刷新同步重算谱系：提交/拉取/切换分支等
        // 操作可能移动分支 tip，旧 OID 集合会静默失真。append 分页不改 tip，
        // 无需重算。
        if !append && self.commit_graph.highlight_branch.is_some() {
            self.refresh_commit_graph_trace();
        }

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let (mut commits, refs_cache) = match path_filter.as_deref() {
                    Some(path) => service.file_history(
                        &repo,
                        scope,
                        path,
                        offset,
                        HISTORY_PAGE_SIZE + 1,
                        refs_cache.as_ref(),
                    )?,
                    None => service.commit_history_with_refs(
                        &repo,
                        scope,
                        offset,
                        HISTORY_PAGE_SIZE + 1,
                        refs_cache.as_ref(),
                    )?,
                };
                let has_more = commits.len() > HISTORY_PAGE_SIZE;
                commits.truncate(HISTORY_PAGE_SIZE);
                perf_log(
                    "history.commits",
                    started,
                    format!(
                        "tab={} scope={} append={} offset={} commits={} has_more={} path_filter={:?}",
                        tab_id.0,
                        scope.label(),
                        append,
                        offset,
                        commits.len(),
                        has_more,
                        path_filter
                    ),
                );
                Ok(UiEvent::HistoryCommitsLoaded {
                    tab_id,
                    commits,
                    refs_cache,
                    append,
                    has_more,
                    scope,
                    path_filter,
                    load_id,
                    seq,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn load_more_history(&mut self) {
        if !self.history_has_more {
            return;
        }
        self.load_history_page(true);
    }

    pub(crate) fn select_history_commit(&mut self, oid: String) {
        if self.history_selected_commit.as_deref() == Some(oid.as_str())
            && !self.history_files.is_empty()
        {
            return;
        }

        self.history_selected_commit = Some(oid.clone());
        self.history_files.clear();
        self.history_selected_file = None;
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
        self.reset_uniform_scroll("history-diff-scroll");
        self.history_loading.diff = false;

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        // 加载标志与状态文字在守卫之后设置：守卫早退时不应挂起
        // “提交文件加载中...”（文件区会永久显示加载占位）。
        self.history_loading.files = true;
        self.status = "正在加载提交文件".to_string();
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let files = service.commit_files(&repo, &oid)?;
                perf_log(
                    "history.files",
                    started,
                    format!("tab={} files={}", tab_id.0, files.len()),
                );
                Ok(UiEvent::HistoryFilesLoaded {
                    tab_id,
                    commit_oid: oid,
                    files,
                    load_id,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn select_history_file(&mut self, path: String) {
        self.select_history_file_with_reload(path, false);
    }

    pub(crate) fn select_history_file_with_reload(&mut self, path: String, force_reload: bool) {
        let Some(commit_oid) = self.history_selected_commit.clone() else {
            return;
        };
        if !force_reload
            && self.history_selected_file.as_deref() == Some(path.as_str())
            && self.history_diff.is_some()
        {
            return;
        }

        self.history_selected_file = Some(path.clone());
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
        self.reset_uniform_scroll("history-diff-scroll");
        self.history_loading.diff = true;
        self.status = "正在加载提交差异".to_string();

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let cache_key = self.diff_cache_key(
            DiffCacheKind::History {
                commit_oid: commit_oid.clone(),
                path: path.clone(),
            },
            &repo_path,
        );
        if !force_reload && let Some(diff) = self.cached_diff(&cache_key) {
            self.history_loading.diff = false;
            self.history_diff_syntax = None;
            self.history_diff = Some(diff);
            self.history_diff_headers_expanded = false;
            self.status = "提交差异已加载".to_string();
            // 缓存命中不走事件落位，语法高亮在此手动调度
            self.schedule_syntax_highlight(SyntaxSlot::HistoryDiff);
            return;
        }
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let diff = service.commit_file_diff(
                    &repo,
                    &commit_oid,
                    Path::new(&path),
                    full_context,
                    encoding,
                )?;
                perf_log(
                    "history.diff",
                    started,
                    format!("tab={} lines={}", tab_id.0, diff.lines.len()),
                );
                Ok(UiEvent::HistoryDiffLoaded {
                    tab_id,
                    commit_oid,
                    path,
                    diff,
                    load_id,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn stage_selected(&mut self) {
        let paths = self.selected_change_paths(DiffScope::Unstaged);
        if paths.is_empty() {
            self.last_error = Some("请先在修改区选择文件".into());
            return;
        }
        self.stage_paths(paths, "已暂存选定文件");
    }

    pub(crate) fn stage_all(&mut self) {
        let paths = self.change_paths(DiffScope::Unstaged);
        if paths.is_empty() {
            self.last_error = Some("修改区没有可暂存文件".into());
            return;
        }
        self.stage_paths(paths, "已暂存所有文件");
    }

    pub(crate) fn stage_paths(&mut self, paths: Vec<String>, label: &'static str) {
        self.with_repo(label, move |service, repo| {
            let path_bufs = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            service.stage_paths(repo, path_bufs.iter().map(|path| path.as_path()))
        });
    }

    pub(crate) fn unstage_selected(&mut self) {
        let paths = self.selected_change_paths(DiffScope::Staged);
        if paths.is_empty() {
            self.last_error = Some("请先在暂存区选择文件".into());
            return;
        }
        self.unstage_paths(paths, "已取消暂存选定文件");
    }

    pub(crate) fn unstage_all(&mut self) {
        let paths = self.change_paths(DiffScope::Staged);
        if paths.is_empty() {
            self.last_error = Some("暂存区没有可取消暂存文件".into());
            return;
        }
        self.unstage_paths(paths, "已取消暂存所有文件");
    }

    pub(crate) fn unstage_paths(&mut self, paths: Vec<String>, label: &'static str) {
        self.with_repo(label, move |service, repo| {
            let path_bufs = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            service.unstage_paths(repo, path_bufs.iter().map(|path| path.as_path()))
        });
    }

    pub(crate) fn commit(&mut self) {
        if self.merge_in_progress() {
            self.finish_merge();
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        if message.is_empty() {
            self.last_error = Some("需要填写提交信息".into());
            return;
        }
        self.commit_message.clear();
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.with_repo_blocking("提交完成", move |service, repo| {
            service.commit(repo, &CommitMessage::new(message))
        });
    }

    /// 修补最后一次提交：以当前暂存区为树重写 HEAD。
    /// 输入框为空时保留原提交信息（只补文件不改信息的场景）。
    pub(crate) fn amend(&mut self) {
        if !self.ensure_no_merge_in_progress("修补提交") {
            return;
        }
        if self.amend_needs_push_warning() {
            self.open_amend_pushed_confirm_dialog(false);
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        self.perform_amend(message);
    }

    /// 修补最后一次提交并推送当前分支。
    pub(crate) fn amend_and_push(&mut self) {
        if !self.ensure_no_merge_in_progress("修补提交并推送") {
            return;
        }
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        if self.amend_needs_push_warning() {
            self.open_amend_pushed_confirm_dialog(true);
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        self.perform_amend_and_push(message, remote);
    }

    pub(crate) fn perform_amend(&mut self, message: String) {
        let message = (!message.is_empty()).then(|| CommitMessage::new(message));
        self.commit_message.clear();
        self.amend_mode = false;
        self.amend_prefill = None;
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.with_repo_blocking("修补提交完成", move |service, repo| {
            service.amend_commit(repo, message.as_ref())
        });
    }

    /// 修补后推送：组合错误处理与 commit_and_push 一致——修补成功但推送
    /// 失败时保留修补结果并给出组合提示。
    pub(crate) fn perform_amend_and_push(&mut self, message: String, remote: String) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let message = (!message.is_empty()).then(|| CommitMessage::new(message));
        self.commit_message.clear();
        self.amend_mode = false;
        self.amend_prefill = None;
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            "正在修补提交并推送",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(path)?;
                let snapshot = service.amend_commit(&mut repo, message.as_ref())?;
                match service.push(&mut repo, &RemoteName::new(remote)) {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: "修补提交并推送完成".to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err(err) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: format!("修补已完成，但推送失败：{err}"),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                }
            },
        );
    }

    /// 修补的 HEAD 是否已推送（据 branch_sync_status 判断，数据可能略陈旧：
    /// 误判为已推送只会多一次确认，安全方向）。无 upstream 视为未推送。
    /// 预填修补模式的 HEAD 提交信息：优先用内存中的历史数据（已加载过
    /// 提交记录时即时命中）；否则后台读取 HEAD（不依赖进入过历史页），
    /// 经 `AmendPrefillLoaded` 事件回填。
    pub(crate) fn prefill_amend_message(&mut self) {
        // 同步路径仅在历史数据确定新鲜时使用：刷新中（提交/推送等操作后
        // 的后台重载还在飞行）旧列表里的 HEAD 徽章已过时，会预填旧提交的
        // 信息，此时直接走后台读 HEAD 的兜底路径。
        if !self.history_refreshing
            && let Some(message) = self
                .history_commits
                .iter()
                .find(|commit| {
                    commit
                        .refs
                        .iter()
                        .any(|reference| reference.kind == khaslana::CommitRefKind::Head)
                })
                .map(|commit| commit.message.clone())
        {
            self.amend_prefill = Some(message.clone());
            self.commit_message.set_value(message);
            // caret 归零：预填信息通常从首行（主题）开始编辑，避免滚动到底。
            self.commit_message.caret = 0;
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let message = (|| -> khaslana::Result<Option<String>> {
                let repo = Repository::open(path)?;
                service.head_commit_message(&repo)
            })()
            // 仓库打开失败等视作无预填，不打断用户。
            .unwrap_or(None);
            send_ui_event(&tx, UiEvent::AmendPrefillLoaded { tab_id, message });
        });
    }

    fn amend_needs_push_warning(&self) -> bool {
        let Some(status) = self.branch_sync_status.as_ref() else {
            return false;
        };
        let Some(head) = self.snapshot.as_ref().map(|snapshot| snapshot.head.clone()) else {
            return false;
        };
        status.branch == head.unwrap_or_default() && status.upstream.is_some() && status.ahead == 0
    }

    fn open_amend_pushed_confirm_dialog(&mut self, and_push: bool) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmAmendPushed { and_push });
        self.last_error = None;
    }

    /// 拣选提交到当前分支（历史页右键菜单入口）。
    pub(crate) fn cherry_pick_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("拣选提交") {
            return;
        }
        // 工作区脏时提前拦截，避免后台任务失败后才反馈。
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.changes.is_empty())
        {
            self.last_error = Some("工作区有未提交修改，请先提交、暂存或丢弃后再拣选".into());
            return;
        }
        self.with_repo_blocking("拣选提交完成", move |service, repo| {
            service.cherry_pick_commit(repo, &oid)
        });
    }

    pub(crate) fn commit_and_push(&mut self) {
        if !self.ensure_no_merge_in_progress("提交并推送") {
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        if message.is_empty() {
            self.last_error = Some("需要填写提交信息".into());
            return;
        }
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.commit_message.clear();
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            "正在提交并推送",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(path)?;
                match service.commit_and_push(
                    &mut repo,
                    &CommitMessage::new(message),
                    &RemoteName::new(remote),
                )? {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: "提交并推送完成".to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err((snapshot, err)) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: format!("提交已完成，但推送失败：{err}"),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                }
            },
        );
    }

    pub(crate) fn load_diff(&mut self, path: String, scope: DiffScope) {
        self.reset_uniform_scroll("diff-scroll");
        // 差异内容变化后行索引失效：清空按行选择。
        self.diff_line_selection.clear();
        self.diff_line_selection_anchor = None;
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let cache_key = self.diff_cache_key(
            DiffCacheKind::Worktree {
                scope: scope.clone(),
                path: path.clone(),
            },
            &repo_path,
        );
        if let Some(diff) = self.cached_diff(&cache_key) {
            self.diff = Some(diff);
            self.diff_headers_expanded = false;
            self.diff_syntax = None;
            self.status = "差异已加载".to_string();
            // 缓存命中不走事件落位，语法高亮在此手动调度
            self.schedule_syntax_highlight(SyntaxSlot::WorktreeDiff);
            return;
        }
        let is_conflicted_path = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.conflicts.iter().any(|conflict| conflict == &path));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab(Some(tab_id), "正在加载差异", move || {
            let started = Instant::now();
            let repo = Repository::open(repo_path)?;
            let diff = service
                .diff_for_path(&repo, Path::new(&path), scope, full_context, encoding)
                .map_err(|err| {
                    if is_conflicted_path {
                        khaslana::GitError::Message(
                            "该文件存在冲突，请选择版本或手动编辑后标记解决".into(),
                        )
                    } else {
                        err
                    }
                })?;
            perf_log(
                "worktree.diff",
                started,
                format!("tab={} lines={}", tab_id.0, diff.lines.len()),
            );
            Ok(UiEvent::OperationFinished {
                tab_id: Some(tab_id),
                message: "差异已加载".to_string(),
                snapshot: None,
                diff: Some(diff),
            })
        });
    }

    /// 暂存/取消暂存（整文件或按块/按行）完成后刷新差异面板：
    /// - 当前差异的 (path, scope) 仍有改动 → 原位重载，反映最新暂存状态；
    /// - 已失效（整文件被挪到对侧列表）→ 清空差异面板，避免残留旧内容
    ///   与失效的「暂存此块/取消暂存此块」按钮。
    /// 存在性按操作后的快照判定（见 `diff_scope_still_present`）。
    pub(crate) fn refresh_diff_after_stage_change(&mut self) {
        let Some(diff) = self.diff.clone() else {
            return;
        };
        let path = diff.path.clone();
        let scope = diff.scope.clone();
        let still_present = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| diff_scope_still_present(&snapshot.changes, &path, &scope));
        if still_present {
            self.load_diff(path, scope);
        } else {
            self.diff = None;
            self.diff_headers_expanded = false;
            self.diff_line_selection.clear();
            self.diff_line_selection_anchor = None;
            self.reset_uniform_scroll("diff-scroll");
        }
    }

    // ── 按行/按块部分暂存（工作区差异视图）─────────────────────────

    /// 切换差异行的选中态。与变更列表同一套语义：普通点击单选（再点取消）、
    /// Ctrl/Cmd 多选、Shift 从锚点范围选择（替换现有选择）。
    pub(crate) fn toggle_diff_line_selection(&mut self, index: usize, multi: bool, shift: bool) {
        // 两个字段经 Deref 落在 RepoTabState 上，需先取出同一可变引用。
        let state = self.active_tab_state_mut();
        toggle_index_selection(
            &mut state.diff_line_selection,
            &mut state.diff_line_selection_anchor,
            index,
            multi,
            shift,
        );
    }

    /// 把选中的 diff 行索引转换为服务层的行号选择
    ///（Added 用 new_lineno、Removed 用 old_lineno；上下文行忽略）。
    fn diff_line_indices_to_selection(
        &self,
        indices: impl Iterator<Item = usize>,
    ) -> Option<LineSelection> {
        let diff = self.diff.as_ref()?;
        let mut selection = LineSelection::new();
        for index in indices {
            let Some(line) = diff.lines.get(index) else {
                continue;
            };
            match line.kind {
                DiffLineKind::Added => {
                    if let Some(lineno) = line.new_lineno {
                        selection.insert(SelectedDiffLine {
                            side: SelectionSide::Added,
                            lineno,
                        });
                    }
                }
                DiffLineKind::Removed => {
                    if let Some(lineno) = line.old_lineno {
                        selection.insert(SelectedDiffLine {
                            side: SelectionSide::Removed,
                            lineno,
                        });
                    }
                }
                _ => {}
            }
        }
        (!selection.is_empty()).then_some(selection)
    }

    /// 当前选中行对应的部分暂存选择。
    fn selected_diff_lines_selection(&self) -> Option<LineSelection> {
        let indices = self.diff_line_selection.iter().copied().collect::<Vec<_>>();
        self.diff_line_indices_to_selection(indices.into_iter())
    }

    /// 指定 hunk 的全部 +/- 行对应的部分暂存选择。
    fn diff_hunk_selection(&self, hunk_index: usize) -> Option<LineSelection> {
        let diff = self.diff.as_ref()?;
        let indices = diff
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.hunk_index == hunk_index)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        self.diff_line_indices_to_selection(indices.into_iter())
    }

    /// 执行部分暂存/取消暂存：按当前差异的 scope 决定方向
    ///（未暂存 → 暂存选中改动；已暂存 → 取消暂存选中改动）。
    fn apply_partial_stage(&mut self, selection: LineSelection) {
        let Some(diff) = self.diff.clone() else {
            return;
        };
        if diff.is_binary {
            self.last_error = Some("二进制文件不支持部分暂存".into());
            return;
        }
        let path = diff.path.clone();
        let is_stage = diff.scope == DiffScope::Unstaged;
        let label: &'static str = if is_stage {
            "已暂存选中改动"
        } else {
            "已取消暂存选中改动"
        };
        // 选区随差异刷新清空（load_diff 开头统一处理，这里立即清掉按钮态）。
        self.diff_line_selection.clear();
        self.diff_line_selection_anchor = None;
        self.with_repo(label, move |service, repo| {
            if is_stage {
                service.stage_lines(repo, Path::new(&path), &selection)
            } else {
                service.unstage_lines(repo, Path::new(&path), &selection)
            }
        });
    }

    /// 工具栏按钮：暂存/取消暂存当前选中的行。
    pub(crate) fn apply_selected_partial_stage(&mut self) {
        let Some(selection) = self.selected_diff_lines_selection() else {
            self.last_error = Some("请先点击差异中的 +/- 行选择要暂存的改动".into());
            return;
        };
        self.apply_partial_stage(selection);
    }

    /// hunk 头按钮：暂存/取消暂存整块。
    pub(crate) fn apply_hunk_partial_stage(&mut self, hunk_index: usize) {
        let Some(selection) = self.diff_hunk_selection(hunk_index) else {
            self.last_error = Some("该块没有可暂存的改动".into());
            return;
        };
        self.apply_partial_stage(selection);
    }

    pub(crate) fn use_credentials(&mut self) {
        let Some(pending) = self.pending_credential.clone() else {
            return;
        };

        let username = self
            .credential_username
            .value
            .trim()
            .to_string()
            .if_empty_then(|| {
                pending
                    .request
                    .username_from_url
                    .clone()
                    .unwrap_or_else(|| "git".into())
            });
        let secret = self.credential_secret.value.clone();
        let key_path = self.credential_key_path.value.trim().to_string();
        let passphrase = self.credential_passphrase.value.clone();
        let display_name = self
            .save_credential
            .then(|| optional_display_name(&self.credential_display_name.value))
            .flatten();

        let credential = if self.credential_form_mode == CredentialFormMode::Ssh {
            GitCredential::SshPassphrase {
                username,
                private_key_path: (!self.credential_use_ssh_agent && !key_path.is_empty())
                    .then_some(key_path),
                passphrase: (!passphrase.is_empty()).then_some(passphrase),
                display_name,
                save_to_keyring: self.save_credential,
                scope: self.credential_scope,
            }
        } else {
            GitCredential::UserPass {
                username,
                secret,
                display_name,
                save_to_keyring: self.save_credential,
                scope: self.credential_scope,
            }
        };

        if !send_credential_response(&pending, Ok(Some(credential))) {
            self.last_error = Some("凭据请求已失效".into());
            return;
        }
        self.show_next_credential_request();
        self.apply_status_event(pending.tab_id, |this| {
            this.status = "凭据已提交，正在继续操作".into();
            this.last_error = None;
        });
        self.reload_credential_records("凭据已提交");
        self.save_remote_credential_bindings();
    }

    pub(crate) fn cancel_credential_request(&mut self) {
        let Some(pending) = self.pending_credential.clone() else {
            return;
        };
        let _ = send_credential_response(
            &pending,
            Err(khaslana::GitError::Credential("已取消凭据输入".into())),
        );
        self.show_next_credential_request();
        self.apply_status_event(pending.tab_id, |this| {
            this.status = "凭据输入已取消".into();
            this.last_error = None;
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
