use super::*;

fn temp_storage() -> (tempfile::TempDir, AppStorage) {
    let temp = tempfile::tempdir().unwrap();
    let storage = AppStorage::open(temp.path().join("app.sqlite3")).unwrap();
    (temp, storage)
}

#[test]
fn code_index_batch_is_atomic_on_mid_batch_failure() {
    let (_temp, storage) = temp_storage();
    storage.set_code_index_enabled("a", false).unwrap();
    storage.lock_conn().unwrap().execute_batch(
        "CREATE TRIGGER reject_index_preference BEFORE INSERT ON code_index_preferences
         WHEN NEW.repo_path = 'fail' BEGIN SELECT RAISE(ABORT, 'injected'); END;"
    ).unwrap();
    assert!(storage.set_code_index_enabled_batch(&["a".into(), "b".into(), "fail".into()], true).is_err());
    let prefs = storage.load_code_index_preferences().unwrap();
    assert_eq!(prefs.repositories.len(), 1);
    assert_eq!(prefs.repositories.get("a"), Some(&false));
    storage.set_code_index_enabled_batch(&["a".into(), "b".into()], true).unwrap();
    let prefs = storage.load_code_index_preferences().unwrap();
    assert_eq!(prefs.repositories.get("a"), Some(&true));
    assert_eq!(prefs.repositories.get("b"), Some(&true));
    storage.set_code_index_enabled_batch(&["b".into()], false).unwrap();
    let prefs = storage.load_code_index_preferences().unwrap();
    assert_eq!(prefs.repositories.get("a"), Some(&true));
    assert_eq!(prefs.repositories.get("b"), Some(&false));
}

#[test]
fn session_state_round_trip() {
    let (_temp, storage) = temp_storage();
    let state = SessionState {
        repo_paths: vec![PathBuf::from("C:/repo/a"), PathBuf::from("C:/repo/b")],
        active_repo_path: Some(PathBuf::from("C:/repo/b")),
    };
    storage.save_session_state(&state).unwrap();
    assert_eq!(storage.load_session_state().unwrap(), Some(state));
}

// 布局偏好：空库返回默认（全 None → UI 层使用内置默认常量）；save→load 恒等；
// 旧 payload 缺字段经 serde(default) 回默认而不报错。
#[test]
fn layout_preferences_default_and_round_trip() {
    let (_temp, storage) = temp_storage();
    assert_eq!(
        storage.load_layout_preferences().unwrap(),
        LayoutPreferences::default()
    );

    let preferences = LayoutPreferences {
        navigator_visible: Some(false),
        sidebar_width: Some(320.0),
        changes_width: Some(410.0),
        workflow_templates_width: Some(500.0),
        history_files_width: Some(760.0),
        history_inspector_files_width: Some(300.0),
        history_graph_width: Some(140.0),
        browse_tree_width: Some(420.0),
        history_details_height: Some(310.0),
        history_inspector_height: Some(360.0),
        history_details_collapsed: true,
    };
    storage.save_layout_preferences(&preferences).unwrap();
    assert_eq!(storage.load_layout_preferences().unwrap(), preferences);
}

// 工作流快捷键绑定：空库返回空 map；save→load 恒等；旧 payload 只有 keystroke
// 字段（无 background）经 serde(default) 回 false。
#[test]
fn workflow_shortcut_bindings_default_and_round_trip() {
    let (_temp, storage) = temp_storage();
    assert_eq!(
        storage.load_workflow_shortcut_bindings().unwrap(),
        WorkflowShortcutBindings::default()
    );

    let bindings = WorkflowShortcutBindings {
        bindings: BTreeMap::from([
            (
                "sync.json5".to_string(),
                WorkflowShortcutBinding {
                    keystroke: "ctrl-alt-1".to_string(),
                    background: false,
                },
            ),
            (
                "release.json5".to_string(),
                WorkflowShortcutBinding {
                    keystroke: "ctrl-alt-2".to_string(),
                    background: true,
                },
            ),
        ]),
    };
    storage.save_workflow_shortcut_bindings(&bindings).unwrap();
    assert_eq!(storage.load_workflow_shortcut_bindings().unwrap(), bindings);

    // 旧 payload（仅有 keystroke 的字符串值形态不兼容，这里模拟缺 background
    // 字段的对象形态）回退 background=false 而不报错。
    let conn = storage.lock_conn().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO workflow_shortcut_bindings (id, payload, updated_at) VALUES (1, ?1, 0)",
        params![r#"{"bindings":{"sync.json5":{"keystroke":"ctrl-alt-1"}}}"#],
    )
    .unwrap();
    drop(conn);
    let loaded = storage.load_workflow_shortcut_bindings().unwrap();
    let binding = loaded.bindings.get("sync.json5").unwrap();
    assert_eq!(binding.keystroke, "ctrl-alt-1");
    assert!(!binding.background);
}

