use std::path::Path;

use git2::RepositoryInitOptions;
use tempfile::TempDir;

use super::*;
use crate::git::test_support::git_test_support as git_support;

fn init_remote_workflow_repo() -> (TempDir, TempDir, Repository, GitService) {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (source_dir, mut source, service) = git_support::init_repo();
    git_support::write_file(source_dir.path(), "README.md", "hello\n");
    git_support::commit_all(&source, "initial");
    source
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push_branch(
            &mut source,
            &RemoteName::new("origin"),
            &BranchName::new("main"),
            true,
        )
        .unwrap();
    service
        .create_branch_from(
            &mut source,
            &BranchName::new("existing"),
            Some(&BranchName::new("main")),
            true,
        )
        .unwrap();
    service
        .push_branch(
            &mut source,
            &RemoteName::new("origin"),
            &BranchName::new("existing"),
            true,
        )
        .unwrap();

    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &crate::RepoPath::new(&clone_path),
        )
        .unwrap();
    let mut clone = Repository::open(&clone_path).unwrap();
    git_support::configure_user(&clone);
    service
        .fetch(&mut clone, &RemoteName::new("origin"))
        .unwrap();
    (remote_dir, clone_dir, clone, service)
}

fn create_remote_branch(remote_dir: &Path, branch: &str) {
    let service = git_support::service();
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("work");
    service
        .clone_repo(
            &git_support::path_url(remote_dir),
            &crate::RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    service
        .create_branch_from(
            &mut repo,
            &BranchName::new(branch),
            Some(&BranchName::new("main")),
            true,
        )
        .unwrap();
    git_support::write_file(&work_path, "branch.txt", branch);
    git_support::commit_all(&repo, "remote branch");
    service
        .push_branch(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new(branch),
            true,
        )
        .unwrap();
}

#[test]
fn parses_json5_with_comments_and_variables() {
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          name: "demo",
          vars: {
            target: "release/${date:%Y%m%d}",
          },
          // comment
          steps: [
            { op: "checkout", branch: "main" },
            { op: "createBranch", name: "${target}", from: "main", checkout: true },
          ],
        }
        "#,
    )
    .unwrap();

    assert_eq!(definition.display_name(), "demo");
    assert_eq!(definition.steps.len(), 2);
}

#[test]
fn parses_workflow_inputs() {
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          inputs: {
            target: {
              label: "目标分支",
              description: "运行前填写",
              default: "feature/${date:%Y%m%d}",
            },
            optionalName: { required: false },
          },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();

    let target = definition.inputs.get("target").unwrap();
    assert_eq!(target.label.as_deref(), Some("目标分支"));
    assert_eq!(target.description.as_deref(), Some("运行前填写"));
    assert!(target.required);
    assert!(!definition.inputs.get("optionalName").unwrap().required);
}

#[test]
fn rejects_unknown_version() {
    let err =
        parse_workflow_json5("{ version: 99, steps: [{ op: \"ensureClean\" }] }").unwrap_err();
    assert!(err.to_string().contains("不支持的工作流版本"));
}

#[test]
fn rejects_builtin_input_names() {
    let err = parse_workflow_json5(
        r#"
        {
          version: 1,
          inputs: { "git.currentBranch": { default: "main" } },
          steps: [{ op: "ensureClean" }],
        }
        "#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("内置变量名"));
}

#[test]
fn detects_variable_cycles() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: { a: "${b}", b: "${a}" },
          steps: [{ op: "assertBranch", branch: "${a}" }],
        }
        "#,
    )
    .unwrap();
    let executor = WorkflowExecutor::new(&service);
    let err = executor
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap_err();
    assert!(err.to_string().contains("循环引用"));
}

#[test]
fn input_values_override_vars() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          inputs: { target: { default: "from-input" } },
          vars: { target: "from-vars" },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();
    let options = WorkflowRunOptions {
        input_vars: BTreeMap::from([("target".to_string(), "chosen".to_string())]),
        ..WorkflowRunOptions::default()
    };

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &options)
        .unwrap();

    assert_eq!(
        preview.steps[0].summary,
        "基于 当前 HEAD 创建分支 chosen并切换"
    );
}

