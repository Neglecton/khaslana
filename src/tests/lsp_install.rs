//! JLS-T4 安装器与运行环境检测测试。
//!
//! fake HTTP 服务覆盖断流、错误 Content-Length、重定向、坏散列、取消；
//! 动态构造 zip / tar.gz 归档覆盖路径穿越、符号链接、条目限额、重复条目；
//! registry / 锁 / 卸载保护走临时目录。真实官方包与 JDT 启动复验见 ignored 测试。

use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::Compression;

use super::install::{
    ArchiveFormat, EnginePackageSpec, InstallError, InstallPhase, InstallProgress, InstallSettings, LspRegistry,
    PackageComponent, PluginInstallRecord, RuntimeSelection, builtin_package_specs,
    check_managed_activation, install_package, install_snapshot, load_registry,
    managed_runtime_java_home, save_registry_for_test, uninstall_package,
    JDT_LS_1_60_0_WINDOWS_X86_64, TEMURIN_JDK_21_WINDOWS_X86_64,
};
use super::plugins::JAVA_JDTLS_PLUGIN;
use super::runtime::java::{
    JdkDetectionError, JdkDiscoveryEnv, JdkProbe, JdkSource, discover_java_homes,
    layout_complete, parse_java_major, parse_show_settings_output,
};

// ---------------------------------------------------------------------------
// fake HTTP 服务器
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum FakeResponse {
    /// 200 + body（Content-Length 为 body 真实长度）。
    Ok(Vec<u8>),
    /// 404 Not Found（空 body）。
    NotFound,
    /// 声明的 Content-Length 与实际发送不符；close_early 时发一半即断流。
    WithDeclaredLength { declared: u64, body: Vec<u8>, close_early: bool },
    /// 302 → Location。
    Redirect { location: String },
    /// 200 + body，但按小块慢速发送（给取消测试留出窗口）。
    Slow { body: Vec<u8>, chunk: usize, delay_ms: u64 },
}

struct FakeServer {
    port: u16,
    script: Mutex<VecDeque<FakeResponse>>,
    requests: AtomicUsize,
}

impl FakeServer {
    fn start(responses: Vec<FakeResponse>) -> Arc<FakeServer> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(FakeServer {
            port,
            script: Mutex::new(responses.into()),
            requests: AtomicUsize::new(0),
        });
        let listener = Arc::new(listener);
        let accept_server = server.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let server = accept_server.clone();
                std::thread::spawn(move || server.handle(stream));
            }
        });
        server
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn handle(&self, mut stream: std::net::TcpStream) {
        // 读请求头直到空行（不解析内容，测试只关心响应脚本）。
        let mut buffer = [0u8; 4096];
        let mut request = Vec::new();
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request_line = String::from_utf8_lossy(
            request.split(|byte| *byte == b'\r').next().unwrap_or(&request),
        )
        .into_owned();
        println!("[fake-server] req: {request_line}");
        self.requests.fetch_add(1, Ordering::SeqCst);
        let response = {
            let mut script = self.script.lock().unwrap();
            script
                .pop_front()
                .unwrap_or_else(|| FakeResponse::Ok(b"exhausted".to_vec()))
        };
        match response {
            FakeResponse::NotFound => {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            FakeResponse::Ok(body) => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
            FakeResponse::WithDeclaredLength { declared, body, close_early } => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(head.as_bytes());
                if close_early {
                    // 发一半然后断流（模拟网络中断）。
                    let _ = stream.write_all(&body[..body.len() / 2]);
                    let _ = stream.flush();
                    return;
                }
                let _ = stream.write_all(&body);
            }
            FakeResponse::Redirect { location } => {
                let head = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(head.as_bytes());
            }
            FakeResponse::Slow { body, chunk, delay_ms } => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                for piece in body.chunks(chunk.max(1)) {
                    if stream.write_all(piece).is_err() {
                        return;
                    }
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
            }
        }
        let _ = stream.flush();
    }
}

// ---------------------------------------------------------------------------
// 动态归档构造
// ---------------------------------------------------------------------------

fn write_tar_gz(entries: &[(&str, &[u8])], dirs: &[&str], symlinks: &[(&str, &str)]) -> Vec<u8> {
    let buffer = std::io::Cursor::new(Vec::new());
    let encoder = flate2::write::GzEncoder::new(buffer, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for dir in dirs {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, dir, std::io::empty()).unwrap();
    }
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *content).unwrap();
    }
    for (name, target) in symlinks {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        builder.append_link(&mut header, name, target).unwrap();
    }
    builder
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .into_inner()
}

/// 直接写 GNU header 的 name 原始字节构造恶意路径条目
///（tar::Builder 的 append_data 会先拒绝 `..`/绝对路径，安全校验是本应用职责）。
fn write_tar_gz_raw_names(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buffer = std::io::Cursor::new(Vec::new());
    let encoder = flate2::write::GzEncoder::new(buffer, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        let gnu = header.as_gnu_mut().unwrap();
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() <= gnu.name.len(), "测试条目名超长");
        gnu.name[..name_bytes.len()].copy_from_slice(name_bytes);
        header.set_cksum();
        builder.append(&header, *content).unwrap();
    }
    builder
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .into_inner()
}

