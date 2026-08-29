use super::*;
use khaslana::MemoryCredentialStore;

fn credential_request(operation_id: Option<u64>) -> CredentialRequest {
    CredentialRequest {
        url: "https://gitee.com/team/repo.git".into(),
        username_from_url: None,
        allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT,
        repo_path: Some(PathBuf::from("C:/work/repo")),
        remote_name: Some("origin".into()),
        operation_id,
    }
}

fn host_credential(secret: &str) -> GitCredential {
    GitCredential::UserPass {
        username: "user@example.com".into(),
        secret: secret.into(),
        display_name: Some("gitee".into()),
        save_to_keyring: true,
        scope: CredentialScope::Host,
    }
}

#[test]
fn repository_file_path_joins_repo_root_and_git_relative_path() {
    let repo_path = if cfg!(windows) {
        PathBuf::from(r"C:\work\repo")
    } else {
        PathBuf::from("/work/repo")
    };

    let absolute_path = repository_file_absolute_path(&repo_path, "src/main.rs");

    assert!(absolute_path.is_absolute());
    assert_eq!(absolute_path, repo_path.join("src").join("main.rs"));
}

fn credential_provider_with_store(
    store: Arc<MemoryCredentialStore>,
    bindings: Arc<Mutex<RemoteCredentialBindings>>,
) -> (TabCredentialProvider, Receiver<UiEvent>) {
    let (tx, rx) = async_channel::unbounded();
    let storage = Arc::new(khaslana::AppStorage::open_in_memory().unwrap());
    (
        TabCredentialProvider::new(
            store,
            storage,
            bindings,
            tx,
            RepoTabId(7),
            NetworkProxySettings::default(),
        ),
        rx,
    )
}

fn save_host_credential(store: &MemoryCredentialStore) -> String {
    let record = store
        .save_record(&credential_request(None), &host_credential("token"))
        .unwrap();
    record.id
}

fn expect_credential_prompt_cancelled(rx: &Receiver<UiEvent>) {
    let event = rx.recv_blocking().expect("credential prompt requested");
    match event {
        UiEvent::CredentialRequested { response_tx, .. } => {
            let tx = response_tx
                .lock()
                .unwrap()
                .take()
                .expect("credential response channel");
            tx.send(Err(khaslana::GitError::Credential(
                "测试取消凭据输入".into(),
            )))
            .unwrap();
        }
        _ => panic!("expected credential request"),
    }
}

fn make_diff_line(kind: DiffLineKind, content: &str) -> khaslana::DiffLine {
    khaslana::DiffLine {
        kind,
        old_lineno: None,
        new_lineno: None,
        content: content.into(),
        hunk_index: 0,
    }
}

fn make_sample_diff(lines: Vec<khaslana::DiffLine>) -> FileDiff {
    FileDiff {
        path: "a.txt".into(),
        scope: DiffScope::Unstaged,
        is_binary: false,
        untracked: false,
        old_size: None,
        new_size: None,
        encoding: khaslana::DiffEncodingInfo {
            requested: DiffEncodingChoice::Auto,
            resolved: DiffEncodingChoice::Utf8,
            lossy: false,
        },
        lines,
    }
}

#[test]
fn display_columns_counts_ascii_and_wide_chars() {
    assert_eq!(display_columns(""), 0);
    assert_eq!(display_columns("abc"), 3);
    // 中日韩等非 ASCII 字符按 2 列计
    assert_eq!(display_columns("中a文"), 5);
    assert_eq!(display_columns("你好"), 4);
}

#[test]
fn widest_diff_row_index_picks_the_longest_line() {
    let diff = make_sample_diff(vec![
        make_diff_line(DiffLineKind::Context, "short"),
        make_diff_line(
            DiffLineKind::Added,
            "this is a much longer line than the others",
        ),
        make_diff_line(DiffLineKind::Removed, "mid length"),
    ]);
    let model = diff_render_model_for(Some(&diff), false);
    // 无 header，行号一一对应，最宽行是第 1 行（索引 1）
    assert_eq!(widest_diff_row_index(Some(&diff), &model), Some(1));
}

#[test]
fn widest_diff_row_index_returns_none_without_diff() {
    let model = diff_render_model_for(None, false);
    assert_eq!(widest_diff_row_index(None, &model), None);
}

#[test]
fn widest_diff_row_index_prefers_wide_cjk_line() {
    // 6 个中文字符 = 12 列，多于 8 个 ASCII = 8 列
    let diff = make_sample_diff(vec![
        make_diff_line(DiffLineKind::Context, "abcdefgh"),
        make_diff_line(DiffLineKind::Added, "你好你好你好"),
    ]);
    let model = diff_render_model_for(Some(&diff), false);
    assert_eq!(widest_diff_row_index(Some(&diff), &model), Some(1));
}

#[test]
fn widest_diff_row_index_skips_collapsed_headers() {
    // 折叠头部时，header 行映射为 HeaderToggle，不参与宽度测量
    let diff = make_sample_diff(vec![
        make_diff_line(DiffLineKind::Header, "diff --git a/x b/x"),
        make_diff_line(DiffLineKind::Context, "short"),
        make_diff_line(DiffLineKind::Added, "longer content line here"),
    ]);
    let model = diff_render_model_for(Some(&diff), false);
    // row0=HeaderToggle，row1=short，row2=longer content line here
    assert_eq!(widest_diff_row_index(Some(&diff), &model), Some(2));
}

#[test]
fn diff_render_rows_keep_first_hunk_header_in_body() {
    // 第一个 @@ hunk 头紧跟文件头且同为 Header kind，但不属于可折叠的文件头：
    // 折叠时必须保留在正文中渲染，否则首个 hunk 的「暂存此块」入口被吞掉、行号跳变。
    let diff = test_diff(
        vec![
            test_line(DiffLineKind::Header, "diff --git a/file.txt b/file.txt"),
            test_line(DiffLineKind::Header, "index 0000000..1111111"),
            test_line(DiffLineKind::Header, "@@ -1,2 +1,3 @@"),
            test_line(DiffLineKind::Context, " ctx"),
            test_line(DiffLineKind::Header, "@@ -9,2 +10,3 @@"),
            test_line(DiffLineKind::Added, "+new"),
        ],
        false,
    );

    assert_eq!(
        diff_render_rows_for(Some(&diff), false),
        vec![
            DiffRenderRow::HeaderToggle,
            DiffRenderRow::DiffLine(2),
            DiffRenderRow::DiffLine(3),
            DiffRenderRow::DiffLine(4),
            DiffRenderRow::DiffLine(5),
        ]
    );
    assert_eq!(
        diff_render_rows_for(Some(&diff), true),
        vec![
            DiffRenderRow::HeaderToggle,
            DiffRenderRow::DiffLine(0),
            DiffRenderRow::DiffLine(1),
            DiffRenderRow::DiffLine(2),
            DiffRenderRow::DiffLine(3),
            DiffRenderRow::DiffLine(4),
            DiffRenderRow::DiffLine(5),
        ]
    );
}

#[test]
fn session_json_round_trips_multiple_repositories() {
    let state = SessionState {
        repo_paths: vec![PathBuf::from("C:/work/a"), PathBuf::from("C:/work/b")],
        active_repo_path: Some(PathBuf::from("C:/work/b")),
    };

    let json = serde_json::to_string(&state).expect("encode session");
    let decoded: SessionState = serde_json::from_str(&json).expect("decode session");

    assert_eq!(decoded.repo_paths, state.repo_paths);
    assert_eq!(decoded.active_repo_path, state.active_repo_path);
}

#[test]
fn session_paths_are_deduped_in_original_order() {
    let paths = dedupe_repo_paths(vec![
        PathBuf::from("C:/work/a"),
        PathBuf::from("C:/work/b"),
        PathBuf::from("C:/work/a"),
    ]);

    assert_eq!(
        paths,
        vec![PathBuf::from("C:/work/a"), PathBuf::from("C:/work/b")]
    );
}

#[test]
fn clone_dialog_defaults_to_recursive_submodules() {
    assert!(default_clone_recursive_submodules());
}

