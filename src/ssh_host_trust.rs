//! SSH 主机身份校验；只读系统 known_hosts，应用 pin 以原子插入保存，绝不隐式替换。
use crate::{
    storage::AppStorage,
    types::{GitError, Result},
    ui_i18n::{LocalizedMessage, UiMessage},
};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const INVALID_ENDPOINT: UiMessage = UiMessage::new(
    "ssh_trust.invalid_endpoint",
    "无法安全解析 SSH 主机与端口。",
    "Could not safely resolve the SSH host and port.",
);
pub const REJECTED: UiMessage = UiMessage::new(
    "ssh_trust.rejected",
    "SSH 主机信任请求已拒绝或取消。",
    "SSH host trust was rejected or cancelled.",
);
pub const TIMED_OUT: UiMessage = UiMessage::new(
    "ssh_trust.timed_out",
    "SSH 主机信任请求已超时。",
    "SSH host trust request timed out.",
);
pub const CHANGED: UiMessage = UiMessage::new(
    "ssh_trust.changed",
    "SSH 主机 {endpoint} 的密钥与应用信任记录不同；原指纹：{old_fingerprint}；当前指纹：{new_fingerprint}。连接已阻止，请独立核验。",
    "The SSH key for {endpoint} differs from the application pin. Previous fingerprint: {old_fingerprint}; current fingerprint: {new_fingerprint}. Connection blocked; verify independently.",
);
pub const SYSTEM_CHANGED: UiMessage = UiMessage::new(
    "ssh_trust.system_changed",
    "SSH 主机 {endpoint} 的密钥与系统 known_hosts 记录不同；当前指纹：{new_fingerprint}。系统旧指纹未读取，连接已阻止，请独立核验。",
    "The SSH key for {endpoint} differs from system known_hosts. Current fingerprint: {new_fingerprint}. The previous system fingerprint was not read. Connection blocked; verify independently.",
);
fn changed(endpoint: &SshEndpoint, old: &[u8], new: &[u8]) -> GitError {
    GitError::Localized(LocalizedMessage::new(
        CHANGED,
        &[
            (
                "endpoint",
                &format!("[{}]:{}", endpoint.host, endpoint.port),
            ),
            ("old_fingerprint", &fingerprint(old)),
            ("new_fingerprint", &fingerprint(new)),
        ],
    ))
}
fn system_changed(endpoint: &SshEndpoint, raw: &[u8]) -> GitError {
    GitError::Localized(LocalizedMessage::new(
        SYSTEM_CHANGED,
        &[
            (
                "endpoint",
                &format!("[{}]:{}", endpoint.host, endpoint.port),
            ),
            ("new_fingerprint", &fingerprint(raw)),
        ],
    ))
}
pub const UNSAFE_KNOWN_HOSTS: UiMessage = UiMessage::new(
    "ssh_trust.unsafe_known_hosts",
    "无法安全验证系统 known_hosts（文件不可读、格式不支持或记录已撤销）。",
    "System known_hosts cannot be safely verified (unreadable file, unsupported format, or revoked entry).",
);
pub const MISSING_RAW_KEY: UiMessage = UiMessage::new(
    "ssh_trust.missing_raw_key",
    "SSH 后端未提供支持的原始主机公钥；连接已阻止。",
    "The SSH backend did not provide a supported raw host key; connection blocked.",
);
pub const UNSUPPORTED_PROXY: UiMessage = UiMessage::new(
    "ssh_trust.unsupported_proxy",
    "内置 SSH 后端不支持当前有效代理；连接已阻止，不会改为直连。",
    "The built-in SSH backend does not support the effective proxy; connection blocked without falling back to a direct connection.",
);
pub const RESOURCES: &[UiMessage] = &[
    INVALID_ENDPOINT,
    REJECTED,
    TIMED_OUT,
    CHANGED,
    SYSTEM_CHANGED,
    UNSAFE_KNOWN_HOSTS,
    MISSING_RAW_KEY,
    UNSUPPORTED_PROXY,
];
fn error(message: UiMessage) -> GitError {
    GitError::Localized(LocalizedMessage::new(message, &[]))
}
fn cert_error(message: UiMessage) -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Certificate,
        git2::ErrorClass::Ssh,
        &crate::ui_i18n::encode(message, &[]),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
}
impl SshEndpoint {
    pub fn from_url(url: &str) -> Result<Option<Self>> {
        let url = url.trim();
        let (authority, explicit_port) = if let Some(rest) = url
            .strip_prefix("ssh://")
            .or_else(|| url.strip_prefix("git+ssh://"))
        {
            (rest.split(['/', '?', '#']).next().unwrap_or(""), true)
        } else if url.contains("://") || !url.contains(':') || url.as_bytes().get(1) == Some(&b':')
        {
            return Ok(None);
        } else {
            let rest = url.rsplit_once('@').map_or(url, |(_, rest)| rest);
            let end = if rest.starts_with('[') {
                rest.find("]:").map(|i| i + 1)
            } else {
                rest.find(':')
            };
            let end = end.ok_or_else(|| error(INVALID_ENDPOINT))?;
            (&rest[..end], false)
        };
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let (host, port) = if authority.starts_with('[') {
            let end = authority.find(']').ok_or_else(|| error(INVALID_ENDPOINT))?;
            let suffix = &authority[end + 1..];
            let port = if suffix.is_empty() {
                22
            } else if explicit_port {
                suffix
                    .strip_prefix(':')
                    .and_then(|p| p.parse::<u16>().ok())
                    .filter(|p| *p > 0)
                    .ok_or_else(|| error(INVALID_ENDPOINT))?
            } else {
                return Err(error(INVALID_ENDPOINT));
            };
            (&authority[1..end], port)
        } else if let Some((host, port)) = authority.split_once(':') {
            (
                host,
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(|| error(INVALID_ENDPOINT))?,
            )
        } else {
            (authority, 22)
        };
        if host.is_empty()
            || host
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '\0' | '[' | ']'))
        {
            return Err(error(INVALID_ENDPOINT));
        }
        Ok(Some(Self {
            host: host.to_ascii_lowercase(),
            port,
        }))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnownHostStatus {
    Unknown,
    Match,
    Mismatch,
    Revoked,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostTrustDecision {
    Reject,
    TrustOnce,
    TrustAndSave,
}
/// 操作身份独立于 tab 和弹窗；作用域结束或业务取消后所有克隆立即失效。
#[derive(Clone, Debug)]
pub struct HostTrustOperation {
    pub id: u64,
    active: Arc<AtomicBool>,
    commit_lock: Arc<std::sync::Mutex<()>>,
    cancellation: Option<Arc<AtomicBool>>,
}
impl HostTrustOperation {
    pub fn cancel(&self) {
        // 与持久化提交共用同步边界：cancel 返回后不能出现新的 pin 写入。
        let _guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.active.store(false, Ordering::Release);
    }
    /// 闭包必须短小，不可回调 UI 或再次调用本 token 的 cancel/commit。
    /// 取消先取得锁则拒绝；提交先取得锁则先完成提交，再允许 cancel 返回。
    pub fn commit_if_active<T>(&self, commit: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self.commit_lock.lock().map_err(|_| error(REJECTED))?;
        if !self.is_active() {
            return Err(error(REJECTED));
        }
        commit()
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && !self
                .cancellation
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
    }
}
thread_local! {
    static CURRENT_OPERATION: std::cell::RefCell<Option<HostTrustOperation>> = const { std::cell::RefCell::new(None) };
}
/// 必须在实际执行任务的线程创建；Drop 同时覆盖成功、失败与 panic unwind。
pub struct HostTrustOperationScope {
    operation: HostTrustOperation,
    previous: Option<HostTrustOperation>,
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl HostTrustOperationScope {
    pub fn enter(cancellation: Option<Arc<AtomicBool>>) -> Self {
        let operation = HostTrustOperation {
            id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            active: Arc::new(AtomicBool::new(true)),
            commit_lock: Arc::new(std::sync::Mutex::new(())),
            cancellation,
        };
        let previous = CURRENT_OPERATION.with(|current| current.replace(Some(operation.clone())));
        Self {
            operation,
            previous,
            _not_send: std::marker::PhantomData,
        }
    }
    pub fn operation(&self) -> HostTrustOperation {
        self.operation.clone()
    }
}
impl Drop for HostTrustOperationScope {
    fn drop(&mut self) {
        self.operation.cancel();
        CURRENT_OPERATION.with(|current| {
            current.replace(self.previous.take());
        });
    }
}
fn current_operation() -> Option<HostTrustOperation> {
    CURRENT_OPERATION.with(|current| current.borrow().clone())
}

#[derive(Clone, Debug)]
pub struct HostTrustRequest {
    pub id: u64,
    pub operation: HostTrustOperation,
    pub endpoint: SshEndpoint,
    pub algorithm: String,
    pub fingerprint: String,
    pub known_host_status: KnownHostStatus,
    pub deadline: Instant,
    pub cancelled: Arc<AtomicBool>,
    pub response: mpsc::Sender<HostTrustDecision>,
}
impl HostTrustRequest {
    pub fn is_active(&self) -> bool {
        self.operation.is_active()
            && !self.cancelled.load(Ordering::Acquire)
            && Instant::now() < self.deadline
    }
}
/// 仅派发 owned 请求，不得等待 UI；后端负责有界等待。丢弃 response 等同拒绝。
pub trait HostTrustProvider: Send + Sync {
    fn request(&self, request: HostTrustRequest);
}
pub struct RejectHostTrust;
impl HostTrustProvider for RejectHostTrust {
    fn request(&self, request: HostTrustRequest) {
        let _ = request.response.send(HostTrustDecision::Reject);
    }
}
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
/// 探测型调用也必须向上传递安全拒绝，不可降级成普通不可用状态。
pub fn is_security_error(error: &GitError) -> bool {
    match error {
        GitError::Git(error) => error.code() == git2::ErrorCode::Certificate,
        GitError::Localized(message) | GitError::LocalizedCredential(message) => RESOURCES
            .iter()
            .any(|resource| resource.key == message.message.key),
        _ => false,
    }
}
pub fn fingerprint(raw: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(raw)))
}