#[test]
fn required_input_values_must_not_be_empty() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          inputs: { target: { label: "目标分支" } },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap_err();

    assert!(err.to_string().contains("请填写工作流变量：目标分支"));
}

#[test]
fn resolves_input_defaults_with_existing_variables() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: { prefix: "feature" },
          inputs: { target: { default: "${prefix}/${git.initialBranch}" } },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();
    let default = WorkflowExecutor::new(&service)
        .resolve_template(
            &repo,
            &definition,
            &WorkflowRunOptions::default(),
            definition.inputs["target"].default.as_deref().unwrap(),
        )
        .unwrap();

    assert_eq!(default, "feature/main");
}

#[test]
fn workflow_methods_can_build_branch_names_from_variables() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: {
            rawBranch: "feature/User Story_123",
            target: "tmp/${rawBranch | split:'/' | last | slug | truncate:12}",
          },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(
        preview.steps[0].summary,
        "基于 当前 HEAD 创建分支 tmp/user-story-1并切换"
    );
}

#[test]
fn workflow_methods_work_in_input_defaults() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          inputs: {
            target: { default: "feature/${git.initialBranch | split:'/' | last | slug}" },
          },
          steps: [{ op: "assertBranch", branch: "${target}" }],
        }
        "#,
    )
    .unwrap();

    let default = WorkflowExecutor::new(&service)
        .resolve_template(
            &repo,
            &definition,
            &WorkflowRunOptions::default(),
            definition.inputs["target"].default.as_deref().unwrap(),
        )
        .unwrap();

    assert_eq!(default, "feature/main");
}

#[test]
fn guard_remote_branch_defaults_are_fail_on_exists_and_continue_on_missing() {
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [{ op: "guardRemoteBranch", branch: "target" }],
        }
        "#,
    )
    .unwrap();

    let WorkflowStep::GuardRemoteBranch {
        remote,
        branch,
        fetch,
        on_exists,
        on_missing,
    } = &definition.steps[0]
    else {
        panic!("expected guardRemoteBranch");
    };

    assert!(remote.is_none());
    assert_eq!(branch, "target");
    assert!(*fetch);
    assert_eq!(*on_exists, RemoteBranchGuardAction::Fail);
    assert_eq!(*on_missing, RemoteBranchGuardAction::Continue);
}

#[test]
fn guard_remote_branch_preview_shows_policy_and_does_not_change_current_branch() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            { op: "guardRemoteBranch", remote: "origin", branch: "target", fetch: false },
            { op: "push", remote: "origin" },
          ],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(
        preview.steps[0].summary,
        "检查远端分支 origin/target（基于本地引用，存在则停止，不存在则继续）"
    );
    assert_eq!(preview.steps[1].summary, "推送分支 main 到 origin");
}

#[test]
fn guard_remote_branch_fails_when_remote_branch_exists_by_default() {
    let (_remote_dir, _clone_dir, mut repo, service) = init_remote_workflow_repo();
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [{ op: "guardRemoteBranch", remote: "origin", branch: "existing", fetch: false }],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap_err();

    assert!(err.to_string().contains("远端分支已存在：origin/existing"));
}

#[test]
fn guard_remote_branch_can_continue_when_remote_branch_exists() {
    let (_remote_dir, _clone_dir, mut repo, service) = init_remote_workflow_repo();
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "guardRemoteBranch",
              remote: "origin",
              branch: "existing",
              fetch: false,
              onExists: "continue",
            },
            { op: "assertBranch", branch: "main" },
          ],
        }
        "#,
    )
    .unwrap();

    let result = WorkflowExecutor::new(&service)
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap();

    assert_eq!(result.steps_run, 2);
}

#[test]
fn guard_remote_branch_can_fail_when_remote_branch_is_missing() {
    let (_remote_dir, _clone_dir, mut repo, service) = init_remote_workflow_repo();
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "guardRemoteBranch",
              remote: "origin",
              branch: "missing",
              fetch: false,
              onExists: "continue",
              onMissing: "fail",
            },
          ],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap_err();

    assert!(err.to_string().contains("远端分支不存在：origin/missing"));
}

