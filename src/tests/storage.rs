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
