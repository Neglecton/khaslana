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
        self.apply_stash_with_options(repo, index, false)
    }

    /// reinstate_index 供切换分支的自动携带使用：恢复时连同暂存区划分一起
    /// 还原（git stash apply --index 语义），避免已暂存内容被抹平成未暂存。
    ///
    /// 恢复产生冲突时返回带 conflicts 的快照而非错误：libgit2 对「未跟踪
    /// 文件与目标分支同名文件」只把冲突标记写入工作区、不写索引阶段，仅凭
    /// 索引冲突会漏掉这类文件，UI 就进不了冲突工作台。这里额外扫描贮藏
    /// 涉及文件的工作区标记，一并纳入 conflicts。
    pub(super) fn apply_stash_with_options(
        &self,
        repo: &mut Repository,
        index: usize,
        reinstate_index: bool,
    ) -> Result<RepositorySnapshot> {
        self.ensure_stash_index(repo, index)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在应用贮藏 stash@{{{index}}}"
        )));
        let mut options = StashApplyOptions::new();
        let apply_result = super::worktree_compat::apply_stash_preserving_locked_directories(
            repo,
            index,
            &mut options,
            reinstate_index,
        );
        let snapshot = self.snapshot_after_operation(repo)?;
        // apply 报错但已留下冲突时交给用户处理；没有冲突线索时保留原始错误。
        if let Err(err) = apply_result
            && snapshot.conflicts.is_empty()
        {
            return Err(err.into());
        }
        self.progress.emit(OperationEvent::Finished(format!(
            "已应用贮藏 stash@{{{index}}}"
        )));
        Ok(snapshot)
    }

    pub fn pop_stash(&self, repo: &mut Repository, index: usize) -> Result<RepositorySnapshot> {
        self.ensure_stash_index(repo, index)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在弹出贮藏 stash@{{{index}}}"
        )));
        // 不用 git_stash_pop：libgit2 的 pop 在恢复冲突时也会删除贮藏条目
        // （与 git 命令行“失败保留条目”不同），用户的兜底拷贝会丢。改为
        // 先 apply，仅在没有任何冲突时才删除条目。
        let snapshot = self.apply_stash_with_options(repo, index, false)?;
        if !snapshot.conflicts.is_empty() {
            // 有冲突：保留条目，交还用户在冲突工作台处理。
            return Ok(snapshot);
        }
        self.progress.emit(OperationEvent::Finished(format!(
            "已弹出贮藏 stash@{{{index}}}"
        )));
        self.drop_stash(repo, index)
    }

    /// 未跟踪文件恢复时与目标分支同名、仅工作区产生冲突标记的路径。
    pub(super) fn worktree_conflict_marked_paths(
        &self,
        repo: &Repository,
        stash_oid: &str,
    ) -> Result<Vec<String>> {
        let Some(workdir) = repo.workdir() else {
            return Ok(Vec::new());
        };
        let stash_commit = self.find_commit_by_oid(repo, stash_oid)?;
        let Some(untracked_tree) = stash_commit
            .parent(2)
            .ok()
            .and_then(|parent| parent.tree().ok())
        else {
            return Ok(Vec::new());
        };
        let head_tree = repo.head()?.peel_to_tree()?;
        let mut marked = Vec::new();
        for file in self.stash_files(repo, stash_oid)? {
            if file.status != ChangeState::Untracked {
                continue;
            }
            let path = Path::new(&file.path);
            let Ok(stash_entry) = untracked_tree.get_path(path) else {
                continue;
            };
            let Ok(stash_blob) = repo.find_blob(stash_entry.id()) else {
                continue;
            };
            let full_path = workdir.join(&file.path);
            // 只把恢复新生成的完整冲突块视为冲突。原贮藏或目标文件里
            // 本来就有的标记是普通内容，不能据此阻止贮藏清理。
            let Ok(bytes) = std::fs::read(&full_path) else {
                continue;
            };
            if matches_stored_text(&bytes, stash_blob.content()) {
                continue;
            }
            if let Ok(head_entry) = head_tree.get_path(path)
                && let Ok(head_blob) = repo.find_blob(head_entry.id())
                && matches_stored_text(&bytes, head_blob.content())
            {
                continue;
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if has_complete_conflict_markers(content) {
                marked.push(file.path);
            }
        }
        Ok(marked)
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
            super::guard_full_file_size(repo, &diff, full_context)?;
            return self.file_diff_from_diff(
                repo,
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
        super::guard_full_file_size(repo, &diff, full_context)?;
        self.file_diff_from_diff(
            repo,
            diff,
            super::path_to_git(path),
            DiffScope::Staged,
            encoding,
        )
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

pub(super) fn has_complete_conflict_markers(content: &str) -> bool {
    let mut started = false;
    let mut divider = false;
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("<<<<<<<") {
            started = true;
            divider = false;
        } else if started && line == "=======" {
            divider = true;
        } else if started && divider && line.starts_with(">>>>>>>") {
            return true;
        }
    }
    false
}

fn matches_stored_text(worktree: &[u8], stored: &[u8]) -> bool {
    if worktree == stored {
        return true;
    }
    let (Ok(worktree), Ok(stored)) = (
        std::str::from_utf8(worktree),
        std::str::from_utf8(stored),
    ) else {
        return false;
    };
    worktree.replace("\r\n", "\n") == stored.replace("\r\n", "\n")
}
