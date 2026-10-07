#![cfg(windows)]

use std::time::Duration;
use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::json;

use super::*;
use super::extensions::{WorkflowExternalGrant, WorkflowMcpConfig, discover_skill_packages, register_external_actions,
    register_external_actions_with_ai};
use super::extensions::{preview_skill_folder, install_skill_folder, remove_skill_package,
    load_user_mcp_config, load_mcp_config, upsert_user_mcp_server, remove_user_mcp_server,
    inspect_mcp_server_tools};
use crate::git::test_support::git_test_support as git_support;

#[path = "workflow_branch_browser.rs"]
mod branch_browser;

fn test_config(marker: &Path) -> WorkflowMcpConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/workflow_mcp_server.ps1");
    serde_json::from_value(json!({
        "servers": {
            "fixture": {
                "command": "powershell.exe",
                "args": ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                    "-File", script, marker],
                "tools": {
                    "echo": {"access": "read"},
                    "write_file": {"access": "write"},
                    "set_state": {"access": "write"},
                    "get_state": {"access": "read"},
                    "stall": {"access": "read"}
                }
            }
        }
    })).unwrap()
}

fn grant_for(steps: &str) -> WorkflowExternalGrant {
    let content = format!("{{version:2, defaults:{{requireCleanWorktree:false}}, steps:[{steps}]}}");
    let definition = parse_workflow_json5(&content).unwrap();
    WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap()
}

fn make_registry(config: WorkflowMcpConfig, grant: Option<WorkflowExternalGrant>) -> WorkflowActionRegistry {
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions(&mut registry, config, grant).unwrap();
    registry
}

fn test_skill(dir: &Path) {
    let package = dir.join("workflow-skills/fixture-skill");
    std::fs::create_dir_all(package.join("references")).unwrap();
    std::fs::write(package.join("SKILL.md"),
        "---\nname: fixture-skill\ndescription: 测试工具循环\n---\n读取 fixture 工具。\n").unwrap();
    std::fs::write(package.join("references/guide.md"), "只读取 hello。\n").unwrap();
}

#[test]
fn skill_import_requires_confirmed_content_and_can_be_removed() {
    let source_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    test_skill(source_dir.path());
    let source = source_dir.path().join("workflow-skills/fixture-skill");
    let preview = preview_skill_folder(&source).unwrap();
    std::fs::write(source.join("references/guide.md"), "内容已改变\n").unwrap();
    assert!(install_skill_folder(data_dir.path(), &source, &preview.content_sha256)
        .unwrap_err().to_string().contains("内容已变化"));
    assert!(discover_skill_packages(data_dir.path()).unwrap().is_empty());

    let preview = preview_skill_folder(&source).unwrap();
    install_skill_folder(data_dir.path(), &source, &preview.content_sha256).unwrap();
    assert_eq!(discover_skill_packages(data_dir.path()).unwrap().len(), 1);
    remove_skill_package(data_dir.path(), "fixture-skill").unwrap();
    assert!(discover_skill_packages(data_dir.path()).unwrap().is_empty());
}

#[test]
fn visual_mcp_config_roundtrip_keeps_builtin_out_of_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = test_config(&dir.path().join("unused"))
        .servers.remove("fixture").unwrap();
    server.tools.retain(|name, _| name == "echo");
    upsert_user_mcp_server(dir.path(), None, "fixture", server).unwrap();
    assert!(load_user_mcp_config(dir.path()).unwrap().servers.contains_key("fixture"));
    assert!(load_mcp_config(dir.path()).unwrap().servers.contains_key("browser.edge"));
    let saved = std::fs::read_to_string(dir.path().join("workflow-mcp.json5")).unwrap();
    assert!(!saved.contains("browser.edge"));
    remove_user_mcp_server(dir.path(), "fixture").unwrap();
    assert!(load_user_mcp_config(dir.path()).unwrap().servers.is_empty());
}

