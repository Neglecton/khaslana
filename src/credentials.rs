use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use git2::{Cred, CredentialType};
use serde::{Deserialize, Serialize};

use crate::storage::AppStorage;
use crate::types::{GitError, Result};

const KEYRING_SERVICE_PREFIX: &str = "khaslana.git.credential";
const OLD_KEYRING_SERVICE_PREFIX: &str = "khaslana";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRequest {
    pub url: String,
    pub username_from_url: Option<String>,
    pub allowed_types: CredentialType,
    pub repo_path: Option<PathBuf>,
    pub remote_name: Option<String>,
    pub operation_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialScope {
    #[default]
    RemoteUrl,
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredCredentialKind {
    HttpsUserPass,
    SshKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteCredentialPolicy {
    AutoMatch,
    NoCredential,
    Record(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub scope: CredentialScope,
    pub kind: StoredCredentialKind,
    pub host: String,
    pub remote_url: String,
    pub username: String,
    pub key_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCredential {
    pub record: CredentialRecord,
    pub credential: GitCredential,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCredential {
    UserPass {
        username: String,
        secret: String,
        display_name: Option<String>,
        save_to_keyring: bool,
        scope: CredentialScope,
    },
    SshPassphrase {
        username: String,
        private_key_path: Option<String>,
        passphrase: Option<String>,
        display_name: Option<String>,
        save_to_keyring: bool,
        scope: CredentialScope,
    },
}

impl GitCredential {
    pub fn username(&self) -> &str {
        match self {
            GitCredential::UserPass { username, .. }
            | GitCredential::SshPassphrase { username, .. } => username,
        }
    }

    pub fn should_save(&self) -> bool {
        match self {
            GitCredential::UserPass {
                save_to_keyring, ..
            }
            | GitCredential::SshPassphrase {
                save_to_keyring, ..
            } => *save_to_keyring,
        }
    }

    pub fn scope(&self) -> CredentialScope {
        match self {
            GitCredential::UserPass { scope, .. } | GitCredential::SshPassphrase { scope, .. } => {
                *scope
            }
        }
    }

    pub fn kind(&self) -> StoredCredentialKind {
        match self {
            GitCredential::UserPass { .. } => StoredCredentialKind::HttpsUserPass,
            GitCredential::SshPassphrase { .. } => StoredCredentialKind::SshKey,
        }
    }

    pub fn key_path(&self) -> Option<&str> {
        match self {
            GitCredential::UserPass { .. } => None,
            GitCredential::SshPassphrase {
                private_key_path, ..
            } => private_key_path.as_deref(),
        }
    }

    pub fn display_name(&self) -> Option<&str> {
        match self {
            GitCredential::UserPass { display_name, .. }
            | GitCredential::SshPassphrase { display_name, .. } => display_name.as_deref(),
        }
    }

    fn secret_for_keyring(&self) -> String {
        match self {
            GitCredential::UserPass { secret, .. } => secret.clone(),
            GitCredential::SshPassphrase { passphrase, .. } => {
                passphrase.clone().unwrap_or_default()
            }
        }
    }

    fn from_record(record: &CredentialRecord, secret: String) -> Option<Self> {
        match record.kind {
            StoredCredentialKind::HttpsUserPass => Some(Self::UserPass {
                username: record.username.clone(),
                secret,
                display_name: record.display_name.clone(),
                save_to_keyring: true,
                scope: record.scope,
            }),
            StoredCredentialKind::SshKey => Some(Self::SshPassphrase {
                username: record.username.clone(),
                private_key_path: record.key_path.clone(),
                passphrase: (!secret.is_empty()).then_some(secret),
                display_name: record.display_name.clone(),
                save_to_keyring: true,
                scope: record.scope,
            }),
        }
    }

    fn from_old_storage(
        username: String,
        secret: String,
        allowed: CredentialType,
        scope: CredentialScope,
    ) -> Option<Self> {
        if let Some(secret) = secret.strip_prefix("https:") {
            return Some(Self::UserPass {
                username,
                secret: secret.to_string(),
                display_name: None,
                save_to_keyring: true,
                scope,
            });
        }

        if let Some(rest) = secret.strip_prefix("ssh:") {
            if !allowed.contains(CredentialType::SSH_KEY) {
                return None;
            }
            let (key_path, passphrase) = rest
                .rsplit_once(':')
                .map(|(path, passphrase)| (path, passphrase))
                .unwrap_or((rest, ""));
            let key_path = (!key_path.is_empty()).then(|| key_path.to_string());
            let passphrase = (!passphrase.is_empty()).then(|| passphrase.to_string());
            return Some(Self::SshPassphrase {
                username,
                private_key_path: key_path,
                passphrase,
                display_name: None,
                save_to_keyring: true,
                scope,
            });
        }

        None
    }
}

pub trait CredentialStore: Send + Sync {
    fn get(&self, request: &CredentialRequest) -> Result<Option<GitCredential>>;
    fn get_stored(
        &self,
        request: &CredentialRequest,
        rejected_record_ids: &[String],
    ) -> Result<Option<StoredCredential>>;
    fn save(&self, request: &CredentialRequest, credential: &GitCredential) -> Result<()>;
    fn save_record(
        &self,
        request: &CredentialRequest,
        credential: &GitCredential,
    ) -> Result<CredentialRecord>;
    fn delete(&self, request: &CredentialRequest, username: &str) -> Result<()>;
    fn delete_record(&self, record_id: &str) -> Result<()>;
    fn list_records(&self) -> Result<Vec<CredentialRecord>>;
    fn credential_for_record(&self, record_id: &str) -> Result<Option<GitCredential>>;
    fn touch_record(&self, record_id: &str) -> Result<Option<CredentialRecord>>;
    fn update_record_remote_url(
        &self,
        record_id: &str,
        remote_url: &str,
    ) -> Result<CredentialRecord>;
}

#[derive(Default)]
pub struct MemoryCredentialStore {
    index: Mutex<Vec<CredentialRecord>>,
    secrets: Mutex<HashMap<String, String>>,
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert_secret(&self, record: &CredentialRecord, credential: &GitCredential) -> Result<()> {
        self.secrets
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .insert(record.id.clone(), credential.secret_for_keyring());
        Ok(())
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, request: &CredentialRequest) -> Result<Option<GitCredential>> {
        self.get_stored(request, &[])
            .map(|stored| stored.map(|stored| stored.credential))
    }

    fn get_stored(
        &self,
        request: &CredentialRequest,
        rejected_record_ids: &[String],
    ) -> Result<Option<StoredCredential>> {
        let records = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .clone();
        let rejected = rejected_record_ids.iter().cloned().collect::<BTreeSet<_>>();
        let Some(record) = select_matching_record(&records, request, &rejected) else {
            return Ok(None);
        };
        let secret = self
            .secrets
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .get(&record.id)
            .cloned();
        let Some(secret) = secret else {
            return Ok(None);
        };
        let Some(credential) = GitCredential::from_record(&record, secret) else {
            return Ok(None);
        };
        let mut updated = record.clone();
        updated.last_used = Some(now_seconds());
        updated.updated_at = updated.last_used.unwrap_or(updated.updated_at);
        let mut index = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?;
        if let Some(existing) = index.iter_mut().find(|candidate| candidate.id == record.id) {
            *existing = updated.clone();
        }
        Ok(Some(StoredCredential {
            record: updated,
            credential,
        }))
    }

    fn save(&self, request: &CredentialRequest, credential: &GitCredential) -> Result<()> {
        self.save_record(request, credential).map(|_| ())
    }

    fn save_record(
        &self,
        request: &CredentialRequest,
        credential: &GitCredential,
    ) -> Result<CredentialRecord> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?;
        let metadata = remote_metadata(&request.url)
            .ok_or_else(|| GitError::Credential("无法解析远端地址，不能保存凭据".to_string()))?;
        let now = next_record_timestamp(&index);
        let display_name = credential.display_name().and_then(normalize_display_name);
        let record = if let Some(existing) = index.iter_mut().find(|record| {
            record.scope == credential.scope()
                && record.kind == credential.kind()
                && record.host == metadata.host_key
                && normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url)
                && record.username == credential.username()
                && record.key_path.as_deref() == credential.key_path()
        }) {
            existing.updated_at = now;
            existing.last_used = Some(now);
            if display_name.is_some() {
                existing.display_name = display_name.clone();
            }
            existing.clone()
        } else {
            let record = CredentialRecord {
                id: new_record_id(),
                display_name: display_name.clone(),
                scope: credential.scope(),
                kind: credential.kind(),
                host: metadata.host_key,
                remote_url: request.url.clone(),
                username: credential.username().to_string(),
                key_path: credential.key_path().map(str::to_string),
                created_at: now,
                updated_at: now,
                last_used: Some(now),
            };
            index.push(record.clone());
            record
        };
        drop(index);
        self.insert_secret(&record, credential)?;
        Ok(record)
    }

    fn delete(&self, request: &CredentialRequest, username: &str) -> Result<()> {
        let ids = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .iter()
            .filter(|record| normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url) && record.username == username)
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.delete_record(&id)?;
        }
        Ok(())
    }

    fn delete_record(&self, record_id: &str) -> Result<()> {
        self.index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .retain(|record| record.id != record_id);
        self.secrets
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .remove(record_id);
        Ok(())
    }

    fn list_records(&self) -> Result<Vec<CredentialRecord>> {
        let mut records = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .clone();
        sort_records(&mut records);
        Ok(records)
    }

    fn credential_for_record(&self, record_id: &str) -> Result<Option<GitCredential>> {
        let record = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .iter()
            .find(|record| record.id == record_id)
            .cloned();
        let Some(record) = record else {
            return Ok(None);
        };
        let secret = self
            .secrets
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?
            .get(record_id)
            .cloned();
        Ok(secret.and_then(|secret| GitCredential::from_record(&record, secret)))
    }

    fn touch_record(&self, record_id: &str) -> Result<Option<CredentialRecord>> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?;
        let now = next_record_timestamp(&index);
        let Some(record) = index.iter_mut().find(|record| record.id == record_id) else {
            return Ok(None);
        };
        record.last_used = Some(now);
        record.updated_at = now;
        Ok(Some(record.clone()))
    }

    fn update_record_remote_url(
        &self,
        record_id: &str,
        remote_url: &str,
    ) -> Result<CredentialRecord> {
        let metadata = remote_metadata(remote_url)
            .ok_or_else(|| GitError::Credential("无法解析远端地址，不能绑定凭据".to_string()))?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| GitError::Credential("凭据缓存状态异常".to_string()))?;
        let now = next_record_timestamp(&index);
        let Some(record) = index.iter_mut().find(|record| record.id == record_id) else {
            return Err(GitError::Credential("凭据记录不存在".into()));
        };
        record.remote_url = remote_url.to_string();
        record.host = metadata.host_key;
        record.last_used = Some(now);
        record.updated_at = now;
        Ok(record.clone())
    }
}

