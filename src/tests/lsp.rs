use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Cursor, Read};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::client::{LspClientError, ServerRequestPolicy};
use super::plugins::{
    DESCRIPTOR_SCHEMA_VERSION, EngineCompatibility, LspPluginDescriptor, PluginRegistry,
    PluginRegistryError, SemanticOperation, TransportKind,
};
use super::protocol::{
    CharacterEncoding, FrameError, LspPosition, MAX_LSP_FRAME_BYTES, byte_offset_to_lsp_position,
    file_uri_to_path, lsp_position_to_byte_offset, path_to_file_uri, read_lsp_frame,
    scalar_position_to_lsp, write_lsp_frame,
};
use super::providers::jdtls::{JdtLsProvider, JdtRuntime, ManualJdtLsConfig};
use super::providers::{ProviderError, SemanticProvider};
use super::{
    LaunchSpec, LspClient, LspSemanticService, RequestCancellation, ServiceLimits, ServiceStatus,
};
use crate::ai::review_store::repo_key;
use crate::code_index::{
    PipelineOptions, ProjectContext, RunOutcome, SourceRef, SourceRefParts, content_fingerprint,
    run_index,
};
use crate::code_understanding::{
    CallTraceDirection, JavaSemanticAnchor, QueryJavaSemanticsArgs, ReadFileArgs,
    SearchSymbolsArgs, SourceService, TraceCallsArgs, UnderstandingTools,
};

struct ChunkedReader {
    inner: Cursor<Vec<u8>>,
    max_chunk: usize,
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = buffer.len().min(self.max_chunk);
        self.inner.read(&mut buffer[..length])
    }
}

fn framed(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_lsp_frame(&mut out, value).unwrap();
    out
}

#[test]
fn content_length_reader_handles_fragmented_and_concatenated_frames() {
    let first = json!({"jsonrpc":"2.0","id":1,"result":{"text":"中文😀"}});
    let second = json!({"jsonrpc":"2.0","method":"ready","params":{}});
    let mut bytes = framed(&first);
    bytes.extend(framed(&second));
    let mut reader = BufReader::new(ChunkedReader {
        inner: Cursor::new(bytes),
        max_chunk: 3,
    });

    assert_eq!(read_lsp_frame(&mut reader).unwrap(), Some(first));
    assert_eq!(read_lsp_frame(&mut reader).unwrap(), Some(second));
    assert_eq!(read_lsp_frame(&mut reader).unwrap(), None);
}

#[test]
fn content_length_reader_rejects_bad_and_oversized_frames_before_allocation() {
    let mut missing = BufReader::new(Cursor::new(b"X-Test: 1\r\n\r\n{}".to_vec()));
    assert!(matches!(
        read_lsp_frame(&mut missing),
        Err(FrameError::MissingContentLength)
    ));

    let oversized = format!("Content-Length: {}\r\n\r\n", MAX_LSP_FRAME_BYTES + 1);
    let mut oversized = BufReader::new(Cursor::new(oversized.into_bytes()));
    assert!(matches!(
        read_lsp_frame(&mut oversized),
        Err(FrameError::FrameTooLarge { .. })
    ));
}

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    (client, server)
}

#[test]
fn client_pairs_out_of_order_responses_by_request_id() {
    let (client_stream, server_stream) = tcp_pair();
    let reader = client_stream.try_clone().unwrap();
    let client = LspClient::start(reader, client_stream, ServerRequestPolicy::default());
    let server = thread::spawn(move || {
        let mut reader = BufReader::new(server_stream.try_clone().unwrap());
        let first = read_lsp_frame(&mut reader).unwrap().unwrap();
        let second = read_lsp_frame(&mut reader).unwrap().unwrap();
        let mut writer = server_stream;
        for request in [&second, &first] {
            write_lsp_frame(
                &mut writer,
                &json!({
                    "jsonrpc":"2.0",
                    "id":request["id"],
                    "result":request["method"]
                }),
            )
            .unwrap();
        }
    });

    let first_client = client.clone();
    let first = thread::spawn(move || {
        first_client.request(
            "first",
            json!({}),
            Duration::from_secs(2),
            &RequestCancellation::default(),
        )
    });
    let second_client = client.clone();
    let second = thread::spawn(move || {
        second_client.request(
            "second",
            json!({}),
            Duration::from_secs(2),
            &RequestCancellation::default(),
        )
    });
    let values = [
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap(),
    ];
    assert!(values.contains(&json!("first")));
    assert!(values.contains(&json!("second")));
    server.join().unwrap();
}