#[test]
fn visual_mcp_config_rejects_builtin_name_and_empty_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = test_config(&dir.path().join("unused"))
        .servers.remove("fixture").unwrap();
    assert!(upsert_user_mcp_server(dir.path(), None, "browser.edge", server.clone()).is_err());
    server.tools.clear();
    assert!(upsert_user_mcp_server(dir.path(), None, "fixture", server).is_err());
    assert!(!dir.path().join("workflow-mcp.json5").exists());
}

#[cfg(windows)]
#[test]
fn mcp_connection_test_lists_tools_without_invoking_them() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("should-not-write.txt");
    let server = test_config(&marker).servers.remove("fixture").unwrap();
    let tools = inspect_mcp_server_tools(&server.command, &server.args).unwrap();
    assert!(tools.contains(&"echo".to_owned()));
    assert!(tools.contains(&"write_file".to_owned()));
    assert!(!marker.exists());
}

#[test]
#[ignore = "需本机 Node.js/npm 与网络；只握手和读取工具列表，不启动浏览器"]
fn npx_chrome_devtools_mcp_lists_tools() {
    let tools = inspect_mcp_server_tools("npx", &[
        "-y".into(), "chrome-devtools-mcp@latest".into(),
    ]).unwrap();
    assert!(tools.contains(&"navigate_page".to_owned()));
    assert!(tools.contains(&"take_snapshot".to_owned()));
}

#[test]
fn cmd_mcp_entry_supports_connection_test_and_workflow_call() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path().join("MCP service with spaces");
    std::fs::create_dir(&directory).unwrap();
    let entry = directory.join("stdio-server.cmd");
    std::fs::write(&entry, "@echo off\r\npowershell.exe %*\r\n").unwrap();
    let marker = directory.join("not written.txt");
    let mut config = test_config(&marker);
    let server = config.servers.get_mut("fixture").unwrap();
    server.command = entry.to_string_lossy().into_owned();
    let tools = inspect_mcp_server_tools(&server.command, &server.args).unwrap();
    assert!(tools.contains(&"echo".to_owned()));
    assert!(!marker.exists());

    let grant = grant_for("{op:'invoke',id:'read',uses:'mcp.call',with:{server:'fixture',tool:'echo'}}");
    let registry = make_registry(config, Some(grant));
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let result = registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"server":"fixture","tool":"echo","arguments":{"value":"hello"}}),
        &WorkflowRunControl::new(),
    ).unwrap();
    assert_eq!(result.output, Some(json!({"value":"hello"})));
    assert!(!marker.exists());
}

fn skill_settings(base_url: String) -> crate::ai::config::AiProviderSettings {
    let mut settings = crate::ai::config::AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = base_url;
    settings.model = "test-model".into();
    settings
}

fn mock_ai(turns: Vec<serde_json::Value>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        for turn in turns {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = Vec::new();
            let mut byte = [0u8; 1];
            while !header.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let header_text = String::from_utf8(header).unwrap();
            let size = header_text.lines().find_map(|line| line.to_ascii_lowercase()
                .strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok()))
                .unwrap_or(0);
            let mut body = vec![0u8; size];
            stream.read_exact(&mut body).unwrap();
            let response = format!("data: {}\n\ndata: [DONE]\n\n", turn);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    (url, thread)
}

fn ai_tool_turn(id: &str, server: &str, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,
        "function":{"name":"mcp_call","arguments":json!({
            "server":server,"tool":tool,"arguments":arguments
        }).to_string()}}]},"finish_reason":"tool_calls"}]})
}

#[test]
fn skill_package_is_frozen_before_authorization_and_requires_grant() {
    let dir = tempfile::tempdir().unwrap();
    test_skill(dir.path());
    assert_eq!(discover_skill_packages(dir.path()).unwrap(),
        vec![("fixture-skill".into(), "测试工具循环".into())]);
    let step = "{op:'invoke',id:'skill',uses:'skill.run',with:{skill:'fixture-skill',task:'读取',tools:[{server:'fixture',tool:'echo'}]}}";
    let mut grant = grant_for(step);
    assert!(grant.permission_lines(&test_config(&dir.path().join("unused"))).is_err());
    grant.prepare_skills(dir.path()).unwrap();
    let lines = grant.permission_lines(&test_config(&dir.path().join("unused"))).unwrap();
    assert!(lines.iter().any(|line| line.contains("AI Skill：fixture-skill")));
    std::fs::write(dir.path().join("workflow-skills/fixture-skill/SKILL.md"), "changed").unwrap();
    let denied = make_registry(test_config(&dir.path().join("unused")), None);
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let result = denied.get("skill.run").unwrap().execute_with_control(
        &service, &mut repo, &json!({"skill":"fixture-skill","task":"读取"}),
        &WorkflowRunControl::new());
    assert!(result.unwrap_err().to_string().contains("尚未授权"));
}

