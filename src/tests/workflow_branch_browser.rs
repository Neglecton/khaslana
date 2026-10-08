use super::*;
use crate::BranchName;

const MCP_ID: &str = "chrome-devtools";

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
