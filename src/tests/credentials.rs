use super::*;

fn request(url: &str, allowed_types: CredentialType) -> CredentialRequest {
    CredentialRequest {
        url: url.to_string(),
        username_from_url: Some("git".to_string()),
        allowed_types,
        repo_path: None,
        remote_name: None,
        operation_id: None,
    }
}

fn https_credential(scope: CredentialScope, secret: &str) -> GitCredential {
    GitCredential::UserPass {
        username: "git".to_string(),
        secret: secret.to_string(),
        display_name: None,
        save_to_keyring: true,
        scope,
    }
}

fn ssh_credential(scope: CredentialScope, key_path: &str) -> GitCredential {
    GitCredential::SshPassphrase {
        username: "git".to_string(),
        private_key_path: Some(key_path.to_string()),
        passphrase: Some("phrase".to_string()),
        display_name: None,
        save_to_keyring: true,
        scope,
    }
}

fn credential_record(display_name: Option<String>) -> CredentialRecord {
    CredentialRecord {
        id: "id".to_string(),
        display_name,
        scope: CredentialScope::RemoteUrl,
        kind: StoredCredentialKind::HttpsUserPass,
        host: "https://example.com".to_string(),
        remote_url: "https://example.com/team/repo.git".to_string(),
        username: "git".to_string(),
        key_path: None,
        created_at: 1,
        updated_at: 1,
        last_used: Some(1),
    }
}

#[test]
fn remote_url_scope_wins_over_host_scope() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    store
        .save(&req, &https_credential(CredentialScope::Host, "host"))
        .unwrap();
    store
        .save(
            &req,
            &https_credential(CredentialScope::RemoteUrl, "remote"),
        )
        .unwrap();

    let credential = store.get(&req).unwrap().unwrap();
    assert!(matches!(
        credential,
        GitCredential::UserPass { secret, .. } if secret == "remote"
    ));
}

