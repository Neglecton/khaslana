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

/// 构造一个带多个命名分支的临时仓库，用于 filterBranches / deleteBranches 测试。
/// 当前分支为 main，并额外创建若干符合 (dev|uat|release)_xxx_yyyyMMdd_xxx 命名规则的分支。
fn init_named_branches_repo() -> (TempDir, Repository, GitService) {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");
    // 基于 main 创建若干命名分支（不切换），分支名内嵌日期
    for name in [
        "dev_wzf_20250418_测试系统",
        "uat_wzf_20250520_测试系统",
        "release_wzf_20250601_发布",
        "dev_abc_20990101_未来", // 未来日期，不应被选中
        "feature_random_branch", // 不匹配前缀
    ] {
        service
            .create_branch_from(
                &mut repo,
                &BranchName::new(name),
                Some(&BranchName::new("main")),
                false,
            )
            .unwrap();
    }
    (dir, repo, service)
}

#[test]
fn months_between_counts_calendar_months() {
    // 同年同月差为 0
    assert_eq!(months_between(date(2026, 7, 1), date(2026, 7, 31)), 0);
    // 同年跨月
    assert_eq!(months_between(date(2026, 1, 31), date(2026, 7, 1)), 6);
    // 跨年
    assert_eq!(months_between(date(2025, 7, 1), date(2026, 7, 1)), 12);
    // 未来日期返回负数
    assert_eq!(months_between(date(2027, 1, 1), date(2026, 1, 1)), -12);
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

#[test]
fn filter_branches_selects_old_named_branches() {
    let (_dir, repo, service) = init_named_branches_repo();
    let pattern = Regex::new(r"^(dev|uat|release)_[^_]+_(?P<date>\d{8})_").unwrap();
    // 阈值 1 个月：2025 年的分支都超过 1 个月（当前 2026-07），未来分支不算
    let plan =
        select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 1, true).unwrap();

    assert_eq!(plan.matched.len(), 3);
    assert!(
        plan.matched
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );
    assert!(
        plan.matched
            .contains(&"uat_wzf_20250520_测试系统".to_string())
    );
    assert!(
        plan.matched
            .contains(&"release_wzf_20250601_发布".to_string())
    );
    // 未来日期分支归入未超期
    assert!(
        plan.skipped_within
            .contains(&"dev_abc_20990101_未来".to_string())
    );
    // 不匹配前缀的分支既不在 matched 也不在 skipped（直接被 pattern 过滤）
}

#[test]
fn filter_branches_keeps_current_branch_when_skip_current() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    // 切换到一个符合命名规则的分支，验证 skip_current 会把它排除
    service
        .checkout_branch(&mut repo, &BranchName::new("dev_wzf_20250418_测试系统"))
        .unwrap();
    let pattern = Regex::new(r"^(dev|uat|release)_[^_]+_(?P<date>\d{8})_").unwrap();
    let plan =
        select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 1, true).unwrap();
    assert!(
        !plan
            .matched
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );
    assert!(
        plan.skipped_current
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );

    // skip_current=false 时当前分支可入选
    let plan2 = select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 1, false)
        .unwrap();
    assert!(
        plan2
            .matched
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );
}

#[test]
fn filter_branches_threshold_boundary_is_strictly_greater() {
    let (_dir, repo, service) = init_named_branches_repo();
    let pattern = Regex::new(r"^(dev|uat|release)_[^_]+_(?P<date>\d{8})_").unwrap();
    // 用一个极大阈值，确保所有分支都"未超过"，验证严格大于语义
    let plan =
        select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 100_000, true)
            .unwrap();
    assert!(plan.matched.is_empty());
    // 命名规则的分支（含未来日期）都应在 skipped_within（未来日期月数差为负，也 <= 阈值）
    assert!(plan.skipped_within.len() >= 3);
}

#[test]
fn filter_branches_falls_back_to_unnamed_group() {
    let (_dir, repo, service) = init_named_branches_repo();
    // 用未命名的第一个捕获组
    let pattern = Regex::new(r"^(?:dev|uat|release)_[^_]+_(\d{8})_").unwrap();
    let plan =
        select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 1, true).unwrap();
    assert!(
        plan.matched
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );
}