#[test]
fn change_list_indexes_keep_large_staged_and_unstaged_lists_separate() {
    let changes = (0..20_000)
        .map(|index| khaslana::WorktreeChange {
            path: format!("generated/file-{index}.txt"),
            staged: (index % 2 == 0).then_some(ChangeState::Added),
            unstaged: (index % 3 != 0).then_some(ChangeState::Modified),
        })
        .collect::<Vec<_>>();

    let indexes = ChangeListIndexes::rebuild(&changes);

    assert_eq!(indexes.for_scope(&DiffScope::Staged).len(), 10_000);
    assert_eq!(indexes.for_scope(&DiffScope::Unstaged).len(), 13_333);
    assert_eq!(indexes.for_scope(&DiffScope::Staged)[..3], [0, 2, 4]);
    assert_eq!(indexes.for_scope(&DiffScope::Unstaged)[..3], [1, 2, 4]);
}

#[test]
fn repo_switcher_menu_anchors_below_trigger_button() {
    // 菜单水平对齐按钮左缘、垂直紧贴按钮下方。
    let anchor = RepoSwitcherAnchor {
        x: 120.0,
        y: 8.0,
        w: 140.0,
        h: 32.0,
    };
    assert_eq!(
        repo_switcher_menu_origin(&anchor, 1280.0, 720.0),
        (120.0, 40.0)
    );
    // 按钮靠近右缘时菜单左缘被钳制，避免溢出视口。
    let right_anchor = RepoSwitcherAnchor {
        x: 1200.0,
        y: 8.0,
        w: 140.0,
        h: 32.0,
    };
    let (x, y) = repo_switcher_menu_origin(&right_anchor, 1280.0, 720.0);
    assert_eq!(x, 1280.0 - REPO_SWITCHER_MENU_WIDTH - MENU_VIEWPORT_MARGIN);
    assert_eq!(y, 40.0);
}

#[test]
fn repo_switcher_hit_test_covers_menu_and_trigger() {
    let menu = RepoSwitcherMenu { x: 120.0, y: 40.0 };
    let anchor = RepoSwitcherAnchor {
        x: 120.0,
        y: 8.0,
        w: 140.0,
        h: 32.0,
    };
    // 菜单内、触发器按钮内均命中；二者之外不命中。
    assert!(point_in_repo_switcher(150.0, 60.0, &menu, Some(&anchor)));
    assert!(point_in_repo_switcher(180.0, 20.0, &menu, Some(&anchor)));
    // 菜单宽 320（x∈[120,440]）、高 480（y∈[40,520]）；500 在菜单与按钮右侧之外。
    assert!(!point_in_repo_switcher(500.0, 300.0, &menu, Some(&anchor)));
    // 四周 4px 容差：菜单左缘常与分栏分割线重合，边缘上的点击视为菜单内部
    assert!(point_in_repo_switcher(117.5, 60.0, &menu, Some(&anchor))); // 左缘内 2.5px
    assert!(point_in_repo_switcher(442.0, 60.0, &menu, Some(&anchor))); // 右缘外 2px
    assert!(!point_in_repo_switcher(114.0, 60.0, &menu, Some(&anchor))); // 容差之外
}

#[test]
fn context_navigator_preferences_are_shared_across_modes() {
    let mut preferences = ContextNavigatorPreferences::default();
    assert!(preferences.is_visible(MainMode::Worktree));
    assert!(preferences.is_visible(MainMode::History));
    assert!(preferences.is_visible(MainMode::Workflow));

    // 任一主模式收起后，切换到其它主模式保持收起，不随页面切换回弹。
    preferences.toggle(MainMode::Worktree);
    assert!(!preferences.is_visible(MainMode::Worktree));
    assert!(!preferences.is_visible(MainMode::History));
    assert!(!preferences.is_visible(MainMode::Workflow));

    // 在历史页重新展开，工作区/工作流同样保持展开。
    preferences.toggle(MainMode::History);
    assert!(preferences.is_visible(MainMode::Worktree));
    assert!(preferences.is_visible(MainMode::History));
    assert!(preferences.is_visible(MainMode::Workflow));

    // 专用模式不承载 Navigator，也不能误改共享的展开状态。
    preferences.toggle(MainMode::Conflict);
    assert!(!preferences.is_visible(MainMode::Conflict));
    assert!(preferences.is_visible(MainMode::Worktree));
    assert!(preferences.is_visible(MainMode::History));
    assert!(preferences.is_visible(MainMode::Workflow));

    // 提交图谱页同为专用模式：不承载 Navigator、不改共享展开状态。
    assert!(!preferences.is_visible(MainMode::CommitGraph));
    preferences.toggle(MainMode::CommitGraph);
    assert!(preferences.is_visible(MainMode::History));
}

// 图谱页跳转无损往返：主历史页 ↔ 图谱页只切换 main_mode，
// 高亮分支、开关与详情卡折叠状态必须完整保留（close_commit_graph 不重置）。
#[test]
fn commit_graph_state_survives_mode_round_trip() {
    let mut tab = RepoTabState::new(RepoTabId(0), None);
    tab.commit_graph.highlight_branch = Some("feature".to_string());
    tab.commit_graph.highlight_ahead_only = true;
    tab.commit_graph.dim_merges = true;
    tab.commit_graph.details_collapsed = true;
    tab.main_mode = MainMode::History;

    // 模拟「打开图谱页 → 跳回提交记录页 → 再返回图谱页」。
    tab.main_mode = MainMode::CommitGraph;
    tab.main_mode = MainMode::History;
    tab.main_mode = MainMode::CommitGraph;

    assert_eq!(
        tab.commit_graph.highlight_branch.as_deref(),
        Some("feature")
    );
    assert!(tab.commit_graph.highlight_ahead_only);
    assert!(tab.commit_graph.dim_merges);
    assert!(tab.commit_graph.details_collapsed);
}

// 切换/打开/克隆仓库保持当前区域：主模式跟随切换带到目标 tab；
// 专用模式（绑定 per-repo 状态）不继承，落回目标 tab 自身模式（新 tab 即默认工作区）。
#[test]
fn inheritable_main_mode_carries_primary_modes_only() {
    // 主模式继承。
    for mode in [
        MainMode::Worktree,
        MainMode::History,
        MainMode::Workflow,
        MainMode::CommitGraph,
    ] {
        assert_eq!(inheritable_main_mode(Some(mode)), Some(mode));
    }

    // 专用模式不继承。
    for mode in [
        MainMode::Conflict,
        MainMode::Stash,
        MainMode::Browse,
        MainMode::Blame,
    ] {
        assert_eq!(inheritable_main_mode(Some(mode)), None);
    }

    // 无前序 tab（首启/会话恢复首个仓库）：不继承。
    assert_eq!(inheritable_main_mode(None), None);
}

// 布局偏好恢复的钳制不变量：每个默认常量必须落在自己的 MIN/MAX 区间内，
// 否则启动恢复时 clamp 会把默认值挤歪（布局跳变）。
#[test]
fn layout_preference_clamp_ranges_cover_defaults() {
    assert!(
        (MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(&DEFAULT_SIDEBAR_WIDTH)
            && (MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(&DEFAULT_CHANGES_WIDTH)
    );
    assert!(
        (MIN_HISTORY_FILES_WIDTH..=MAX_HISTORY_FILES_WIDTH).contains(&DEFAULT_HISTORY_FILES_WIDTH)
    );
    assert!(
        (MIN_HISTORY_INSPECTOR_FILES_WIDTH..=MAX_HISTORY_INSPECTOR_FILES_WIDTH)
            .contains(&DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH)
    );
    assert!(
        (MIN_WORKFLOW_TEMPLATES_WIDTH..=MAX_WORKFLOW_TEMPLATES_WIDTH)
            .contains(&DEFAULT_WORKFLOW_TEMPLATES_WIDTH)
    );
    assert!((MIN_BROWSE_TREE_WIDTH..=MAX_BROWSE_TREE_WIDTH).contains(&DEFAULT_BROWSE_TREE_WIDTH));
    assert!(
        (MIN_HISTORY_GRAPH_WIDTH..=MAX_HISTORY_GRAPH_WIDTH).contains(&DEFAULT_HISTORY_GRAPH_WIDTH)
    );
    assert!(
        (MIN_HISTORY_DETAILS_HEIGHT..=MAX_HISTORY_DETAILS_HEIGHT)
            .contains(&DEFAULT_HISTORY_DETAILS_HEIGHT)
    );
}

// 设置中心分类完整性：「关于」页承载版本号与版本说明，标签不得与其他分类重复。
#[test]
fn settings_categories_include_about_with_unique_labels() {
    let categories = [
        (SettingsCategory::Credentials, "凭据管理"),
        (SettingsCategory::Proxy, "网络代理"),
        (SettingsCategory::Ai, "AI 设置"),
        (SettingsCategory::ExternalMerge, "合并工具"),
        (SettingsCategory::Theme, "外观"),
        (SettingsCategory::Update, "更新设置"),
        (SettingsCategory::Shortcuts, "快捷键"),
        (SettingsCategory::About, "关于"),
    ];
    let mut labels: Vec<&str> = categories.iter().map(|(_, label)| *label).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), categories.len(), "设置分类标签必须唯一");
    assert!(
        categories
            .iter()
            .any(|(category, _)| matches!(category, SettingsCategory::About))
    );
}