fn write_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buffer);
    let options: zip::write::SimpleFileOptions = Default::default();
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 构造指向 fake 服务器的测试 spec（非官方 plugin_id，绕过内置清单限制）。
fn fake_spec(
    url: String,
    body_sha: String,
    size: u64,
    format: ArchiveFormat,
    component: PackageComponent,
) -> EnginePackageSpec {
    EnginePackageSpec {
        package_id: Box::leak(
            format!(
                "fake-{}-test",
                match component {
                    PackageComponent::Engine => "engine",
                    PackageComponent::JavaRuntime => "runtime",
                }
            )
            .into_boxed_str(),
        ),
        plugin_id: "test-plugin",
        component,
        component_id: "fake",
        version: "9.9.9.test",
        os: "windows",
        arch: "x86_64",
        url: Box::leak(url.into_boxed_str()),
        mirror_url: None,
        sha256: Box::leak(body_sha.into_boxed_str()),
        size_bytes: size,
        format,
        archive_read_limit: 64 * 1024 * 1024,
        extract_total_limit: 64 * 1024 * 1024,
        extract_file_limit: 8 * 1024 * 1024,
        max_entries: 1000,
        required_entries: match component {
            PackageComponent::Engine => &["plugins"],
            PackageComponent::JavaRuntime => &["bin"],
        },
        min_service_jdk: 21,
        max_service_jdk: 21,
        license_name: "test",
        license_url: "https://example.invalid/license",
    }
}

fn test_settings() -> InstallSettings {
    InstallSettings {
        allowed_archive_hosts: Some(vec!["127.0.0.1".to_string()]),
        allowed_redirect_hosts: Some(vec!["127.0.0.1".to_string()]),
        ..InstallSettings::default()
    }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn no_progress(_: InstallProgress) {}

/// 组装标准引擎 fake 归档（唯一顶层目录 + plugins/ 入口 + core jar 版本匹配 spec）。
fn fake_engine_archive(top: &str) -> Vec<u8> {
    let core_jar = format!("{top}/plugins/org.eclipse.jdt.ls.core_9.9.9.test.jar");
    let launcher = format!("{top}/plugins/org.eclipse.equinox.launcher_1.8.0.jar");
    let config_ini = format!("{top}/config_win/config.ini");
    let config_dir = format!("{top}/config_win");
    write_tar_gz(
        &[
            (core_jar.as_str(), b"jar-bytes".as_slice()),
            (launcher.as_str(), b"launcher".as_slice()),
            (config_ini.as_str(), b"osgi.bundles=1".as_slice()),
        ],
        &[config_dir.as_str()],
        &[],
    )
}

fn serve_engine_ok(archive: Vec<u8>) -> (Arc<FakeServer>, EnginePackageSpec) {
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/engine.tar.gz"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    (server, spec)
}

fn install_failing(spec: &EnginePackageSpec) -> InstallError {
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    install_package(spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap_err()
}


#[test]
fn install_engine_package_publishes_and_activates() {
    let archive = fake_engine_archive("jdt-language-server-9.9.9.test");
    let (_server, spec) = serve_engine_ok(archive);
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    let outcome =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, Some(&no_progress))
            .unwrap();

    // 唯一顶层目录被剥掉，包根内直接是 plugins/config_win。
    assert!(outcome.package_root.join("plugins").is_dir());
    assert!(outcome
        .package_root
        .join("plugins/org.eclipse.jdt.ls.core_9.9.9.test.jar")
        .is_file());
    assert_eq!(
        outcome.engine_version.as_deref(),
        Some("9.9.9.test"),
        "引擎版本必须从 core jar 名提取"
    );
    // 激活记录与所有权标记。
    let registry = load_registry(lsp_root.path()).unwrap();
    let record = registry.plugins.get("test-plugin").unwrap();
    let engine = record.engine.as_ref().unwrap();
    assert_eq!(engine.version, "9.9.9.test");
    assert!(engine.path.replace('\\', "/").starts_with("packages/test-plugin/9.9.9.test/"));
    assert!(outcome.package_root.join(".khaslana-owned.json").is_file());
    // staging 已清空（只留目录）。
    let staging_leftover: Vec<_> =
        lsp_root.path().join("staging").read_dir().unwrap().collect();
    assert!(staging_leftover.is_empty());
}

#[test]
fn install_runtime_package_lands_in_runtimes_dir() {
    let archive = write_zip(&[
        ("jdk-9/bin/java.exe", b"java".as_slice()),
        ("jdk-9/bin/javac.exe", b"javac".as_slice()),
        ("jdk-9/release", b"JAVA_VERSION=\"9\"".as_slice()),
    ]);
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/jdk.zip"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::Zip,
        PackageComponent::JavaRuntime,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    let outcome =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
    assert!(outcome.package_root.join("bin/java.exe").is_file());
    let snapshot = install_snapshot(lsp_root.path(), "test-plugin").unwrap();
    assert!(matches!(snapshot.runtime, Some(RuntimeSelection::Managed(_))));
    assert!(snapshot.engine.is_none());
    let registry = load_registry(lsp_root.path()).unwrap();
    let runtime = registry.plugins["test-plugin"].runtime.as_ref().unwrap();
    match runtime {
        RuntimeSelection::Managed(component) => {
            assert_eq!(component.package_id, spec.package_id);
        }
        other => panic!("期望 Managed 运行环境，实际 {other:?}"),
    }
}

#[test]
fn duplicate_install_is_rejected() {
    let archive = fake_engine_archive("top");
    let (_server, spec) = serve_engine_ok(archive);
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
    let error =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap_err();
    assert!(matches!(error, InstallError::AlreadyInstalled(_)));
}

#[test]
fn sha256_mismatch_is_rejected() {
    let archive = fake_engine_archive("top");
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/engine.tar.gz"),
        "0".repeat(64), // 散列错误，尺寸正确 → 走到散列核对。
        archive.len() as u64,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::Sha256Mismatch { .. }), "实际：{error}");
}

