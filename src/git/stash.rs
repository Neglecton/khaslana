use std::path::Path;

use git2::{DiffOptions, Repository, StashApplyOptions, StashFlags};

use crate::{
    GitService,
    types::{
        ChangeState, DiffEncodingChoice, DiffScope, FileDiff, GitError, OperationEvent,
        RepositorySnapshot, Result, StashFileChange,
    },
};

impl GitService {
    pub fn save_stash(
        &self,
        repo: &mut Repository,
        message: &str,
        include_untracked: bool,
        keep_index: bool,
    ) -> Result<RepositorySnapshot> {
        let changes = self.status_full(repo)?;
        if !self.conflicts(repo)?.is_empty() {
            return Err(GitError::Message("存在冲突，无法创建贮藏".into()));
        }
        let has_stashable_change = changes.iter().any(|change| {
            change.staged.is_some()
                || change.unstaged.as_ref().is_some_and(|state| {
                    include_untracked || !matches!(state, ChangeState::Untracked)
                })
        });
        if !has_stashable_change {
            if changes
                .iter()
                .any(|change| matches!(change.unstaged, Some(ChangeState::Untracked)))
            {
                return Err(GitError::Message(
                    "当前只有未跟踪文件；如需贮藏请勾选“包含未跟踪文件”".into(),
                ));
            }
            return Err(GitError::Message("当前没有可贮藏的更改".into()));
        }

        self.progress
            .emit(OperationEvent::Started("正在贮藏当前修改".into()));
        let signature = super::signature(repo)?;
        let mut flags = StashFlags::empty();
        if include_untracked {
            flags.insert(StashFlags::INCLUDE_UNTRACKED);
        }
        if keep_index {
            flags.insert(StashFlags::KEEP_INDEX);
        }
        let message = message.trim();
        let message = if message.is_empty() {
            "Khaslana stash"
        } else {
            message
        };
        super::worktree_compat::save_stash_preserving_locked_directories(
            repo, &signature, message, flags,
        )?;
        self.progress
            .emit(OperationEvent::Finished("已贮藏当前修改".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn drop_stash(&self, repo: &mut Repository, index: usize) -> Result<RepositorySnapshot> {
        self.ensure_stash_index(repo, index)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在删除贮藏 stash@{{{index}}}"
        )));
        repo.stash_drop(index)?;
        self.progress.emit(OperationEvent::Finished(format!(
            "已删除贮藏 stash@{{{index}}}"
        )));
        self.snapshot_after_operation(repo)
    }