#[test]
fn stale_submodule_requests_do_not_match_current_state() {
    let mut state = SubmoduleDialogState::default();
    state.request_id = 8;

    assert!(submodule_request_matches(&state, 3, 3, 8));
    assert!(!submodule_request_matches(&state, 3, 2, 8));
    assert!(!submodule_request_matches(&state, 3, 3, 7));
}

#[test]
fn stale_submodule_remote_status_requests_do_not_match_current_state() {
    let mut state = SubmoduleDialogState::default();
    state.remote_request_id = 12;

    assert!(submodule_remote_request_matches(&state, 3, 3, 12));
    assert!(!submodule_remote_request_matches(&state, 3, 2, 12));
    assert!(!submodule_remote_request_matches(&state, 3, 3, 11));
}

#[test]
fn submodule_dialog_refreshes_after_all_update_modes() {
    assert!(operation_refreshes_submodule_dialog("子模块已同步记录版本"));
    assert!(operation_refreshes_submodule_dialog(
        "子模块已更新到远端最新"
    ));
    assert!(operation_refreshes_submodule_dialog(
        "子模块 deps/core 已更新到远端最新"
    ));
    assert!(!operation_refreshes_submodule_dialog("已获取 origin"));
}

#[test]
fn column_splitter_mouse_events_are_blocked_while_dialog_is_open() {
    assert!(column_splitter_accepts_mouse_events(false, false));
    assert!(!column_splitter_accepts_mouse_events(true, false));
    // 弹层菜单（无遮罩）打开时同样不响应，避免抢走弹层边缘容差区的交互
    assert!(!column_splitter_accepts_mouse_events(false, true));
    assert!(!column_splitter_accepts_mouse_events(true, true));
}

#[test]
fn column_splitter_clears_active_resize_when_dialog_opens() {
    assert!(column_splitter_should_clear_resize(true, true));
    assert!(!column_splitter_should_clear_resize(true, false));
    assert!(!column_splitter_should_clear_resize(false, true));
}

#[test]
fn dialog_parent_only_stops_mouse_down() {
    assert!(dialog_parent_should_stop_mouse_event("mouse_down"));
    assert!(!dialog_parent_should_stop_mouse_event("mouse_move"));
    assert!(!dialog_parent_should_stop_mouse_event("mouse_up"));
    assert!(!dialog_parent_should_stop_mouse_event("mouse_up_out"));
}

#[test]
fn submodule_state_labels_cover_common_states() {
    let ready = khaslana::SubmoduleState {
        initialized: true,
        checked_out: true,
        head_matches_index: true,
        workdir_modified: false,
        workdir_untracked: false,
    };
    let dirty = khaslana::SubmoduleState {
        workdir_modified: true,
        ..ready.clone()
    };
    let missing = khaslana::SubmoduleState {
        initialized: false,
        checked_out: false,
        head_matches_index: false,
        workdir_modified: false,
        workdir_untracked: false,
    };

    assert_eq!(ready.label(), "已同步");
    assert_eq!(dirty.label(), "有改动");
    assert_eq!(missing.label(), "未初始化");
}

#[test]
fn worktree_diff_load_completion_does_not_emit_toast() {
    assert!(!should_notify_operation_finished("差异已加载", false, true));
    assert!(should_notify_operation_finished("差异已加载", true, true));
    assert!(should_notify_operation_finished("拉取完成", true, false));
    assert!(should_notify_operation_finished(
        "提交已还原到暂存区",
        true,
        false
    ));
}

#[test]
fn update_check_toast_only_for_manual_checks() {
    // 手动检查：已是最新 → 成功气泡；真失败 → 错误气泡（带前缀）
    assert_eq!(
        update_check_toast("当前已是最新版本", true),
        Some((AppToastKind::Success, "当前已是最新版本".to_string()))
    );
    assert_eq!(
        update_check_toast("清单下载失败", true),
        Some((
            AppToastKind::Error,
            "检查更新失败：清单下载失败".to_string()
        ))
    );
    // 手动但静默：跳过版本（空 error）
    assert_eq!(update_check_toast("", true), None);
    // 自动检查：一律不弹（每次启动都弹会打扰）
    assert_eq!(update_check_toast("当前已是最新版本", false), None);
    assert_eq!(update_check_toast("清单下载失败", false), None);
}

#[test]
fn branch_reference_operations_require_repository_refresh() {
    for message in [
        "切换分支完成",
        "远端分支已拉取到本地",
        "分支已创建",
        "分支已重命名",
        "分支已删除",
        "拉取远程引用完成",
        "拉取完成",
        "变基拉取完成",
        "分支拉取完成",
        "推送完成",
        "upstream 已设置",
    ] {
        assert!(
            operation_requires_repository_refresh(message),
            "{message} 应触发仓库刷新"
        );
    }
}

#[test]
fn ordinary_worktree_operations_do_not_require_repository_refresh() {
    for message in ["暂存完成", "取消暂存完成", "差异已加载", "应用贮藏完成"] {
        assert!(
            !operation_requires_repository_refresh(message),
            "{message} 不应触发仓库刷新"
        );
    }
}

#[test]
fn commit_creating_operations_affect_commit_history() {
    for message in [
        "提交完成",
        "提交并推送完成",
        "合并操作已完成",
        "合并已完成",
        "合并已中止",
        "变基完成",
        "变基已中止",
        "分支已重置",
        "回滚提交完成",
        "撤销合并完成",
        "提交已还原到暂存区",
    ] {
        assert!(
            operation_affects_commit_history(message),
            "{message} 应触发提交历史后台刷新"
        );
    }
}

#[test]
fn history_untouched_operations_do_not_affect_commit_history() {
    // 不改提交历史的操作不触发后台刷新
    for message in [
        "暂存完成",
        "取消暂存完成",
        "差异已加载",
        "已贮藏当前修改",
        "应用贮藏完成",
    ] {
        assert!(
            !operation_affects_commit_history(message),
            "{message} 不应触发提交历史后台刷新"
        );
    }
}

#[test]
fn commit_history_and_repository_refresh_message_lists_are_disjoint() {
    // 两个名单互不重叠：引用类操作走完整仓库重载（RepositoryFastLoaded 统一刷历史），
    // 提交/HEAD 类操作走 OperationFinished 直接刷历史。
    for message in [
        "提交完成",
        "提交并推送完成",
        "合并操作已完成",
        "合并已完成",
        "合并已中止",
        "变基完成",
        "变基已中止",
        "分支已重置",
        "回滚提交完成",
        "撤销合并完成",
        "提交已还原到暂存区",
    ] {
        assert!(
            !operation_requires_repository_refresh(message),
            "{message} 不应同时出现在仓库刷新名单中"
        );
    }
    for message in ["切换分支完成", "拉取完成", "推送完成", "upstream 已设置"] {
        assert!(
            !operation_affects_commit_history(message),
            "{message} 不应同时出现在提交历史刷新名单中"
        );
    }
}