#[test]
fn guard_remote_branch_fetch_true_refreshes_remote_refs_before_checking() {
    let (remote_dir, _clone_dir, mut repo, service) = init_remote_workflow_repo();
    create_remote_branch(remote_dir.path(), "fresh");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [{ op: "guardRemoteBranch", remote: "origin", branch: "fresh" }],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap_err();

    assert!(err.to_string().contains("远端分支已存在：origin/fresh"));
}

#[test]
fn guard_remote_branch_fetch_false_uses_local_remote_refs_only() {
    let (remote_dir, _clone_dir, mut repo, service) = init_remote_workflow_repo();
    create_remote_branch(remote_dir.path(), "fresh");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [{ op: "guardRemoteBranch", remote: "origin", branch: "fresh", fetch: false }],
        }
        "#,
    )
    .unwrap();

    let result = WorkflowExecutor::new(&service)
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap();

    assert_eq!(result.steps_run, 1);
}

#[test]
fn guard_remote_branch_rejects_branch_with_remote_prefix() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [{ op: "guardRemoteBranch", remote: "origin", branch: "origin/target" }],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap_err();

    assert!(err.to_string().contains("不要带远端名前缀"));
}

#[test]
fn guard_remote_branch_branch_supports_expression_methods() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: { target: "feature/demo" },
          steps: [
            { op: "guardRemoteBranch", remote: "origin", branch: "${target | split:'/' | last}", fetch: false },
          ],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(
        preview.steps[0].summary,
        "检查远端分支 origin/demo（基于本地引用，存在则停止，不存在则继续）"
    );
}

#[test]
fn final_array_workflow_expression_is_rejected() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: { target: "${git.initialBranch | split:'/'}" },
          steps: [{ op: "createBranch", name: "${target}" }],
        }
        "#,
    )
    .unwrap();

    let err = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap_err();

    assert!(err.to_string().contains("最终结果是数组"));
}

#[test]
fn preview_uses_checkout_branch_for_implicit_push_branch() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            { op: "checkout", branch: "test" },
            { op: "push", remote: "origin" },
          ],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(preview.steps[1].summary, "推送分支 test 到 origin");
}

#[test]
fn preview_tracks_create_branch_checkout_for_current_branch() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            { op: "createBranch", name: "A", checkout: true },
            { op: "assertBranch", branch: "${git.currentBranch}" },
            { op: "push", remote: "origin" },
          ],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(preview.steps[1].summary, "确认当前分支是 A");
    assert_eq!(preview.steps[2].summary, "推送分支 A 到 origin");
}

#[test]
fn preview_does_not_track_create_branch_without_checkout() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello\n");
    git_support::commit_all(&repo, "initial");
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            { op: "createBranch", name: "A", checkout: false },
            { op: "push", remote: "origin" },
          ],
        }
        "#,
    )
    .unwrap();

    let preview = WorkflowExecutor::new(&service)
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();

    assert_eq!(preview.steps[1].summary, "推送分支 main 到 origin");
}

#[test]
fn workflow_creates_branch_merges_and_asserts_dynamic_variables() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch_from(
            &mut repo,
            &BranchName::new("B"),
            Some(&BranchName::new("main")),
            true,
        )
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();

    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          vars: {
            target: "A-${git.initialBranch}",
          },
          steps: [
            { op: "checkout", branch: "main" },
            { op: "createBranch", name: "${target}", from: "main", checkout: true },
            { op: "merge", branch: "B" },
            { op: "assertBranch", branch: "${target}" },
          ],
        }
        "#,
    )
    .unwrap();

    let executor = WorkflowExecutor::new(&service);
    let mut events = Vec::new();
    let result = executor
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |event| events.push(event),
        )
        .unwrap();

    assert_eq!(result.steps_run, 4);
    assert_eq!(service.current_branch(&repo).as_deref(), Some("A-main"));
    assert!(dir.path().join("feature.txt").exists());
    assert!(events.len() >= 6);
}

#[test]
fn default_clean_worktree_check_rejects_dirty_repo() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "README.md", "dirty\n");
    let definition =
        parse_workflow_json5("{ version: 1, steps: [{ op: \"ensureClean\" }] }").unwrap();
    let executor = WorkflowExecutor::new(&service);

    let err = executor
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap_err();

    assert!(err.to_string().contains("工作区存在未提交更改"));
}
