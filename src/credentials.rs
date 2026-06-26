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
            .filter(|record| {
                normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url)
                    && record.username == username
            })
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
            .filter(|record| {
                normalize_remote_url(&record.remote_url) == normalize_remote_url(&request.url)
                    && record.username == username
            })
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
            return format!(
                "{scheme}://{}{}",
                authority.to_ascii_lowercase(),
                path.to_ascii_lowercase()
            );
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
#[path = "tests/credentials.rs"]
mod tests;