#[test]
fn context_menu_position_opens_from_cursor_when_space_allows() {
    assert_eq!(
        context_menu_position(120.0, 160.0, 800.0, 600.0, 170.0, 110.0),
        (120.0, 160.0)
    );
}

#[test]
fn context_menu_position_flips_left_near_right_edge() {
    assert_eq!(
        context_menu_position(760.0, 160.0, 800.0, 600.0, 170.0, 110.0),
        (590.0, 160.0)
    );
}

#[test]
fn context_menu_position_clamps_to_bottom_near_bottom_edge() {
    assert_eq!(
        context_menu_position(120.0, 570.0, 800.0, 600.0, 170.0, 110.0),
        (120.0, 482.0)
    );
}

#[test]
fn context_menu_position_flips_left_and_clamps_bottom_near_bottom_right() {
    assert_eq!(
        context_menu_position(790.0, 590.0, 800.0, 600.0, 170.0, 110.0),
        (620.0, 482.0)
    );
}

#[test]
fn context_menu_position_uses_viewport_bounds_for_bottom_clamp() {
    assert_eq!(
        context_menu_position(280.0, 510.0, 900.0, 540.0, 170.0, 110.0),
        (280.0, 422.0)
    );
}

#[test]
fn diff_encoding_preferences_round_trip() {
    let mut preferences = DiffEncodingPreferences::default();
    preferences
        .repositories
        .insert("c:/work/a".to_string(), DiffEncodingChoice::Gb18030);
    preferences
        .repositories
        .insert("c:/work/b".to_string(), DiffEncodingChoice::Big5);

    let json = serde_json::to_string(&preferences).expect("encode preferences");
    let decoded: DiffEncodingPreferences = serde_json::from_str(&json).expect("decode preferences");

    assert_eq!(
        decoded.repositories.get("c:/work/a"),
        Some(&DiffEncodingChoice::Gb18030)
    );
    assert_eq!(
        decoded.repositories.get("c:/work/b"),
        Some(&DiffEncodingChoice::Big5)
    );
    assert_eq!(DiffEncodingChoice::default(), DiffEncodingChoice::Auto);
}

#[test]
fn remote_credential_bindings_round_trip() {
    let bindings = RemoteCredentialBindings {
        remotes: vec![
            RemoteCredentialBinding {
                repo_path: "c:/work/a".to_string(),
                remote_name: "origin".to_string(),
                remote_url: "https://example.com/a.git".to_string(),
                policy: RemoteCredentialPolicy::NoCredential,
            },
            RemoteCredentialBinding {
                repo_path: "c:/work/b".to_string(),
                remote_name: "upstream".to_string(),
                remote_url: "git@example.com:b.git".to_string(),
                policy: RemoteCredentialPolicy::Record("record-1".to_string()),
            },
        ],
    };

    let json = serde_json::to_string(&bindings).expect("encode bindings");
    let decoded: RemoteCredentialBindings = serde_json::from_str(&json).expect("decode bindings");

    assert_eq!(decoded.remotes, bindings.remotes);
}

#[test]
fn remote_binding_for_request_defaults_to_auto_match() {
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    let request = CredentialRequest {
        url: "https://example.com/a.git".into(),
        username_from_url: None,
        allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT,
        repo_path: Some(PathBuf::from("C:/work/a")),
        remote_name: Some("origin".into()),
        operation_id: None,
    };

    assert_eq!(
        remote_binding_for_request(&bindings, &request),
        RemoteCredentialPolicy::AutoMatch
    );
}

#[test]
fn remote_binding_for_request_matches_repo_remote_and_url() {
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    let request = CredentialRequest {
        url: "https://example.com/a.git".into(),
        username_from_url: None,
        allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT,
        repo_path: Some(PathBuf::from("C:/work/a")),
        remote_name: Some("origin".into()),
        operation_id: None,
    };

    set_remote_binding_for_request(
        &bindings,
        &request,
        RemoteCredentialPolicy::Record("record-1".into()),
    );

    assert_eq!(
        remote_binding_for_request(&bindings, &request),
        RemoteCredentialPolicy::Record("record-1".into())
    );

    let changed_url = CredentialRequest {
        url: "https://example.com/renamed.git".into(),
        ..request
    };
    assert_eq!(
        remote_binding_for_request(&bindings, &changed_url),
        RemoteCredentialPolicy::AutoMatch
    );
}

#[test]
fn stored_host_credential_is_reused_across_workflow_remote_steps() {
    let store = Arc::new(MemoryCredentialStore::new());
    save_host_credential(&store);
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    let (provider, rx) = credential_provider_with_store(store.clone(), bindings);

    let first = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();
    let second = provider
        .credential_for(credential_request(Some(2)))
        .unwrap()
        .unwrap();

    assert_eq!(first.username(), "user@example.com");
    assert_eq!(second.username(), "user@example.com");
    assert!(rx.try_recv().is_err());
    assert_eq!(store.list_records().unwrap().len(), 1);
}

#[test]
fn stored_credential_is_reused_for_repeated_callbacks_in_same_remote_operation() {
    let store = Arc::new(MemoryCredentialStore::new());
    save_host_credential(&store);
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    let (provider, rx) = credential_provider_with_store(store, bindings);

    let first = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();
    let second = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();

    assert_eq!(first.username(), "user@example.com");
    assert_eq!(second.username(), "user@example.com");
    assert!(rx.try_recv().is_err());
}

#[test]
fn repeated_remote_operation_retry_rejects_last_stored_record_without_deleting_it() {
    let store = Arc::new(MemoryCredentialStore::new());
    let record_id = save_host_credential(&store);
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    let (provider, rx) = credential_provider_with_store(store.clone(), bindings);

    let first = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();
    assert_eq!(first.username(), "user@example.com");
    let second = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();
    assert_eq!(second.username(), "user@example.com");

    let retry = provider.clone();
    let handle = thread::spawn(move || retry.credential_for(credential_request(Some(1))));
    expect_credential_prompt_cancelled(&rx);
    assert!(handle.join().unwrap().is_err());
    assert!(store.credential_for_record(&record_id).unwrap().is_some());
}

#[test]
fn no_credential_binding_still_skips_saved_credentials_for_workflow() {
    let store = Arc::new(MemoryCredentialStore::new());
    save_host_credential(&store);
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    set_remote_binding_for_request(
        &bindings,
        &credential_request(Some(1)),
        RemoteCredentialPolicy::NoCredential,
    );
    let (provider, rx) = credential_provider_with_store(store, bindings);

    let handle = thread::spawn(move || provider.credential_for(credential_request(Some(1))));
    expect_credential_prompt_cancelled(&rx);
    assert!(handle.join().unwrap().is_err());
}

#[test]
fn record_binding_is_reused_across_workflow_remote_steps() {
    let store = Arc::new(MemoryCredentialStore::new());
    let record_id = save_host_credential(&store);
    let bindings = Arc::new(Mutex::new(RemoteCredentialBindings::default()));
    set_remote_binding_for_request(
        &bindings,
        &credential_request(Some(1)),
        RemoteCredentialPolicy::Record(record_id),
    );
    let (provider, rx) = credential_provider_with_store(store, bindings);

    let first = provider
        .credential_for(credential_request(Some(1)))
        .unwrap()
        .unwrap();
    let second = provider
        .credential_for(credential_request(Some(2)))
        .unwrap()
        .unwrap();

    assert_eq!(first.username(), "user@example.com");
    assert_eq!(second.username(), "user@example.com");
    assert!(rx.try_recv().is_err());
}

#[test]
fn clone_directory_name_is_inferred_from_remote_url() {
    assert_eq!(
        infer_clone_directory_name("https://github.com/FuturePrayer/khaslana.git"),
        Some("khaslana".to_string())
    );
    assert_eq!(
        infer_clone_directory_name("https://example.invalid/team/repo/"),
        Some("repo".to_string())
    );
    assert_eq!(
        infer_clone_directory_name("git@github.com:FuturePrayer/khaslana.git"),
        Some("khaslana".to_string())
    );
    assert_eq!(
        infer_clone_directory_name("https://example.invalid/team/repo.git?ref=main"),
        Some("repo".to_string())
    );
    assert_eq!(infer_clone_directory_name(""), None);
    assert_eq!(infer_clone_directory_name("https://example.invalid/"), None);
}

