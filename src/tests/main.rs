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
        TabCredentialProvider::new(store, storage, bindings, tx, RepoTabId(7)),
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
    }
}

fn make_sample_diff(lines: Vec<khaslana::DiffLine>) -> FileDiff {
    FileDiff {
        path: "a.txt".into(),
        scope: DiffScope::Unstaged,
        is_binary: false,
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
    assert!(column_splitter_accepts_mouse_events(false));
    assert!(!column_splitter_accepts_mouse_events(true));
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
fn update_check_result_stays_in_status_bar_without_toast() {
    assert_eq!(update_check_toast_message("当前已是最新版本"), None);
    assert_eq!(update_check_toast_message("更新检查失败"), None);
    assert_eq!(update_check_toast_message(""), None);
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

    sync_conflict_state_from_paths(
        &mut mode,
        &mut state,
        &["conflict.txt".into()],
        true,
    );

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
