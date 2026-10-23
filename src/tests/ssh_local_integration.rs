//! 挂载于 ssh_host_trust 的私有子模块；不调用默认存储或读取真实 known_hosts。
//! 手动运行：cargo test --lib ssh_local_integration -- --ignored --nocapture
//! 仅需现有 Python + paramiko；不执行 ssh、ssh-keygen、git 或访问网络远端。
use super::*;
use crate::ssh_keys::{GenerateSshKeyRequest, KeyProtection, SshKeyManager};
use base64::engine::general_purpose::STANDARD;
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
};

struct LocalServer {
    child: Child,
    stdin: ChildStdin,
    replies: mpsc::Receiver<String>,
    port: u16,
    raw: Vec<u8>,
    events: PathBuf,
}

impl LocalServer {
    fn start(public_key: &Path, events: PathBuf) -> Self {
        // 可用绝对解释器路径覆盖；PATH 不参与任何 SSH 可执行程序查找。
        let python = std::env::var_os("KHASLANA_TEST_PYTHON").unwrap_or_else(|| "python".into());
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ssh-local-test-server.py");
        let mut command = Command::new(python);
        command.args(["-u", "-B"]).arg(script)
            .arg("--authorized-key").arg(public_key)
            .arg("--events").arg(&events)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = command.spawn().expect("需已安装 paramiko 的 Python；可设置 KHASLANA_TEST_PYTHON");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        // 有界读取避免 Python 启动错误使测试一直等待；进程由 Drop 强制回收。
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() { break; }
            }
        });
        let mut server = Self { child, stdin, replies, port: 0, raw: Vec::new(), events };
        server.read_reply("ready");
        server
    }

    fn read_reply(&mut self, expected: &str) {
        let line = self.replies.recv_timeout(Duration::from_secs(15))
            .expect("本机 SSH 测试服务未在期限内回应");
        let reply: serde_json::Value = serde_json::from_str(&line).expect("服务输出须为 JSON");
        assert_eq!(reply["event"], expected);
        assert_eq!(reply["algorithm"], "ssh-ed25519");
        let port = reply["port"].as_u64().unwrap() as u16;
        if self.port != 0 { assert_eq!(port, self.port); }
        self.port = port;
        self.raw = STANDARD.decode(reply["raw_key"].as_str().unwrap()).unwrap();
    }

    fn rotate(&mut self) {
        let previous = self.raw.clone();
        writeln!(self.stdin, "rotate").unwrap();
        self.stdin.flush().unwrap();
        self.read_reply("rotated");
        assert_ne!(self.raw, previous);
    }

    fn authenticated_connections(&self) -> usize {
        let data = std::fs::read_to_string(&self.events).unwrap_or_default();
        data.lines().filter(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["event"] == "authenticated_exec"
        }).count()
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        // 即使断言 panic，也不能留下监听本地端口的辅助进程。
        let _ = writeln!(self.stdin, "stop");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct DecisionRecorder {
    decision: HostTrustDecision,
    endpoint: SshEndpoint,
    fingerprint: String,
    requests: std::sync::Mutex<usize>,
}
impl DecisionRecorder {
    fn new(server: &LocalServer, decision: HostTrustDecision) -> Self {
        Self {
            decision,
            endpoint: SshEndpoint { host: "127.0.0.1".into(), port: server.port },
            fingerprint: fingerprint(&server.raw),
            requests: std::sync::Mutex::new(0),
        }
    }
    fn count(&self) -> usize { *self.requests.lock().unwrap() }
}
impl HostTrustProvider for DecisionRecorder {
    fn request(&self, request: HostTrustRequest) {
        assert_eq!(request.endpoint, self.endpoint);
        assert_eq!(request.algorithm, "ssh-ed25519");
        assert_eq!(request.fingerprint, self.fingerprint);
        assert_eq!(request.known_host_status, KnownHostStatus::Unknown);
        *self.requests.lock().unwrap() += 1;
        request.response.send(self.decision).unwrap();
    }
}

struct Attempt {
    result: std::result::Result<(), git2::Error>,
    trust_failure: Option<&'static str>,
    certificates: usize,
    credentials: usize,
}

