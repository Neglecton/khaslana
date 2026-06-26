use crate::git::test_support::git_test_support as git_support;
use crate::types::{BranchName, RebaseOutcome};
use git2::Repository;

/// 创建测试仓库并写入初始提交。
fn setup_repo(dir: &std::path::Path) -> Repository {
    let repo = Repository::init(dir).unwrap();
    {
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        std::fs::write(dir.join("file.txt"), "line 1\nline 2\nline 3\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }
    repo
}

fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) {
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(repo.workdir().unwrap().join(path), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new(path)).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
        .unwrap();
}

/// 切换到指定分支。
fn checkout_branch(repo: &Repository, name: &str) {
    {
        let obj = repo.revparse_single(name).unwrap();
        repo.checkout_tree(&obj, None).unwrap();
    }
    repo.set_head(&format!("refs/heads/{name}")).unwrap();
}

/// 在当前 HEAD 上创建分支。
fn create_branch_from_head(repo: &Repository, name: &str) {
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(name, &head_commit, false).unwrap();
}

#[test]
fn rebase_fast_forward_without_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let mut repo = setup_repo(tmp.path());

    // 在 master 分叉前创建 feature 分支
    create_branch_from_head(&repo, "feature");

    // 在 master 上新增提交
    commit_file(
        &repo,
        "file.txt",
        "line 1\nline 2\nline 3\nline 4\n",
        "add line 4 on master",
    );

    // 切换到 feature 并新增不同文件
    checkout_branch(&repo, "feature");
    commit_file(
        &repo,
        "other.txt",
        "feature content\n",
        "add other.txt on feature",
    );

    // 变基 feature 到 master：应该无冲突
    let service = git_support::service();
    let outcome = service
        .rebase_branch(&mut repo, &BranchName::new("master"))
        .unwrap();

    match outcome {
        RebaseOutcome::Completed(snapshot) => {
            assert!(!snapshot.rebase_in_progress);
        }
        RebaseOutcome::Conflicts { .. } => panic!("expected clean rebase"),
    }
}

#[test]
fn rebase_detects_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let mut repo = setup_repo(tmp.path());

    // 在 master 分叉前创建 feature 分支
    create_branch_from_head(&repo, "feature");

    // 在 master 上修改同一行
    commit_file(
        &repo,
        "file.txt",
        "MODIFIED BY MAIN\nline 2\nline 3\n",
        "modify line 1 on master",
    );

    // 切换到 feature 并修改同一行为不同内容
    checkout_branch(&repo, "feature");
    commit_file(
        &repo,
        "file.txt",
        "MODIFIED BY FEATURE\nline 2\nline 3\n",
        "modify line 1 on feature",
    );

    // 变基 feature 到 master：应该产生冲突
    let service = git_support::service();
    let outcome = service
        .rebase_branch(&mut repo, &BranchName::new("master"))
        .unwrap();

    match outcome {
        RebaseOutcome::Conflicts {
            snapshot,
            current,
            total,
        } => {
            assert!(snapshot.rebase_in_progress);
            assert_eq!(current, 1);
            assert_eq!(total, 1);
            assert!(!snapshot.conflicts.is_empty());
        }
        RebaseOutcome::Completed(_) => panic!("expected conflict"),
    }

    // 中止变基
    let snapshot = service.rebase_abort(&mut repo).unwrap();
    assert!(!snapshot.rebase_in_progress);
}

#[test]
fn rebase_up_to_date_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let mut repo = setup_repo(tmp.path());

    let service = git_support::service();
    let outcome = service
        .rebase_branch(&mut repo, &BranchName::new("master"))
        .unwrap();

    match outcome {
        RebaseOutcome::Completed(snapshot) => {
            assert!(!snapshot.rebase_in_progress);
        }
        RebaseOutcome::Conflicts { .. } => panic!("expected up-to-date no-op"),
    }
}