#[test]
fn skill_ai_tool_loop_uses_granted_mcp_and_records_result() {
    let dir = tempfile::tempdir().unwrap();
    test_skill(dir.path());
    let mut grant = grant_for("{op:'invoke',id:'skill',uses:'skill.run',with:{skill:'fixture-skill',task:'读取',tools:[{server:'fixture',tool:'echo'}]}}");
    grant.prepare_skills(dir.path()).unwrap();
    let (url, server) = mock_ai(vec![
        json!({"choices":[{"delta":{"content":"准备读取", "tool_calls":[{"index":0,"id":"call1","function":{"name":"mcp_call","arguments":"{\"server\":\"fixture\",\"tool\":\"echo\",\"arguments\":{\"value\":\"hello\"}}"}}]},"finish_reason":"tool_calls"}]}),
        json!({"choices":[{"delta":{"content":"已读取 hello"},"finish_reason":"stop"}]}),
    ]);
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, test_config(&dir.path().join("unused")),
        Some(grant), Some(skill_settings(url)), None, None).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let mut progress = Vec::new();
    let result = registry.get("skill.run").unwrap().execute_with_control_and_progress(&service, &mut repo,
        &json!({"skill":"fixture-skill","task":"读取","tools":[{"server":"fixture","tool":"echo"}]}),
        &WorkflowRunControl::new(), &mut |detail| progress.push(detail)).unwrap();
    server.join().unwrap();
    assert_eq!(result.output, Some(json!("已读取 hello")));
    assert!(progress.iter().any(|line| line.contains("AI 工具 1：fixture / echo")));
}

#[test]
fn skill_ai_cannot_call_tool_outside_step_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    test_skill(dir.path());
    let marker = dir.path().join("must-not-write.txt");
    let mut grant = grant_for("{op:'invoke',id:'skill',uses:'skill.run',with:{skill:'fixture-skill',task:'读取',tools:[{server:'fixture',tool:'echo'}]}}");
    grant.prepare_skills(dir.path()).unwrap();
    let (url, server) = mock_ai(vec![json!({"choices":[{"delta":{"tool_calls":[{
        "index":0,"id":"call1","function":{"name":"mcp_call",
            "arguments":"{\"server\":\"fixture\",\"tool\":\"write_file\",\"arguments\":{\"value\":\"bad\"}}"}
    }]},"finish_reason":"tool_calls"}]})]);
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, test_config(&marker), Some(grant),
        Some(skill_settings(url)), None, None).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let result = registry.get("skill.run").unwrap().execute_with_control(&service, &mut repo,
        &json!({"skill":"fixture-skill","task":"读取","tools":[{"server":"fixture","tool":"echo"}]}),
        &WorkflowRunControl::new());
    server.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("未授权工具"));
    assert!(!marker.exists());
}