pub struct KeyringCredentialStore {
    initialized: Mutex<bool>,
    storage: Arc<AppStorage>,
}

impl KeyringCredentialStore {
    pub fn new() -> Result<Self> {
        Ok(Self {
            initialized: Mutex::new(false),
            storage: Arc::new(AppStorage::open_default()?),
        })
    }

    pub fn new_with_recreated_database() -> Result<Self> {
        Ok(Self {
            initialized: Mutex::new(false),
            storage: Arc::new(AppStorage::recreate_default_after_failure()?),
        })
    }

    pub fn with_storage_path(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            initialized: Mutex::new(false),
            storage: Arc::new(AppStorage::open(path)?),
        })
    }

    pub fn with_storage(storage: Arc<AppStorage>) -> Self {
        Self {
            initialized: Mutex::new(false),
            storage,
        }
    }

    fn ensure_store(&self) -> Result<()> {
        let mut initialized = self
            .initialized
            .lock()
            .map_err(|_| GitError::Credential("系统凭据管理器初始化状态异常".into()))?;
        if !*initialized {
            keyring::use_native_store(false)
                .map_err(|err| GitError::Credential(format!("系统凭据管理器不可用：{err:?}")))?;
            *initialized = true;
        }
        Ok(())
    }

    fn load_index(&self) -> Result<CredentialIndex> {
        Ok(CredentialIndex {
            records: self.storage.load_credential_records()?,
        })
    }

    fn save_index(&self, index: &CredentialIndex) -> Result<()> {
        self.storage.save_credential_records(&index.records)
    }

    fn entry_for_record(record_id: &str, username: &str) -> Result<keyring_core::Entry> {
        keyring_core::Entry::new(&new_keyring_service(record_id), username)
            .map_err(|err| GitError::Credential(format!("系统凭据条目创建失败：{err:?}")))
    }

    fn old_user(request: &CredentialRequest) -> String {
        request
            .username_from_url
            .clone()
            .unwrap_or_else(|| "git".into())
    }

    fn old_service(request: &CredentialRequest) -> String {
        format!("{OLD_KEYRING_SERVICE_PREFIX}:{}", request.url)
    }

    fn get_old_and_migrate(&self, request: &CredentialRequest) -> Result<Option<StoredCredential>> {
        self.ensure_store()?;
        let user = Self::old_user(request);
        let entry = keyring_core::Entry::new(&Self::old_service(request), &user)
            .map_err(|err| GitError::Credential(format!("系统凭据条目创建失败：{err:?}")))?;
        let secret = match entry.get_password() {
            Ok(secret) => secret,
            Err(keyring_core::Error::NoEntry) => return Ok(None),
            Err(err) => return Err(GitError::Credential(format!("系统凭据读取失败：{err:?}"))),
        };

        let Some(mut credential) = GitCredential::from_old_storage(
            user.clone(),
            secret,
            request.allowed_types,
            CredentialScope::RemoteUrl,
        ) else {
            return Ok(None);
        };
        set_credential_save_scope(&mut credential, true, CredentialScope::RemoteUrl);
        let record = self.save_record(request, &credential)?;
        let _ = entry.delete_credential();
        Ok(Some(StoredCredential { record, credential }))
    }

    fn touch_record(&self, record_id: &str) -> Result<Option<CredentialRecord>> {
        let mut index = self.load_index()?;
        let now = next_record_timestamp(&index.records);
        let touched = index
            .records
            .iter_mut()
            .find(|record| record.id == record_id);
        let Some(touched) = touched else {
            return Ok(None);
        };
        touched.last_used = Some(now);
        touched.updated_at = now;
        let record = touched.clone();
        self.save_index(&index)?;
        Ok(Some(record))
    }
}

