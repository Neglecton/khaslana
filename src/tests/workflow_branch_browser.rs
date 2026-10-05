use super::*;
use std::sync::{Arc, Mutex};
use crate::BranchName;

const MCP_ID: &str = "chrome-devtools";
const PAGE_URL: &str = "https://www.selenium.dev/selenium/web/web-form.html";

fn sample_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/branch-to-browser")
}

fn sample_definition() -> WorkflowDefinition {
    parse_workflow_json5(&std::fs::read_to_string(sample_root().join("create-merge-fill.json5")).unwrap()).unwrap()
}

fn browser_config(marker: &Path) -> WorkflowMcpConfig {
    let mut config = test_config(marker);
    let mut server = config.servers.remove("fixture").unwrap();
    for (name, access) in [("new_page", "write"), ("take_snapshot", "read"), ("fill", "write")] {
        server.tools.insert(name.into(), serde_json::from_value(json!({"access":access})).unwrap());
    }
    config.servers.insert(MCP_ID.into(), server);
    config
}

fn options(target: &str) -> WorkflowRunOptions {
    let mut options = WorkflowRunOptions::default();
    options.input_vars.insert("base".into(), "main".into());
    options.input_vars.insert("target".into(), target.into());
    options.input_vars.insert("source".into(), "source".into());
    options
}

fn repo_with_source() -> (tempfile::TempDir, Repository, GitService, git2::Oid) {
    let (directory, mut repo, service) = git_support::init_repo();
    git_support::write_file(directory.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");
    service.create_branch_from(&mut repo, &BranchName::new("source"),
        Some(&BranchName::new("main")), true).unwrap();
    git_support::write_file(directory.path(), "source.txt", "merged\n");
    let source = git_support::commit_all(&repo, "source change");
    service.checkout_branch(&mut repo, &BranchName::new("main")).unwrap();
    (directory, repo, service, source)
}

fn recorded_ai(turns: Vec<Value>) -> (String, std::thread::JoinHandle<()>, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let thread = std::thread::spawn(move || {
        for turn in turns {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                    Err(error) => panic!("等待 AI 请求失败：{error}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let mut header = Vec::new();
            let mut byte = [0u8; 1];
            while !header.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let size = String::from_utf8(header).unwrap().lines().find_map(|line|
                line.to_ascii_lowercase().strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())).unwrap();
            let mut body = vec![0; size];
            stream.read_exact(&mut body).unwrap();
            captured.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
            let response = format!("data: {turn}\n\ndata: [DONE]\n\n");
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    (url, thread, requests)
}

#[test]
fn branch_is_created_merged_and_passed_to_skill_browser_fill() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("filled.txt");
    let (repo_directory, mut repo, service, source) = repo_with_source();
    let definition = sample_definition();
    let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
    grant.prepare_skills(&sample_root()).unwrap();
    let config = browser_config(&marker);
    let lines = grant.permission_lines(&config).unwrap();
    assert!(lines.iter().any(|line| line.contains("branch-to-browser")));
    assert!(!lines.iter().any(|line| line.contains("write_file")));
    let target = "demo/fill-feature-123";
    let (url, thread, requests) = recorded_ai(vec![
        ai_tool_turn("open", MCP_ID, "new_page", json!({"url":PAGE_URL})),
        ai_tool_turn("read", MCP_ID, "take_snapshot", json!({"pageId":1})),
        ai_tool_turn("fill", MCP_ID, "fill", json!({"pageId":1,"uid":"1_1","value":target})),
        ai_tool_turn("verify", MCP_ID, "take_snapshot", json!({"pageId":1})),
        json!({"choices":[{"delta":{"content":format!("已填写 {target} 并复核，未提交")},"finish_reason":"stop"}]}),
    ]);
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, config, Some(grant),
        Some(skill_settings(url)), None, Some(&sample_root())).unwrap();
    let result = WorkflowExecutor::with_actions(&service, &registry)
        .run(&mut repo, &definition, options(target), |_| {});
    assert!(result.is_ok(), "{}", result.unwrap_err());
    thread.join().unwrap();
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), target);
    assert_eq!(repo.head().unwrap().target(), Some(source));
    git_support::assert_file_text(repo_directory.path(), "source.txt", "merged\n");
    assert_eq!(std::fs::read_to_string(marker).unwrap(), target);
    let requests = requests.lock().unwrap();
    let first_messages = requests[0]["messages"].to_string();
    assert!(first_messages.contains(target));
    assert!(!first_messages.contains("${target}"));
    assert!(first_messages.contains("inputSchema"));
    assert!(first_messages.contains("pageId"));
    assert!(!first_messages.contains("write_file"));
    assert!(requests.last().unwrap()["messages"].to_string().contains("value=demo/fill-feature-123"));
}

#[test]
fn failed_merge_stops_before_skill_or_browser_runs() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("must-not-fill.txt");
    let (_repo_directory, mut repo, service, _) = repo_with_source();
    let definition = sample_definition();
    let mut grant = WorkflowExternalGrant::for_definition(&definition).unwrap().unwrap();
    grant.prepare_skills(&sample_root()).unwrap();
    let mut registry = WorkflowActionRegistry::default();
    register_external_actions_with_ai(&mut registry, browser_config(&marker), Some(grant),
        Some(skill_settings("http://127.0.0.1:1".into())), None, Some(&sample_root())).unwrap();
    let mut options = options("demo/merge-missing");
    options.input_vars.insert("source".into(), "missing-branch".into());
    let mut started = Vec::new();
    let result = WorkflowExecutor::with_actions(&service, &registry).run(&mut repo, &definition,
        options, |event| if let WorkflowProgressEvent::StepStarted { index, .. } = event { started.push(index); });
    assert!(result.is_err());
    assert_eq!(started, vec![0, 1]);
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), "demo/merge-missing");
    assert!(!marker.exists());
}