#[test]
fn skill_page_guard_rejects_wrong_navigation_before_browser_call() {
    let dir = tempfile::tempdir().unwrap();
    test_skill(dir.path());
    let marker = dir.path().join("must-not-fill.txt");
    let arguments = json!({"skill":"fixture-skill","task":"填写",
        "browserGuard":{"url":"https://www.selenium.dev/selenium/web/web-form.html",
            "contains":["Web form","Text input"],"target":"input[name=\"my-text\"]"},
        "tools":[{"server":"fixture","tool":"browser_navigate"},
            {"server":"fixture","tool":"browser_snapshot"},
            {"server":"fixture","tool":"browser_type"}]});
    let definition: WorkflowDefinition = serde_json::from_value(json!({"version":2,
        "steps":[{"op":"invoke","id":"skill","uses":"skill.run","with":arguments}]})).unwrap();
    let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
    grant.prepare_skills(dir.path()).unwrap();
    let mut config = test_config(&marker);
    for (name, access) in [("browser_navigate", "write"), ("browser_snapshot", "read"), ("browser_type", "write")] {
        config.servers.get_mut("fixture").unwrap().tools.insert(name.into(),
            serde_json::from_value(json!({"access":access})).unwrap());
    }
    let (url, server) = mock_ai(vec![json!({"choices":[{"delta":{"tool_calls":[{
        "index":0,"id":"call1","function":{"name":"mcp_call",
            "arguments":"{\"server\":\"fixture\",\"tool\":\"browser_navigate\",\"arguments\":{\"url\":\"https://example.com\"}}"}
    }]},"finish_reason":"tool_calls"}]})]);
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, config, Some(grant),
        Some(skill_settings(url)), None, None).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let result = registry.get("skill.run").unwrap().execute_with_control(&service, &mut repo,
        &arguments, &WorkflowRunControl::new());
    server.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("页面守卫以外"));
    assert!(!marker.exists());
}

#[test]
fn skill_page_guard_reads_fills_and_verifies_with_mock_browser() {
    let dir = tempfile::tempdir().unwrap();
    test_skill(dir.path());
    let marker = dir.path().join("filled.txt");
    let target_url = "https://www.selenium.dev/selenium/web/web-form.html";
    let target = "input[name=\"my-text\"]";
    let arguments = json!({"skill":"fixture-skill","task":"填写 hello",
        "browserGuard":{"url":target_url,"contains":["Web form","Text input"],"target":target},
        "tools":[{"server":"fixture","tool":"browser_navigate"},
            {"server":"fixture","tool":"browser_snapshot"},
            {"server":"fixture","tool":"browser_type"}]});
    let definition: WorkflowDefinition = serde_json::from_value(json!({"version":2,
        "steps":[{"op":"invoke","id":"skill","uses":"skill.run","with":arguments}]})).unwrap();
    let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
    grant.prepare_skills(dir.path()).unwrap();
    let mut config = test_config(&marker);
    for (name, access) in [("browser_navigate", "write"), ("browser_snapshot", "read"), ("browser_type", "write")] {
        config.servers.get_mut("fixture").unwrap().tools.insert(name.into(),
            serde_json::from_value(json!({"access":access})).unwrap());
    }
    let (url, server) = mock_ai(vec![
        ai_tool_turn("c1", "fixture", "browser_navigate", json!({"url":target_url})),
        ai_tool_turn("c2", "fixture", "browser_snapshot", json!({})),
        ai_tool_turn("c3", "fixture", "browser_type", json!({"target":target,"text":"hello","submit":false})),
        ai_tool_turn("c4", "fixture", "browser_snapshot", json!({})),
        json!({"choices":[{"delta":{"content":"已填写 hello，未提交"},"finish_reason":"stop"}]}),
    ]);
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, config, Some(grant),
        Some(skill_settings(url)), None, None).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let mut details = Vec::new();
    let result = registry.get("skill.run").unwrap().execute_with_control_and_progress(
        &service, &mut repo, &arguments, &WorkflowRunControl::new(),
        &mut |detail| details.push(detail)).unwrap();
    server.join().unwrap();
    assert_eq!(result.output, Some(json!("已填写 hello，未提交")));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "hello");
    assert!(details.iter().any(|detail| detail.contains("AI 工具 4：fixture / browser_snapshot")));
}

#[test]
#[ignore = "需要下载 Playwright MCP 依赖"]
fn browser_runtime_downloads_and_starts_without_npx() {
    let data = tempfile::tempdir().unwrap();
    super::browser_runtime::install(data.path(), &crate::proxy::NetworkProxySettings::default(),
        |message| eprintln!("{message}")).unwrap();
    let (node, mut args) = super::browser_runtime::launch_command(data.path()).unwrap();
    args.push("--help".into());
    let output = std::process::Command::new(node).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "需要下载 Node 与 Playwright MCP 依赖"]
