//! 托管 SSH 密钥。口令只在调用栈中使用，不进入元数据或调试输出。
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use zeroize::Zeroizing;
use crate::storage::AppStorage;
use crate::types::{GitError, Result};
use crate::ui_i18n::{LocalizedMessage, UiMessage};

#[path = "ssh_keys/security.rs"]
mod security;

pub(crate) const INVALID: UiMessage = UiMessage::new("ssh_keys.invalid", "密钥名称、备注或口令无效", "Invalid key name, comment, or passphrase");
pub(crate) const UNSAFE: UiMessage = UiMessage::new("ssh_keys.unsafe", "密钥路径或权限不安全，已拒绝操作", "The key path or permissions are unsafe; operation refused");
pub(crate) const MISSING: UiMessage = UiMessage::new("ssh_keys.missing", "托管密钥不存在或已损坏", "The managed key is missing or damaged");
pub(crate) const REFERENCED: UiMessage = UiMessage::new("ssh_keys.referenced", "密钥仍被凭据引用，请先删除或修改相关凭据", "The key is still referenced by credentials; remove or edit those credentials first");
const FAILED: UiMessage = UiMessage::new("ssh_keys.failed", "SSH 密钥操作失败", "SSH key operation failed");

pub(crate) fn error(message: UiMessage) -> GitError {
    GitError::LocalizedCredential(LocalizedMessage::new(message, &[]))
}

/// 不包含私钥或口令，可安全持久化至普通 SQLite。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSshKey {
    pub id: String,
    pub display_name: String,
    pub comment: String,
    pub public_key: String,
    pub fingerprint: String,
    pub encrypted: bool,
    pub created_at: i64,
}

/// 无口令必须显式选择；空字符串不作为无口令的隐式替代。
pub enum KeyProtection {
    Passphrase(String),
    Unencrypted,
}

pub struct GenerateSshKeyRequest {
    pub display_name: String,
    pub comment: String,
    pub protection: KeyProtection,
}

#[derive(Clone)]
pub struct SshKeyManager {
    storage: Arc<AppStorage>,
}

impl SshKeyManager {
    pub fn new(storage: Arc<AppStorage>) -> Self { Self { storage } }

    pub fn list(&self) -> Result<Vec<ManagedSshKey>> { self.storage.load_managed_ssh_keys() }

    /// 生成与加密属于阻塞工作，由调用者经 TaskExecutor 调度。
    pub fn generate(&self, request: GenerateSshKeyRequest) -> Result<ManagedSshKey> {
        let passphrase = match request.protection {
            KeyProtection::Passphrase(value) if !value.is_empty() => Some(Zeroizing::new(value)),
            KeyProtection::Passphrase(_) => return Err(error(INVALID)),
            KeyProtection::Unencrypted => None,
        };
        if request.display_name.trim().is_empty() || request.display_name.len() > 256
            || request.comment.len() > 1024 || request.comment.contains(['\r', '\n', '\0'])
        { return Err(error(INVALID)); }
        let mut key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).map_err(|_| error(FAILED))?;
        key.set_comment(request.comment.clone());
        let public_key = key.public_key().to_openssh().map_err(|_| error(FAILED))?;
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let encrypted = passphrase.is_some();
        if let Some(passphrase) = passphrase.as_ref() {
            key = key.encrypt(&mut OsRng, passphrase.as_bytes()).map_err(|_| error(FAILED))?;
        }
        let pem = key.to_openssh(LineEnding::LF).map_err(|_| error(FAILED))?;
        let mut random = [0u8; 16];
        OsRng.try_fill_bytes(&mut random).map_err(|_| error(FAILED))?;
        let id = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let root = self.storage.managed_ssh_root()?;
        let parent = security::Directory::open_tree(&root)?;
        // 独占创建，不接管碰撞目录；失败仅清理本次实际创建的文件。
        let directory = parent.create_child(&id)?;
        let result = (|| {
            directory.write_new("id_ed25519", pem.as_bytes())?;
            directory.write_new("id_ed25519.pub", format!("{public_key}\n").as_bytes())?;
            let record = ManagedSshKey {
                id: id.clone(), display_name: request.display_name.trim().into(),
                comment: request.comment, public_key, fingerprint, encrypted,
                created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64,
            };
            self.storage.insert_managed_ssh_key(&record)?;
            Ok(record)
        })();
        if result.is_err() {
            let _ = directory.remove_file("id_ed25519");
            let _ = directory.remove_file("id_ed25519.pub");
        }
        drop(directory);
        if result.is_err() { let _ = parent.remove_child(&id); }
        result
    }

    pub fn private_key_path(&self, id: &str) -> Result<PathBuf> {
        let record = self.list()?.into_iter().find(|key| key.id == id).ok_or_else(|| error(MISSING))?;
        validate_id(id)?;
        let root = self.storage.managed_ssh_root()?;
        let parent = security::Directory::open_tree(&root)?;
        let directory = parent.open_child(id)?;
        let mut file = directory.open_file("id_ed25519")?;
        // 有界读取并验证公钥身份，损坏或替换后的文件不能继续用于认证。
        let mut bytes = Zeroizing::new(Vec::new());
        (&mut file).take(1024 * 1024).read_to_end(&mut bytes)?;
        let key = PrivateKey::from_openssh(&*bytes).map_err(|_| error(MISSING))?;
        // OpenSSH 加密私钥的明文公钥区不含备注；比较密码学身份而非含备注的文本。
        if key.fingerprint(HashAlg::Sha256).to_string() != record.fingerprint
            || key.is_encrypted() != record.encrypted { return Err(error(MISSING)); }
        Ok(root.join(id).join("id_ed25519"))
    }

    pub fn public_key(&self, id: &str) -> Result<String> {
        self.list()?.into_iter().find(|key| key.id == id).map(|key| key.public_key).ok_or_else(|| error(MISSING))
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        let root = self.storage.managed_ssh_root()?;
        // 与凭据保存使用同一连接锁和事务，检查引用后不能再插入新的引用。
        self.storage.delete_managed_ssh_key(id, || {
            let parent = security::Directory::open_tree(&root)?;
            let directory = match parent.open_child(id) {
                Ok(directory) => directory,
                Err(GitError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(err) => return Err(err),
            };
            // 删除可重试：上次文件删除成功、SQLite 提交失败时允许继续收尾。
            for name in ["id_ed25519", "id_ed25519.pub"] {
                match directory.remove_file(name) {
                    Ok(()) => (),
                    Err(GitError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => (),
                    Err(err) => return Err(err),
                }
            }
            drop(directory);
            parent.remove_child(id)
        })
    }
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) { Ok(()) }
    else { Err(error(UNSAFE)) }
}