#[test]
fn layout_preferences_tolerates_partial_legacy_payload() {
    let (_temp, storage) = temp_storage();
    // 模拟旧版本只存了部分字段的 payload：缺失字段回默认，不报错。
    let conn = storage.lock_conn().unwrap();
    conn.execute(
        "INSERT INTO layout_preferences (id, payload, updated_at) VALUES (1, ?1, 0)",
        params![r#"{"sidebar_width":300.0}"#],
    )
    .unwrap();
    drop(conn);

    let loaded = storage.load_layout_preferences().unwrap();
    assert_eq!(loaded.sidebar_width, Some(300.0));
    assert_eq!(loaded.navigator_visible, None);
    assert_eq!(loaded.history_details_height, None);
    assert!(!loaded.history_details_collapsed);
}

#[test]
fn proxy_settings_round_trip() {
    let (_temp, storage) = temp_storage();
    let settings = NetworkProxySettings {
        mode: NetworkProxyMode::Custom,
        custom: CustomProxySettings {
            http_proxy: "http://127.0.0.1:7890".into(),
            https_proxy: "https://127.0.0.1:7891".into(),
            socks5_proxy: "socks5h://127.0.0.1:7892".into(),
        },
    };
    storage.save_proxy_settings(&settings).unwrap();
    assert_eq!(storage.load_proxy_settings().unwrap(), settings);
}

#[test]
fn ai_provider_settings_round_trip() {
    let (_temp, storage) = temp_storage();
    // 空表默认值。
    assert_eq!(
        storage.load_ai_provider_settings().unwrap(),
        AiProviderSettings::default()
    );

    let mut settings = AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = "https://api.deepseek.com".into();
    settings.api_key = "sk-test-key".into();
    settings.model = "deepseek-chat".into();
    settings.temperature = 0.1;
    settings.max_tokens = 1200;
    settings.request_timeout_secs = 90;

    storage.save_ai_provider_settings(&settings).unwrap();
    let loaded = storage.load_ai_provider_settings().unwrap();
    assert_eq!(loaded.enabled, settings.enabled);
    assert_eq!(loaded.base_url, settings.base_url);
    assert_eq!(loaded.api_key, settings.api_key);
    assert_eq!(loaded.model, settings.model);
    assert_eq!(loaded, settings);
}

#[test]
fn ai_provider_settings_replace_overwrites_previous() {
    let (_temp, storage) = temp_storage();
    let mut settings = AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = "https://api.openai.com/v1".into();
    settings.api_key = "sk-first".into();
    settings.model = "gpt-4o-mini".into();
    storage.save_ai_provider_settings(&settings).unwrap();

    settings.api_key = "sk-second".into();
    settings.model = "gpt-4o".into();
    storage.save_ai_provider_settings(&settings).unwrap();

    let loaded = storage.load_ai_provider_settings().unwrap();
    assert_eq!(loaded.api_key, "sk-second");
    assert_eq!(loaded.model, "gpt-4o");
}

#[test]
fn external_merge_settings_round_trip() {
    let (_temp, storage) = temp_storage();
    assert_eq!(
        storage.load_external_merge_settings().unwrap(),
        ExternalMergeSettings::default()
    );

    let settings = ExternalMergeSettings {
        enabled: true,
        auto_open_intellij: true,
        intellij_path: "D:/Tools/idea64.exe".into(),
    };

    storage.save_external_merge_settings(&settings).unwrap();

    assert_eq!(storage.load_external_merge_settings().unwrap(), settings);
}

#[test]
fn remote_credential_bindings_round_trip() {
    let (_temp, storage) = temp_storage();
    let bindings = RemoteCredentialBindings {
        remotes: vec![
            RemoteCredentialBinding {
                repo_path: "C:/repo/a".into(),
                remote_name: "origin".into(),
                remote_url: "https://example.com/a.git".into(),
                policy: RemoteCredentialPolicy::AutoMatch,
            },
            RemoteCredentialBinding {
                repo_path: "C:/repo/b".into(),
                remote_name: "upstream".into(),
                remote_url: "git@example.com:b.git".into(),
                policy: RemoteCredentialPolicy::Record("abc".into()),
            },
        ],
    };
    storage.save_remote_credential_bindings(&bindings).unwrap();
    assert_eq!(
        storage.load_remote_credential_bindings().unwrap().remotes,
        bindings.remotes
    );
}

#[test]
fn ssh_host_pins_round_trip_without_overwriting_identity() {
    let (_temp, storage) = temp_storage();
    assert_eq!(storage.load_ssh_host_pin("example.com", 22).unwrap(), None);
    assert!(storage.insert_ssh_host_pin("example.com", 22, "ssh-ed25519", &[0, 1, 255]).unwrap());
    assert!(!storage.insert_ssh_host_pin("example.com", 22, "ssh-rsa", &[9]).unwrap());
    assert_eq!(storage.load_ssh_host_pin("example.com", 22).unwrap(), Some(("ssh-ed25519".into(), vec![0, 1, 255])));
    assert!(storage.insert_ssh_host_pin("example.com", 2222, "ssh-ed25519", &[2]).unwrap());
    assert_eq!(storage.list_ssh_host_pins().unwrap(), vec![
        ("example.com".into(), 22, "ssh-ed25519".into(), vec![0, 1, 255]),
        ("example.com".into(), 2222, "ssh-ed25519".into(), vec![2]),
    ]);
    storage.delete_ssh_host_pin("example.com", 22).unwrap();
    storage.delete_ssh_host_pin("example.com", 22).unwrap();
    assert_eq!(storage.load_ssh_host_pin("example.com", 22).unwrap(), None);
    assert!(storage.load_ssh_host_pin("example.com", 2222).unwrap().is_some());
}

#[test]
fn ssh_host_pin_atomic_insert_across_connections() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.sqlite3");
    let first = AppStorage::open(&path).unwrap();
    let second = AppStorage::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [first, second].into_iter().enumerate().map(|(index, storage)| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            storage.insert_ssh_host_pin("example.com", 22, "ssh-ed25519", &[index as u8]).unwrap()
        })
    }).collect();
    let inserted = handles.into_iter().map(|handle| usize::from(handle.join().unwrap())).sum::<usize>();
    assert_eq!(inserted, 1);
    assert_eq!(AppStorage::open(path).unwrap().list_ssh_host_pins().unwrap().len(), 1);
}