fn browser_runtime_downloads_node_when_system_node_is_unavailable() {
    let data = tempfile::tempdir().unwrap();
    super::browser_runtime::install_without_system_node(data.path(),
        &crate::proxy::NetworkProxySettings::default(),
        |message| eprintln!("{message}")).unwrap();
    let (node, mut args) = super::browser_runtime::launch_command(data.path()).unwrap();
    assert!(node.starts_with(data.path()));
    args.push("--help".into());
    let output = std::process::Command::new(node).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn builtin_edge_mcp_requires_no_user_config_and_legacy_sample_is_migrated_in_memory() {
    let data = tempfile::tempdir().unwrap();
    let config = super::extensions::load_mcp_config(data.path()).unwrap();
    let builtin = config.servers.get(super::browser_runtime::SERVER_ID).unwrap();
    assert_eq!(builtin.command, super::browser_runtime::COMMAND_MARKER);
    assert!(builtin.tools.contains_key("browser_snapshot"));
    assert!(!super::browser_runtime::ready(data.path()));
    let mut config = config;
    config.configure_browser_proxy(&crate::proxy::NetworkProxySettings::default()).unwrap();
    assert!(config.servers[super::browser_runtime::SERVER_ID].args.is_empty());
    let different_proxies: crate::proxy::NetworkProxySettings = serde_json::from_value(json!({
        "mode": "Custom",
        "custom": {
            "http_proxy": "http://127.0.0.1:8001",
            "https_proxy": "http://127.0.0.1:8002"
        }
    })).unwrap();
    assert!(config.configure_browser_proxy(&different_proxies).is_err());

    let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/examples/workflow-edge-demo/workflow-mcp.json5");
    std::fs::copy(sample, data.path().join("workflow-mcp.json5")).unwrap();
    let config = super::extensions::load_mcp_config(data.path()).unwrap();
    assert_eq!(config.servers[super::browser_runtime::LEGACY_SERVER_ID].command,
        super::browser_runtime::COMMAND_MARKER);
    assert!(std::fs::read_to_string(data.path().join("workflow-mcp.json5")).unwrap()
        .contains("npx.cmd"));
}

#[test]
fn edge_demo_templates_parse_and_allow_only_read_fill_tools() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/workflow-edge-demo");
    let (_repo_dir, repo, service) = git_support::init_repo();
    for file in ["edge-web-form.json5", "edge-web-form-skill.json5"] {
        let definition = parse_workflow_json5(&std::fs::read_to_string(root.join(file)).unwrap()).unwrap();
        let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
        grant.prepare_skills(&root).unwrap();
        let config = super::extensions::load_mcp_config(&root).unwrap();
        let lines = grant.permission_lines(&config).unwrap();
        assert!(lines.iter().any(|line| line.contains("browser_type")));
        assert!(!lines.iter().any(|line| line.contains("browser_click")));
        let mut registry = WorkflowActionRegistry::default();
        register_external_actions_with_ai(&mut registry, config, None,
            Some(skill_settings("http://127.0.0.1".into())), None, Some(&root)).unwrap();
        let mut options = WorkflowRunOptions::default();
        options.input_vars.insert("demoText".into(), "preview".into());
        assert_eq!(WorkflowExecutor::with_actions(&service, &registry)
            .preview(&repo, &definition, &options).unwrap().steps.len(), 1);
    }
}

#[test]
fn edge_demo_stops_before_fill_when_page_snapshot_is_wrong() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/workflow-edge-demo");
    let definition = parse_workflow_json5(&std::fs::read_to_string(root.join("edge-web-form.json5")).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("wrong-page.txt");
    let mut config = test_config(&marker);
    let mut server = config.servers.remove("fixture").unwrap();
    for (name, access) in [("browser_navigate", "write"), ("browser_snapshot", "read"), ("browser_type", "write")] {
        server.tools.insert(name.into(), serde_json::from_value(json!({"access":access})).unwrap());
    }
    config.servers.insert("browser.edge".into(), server);
    let grant = WorkflowExternalGrant::for_definition(&definition).unwrap();
    let registry = make_registry(config, grant);
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let mut options = WorkflowRunOptions::default();
    options.input_vars.insert("demoText".into(), "must-not-appear".into());
    let result = WorkflowExecutor::with_actions(&service, &registry).run(&mut repo, &definition,
        options, |_| {});
    assert!(result.unwrap_err().to_string().contains("JavaScript 执行失败"));
    assert!(!marker.exists());
}

