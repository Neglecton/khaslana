use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use git2::Repository;
use tempfile::TempDir;

use super::*;
use crate::git::test_support::git_test_support as git_support;
use crate::{BranchName, CommitMessage, GitError};

static IDEA_ENV_LOCK: Mutex<()> = Mutex::new(());

struct IdeaEnvGuard {
    _guard: MutexGuard<'static, ()>,
    previous: Option<String>,
}

impl IdeaEnvGuard {
    fn set(path: &Path) -> Self {
        let guard = IDEA_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var("KHASLANA_IDEA_PATH").ok();
        // 测试串行持有锁，避免进程级环境变量影响其它外部合并测试。
        unsafe {
            std::env::set_var("KHASLANA_IDEA_PATH", path);
        }
        Self {
            _guard: guard,
            previous,
        }
    }
}

impl Drop for IdeaEnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var("KHASLANA_IDEA_PATH", previous);
            } else {
                std::env::remove_var("KHASLANA_IDEA_PATH");
            }
        }
    }
}

fn fake_idea_tool(dir: &Path, behavior: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let path = dir.join(format!("{behavior}.cmd"));
        let body = match behavior {
            "copy_theirs" => {
                "@echo off\r\nif not \"%~1\"==\"merge\" exit /b 9\r\ntype \"%~3\" > \"%~5\"\r\n"
            }
            "no_result" => "@echo off\r\nexit /b 0\r\n",
            "fail" => "@echo off\r\nexit /b 7\r\n",
            _ => unreachable!(),
        };
        fs::write(&path, body).unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(behavior);
        let body = match behavior {
            "copy_theirs" => "#!/bin/sh\n[ \"$1\" = \"merge\" ] || exit 9\ncat \"$3\" > \"$5\"\n",
            "no_result" => "#!/bin/sh\nexit 0\n",
            "fail" => "#!/bin/sh\nexit 7\n",
            _ => unreachable!(),
        };
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

fn create_named_text_conflict(path: &str) -> (TempDir, Repository, GitService) {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), path, "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), path, "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), path, "main\n");
    git_support::commit_all(&repo, "main");

    let err = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    assert!(matches!(err, GitError::Conflicts(paths) if paths == vec![path]));
    (dir, repo, service)
}

fn create_text_conflict() -> (TempDir, Repository, GitService) {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let err = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    assert!(matches!(err, GitError::Conflicts(paths) if paths == vec!["same.txt"]));
    (dir, repo, service)
}

fn create_multi_block_text_conflict() -> (TempDir, Repository, GitService) {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "start\none\nmiddle\ntwo\nend\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(
        dir.path(),
        "same.txt",
        "start\nfeature-one\nmiddle\nfeature-two\nend\n",
    );
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(
        dir.path(),
        "same.txt",
        "start\nmain-one\nmiddle\nmain-two\nend\n",
    );
    git_support::commit_all(&repo, "main");

    let err = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    assert!(matches!(err, GitError::Conflicts(paths) if paths == vec!["same.txt"]));
    (dir, repo, service)
}

fn create_modify_delete_conflict() -> (TempDir, Repository, GitService) {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    fs::remove_file(dir.path().join("same.txt")).unwrap();
    {
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("same.txt")).unwrap();
        index.write().unwrap();
    }
    git_support::commit_all(&repo, "feature deletes");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main modifies");

    let err = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    assert!(matches!(err, GitError::Conflicts(paths) if paths == vec!["same.txt"]));
    (dir, repo, service)
}