pub fn ensure_supported_proxy(
    settings: &crate::proxy::NetworkProxySettings,
    url: Option<&str>,
) -> Result<()> {
    settings.validate()?;
    if let Some(url) = url
        && SshEndpoint::from_url(url)?.is_some()
    {
        let git_proxy = settings.mode == crate::proxy::NetworkProxyMode::System
            && git2::Config::open_default()
                .ok()
                .and_then(|config| config.get_string("http.proxy").ok())
                .is_some_and(|value| !value.trim().is_empty());
        if settings.proxy_url_for_target(url).is_some() || git_proxy {
            return Err(error(UNSUPPORTED_PROXY));
        }
    }
    Ok(())
}

pub fn install_certificate_check<'a>(
    callbacks: &mut git2::RemoteCallbacks<'a>,
    remote_url: Option<&str>,
    provider: Arc<dyn HostTrustProvider>,
) {
    let endpoint = remote_url.map(|url| SshEndpoint::from_url(url));
    callbacks.certificate_check(move |cert, hostname| {
        let Some(key) = cert.as_hostkey() else {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        };
        let endpoint = match &endpoint {
            Some(Ok(Some(endpoint))) => endpoint.clone(),
            _ => return Err(cert_error(INVALID_ENDPOINT)),
        };
        if !endpoint
            .host
            .eq_ignore_ascii_case(hostname.trim_matches(['[', ']']))
        {
            return Err(cert_error(INVALID_ENDPOINT));
        }
        let raw = key
            .hostkey()
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| cert_error(MISSING_RAW_KEY))?;
        let algorithm = key
            .hostkey_type()
            .map(|kind| kind.name())
            .filter(|name| *name != "unknown")
            .ok_or_else(|| cert_error(MISSING_RAW_KEY))?;
        verify(
            &endpoint,
            algorithm,
            raw,
            provider.as_ref(),
            Duration::from_secs(120),
        )
        .map_err(|err| {
            let message = match err {
                GitError::Localized(message) | GitError::LocalizedCredential(message) => {
                    let args: Vec<_> = message
                        .args
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str()))
                        .collect();
                    crate::ui_i18n::encode(message.message, &args)
                }
                other => other.to_string(),
            };
            git2::Error::new(
                git2::ErrorCode::Certificate,
                git2::ErrorClass::Ssh,
                &message,
            )
        })?;
        Ok(git2::CertificateCheckStatus::CertificateOk)
    });
}