impl CredentialStore for KeyringCredentialStore {
    fn get(&self, request: &CredentialRequest) -> Result<Option<GitCredential>> {
        self.get_stored(request, &[])
            .map(|stored| stored.map(|stored| stored.credential))
    }

    fn get_stored(
        &self,
        request: &CredentialRequest,
        rejected_record_ids: &[String],
    ) -> Result<Option<StoredCredential>> {
        self.ensure_store()?;
        let index = self.load_index()?;
        let rejected = rejected_record_ids.iter().cloned().collect::<BTreeSet<_>>();
        let Some(record) = select_matching_record(&index.records, request, &rejected) else {
            return self.get_old_and_migrate(request);
        };

        let entry = Self::entry_for_record(&record.id, &record.username)?;
        let secret = match entry.get_password() {
            Ok(secret) => secret,
            Err(keyring_core::Error::NoEntry) => {
                self.delete_record(&record.id)?;
                return Ok(None);
            }
            Err(err) => return Err(GitError::Credential(format!("系统凭据读取失败：{err:?}"))),
        };
        let Some(credential) = GitCredential::from_record(&record, secret) else {
            return Ok(None);
        };
        let record = self.touch_record(&record.id)?.unwrap_or(record);
        Ok(Some(StoredCredential { record, credential }))
    }