#[test]
fn resolve_conflict_with_ours_keeps_current_branch_version() {
    let (dir, mut repo, service) = create_text_conflict();

    let snapshot = service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            ConflictResolutionSide::Ours,
        )
        .unwrap();

    assert!(snapshot.conflicts.is_empty());
    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
    service
        .commit(&mut repo, &CommitMessage::new("resolve with ours"))
        .unwrap();
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn resolve_conflict_with_theirs_keeps_incoming_branch_version() {
    let (dir, mut repo, service) = create_text_conflict();

    let snapshot = service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            ConflictResolutionSide::Theirs,
        )
        .unwrap();

    assert!(snapshot.conflicts.is_empty());
    git_support::assert_file_text(dir.path(), "same.txt", "feature\n");
    service
        .commit(&mut repo, &CommitMessage::new("resolve with theirs"))
        .unwrap();
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn mark_conflict_resolved_accepts_manual_resolution() {
    let (dir, mut repo, service) = create_text_conflict();
    git_support::write_file(dir.path(), "same.txt", "manual\n");

    let snapshot = service
        .mark_conflict_resolved(&mut repo, Path::new("same.txt"))
        .unwrap();

    assert!(snapshot.conflicts.is_empty());
    git_support::assert_file_text(dir.path(), "same.txt", "manual\n");
    service
        .commit(&mut repo, &CommitMessage::new("manual resolution"))
        .unwrap();
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn resolve_modify_delete_conflict_can_keep_or_delete_file() {
    let (dir, mut repo, service) = create_modify_delete_conflict();
    service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            ConflictResolutionSide::Ours,
        )
        .unwrap();
    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
    assert!(service.conflicts(&repo).unwrap().is_empty());

    let (dir, mut repo, service) = create_modify_delete_conflict();
    service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            ConflictResolutionSide::Theirs,
        )
        .unwrap();
    assert!(!dir.path().join("same.txt").exists());
    assert!(service.conflicts(&repo).unwrap().is_empty());
}

#[test]
fn resolve_conflict_rejects_non_conflicted_path() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    let error = service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            ConflictResolutionSide::Ours,
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("不存在冲突"));
}

#[test]
fn conflict_file_view_parses_multiple_text_blocks_and_starts_with_ours_result() {
    let (_dir, repo, service) = create_multi_block_text_conflict();

    let view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();

    assert_eq!(view.kind, crate::ConflictFileKind::Text);
    assert_eq!(view.blocks.len(), 2);
    assert_eq!(view.ours_text, "start\nmain-one\nmiddle\nmain-two\nend\n");
    assert_eq!(
        view.theirs_text,
        "start\nfeature-one\nmiddle\nfeature-two\nend\n"
    );
    assert_eq!(view.blocks[0].ours, "main-one\n");
    assert_eq!(view.blocks[0].theirs, "feature-one\n");
    assert_eq!(view.blocks[0].base.as_deref(), Some("one\n"));
    assert_eq!(view.blocks[1].ours, "main-two\n");
    assert_eq!(view.blocks[1].theirs, "feature-two\n");
    assert_eq!(view.draft, "start\nmain-one\nmiddle\nmain-two\nend\n");
    assert_eq!(view.draft_status, crate::ConflictDraftStatus::Clean);
    assert_eq!(view.unresolved_block_count(), 2);
    assert!(view.requires_resolution_confirmation());
}

#[test]
fn conflict_file_view_block_actions_update_draft_and_shift_ranges() {
    let (_dir, repo, service) = create_multi_block_text_conflict();

    let mut view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();
    view.apply_block_resolution(0, crate::ConflictBlockResolution::Theirs);
    view.apply_block_resolution(1, crate::ConflictBlockResolution::BothOursFirst);

    assert_eq!(
        view.draft,
        "start\nfeature-one\nmiddle\nmain-two\nfeature-two\nend\n"
    );
    assert_eq!(
        view.blocks[0].status,
        crate::ConflictBlockStatus::Resolved(crate::ConflictBlockResolution::Theirs)
    );
    assert_eq!(
        view.blocks[1].status,
        crate::ConflictBlockStatus::Resolved(crate::ConflictBlockResolution::BothOursFirst)
    );
    assert_eq!(view.draft_status, crate::ConflictDraftStatus::Dirty);
    assert!(!view.requires_resolution_confirmation());
}

#[test]
fn ignoring_a_block_preserves_draft_and_marks_it_handled() {
    let (_dir, repo, service) = create_multi_block_text_conflict();

    let mut view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();
    let original = view.draft.clone();

    view.ignore_block(0);

    assert_eq!(view.draft, original);
    assert_eq!(view.unresolved_block_count(), 1);
    assert_eq!(view.ignored_block_count(), 1);
    assert_eq!(view.handled_block_count(), 1);
    assert_eq!(view.blocks[0].status, crate::ConflictBlockStatus::Ignored);
    assert!(view.requires_resolution_confirmation());
}