/// 保存凭据时只识别本库中的托管路径，外部路径保留旧语义。
pub(crate) fn managed_id_for_path(root: &Path, path: &str) -> Option<String> {
    let relative = Path::new(path).strip_prefix(root).ok()?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 2 || parts[1].as_os_str() != "id_ed25519" { return None; }
    let id = parts[0].as_os_str().to_str()?;
    validate_id(id).ok()?;
    Some(id.to_owned())
}

/// 密钥复制与数据库发布共用提交边界；未提交时只回收本次独占创建的条目。
/// 持有安全目录句柄，回滚仍通过权限与重解析点检查，绝不递归删除目标 keys。
pub(crate) struct PendingManagedCopy {
    target: Option<security::Directory>,
    created: Vec<String>,
}

impl PendingManagedCopy {
    pub(crate) fn commit(mut self) { self.created.clear(); }
}

impl Drop for PendingManagedCopy {
    fn drop(&mut self) {
        let Some(target) = self.target.as_ref() else { return; };
        for id in &self.created {
            if let Ok(directory) = target.open_child(id) {
                let _ = directory.remove_file("id_ed25519");
                let _ = directory.remove_file("id_ed25519.pub");
                drop(directory);
                let _ = target.remove_child(id);
            }
        }
    }
}

/// 数据目录迁移保留稳定 id；拷贝过程中重新建立严格权限，绝不追随链接。
/// 调用者须在数据库发布、迁移标记等后续步骤全部成功后显式 commit。
pub(crate) fn prepare_managed_tree_copy(source: &Path, target: &Path) -> std::io::Result<PendingManagedCopy> {
    let operation = || -> Result<PendingManagedCopy> {
        match fs::symlink_metadata(source) {
            Ok(_) => (),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PendingManagedCopy { target: None, created: Vec::new() });
            }
            Err(err) => return Err(err.into()),
        }
        let source_dir = security::Directory::open_tree(source)?;
        let mut pending = PendingManagedCopy {
            target: Some(security::Directory::open_tree(target)?), created: Vec::new(),
        };
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            validate_id(&id)?;
            let from = source_dir.open_child(&id)?;
            let to = pending.target.as_ref().expect("复制守卫持有目标目录").create_child(&id)?;
            pending.created.push(id);
            for name in ["id_ed25519", "id_ed25519.pub"] {
                let mut data = Zeroizing::new(Vec::new());
                from.open_file(name)?.take(1024 * 1024).read_to_end(&mut data)?;
                to.write_new(name, &data)?;
            }
        }
        Ok(pending)
    };
    operation().map_err(|err| std::io::Error::other(err.to_string()))
}

pub(crate) fn copy_managed_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    prepare_managed_tree_copy(source, target)?.commit();
    Ok(())
}

#[cfg(test)]
#[path = "tests/ssh_keys.rs"]
mod tests;