#[test]
fn credential_records_round_trip() {
    let (_temp, storage) = temp_storage();
    let records = vec![CredentialRecord {
        id: "id-1".into(),
        display_name: Some("GitHub".into()),
        scope: CredentialScope::Host,
        kind: StoredCredentialKind::SshKey,
        host: "ssh://github.com".into(),
        remote_url: "git@github.com:owner/repo.git".into(),
        username: "git".into(),
        key_path: Some("C:/Users/test/.ssh/id_ed25519".into()),
        managed_key_id: None,
        created_at: 1,
        updated_at: 2,
        last_used: Some(3),
    }];
    storage.save_credential_records(&records).unwrap();
    assert_eq!(storage.load_credential_records().unwrap(), records);
}

#[test]
fn update_preferences_default() {
    let (_temp, storage) = temp_storage();
    // 空表返回默认值：自动检查开、未跳过、只走正式版渠道。
    let prefs = storage.load_update_preferences().unwrap();
    assert!(prefs.auto_check);
    assert_eq!(prefs.skipped_version, None);
    assert!(!prefs.include_beta);
}

#[test]
fn update_preferences_round_trip() {
    let (_temp, storage) = temp_storage();
    let prefs = UpdatePreferences {
        auto_check: false,
        skipped_version: Some("1.1.0-beta.1".into()),
        include_beta: true,
    };
    storage.save_update_preferences(&prefs).unwrap();
    let loaded = storage.load_update_preferences().unwrap();
    assert_eq!(loaded, prefs);
}

#[test]
fn update_preferences_auto_check_toggle() {
    let (_temp, storage) = temp_storage();
    // 默认 auto_check = true。
    assert!(storage.load_update_preferences().unwrap().auto_check);

    let prefs = UpdatePreferences {
        auto_check: false,
        skipped_version: None,
        include_beta: false,
    };
    storage.save_update_preferences(&prefs).unwrap();
    assert!(!storage.load_update_preferences().unwrap().auto_check);
}

// 旧库的 update_preferences 没有 include_beta 列：打开时幂等补列，
// 已有行保持原值且 include_beta 默认 0（不改变旧用户的正式版渠道行为）。
#[test]
fn update_preferences_include_beta_column_added_to_legacy_database() {
    let (temp, storage) = temp_storage();
    {
        let conn = storage.lock_conn().unwrap();
        conn.execute("DROP TABLE update_preferences", []).unwrap();
        conn.execute(
            "CREATE TABLE update_preferences (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                auto_check INTEGER NOT NULL,
                skipped_version TEXT,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO update_preferences (id, auto_check, skipped_version, updated_at)
             VALUES (1, 0, '0.9.0', 1)",
            [],
        )
        .unwrap();
    }
    drop(storage);

    // 重新打开同一库文件触发 ensure_update_include_beta_column 迁移。
    let storage = AppStorage::open(temp.path().join("app.sqlite3")).unwrap();
    let prefs = storage.load_update_preferences().unwrap();
    assert!(!prefs.auto_check);
    assert_eq!(prefs.skipped_version.as_deref(), Some("0.9.0"));
    assert!(!prefs.include_beta);
}