#[test]
fn client_answers_read_only_reverse_requests_and_tracks_ready_notification() {
    let (client_stream, server_stream) = tcp_pair();
    let reader = client_stream.try_clone().unwrap();
    let policy = ServerRequestPolicy::new(json!({"java":{"autobuild":{"enabled":false}}}));
    let client = LspClient::start(reader, client_stream, policy.clone());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut writer = server_stream.try_clone().unwrap();
        write_lsp_frame(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":"apply","method":"workspace/applyEdit","params":{}}),
        )
        .unwrap();
        write_lsp_frame(
            &mut writer,
            &json!({
                "jsonrpc":"2.0",
                "id":"watch",
                "method":"client/registerCapability",
                "params":{"registrations":[{
                    "id":"watch-1",
                    "method":"workspace/didChangeWatchedFiles",
                    "registerOptions":{"watchers":[{"globPattern":"**/*.java"}]}
                }]}
            }),
        )
        .unwrap();
        write_lsp_frame(
            &mut writer,
            &json!({
                "jsonrpc":"2.0",
                "id":"config",
                "method":"workspace/configuration",
                "params":{"items":[{"section":"java"},{"section":"missing"}]}
            }),
        )
        .unwrap();
        write_lsp_frame(
            &mut writer,
            &json!({"jsonrpc":"2.0","method":"language/status","params":{"type":"ServiceReady"}}),
        )
        .unwrap();
        write_lsp_frame(
            &mut writer,
            &json!({
                "jsonrpc":"2.0",
                "method":"textDocument/publishDiagnostics",
                "params":{"uri":"file:///Example.java","diagnostics":[{"severity":1}]}
            }),
        )
        .unwrap();
        let mut reader = BufReader::new(server_stream);
        let apply = read_lsp_frame(&mut reader).unwrap().unwrap();
        let watch = read_lsp_frame(&mut reader).unwrap().unwrap();
        let configuration = read_lsp_frame(&mut reader).unwrap().unwrap();
        tx.send((apply, watch, configuration)).unwrap();
    });
    let (apply, watch, configuration) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(apply["result"]["applied"], false);
    assert_eq!(watch["result"], Value::Null);
    assert_eq!(configuration["result"][0]["autobuild"]["enabled"], false);
    assert_eq!(configuration["result"][1], Value::Null);
    for _ in 0..20 {
        if policy.service_ready() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(policy.service_ready());
    for _ in 0..20 {
        if policy.has_diagnostic_errors() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(policy.has_diagnostic_errors());
    assert!(policy.watched_files_registered());
    drop(client);
}

#[test]
fn cancellation_sends_protocol_notification_and_discards_late_result() {
    let (client_stream, server_stream) = tcp_pair();
    let reader = client_stream.try_clone().unwrap();
    let client = LspClient::start(reader, client_stream, ServerRequestPolicy::default());
    let (cancel_seen_tx, cancel_seen_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(server_stream.try_clone().unwrap());
        let request = read_lsp_frame(&mut reader).unwrap().unwrap();
        let cancel = read_lsp_frame(&mut reader).unwrap().unwrap();
        cancel_seen_tx.send(cancel.clone()).unwrap();
        let mut writer = server_stream;
        write_lsp_frame(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":request["id"],"result":"too late"}),
        )
        .unwrap();
    });
    let cancellation = RequestCancellation::default();
    let request_client = client.clone();
    let request_cancel = cancellation.clone();
    let request = thread::spawn(move || {
        request_client.request("slow", json!({}), Duration::from_secs(2), &request_cancel)
    });
    thread::sleep(Duration::from_millis(40));
    cancellation.cancel();
    assert!(matches!(
        request.join().unwrap(),
        Err(LspClientError::Cancelled)
    ));
    let cancel = cancel_seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(cancel["method"], "$/cancelRequest");
}

