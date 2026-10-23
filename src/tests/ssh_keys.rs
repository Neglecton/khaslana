use super::*;
use crate::credentials::{CredentialRecord, CredentialScope, StoredCredentialKind};

fn manager() -> (tempfile::TempDir, Arc<AppStorage>, SshKeyManager) {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(AppStorage::open(temp.path().join("settings.sqlite3")).unwrap());
    let manager = SshKeyManager::new(storage.clone());
    (temp, storage, manager)
}
fn request(protection: KeyProtection) -> GenerateSshKeyRequest {
    GenerateSshKeyRequest { display_name: "Work key".into(), comment: "user@example.com".into(), protection }
}
fn reference(key: &ManagedSshKey, path: &Path) -> CredentialRecord {
    CredentialRecord {
        id: "credential".into(), display_name: None, scope: CredentialScope::Host,
        kind: StoredCredentialKind::SshKey, host: "example.com".into(),
        remote_url: "git@example.com:repo.git".into(), username: "git".into(),
        key_path: Some(path.to_string_lossy().into_owned()), managed_key_id: Some(key.id.clone()),
        created_at: 1, updated_at: 1, last_used: None,
    }
}

#[test]
fn explicit_unencrypted_key_is_valid_openssh_and_persists() {
    let (_temp, storage, manager) = manager();
    let record = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let path = manager.private_key_path(&record.id).unwrap();
    let key = PrivateKey::read_openssh_file(&path).unwrap();
    assert_eq!(key.algorithm(), Algorithm::Ed25519);
    assert!(!key.is_encrypted());
    assert_eq!(key.comment(), "user@example.com");
    assert_eq!(key.public_key().to_openssh().unwrap(), record.public_key);
    assert_eq!(manager.list().unwrap(), vec![record.clone()]);
    assert_eq!(manager.public_key(&record.id).unwrap(), record.public_key);
    let root = storage.managed_ssh_root().unwrap();
    assert_eq!(path, root.join(&record.id).join("id_ed25519"));
    assert!(path.with_extension("pub").is_file());
    manager.delete(&record.id).unwrap();
    assert!(manager.list().unwrap().is_empty());
    assert!(!root.join(record.id).exists());
}

#[test]
fn encrypted_key_requires_correct_passphrase_and_secret_never_reaches_database() {
    let (temp, storage, manager) = manager();
    let password = "test-passphrase-unique-secret-value";
    let record = manager.generate(request(KeyProtection::Passphrase(password.into()))).unwrap();
    assert!(record.encrypted);
    let key = PrivateKey::read_openssh_file(&manager.private_key_path(&record.id).unwrap()).unwrap();
    assert!(key.is_encrypted());
    assert!(key.decrypt("wrong").is_err());
    assert_eq!(key.decrypt(password).unwrap().algorithm(), Algorithm::Ed25519);
    drop(manager);
    drop(storage);
    let database = fs::read(temp.path().join("settings.sqlite3")).unwrap();
    assert!(!database.windows(password.len()).any(|bytes| bytes == password.as_bytes()));
}

#[test]
fn empty_passphrase_and_multiline_comment_are_rejected_without_files() {
    let (_temp, storage, manager) = manager();
    assert!(manager.generate(request(KeyProtection::Passphrase(String::new()))).is_err());
    let mut bad = request(KeyProtection::Unencrypted);
    bad.comment = "first\nsecond".into();
    assert!(manager.generate(bad).is_err());
    assert!(!storage.managed_ssh_root().unwrap().exists());
}

#[test]
fn id_traversal_is_rejected() {
    let (_temp, _storage, manager) = manager();
    for id in ["..", "../key", "", "ABCDEF0123456789abcdef0123456789"] {
        assert!(manager.delete(id).is_err());
        assert!(manager.private_key_path(id).is_err());
    }
}

