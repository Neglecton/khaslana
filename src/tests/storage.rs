use super::*;

fn temp_storage() -> (tempfile::TempDir, AppStorage) {
    let temp = tempfile::tempdir().unwrap();
    let storage = AppStorage::open(temp.path().join("app.sqlite3")).unwrap();
    (temp, storage)
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
    assert_eq!(loaded.temperature, settings.temperature);
    assert_eq!(loaded.max_tokens, settings.max_tokens);
    assert_eq!(loaded.request_timeout_secs, settings.request_timeout_secs);
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
        created_at: 1,
        updated_at: 2,
        last_used: Some(3),
    }];
    storage.save_credential_records(&records).unwrap();
    assert_eq!(storage.load_credential_records().unwrap(), records);
}

#[test]
fn legacy_json_imports_existing_files() {
    let temp = tempfile::tempdir().unwrap();
    let paths = legacy_storage_paths(temp.path());
    fs::write(
        &paths.session,
        serde_json::to_string(&SessionState {
            repo_paths: vec![PathBuf::from("C:/repo/a")],
            active_repo_path: Some(PathBuf::from("C:/repo/a")),
        })
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &paths.network_proxy,
        serde_json::to_string(&NetworkProxySettings {
            mode: NetworkProxyMode::System,
            custom: CustomProxySettings::default(),
        })
        .unwrap(),
    )
    .unwrap();

    let storage = AppStorage::open(temp.path().join("app.sqlite3")).unwrap();
    let summary = storage.import_legacy_json(&paths, false).unwrap();

    assert!(summary.session);
    assert!(summary.network_proxy);
    assert_eq!(
        storage.load_session_state().unwrap().unwrap().repo_paths,
        vec![PathBuf::from("C:/repo/a")]
    );
    assert_eq!(
        storage.load_proxy_settings().unwrap().mode,
        NetworkProxyMode::System
    );
}

#[test]
fn update_preferences_default() {
    let (_temp, storage) = temp_storage();
    // 空表返回默认值。
    let prefs = storage.load_update_preferences().unwrap();
    assert!(prefs.auto_check);
    assert_eq!(prefs.skipped_version, None);
}

#[test]
fn update_preferences_round_trip() {
    let (_temp, storage) = temp_storage();
    let prefs = UpdatePreferences {
        auto_check: false,
        skipped_version: Some("0.1.0".into()),
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
    };
    storage.save_update_preferences(&prefs).unwrap();
    assert!(!storage.load_update_preferences().unwrap().auto_check);
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
        ),
        Some(legacy_db)
    );
}

// 旧库文件不存在（新机器）时默认走便携路径。
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
            Some(legacy_dir.to_path_buf())
        ),
        Some(legacy_db)
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