#[test]
fn position_conversion_handles_utf16_emoji_chinese_and_crlf() {
    let text = "class 示例 {\r\n  String value = \"A😀中\";\r\n}\r\n";
    let scalar = scalar_position_to_lsp(text, 2, 21, CharacterEncoding::Utf16).unwrap();
    assert_eq!(
        scalar,
        LspPosition {
            line: 1,
            character: 21
        }
    );
    let byte = lsp_position_to_byte_offset(text, scalar, CharacterEncoding::Utf16).unwrap();
    assert_eq!(&text[byte..], "中\";\r\n}\r\n");
    assert_eq!(
        byte_offset_to_lsp_position(text, byte, CharacterEncoding::Utf16).unwrap(),
        scalar
    );
    assert!(
        lsp_position_to_byte_offset(
            text,
            LspPosition {
                line: 1,
                character: 20
            },
            CharacterEncoding::Utf16
        )
        .is_err()
    );
}

#[test]
fn file_uri_round_trip_preserves_spaces_and_unicode() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("中文 space.java");
    std::fs::write(&file, "class A {}").unwrap();
    let uri = path_to_file_uri(&file).unwrap();
    assert!(uri.contains("%20"));
    assert!(!uri.contains("%3F"));
    assert_eq!(
        std::fs::canonicalize(file_uri_to_path(&uri).unwrap()).unwrap(),
        std::fs::canonicalize(file).unwrap()
    );
}

const TEST_COMPAT: &[EngineCompatibility] = &[];
const TEST_OPS: &[SemanticOperation] = &[SemanticOperation::Definition];
static DUPLICATE: LspPluginDescriptor = LspPluginDescriptor {
    plugin_id: "java-jdtls",
    display_name: "duplicate",
    descriptor_version: DESCRIPTOR_SCHEMA_VERSION,
    adapter_id: "jdtls",
    languages: &["java"],
    operations: TEST_OPS,
    transport: TransportKind::Stdio,
    runtime: "java>=21",
    engine_packages: &[],
    compatibility: TEST_COMPAT,
};
static CONFLICT: LspPluginDescriptor = LspPluginDescriptor {
    plugin_id: "other-java",
    display_name: "conflict",
    descriptor_version: DESCRIPTOR_SCHEMA_VERSION,
    adapter_id: "jdtls",
    languages: &["java"],
    operations: TEST_OPS,
    transport: TransportKind::Stdio,
    runtime: "java>=21",
    engine_packages: &[],
    compatibility: TEST_COMPAT,
};
static UNKNOWN_SCHEMA: LspPluginDescriptor = LspPluginDescriptor {
    plugin_id: "schema-test",
    display_name: "schema",
    descriptor_version: 99,
    adapter_id: "jdtls",
    languages: &["schema-language"],
    operations: TEST_OPS,
    transport: TransportKind::Stdio,
    runtime: "java>=21",
    engine_packages: &[],
    compatibility: TEST_COMPAT,
};
static UNKNOWN_ADAPTER: LspPluginDescriptor = LspPluginDescriptor {
    plugin_id: "adapter-test",
    display_name: "adapter",
    descriptor_version: DESCRIPTOR_SCHEMA_VERSION,
    adapter_id: "external-script",
    languages: &["adapter-language"],
    operations: TEST_OPS,
    transport: TransportKind::Stdio,
    runtime: "java>=21",
    engine_packages: &[],
    compatibility: TEST_COMPAT,
};

#[test]
fn plugin_registry_rejects_duplicate_ids_language_conflicts_and_unknown_ids() {
    assert!(matches!(
        PluginRegistry::new([&super::plugins::JAVA_JDTLS_PLUGIN, &DUPLICATE]),
        Err(PluginRegistryError::DuplicatePluginId(_))
    ));
    assert!(matches!(
        PluginRegistry::new([&super::plugins::JAVA_JDTLS_PLUGIN, &CONFLICT]),
        Err(PluginRegistryError::LanguageConflict { .. })
    ));
    let registry = PluginRegistry::new([&super::plugins::JAVA_JDTLS_PLUGIN]).unwrap();
    assert!(matches!(
        registry.get("missing"),
        Err(PluginRegistryError::UnknownPlugin(_))
    ));
    assert!(matches!(
        PluginRegistry::new([&UNKNOWN_SCHEMA]),
        Err(PluginRegistryError::UnknownSchema(99))
    ));
    assert!(matches!(
        PluginRegistry::new([&UNKNOWN_ADAPTER]),
        Err(PluginRegistryError::UnknownAdapter(_))
    ));
}