#[test]
fn apply_conflict_draft_writes_file_but_keeps_conflict_unresolved() {
    let (dir, mut repo, service) = create_text_conflict();

    let mut view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();
    view.apply_block_resolution(0, crate::ConflictBlockResolution::Theirs);
    let snapshot = service
        .apply_conflict_draft(&mut repo, Path::new("same.txt"), &view.draft)
        .unwrap();

    git_support::assert_file_text(dir.path(), "same.txt", "feature\n");
    assert_eq!(snapshot.conflicts, vec!["same.txt".to_string()]);
    assert_eq!(
        service.conflicts(&repo).unwrap(),
        vec!["same.txt".to_string()]
    );
}

#[test]
fn apply_conflict_draft_and_resolve_clears_conflict_and_allows_commit() {
    let (dir, mut repo, service) = create_text_conflict();

    let view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();
    let snapshot = service
        .apply_conflict_draft_and_resolve(&mut repo, Path::new("same.txt"), &view.draft)
        .unwrap();

    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
    assert!(snapshot.conflicts.is_empty());
    let committed = service
        .commit(&mut repo, &CommitMessage::new("resolve from workbench"))
        .unwrap();
    assert!(!committed.merge_in_progress);
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.parent_count(), 2);
}

#[test]
fn mark_conflict_file_with_missing_side_as_unsupported() {
    let (_dir, repo, service) = create_modify_delete_conflict();

    let view = service
        .conflict_file_view(&repo, Path::new("same.txt"))
        .unwrap();

    assert_eq!(view.kind, crate::ConflictFileKind::Unsupported);
    assert!(view.blocks.is_empty());
    assert!(view.fallback_reason.is_some());
}

#[test]
fn intellij_external_merge_writes_result_and_clears_conflict() {
    let (dir, mut repo, service) = create_text_conflict();
    let tool = fake_idea_tool(dir.path(), "copy_theirs");
    let _env = IdeaEnvGuard::set(&tool);

    let snapshot = service
        .resolve_conflict_with_intellij_idea(&mut repo, Path::new("same.txt"))
        .unwrap();

    assert!(snapshot.conflicts.is_empty());
    git_support::assert_file_text(dir.path(), "same.txt", "feature\n");
    service
        .commit(&mut repo, &CommitMessage::new("resolve with intellij"))
        .unwrap();
}

#[test]
fn intellij_external_merge_errors_when_result_file_is_missing() {
    let (dir, mut repo, service) = create_text_conflict();
    let tool = fake_idea_tool(dir.path(), "no_result");
    let _env = IdeaEnvGuard::set(&tool);

    let error = service
        .resolve_conflict_with_intellij_idea(&mut repo, Path::new("same.txt"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("IntelliJ IDEA 合并未生成结果文件"));
    assert_eq!(service.conflicts(&repo).unwrap(), vec!["same.txt"]);
}

#[test]
fn intellij_external_merge_errors_when_tool_exits_with_failure() {
    let (dir, mut repo, service) = create_text_conflict();
    let tool = fake_idea_tool(dir.path(), "fail");
    let _env = IdeaEnvGuard::set(&tool);

    let error = service
        .resolve_conflict_with_intellij_idea(&mut repo, Path::new("same.txt"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("IntelliJ IDEA 合并工具退出失败"));
    assert_eq!(service.conflicts(&repo).unwrap(), vec!["same.txt"]);
}

#[test]
fn intellij_external_merge_rejects_modify_delete_conflict() {
    let (dir, mut repo, service) = create_modify_delete_conflict();
    let tool = fake_idea_tool(dir.path(), "copy_theirs");
    let _env = IdeaEnvGuard::set(&tool);

    let error = service
        .resolve_conflict_with_intellij_idea(&mut repo, Path::new("same.txt"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("暂不能用 IntelliJ IDEA 三方合并"));
    assert_eq!(service.conflicts(&repo).unwrap(), vec!["same.txt"]);
}

#[test]
fn intellij_external_merge_handles_chinese_paths() {
    let (dir, mut repo, service) = create_named_text_conflict("目录/同名.txt");
    let tool = fake_idea_tool(dir.path(), "copy_theirs");
    let _env = IdeaEnvGuard::set(&tool);

    let snapshot = service
        .resolve_conflict_with_intellij_idea(&mut repo, Path::new("目录/同名.txt"))
        .unwrap();

    assert!(snapshot.conflicts.is_empty());
    git_support::assert_file_text(dir.path(), "目录/同名.txt", "feature\n");
}