#[test]
fn remote_url_scope_ignores_username_in_callback_url() {
    let store = MemoryCredentialStore::new();
    let saved_req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let callback_req = request(
        "https://git@example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    store
        .save(
            &saved_req,
            &https_credential(CredentialScope::RemoteUrl, "remote"),
        )
        .unwrap();

    let credential = store.get(&callback_req).unwrap().unwrap();
    assert!(matches!(
        credential,
        GitCredential::UserPass { secret, .. } if secret == "remote"
    ));
}

#[test]
fn https_and_ssh_same_host_do_not_cross_reuse() {
    let store = MemoryCredentialStore::new();
    let https_req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let ssh_req = request("git@example.com:team/repo.git", CredentialType::SSH_KEY);
    store
        .save(
            &https_req,
            &https_credential(CredentialScope::Host, "https"),
        )
        .unwrap();
    store
        .save(
            &ssh_req,
            &ssh_credential(CredentialScope::Host, "C:/Users/me/.ssh/id_ed25519"),
        )
        .unwrap();

    assert!(matches!(
        store.get(&https_req).unwrap().unwrap(),
        GitCredential::UserPass { .. }
    ));
    assert!(matches!(
        store.get(&ssh_req).unwrap().unwrap(),
        GitCredential::SshPassphrase { .. }
    ));
}

#[test]
fn host_scope_uses_most_recent_last_used() {
    let store = MemoryCredentialStore::new();
    let req_a = request(
        "https://example.com/team/a.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let req_b = request(
        "https://example.com/team/b.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    store
        .save(&req_a, &https_credential(CredentialScope::Host, "old"))
        .unwrap();
    store
        .save(&req_b, &https_credential(CredentialScope::Host, "new"))
        .unwrap();
    let credential = store.get(&req_a).unwrap().unwrap();
    assert!(matches!(
        credential,
        GitCredential::UserPass { secret, .. } if secret == "new"
    ));
}

#[test]
fn touch_record_makes_host_scope_credential_preferred() {
    let store = MemoryCredentialStore::new();
    let req_a = request(
        "https://example.com/team/a.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let req_b = request(
        "https://example.com/team/b.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let old = store
        .save_record(&req_a, &https_credential(CredentialScope::Host, "old"))
        .unwrap();
    store
        .save_record(&req_b, &https_credential(CredentialScope::Host, "new"))
        .unwrap();

    store.touch_record(&old.id).unwrap();

    let credential = store.get(&req_b).unwrap().unwrap();
    assert!(matches!(
        credential,
        GitCredential::UserPass { secret, .. } if secret == "old"
    ));
}

#[test]
fn rejected_record_is_not_reused() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let record = store
        .save_record(
            &req,
            &https_credential(CredentialScope::RemoteUrl, "secret"),
        )
        .unwrap();
    let stored = store.get_stored(&req, &[record.id]).unwrap();
    assert!(stored.is_none());
}

#[test]
fn credential_record_url_compatibility_matches_protocol_family() {
    let https = CredentialRecord {
        id: "https".to_string(),
        display_name: None,
        scope: CredentialScope::RemoteUrl,
        kind: StoredCredentialKind::HttpsUserPass,
        host: "https://example.com".to_string(),
        remote_url: "https://example.com/team/repo.git".to_string(),
        username: "git".to_string(),
        key_path: None,
        created_at: 1,
        updated_at: 1,
        last_used: Some(1),
    };
    let ssh = CredentialRecord {
        id: "ssh".to_string(),
        display_name: None,
        scope: CredentialScope::Host,
        kind: StoredCredentialKind::SshKey,
        host: "ssh://example.com".to_string(),
        remote_url: "git@example.com:team/repo.git".to_string(),
        username: "git".to_string(),
        key_path: Some("C:/Users/me/.ssh/id_ed25519".to_string()),
        created_at: 1,
        updated_at: 1,
        last_used: Some(1),
    };

    assert!(credential_record_is_compatible_with_url(
        &https,
        "https://example.com/other/repo.git"
    ));
    assert!(!credential_record_is_compatible_with_url(
        &https,
        "git@example.com:other/repo.git"
    ));
    assert!(credential_record_matches_remote_url(
        &ssh,
        "git@example.com:other/repo.git"
    ));
    assert!(!credential_record_matches_remote_url(
        &ssh,
        "https://example.com/other/repo.git"
    ));
}

#[test]
fn update_record_remote_url_rebinds_remote_url_scope_record() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let record = store
        .save_record(
            &req,
            &https_credential(CredentialScope::RemoteUrl, "secret"),
        )
        .unwrap();

    let updated = store
        .update_record_remote_url(&record.id, "https://other.example/new/repo.git")
        .unwrap();

    assert_eq!(updated.remote_url, "https://other.example/new/repo.git");
    assert_eq!(updated.host, "https://other.example");
    assert!(!credential_record_matches_remote_url(
        &updated,
        "https://example.com/team/repo.git"
    ));
    assert!(credential_record_matches_remote_url(
        &updated,
        "https://other.example/new/repo.git"
    ));
    assert!(store.credential_for_record(&record.id).unwrap().is_some());
}

#[test]
fn delete_record_removes_index_and_secret() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let record = store
        .save_record(
            &req,
            &https_credential(CredentialScope::RemoteUrl, "secret"),
        )
        .unwrap();
    store.delete_record(&record.id).unwrap();
    assert!(store.get(&req).unwrap().is_none());
    assert!(store.list_records().unwrap().is_empty());
}

#[test]
fn credential_index_does_not_serialize_secrets() {
    let record = credential_record(Some("Example PAT".to_string()));
    let index = CredentialIndex {
        records: vec![record],
    };
    let json = serde_json::to_string(&index).unwrap();
    assert!(json.contains("Example PAT"));
    assert!(!json.contains("password"));
    assert!(!json.contains("token"));
    assert!(!json.contains("secret"));
}

#[test]
fn credential_record_json_without_display_name_is_compatible() {
    let json = r#"{
        "id":"id",
        "scope":"RemoteUrl",
        "kind":"HttpsUserPass",
        "host":"https://example.com",
        "remote_url":"https://example.com/team/repo.git",
        "username":"git",
        "key_path":null,
        "created_at":1,
        "updated_at":1,
        "last_used":1
    }"#;

    let record: CredentialRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.display_name, None);
    assert_eq!(credential_record_label(&record), "HTTPS 仅此远端 git");
}