#[test]
fn truncated_stream_fails_and_cleans_staging() {
    let archive = fake_engine_archive("top");
    let server = FakeServer::start(vec![FakeResponse::WithDeclaredLength {
        declared: archive.len() as u64,
        body: archive,
        close_early: true,
    }]);
    let spec = fake_spec(
        server.url("/engine.tar.gz"),
        "0".repeat(64),
        0, // 尺寸核对必然失败（实际读到一半）。
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    let error =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap_err();
    assert!(
        matches!(error, InstallError::SizeMismatch { .. } | InstallError::Network(_)),
        "实际错误：{error}"
    );
    let staging_leftover: Vec<_> =
        lsp_root.path().join("staging").read_dir().unwrap().collect();
    assert!(staging_leftover.is_empty());
    assert!(load_registry(lsp_root.path()).unwrap().plugins.is_empty());
}

#[test]
fn content_length_over_limit_is_rejected_before_body() {
    let archive = fake_engine_archive("top");
    let server = FakeServer::start(vec![FakeResponse::WithDeclaredLength {
        declared: 200 * 1024 * 1024, // 声明 200MB > archive_read_limit 64MB。
        body: archive,
        close_early: false,
    }]);
    let spec = fake_spec(
        server.url("/engine.tar.gz"),
        "0".repeat(64),
        0,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let error = install_failing(&spec);
    assert!(
        matches!(error, InstallError::ExtractSizeLimit { .. }),
        "实际：{error}"
    );
}

#[test]
fn download_is_cancelled_mid_stream() {
    let archive = vec![0xA5u8; 4 * 1024 * 1024];
    let server = FakeServer::start(vec![FakeResponse::Slow {
        body: archive,
        chunk: 32 * 1024,
        delay_ms: 5,
    }]);
    let spec = fake_spec(
        server.url("/big.bin"),
        "0".repeat(64),
        4 * 1024 * 1024,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let spinner = cancel.clone();
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        spinner.store(true, Ordering::SeqCst);
    });
    let error =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap_err();
    watcher.join().unwrap();
    assert!(matches!(error, InstallError::Cancelled), "实际错误：{error}");
}

#[test]
fn redirect_outside_whitelist_is_rejected() {
    let server = FakeServer::start(vec![FakeResponse::Redirect {
        location: "https://evil.example.com/engine.tar.gz".to_string(),
    }]);
    let spec = fake_spec(
        server.url("/engine.tar.gz"),
        "0".repeat(64),
        0,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::UntrustedHost { .. }), "实际：{error}");
}

#[test]
fn redirect_chain_within_whitelist_reaches_archive() {
    let archive = fake_engine_archive("top");
    let origin = FakeServer::start(vec![
        FakeResponse::Redirect { location: "/real/engine.tar.gz".to_string() },
        FakeResponse::Ok(archive.clone()),
    ]);
    let spec = fake_spec(
        origin.url("/engine.tar.gz"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
}

#[test]
fn too_many_redirects_are_rejected() {
    let server = FakeServer::start(
        (0..20)
            .map(|_| FakeResponse::Redirect { location: "/loop".to_string() })
            .collect(),
    );
    let spec = fake_spec(
        server.url("/start"),
        "0".repeat(64),
        0,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    let error = install_failing(&spec);
    assert!(
        matches!(error, InstallError::TooManyRedirects(5) | InstallError::UntrustedHost { .. }),
        "实际：{error}"
    );
}

// ---------------------------------------------------------------------------
// 解压守卫
// ---------------------------------------------------------------------------

#[test]
fn traversal_entry_is_rejected() {
    let archive = write_tar_gz_raw_names(&[("../evil.txt", b"pwn".as_slice())]);
    let (_server, spec) = serve_engine_ok(archive);
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::ArchivePathUnsafe(_)), "实际：{error}");
}

#[test]
fn symlink_entry_is_rejected() {
    let archive = write_tar_gz(&[], &[], &[("top/evil", "/etc/passwd")]);
    let (_server, spec) = serve_engine_ok(archive);
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::LinkEntryRejected(_)), "实际：{error}");
}

#[test]
fn entry_count_limit_is_enforced() {
    let mut contents: Vec<Vec<u8>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for i in 0..8 {
        contents.push(vec![i as u8; 8]);
        names.push(format!("top/plugins/file{i}.txt"));
    }
    let entries: Vec<(&str, &[u8])> = names
        .iter()
        .zip(&contents)
        .map(|(name, content)| (name.as_str(), content.as_slice()))
        .collect();
    let archive = write_tar_gz(&entries, &[], &[]);
    let (_server, mut spec) = serve_engine_ok(archive);
    spec.max_entries = 2;
    let error = install_failing(&spec);
    assert!(
        matches!(error, InstallError::EntryCountLimit { limit: 2 }),
        "实际：{error}"
    );
}

#[test]
fn duplicate_entries_are_rejected() {
    let archive = write_tar_gz(
        &[("top/plugins/a.txt", b"one".as_slice()), ("top/plugins/a.txt", b"two".as_slice())],
        &[],
        &[],
    );
    let (_server, spec) = serve_engine_ok(archive);
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::DuplicateEntry(_)), "实际：{error}");
}

#[test]
fn unsafe_paths_are_rejected_in_tar() {
    for raw in ["/abs/evil.txt", "C:/evil.txt", "..\\evil.txt"] {
        let archive = write_tar_gz_raw_names(&[(
            Box::leak(raw.to_string().into_boxed_str()),
            b"x".as_slice(),
        )]);
        let (_server, spec) = serve_engine_ok(archive);
        let error = install_failing(&spec);
        assert!(
            matches!(error, InstallError::ArchivePathUnsafe(_)),
            "路径 {raw} 应被拒绝，实际：{error}"
        );
    }
}