#[test]
fn filter_branches_reports_unparseable_date() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    // 一个匹配 pattern 但日期格式无法解析的分支
    service
        .create_branch_from(
            &mut repo,
            &BranchName::new("dev_wzf_BADDATE_测试"),
            Some(&BranchName::new("main")),
            false,
        )
        .unwrap();
    // pattern 匹配任意非下划线段作为"日期"
    let pattern = Regex::new(r"^dev_[^_]+_(?P<date>[^_]+)_").unwrap();
    let plan =
        select_branches_for_deletion(&service, &repo, &pattern, "%Y%m%d", "date", 1, true).unwrap();
    assert!(
        plan.skipped_nodate
            .contains(&"dev_wzf_BADDATE_测试".to_string())
    );
    // 能解析的正常入选
    assert!(
        plan.matched
            .contains(&"dev_wzf_20250418_测试系统".to_string())
    );
}

#[test]
fn filter_then_delete_workflow_passes_array_between_steps() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    // 两步工作流：filterBranches → deleteBranches(dryRun)
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "filterBranches",
              output: "out.stale",
              pattern: "^(dev|uat|release)_[^_]+_(?P<date>\\d{8})_",
              dateFormat: "%Y%m%d",
              olderThanMonths: "1",
              skipCurrent: true,
            },
            {
              op: "deleteBranches",
              branches: "${out.stale}",
              dryRun: true,
              skipCurrent: true,
            },
          ],
        }
        "#,
    )
    .unwrap();
    let mut events = Vec::new();
    let executor = WorkflowExecutor::new(&service);
    let result = executor
        .run(&mut repo, &definition, WorkflowRunOptions::default(), |e| {
            events.push(e);
        })
        .unwrap();

    // dryRun 模式不真正删除，分支仍存在
    let names: Vec<String> = service
        .local_branches(&repo)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"dev_wzf_20250418_测试系统".to_string()));

    // 第二步（deleteBranches）完成事件应携带逐个"将删除"明细
    let finished = events
        .iter()
        .filter_map(|e| match e {
            WorkflowProgressEvent::StepFinished { details, .. } => Some(details),
            _ => None,
        })
        .nth(1)
        .unwrap();
    assert!(
        finished
            .iter()
            .any(|d| d.contains("将删除") && d.contains("dev_wzf_20250418"))
    );
    // 步骤计数为 2
    assert_eq!(result.steps_run, 2);
}

#[test]
fn delete_branches_actually_removes_when_not_dry_run() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "filterBranches",
              output: "out.stale",
              pattern: "^(dev|uat|release)_[^_]+_(?P<date>\\d{8})_",
              olderThanMonths: "1",
              skipCurrent: true,
            },
            {
              op: "deleteBranches",
              branches: "${out.stale}",
              dryRun: false,
              skipCurrent: true,
            },
          ],
        }
        "#,
    )
    .unwrap();
    let executor = WorkflowExecutor::new(&service);
    executor
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap();

    let names: Vec<String> = service
        .local_branches(&repo)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    // 三个超期分支被删除
    assert!(!names.contains(&"dev_wzf_20250418_测试系统".to_string()));
    assert!(!names.contains(&"uat_wzf_20250520_测试系统".to_string()));
    assert!(!names.contains(&"release_wzf_20250601_发布".to_string()));
    // 不匹配前缀的分支保留
    assert!(names.contains(&"feature_random_branch".to_string()));
    // main 仍在
    assert!(names.contains(&"main".to_string()));
}

#[test]
fn delete_branches_string_input_splits_by_newline() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    // branches 用换行分隔的字符串而非数组变量
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "deleteBranches",
              branches: "dev_wzf_20250418_测试系统\nuat_wzf_20250520_测试系统",
              dryRun: false,
              skipCurrent: true,
            },
          ],
        }
        "#,
    )
    .unwrap();
    let executor = WorkflowExecutor::new(&service);
    executor
        .run(
            &mut repo,
            &definition,
            WorkflowRunOptions::default(),
            |_| {},
        )
        .unwrap();
    let names: Vec<String> = service
        .local_branches(&repo)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(!names.contains(&"dev_wzf_20250418_测试系统".to_string()));
    assert!(!names.contains(&"uat_wzf_20250520_测试系统".to_string()));
    // 其余分支不受影响
    assert!(names.contains(&"release_wzf_20250601_发布".to_string()));
}