#[test]
fn theme_mode_defaults_to_system_and_round_trips() {
    let (_temp, storage) = temp_storage();
    assert_eq!(storage.load_theme_mode().unwrap(), ThemeMode::System);

    storage.save_theme_mode(ThemeMode::Dark).unwrap();
    assert_eq!(storage.load_theme_mode().unwrap(), ThemeMode::Dark);

    storage.save_theme_mode(ThemeMode::Light).unwrap();
    assert_eq!(storage.load_theme_mode().unwrap(), ThemeMode::Light);
}

#[test]
fn theme_accent_defaults_to_zero_and_round_trips() {
    let (_temp, storage) = temp_storage();
    // 默认值 0（靛蓝）
    assert_eq!(storage.load_theme_accent().unwrap(), 0);

    storage.save_theme_accent(3).unwrap();
    assert_eq!(storage.load_theme_accent().unwrap(), 3);

    // 切换 mode 不应清掉 accent
    storage.save_theme_mode(ThemeMode::Dark).unwrap();
    assert_eq!(storage.load_theme_accent().unwrap(), 3);

    // 反之 accent 也不应清掉 mode
    storage.save_theme_accent(5).unwrap();
    assert_eq!(storage.load_theme_mode().unwrap(), ThemeMode::Dark);
}

#[test]
fn theme_accent_migration_adds_column_to_legacy_database() {
    // 模拟旧版数据库：theme_preferences 表没有 accent 列。
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("legacy.sqlite3");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE theme_preferences (id INTEGER PRIMARY KEY CHECK (id = 1), mode TEXT NOT NULL, updated_at INTEGER NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO theme_preferences (id, mode, updated_at) VALUES (1, 'dark', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
    }
    // 重新打开：initialize_schema 应幂等补上 accent 列
    let storage = AppStorage::open(&db_path).unwrap();
    // 迁移后旧记录的 accent 默认为 0，mode 保持 dark
    assert_eq!(storage.load_theme_accent().unwrap(), 0);
    assert_eq!(storage.load_theme_mode().unwrap(), ThemeMode::Dark);
    // 能正常读写 accent
    storage.save_theme_accent(7).unwrap();
    assert_eq!(storage.load_theme_accent().unwrap(), 7);
    // 再次打开仍然幂等（不会重复加列报错）
    drop(storage);
    let storage2 = AppStorage::open(&db_path).unwrap();
    assert_eq!(storage2.load_theme_accent().unwrap(), 7);
}

// 同一仓库重复记录只保留一行，且最后打开时间被刷新。
#[test]
fn recent_repo_upsert_dedups_same_path() {
    let (_temp, storage) = temp_storage();
    storage.upsert_recent_repo(Path::new("C:/repo/a")).unwrap();
    storage.upsert_recent_repo(Path::new("C:/repo/a")).unwrap();

    let recent = storage.load_recent_repos().unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].0, PathBuf::from("C:/repo/a"));
}

// 最近仓库按最后打开时间倒序返回；跨秒 upsert 保证顺序确定。
#[test]
fn recent_repo_load_orders_by_last_opened_desc() {
    let (_temp, storage) = temp_storage();
    storage
        .upsert_recent_repo(Path::new("C:/repo/old"))
        .unwrap();
    // now_seconds 为秒级精度，sleep 跨秒以保证 old 的时间戳严格早于 new。
    std::thread::sleep(std::time::Duration::from_millis(1100));
    storage
        .upsert_recent_repo(Path::new("C:/repo/new"))
        .unwrap();

    let recent = storage.load_recent_repos().unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].0, PathBuf::from("C:/repo/new"));
    assert_eq!(recent[1].0, PathBuf::from("C:/repo/old"));
}

// 旧目录存在「已迁移」标记时，即使旧库文件仍在，也强制走便携路径。
#[test]
fn pick_active_path_prefers_portable_when_migrated_marker_present() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    let portable_dir = tmp.path().join("portable");
    fs::create_dir_all(&legacy_dir).unwrap();
    fs::create_dir_all(&portable_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    let portable_db = portable_dir.join(DB_FILE_NAME);
    fs::write(&legacy_db, b"legacy").unwrap();
    fs::write(legacy_dir.join(PORTABLE_MIGRATED_MARKER), []).unwrap();
    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            Some(portable_db.clone()),
            Some(legacy_dir.to_path_buf()),
            None,
            None,
            ExeLocationRisk::Safe,
        ),
        Some(portable_db)
    );
}