#[test]
fn effective_capabilities_are_three_way_intersection() {
    let registry = PluginRegistry::new([&super::plugins::JAVA_JDTLS_PLUGIN]).unwrap();
    let adapter = SemanticOperation::ALL.into_iter().collect();
    let server = BTreeSet::from([SemanticOperation::Definition, SemanticOperation::References]);
    let effective = registry
        .effective_operations("java-jdtls", &adapter, &server)
        .unwrap();
    assert_eq!(effective, server);
    assert!(!effective.contains(&SemanticOperation::Implementations));
}

#[test]
fn jdt_provider_generates_controlled_launch_spec_and_development_identity() {
    let directory = tempfile::tempdir().unwrap();
    let java_home = directory.path().join("jdk");
    let jdtls_home = directory.path().join("jdtls");
    std::fs::create_dir_all(java_home.join("bin")).unwrap();
    std::fs::create_dir_all(jdtls_home.join("plugins")).unwrap();
    std::fs::create_dir_all(jdtls_home.join(if cfg!(windows) {
        "config_win"
    } else if cfg!(target_os = "macos") {
        "config_mac"
    } else {
        "config_linux"
    }))
    .unwrap();
    let java_name = if cfg!(windows) { "java.exe" } else { "java" };
    let javac_name = if cfg!(windows) { "javac.exe" } else { "javac" };
    std::fs::write(java_home.join("bin").join(java_name), "").unwrap();
    std::fs::write(java_home.join("bin").join(javac_name), "").unwrap();
    std::fs::write(
        jdtls_home
            .join("plugins")
            .join("org.eclipse.equinox.launcher_1.8.0.jar"),
        "",
    )
    .unwrap();
    std::fs::write(
        jdtls_home
            .join("plugins")
            .join("org.eclipse.jdt.ls.core_1.61.0.202609031315.jar"),
        "",
    )
    .unwrap();
    let runtime_17 = directory.path().join("project-jdk-17");
    let provider = JdtLsProvider::new(ManualJdtLsConfig {
        java_home,
        jdtls_home,
        workspace_root: directory.path().join("workspaces"),
        project_runtimes: vec![JdtRuntime {
            name: "JavaSE-17".to_string(),
            path: runtime_17.clone(),
            default: true,
        }],
    })
    .unwrap();
    let project = ProjectContext {
        project_key: "project".to_string(),
        canonical_root: directory.path().to_string_lossy().into_owned(),
        index_db_path: String::new(),
    };
    let spec = provider.launch_spec(&project).unwrap();
    assert!(spec.args.iter().any(|arg| arg == "-Xmx1G"));
    assert!(spec.env_remove.iter().any(|name| name == "CLIENT_PORT"));
    assert_eq!(provider.engine_version(), "1.61.0.202609031315");
    assert!(provider.development_snapshot());
    let initialize = provider.initialize_params(&project).unwrap();
    assert_eq!(
        initialize.pointer("/initializationOptions/settings/java/configuration/runtimes/0/name"),
        Some(&json!("JavaSE-17"))
    );
    assert_eq!(
        initialize
            .pointer("/initializationOptions/settings/java/configuration/runtimes/0/path")
            .and_then(Value::as_str),
        Some(runtime_17.to_string_lossy().as_ref())
    );
}

#[derive(Clone)]
struct AnchorOnlyProvider;

impl SemanticProvider for AnchorOnlyProvider {
    fn plugin_id(&self) -> &'static str {
        "java-jdtls"
    }

    fn adapter_id(&self) -> &'static str {
        "jdtls"
    }

    fn engine_version(&self) -> &str {
        "fake"
    }

    fn implemented_operations(&self) -> BTreeSet<SemanticOperation> {
        SemanticOperation::ALL.into_iter().collect()
    }

    fn launch_spec(&self, _: &ProjectContext) -> Result<LaunchSpec, ProviderError> {
        Ok(LaunchSpec {
            executable: PathBuf::from("never-started"),
            args: Vec::new(),
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            current_dir: None,
        })
    }

    fn initialize_params(&self, _: &ProjectContext) -> Result<Value, ProviderError> {
        Ok(json!({}))
    }

    fn request_policy(&self) -> ServerRequestPolicy {
        ServerRequestPolicy::default()
    }
}