#[test]
#[ignore = "需要本机 Edge、网络和可用的 Selenium 演示页"]
fn edge_demo_reads_and_fills_real_public_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/workflow-edge-demo");
    let definition = parse_workflow_json5(&std::fs::read_to_string(root.join("edge-web-form.json5")).unwrap()).unwrap();
    let runtime = tempfile::tempdir().unwrap();
    super::browser_runtime::install(runtime.path(), &crate::proxy::NetworkProxySettings::default(), |_| {}).unwrap();
    let mut config = super::extensions::load_mcp_config(runtime.path()).unwrap();
    config.configure_browser_proxy(&crate::proxy::NetworkProxySettings::default()).unwrap();
    let grant = WorkflowExternalGrant::for_definition(&definition).unwrap();
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, config, grant, None, None,
        Some(runtime.path())).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let mut options = WorkflowRunOptions::default();
    options.input_vars.insert("demoText".into(), "Khaslana Edge demo".into());
    let result = WorkflowExecutor::with_actions(&service, &registry).run(&mut repo, &definition, options, |_| {});
    assert!(result.is_ok(), "{}", result.unwrap_err());
}

#[test]
#[ignore = "需要显式提供 AI 与代理配置、本机 Edge 和网络；会产生模型请求"]
fn edge_skill_reads_and_fills_real_public_page() {
    // 配置仅通过当前测试进程环境传入，不能将真实密钥写进样板或测试输出。
    let mut settings: crate::ai::config::AiProviderSettings = serde_json::from_str(
        &std::env::var("KHASLANA_WORKFLOW_AI_SETTINGS").expect("请提供验收用 AI 配置")
    ).expect("验收用 AI 配置格式无效");
    let defaults = crate::ai::config::AiProviderSettings::default();
    settings.temperature = defaults.temperature;
    settings.max_tokens = defaults.max_tokens;
    settings.request_timeout_secs = defaults.request_timeout_secs;
    assert!(settings.is_usable(), "验收用 AI 配置未启用或不完整");
    let proxy: crate::proxy::NetworkProxySettings = serde_json::from_str(
        &std::env::var("KHASLANA_WORKFLOW_PROXY_SETTINGS").expect("请提供验收用代理配置")
    ).expect("验收用代理配置格式无效");
    proxy.validate().unwrap();
    let proxy_url = proxy.proxy_url_for_target(&settings.base_url);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/workflow-edge-demo");
    let definition = parse_workflow_json5(
        &std::fs::read_to_string(root.join("edge-web-form-skill.json5")).unwrap()
    ).unwrap();
    let runtime = tempfile::tempdir().unwrap();
    super::browser_runtime::install(runtime.path(), &proxy, |_| {}).unwrap();
    let mut config = super::extensions::load_mcp_config(runtime.path()).unwrap();
    config.configure_browser_proxy(&proxy).unwrap();
    let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
    grant.prepare_skills(&root).unwrap();
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, config, Some(grant), Some(settings),
        proxy_url, Some(runtime.path())).unwrap();
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let mut options = WorkflowRunOptions::default();
    options.input_vars.insert("demoText".into(), "Khaslana AI Skill acceptance".into());
    let mut details = Vec::new();
    let result = WorkflowExecutor::with_actions(&service, &registry).run(&mut repo,
        &definition, options, |event| {
            if let WorkflowProgressEvent::StepDetail { detail, .. } = event {
                details.push(detail);
            }
        });
    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert!(details.iter().any(|detail| detail.contains("browser_type")));
    assert!(details.iter().any(|detail| detail.contains("browser_snapshot")));
}