// 无迁移标记且旧库文件存在时，继续使用旧路径（老用户兼容）。
#[test]
fn pick_active_path_prefers_legacy_when_db_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    let portable_db = tmp.path().join("portable").join(DB_FILE_NAME);
    fs::write(&legacy_db, b"legacy").unwrap();
    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            Some(portable_db.clone()),
            Some(legacy_dir.to_path_buf()),
            None,
            None,
            ExeLocationRisk::Safe,
        ),
        Some(legacy_db)
    );
}

// 旧库文件不存在（新机器、exe 位置安全）时默认走便携路径。
#[test]
fn pick_active_path_defaults_to_portable_for_new_machine() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    let portable_db = tmp.path().join("portable").join(DB_FILE_NAME);
    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            Some(portable_db.clone()),
            Some(legacy_dir.to_path_buf()),
            None,
            None,
            ExeLocationRisk::Safe,
        ),
        Some(portable_db)
    );
}

// exe 位于危险/下载目录且无任何既有数据时，新库落固定目录（数据永不
// 落在可能被清理的位置）；exe 位置安全时维持便携。
#[test]
fn pick_active_path_routes_fresh_data_away_from_risky_exe_location() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    let portable_db = tmp.path().join("portable").join(DB_FILE_NAME);
    let fixed_db = tmp.path().join("fixed").join(DB_FILE_NAME);
    for risk in [ExeLocationRisk::Volatile, ExeLocationRisk::Downloads] {
        assert_eq!(
            pick_active_path(
                Some(legacy_db.clone()),
                Some(portable_db.clone()),
                Some(legacy_dir.clone()),
                Some(fixed_db.clone()),
                None,
                risk,
            ),
            Some(fixed_db.clone()),
            "风险位置 {risk:?} 的新用户数据应落固定目录"
        );
    }
    // 固定目录不可用（极端情况）时退回便携兜底，不能没有家。
    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            Some(portable_db.clone()),
            Some(legacy_dir.clone()),
            None,
            None,
            ExeLocationRisk::Volatile,
        ),
        Some(portable_db.clone())
    );
}

// exe 旁便携库已存在：真正在用的便携安装（含 U 盘场景），即使 exe 位于
// 危险目录也零打扰沿用（搬迁由对话引导，解析层不改判）。
#[test]
fn pick_active_path_prefers_existing_portable_even_in_risky_location() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    fs::write(&legacy_db, b"legacy").unwrap();
    let portable_dir = tmp.path().join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let portable_db = portable_dir.join(DB_FILE_NAME);
    fs::write(&portable_db, b"portable").unwrap();
    let fixed_db = tmp.path().join("fixed").join(DB_FILE_NAME);
    assert_eq!(
        pick_active_path(
            Some(legacy_db),
            Some(portable_db.clone()),
            Some(legacy_dir),
            Some(fixed_db),
            Some(tmp.path().join("pointer").join(DB_FILE_NAME)),
            ExeLocationRisk::Volatile,
        ),
        Some(portable_db)
    );
}

// 指针重定向：exe 旁无数据但指针指向的库仍存在时延续旧数据；
// 指针失效（目录被删）时忽略，按新装流程解析。
#[test]
fn pick_active_path_follows_and_ignores_stale_data_home_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    let portable_db = tmp.path().join("portable").join(DB_FILE_NAME);
    let fixed_db = tmp.path().join("fixed").join(DB_FILE_NAME);
    let old_home = tmp.path().join("old-home");
    fs::create_dir_all(&old_home).unwrap();
    let pointer_db = old_home.join(DB_FILE_NAME);
    fs::write(&pointer_db, b"old").unwrap();

    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            Some(portable_db.clone()),
            Some(legacy_dir.clone()),
            Some(fixed_db.clone()),
            Some(pointer_db.clone()),
            ExeLocationRisk::Safe,
        ),
        Some(pointer_db.clone()),
        "exe 挪走后应按指针延续旧数据"
    );

    // 指针失效：指向的库不存在 → 忽略指针，安全位置走便携。
    fs::remove_file(&pointer_db).unwrap();
    assert_eq!(
        pick_active_path(
            Some(legacy_db),
            Some(portable_db.clone()),
            Some(legacy_dir),
            Some(fixed_db),
            Some(pointer_db),
            ExeLocationRisk::Safe,
        ),
        Some(portable_db)
    );
}