#[test]
fn zip_traversal_entry_is_rejected() {
    let archive = write_zip(&[("../evil.txt", b"pwn".as_slice())]);
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/jdk.zip"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::Zip,
        PackageComponent::JavaRuntime,
    );
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::ArchivePathUnsafe(_)), "实际：{error}");
}

#[test]
fn extract_total_limit_is_enforced() {
    let big = vec![0u8; 1024];
    let archive = write_tar_gz(
        &[
            ("top/plugins/a.bin", &big[..]),
            ("top/plugins/b.bin", &big[..]),
        ],
        &[],
        &[],
    );
    let (_server, mut spec) = serve_engine_ok(archive);
    spec.extract_total_limit = 1500; // 两个 1024 文件必然超。
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::ExtractSizeLimit { .. }), "实际：{error}");
}

#[test]
fn missing_required_entry_fails() {
    let archive = write_tar_gz(&[("top/readme.txt", b"no plugins dir".as_slice())], &[], &[]);
    let (_server, spec) = serve_engine_ok(archive);
    let error = install_failing(&spec);
    assert!(matches!(error, InstallError::EntryMissing(_)), "实际：{error}");
}

#[test]
fn engine_version_mismatch_fails() {
    let archive = write_tar_gz(
        &[("top/plugins/org.eclipse.jdt.ls.core_0.0.0.wrong.jar", b"jar".as_slice())],
        &["top/plugins"],
        &[],
    );
    let (_server, spec) = serve_engine_ok(archive);
    let error = install_failing(&spec);
    assert!(
        matches!(error, InstallError::EngineVersionMismatch { .. }),
        "实际：{error}"
    );
}

// ---------------------------------------------------------------------------
// 锁与卸载
// ---------------------------------------------------------------------------

#[test]
fn stale_lock_is_taken_over() {
    let archive = fake_engine_archive("top");
    let (_server, spec) = serve_engine_ok(archive);
    let lsp_root = tempfile::tempdir().unwrap();
    let staging = lsp_root.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("install.lock"), "stale").unwrap();
    let mut settings = test_settings();
    settings.stale_lock_after = Duration::ZERO;
    let cancel = no_cancel();
    install_package(&spec, lsp_root.path(), &settings, &cancel, None).unwrap();
}

#[test]
fn fresh_lock_blocks_concurrent_install() {
    let lsp_root = tempfile::tempdir().unwrap();
    let staging = lsp_root.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("install.lock"), "fresh").unwrap();
    let archive = fake_engine_archive("top");
    let (_server, spec) = serve_engine_ok(archive);
    let cancel = no_cancel();
    let error =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap_err();
    assert!(matches!(error, InstallError::LockHeld), "实际：{error}");
}

#[test]
fn uninstall_removes_managed_package_and_updates_registry() {
    let archive = write_zip(&[
        ("jdk-9/bin/java.exe", b"java".as_slice()),
        ("jdk-9/bin/javac.exe", b"javac".as_slice()),
    ]);
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/jdk.zip"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::Zip,
        PackageComponent::JavaRuntime,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    let outcome =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
    assert!(outcome.package_root.is_dir());
    uninstall_package(lsp_root.path(), "test-plugin", &spec.package_id).unwrap();
    assert!(!outcome.package_root.exists());
    let snapshot = install_snapshot(lsp_root.path(), "test-plugin").unwrap();
    assert!(snapshot.runtime.is_none());
    assert!(snapshot.engine.is_none());
}