#[test]
fn display_name_is_saved_and_used_as_record_label() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let credential = GitCredential::UserPass {
        username: "git".to_string(),
        secret: "secret".to_string(),
        display_name: Some("Example PAT".to_string()),
        save_to_keyring: true,
        scope: CredentialScope::RemoteUrl,
    };

    let record = store.save_record(&req, &credential).unwrap();

    assert_eq!(record.display_name.as_deref(), Some("Example PAT"));
    assert_eq!(credential_record_label(&record), "Example PAT");
    let json = serde_json::to_string(&CredentialIndex {
        records: vec![record],
    })
    .unwrap();
    assert!(json.contains("Example PAT"));
    assert!(!json.contains("secret"));
}

#[test]
fn blank_display_name_falls_back_to_generated_label() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let credential = GitCredential::UserPass {
        username: "git".to_string(),
        secret: "secret".to_string(),
        display_name: Some("   ".to_string()),
        save_to_keyring: true,
        scope: CredentialScope::RemoteUrl,
    };

    let record = store.save_record(&req, &credential).unwrap();

    assert_eq!(record.display_name, None);
    assert_eq!(credential_record_label(&record), "HTTPS 仅此远端 git");
}

#[test]
fn saving_same_record_with_name_updates_display_name() {
    let store = MemoryCredentialStore::new();
    let req = request(
        "https://example.com/team/repo.git",
        CredentialType::USER_PASS_PLAINTEXT,
    );
    let first = GitCredential::UserPass {
        username: "git".to_string(),
        secret: "old".to_string(),
        display_name: Some("Old name".to_string()),
        save_to_keyring: true,
        scope: CredentialScope::RemoteUrl,
    };
    let second = GitCredential::UserPass {
        username: "git".to_string(),
        secret: "new".to_string(),
        display_name: Some("New name".to_string()),
        save_to_keyring: true,
        scope: CredentialScope::RemoteUrl,
    };

    let first = store.save_record(&req, &first).unwrap();
    let second = store.save_record(&req, &second).unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(second.display_name.as_deref(), Some("New name"));
}

#[test]
fn old_storage_format_parses_for_migration() {
    let credential = GitCredential::from_old_storage(
        "git".to_string(),
        "ssh:C:/id:phrase".to_string(),
        CredentialType::SSH_KEY,
        CredentialScope::RemoteUrl,
    )
    .unwrap();
    assert!(matches!(
        credential,
        GitCredential::SshPassphrase {
            private_key_path: Some(path),
            passphrase: Some(passphrase),
            ..
        } if path == "C:/id" && passphrase == "phrase"
    ));
}

#[test]
fn host_key_distinguishes_protocol_families() {
    let https = remote_metadata("https://example.com/team/repo.git").unwrap();
    let ssh = remote_metadata("git@example.com:team/repo.git").unwrap();
    assert_eq!(https.host_key, "https://example.com");
    assert_eq!(ssh.host_key, "ssh://example.com");
    assert_ne!(https.host_key, ssh.host_key);
}

#[test]
fn ssh_username_prefers_remote_url_identity() {
    assert_eq!(
        ssh_username_from_remote_url("git@github.com:owner/repo.git").as_deref(),
        Some("git")
    );
    assert_eq!(
        ssh_username_from_remote_url("ssh://alice@example.com/repo").as_deref(),
        Some("alice")
    );
}

#[test]
fn system_git_ssh_command_uses_selected_key_and_strict_host_check() {
    let credential = ssh_credential(CredentialScope::Host, "C:/Users/me/.ssh/id_ed25519");
    let command = git_ssh_command_for_credential(&credential).unwrap();
    // 路径用单引号包裹（' 本身以 '\'' 转义），防止 $()、反引号等 shell 展开。
    assert!(command.contains("-i 'C:/Users/me/.ssh/id_ed25519'"));
    assert!(command.contains("IdentitiesOnly=yes"));
    assert!(command.contains("BatchMode=yes"));
    assert!(command.contains("StrictHostKeyChecking=yes"));
}

#[test]
fn system_git_ssh_command_escapes_single_quotes_in_key_path() {
    let credential = ssh_credential(CredentialScope::Host, "C:/odd'name/key");
    let command = git_ssh_command_for_credential(&credential).unwrap();
    // 单引号路径内的 ' 必须转义为 '\''，否则会提前闭合引号。
    assert!(command.contains("-i 'C:/odd'\\''name/key'"));
}