    fn save(&self, request: &CredentialRequest, credential: &GitCredential) -> Result<()> {
        self.save_record(request, credential).map(|_| ())
    }

    fn save_record(
        &self,
        request: &CredentialRequest,
        credential: &GitCredential,
    ) -> Result<CredentialRecord> {
        self.ensure_store()?;
        let metadata = remote_metadata(&request.url)
            .ok_or_else(|| GitError::Credential("无法解析远端地址，不能保存凭据".to_string()))?;
        let mut index = self.load_index()?;
        let now = now_seconds();
        let display_name = credential.display_name().and_then(normalize_display_name);
        let record = if let Some(existing) = index.records.iter_mut().find(|record| {
            record.scope == credential.scope()
                && record.kind == credential.kind()
                && record.host == metadata.host_key
                && normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url)
                && record.username == credential.username()
                && record.key_path.as_deref() == credential.key_path()
        }) {
            existing.updated_at = now;
            existing.last_used = Some(now);
            if display_name.is_some() {
                existing.display_name = display_name.clone();
            }
            existing.clone()
        } else {
            let record = CredentialRecord {
                id: new_record_id(),
                display_name: display_name.clone(),
                scope: credential.scope(),
                kind: credential.kind(),
                host: metadata.host_key,
                remote_url: request.url.clone(),
                username: credential.username().to_string(),
                key_path: credential.key_path().map(str::to_string),
                created_at: now,
                updated_at: now,
                last_used: Some(now),
            };
            index.records.push(record.clone());
            record
        };