#[test]
fn uninstall_rejects_missing_ownership_marker() {
    let archive = write_zip(&[
        ("jdk-9/bin/java.exe", b"java".as_slice()),
        ("jdk-9/bin/javac.exe", b"javac".as_slice()),
    ]);
    let server = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let spec = fake_spec(
        server.url("/jdk.zip"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::Zip,
        PackageComponent::JavaRuntime,
    );
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    let outcome =
        install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
    // 伪造：删除所有权标记后卸载必须拒绝（防止误删外部目录）。
    std::fs::remove_file(outcome.package_root.join(".khaslana-owned.json")).unwrap();
    let error = uninstall_package(lsp_root.path(), "test-plugin", &spec.package_id).unwrap_err();
    assert!(matches!(error, InstallError::NotOwnedByApp(_)), "实际：{error}");
    assert!(outcome.package_root.exists());
}

#[test]
fn uninstall_unknown_package_fails() {
    let lsp_root = tempfile::tempdir().unwrap();
    let error = uninstall_package(lsp_root.path(), "test-plugin", "ghost").unwrap_err();
    assert!(matches!(error, InstallError::UnknownInstalledPackage(_)));
}

// ---------------------------------------------------------------------------
// registry 与托管激活
// ---------------------------------------------------------------------------

#[test]
fn registry_roundtrip_and_missing_file_defaults() {
    let lsp_root = tempfile::tempdir().unwrap();
    let empty = load_registry(lsp_root.path()).unwrap();
    assert!(empty.plugins.is_empty());
    let mut registry = LspRegistry::default();
    registry.plugins.insert(
        "test-plugin".to_string(),
        PluginInstallRecord {
            plugin_id: "test-plugin".to_string(),
            engine: None,
            runtime: Some(RuntimeSelection::External {
                java_home: r"D:\jdk".to_string(),
            }),
        },
    );
    save_registry_for_test(lsp_root.path(), &registry).unwrap();
    let loaded = load_registry(lsp_root.path()).unwrap();
    assert_eq!(
        loaded.plugins["test-plugin"].runtime,
        Some(RuntimeSelection::External {
            java_home: r"D:\jdk".to_string()
        })
    );
}

#[test]
fn managed_runtime_java_home_is_none_without_registry() {
    let lsp_root = tempfile::tempdir().unwrap();
    assert!(managed_runtime_java_home(lsp_root.path(), "test-plugin").unwrap().is_none());
}

#[test]
fn managed_activation_requires_release_build_in_compatibility_list() {
    // snapshot 版本（development_build=true）不得通过托管激活校验。
    assert!(check_managed_activation("1.61.0.202609031315", 21).is_err());
    // 未登记版本拒绝。
    assert!(check_managed_activation("9.9.9", 21).is_err());
}

// ---------------------------------------------------------------------------
// 内置清单完整性
// ---------------------------------------------------------------------------

#[test]
fn builtin_specs_match_descriptor_packages() {
    let descriptor_ids: BTreeSet<&str> = JAVA_JDTLS_PLUGIN.engine_packages.iter().copied().collect();
    let spec_ids: BTreeSet<&str> =
        builtin_package_specs().iter().map(|spec| spec.package_id).collect();
    assert_eq!(
        descriptor_ids, spec_ids,
        "descriptor.engine_packages 与内置清单必须一一对应"
    );
}

#[test]
fn builtin_specs_have_strict_sha_and_https() {
    for spec in builtin_package_specs() {
        assert!(spec.url.starts_with("https://"), "{}", spec.package_id);
        assert_eq!(spec.sha256.len(), 64, "{}", spec.package_id);
        assert!(spec.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(spec.size_bytes > 0);
        assert!(!spec.required_entries.is_empty());
    }
}

#[test]
fn jdt_1_60_0_release_entry_is_registered_for_managed_activation() {
    let matched = JAVA_JDTLS_PLUGIN
        .compatibility
        .iter()
        .any(|entry| entry.engine_version == "1.60.0.202606262232" && !entry.development_build);
    assert!(matched);
}

// ---------------------------------------------------------------------------
// JDK 检测
// ---------------------------------------------------------------------------

fn static_probe(version: &'static str, arch: &'static str) -> impl Fn(&Path) -> JdkProbe {
    move |_| JdkProbe {
        version: Some(version.to_string()),
        arch: Some(arch.to_string()),
        error: None,
    }
}

fn make_fake_jdk(dir: &Path, name: &str, with_javac: bool) -> PathBuf {
    let jdk = dir.join(name);
    std::fs::create_dir_all(jdk.join("bin")).unwrap();
    std::fs::write(jdk.join("bin/java.exe"), b"").unwrap();
    if with_javac {
        std::fs::write(jdk.join("bin/javac.exe"), b"").unwrap();
    }
    jdk
}

#[test]
fn discover_java_homes_orders_sources_and_dedupes() {
    let dir = tempfile::tempdir().unwrap();
    let jdk = make_fake_jdk(dir.path(), "jdk", true);
    // managed 与 JAVA_HOME 指向同一路径 → 只保留优先级高的 Managed。
    let env = JdkDiscoveryEnv {
        java_home: Some(jdk.clone()),
        path_entries: Vec::new(),
    };
    let candidates =
        discover_java_homes(None, Some(&jdk), None, &env, &static_probe("21.0.12", "amd64"))
            .unwrap();
    assert_eq!(candidates.len(), 1, "同一路径应去重：{candidates:?}");
    assert_eq!(candidates[0].source, JdkSource::Managed);
    assert!(candidates[0].complete);
    assert_eq!(candidates[0].probe.major_version(), Some(21));
    // static_probe 返回原始值；normalize 行为由 show_settings_output_parsing 覆盖。
    assert_eq!(candidates[0].probe.arch.as_deref(), Some("amd64"));
}

#[test]
fn discover_java_homes_rejects_invalid_explicit_path() {
    let env = JdkDiscoveryEnv::default();
    let error = discover_java_homes(
        Some(Path::new("Z:/definitely/not/here")),
        None,
        None,
        &env,
        &static_probe("21", "x86_64"),
    )
    .unwrap_err();
    assert!(matches!(error, JdkDetectionError::InvalidExplicitPath(_)));
}

#[test]
fn discover_java_homes_excludes_project_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    make_fake_jdk(&project, "embedded", true);
    // PATH 中的 bin 目录是 project/embedded/bin。
    let env = JdkDiscoveryEnv {
        java_home: None,
        path_entries: vec![project.join("embedded").join("bin")],
    };
    let candidates = discover_java_homes(
        None,
        None,
        Some(dir.path().join("project").as_path()),
        &env,
        &static_probe("21", "x86_64"),
    )
    .unwrap();
    assert!(candidates.is_empty(), "项目内候选必须被排除：{candidates:?}");
    // 不带 project_root 时同一候选可以出现。
    let env = JdkDiscoveryEnv {
        java_home: None,
        path_entries: vec![project.join("embedded").join("bin")],
    };
    let candidates = discover_java_homes(None, None, None, &env, &static_probe("21", "x86_64"))
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source, JdkSource::Path);
}

#[test]
fn incomplete_layout_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let jre = make_fake_jdk(dir.path(), "jre-only", false);
    let env = JdkDiscoveryEnv {
        java_home: Some(jre),
        path_entries: Vec::new(),
    };
    let candidates =
        discover_java_homes(None, None, None, &env, &static_probe("21", "x86_64")).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(!candidates[0].complete);
}

