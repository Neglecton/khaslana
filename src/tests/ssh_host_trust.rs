use super::*;
use base64::engine::general_purpose::STANDARD;

fn endpoint(port: u16) -> SshEndpoint {
    SshEndpoint {
        host: "example.com".into(),
        port,
    }
}
struct Decide(HostTrustDecision);
impl HostTrustProvider for Decide {
    fn request(&self, request: HostTrustRequest) {
        request.response.send(self.0).unwrap();
    }
}
#[test]
fn endpoints_keep_ports_and_strip_users() {
    for (url, host, port) in [
        ("git@EXAMPLE.com:repo.git", "example.com", 22),
        ("ssh://git@example.com:2222/repo", "example.com", 2222),
        ("ssh://git@[::1]:222/repo", "::1", 222),
        ("git@[::1]:repo", "::1", 22),
    ] {
        assert_eq!(
            SshEndpoint::from_url(url).unwrap().unwrap(),
            SshEndpoint {
                host: host.into(),
                port
            }
        );
    }
    assert!(
        SshEndpoint::from_url("https://example.com/a")
            .unwrap()
            .is_none()
    );
    assert!(SshEndpoint::from_url("C:/repo").unwrap().is_none());
    assert!(SshEndpoint::from_url("ssh://example.com:0/a").is_err());
    assert!(SshEndpoint::from_url("ssh://example.com:bad/a").is_err());
}
#[test]
fn raw_key_fingerprint_is_sha256_not_certificate_hash() {
    assert_eq!(
        fingerprint(b"abc"),
        "SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0"
    );
}
#[test]
fn known_hosts_exact_ports_and_changed_keys() {
    let raw = b"test raw key with sufficient length";
    let line = format!("example.com ssh-ed25519 {}\n", STANDARD.encode(raw));
    assert_eq!(
        classify_known_hosts(&line, &endpoint(22), "ssh-ed25519", raw),
        KnownHostStatus::Match
    );
    assert_eq!(
        classify_known_hosts(&line, &endpoint(2222), "ssh-ed25519", raw),
        KnownHostStatus::Unknown
    );
    assert_eq!(
        classify_known_hosts(&line, &endpoint(22), "ssh-ed25519", b"changed"),
        KnownHostStatus::Mismatch
    );
    let line = format!("[example.com]:2222 ssh-ed25519 {}\n", STANDARD.encode(raw));
    assert_eq!(
        classify_known_hosts(&line, &endpoint(2222), "ssh-ed25519", raw),
        KnownHostStatus::Match
    );
    assert_eq!(
        classify_known_hosts(&line, &endpoint(22), "ssh-ed25519", raw),
        KnownHostStatus::Unknown
    );
}
#[test]
fn hashed_known_hosts_are_checked_without_rewriting() {
    let line = format!(
        "|1|MDEyMzQ1Njc4OTAxMjM0NTY3ODk=|ExN2LhpSizfMMcs/YIDGzwsgBK0= ssh-ed25519 {}",
        STANDARD.encode(b"test key with sufficient length")
    );
    assert_eq!(
        classify_known_hosts(
            &line,
            &endpoint(22),
            "ssh-ed25519",
            b"test key with sufficient length"
        ),
        KnownHostStatus::Match
    );
    assert_eq!(
        classify_known_hosts(&line, &endpoint(2222), "ssh-ed25519", b"key"),
        KnownHostStatus::Unknown
    );
    assert_eq!(
        classify_known_hosts(&line, &endpoint(22), "ssh-ed25519", b"changed"),
        KnownHostStatus::Mismatch
    );
}
#[test]
fn unsafe_system_records_never_become_unknown() {
    let line = format!(
        "example.com ssh-ed25519 {}",
        STANDARD.encode(b"test key with sufficient length")
    );
    assert_eq!(
        classify_known_hosts(
            &format!("@revoked {line}"),
            &endpoint(22),
            "ssh-ed25519",
            b"key"
        ),
        KnownHostStatus::Revoked
    );
    assert_eq!(
        classify_known_hosts(
            &format!("@cert-authority {line}"),
            &endpoint(22),
            "ssh-ed25519",
            b"key"
        ),
        KnownHostStatus::Unsupported
    );
    assert_eq!(
        classify_known_hosts(
            "*.example.com ssh-ed25519 AAAA",
            &endpoint(22),
            "ssh-ed25519",
            b"key"
        ),
        KnownHostStatus::Unsupported
    );
    assert_eq!(
        classify_known_hosts("broken", &endpoint(22), "ssh-ed25519", b"key"),
        KnownHostStatus::Unsupported
    );
}
#[test]
fn default_provider_rejects_without_interaction() {
    let result = request_decision(
        &endpoint(22),
        "ssh-ed25519",
        b"key",
        &RejectHostTrust,
        Duration::from_secs(1),
    );
    assert!(result.unwrap().eq(&HostTrustDecision::Reject));
}
#[test]
fn timeout_and_cancellation_close_owned_request() {
    struct Cancel;
    impl HostTrustProvider for Cancel {
        fn request(&self, request: HostTrustRequest) {
            request.cancelled.store(true, Ordering::Release);
        }
    }
    assert!(
        request_decision(
            &endpoint(22),
            "ssh-ed25519",
            b"key",
            &Cancel,
            Duration::from_secs(1)
        )
        .unwrap_err()
        .is_message(REJECTED.key)
    );
    struct Hold(std::sync::Mutex<Option<HostTrustRequest>>);
    impl HostTrustProvider for Hold {
        fn request(&self, request: HostTrustRequest) {
            *self.0.lock().unwrap() = Some(request);
        }
    }
    let provider = Hold(std::sync::Mutex::new(None));
    assert!(
        request_decision(
            &endpoint(22),
            "ssh-ed25519",
            b"key",
            &provider,
            Duration::from_millis(1)
        )
        .unwrap_err()
        .is_message(TIMED_OUT.key)
    );
    assert!(
        provider
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .cancelled
            .load(Ordering::Acquire)
    );
}
#[test]
fn pins_persist_and_never_overwrite() {
    let storage = AppStorage::open_in_memory().unwrap();
    verify_with_storage(
        &storage,
        &endpoint(22),
        "ssh-ed25519",
        b"key",
        &Decide(HostTrustDecision::TrustAndSave),
        Duration::from_secs(1),
        KnownHostStatus::Unknown,
    )
    .unwrap();
    verify_with_storage(
        &storage,
        &endpoint(22),
        "ssh-ed25519",
        b"key",
        &RejectHostTrust,
        Duration::from_secs(1),
        KnownHostStatus::Unknown,
    )
    .unwrap();
    assert!(
        verify_with_storage(
            &storage,
            &endpoint(22),
            "ssh-ed25519",
            b"changed",
            &Decide(HostTrustDecision::TrustAndSave),
            Duration::from_secs(1),
            KnownHostStatus::Unknown
        )
        .unwrap_err()
        .is_message(CHANGED.key)
    );
    assert_eq!(
        storage
            .load_ssh_host_pin("example.com", 22)
            .unwrap()
            .unwrap()
            .1,
        b"key"
    );
    assert!(
        storage
            .load_ssh_host_pin("example.com", 2222)
            .unwrap()
            .is_none()
    );
}
#[test]
fn concurrent_pin_save_during_challenge_cannot_be_overridden() {
    struct Race<'a>(&'a AppStorage);
    impl HostTrustProvider for Race<'_> {
        fn request(&self, request: HostTrustRequest) {
            assert!(
                self.0
                    .insert_ssh_host_pin(
                        &request.endpoint.host,
                        request.endpoint.port,
                        "ssh-ed25519",
                        b"other"
                    )
                    .unwrap()
            );
            request
                .response
                .send(HostTrustDecision::TrustAndSave)
                .unwrap();
        }
    }
    let storage = AppStorage::open_in_memory().unwrap();
    let result = verify_with_storage(
        &storage,
        &endpoint(22),
        "ssh-ed25519",
        b"key",
        &Race(&storage),
        Duration::from_secs(1),
        KnownHostStatus::Unknown,
    );
    assert!(result.unwrap_err().is_message(CHANGED.key));
}
#[test]
fn trust_once_does_not_persist() {
    let storage = AppStorage::open_in_memory().unwrap();
    verify_with_storage(
        &storage,
        &endpoint(22),
        "ssh-ed25519",
        b"key",
        &Decide(HostTrustDecision::TrustOnce),
        Duration::from_secs(1),
        KnownHostStatus::Unknown,
    )
    .unwrap();
    assert!(
        storage
            .load_ssh_host_pin("example.com", 22)
            .unwrap()
            .is_none()
    );
}
#[test]
fn ssh_custom_proxy_is_refused_instead_of_direct() {
    let mut settings = crate::proxy::NetworkProxySettings::default();
    settings.mode = crate::proxy::NetworkProxyMode::Custom;
    settings.custom.socks5_proxy = "socks5://127.0.0.1:1080".into();
    assert!(
        ensure_supported_proxy(&settings, Some("git@example.com:a"))
            .unwrap_err()
            .is_message(UNSUPPORTED_PROXY.key)
    );
    assert!(ensure_supported_proxy(&settings, Some("https://example.com/a")).is_ok());
}
#[test]
fn completed_and_panicked_operations_invalidate_owned_requests() {
    let operation = {
        let scope = HostTrustOperationScope::enter(None);
        let operation = scope.operation();
        assert!(operation.is_active());
        operation
    };
    assert!(!operation.is_active());
    let retained = std::sync::Mutex::new(None);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scope = HostTrustOperationScope::enter(None);
        *retained.lock().unwrap() = Some(scope.operation());
        panic!("test operation unwind");
    }));
    assert!(!retained.lock().unwrap().as_ref().unwrap().is_active());
}
#[test]
fn business_cancellation_rejects_a_queued_trust_response() {
    struct CancelThenTrust(Arc<AtomicBool>);
    impl HostTrustProvider for CancelThenTrust {
        fn request(&self, request: HostTrustRequest) {
            assert!(request.is_active());
            self.0.store(true, Ordering::Release);
            request
                .response
                .send(HostTrustDecision::TrustAndSave)
                .unwrap();
            assert!(!request.is_active());
        }
    }
    let cancellation = Arc::new(AtomicBool::new(false));
    let _scope = HostTrustOperationScope::enter(Some(cancellation.clone()));
    let provider = CancelThenTrust(cancellation);
    assert!(
        request_decision(
            &endpoint(22),
            "ssh-ed25519",
            b"key",
            &provider,
            Duration::from_secs(1)
        )
        .unwrap_err()
        .is_message(REJECTED.key)
    );
}
#[test]
fn known_hosts_mixed_algorithms_and_rotation_ignore_line_order() {
    // SSH wire blob：string algorithm + Ed25519 的32字节公钥；RSA 为 exponent/modulus mpint。
    fn string(bytes: &[u8], output: &mut Vec<u8>) {
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(bytes);
    }
    let mut key = Vec::new();
    string(b"ssh-ed25519", &mut key);
    string(&[0x42; 32], &mut key);
    let mut rsa_key = Vec::new();
    string(b"ssh-rsa", &mut rsa_key);
    string(&[1, 0, 1], &mut rsa_key);
    string(&[0x61; 256], &mut rsa_key);
    let mut old_key = Vec::new();
    string(b"ssh-ed25519", &mut old_key);
    string(&[0x24; 32], &mut old_key);
    let key = key.as_slice();
    let ed = format!("example.com ssh-ed25519 {}", STANDARD.encode(key));
    let rsa = format!("example.com ssh-rsa {}", STANDARD.encode(rsa_key));
    let old = format!("example.com ssh-ed25519 {}", STANDARD.encode(old_key));
    for lines in [
        format!("{rsa}\n{ed}"),
        format!("{ed}\n{rsa}"),
        format!("{old}\n{ed}"),
        format!("{ed}\n{old}"),
    ] {
        assert_eq!(
            classify_known_hosts(&lines, &endpoint(22), "ssh-ed25519", key),
            KnownHostStatus::Match
        );
    }
    assert_eq!(
        classify_known_hosts(&rsa, &endpoint(22), "ssh-ed25519", key),
        KnownHostStatus::Unknown
    );
}
#[test]
fn cancel_and_commit_are_serialized() {
    let scope = HostTrustOperationScope::enter(None);
    let operation = scope.operation();
    operation.cancel();
    let wrote = AtomicBool::new(false);
    assert!(
        operation
            .commit_if_active(|| {
                wrote.store(true, Ordering::Release);
                Ok(())
            })
            .is_err()
    );
    assert!(!wrote.load(Ordering::Acquire));
    let scope = HostTrustOperationScope::enter(None);
    let operation = scope.operation();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let worker_operation = operation.clone();
    let worker = std::thread::spawn(move || {
        worker_operation.commit_if_active(|| {
            entered_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            Ok(())
        })
    });
    entered_rx.recv().unwrap();
    let cancel_operation = operation.clone();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let cancel = std::thread::spawn(move || {
        cancel_operation.cancel();
        cancel_tx.send(()).unwrap();
    });
    assert!(cancel_rx.recv_timeout(Duration::from_millis(20)).is_err());
    finish_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    cancel.join().unwrap();
    assert!(!operation.is_active());
    assert!(operation.commit_if_active(|| Ok(())).is_err());
}
#[test]
fn changed_pin_reports_both_fingerprints_and_endpoint() {
    let error = changed(&endpoint(2222), b"old", b"new");
    let text = error.display_in(crate::language::Language::En);
    assert!(text.contains("[example.com]:2222"));
    assert!(text.contains(&fingerprint(b"old")));
    assert!(text.contains(&fingerprint(b"new")));
}
#[test]
fn bilingual_resources_are_complete() {
    crate::ui_i18n::validate(RESOURCES).unwrap();
}