#[test]
fn mcp_read_write_schema_and_permission_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("written.txt");
    let config = test_config(&marker);
    let grant = grant_for(r#"
        {op:'invoke', id:'read', uses:'mcp.call', with:{server:'fixture',tool:'echo'}},
        {op:'invoke', id:'write', uses:'mcp.call', with:{server:'fixture',tool:'write_file'}}
    "#);
    let permissions = grant.permission_lines(&config).unwrap();
    assert!(permissions.iter().any(|line| line.contains("读取 MCP 工具：fixture / echo")));
    assert!(permissions.iter().any(|line| line.contains("写入 MCP 工具：fixture / write_file")));
    let registry = make_registry(config.clone(), Some(grant));
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let control = WorkflowRunControl::new();
    let read = registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"server":"fixture","tool":"echo","arguments":{"value":"hello"}}),
        &control,
    ).unwrap();
    assert_eq!(read.output, Some(json!({"value":"hello"})));
    let invalid = registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"server":"fixture","tool":"write_file","arguments":{"value":3}}),
        &control,
    );
    assert!(invalid.unwrap_err().to_string().contains("schema"));
    assert!(!marker.exists());
    let write = registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"server":"fixture","tool":"write_file","arguments":{"value":"saved"}}),
        &control,
    ).unwrap();
    assert_eq!(write.output, Some(json!({"written":true})));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "saved");

    let denied_registry = make_registry(config, None);
    let denied = denied_registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"server":"fixture","tool":"write_file","arguments":{"value":"changed"}}),
        &control,
    );
    assert!(denied.unwrap_err().to_string().contains("尚未授权"));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "saved");
}

#[test]
fn js_transforms_data_and_calls_only_declared_tools() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(&dir.path().join("untouched.txt"));
    let script = "const reply = mcp.call('fixture', 'echo', {value: input.value}); return {upper: reply.value.toUpperCase()};";
    let definition = parse_workflow_json5(&format!(r#"{{version:2,steps:[{{op:'invoke',id:'js',uses:'js.run',with:{{script:{},tools:[{{server:'fixture',tool:'echo'}}]}}}}]}}"#,
        serde_json::to_string(script).unwrap())).unwrap();
    let grant = WorkflowExternalGrant::for_definition(&definition).unwrap();
    let registry = make_registry(config, grant);
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let result = registry.get("js.run").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"script":script,"tools":[{"server":"fixture","tool":"echo"}],
            "input":{"value":"hello"}}),
        &WorkflowRunControl::new(),
    ).unwrap();
    assert_eq!(result.output, Some(json!({"upper":"HELLO"})));

    let denied_script = "return mcp.call('fixture', 'write_file', {value:'bad'});";
    let denied_grant = grant_for(&format!(r#"{{op:'invoke',id:'js',uses:'js.run',with:{{script:{},tools:[{{server:'fixture',tool:'echo'}}]}}}}"#,
        serde_json::to_string(denied_script).unwrap()));
    let denied_registry = make_registry(test_config(&dir.path().join("untouched.txt")), Some(denied_grant));
    let denied = denied_registry.get("js.run").unwrap().execute_with_control(
        &service, &mut repo,
        &json!({"script":denied_script,"tools":[{"server":"fixture","tool":"echo"}]}),
        &WorkflowRunControl::new(),
    );
    assert!(denied.is_err());
    assert!(!dir.path().join("untouched.txt").exists());
}

#[test]
fn js_infinite_loop_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let script = "for (;;) {}";
    let grant = grant_for(&format!(r#"{{op:'invoke',id:'js',uses:'js.run',with:{{script:{}}}}}"#,
        serde_json::to_string(script).unwrap()));
    let registry = make_registry(test_config(&dir.path().join("unused.txt")), Some(grant));
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let control = WorkflowRunControl::new();
    let cancel = control.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    let result = registry.get("js.run").unwrap().execute_with_control(
        &service, &mut repo, &json!({"script":script}), &control);
    thread.join().unwrap();
    assert!(matches!(result, Err(GitError::WorkflowCancelled)));
}