#[test]
fn referenced_key_cannot_be_deleted_and_missing_id_cannot_be_saved() {
    let (_temp, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let path = manager.private_key_path(&key.id).unwrap();
    let mut record = reference(&key, &path);
    storage.save_credential_records(&[record.clone()]).unwrap();
    assert!(manager.delete(&key.id).is_err());
    assert!(path.exists());
    record.managed_key_id = Some("00000000000000000000000000000000".into());
    assert!(storage.save_credential_records(&[record]).is_err());
    // 失败事务不能清空原有凭据记录。
    assert_eq!(storage.load_credential_records().unwrap().len(), 1);
    storage.save_credential_records(&[]).unwrap();
    manager.delete(&key.id).unwrap();
}

#[test]
fn legacy_path_reference_also_blocks_deletion() {
    let (_temp, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let mut record = reference(&key, &manager.private_key_path(&key.id).unwrap());
    record.managed_key_id = None;
    storage.save_credential_records(&[record]).unwrap();
    assert!(manager.delete(&key.id).is_err());
}

#[test]
fn stable_id_survives_data_directory_migration() {
    let (source, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    storage.save_credential_records(&[reference(&key, &manager.private_key_path(&key.id).unwrap())]).unwrap();
    let target = tempfile::tempdir().unwrap();
    let target_db = target.path().join("settings.sqlite3");
    copy_managed_tree(&storage.managed_ssh_root().unwrap(), &target.path().join("ssh/keys")).unwrap();
    drop(manager);
    drop(storage);
    fs::copy(source.path().join("settings.sqlite3"), &target_db).unwrap();
    let migrated = Arc::new(AppStorage::open(target_db).unwrap());
    let manager = SshKeyManager::new(migrated.clone());
    let path = manager.private_key_path(&key.id).unwrap();
    assert!(path.starts_with(target.path()));
    assert_eq!(migrated.load_credential_records().unwrap()[0].key_path.as_deref(), path.to_str());
    assert!(manager.delete(&key.id).is_err());
}

#[test]
fn corrupted_private_key_is_not_returned_for_authentication() {
    let (_temp, _storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let path = manager.private_key_path(&key.id).unwrap();
    fs::write(path, "not a private key").unwrap();
    assert!(manager.private_key_path(&key.id).is_err());
}

#[test]
fn exclusive_creation_does_not_overwrite_existing_file() {
    let (_temp, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let root = security::Directory::open_tree(&storage.managed_ssh_root().unwrap()).unwrap();
    assert!(root.create_child(&key.id).is_err());
    let directory = root.open_child(&key.id).unwrap();
    assert!(directory.write_new("id_ed25519", b"overwrite").is_err());
    assert!(manager.private_key_path(&key.id).is_ok());
}

#[test]
fn old_schema_migrates_without_changing_external_key_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.sqlite3");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE credential_records(id TEXT PRIMARY KEY,display_name TEXT,scope TEXT NOT NULL,kind TEXT NOT NULL,host TEXT NOT NULL,remote_url TEXT NOT NULL,username TEXT NOT NULL,key_path TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,last_used INTEGER);
        INSERT INTO credential_records VALUES('old',NULL,'host','ssh_key','example.com','git@example.com:x','git','/external/id_ed25519',1,1,NULL);").unwrap();
    drop(conn);
    let storage = AppStorage::open(&path).unwrap();
    let records = storage.load_credential_records().unwrap();
    assert_eq!(records[0].managed_key_id, None);
    assert_eq!(records[0].key_path.as_deref(), Some("/external/id_ed25519"));
    drop(storage);
    assert!(AppStorage::open(path).is_ok());
}

#[test]
fn database_failure_removes_new_private_and_public_files() {
    let (temp, storage, manager) = manager();
    let connection = rusqlite::Connection::open(temp.path().join("settings.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_key BEFORE INSERT ON managed_ssh_keys BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(manager.generate(request(KeyProtection::Unencrypted)).is_err());
    assert!(manager.list().unwrap().is_empty());
    assert_eq!(fs::read_dir(storage.managed_ssh_root().unwrap()).unwrap().count(), 0);
}

#[test]
fn bilingual_resources_have_consistent_parameters() {
    crate::ui_i18n::validate(&[INVALID, UNSAFE, MISSING, REFERENCED, FAILED]).unwrap();
}

#[cfg(unix)]
#[test]
fn unix_permissions_and_symlink_protection() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (temp, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let path = manager.private_key_path(&key.id).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(manager.private_key_path(&key.id).is_err());
    fs::remove_file(&path).unwrap();
    let external = temp.path().join("outside");
    fs::write(&external, "outside").unwrap();
    symlink(&external, &path).unwrap();
    assert!(manager.private_key_path(&key.id).is_err());
    fs::create_dir(temp.path().join("copy")).unwrap();
    assert!(copy_managed_tree(&storage.managed_ssh_root().unwrap(), &temp.path().join("copy/ssh/keys")).is_err());
    assert_eq!(fs::read_to_string(external).unwrap(), "outside");
}

#[cfg(windows)]
#[test]
fn windows_permissive_acl_is_rejected() {
    let (_temp, _storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let path = manager.private_key_path(&key.id).unwrap();
    // SID 参数不依赖 Windows 系统语言；授予 Everyone 读权限后必须拒绝认证。
    let output = std::process::Command::new("icacls").arg(&path)
        .args(["/grant", "*S-1-1-0:R"]).output().unwrap();
    assert!(output.status.success(), "ACL update failed");
    assert!(manager.private_key_path(&key.id).is_err());
}

#[cfg(windows)]
#[test]
fn windows_reparse_directory_is_rejected() {
    let (temp, storage, manager) = manager();
    let key = manager.generate(request(KeyProtection::Unencrypted)).unwrap();
    let root = storage.managed_ssh_root().unwrap();
    let destination = temp.path().join("external");
    fs::create_dir(&destination).unwrap();
    manager.delete(&key.id).unwrap();
    let junction = root.join(&key.id);
    let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"])
        .arg(&junction).arg(&destination).output().unwrap();
    assert!(output.status.success(), "junction creation failed");
    let parent = security::Directory::open_tree(&root).unwrap();
    assert!(parent.open_child(&key.id).is_err());
    drop(parent);
    fs::remove_dir(junction).unwrap();
}