// 便携为 None（current_exe 不可用）且无旧库时，回退旧路径兜底。
#[test]
fn pick_active_path_falls_back_to_legacy_without_portable() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_db = legacy_dir.join(DB_FILE_NAME);
    assert_eq!(
        pick_active_path(
            Some(legacy_db.clone()),
            None,
            Some(legacy_dir.to_path_buf()),
            None,
            None,
            ExeLocationRisk::Safe,
        ),
        Some(legacy_db)
    );
}

// exe 位置风险分级：按路径组件匹配，Volatile 优先于 Downloads，
// "Templates" 这类含 temp 前缀的正常目录不误伤。
#[test]
fn classify_exe_location_detects_volatile_and_downloads() {
    use ExeLocationRisk::{Downloads, Safe, Volatile};
    let volatile_cases = [
        r"C:\Users\u\AppData\Local\Temp\khaslana\khaslana.exe",
        r"C:\Users\u\AppData\Local\Temp\khaslana.exe",
        r"D:\tmp\khaslana.exe",
        r"C:\$Recycle.Bin\S-1-5\khaslana.exe",
        r"D:\WeChat Files\wxid_abc\FileStorage\File\2026-08\khaslana.exe",
        r"E:\Tencent Files\123456\FileRecv\khaslana.exe",
        r"C:\Users\u\Downloads\Telegram Desktop\khaslana.exe",
        r"C:\Users\u\AppData\Local\Microsoft\Windows\INetCache\khaslana.exe",
    ];
    for path in volatile_cases {
        assert_eq!(
            classify_exe_location(Path::new(path)),
            Volatile,
            "应判危险：{path}"
        );
    }
    assert_eq!(
        classify_exe_location(Path::new(r"C:\Users\u\Downloads\khaslana.exe")),
        Downloads
    );
    // 正常目录：组件级匹配不误伤（Templates 不等于 temp）。
    for path in [
        r"C:\Users\u\Templates\khaslana.exe",
        r"D:\Tools\khaslana\khaslana.exe",
        r"C:\Program Files\TempUtils\khaslana.exe",
    ] {
        assert_eq!(
            classify_exe_location(Path::new(path)),
            Safe,
            "应判安全：{path}"
        );
    }
    // 大小写不敏感。
    assert_eq!(
        classify_exe_location(Path::new(r"D:\WeChat FILES\x\khaslana.exe")),
        Volatile
    );
}

// 完整迁移流程：拷贝旧库与 updates、验证新库可读、写迁移标记、删除旧数据。
#[test]
fn perform_portable_migration_files_copies_and_cleans_legacy() {
    let tmp = tempfile::tempdir().unwrap();
    let src_dir = tmp.path().join("legacy");
    let dst_dir = tmp.path().join("portable");
    fs::create_dir_all(&src_dir).unwrap();
    let src_db = src_dir.join(DB_FILE_NAME);

    // 创建一个合法的旧库（含 schema_meta 表），随后关闭连接以释放文件占用。
    {
        let _storage = AppStorage::open(&src_db).unwrap();
    }
    // 构造旧 updates 目录及一个下载文件。
    let src_updates = src_dir.join("updates");
    fs::create_dir_all(src_updates.join("downloads")).unwrap();
    fs::write(src_updates.join("downloads").join("a.txt"), "payload").unwrap();

    let dst_db = dst_dir.join(DB_FILE_NAME);
    let dst_updates = dst_dir.join("updates");
    let migrated_marker = src_dir.join(PORTABLE_MIGRATED_MARKER);

    perform_portable_migration_files(
        &src_db,
        &dst_db,
        &src_updates,
        &dst_updates,
        &migrated_marker,
    )
    .expect("migration should succeed");

    assert!(dst_db.exists(), "便携库应已生成");
    assert!(
        dst_updates.join("downloads").join("a.txt").exists(),
        "updates 应已拷贝"
    );
    assert!(migrated_marker.exists(), "应写入迁移完成标记");
    assert!(!src_db.exists(), "旧库应已删除");
    assert!(!src_updates.exists(), "旧 updates 应已删除");
    // 新库应可打开且 schema_meta 表可用。
    let conn = Connection::open(&dst_db).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM schema_meta", [], |row| row.get(0))
        .unwrap();
    assert!(count >= 0);
}