        let entry = Self::entry_for_record(&record.id, credential.username())?;
        entry
            .set_password(&credential.secret_for_keyring())
            .map_err(|err| GitError::Credential(format!("系统凭据写入失败：{err:?}")))?;
        self.save_index(&index)?;
        Ok(record)
    }

    fn delete(&self, request: &CredentialRequest, username: &str) -> Result<()> {
        let index = self.load_index()?;
        let ids = index
            .records
            .iter()
            .filter(|record| normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url) && record.username == username)
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.delete_record(&id)?;
        }

        self.ensure_store()?;
        let entry = keyring_core::Entry::new(&Self::old_service(request), username)
            .map_err(|err| GitError::Credential(format!("系统凭据条目创建失败：{err:?}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(err) => Err(GitError::Credential(format!("系统凭据删除失败：{err:?}"))),
        }
    }

    fn delete_record(&self, record_id: &str) -> Result<()> {
        self.ensure_store()?;
        let mut index = self.load_index()?;
        let removed = index
            .records
            .iter()
            .find(|record| record.id == record_id)
            .cloned();
        index.records.retain(|record| record.id != record_id);
        self.save_index(&index)?;
        if let Some(record) = removed {
            let entry = Self::entry_for_record(record_id, &record.username)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                Err(err) => {
                    return Err(GitError::Credential(format!("系统凭据删除失败：{err:?}")));
                }
            }
        }
        Ok(())
    }

    fn list_records(&self) -> Result<Vec<CredentialRecord>> {
        let mut records = self.load_index()?.records;
        sort_records(&mut records);
        Ok(records)
    }

    fn credential_for_record(&self, record_id: &str) -> Result<Option<GitCredential>> {
        self.ensure_store()?;
        let index = self.load_index()?;
        let Some(record) = index
            .records
            .into_iter()
            .find(|record| record.id == record_id)
        else {
            return Ok(None);
        };
        let entry = Self::entry_for_record(&record.id, &record.username)?;
        match entry.get_password() {
            Ok(secret) => Ok(GitCredential::from_record(&record, secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) => Err(GitError::Credential(format!("系统凭据读取失败：{err:?}"))),
        }
    }

    fn touch_record(&self, record_id: &str) -> Result<Option<CredentialRecord>> {
        KeyringCredentialStore::touch_record(self, record_id)
    }

    fn update_record_remote_url(
        &self,
        record_id: &str,
        remote_url: &str,
    ) -> Result<CredentialRecord> {
        self.ensure_store()?;
        let metadata = remote_metadata(remote_url)
            .ok_or_else(|| GitError::Credential("无法解析远端地址，不能绑定凭据".to_string()))?;
        let mut index = self.load_index()?;
        let now = next_record_timestamp(&index.records);
        let Some(record) = index
            .records
            .iter_mut()
            .find(|record| record.id == record_id)
        else {
            return Err(GitError::Credential("凭据记录不存在".into()));
        };
        record.remote_url = remote_url.to_string();
        record.host = metadata.host_key;
        record.last_used = Some(now);
        record.updated_at = now;
        let updated = record.clone();
        self.save_index(&index)?;
        Ok(updated)
    }
}