#[test]
fn clone_target_path_uses_selected_parent_directory() {
    assert_eq!(
        infer_clone_target_path("https://github.com/example/abc", "D:/dev"),
        Some(PathBuf::from("D:/dev").join("abc"))
    );
    assert_eq!(
        infer_clone_target_path("https://github.com/example/abc.git", "D:/dev/"),
        Some(PathBuf::from("D:/dev/").join("abc"))
    );
    assert_eq!(infer_clone_target_path("", "D:/dev"), None);
    assert_eq!(
        infer_clone_target_path("https://github.com/example/abc", ""),
        None
    );
}

#[test]
fn repo_tab_workflow_state_is_isolated_per_tab() {
    let mut left = RepoTabState::new(RepoTabId(1), Some(PathBuf::from("C:/repos/left")));
    let right = RepoTabState::new(RepoTabId(2), Some(PathBuf::from("C:/repos/right")));

    left.workflow_state.file_path =
        Some(PathBuf::from("C:/Users/test/.khaslana/workflows/a.json5"));
    left.workflow_state.selected_template_path =
        Some(PathBuf::from("C:/Users/test/.khaslana/workflows/a.json5"));
    left.workflow_state.log.push("left workflow".into());

    assert!(right.workflow_state.file_path.is_none());
    assert!(right.workflow_state.selected_template_path.is_none());
    assert!(right.workflow_state.log.is_empty());
}

fn sample_conflict_view(path: &str) -> ConflictFileView {
    ConflictFileView {
        path: path.to_string(),
        kind: ConflictFileKind::Text,
        draft: "main\n".to_string(),
        ours_text: "main\n".to_string(),
        theirs_text: "feature\n".to_string(),
        blocks: vec![khaslana::ConflictBlock {
            base: Some("base\n".to_string()),
            ours: "main\n".to_string(),
            theirs: "feature\n".to_string(),
            start: 0,
            end: 5,
            ours_start: 0,
            ours_end: 5,
            theirs_start: 0,
            theirs_end: 8,
            status: khaslana::ConflictBlockStatus::Unresolved,
            has_manual_edits: false,
        }],
        draft_status: khaslana::ConflictDraftStatus::Dirty,
        fallback_reason: None,
    }
}

#[test]
fn conflict_state_enters_worktree_and_selects_first_path() {
    let mut mode = MainMode::Worktree;
    let mut state = ConflictWorkbenchState::default();
    let paths = vec!["b.txt".to_string(), "a.txt".to_string()];

    sync_conflict_state_from_paths(&mut mode, &mut state, &paths, false);

    assert_eq!(mode, MainMode::Worktree);
    assert_eq!(state.selected_path.as_deref(), Some("b.txt"));
    assert_eq!(state.selected_block, 0);
}

#[test]
fn non_merge_conflict_state_still_opens_conflict_mode() {
    let mut mode = MainMode::History;
    let mut state = ConflictWorkbenchState::default();

    sync_conflict_state_from_paths(&mut mode, &mut state, &["conflict.txt".into()], true);

    assert_eq!(mode, MainMode::Conflict);
    assert_eq!(state.selected_path.as_deref(), Some("conflict.txt"));
}

#[test]
fn conflict_state_returns_to_worktree_when_last_conflict_disappears() {
    let mut mode = MainMode::Conflict;
    let mut state = ConflictWorkbenchState {
        selected_path: Some("a.txt".into()),
        selected_block: 1,
        show_base: true,
        pending_resolve: Some(PendingConflictResolve {
            path: "a.txt".into(),
            unresolved_count: 1,
        }),
        files: BTreeMap::from([(String::from("a.txt"), sample_conflict_view("a.txt"))]),
        external_merge_auto_opened: BTreeSet::new(),
        syntax: BTreeMap::new(),
    };

    sync_conflict_state_from_paths(&mut mode, &mut state, &[], true);

    assert_eq!(mode, MainMode::Worktree);
    assert!(state.selected_path.is_none());
    assert!(state.pending_resolve.is_none());
    assert!(state.files.is_empty());
}

#[test]
fn conflict_state_prunes_removed_files_and_keeps_existing_drafts() {
    let mut mode = MainMode::Conflict;
    let mut state = ConflictWorkbenchState {
        selected_path: Some("b.txt".into()),
        selected_block: 0,
        show_base: false,
        pending_resolve: Some(PendingConflictResolve {
            path: "a.txt".into(),
            unresolved_count: 1,
        }),
        files: BTreeMap::from([
            (String::from("a.txt"), sample_conflict_view("a.txt")),
            (String::from("b.txt"), sample_conflict_view("b.txt")),
        ]),
        external_merge_auto_opened: BTreeSet::new(),
        syntax: BTreeMap::new(),
    };

    sync_conflict_state_from_paths(&mut mode, &mut state, &["b.txt".into()], true);

    assert_eq!(mode, MainMode::Conflict);
    assert_eq!(state.selected_path.as_deref(), Some("b.txt"));
    assert_eq!(
        state.files.get("b.txt").map(|view| view.draft.as_str()),
        Some("main\n")
    );
    assert!(state.pending_resolve.is_none());
    assert!(!state.files.contains_key("a.txt"));
}

#[test]
fn conflict_state_tracks_auto_open_once_per_conflict_path() {
    let mut state = ConflictWorkbenchState::default();

    assert!(state.mark_external_merge_auto_opened("a.txt"));
    assert!(!state.mark_external_merge_auto_opened("a.txt"));
    assert!(state.mark_external_merge_auto_opened("b.txt"));

    state.prune_external_merge_auto_opened(&["b.txt".into()]);

    assert!(state.mark_external_merge_auto_opened("a.txt"));
    assert!(!state.mark_external_merge_auto_opened("b.txt"));
}

#[test]
fn conflict_state_requests_resolve_confirmation_only_for_unresolved_blocks() {
    let mut state = ConflictWorkbenchState::default();
    let unresolved = sample_conflict_view("a.txt");
    assert!(state.request_resolve_confirmation(
        unresolved.path.clone(),
        unresolved.unresolved_block_count()
    ));
    assert_eq!(
        state.pending_resolve,
        Some(PendingConflictResolve {
            path: "a.txt".into(),
            unresolved_count: 1,
        })
    );

    let mut resolved = sample_conflict_view("b.txt");
    resolved.blocks[0].status =
        khaslana::ConflictBlockStatus::Resolved(khaslana::ConflictBlockResolution::Ours);
    resolved.draft = "main\n".into();
    state.pending_resolve = None;

    assert!(
        !state
            .request_resolve_confirmation(resolved.path.clone(), resolved.unresolved_block_count())
    );
    assert!(state.pending_resolve.is_none());
}

#[test]
fn conflict_workbench_uses_distinct_scroll_handles_per_pane() {
    let handles = conflict_workbench_scroll_handle_ids();
    let unique = handles
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(unique.len(), 3);
}

#[test]
fn conflict_result_pane_uses_document_view_instead_of_editor() {
    assert!(!conflict_result_pane_uses_editor());
}

#[test]
fn conflict_editor_does_not_store_text_conflict_draft_when_result_is_document() {
    assert!(!conflict_editor_should_store_draft(ConflictFileKind::Text));
}

#[test]
fn conflict_editor_always_uses_scrollable_multiline_viewport() {
    assert!(multiline_input_should_scroll(
        FieldId::ConflictEditor,
        "short"
    ));
    assert!(!multiline_input_should_scroll(
        FieldId::CommitMessage,
        "short"
    ));
}

#[test]
fn conflict_editor_multiline_frame_expands_to_allow_scroll_viewport() {
    assert!(!multiline_input_uses_input_frame(FieldId::ConflictEditor));
    assert!(multiline_input_uses_input_frame(FieldId::CommitMessage));
}

fn test_diff(lines: Vec<khaslana::DiffLine>, is_binary: bool) -> FileDiff {
    FileDiff {
        path: "file.txt".to_string(),
        scope: DiffScope::Unstaged,
        is_binary,
        untracked: false,
        old_size: None,
        new_size: None,
        encoding: khaslana::DiffEncodingInfo {
            requested: DiffEncodingChoice::Utf8,
            resolved: DiffEncodingChoice::Utf8,
            lossy: false,
        },
        lines,
    }
}