#[test]
fn semantic_anchor_is_project_hash_range_and_allowlist_bound() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("示例.java");
    let text = "class 示例 {\r\n  void run() {}\r\n}\r\n";
    std::fs::write(&file, text).unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let project_key = repo_key(&root.to_string_lossy());
    let context = ProjectContext {
        project_key: project_key.clone(),
        canonical_root: root.to_string_lossy().into_owned(),
        index_db_path: String::new(),
    };
    let service = LspSemanticService::new(Arc::new(AnchorOnlyProvider)).unwrap();
    let plugin = service.plugin_state("java-jdtls");
    assert!(plugin.registered && plugin.enabled);
    assert_eq!(
        plugin.package_status,
        super::EnginePackageStatus::ManualConfigured
    );
    assert!(!service.plugin_state("unknown").registered);
    service
        .configure_project(context.clone(), true, true)
        .unwrap();
    let source_service = SourceService::open(context, 7).unwrap();
    let issued = source_service.read_file("示例.java", 1, 3).unwrap();
    let source = issued.source_ref.clone();
    let anchor = service
        .register_source_anchor(&source_service, &issued.source_id, 2, 8)
        .unwrap();
    assert!(anchor.anchor_id.starts_with("sa1:"));
    assert_eq!(anchor.generation, 7);
    assert!(matches!(
        service.register_source_anchor(&source_service, "sr1:deadbeef", 2, 8),
        Err(super::SemanticServiceError::SourceValidation(_))
    ));
    assert!(matches!(
        service.query(
            "missing",
            SemanticOperation::Definition,
            Some(0),
            &RequestCancellation::default()
        ),
        Err(super::SemanticServiceError::InvalidLimit { .. })
    ));

    std::fs::write(&file, text.replace("run", "changed")).unwrap();
    assert!(matches!(
        service.register_verified_anchor(&project_key, &source, 2, 8),
        Err(super::SemanticServiceError::SourceChanged(_))
    ));
    assert_eq!(service.status(&project_key, "java"), ServiceStatus::Stopped);
}

#[test]
#[cfg(windows)]
fn owned_process_shutdown_reaps_the_exact_spawned_process() {
    let spec = LaunchSpec {
        executable: PathBuf::from("powershell.exe"),
        args: vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Start-Sleep -Seconds 30".to_string(),
        ],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        current_dir: None,
    };
    let mut process = super::OwnedLspProcess::spawn(&spec, ServerRequestPolicy::default()).unwrap();
    let process_id = process.id();
    process.shutdown(Duration::from_millis(100));
    assert!(
        process.try_wait().unwrap().is_some(),
        "PID {process_id} 未被回收"
    );
}