// 密钥复制已成功后，数据库验证、发布或标记失败均须回滚本次密钥并允许安全重试。
#[test]
fn portable_migration_rolls_back_copied_keys_after_database_or_marker_failure() {
    use crate::ssh_keys::{GenerateSshKeyRequest, KeyProtection, SshKeyManager};
    use std::sync::Arc;

    for failure in ["validation", "rename", "existing", "marker"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("legacy");
        let target = temp.path().join("portable");
        let source_db = source.join(DB_FILE_NAME);
        let target_db = target.join(DB_FILE_NAME);
        // 标记父路径被普通文件占用，写入失败但不会伪造“已迁移”标记。
        let marker = if failure == "marker" { source.join("blocked").join(PORTABLE_MIGRATED_MARKER) }
            else { source.join(PORTABLE_MIGRATED_MARKER) };
        let source_storage = Arc::new(AppStorage::open(&source_db).unwrap());
        let source_manager = SshKeyManager::new(source_storage.clone());
        let key = source_manager.generate(GenerateSshKeyRequest {
            display_name: "migration".into(), comment: String::new(), protection: KeyProtection::Unencrypted,
        }).unwrap();
        let original_private = fs::read(source_manager.private_key_path(&key.id).unwrap()).unwrap();
        drop(source_manager);
        drop(source_storage);

        // 目标已有的合法密钥必须完整保留，不能用递归删除 keys 的方式回滚。
        let target_storage = Arc::new(AppStorage::open(target.join("existing.sqlite3")).unwrap());
        let target_manager = SshKeyManager::new(target_storage.clone());
        let existing = target_manager.generate(GenerateSshKeyRequest {
            display_name: "existing".into(), comment: String::new(), protection: KeyProtection::Unencrypted,
        }).unwrap();
        let existing_path = target_manager.private_key_path(&existing.id).unwrap();
        let existing_private = fs::read(&existing_path).unwrap();
        let copied_path = target.join("ssh/keys").join(&key.id);

        match failure {
            "validation" => {
                let conn = Connection::open(&source_db).unwrap();
                conn.execute("DROP TABLE schema_meta", []).unwrap();
            }
            "rename" => { fs::create_dir(&target_db).unwrap(); }
            "existing" => { fs::write(&target_db, b"existing database untouched").unwrap(); }
            "marker" => { fs::write(marker.parent().unwrap(), "blocked").unwrap(); }
            _ => unreachable!(),
        }
        assert!(perform_portable_migration_files(&source_db, &target_db,
            &source.join("updates"), &target.join("updates"), &marker).is_err(), "{failure}");
        assert!(!copied_path.exists(), "失败必须清理本次复制的密钥：{failure}");
        assert!(source_db.exists(), "失败必须保留源数据库：{failure}");
        if failure == "marker" {
            assert!(!target_db.exists(), "标记失败必须撤回已发布库");
            assert_eq!(pick_active_path(Some(source_db.clone()), Some(target_db.clone()),
                Some(source.clone()), None, None, ExeLocationRisk::Safe), Some(source_db.clone()));
        }
        if failure == "existing" {
            assert_eq!(fs::read(&target_db).unwrap(), b"existing database untouched");
        }
        assert_eq!(fs::read(&existing_path).unwrap(), existing_private);
        assert_eq!(fs::read(source.join("ssh/keys").join(&key.id).join("id_ed25519")).unwrap(), original_private);

        match failure {
            "validation" => { drop(AppStorage::open(&source_db).unwrap()); }
            "rename" => { fs::remove_dir(&target_db).unwrap(); }
            "existing" => { fs::remove_file(&target_db).unwrap(); }
            "marker" => { fs::remove_file(marker.parent().unwrap()).unwrap(); }
            _ => unreachable!(),
        }
        perform_portable_migration_files(&source_db, &target_db,
            &source.join("updates"), &target.join("updates"), &marker).unwrap();
        assert_eq!(fs::read(copied_path.join("id_ed25519")).unwrap(), original_private);
        assert_eq!(fs::read(&existing_path).unwrap(), existing_private);
        assert!(marker.is_file());
        assert!(!source_db.exists());
        assert_eq!(pick_active_path(Some(source_db.clone()), Some(target_db.clone()),
            Some(source.clone()), None, None, ExeLocationRisk::Safe), Some(target_db));
    }
}

// meta 通用读写与「忽略便携迁移提示」标记。
#[test]
fn meta_value_round_trip_and_portable_migration_dismissed() {
    let (_temp, storage) = temp_storage();
    assert!(!storage.portable_migration_dismissed());
    storage.mark_portable_migration_dismissed().unwrap();
    assert!(storage.portable_migration_dismissed());

    assert_eq!(storage.get_meta_value("custom_meta_key").unwrap(), None);
    storage
        .set_meta_value("custom_meta_key", "value42")
        .unwrap();
    assert_eq!(
        storage.get_meta_value("custom_meta_key").unwrap(),
        Some("value42".to_string())
    );
}