#[test]
fn java_major_parsing_handles_legacy_and_modern() {
    assert_eq!(parse_java_major("21.0.12"), Some(21));
    assert_eq!(parse_java_major("1.8.0_392"), Some(8));
    assert_eq!(parse_java_major("17.0.2+8"), Some(17));
    assert_eq!(parse_java_major("21-ea"), Some(21));
    assert_eq!(parse_java_major("garbage"), None);
}

#[test]
fn show_settings_output_parsing() {
    let probe = parse_show_settings_output(
        "non property line\n java.version = 17.0.1 \n os.arch = amd64 \n",
    );
    assert_eq!(probe.version.as_deref(), Some("17.0.1"));
    assert_eq!(probe.arch.as_deref(), Some("x86_64"));
}

#[cfg(windows)]
#[test]
fn real_probe_reports_current_java_when_available() {
    // 不依赖具体 JDK：若本机 JAVA_HOME 是有效 JDK，则探测应得到主版本号。
    let Some(java_home) = std::env::var_os("JAVA_HOME").map(PathBuf::from) else {
        return;
    };
    if !layout_complete(&java_home) {
        return;
    }
    let probe = super::runtime::java::probe_java_runtime(&java_home, Duration::from_secs(15));
    assert!(probe.error.is_none(), "探测失败：{probe:?}");
    assert!(probe.major_version().is_some(), "应能解析主版本：{probe:?}");
}

// ---------------------------------------------------------------------------
// 真实环境（ignored）
// ---------------------------------------------------------------------------

/// 真实下载 Eclipse JDT LS 1.60.0 官方归档并安装（~50MB，需网络）。
/// 运行：cargo test --lib jls_t4_real_download_install_engine -- --ignored --nocapture
#[test]
#[ignore = "需要网络下载 ~50MB 官方归档；JLS-T4 验收时显式运行"]
fn jls_t4_real_download_install_engine() {
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let phases: Mutex<Vec<InstallPhase>> = Mutex::new(Vec::new());
    let outcome = install_package(
        &JDT_LS_1_60_0_WINDOWS_X86_64,
        lsp_root.path(),
        &InstallSettings::default(),
        &cancel,
        Some(&|progress| {
            let mut seen = phases.lock().unwrap();
            if seen.last() != Some(&progress.phase) {
                seen.push(progress.phase);
                println!("阶段：{:?}", progress.phase);
            }
        }),
    )
    .unwrap();
    assert_eq!(outcome.engine_version.as_deref(), Some("1.60.0.202606262232"));
    assert!(outcome.package_root.join("config_win").is_dir());
    let registry = load_registry(lsp_root.path()).unwrap();
    assert_eq!(
        registry.plugins["java-jdtls"].engine.as_ref().unwrap().version,
        "1.60.0.202606262232"
    );
    // 托管激活校验：JDK 21 组合应通过。
    check_managed_activation("1.60.0.202606262232", 21).unwrap();
    println!("安装位置：{}", outcome.package_root.display());
}