fn test_line(kind: DiffLineKind, content: &str) -> khaslana::DiffLine {
    khaslana::DiffLine {
        kind,
        old_lineno: None,
        new_lineno: None,
        content: content.to_string(),
        hunk_index: 0,
    }
}

#[test]
fn diff_render_rows_track_headers_and_empty_states() {
    let diff = test_diff(
        vec![
            test_line(DiffLineKind::Header, "diff --git a/file.txt b/file.txt"),
            test_line(DiffLineKind::Header, "index 0000000..1111111"),
            test_line(DiffLineKind::Removed, "-old"),
            test_line(DiffLineKind::Added, "+new"),
        ],
        false,
    );

    assert_eq!(
        diff_render_rows_for(Some(&diff), false),
        vec![
            DiffRenderRow::HeaderToggle,
            DiffRenderRow::DiffLine(2),
            DiffRenderRow::DiffLine(3),
        ]
    );
    assert_eq!(
        diff_render_rows_for(Some(&diff), true),
        vec![
            DiffRenderRow::HeaderToggle,
            DiffRenderRow::DiffLine(0),
            DiffRenderRow::DiffLine(1),
            DiffRenderRow::DiffLine(2),
            DiffRenderRow::DiffLine(3),
        ]
    );

    let empty_text_diff = test_diff(Vec::new(), false);
    let empty_binary_diff = test_diff(Vec::new(), true);
    assert_eq!(
        diff_render_rows_for(Some(&empty_text_diff), false),
        vec![DiffRenderRow::Empty]
    );
    assert_eq!(
        diff_render_rows_for(Some(&empty_binary_diff), false),
        vec![DiffRenderRow::Empty]
    );
    assert_eq!(
        diff_render_rows_for(None, false),
        vec![DiffRenderRow::Empty]
    );
}

#[test]
fn format_byte_size_uses_binary_units_with_one_decimal() {
    assert_eq!(format_byte_size(0), "0 B");
    assert_eq!(format_byte_size(512), "512 B");
    assert_eq!(format_byte_size(1024), "1 KB");
    assert_eq!(format_byte_size(1024 + 512), "1.5 KB");
    assert_eq!(format_byte_size(1024 * 1024), "1 MB");
    assert_eq!(
        format_byte_size((1.2 * 1024.0 * 1024.0 * 1024.0) as u64),
        "1.2 GB"
    );
    assert_eq!(format_byte_size(3 * 1024 * 1024), "3 MB");
}

fn switcher_tab(key: &str, name: &str, last_active: i64, tab_id: u64) -> RepoSwitcherTabInput {
    RepoSwitcherTabInput {
        key: key.to_string(),
        name: name.to_string(),
        full_path: format!("C:/repos/{key}"),
        last_active,
        tab_id: RepoTabId(tab_id),
    }
}

fn switcher_recent(key: &str, name: &str, last_opened: i64) -> RepoSwitcherRecentInput {
    RepoSwitcherRecentInput {
        key: key.to_string(),
        name: name.to_string(),
        full_path: format!("C:/repos/{key}"),
        last_opened,
    }
}

#[test]
fn repo_switcher_sections_actions_fixed_order() {
    let sections = build_repo_switcher_sections(None, vec![], vec![]);
    assert_eq!(
        sections.actions,
        vec![RepoSwitcherAction::Clone, RepoSwitcherAction::Open]
    );
    assert!(sections.open.is_empty());
    assert!(sections.recent.is_empty());
}

#[test]
fn repo_switcher_sections_active_first_then_recent_activity() {
    // 活动仓库 c 置顶；其余按 last_active 倒序（b=20 先于 a=10）。
    let tabs = vec![
        switcher_tab("a", "alpha", 10, 1),
        switcher_tab("b", "beta", 20, 2),
        switcher_tab("c", "gamma", 5, 3),
    ];
    let sections = build_repo_switcher_sections(Some("c"), tabs, vec![]);

    assert_eq!(sections.open.len(), 3);
    assert_eq!(sections.open[0].path_key, "c");
    assert!(sections.open[0].active);
    assert_eq!(sections.open[0].tab_id, Some(RepoTabId(3)));
    assert_eq!(sections.open[1].path_key, "b");
    assert!(!sections.open[1].active);
    assert_eq!(sections.open[2].path_key, "a");
}

#[test]
fn repo_switcher_sections_recent_excludes_open_tabs() {
    // b 已打开，应只出现在 open 区，不出现在 recent 区。
    let tabs = vec![switcher_tab("b", "beta", 100, 2)];
    let recent = vec![
        switcher_recent("b", "beta", 90),
        switcher_recent("z", "zeta", 80),
    ];
    let sections = build_repo_switcher_sections(Some("b"), tabs, recent);

    assert_eq!(sections.open.len(), 1);
    assert_eq!(sections.open[0].path_key, "b");
    // recent 区只剩未打开的 z，且 tab_id 为 None。
    assert_eq!(sections.recent.len(), 1);
    assert_eq!(sections.recent[0].path_key, "z");
    assert_eq!(sections.recent[0].tab_id, None);
}

#[test]
fn repo_switcher_search_matches_name_and_path_case_insensitively() {
    let repo = |name: &str, full_path: &str| RepoSwitcherRepo {
        path_key: full_path.to_string(),
        name: name.to_string(),
        full_path: full_path.to_string(),
        tab_id: None,
        active: false,
    };
    // 名称匹配（大小写不敏感）
    assert!(repo_switcher_repo_matches_query(
        &repo("Khaslana", "C:/x/k"),
        "KHAS"
    ));
    // 完整路径子串匹配
    assert!(repo_switcher_repo_matches_query(
        &repo("other", "D:/devProjects/workplace/khaslana"),
        "workplace/khas"
    ));
    // 空白查询恒匹配
    assert!(repo_switcher_repo_matches_query(&repo("a", "C:/a"), ""));
    assert!(repo_switcher_repo_matches_query(&repo("a", "C:/a"), "   "));
    // 不匹配
    assert!(!repo_switcher_repo_matches_query(
        &repo("alpha", "C:/a"),
        "zzz"
    ));
}

#[test]
fn repo_switcher_filter_sections_filters_both_areas() {
    let tabs = vec![
        switcher_tab("khaslana", "khaslana", 20, 1),
        switcher_tab("other", "other", 10, 2),
    ];
    let recent = vec![
        switcher_recent("khaslana-broker", "khaslana-broker", 5),
        switcher_recent("unrelated", "unrelated", 4),
    ];
    let sections = build_repo_switcher_sections(None, tabs, recent);

    // 命中 khas：打开区 1 项、最近区 1 项，功能区保持不动
    let filtered = filter_repo_switcher_sections(sections.clone(), "khas");
    assert_eq!(filtered.open.len(), 1);
    assert_eq!(filtered.open[0].path_key, "khaslana");
    assert_eq!(filtered.recent.len(), 1);
    assert_eq!(filtered.recent[0].path_key, "khaslana-broker");
    assert_eq!(
        filtered.actions,
        vec![RepoSwitcherAction::Clone, RepoSwitcherAction::Open]
    );

    // 空查询原样返回
    let unfiltered = filter_repo_switcher_sections(sections.clone(), "  ");
    assert_eq!(unfiltered.open.len(), 2);
    assert_eq!(unfiltered.recent.len(), 2);

    // 无匹配清空两区
    let none = filter_repo_switcher_sections(sections, "zzz");
    assert!(none.open.is_empty());
    assert!(none.recent.is_empty());
}

