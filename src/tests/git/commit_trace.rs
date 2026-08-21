use crate::git::COMMIT_TRACE_OID_LIMIT;
use crate::git::test_support::git_test_support as git_support;
use std::collections::HashSet;

/// 构建测试仓库：main 两个提交（base、main-tip），feature 分支自 base
/// 分叉出两个独有提交（feat-1、feat-2），最终 HEAD 停在 main。
///
/// ```text
/// base ── main-tip          (main / HEAD)
///   └── feat-1 ── feat-2    (feature)
/// ```
fn build_trace_repo() -> (tempfile::TempDir, [git2::Oid; 4]) {
    let (dir, repo, _svc) = git_support::init_repo();

    git_support::write_file(dir.path(), "a.txt", "base\n");
    let base = git_support::commit_all(&repo, "base");

    git_support::write_file(dir.path(), "a.txt", "main tip\n");
    let main_tip = git_support::commit_all(&repo, "main-tip");

    // feature 自 base 分叉：把 HEAD 指到新分支后再提交，避免落在 main 上。
    repo.branch("feature", &repo.find_commit(base).unwrap(), false)
        .unwrap();
    repo.set_head("refs/heads/feature").unwrap();
    git_support::write_file(dir.path(), "b.txt", "feat 1\n");
    let feat1 = git_support::commit_all(&repo, "feat-1");
    git_support::write_file(dir.path(), "b.txt", "feat 2\n");
    let feat2 = git_support::commit_all(&repo, "feat-2");

    // HEAD 回到 main：ahead_only 语义以 HEAD 可达集为基准。
    repo.set_head("refs/heads/main").unwrap();

    (dir, [base, feat1, feat2, main_tip])
}

#[test]
fn full_trace_includes_ancestors() {
    let (dir, [base, _feat1, feat2, main_tip]) = build_trace_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (oids, truncated) = svc.branch_commit_oids(&repo, "feature", false).unwrap();
    let set: HashSet<String> = oids.iter().map(|oid| oid.clone()).collect();

    // 全谱系 = feature tip 及其全部祖先（base、feat-1、feat-2），不含 main 独有提交。
    assert!(!truncated);
    assert_eq!(set.len(), 3);
    assert!(set.contains(&feat2.to_string()));
    assert!(set.contains(&base.to_string()));
    assert!(!set.contains(&main_tip.to_string()));
}

#[test]
fn ahead_only_trace_excludes_head_reachable_commits() {
    let (dir, [base, feat1, feat2, _main_tip]) = build_trace_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (oids, _) = svc.branch_commit_oids(&repo, "feature", true).unwrap();
    let set: HashSet<String> = oids.iter().map(|oid| oid.clone()).collect();

    // 仅领先 HEAD：base 可从 HEAD 到达，不应出现；只保留 feature 独有提交。
    assert_eq!(set.len(), 2);
    assert!(set.contains(&feat1.to_string()));
    assert!(set.contains(&feat2.to_string()));
    assert!(!set.contains(&base.to_string()));
}

#[test]
fn trace_missing_branch_reports_chinese_error() {
    let (dir, _oids) = build_trace_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let err = svc.branch_commit_oids(&repo, "nope", false).unwrap_err();
    assert!(err.to_string().contains("本地分支不存在"));
}

#[test]
fn trace_small_repo_is_not_truncated() {
    let (dir, _oids) = build_trace_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    // 小仓库不截断；上限常量为 2000（截断标记有明确语义，UI 据此提示）。
    let (oids, truncated) = svc.branch_commit_oids(&repo, "main", false).unwrap();
    assert!(!truncated);
    assert!(!oids.is_empty());
    assert_eq!(COMMIT_TRACE_OID_LIMIT, 2000);
}