/// 完整验收链路：真实官方包安装 → 托管 JDK/JDT 构建 provider → 启动真实 JDT
/// → Maven fixture 五类核心查询复验（T0 同一组期望）。
///
/// 安装产物缓存在 `target/jls-t4-acceptance/`：重跑时 registry 已有记录则跳过下载。
/// 运行：cargo test --lib jls_t4_real_managed_service_answers_queries -- --ignored --nocapture
#[test]
#[ignore = "首次需网络下载 ~255MB 官方归档（Eclipse 源较慢）；JLS-T4 验收时显式运行"]
fn jls_t4_real_managed_service_answers_queries() {
    use super::install::{find_builtin_spec, install_package};
    use super::providers::{SemanticProvider, jdtls::{JdtLsProvider, ManualJdtLsConfig}};
    use super::{LspSemanticService, SemanticOperation, ServiceLimits};
    use crate::ai::review_store::repo_key;
    use crate::code_index::{
        PipelineOptions, ProjectContext, RunOutcome, SourceRef, SourceRefParts, content_fingerprint,
        run_index,
    };
    use crate::lsp::RequestCancellation;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    // 安装产物缓存目录（target 下，不进 Git）。
    let acceptance_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/jls-t4-acceptance");
    let lsp_root = acceptance_root.join("lsp");
    std::fs::create_dir_all(&lsp_root).unwrap();

    let install_once = |spec: &super::install::EnginePackageSpec| {
        let snapshot = install_snapshot(&lsp_root, spec.plugin_id).unwrap();
        let already = match spec.component {
            PackageComponent::Engine => snapshot
                .engine
                .as_ref()
                .is_some_and(|engine| engine.version == spec.version),
            PackageComponent::JavaRuntime => matches!(snapshot.runtime,
                Some(RuntimeSelection::Managed(ref component)) if component.version == spec.version),
        };
        if already {
            println!("复用已安装：{}", spec.package_id);
            return;
        }
        let cancel = AtomicBool::new(false);
        install_package(spec, &lsp_root, &InstallSettings::default(), &cancel, None).unwrap();
    };
    let engine_spec = find_builtin_spec("jdtls-1.60.0.202606262232-windows-x86_64")
        .expect("内置清单必须包含 JDT 1.60.0");
    let runtime_spec =
        find_builtin_spec("temurin-jdk-21.0.12.1+1-windows-x86_64").expect("内置清单必须包含 JDK");
    install_once(runtime_spec);
    install_once(engine_spec);

    let snapshot = install_snapshot(&lsp_root, "java-jdtls").unwrap();
    let engine_component = snapshot.engine.expect("JDT 引擎应已安装");
    let java_home = managed_runtime_java_home(&lsp_root, "java-jdtls")
        .unwrap()
        .expect("托管 JDK 应已安装");
    let jdtls_home = lsp_root.join(&engine_component.path);
    assert!(layout_complete(&java_home), "托管 JDK 布局不完整");
    let probe =
        super::runtime::java::probe_java_runtime(&java_home, Duration::from_secs(15));
    assert_eq!(probe.major_version(), Some(21), "托管 JDK 探测：{probe:?}");

    // 托管激活校验（正式发行条目 + JDK 21）。
    check_managed_activation("1.60.0.202606262232", 21).unwrap();

    // Maven fixture 副本 + 基础索引。
    let directory = tempfile::tempdir().unwrap();
    let project_root = directory.path().join("maven-multi");
    super::tests::copy_tree(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/tests/fixtures/java_semantic/maven-multi")
            .as_path(),
        &project_root,
    );
    let canonical_root = std::fs::canonicalize(&project_root).unwrap();
    let project_key = repo_key(&canonical_root.to_string_lossy());
    let index_db_path = directory.path().join("index.db");
    let mut index_options =
        PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    assert!(matches!(
        run_index(&canonical_root, &index_db_path, true, &mut index_options).unwrap(),
        RunOutcome::Completed(_)
    ));
    let context = ProjectContext {
        project_key: project_key.clone(),
        canonical_root: canonical_root.to_string_lossy().into_owned(),
        index_db_path: index_db_path.to_string_lossy().into_owned(),
    };

    // 用安装产物构建 provider（与手动配置同一形状，路径来自 registry）。
    let provider = JdtLsProvider::new(ManualJdtLsConfig {
        java_home,
        jdtls_home,
        workspace_root: acceptance_root.join("jdt-workspaces"),
        project_runtimes: Vec::new(),
    })
    .unwrap();
    assert_eq!(provider.engine_version(), "1.60.0.202606262232");
    let limits = ServiceLimits {
        query_timeout: Duration::from_secs(5),
        import_timeout: Duration::from_secs(120),
        ..ServiceLimits::default()
    };
    let service = Arc::new(LspSemanticService::with_limits(Arc::new(provider), limits).unwrap());
    service.configure_project(context, true, true).unwrap();
    service.acquire(&project_key, "jls-t4-acceptance").unwrap();
    let started = service.ensure_started(&project_key, "java");
    if let Err(error) = &started {
        panic!("托管安装的 JDT 启动失败：{error}");
    }
    assert_eq!(started.unwrap(), super::ServiceStatus::Ready);

    // 五类核心查询（与 T1 真实测试同一组 T0 期望）。
    let make_anchor = |relative: &str, line: u32, column: u32| -> crate::lsp::SemanticAnchor {
        let text = std::fs::read_to_string(canonical_root.join(relative)).unwrap();
        let source = SourceRef::new(SourceRefParts {
            project_key: project_key.clone(),
            generation: 1,
            relative_path: relative.to_string(),
            content_sha256: content_fingerprint(text.as_bytes()),
            start_byte: 0,
            end_byte: text.len() as u64,
            start_line: 1,
            end_line: text.lines().count() as u32,
        })
        .unwrap();
        service
            .register_verified_anchor(&project_key, &source, line, column)
            .unwrap()
    };
    let cancel = RequestCancellation::default();

    // 1) 跨模块定义：controller 的 service.login 定位到 core 实现。
    let controller = make_anchor(
        "web/src/main/java/com/example/web/LoginController.java",
        14,
        24,
    );
    let definition = service
        .query(&controller.anchor_id, SemanticOperation::Definition, Some(20), &cancel)
        .unwrap();
    assert!(
        definition.items.iter().any(|item| {
            item.target.relative_path.as_deref()
                == Some("core/src/main/java/com/example/core/LoginApplicationService.java")
                && item.target.line == 13
        }),
        "定义结果：{definition:#?}"
    );
    assert_eq!(definition.engine_version, "1.60.0.202606262232");

    // 2) 引用：service.login 共 3 处。
    let service_method = make_anchor(
        "core/src/main/java/com/example/core/LoginApplicationService.java",
        13,
        24,
    );
    let references = service
        .query(&service_method.anchor_id, SemanticOperation::References, Some(20), &cancel)
        .unwrap();
    assert_eq!(references.items.len(), 3, "引用结果：{references:#?}");

    // 3) 入调用 2 处（controller + batch job），带显式端点。
    let incoming = service
        .query(&service_method.anchor_id, SemanticOperation::IncomingCalls, Some(20), &cancel)
        .unwrap();
    assert_eq!(incoming.items.len(), 2, "入调用：{incoming:#?}");
    assert!(incoming
        .items
        .iter()
        .all(|item| item.caller.is_some() && item.callee.is_some() && item.call_site.is_some()));

    // 4) 出调用 1 处（AuthProvider 接口）。
    let outgoing = service
        .query(&service_method.anchor_id, SemanticOperation::OutgoingCalls, Some(20), &cancel)
        .unwrap();
    assert_eq!(outgoing.items.len(), 1, "出调用：{outgoing:#?}");

    // 5) 接口实现候选 2 个（Local/Sso），不假装唯一。
    let interface = make_anchor("api/src/main/java/com/example/api/AuthProvider.java", 4, 17);
    let implementations = service
        .query(&interface.anchor_id, SemanticOperation::Implementations, Some(20), &cancel)
        .unwrap();
    assert_eq!(implementations.items.len(), 2, "实现候选：{implementations:#?}");

    // 收尾：释放消费者并停止进程。
    service.release(&project_key, "jls-t4-acceptance");
    println!("托管安装链路验收通过：{}", lsp_root.display());
}