fn connect(
    server: &LocalServer,
    storage: &AppStorage,
    private_key: &Path,
    passphrase: Option<&str>,
    provider: &dyn HostTrustProvider,
    repo: &git2::Repository,
) -> Attempt {
    let endpoint = SshEndpoint { host: "127.0.0.1".into(), port: server.port };
    let certificates = RefCell::new(0usize);
    let credentials = RefCell::new(0usize);
    let trust_failure = RefCell::new(None);
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.certificate_check(|certificate, hostname| {
        *certificates.borrow_mut() += 1;
        assert_eq!(hostname, "127.0.0.1");
        let key = certificate.as_hostkey().expect("必须走内置 libssh2 主机证书回调");
        let raw = key.hostkey().expect("后端须提供真实主机公钥");
        assert_eq!(raw, server.raw);
        let algorithm = key.hostkey_type().unwrap().name();
        assert_eq!(algorithm, "ssh-ed25519");
        // 空 known_hosts 输入明确代表隔离测试环境；绝不读取系统 HOME 或默认数据库。
        let system = classify_known_hosts("", &endpoint, algorithm, raw);
        assert_eq!(system, KnownHostStatus::Unknown);
        verify_with_storage(storage, &endpoint, algorithm, raw, provider,
            Duration::from_secs(2), system).map_err(|error| {
                *trust_failure.borrow_mut() = Some(if error.is_message(CHANGED.key) {
                    CHANGED.key
                } else if error.is_message(REJECTED.key) {
                    REJECTED.key
                } else {
                    panic!("非预期主机验证错误：{error}");
                });
                git2::Error::new(git2::ErrorCode::Certificate, git2::ErrorClass::Ssh,
                    "isolated SSH host verification rejected")
            })?;
        Ok(git2::CertificateCheckStatus::CertificateOk)
    });
    callbacks.credentials(|_, username, allowed| {
        let mut count = credentials.borrow_mut();
        *count += 1;
        if *count > 3 { return Err(git2::Error::from_str("test authentication retry limit")); }
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            git2::Cred::ssh_key(username.unwrap_or("git"), None, private_key, passphrase)
        } else if allowed.contains(git2::CredentialType::USERNAME) {
            git2::Cred::username("git")
        } else {
            Err(git2::Error::from_str("only explicit test public-key credentials are allowed"))
        }
    });
    let url = format!("ssh://git@127.0.0.1:{}/offline.git", server.port);
    let mut remote = repo.remote_anonymous(&url).unwrap();
    // 明确无代理；不继承用户 http.proxy，也不调用 SSH agent 或外部 ssh。
    // git2 0.21 的默认 kind 为 GIT_PROXY_NONE，没有 none() setter。
    let proxy = git2::ProxyOptions::new();
    let result = remote.connect_auth(git2::Direction::Fetch, Some(callbacks), Some(proxy))
        .map(|connection| drop(connection));
    Attempt {
        result,
        trust_failure: trust_failure.into_inner(),
        certificates: certificates.into_inner(),
        credentials: credentials.into_inner(),
    }
}

#[test]
#[ignore = "离线本机集成：需现有 Python/paramiko，统一构建后显式运行"]
fn ssh_local_integration_ed25519_auth_and_host_pinning() {
    for encrypted in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("isolated.sqlite");
        let storage = Arc::new(AppStorage::open(&database).unwrap());
        let manager = SshKeyManager::new(storage.clone());
        let passphrase = encrypted.then_some("offline-test-passphrase-only");
        let key = manager.generate(GenerateSshKeyRequest {
            display_name: "offline integration".into(),
            comment: "generated disposable test key".into(),
            protection: match passphrase {
                Some(value) => KeyProtection::Passphrase(value.into()),
                None => KeyProtection::Unencrypted,
            },
        }).unwrap();
        assert_eq!(key.encrypted, encrypted);
        let private_key = manager.private_key_path(&key.id).unwrap();
        assert!(private_key.starts_with(temporary.path()));
        let mut server = LocalServer::start(&private_key.with_extension("pub"),
            temporary.path().join("server-events.jsonl"));
        let repo = git2::Repository::init_bare(temporary.path().join("local.git")).unwrap();
        let reject = DecisionRecorder::new(&server, HostTrustDecision::Reject);
        let first = connect(&server, &storage, &private_key, passphrase, &reject, &repo);
        assert!(first.result.is_err(), "未知主机必须先拒绝");
        assert_eq!(first.trust_failure, Some(REJECTED.key));
        assert!(first.certificates > 0);
        assert_eq!(reject.count(), 1);
        assert_eq!(server.authenticated_connections(), 0);
        assert!(storage.load_ssh_host_pin("127.0.0.1", server.port).unwrap().is_none());

        let accept = DecisionRecorder::new(&server, HostTrustDecision::TrustAndSave);
        let accepted = connect(&server, &storage, &private_key, passphrase, &accept, &repo);
        accepted.result.expect("生成的 Ed25519 必须由 git2/libssh2 实际认证成功");
        assert!(accepted.certificates > 0 && accepted.credentials > 0);
        assert_eq!(accept.count(), 1);
        assert_eq!(server.authenticated_connections(), 1);
        let saved = storage.load_ssh_host_pin("127.0.0.1", server.port).unwrap().unwrap();
        assert_eq!(saved, ("ssh-ed25519".into(), server.raw.clone()));

        // 重开 SQLite，不能仅证明同一个连接的内存状态。
        drop(manager);
        drop(storage);
        let storage = AppStorage::open(&database).unwrap();
        let reconnect = connect(&server, &storage, &private_key, passphrase, &reject, &repo);
        reconnect.result.expect("持久化 pin 后应免确认重连");
        assert_eq!(reject.count(), 1, "已信任主机不应再请求确认");
        assert_eq!(server.authenticated_connections(), 2);

        if encrypted {
            let wrong = connect(&server, &storage, &private_key, Some("wrong-test-passphrase"), &reject, &repo);
            assert!(wrong.result.is_err(), "错误口令不可认证成功");
            assert!(wrong.credentials > 0);
            assert_eq!(wrong.trust_failure, None);
            assert_eq!(server.authenticated_connections(), 2);
        }

        server.rotate();
        let changed = DecisionRecorder::new(&server, HostTrustDecision::TrustAndSave);
        let blocked = connect(&server, &storage, &private_key, passphrase, &changed, &repo);
        assert!(blocked.result.is_err(), "同端口替换主机密钥必须拒绝");
        assert_eq!(blocked.trust_failure, Some(CHANGED.key));
        assert_eq!(changed.count(), 0, "不能通过再次确认覆盖 pin");
        assert_eq!(server.authenticated_connections(), 2);
        assert_eq!(storage.load_ssh_host_pin("127.0.0.1", server.port).unwrap().unwrap(), saved);
        eprintln!("PASS encrypted={encrypted}: reject -> explicit save -> authenticated reconnect -> changed host rejected");
    }
}