pub trait CredentialProvider: Send + Sync {
    fn credential_for(&self, request: CredentialRequest) -> Result<Option<GitCredential>>;
}

#[derive(Clone)]
pub struct PromptCredentialProvider {
    store: Arc<dyn CredentialStore>,
    prompt: Arc<dyn Fn(CredentialRequest) -> Result<Option<GitCredential>> + Send + Sync>,
}

impl PromptCredentialProvider {
    pub fn new(
        store: Arc<dyn CredentialStore>,
        prompt: impl Fn(CredentialRequest) -> Result<Option<GitCredential>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            prompt: Arc::new(prompt),
        }
    }

    pub fn memory_only(
        prompt: impl Fn(CredentialRequest) -> Result<Option<GitCredential>> + Send + Sync + 'static,
    ) -> Self {
        Self::new(Arc::new(MemoryCredentialStore::new()), prompt)
    }
}

impl CredentialProvider for PromptCredentialProvider {
    fn credential_for(&self, request: CredentialRequest) -> Result<Option<GitCredential>> {
        if let Some(credential) = self.store.get(&request)? {
            return Ok(Some(credential));
        }

        let credential = (self.prompt)(request.clone())?;
        if let Some(credential) = credential.as_ref()
            && credential.should_save()
        {
            self.store.save(&request, credential)?;
        }
        Ok(credential)
    }
}

pub fn test_credential_connection(
    store: &dyn CredentialStore,
    record: &CredentialRecord,
) -> Result<()> {
    let credential = store
        .credential_for_record(&record.id)?
        .ok_or_else(|| GitError::Credential("系统凭据管理器中未找到该凭据密文".to_string()))?;
    let request = CredentialRequest {
        url: record.remote_url.clone(),
        username_from_url: Some(record.username.clone()),
        allowed_types: match record.kind {
            StoredCredentialKind::HttpsUserPass => CredentialType::USER_PASS_PLAINTEXT,
            StoredCredentialKind::SshKey => CredentialType::SSH_KEY,
        },
        repo_path: None,
        remote_name: None,
        operation_id: None,
    };
    let temp = tempfile_dir_for_credential_test()?;
    let repo = git2::Repository::init_bare(&temp)?;
    let mut remote = repo.remote_anonymous(&record.remote_url)?;
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
        to_git_credential(&request, credential.clone())
    });
    {
        let _connection = remote
            .connect_auth(git2::Direction::Fetch, Some(callbacks), None)
            .map_err(GitError::from)?;
    }
    let _ = fs::remove_dir_all(temp);
    Ok(())
}