#[test]
#[ignore = "诊断用：直接验证 ureq 访问 GitHub release 资产"]
fn diag_ureq_github_asset() {
    use std::time::Instant;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .timeout_send_body(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .build()
        .new_agent();
    for url in [
        "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip",
        "https://github.com/",
    ] {
        let started = Instant::now();
        let result = agent.get(url).call();
        println!(
            "{url}\n  -> {:?} in {:?}",
            result.as_ref().map(|r| r.status().as_u16()).map_err(|e| e.to_string()),
            started.elapsed()
        );
    }
    // 裸 TCP 对照：绕开 ureq/TLS，直接连接 github.com:443。
    for target in [("domain", "github.com:443"), ("literal", "20.205.243.166:443")] {
        let started = Instant::now();
        let result = std::net::TcpStream::connect(target.1);
        println!(
            "tcp {} {} -> {:?} in {:?}",
            target.0,
            target.1,
            result.map(|s| drop(s)).map_err(|e| e.to_string()),
            started.elapsed()
        );
    }
}

#[test]
fn mirror_source_is_preferred_and_official_fallback_used_on_failure() {
    let archive = fake_engine_archive("top");
    // 镜像返回 404 → 滑落官方源成功。
    let mirror = FakeServer::start(vec![FakeResponse::NotFound]);
    let official = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let mut spec = fake_spec(
        official.url("/engine.tar.gz"),
        sha256_hex(&archive),
        archive.len() as u64,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    spec.mirror_url = Some(Box::leak(mirror.url("/engine.tar.gz").into_boxed_str()));
    let lsp_root = tempfile::tempdir().unwrap();
    let cancel = no_cancel();
    install_package(&spec, lsp_root.path(), &test_settings(), &cancel, None).unwrap();
    assert_eq!(mirror.requests.load(Ordering::SeqCst), 1, "镜像应被首先请求");
    assert_eq!(official.requests.load(Ordering::SeqCst), 1, "官方源应被兜底请求");

    // 镜像可用 → 官方源零请求。
    let mirror2 = FakeServer::start(vec![FakeResponse::Ok(archive.clone())]);
    let official2 = FakeServer::start(vec![FakeResponse::Ok(archive)]);
    let mut spec2 = fake_spec(
        official2.url("/engine.tar.gz"),
        sha256_hex(&fake_engine_archive("top")),
        fake_engine_archive("top").len() as u64,
        ArchiveFormat::TarGz,
        PackageComponent::Engine,
    );
    spec2.mirror_url = Some(Box::leak(mirror2.url("/engine.tar.gz").into_boxed_str()));
    let lsp_root2 = tempfile::tempdir().unwrap();
    install_package(&spec2, lsp_root2.path(), &test_settings(), &cancel, None).unwrap();
    assert_eq!(official2.requests.load(Ordering::SeqCst), 0, "镜像成功时不得请求官方源");
}

/// 真实安装 Temurin JDK + JDT 引擎并探测私有 JDK 版本（~255MB 下载，很慢）。
/// 真实 JDT 启动查询复验见 `jls_t4_real_managed_service_answers_queries`。
/// 运行：cargo test --lib jls_t4_real_install_runtime -- --ignored --nocapture
#[test]
#[ignore = "需要网络下载 ~205MB JDK + ~50MB JDT；JLS-T4 验收时显式运行"]
fn jls_t4_real_install_runtime() {
    // 产物缓存在 target 下：重跑时 registry 已有记录则跳过下载。
    let lsp_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/jls-t4-acceptance/lsp");
    std::fs::create_dir_all(&lsp_root).unwrap();
    let snapshot = install_snapshot(&lsp_root, "java-jdtls").unwrap();
    if snapshot.engine.is_some() && matches!(snapshot.runtime, Some(RuntimeSelection::Managed(_))) {
        println!("产物已缓存，跳过下载：{}", lsp_root.display());
        return;
    }
    let cancel = AtomicBool::new(false);
    let runtime = install_package(
        &TEMURIN_JDK_21_WINDOWS_X86_64,
        &lsp_root,
        &InstallSettings::default(),
        &cancel,
        Some(&|progress| println!("JDK 阶段：{:?}", progress.phase)),
    )
    .unwrap();
    assert!(layout_complete(&runtime.package_root), "私有 JDK 布局不完整");
    let probe =
        super::runtime::java::probe_java_runtime(&runtime.package_root, Duration::from_secs(30));
    assert_eq!(probe.major_version(), Some(21), "私有 JDK 版本探测：{probe:?}");
    assert_eq!(probe.arch.as_deref(), Some("x86_64"));
    let engine = install_package(
        &JDT_LS_1_60_0_WINDOWS_X86_64,
        &lsp_root,
        &InstallSettings::default(),
        &cancel,
        Some(&|progress| println!("JDT 阶段：{:?}", progress.phase)),
    )
    .unwrap();
    let snapshot = install_snapshot(&lsp_root, "java-jdtls").unwrap();
    assert!(snapshot.engine.is_some() && snapshot.runtime.is_some());
    // registry 记录的路径应能重新定位两个包根。
    let engine_component = snapshot.engine.unwrap();
    assert!(lsp_root.join(&engine_component.path).is_dir());
    println!("JDT：{}\nJDK：{}", engine.package_root.display(), runtime.package_root.display());
}