#[test]
fn repo_switcher_filter_ranks_name_matches_before_path_matches() {
    // build 按最后活动排序：alpha(30) > khaslana(20) > repos-tool(10)；
    // query "repos" 三者路径都命中（C:/repos/...），其中仅 repos-tool 名称命中。
    let tabs = vec![
        switcher_tab("alpha", "alpha", 30, 1),
        switcher_tab("khaslana", "khaslana", 20, 2),
        switcher_tab("repos-tool", "repos-tool", 10, 3),
    ];
    let sections = build_repo_switcher_sections(None, tabs, vec![]);
    let filtered = filter_repo_switcher_sections(sections, "repos");

    // 名称命中的排在最前，仅路径命中的保持原有相对顺序跟在后面
    assert_eq!(filtered.open.len(), 3);
    assert_eq!(filtered.open[0].path_key, "repos-tool");
    assert_eq!(filtered.open[1].path_key, "alpha");
    assert_eq!(filtered.open[2].path_key, "khaslana");
}

// 仓库切换下拉的键盘导航（↑↓ 高亮 / Enter 确认 / Esc 关闭）已按键盘白名单
// 整体移除：下拉仅支持鼠标点击与搜索框文本过滤。

#[test]
fn stage_operations_refresh_worktree_diff() {
    // 整文件（含行内 +/- 按钮路径）与按块/按行部分暂存的消息都触发差异面板跟随刷新
    for message in [
        "暂存",
        "取消暂存",
        "已暂存选定文件",
        "已暂存所有文件",
        "已取消暂存选定文件",
        "已取消暂存所有文件",
        "已暂存选中改动",
        "已取消暂存选中改动",
    ] {
        assert!(operation_refreshes_worktree_diff(message), "{message}");
    }
    // 其它操作不触发（差异加载/刷新/提交各有自己的路径）
    for message in ["差异已加载", "已刷新", "提交完成", "拉取完成"] {
        assert!(!operation_refreshes_worktree_diff(message), "{message}");
    }
}

fn change_entry(
    path: &str,
    staged: Option<ChangeState>,
    unstaged: Option<ChangeState>,
) -> khaslana::WorktreeChange {
    khaslana::WorktreeChange {
        path: path.to_string(),
        staged,
        unstaged,
    }
}

#[test]
fn diff_scope_presence_detects_side_moves_and_untracked() {
    // 部分暂存后未暂存侧仍有改动：两侧都保留，原位重载
    let partially_staged = vec![change_entry(
        "a.rs",
        Some(ChangeState::Modified),
        Some(ChangeState::Modified),
    )];
    assert!(diff_scope_still_present(
        &partially_staged,
        "a.rs",
        &DiffScope::Unstaged
    ));
    assert!(diff_scope_still_present(
        &partially_staged,
        "a.rs",
        &DiffScope::Staged
    ));

    // 整文件暂存后：未暂存侧失效（清空差异面板），已暂存侧仍在
    let fully_staged = vec![change_entry("a.rs", Some(ChangeState::Modified), None)];
    assert!(!diff_scope_still_present(
        &fully_staged,
        "a.rs",
        &DiffScope::Unstaged
    ));
    assert!(diff_scope_still_present(
        &fully_staged,
        "a.rs",
        &DiffScope::Staged
    ));

    // 未跟踪文件不在 fast 快照中：未暂存侧视为仍存在，已暂存侧视为失效
    assert!(diff_scope_still_present(
        &[],
        "new.txt",
        &DiffScope::Unstaged
    ));
    assert!(!diff_scope_still_present(
        &[],
        "new.txt",
        &DiffScope::Staged
    ));

    // 路径仅存在于未暂存侧（其它文件不影响判定）：已暂存侧失效
    let others = vec![change_entry("b.rs", None, Some(ChangeState::Modified))];
    assert!(diff_scope_still_present(
        &others,
        "b.rs",
        &DiffScope::Unstaged
    ));
    assert!(!diff_scope_still_present(
        &others,
        "b.rs",
        &DiffScope::Staged
    ));
}

#[test]
fn diff_line_selection_plain_click_replaces_and_toggles_off() {
    let mut selection = BTreeSet::new();
    let mut anchor = None;
    toggle_index_selection(&mut selection, &mut anchor, 3, false, false);
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [3]);
    assert_eq!(anchor, Some(3));
    // 普通点击另一行：替换选择
    toggle_index_selection(&mut selection, &mut anchor, 7, false, false);
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [7]);
    // 再点同一行：取消选择
    toggle_index_selection(&mut selection, &mut anchor, 7, false, false);
    assert!(selection.is_empty());
}

#[test]
fn diff_line_selection_ctrl_click_toggles_multiple_lines() {
    let mut selection = BTreeSet::new();
    let mut anchor = None;
    toggle_index_selection(&mut selection, &mut anchor, 3, false, false);
    toggle_index_selection(&mut selection, &mut anchor, 8, true, false);
    toggle_index_selection(&mut selection, &mut anchor, 12, true, false);
    // Ctrl/Cmd 多选可同时选中多行，一次「暂存选中行(N)」全部生效
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [3, 8, 12]);
    // Ctrl 再点已选行：仅移除该行
    toggle_index_selection(&mut selection, &mut anchor, 8, true, false);
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [3, 12]);
}

#[test]
fn diff_line_selection_shift_click_selects_range() {
    let mut selection = BTreeSet::new();
    let mut anchor = None;
    toggle_index_selection(&mut selection, &mut anchor, 10, false, false);
    // Shift 向下范围选择：替换现有选择；范围内的上下文行索引保留，
    // 转换为部分暂存选择时只取 +/- 行
    toggle_index_selection(&mut selection, &mut anchor, 14, false, true);
    assert_eq!(
        selection.iter().copied().collect::<Vec<_>>(),
        [10, 11, 12, 13, 14]
    );
    // Shift 向上范围选择同样成立（锚点不变）
    toggle_index_selection(&mut selection, &mut anchor, 8, false, true);
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [8, 9, 10]);
    // 无锚点时 Shift 等价普通选择并记录锚点（不清空已有选择）
    let mut selection = BTreeSet::from([2]);
    let mut anchor = None;
    toggle_index_selection(&mut selection, &mut anchor, 5, false, true);
    assert_eq!(selection.iter().copied().collect::<Vec<_>>(), [2, 5]);
    assert_eq!(anchor, Some(5));
}

#[test]
fn dedicated_fields_registry_has_no_duplicates_and_covers_tag_inputs() {
    let ids: Vec<FieldId> = DEDICATED_FIELDS.iter().map(|(id, _)| *id).collect();
    // 无重复注册：find 永远命中第一条，重复会让后注册的字段收不到输入
    for (index, id) in ids.iter().enumerate() {
        assert!(
            !ids[..index].contains(id),
            "DEDICATED_FIELDS 存在重复注册：{id:?}"
        );
    }
    // 回归：创建标签弹窗的名称与附注输入框必须注册到聚焦字段清单，
    // 漏注册时字段可渲染但 EntityInputHandler 静默丢弃全部输入
    assert!(ids.contains(&FieldId::TagName), "TagName 未注册");
    assert!(ids.contains(&FieldId::TagMessage), "TagMessage 未注册");
}

// ===== 文件历史过滤（history_file_filter）=====

fn filter_test_commit(oid: &str) -> CommitInfo {
    CommitInfo {
        oid: oid.to_string(),
        short_oid: oid.to_string(),
        summary: oid.to_string(),
        message: oid.to_string(),
        author: "测试作者".to_string(),
        author_email: None,
        committer: "测试作者".to_string(),
        committer_email: None,
        time: 0,
        parents: Vec::new(),
        refs: Vec::new(),
    }
}

#[test]
fn history_file_filter_survives_clear_history() {
    // 过滤器是用户意图：clear_history（切 scope/刷新共用路径）清列表但保留过滤
    let mut tab = RepoTabState::new(RepoTabId(1), None);
    tab.history_file_filter = Some("src/lib.rs".into());
    tab.history_commits = vec![filter_test_commit("a"), filter_test_commit("b")];
    tab.history_has_more = true;
    tab.history_selected_commit = Some("a".into());

    tab.clear_history();

    assert_eq!(tab.history_file_filter.as_deref(), Some("src/lib.rs"));
    assert!(tab.history_commits.is_empty());
    assert!(!tab.history_has_more);
    assert!(tab.history_selected_commit.is_none());
}