pub(crate) fn to_git_credential(
    request: &CredentialRequest,
    credential: GitCredential,
) -> std::result::Result<Cred, git2::Error> {
    match credential {
        GitCredential::UserPass {
            username, secret, ..
        } => Cred::userpass_plaintext(&username, &secret),
        GitCredential::SshPassphrase {
            username,
            private_key_path,
            passphrase,
            ..
        } => {
            if let Some(private_key_path) = private_key_path {
                Cred::ssh_key(
                    &username,
                    None,
                    std::path::Path::new(&private_key_path),
                    passphrase.as_deref(),
                )
            } else if request.allowed_types.contains(CredentialType::SSH_KEY) {
                Cred::ssh_key_from_agent(&username)
            } else {
                Err(git2::Error::from_str("远端不接受 SSH 密钥凭据"))
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CredentialIndex {
    #[serde(default)]
    records: Vec<CredentialRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteMetadata {
    host_key: String,
    protocol_family: ProtocolFamily,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtocolFamily {
    Https,
    Ssh,
}

fn select_matching_record(
    records: &[CredentialRecord],
    request: &CredentialRequest,
    rejected_record_ids: &BTreeSet<String>,
) -> Option<CredentialRecord> {
    let metadata = remote_metadata(&request.url)?;
    let kind = requested_kind(request, metadata.protocol_family)?;
    let mut candidates = records
        .iter()
        .filter(|record| !rejected_record_ids.contains(&record.id))
        .filter(|record| record.kind == kind)
        .filter(|record| record.host == metadata.host_key)
        .filter(|record| match record.scope {
            CredentialScope::RemoteUrl => {
                normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url)
            }
            CredentialScope::Host => true,
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| b.last_used.unwrap_or(0).cmp(&a.last_used.unwrap_or(0)))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    candidates.into_iter().next()
}

fn requested_kind(
    request: &CredentialRequest,
    protocol_family: ProtocolFamily,
) -> Option<StoredCredentialKind> {
    if protocol_family == ProtocolFamily::Ssh
        && request.allowed_types.contains(CredentialType::SSH_KEY)
    {
        return Some(StoredCredentialKind::SshKey);
    }
    if request
        .allowed_types
        .contains(CredentialType::USER_PASS_PLAINTEXT)
        || protocol_family == ProtocolFamily::Https
    {
        return Some(StoredCredentialKind::HttpsUserPass);
    }
    None
}

fn scope_rank(scope: CredentialScope) -> u8 {
    match scope {
        CredentialScope::RemoteUrl => 0,
        CredentialScope::Host => 1,
    }
}

fn sort_records(records: &mut [CredentialRecord]) {
    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.remote_url.cmp(&b.remote_url))
            .then_with(|| a.username.cmp(&b.username))
    });
}

pub fn credential_record_label(record: &CredentialRecord) -> String {
    if let Some(name) = record
        .display_name
        .as_deref()
        .and_then(normalize_display_name)
    {
        return name;
    }
    let scope = match record.scope {
        CredentialScope::RemoteUrl => "仅此远端",
        CredentialScope::Host => "同站点",
    };
    let kind = match record.kind {
        StoredCredentialKind::HttpsUserPass => "HTTPS",
        StoredCredentialKind::SshKey => "SSH",
    };
    format!("{kind} {scope} {}", record.username)
}

pub fn credential_scope_label(scope: CredentialScope) -> &'static str {
    match scope {
        CredentialScope::RemoteUrl => "仅此远端",
        CredentialScope::Host => "同站点",
    }
}

pub fn credential_kind_label(kind: StoredCredentialKind) -> &'static str {
    match kind {
        StoredCredentialKind::HttpsUserPass => "HTTPS 用户名/PAT",
        StoredCredentialKind::SshKey => "SSH 私钥",
    }
}

pub fn credential_display_target(record: &CredentialRecord) -> String {
    match record.scope {
        CredentialScope::RemoteUrl => record.remote_url.clone(),
        CredentialScope::Host => record.host.clone(),
    }
}

pub fn credential_key_filename(record: &CredentialRecord) -> String {
    record
        .key_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn credential_record_is_compatible_with_url(record: &CredentialRecord, url: &str) -> bool {
    let Some(metadata) = remote_metadata(url) else {
        return false;
    };
    matches!(
        (metadata.protocol_family, record.kind),
        (ProtocolFamily::Https, StoredCredentialKind::HttpsUserPass)
            | (ProtocolFamily::Ssh, StoredCredentialKind::SshKey)
    )
}

pub fn credential_record_matches_remote_url(record: &CredentialRecord, url: &str) -> bool {
    if !credential_record_is_compatible_with_url(record, url) {
        return false;
    }
    let Some(metadata) = remote_metadata(url) else {
        return false;
    };
    if record.host != metadata.host_key {
        return false;
    }
    match record.scope {
        CredentialScope::RemoteUrl => {
            normalize_remote_url(&record.remote_url) == normalize_remote_url(url)
        }
        CredentialScope::Host => true,
    }
}

fn remote_metadata(url: &str) -> Option<RemoteMetadata> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("http://") || lower.starts_with("https://") {
        let scheme_end = trimmed.find("://")?;
        let scheme = &lower[..scheme_end];
        let rest = &trimmed[scheme_end + 3..];
        let authority = rest.split(['/', '?', '#']).next()?.trim();
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let host_port = authority.trim_matches(['[', ']']);
        if host_port.is_empty() {
            return None;
        }
        return Some(RemoteMetadata {
            host_key: format!("https://{}", host_port.to_ascii_lowercase()),
            protocol_family: if scheme == "https" || scheme == "http" {
                ProtocolFamily::Https
            } else {
                ProtocolFamily::Ssh
            },
        });
    }

    if lower.starts_with("ssh://") {
        let rest = &trimmed[6..];
        let authority = rest.split(['/', '?', '#']).next()?.trim();
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        if authority.is_empty() {
            return None;
        }
        return Some(RemoteMetadata {
            host_key: format!("ssh://{}", authority.to_ascii_lowercase()),
            protocol_family: ProtocolFamily::Ssh,
        });
    }

    if let Some((left, _path)) = trimmed.split_once(':')
        && !left.contains('/')
    {
        let host = left.rsplit('@').next().unwrap_or(left);
        if !host.is_empty() {
            return Some(RemoteMetadata {
                host_key: format!("ssh://{}", host.to_ascii_lowercase()),
                protocol_family: ProtocolFamily::Ssh,
            });
        }
    }

    None
}

pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        if let Some(scheme_end) = trimmed.find("://") {
            let scheme = &lower[..scheme_end];
            let rest = &trimmed[scheme_end + 3..];
            let (authority, path) = rest
                .find(['/', '?', '#'])
                .map(|index| (&rest[..index], &rest[index..]))
                .unwrap_or((rest, ""));
            let authority = authority.rsplit('@').next().unwrap_or(authority);
            return format!("{scheme}://{}{}", authority.to_ascii_lowercase(), path.to_ascii_lowercase());
        }
    }
    lower
}

fn new_keyring_service(record_id: &str) -> String {
    format!("{KEYRING_SERVICE_PREFIX}:{record_id}")
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn next_record_timestamp(records: &[CredentialRecord]) -> i64 {
    let now = now_seconds();
    records
        .iter()
        .map(|record| record.updated_at.max(record.last_used.unwrap_or(0)))
        .max()
        .map(|latest| now.max(latest + 1))
        .unwrap_or(now)
}

fn new_record_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

fn normalize_display_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn set_credential_save_scope(
    credential: &mut GitCredential,
    save_to_keyring: bool,
    scope: CredentialScope,
) {
    match credential {
        GitCredential::UserPass {
            save_to_keyring: save,
            scope: existing_scope,
            ..
        }
        | GitCredential::SshPassphrase {
            save_to_keyring: save,
            scope: existing_scope,
            ..
        } => {
            *save = save_to_keyring;
            *existing_scope = scope;
        }
    }
}

fn tempfile_dir_for_credential_test() -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("khaslana-credential-test-{}", new_record_id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
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
}