// 程序搬迁文件操作：复制 exe、staging 拷贝 data、验证库可读后落位并清理旧目录。
#[test]
fn perform_exe_relocation_files_moves_program_and_data() {
    let tmp = tempfile::tempdir().unwrap();
    // 模拟危险目录中的程序与数据。
    let home = tmp.path().join("wechat-file");
    let exe = home.join("khaslana.exe");
    fs::create_dir_all(home.join("data").join("ai-reviews")).unwrap();
    fs::write(&exe, b"exe-bytes").unwrap();
    let db_path = home.join("data").join(DB_FILE_NAME);
    {
        let storage = AppStorage::open(&db_path).unwrap();
        storage.set_meta_value("relocation-test", "1").unwrap();
    }
    fs::write(home.join("data").join("ai-reviews").join("abc.json"), b"{}").unwrap();
    {
        use crate::ssh_keys::{GenerateSshKeyRequest, KeyProtection, SshKeyManager};
        let manager = SshKeyManager::new(std::sync::Arc::new(AppStorage::open(&db_path).unwrap()));
        manager.generate(GenerateSshKeyRequest { display_name: "relocate".into(),
            comment: String::new(), protection: KeyProtection::Unencrypted }).unwrap();
    }
    fs::create_dir_all(home.join("data/ssh/custom/nested")).unwrap();
    fs::write(home.join("data/ssh/config"), b"custom config").unwrap();
    fs::write(home.join("data/ssh/custom/nested/keep"), b"custom payload").unwrap();

    let target = tmp.path().join("Programs").join("Khaslana");
    let target_exe = perform_exe_relocation_files(&exe, Some(&home.join("data")), &target).unwrap();
    assert_eq!(target_exe, target.join("khaslana.exe"));
    assert_eq!(fs::read(&target_exe).unwrap(), b"exe-bytes");
    assert_eq!(fs::read(target.join("data/ssh/config")).unwrap(), b"custom config");
    assert_eq!(fs::read(target.join("data/ssh/custom/nested/keep")).unwrap(), b"custom payload");
    {
        let manager = crate::ssh_keys::SshKeyManager::new(std::sync::Arc::new(
            AppStorage::open(target.join("data").join(DB_FILE_NAME)).unwrap()));
        let key = manager.list().unwrap().remove(0);
        assert!(manager.private_key_path(&key.id).unwrap().is_file());
    }
    // 数据整体落位且库可打开。
    let relocated_db = target.join("data").join(DB_FILE_NAME);
    assert!(relocated_db.exists());
    let storage = AppStorage::open(&relocated_db).unwrap();
    assert_eq!(
        storage
            .get_meta_value("relocation-test")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    assert!(
        target
            .join("data")
            .join("ai-reviews")
            .join("abc.json")
            .exists()
    );
    // staging 不残留；旧数据目录被清理（旧 exe 仍在，属预期——运行中的
    // exe 不能删，由用户或后续清理处理）。
    assert!(!target.join("data.relocating").exists());
    assert!(!home.join("data").exists());
}

// 没有数据目录（仅一个裸 exe）时搬迁也成立：只复制程序。
#[test]
fn perform_exe_relocation_files_works_without_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("Temp");
    fs::create_dir_all(&home).unwrap();
    let exe = home.join("khaslana.exe");
    fs::write(&exe, b"exe-bytes").unwrap();

    let target = tmp.path().join("target");
    let target_exe = perform_exe_relocation_files(&exe, Some(&home.join("data")), &target).unwrap();
    assert_eq!(fs::read(&target_exe).unwrap(), b"exe-bytes");
    assert!(!target.join("data").exists());
}

#[test]
fn ai_protocols_and_reasoning_survive_storage() {
    let (_temp, storage) = temp_storage();
    for api_type in [crate::AiApiType::ChatCompletions, crate::AiApiType::Responses, crate::AiApiType::AnthropicMessages] {
        let settings = AiProviderSettings {
            api_type, temperature: 1.0, max_tokens: 800, request_timeout_secs: 123,
            reasoning_effort: Some("custom-effort".into()), ..Default::default()
        };
        storage.save_ai_provider_settings(&settings).unwrap();
        assert_eq!(storage.load_ai_provider_settings().unwrap(), settings);
    }
}

#[test]
fn legacy_ai_payload_without_effort_keeps_saved_advanced_values() {
    let (_temp, storage) = temp_storage();
    let mut json = serde_json::to_value(AiProviderSettings::default()).unwrap();
    json.as_object_mut().unwrap().remove("reasoning_effort");
    json["max_tokens"] = 800.into();
    let conn = storage.lock_conn().unwrap();
    conn.execute("INSERT INTO ai_provider_settings (id, enabled, payload, updated_at) VALUES (1, 0, ?1, 0)",
        [json.to_string()]).unwrap();
    drop(conn);
    let loaded = storage.load_ai_provider_settings().unwrap();
    assert_eq!(loaded.max_tokens, 800);
    assert_eq!(loaded.reasoning_effort, None);
}