    pub fn apply_stash(&self, repo: &mut Repository, index: usize) -> Result<RepositorySnapshot> {
        self.ensure_stash_index(repo, index)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在应用贮藏 stash@{{{index}}}"
        )));
        let mut options = StashApplyOptions::new();
        super::worktree_compat::apply_stash_preserving_locked_directories(
            repo,
            index,
            &mut options,
        )?;
        self.progress.emit(OperationEvent::Finished(format!(
            "已应用贮藏 stash@{{{index}}}"
        )));
        self.snapshot_after_operation(repo)
    }

    pub fn pop_stash(&self, repo: &mut Repository, index: usize) -> Result<RepositorySnapshot> {
        self.ensure_stash_index(repo, index)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在弹出贮藏 stash@{{{index}}}"
        )));
        let mut options = StashApplyOptions::new();
        super::worktree_compat::pop_stash_preserving_locked_directories(repo, index, &mut options)?;
        self.progress.emit(OperationEvent::Finished(format!(
            "已弹出贮藏 stash@{{{index}}}"
        )));
        self.snapshot_after_operation(repo)
    }

    pub fn stash_files(&self, repo: &Repository, stash_oid: &str) -> Result<Vec<StashFileChange>> {
        let stash_commit = self.find_commit_by_oid(repo, stash_oid)?;
        let mut files = Vec::new();
        self.collect_stash_worktree_files(repo, &stash_commit, &mut files)?;
        self.collect_stash_untracked_files(repo, &stash_commit, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path && a.status == b.status);
        Ok(files)
    }

    pub fn stash_file_diff(
        &self,
        repo: &Repository,
        stash_oid: &str,
        path: &Path,
        full_context: bool,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let stash_commit = self.find_commit_by_oid(repo, stash_oid)?;
        if let Some(diff) =
            self.stash_untracked_diff_for_path(repo, &stash_commit, path, full_context)?
        {
            super::guard_full_file_size(&diff, full_context)?;
            return self.file_diff_from_diff(
                diff,
                super::path_to_git(path),
                DiffScope::Staged,
                encoding,
            );
        }

        let base_tree = stash_commit
            .parent(0)
            .ok()
            .and_then(|parent| parent.tree().ok());
        let stash_tree = stash_commit.tree()?;
        let mut options = DiffOptions::new();
        options
            .context_lines(super::diff_context_lines(full_context))
            .pathspec(path);
        let diff =
            repo.diff_tree_to_tree(base_tree.as_ref(), Some(&stash_tree), Some(&mut options))?;
        super::guard_full_file_size(&diff, full_context)?;
        self.file_diff_from_diff(diff, super::path_to_git(path), DiffScope::Staged, encoding)
    }

    fn ensure_stash_index(&self, repo: &mut Repository, index: usize) -> Result<()> {
        if self.stashes(repo)?.iter().any(|stash| stash.index == index) {
            Ok(())
        } else {
            Err(GitError::Message(format!("贮藏不存在：stash@{{{index}}}")))
        }
    }

    fn collect_stash_worktree_files(
        &self,
        repo: &Repository,
        stash_commit: &git2::Commit<'_>,
        files: &mut Vec<StashFileChange>,
    ) -> Result<()> {
        let base_tree = stash_commit
            .parent(0)
            .ok()
            .and_then(|parent| parent.tree().ok());
        let stash_tree = stash_commit.tree()?;
        let diff = repo.diff_tree_to_tree(base_tree.as_ref(), Some(&stash_tree), None)?;
        for delta in diff.deltas() {
            let Some(path) = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(super::path_to_git)
            else {
                continue;
            };
            files.push(StashFileChange {
                path,
                old_path: delta.old_file().path().map(super::path_to_git),
                status: super::change_state_from_delta(delta.status()),
            });
        }
        Ok(())
    }

    fn collect_stash_untracked_files(
        &self,
        repo: &Repository,
        stash_commit: &git2::Commit<'_>,
        files: &mut Vec<StashFileChange>,
    ) -> Result<()> {
        let Some(untracked_tree) = stash_commit
            .parent(2)
            .ok()
            .and_then(|parent| parent.tree().ok())
        else {
            return Ok(());
        };
        let diff = repo.diff_tree_to_tree(None, Some(&untracked_tree), None)?;
        for delta in diff.deltas() {
            let Some(path) = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(super::path_to_git)
            else {
                continue;
            };
            files.push(StashFileChange {
                path,
                old_path: None,
                status: ChangeState::Untracked,
            });
        }
        Ok(())
    }

    fn stash_untracked_diff_for_path<'repo>(
        &self,
        repo: &'repo Repository,
        stash_commit: &git2::Commit<'repo>,
        path: &Path,
        full_context: bool,
    ) -> Result<Option<git2::Diff<'repo>>> {
        let Some(untracked_tree) = stash_commit
            .parent(2)
            .ok()
            .and_then(|parent| parent.tree().ok())
        else {
            return Ok(None);
        };
        if untracked_tree.get_path(path).is_err() {
            return Ok(None);
        }
        let mut options = DiffOptions::new();
        options
            .context_lines(super::diff_context_lines(full_context))
            .pathspec(path);
        Ok(Some(repo.diff_tree_to_tree(
            None,
            Some(&untracked_tree),
            Some(&mut options),
        )?))
    }
}