#[test]
#[ignore = "需要本机 JDK 21 与 JDT LS；JLS-T1/T2 验收时显式运行"]
fn real_jdt_service_resolves_cross_module_definition() {
    let java_home = std::env::var_os("KHASLANA_TEST_JAVA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\khaslana\jdk-21.0.12.1+1"));
    let jdtls_home = std::env::var_os("KHASLANA_TEST_JDTLS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\khaslana\jdt-language-server"));
    assert!(java_home.is_dir(), "缺少测试 JDK：{}", java_home.display());
    assert!(
        jdtls_home.is_dir(),
        "缺少测试 JDT LS：{}",
        jdtls_home.display()
    );

    let directory = tempfile::tempdir().unwrap();
    let project_root = directory.path().join("maven-multi");
    copy_tree(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/tests/fixtures/java_semantic/maven-multi")
            .as_path(),
        &project_root,
    );
    let canonical_root = std::fs::canonicalize(&project_root).unwrap();
    let project_key = repo_key(&canonical_root.to_string_lossy());
    let index_db_path = directory.path().join("index.db");
    let mut index_options = PipelineOptions::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Box::new(|_| {}),
    );
    assert!(matches!(
        run_index(&canonical_root, &index_db_path, true, &mut index_options).unwrap(),
        RunOutcome::Completed(_)
    ));
    let context = ProjectContext {
        project_key: project_key.clone(),
        canonical_root: canonical_root.to_string_lossy().into_owned(),
        index_db_path: index_db_path.to_string_lossy().into_owned(),
    };
    let provider = JdtLsProvider::new(ManualJdtLsConfig {
        java_home,
        jdtls_home,
        workspace_root: directory.path().join("jdt-workspaces"),
        project_runtimes: Vec::new(),
    })
    .unwrap();
    let limits = ServiceLimits {
        query_timeout: Duration::from_secs(5),
        import_timeout: Duration::from_secs(60),
        ..ServiceLimits::default()
    };
    let service = Arc::new(LspSemanticService::with_limits(Arc::new(provider), limits).unwrap());
    service.configure_project(context, true, true).unwrap();
    service.acquire(&project_key, "test").unwrap();
    service.acquire(&project_key, "ui").unwrap();
    let started = service.ensure_started(&project_key, "java");
    if let Err(error) = &started {
        let mut logs = Vec::new();
        collect_named_files(directory.path(), ".log", &mut logs);
        let contents = logs
            .iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("真实 JDT 启动失败：{error}\n{contents}");
    }
    assert_eq!(started.unwrap(), ServiceStatus::Ready);

    let (source, anchor) = real_anchor(
        &service,
        &project_key,
        &canonical_root,
        "web/src/main/java/com/example/web/LoginController.java",
        14,
        24,
    );
    let result = service
        .query(
            &anchor.anchor_id,
            SemanticOperation::Definition,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert!(
        result.items.iter().any(|item| {
            item.target.relative_path.as_deref()
                == Some("core/src/main/java/com/example/core/LoginApplicationService.java")
                && item.target.line == 13
        }),
        "实际定义结果：{result:#?}"
    );
    assert_eq!(result.plugin_id, "java-jdtls");
    assert_eq!(result.provider, "jdtls");

    let symbols = service
        .register_verified_symbol_anchors(
            &project_key,
            &source,
            13,
            Some("login"),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!((symbols[0].line, symbols[0].column), (13, 24));

    let (_, service_anchor) = real_anchor(
        &service,
        &project_key,
        &canonical_root,
        "core/src/main/java/com/example/core/LoginApplicationService.java",
        13,
        24,
    );
    let references = service
        .query(
            &service_anchor.anchor_id,
            SemanticOperation::References,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert_eq!(references.items.len(), 3, "实际引用结果：{references:#?}");
    let incoming = service
        .query(
            &service_anchor.anchor_id,
            SemanticOperation::IncomingCalls,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert_eq!(incoming.items.len(), 2, "实际入调用：{incoming:#?}");
    assert!(
        incoming
            .items
            .iter()
            .all(|item| item.caller.is_some() && item.callee.is_some() && item.call_site.is_some())
    );
    let outgoing = service
        .query(
            &service_anchor.anchor_id,
            SemanticOperation::OutgoingCalls,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert_eq!(outgoing.items.len(), 1, "实际出调用：{outgoing:#?}");

    let (_, interface_anchor) = real_anchor(
        &service,
        &project_key,
        &canonical_root,
        "api/src/main/java/com/example/api/AuthProvider.java",
        4,
        17,
    );
    let implementations = service
        .query(
            &interface_anchor.anchor_id,
            SemanticOperation::Implementations,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert_eq!(
        implementations.items.len(),
        2,
        "实际实现候选：{implementations:#?}"
    );

    // T2：AI 工具层只接统一语义服务，使用本问 sr1 锚点并为返回位置发放来源。
    let tools = UnderstandingTools::open_with_semantic_service(
        &canonical_root,
        &index_db_path,
        service.clone(),
    )
    .unwrap();
    let controller_source = tools
        .read_file(
            "t2-source",
            ReadFileArgs {
                path: "web/src/main/java/com/example/web/LoginController.java".to_string(),
                start_line: Some(14),
                end_line: Some(14),
            },
        )
        .unwrap();
    let tool_definition = tools
        .query_java_semantics(
            "t2-definition",
            QueryJavaSemanticsArgs {
                operation: SemanticOperation::Definition,
                anchor: JavaSemanticAnchor::Source {
                    source_id: controller_source.data.source_id,
                    line: 14,
                    column: 24,
                },
                limit: Some(20),
            },
        )
        .unwrap();
    assert!(
        tool_definition
            .data
            .semantic
            .as_ref()
            .is_some_and(|result| {
                result.items.iter().any(|item| {
                    item.target.relative_path.as_deref()
                        == Some("core/src/main/java/com/example/core/LoginApplicationService.java")
                        && item.target.line == 13
                })
            })
    );
    assert!(tool_definition.data.source_links.iter().any(|link| {
        link.source_id
            .as_deref()
            .is_some_and(|id| id.starts_with("sr1:"))
    }));

    let search = tools
        .search_symbols(
            "t2-search",
            SearchSymbolsArgs {
                query: "login".to_string(),
                language: Some("java".to_string()),
                limit: Some(50),
                ..Default::default()
            },
        )
        .unwrap();
    let service_candidate = search
        .data
        .candidates
        .iter()
        .find(|candidate| {
            candidate.relative_path
                == "core/src/main/java/com/example/core/LoginApplicationService.java"
                && candidate.start_line == 13
        })
        .expect("基础索引应提供 LoginApplicationService.login 候选");
    let tool_trace = tools
        .trace_calls(
            "t2-trace",
            TraceCallsArgs {
                candidate_id: service_candidate.candidate_id.clone(),
                direction: CallTraceDirection::Both,
                depth: Some(3),
                limit: Some(20),
            },
        )
        .unwrap();
    let semantic_trace = tool_trace
        .data
        .semantic
        .expect("Java trace 应包含一跳语义增强");
    assert_eq!(semantic_trace.depth, 1);
    assert!(
        semantic_trace
            .inbound
            .as_ref()
            .and_then(|result| result.semantic.as_ref())
            .is_some_and(|result| !result.items.is_empty())
    );
    assert!(
        semantic_trace
            .outbound
            .as_ref()
            .and_then(|result| result.semantic.as_ref())
            .is_some_and(|result| !result.items.is_empty())
    );

    let changed_path =
        canonical_root.join("web/src/main/java/com/example/web/LoginController.java");
    let changed_text = std::fs::read_to_string(&changed_path).unwrap().replace(
        "// JLS: cross-module definition",
        "// JLS: cross-module definition ",
    );
    std::fs::write(&changed_path, changed_text).unwrap();
    service
        .mark_file_changed(
            &project_key,
            "web/src/main/java/com/example/web/LoginController.java",
            2,
        )
        .unwrap();
    assert!(matches!(
        service.query(
            &anchor.anchor_id,
            SemanticOperation::Definition,
            Some(20),
            &RequestCancellation::default()
        ),
        Err(super::SemanticServiceError::InvalidAnchor)
    ));
    let (_, changed_anchor) = real_anchor(
        &service,
        &project_key,
        &canonical_root,
        "web/src/main/java/com/example/web/LoginController.java",
        14,
        24,
    );
    let changed_result = service
        .query(
            &changed_anchor.anchor_id,
            SemanticOperation::Definition,
            Some(20),
            &RequestCancellation::default(),
        )
        .unwrap();
    assert!(changed_result.workspace_revision > result.workspace_revision);
    service.release(&project_key, "test");
    assert_ne!(service.status(&project_key, "java"), ServiceStatus::Stopped);
    service.release(&project_key, "ui");
}

pub(super) fn real_anchor(
    service: &LspSemanticService,
    project_key: &str,
    root: &std::path::Path,
    relative: &str,
    line: u32,
    column: u32,
) -> (SourceRef, super::SemanticAnchor) {
    let text = std::fs::read_to_string(root.join(relative)).unwrap();
    let source = SourceRef::new(SourceRefParts {
        project_key: project_key.to_string(),
        generation: 1,
        relative_path: relative.to_string(),
        content_sha256: content_fingerprint(text.as_bytes()),
        start_byte: 0,
        end_byte: text.len() as u64,
        start_line: 1,
        end_line: text.lines().count() as u32,
    })
    .unwrap();
    let anchor = service
        .register_verified_anchor(project_key, &source, line, column)
        .unwrap();
    (source, anchor)
}

pub(super) fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), destination).unwrap();
        }
    }
}

pub(super) fn collect_named_files(directory: &std::path::Path, name: &str, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_named_files(&entry.path(), name, output);
        } else if entry.file_name() == name {
            output.push(entry.path());
        }
    }
}