#[test]
fn history_commits_event_guard_requires_matching_path_filter() {
    // scope + path_filter 双比较：切换过滤后，旧一代请求晚到的结果被丢弃
    let mut tab = RepoTabState::new(RepoTabId(1), None);
    tab.repository_load_id = 3;
    tab.history_scope = HistoryScope::CurrentBranch;
    tab.history_file_filter = Some("src/lib.rs".into());

    // 全部匹配
    assert!(tab.history_commits_event_matches(3, HistoryScope::CurrentBranch, Some("src/lib.rs")));

    // 过滤已被清除但旧请求仍带过滤器
    tab.history_file_filter = None;
    assert!(!tab.history_commits_event_matches(3, HistoryScope::CurrentBranch, Some("src/lib.rs")));

    // 新过滤器与旧请求路径不同
    tab.history_file_filter = Some("other.rs".into());
    assert!(!tab.history_commits_event_matches(3, HistoryScope::CurrentBranch, Some("src/lib.rs")));

    // load_id 或 scope 变化同样丢弃
    assert!(!tab.history_commits_event_matches(4, HistoryScope::CurrentBranch, Some("other.rs")));
    assert!(!tab.history_commits_event_matches(3, HistoryScope::AllRefs, Some("other.rs")));

    // 无过滤的普通请求在无过滤状态下正常落地
    tab.history_file_filter = None;
    assert!(tab.history_commits_event_matches(3, HistoryScope::CurrentBranch, None));
}

#[test]
fn preferred_history_file_favors_filter_path_when_present() {
    let files = vec![
        CommitFileChange {
            path: "src/other.rs".to_string(),
            old_path: None,
            status: ChangeState::Modified,
        },
        CommitFileChange {
            path: "src/lib.rs".to_string(),
            old_path: None,
            status: ChangeState::Modified,
        },
    ];

    // 过滤路径在列表中：优先选中，提交差异立即可见
    assert_eq!(
        preferred_history_file(Some("src/lib.rs"), &files),
        Some("src/lib.rs".to_string())
    );
    // 过滤路径不在列表中：回退到首个文件
    assert_eq!(
        preferred_history_file(Some("missing.rs"), &files),
        Some("src/other.rs".to_string())
    );
    // 无过滤：首个文件
    assert_eq!(
        preferred_history_file(None, &files),
        Some("src/other.rs".to_string())
    );
    // 空列表：None（调用方显示“该提交没有文件变更”）
    assert_eq!(preferred_history_file(Some("src/lib.rs"), &[]), None);
}

// 未跟踪文件差异的展示行类型：Added 映射为 Context 配色（白底），
// 其余 kind 与普通 diff 一致；显示映射不影响服务层原始 kind。
#[test]
fn display_diff_line_kind_maps_untracked_added_to_context() {
    assert_eq!(
        display_diff_line_kind(DiffLineKind::Added, true),
        DiffLineKind::Context
    );
    assert_eq!(
        display_diff_line_kind(DiffLineKind::Added, false),
        DiffLineKind::Added
    );
    // 未跟踪文件没有 Removed 行；Context/Header 原样保留
    assert_eq!(
        display_diff_line_kind(DiffLineKind::Removed, true),
        DiffLineKind::Removed
    );
    assert_eq!(
        display_diff_line_kind(DiffLineKind::Context, true),
        DiffLineKind::Context
    );
    assert_eq!(
        display_diff_line_kind(DiffLineKind::Header, true),
        DiffLineKind::Header
    );
}

// Gitee 自动续期的记录门控：record.host 是 host_key 形态（协议 + 主机），
// 纯主机名 "gitee.com" 或其它平台不得命中。
#[test]
fn is_gitee_https_record_matches_host_key_form_only() {
    let mut record = khaslana::credentials::CredentialRecord {
        id: "rec".into(),
        display_name: None,
        scope: CredentialScope::Host,
        kind: khaslana::StoredCredentialKind::HttpsUserPass,
        host: "https://gitee.com".into(),
        remote_url: "https://gitee.com/team/repo.git".into(),
        username: "user".into(),
        key_path: None,
        created_at: 1,
        updated_at: 1,
        last_used: None,
    };
    assert!(TabCredentialProvider::is_gitee_https_record(&record));

    record.host = "gitee.com".into();
    assert!(
        !TabCredentialProvider::is_gitee_https_record(&record),
        "纯主机名形态不是存储格式，不应命中"
    );

    record.host = "https://github.com".into();
    assert!(!TabCredentialProvider::is_gitee_https_record(&record));

    record.host = "ssh://gitee.com".into();
    assert!(!TabCredentialProvider::is_gitee_https_record(&record));
}

// 凭据测试通过必须走气泡（与代理/AI 测试一致）：此前「凭据测试通过」不含
// 任何已收录关键词，只有状态栏小字。
#[test]
fn should_toast_completion_accepts_test_passed_messages() {
    assert!(RepositoryView::should_toast_completion("凭据测试通过"));
    assert!(RepositoryView::should_toast_completion("代理测试通过"));
    // 非完成语义的消息仍然不打扰
    assert!(!RepositoryView::should_toast_completion("正在测试凭据连接"));
    assert!(!RepositoryView::should_toast_completion(
        "已发现 3 个 SSH 私钥"
    ));
}

// 凭据测试地址校验矩阵：空/协议族/HTTPS 跨站点拦截、SSH 跨站点放行。
#[test]
fn validate_credential_test_url_matrix() {
    use khaslana::StoredCredentialKind as Kind;

    // 空/空白
    assert!(validate_credential_test_url(Kind::HttpsUserPass, "https://gitee.com", "").is_err());
    assert!(validate_credential_test_url(Kind::HttpsUserPass, "https://gitee.com", "   ").is_err());

    // HTTPS 记录：同站点任一仓库地址通过（含裸主机）
    assert!(
        validate_credential_test_url(
            Kind::HttpsUserPass,
            "https://gitee.com",
            "https://gitee.com"
        )
        .is_ok()
    );
    assert!(
        validate_credential_test_url(
            Kind::HttpsUserPass,
            "https://gitee.com",
            "https://gitee.com/user/repo.git"
        )
        .is_ok()
    );

    // HTTPS 记录：跨站点拦截（Gitee 令牌测 GitHub / 其它平台）
    let err = validate_credential_test_url(
        Kind::HttpsUserPass,
        "https://gitee.com",
        "https://github.com/user/repo.git",
    )
    .unwrap_err();
    assert!(
        err.contains("令牌不通用"),
        "拦截文案应说明令牌不通用：{err}"
    );
    assert!(
        err.contains("https://gitee.com"),
        "拦截文案应指明绑定站点：{err}"
    );

    // HTTPS 记录：SSH 地址属协议族错误
    assert!(
        validate_credential_test_url(
            Kind::HttpsUserPass,
            "https://gitee.com",
            "git@gitee.com:user/repo.git"
        )
        .is_err()
    );

    // SSH 记录：跨站点放行（私钥主机无关）
    assert!(
        validate_credential_test_url(
            Kind::SshKey,
            "ssh://gitee.com",
            "git@github.com:user/repo.git"
        )
        .is_ok()
    );
    // SSH 记录：http(s) 地址属协议族错误
    assert!(
        validate_credential_test_url(
            Kind::SshKey,
            "ssh://gitee.com",
            "https://gitee.com/user/repo.git"
        )
        .is_err()
    );
}

/// DEDICATED_FIELDS 注册表完整性：每个 FieldId 变体都必须注册——漏注册的
/// 字段经 field() 查找会 expect panic，即使侥幸渲染聚焦也会让
/// focused_field 扫不到、EntityInputHandler 静默丢弃全部输入。
#[test]
fn dedicated_fields_cover_all_field_ids() {
    use super::ALL_FIELD_IDS;
    assert_eq!(
        ALL_FIELD_IDS.len(),
        DEDICATED_FIELDS.len(),
        "ALL_FIELD_IDS 与 DEDICATED_FIELDS 数量不一致：新增 FieldId 时两处都要同步"
    );
    for id in ALL_FIELD_IDS {
        assert!(
            DEDICATED_FIELDS
                .iter()
                .any(|(registered, _)| registered == id),
            "FieldId {id:?} 未注册到 DEDICATED_FIELDS（渲染即 panic / 输入静默丢失）"
        );
    }
}