#[test]
fn v2_workflow_passes_mcp_output_through_js_to_write_tool() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("workflow-result.txt");
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": 2,
        "defaults": {"requireCleanWorktree": true},
        "steps": [
            {"op":"invoke","id":"read","uses":"mcp.call","saveAs":"readResult",
                "with":{"server":"fixture","tool":"echo","arguments":{"value":"hello"}}},
            {"op":"invoke","id":"transform","uses":"js.run","saveAs":"prepared",
                "with":{"script":"return {value: input.value.toUpperCase()};",
                    "input":{"value":"${readResult.value}"}}},
            {"op":"invoke","id":"write","uses":"mcp.call",
                "with":{"server":"fixture","tool":"write_file",
                    "arguments":{"value":"${prepared.value}"}}}
        ]
    })).unwrap();
    let grant = WorkflowExternalGrant::for_definition(&definition).unwrap();
    let registry = make_registry(test_config(&marker), grant);
    let (repo_dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(repo_dir.path(), "README.md", "initial\n");
    git_support::commit_all(&repo, "initial");
    let executor = WorkflowExecutor::with_actions(&service, &registry);
    let preview = executor.preview(&repo, &definition, &WorkflowRunOptions::default()).unwrap();
    assert_eq!(preview.steps.len(), 3);
    let result = executor.run(&mut repo, &definition, WorkflowRunOptions::default(), |_| {}).unwrap();
    assert_eq!(result.steps_run, 3);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "HELLO");
}

#[test]
fn mcp_call_can_be_cancelled_while_server_is_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let registry = make_registry(test_config(&dir.path().join("unused.txt")),
        Some(grant_for("{op:'invoke',id:'stall',uses:'mcp.call',with:{server:'fixture',tool:'stall'}}")));
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let control = WorkflowRunControl::new();
    let cancel = control.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel.cancel();
    });
    let started = std::time::Instant::now();
    let result = registry.get("mcp.call").unwrap().execute_with_control(
        &service, &mut repo, &json!({"server":"fixture","tool":"stall"}), &control);
    thread.join().unwrap();
    assert!(matches!(result, Err(GitError::WorkflowCancelled)));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn external_wait_stops_at_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let control = WorkflowRunControl::new();
    let result: Result<()> = runtime.block_on(super::extensions::wait_checked(
        std::future::pending::<std::result::Result<(), ()>>(),
        &control, std::time::Instant::now() + Duration::from_millis(100), "MCP 工具调用",
    ));
    assert!(result.unwrap_err().to_string().contains("超时"));
}

#[test]
fn external_permission_rejects_dynamic_tool_identity() {
    let definition = parse_workflow_json5(r#"{
        version: 2,
        steps: [{op:'invoke',id:'call',uses:'mcp.call',
            with:{server:'fixture',tool:'${selectedTool}',arguments:{}}}]
    }"#).unwrap();
    assert!(WorkflowExternalGrant::for_definition(&definition).unwrap_err()
        .to_string().contains("固定值"));
}

#[test]
fn mcp_session_persists_between_steps_of_one_run() {
    let dir = tempfile::tempdir().unwrap();
    let grant = grant_for(r#"
        {op:'invoke',id:'set',uses:'mcp.call',with:{server:'fixture',tool:'set_state'}},
        {op:'invoke',id:'get',uses:'mcp.call',with:{server:'fixture',tool:'get_state'}}
    "#);
    let registry = make_registry(test_config(&dir.path().join("unused.txt")), Some(grant));
    let (_repo_dir, mut repo, service) = git_support::init_repo();
    let control = WorkflowRunControl::new();
    registry.get("mcp.call").unwrap().execute_with_control(&service, &mut repo,
        &json!({"server":"fixture","tool":"set_state","arguments":{"value":"same session"}}),
        &control).unwrap();
    let result = registry.get("mcp.call").unwrap().execute_with_control(&service, &mut repo,
        &json!({"server":"fixture","tool":"get_state"}), &control).unwrap();
    assert_eq!(result.output, Some(json!({"value":"same session"})));
}