#[test]
fn delete_local_branches_removes_multiple_and_errors_on_missing() {
    let (_dir, mut repo, service) = init_named_branches_repo();
    service
        .delete_local_branches(
            &mut repo,
            &[
                BranchName::new("dev_wzf_20250418_测试系统"),
                BranchName::new("uat_wzf_20250520_测试系统"),
            ],
        )
        .unwrap();
    let names: Vec<String> = service
        .local_branches(&repo)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(!names.contains(&"dev_wzf_20250418_测试系统".to_string()));
    assert!(!names.contains(&"uat_wzf_20250520_测试系统".to_string()));

    // 不存在的分支会报错
    let err = service
        .delete_local_branches(&mut repo, &[BranchName::new("not_exist")])
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn filter_branches_preview_shows_matched_details() {
    let (_dir, repo, service) = init_named_branches_repo();
    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          steps: [
            {
              op: "filterBranches",
              output: "out.stale",
              pattern: "^(dev|uat|release)_[^_]+_(?P<date>\\d{8})_",
              olderThanMonths: "1",
              skipCurrent: true,
            },
          ],
        }
        "#,
    )
    .unwrap();
    let executor = WorkflowExecutor::new(&service);
    let preview = executor
        .preview(&repo, &definition, &WorkflowRunOptions::default())
        .unwrap();
    // 预览步骤应携带命中明细（只读计算，不改动仓库）
    let details = &preview.steps[0].details;
    assert!(details.iter().any(|d| d.contains("dev_wzf_20250418")));
    assert!(details.iter().any(|d| d.contains("命中")));
}

#[test]
fn reproduce_user_scenario_uat_branch_with_alt_and_inputs() {
    // 精确复现用户反馈场景：
    // 分支 uat_wzf_20250924_智慧消保系统，前缀 uat,release，阈值 2 个月（今天 2026-07-31，距今约 10 个月）。
    // 使用示例 4 的完整结构（inputs + vars + alt + 两步骤）。
    let (_dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(_dir.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch_from(
            &mut repo,
            &BranchName::new("uat_wzf_20250924_智慧消保系统"),
            Some(&BranchName::new("main")),
            false,
        )
        .unwrap();

    let definition = parse_workflow_json5(
        r#"
        {
          version: 1,
          name: "清理过期命名分支",
          defaults: { requireCleanWorktree: false },
          inputs: {
            prefixes: { label: "前缀", default: "uat,release", required: true },
            months: { label: "月数", default: "2", required: true },
          },
          vars: {
            prefixAlt: "${prefixes|alt}",
          },
          steps: [
            {
              op: "filterBranches",
              output: "out.staleBranches",
              pattern: "^${prefixAlt}_[^_]+_(?P<date>\\d{8})_",
              dateFormat: "%Y%m%d",
              dateGroup: "date",
              olderThanMonths: "${months}",
              skipCurrent: true,
            },
            {
              op: "deleteBranches",
              branches: "${out.staleBranches}",
              dryRun: true,
              skipCurrent: true,
            },
          ],
        }
        "#,
    )
    .unwrap();

    let executor = WorkflowExecutor::new(&service);
    // 模拟 UI 行为：用户填入前缀 uat,release 和月数 2
    let options = WorkflowRunOptions {
        default_remote: "origin".to_string(),
        input_vars: {
            let mut m = BTreeMap::new();
            m.insert("prefixes".to_string(), "uat,release".to_string());
            m.insert("months".to_string(), "2".to_string());
            m
        },
    };
    let preview = executor.preview(&repo, &definition, &options).unwrap();

    // 预览第一步（filterBranches）的明细应命中该分支
    let filter_details = &preview.steps[0].details;
    assert!(
        filter_details
            .iter()
            .any(|d| d.contains("uat_wzf_20250924_智慧消保系统") && d.contains("命中")),
        "应命中 uat 分支，实际 details: {filter_details:?}"
    );
}