fn verify(
    endpoint: &SshEndpoint,
    algorithm: &str,
    raw: &[u8],
    provider: &dyn HostTrustProvider,
    timeout: Duration,
) -> Result<()> {
    let system = system_known_hosts(endpoint, algorithm, raw);
    match system {
        KnownHostStatus::Mismatch => return Err(system_changed(endpoint, raw)),
        KnownHostStatus::Revoked | KnownHostStatus::Unsupported => {
            return Err(error(UNSAFE_KNOWN_HOSTS));
        }
        _ => {}
    }
    let storage = AppStorage::open_default()?;
    verify_with_storage(
        &storage, endpoint, algorithm, raw, provider, timeout, system,
    )
}

fn verify_with_storage(
    storage: &AppStorage,
    endpoint: &SshEndpoint,
    algorithm: &str,
    raw: &[u8],
    provider: &dyn HostTrustProvider,
    timeout: Duration,
    system: KnownHostStatus,
) -> Result<()> {
    match system {
        KnownHostStatus::Mismatch => return Err(system_changed(endpoint, raw)),
        KnownHostStatus::Revoked | KnownHostStatus::Unsupported => {
            return Err(error(UNSAFE_KNOWN_HOSTS));
        }
        _ => {}
    }
    if let Some((saved_algorithm, saved_raw)) =
        storage.load_ssh_host_pin(&endpoint.host, endpoint.port)?
    {
        return if saved_algorithm == algorithm && saved_raw == raw {
            Ok(())
        } else {
            Err(changed(endpoint, &saved_raw, raw))
        };
    }
    if system == KnownHostStatus::Match {
        return Ok(());
    }
    // 非调度器入口的作用域必须覆盖最终提交，不能在等待结束时先失效。
    let _fallback_scope = current_operation()
        .is_none()
        .then(|| HostTrustOperationScope::enter(None));
    let operation = current_operation().expect("host trust operation scope");
    let decision = request_decision(endpoint, algorithm, raw, provider, timeout)?;
    operation.commit_if_active(|| {
        // 即使只信任一次也重新读取，防止等待 UI 时另一任务保存了不同密钥。
        if let Some((saved_algorithm, saved_raw)) =
            storage.load_ssh_host_pin(&endpoint.host, endpoint.port)?
            && (saved_algorithm != algorithm || saved_raw != raw)
        {
            return Err(changed(endpoint, &saved_raw, raw));
        }
        match decision {
            HostTrustDecision::Reject => Err(error(REJECTED)),
            HostTrustDecision::TrustOnce => Ok(()),
            HostTrustDecision::TrustAndSave => {
                if storage.insert_ssh_host_pin(&endpoint.host, endpoint.port, algorithm, raw)? {
                    Ok(())
                } else {
                    match storage.load_ssh_host_pin(&endpoint.host, endpoint.port)? {
                        Some((_, old)) => Err(changed(endpoint, &old, raw)),
                        None => Err(error(REJECTED)),
                    }
                }
            }
        }
    })
}
fn request_decision(
    endpoint: &SshEndpoint,
    algorithm: &str,
    raw: &[u8],
    provider: &dyn HostTrustProvider,
    timeout: Duration,
) -> Result<HostTrustDecision> {
    // 非 TaskExecutor 调用也有请求本身的生命周期，不依赖 UI 存活。
    let fallback_scope = current_operation()
        .is_none()
        .then(|| HostTrustOperationScope::enter(None));
    let operation = current_operation().expect("host trust operation scope");
    let _fallback_scope = fallback_scope;
    if !operation.is_active() {
        return Err(error(REJECTED));
    }
    let (response, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + timeout;
    provider.request(HostTrustRequest {
        id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
        operation: operation.clone(),
        endpoint: endpoint.clone(),
        algorithm: algorithm.to_owned(),
        fingerprint: fingerprint(raw),
        known_host_status: KnownHostStatus::Unknown,
        deadline,
        cancelled: cancelled.clone(),
        response,
    });
    let result = loop {
        if cancelled.load(Ordering::Acquire) || !operation.is_active() {
            break Err(error(REJECTED));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(error(TIMED_OUT));
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(decision) => {
                if !operation.is_active() || cancelled.load(Ordering::Acquire) {
                    break Err(error(REJECTED));
                }
                if Instant::now() >= deadline {
                    break Err(error(TIMED_OUT));
                }
                break Ok(decision);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err(error(REJECTED)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    cancelled.store(true, Ordering::Release);
    result
}

fn system_known_hosts(endpoint: &SshEndpoint, algorithm: &str, raw: &[u8]) -> KnownHostStatus {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let ssh = PathBuf::from(home).join(".ssh");
        paths.push(ssh.join("known_hosts"));
        paths.push(ssh.join("known_hosts2"));
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("ProgramData") {
        paths.push(PathBuf::from(root).join("ssh/ssh_known_hosts"));
    }
    #[cfg(not(windows))]
    paths.extend([
        PathBuf::from("/etc/ssh/ssh_known_hosts"),
        PathBuf::from("/etc/ssh/ssh_known_hosts2"),
    ]);
    let mut result = KnownHostStatus::Unknown;
    for path in paths {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return KnownHostStatus::Unsupported,
        };
        match classify_known_hosts(&contents, endpoint, algorithm, raw) {
            KnownHostStatus::Unknown => {}
            KnownHostStatus::Match => result = KnownHostStatus::Match,
            other => return other,
        }
    }
    result
}

/// libssh2 解析哈希主机和标准公钥；不支持的通配、CA、撤销或损坏输入绝不当作未知主机。
pub fn classify_known_hosts(
    contents: &str,
    endpoint: &SshEndpoint,
    algorithm: &str,
    raw: &[u8],
) -> KnownHostStatus {
    use libssh2_sys::*;
    let key_type = match algorithm {
        "ssh-rsa" => LIBSSH2_KNOWNHOST_KEY_SSHRSA,
        "ssh-dss" => LIBSSH2_KNOWNHOST_KEY_SSHDSS,
        "ecdsa-sha2-nistp256" => LIBSSH2_KNOWNHOST_KEY_ECDSA_256,
        "ecdsa-sha2-nistp384" => LIBSSH2_KNOWNHOST_KEY_ECDSA_384,
        "ecdsa-sha2-nistp521" => LIBSSH2_KNOWNHOST_KEY_ECDSA_521,
        "ssh-ed25519" => LIBSSH2_KNOWNHOST_KEY_ED25519,
        _ => return KnownHostStatus::Unsupported,
    };
    // checkp 会把非默认端口回退成裸主机，破坏 host+port 隔离；改用精确 OpenSSH 名称。
    let host_name = if endpoint.port == 22 {
        endpoint.host.clone()
    } else {
        format!("[{}]:{}", endpoint.host, endpoint.port)
    };
    let Ok(host) = CString::new(host_name) else {
        return KnownHostStatus::Unsupported;
    };
    libssh2_sys::init();
    // session 仅用于 known_hosts 解析，不连接网络；每行独立容器确保错误和 marker 不被库静默跳过。
    unsafe {
        let session = libssh2_session_init_ex(None, None, None, std::ptr::null_mut());
        if session.is_null() {
            return KnownHostStatus::Unsupported;
        }
        let mut result = KnownHostStatus::Unknown;
        for line in contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let mut parts = line.split_whitespace();
            let first = parts.next().unwrap_or("");
            let (marker, names) = if first.starts_with('@') {
                (Some(first), parts.next().unwrap_or(""))
            } else {
                (None, first)
            };
            if names.is_empty() || names.contains(['*', '?', '!']) {
                result = KnownHostStatus::Unsupported;
                break;
            }
            let unmarked = if marker.is_some() {
                line[first.len()..].trim_start()
            } else {
                line
            };
            // 该 C API 虽有 len，内部仍读取结尾字节；必须提供 NUL 终止，避免窥读下一行。
            let Ok(unmarked) = CString::new(unmarked) else {
                result = KnownHostStatus::Unsupported;
                break;
            };
            let hosts = libssh2_knownhost_init(session);
            if hosts.is_null() {
                result = KnownHostStatus::Unsupported;
                break;
            }
            let parsed = libssh2_knownhost_readline(
                hosts,
                unmarked.as_ptr(),
                unmarked.as_bytes().len(),
                LIBSSH2_KNOWNHOST_FILE_OPENSSH,
            );
            let checked = if parsed == 0 {
                libssh2_knownhost_check(
                    hosts,
                    host.as_ptr(),
                    raw.as_ptr().cast(),
                    raw.len(),
                    LIBSSH2_KNOWNHOST_TYPE_PLAIN
                        | LIBSSH2_KNOWNHOST_KEYENC_RAW
                        | if marker.is_some() { 0 } else { key_type },
                    std::ptr::null_mut(),
                )
            } else {
                LIBSSH2_KNOWNHOST_CHECK_FAILURE
            };
            libssh2_knownhost_free(hosts);
            match checked {
                LIBSSH2_KNOWNHOST_CHECK_NOTFOUND => {}
                LIBSSH2_KNOWNHOST_CHECK_MATCH | LIBSSH2_KNOWNHOST_CHECK_MISMATCH
                    if marker.is_some() =>
                {
                    result = if marker == Some("@revoked") {
                        KnownHostStatus::Revoked
                    } else {
                        KnownHostStatus::Unsupported
                    };
                    break;
                }
                LIBSSH2_KNOWNHOST_CHECK_MATCH => result = KnownHostStatus::Match,
                LIBSSH2_KNOWNHOST_CHECK_MISMATCH => {
                    // 同算法多条记录允许轮换：只要有精确匹配就成立，不能由行序决定。
                    if result != KnownHostStatus::Match {
                        result = KnownHostStatus::Mismatch;
                    }
                }
                _ => {
                    result = KnownHostStatus::Unsupported;
                    break;
                }
            }
        }
        libssh2_session_free(session);
        result
    }
}

#[cfg(test)]
#[path = "tests/ssh_host_trust.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/ssh_local_integration.rs"]
mod local_integration_tests;
