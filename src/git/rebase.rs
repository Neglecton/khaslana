//! 变基（Rebase）Git 服务。
//!
//! 使用 libgit2 的 rebase API 实现。变基把当前分支的提交逐个"重放"到目标分支之上。
//! 与 merge 的关键区别：变基是多轮的——每个提交都可能产生冲突，需要解决后继续。
//!
//! 流程：
//! 1. `rebase_branch`：初始化变基，逐个重放提交，遇冲突则暂停并返回进度。
//! 2. 用户在冲突工作台解决冲突后，调用 `rebase_continue` 提交当前操作并继续。
//! 3. `rebase_skip`：跳过当前冲突提交（reset + 空 commit 推进操作指针）。
//! 4. `rebase_abort`：中止变基，回到变基前状态。

use git2::{RebaseOperationType, RebaseOptions, Repository};

use crate::types::{BranchName, GitError, OperationEvent, RebaseOutcome, Result};
use crate::types::{RemoteName, RepositorySnapshot};

impl super::GitService {
    /// 把当前分支变基到 `onto` 分支之上。
    pub fn rebase_branch(&self, repo: &mut Repository, onto: &BranchName) -> Result<RebaseOutcome> {
        self.progress
            .emit(OperationEvent::Started(format!("正在变基 {}", onto.0)));

        let head_ref = repo.head()?;
        let branch_annotated = repo.reference_to_annotated_commit(&head_ref)?;
        let onto_ref = self.find_branch_reference(repo, &onto.0)?;
        let onto_annotated = repo.reference_to_annotated_commit(&onto_ref)?;

        let mut opts = RebaseOptions::new();
        let mut rebase = super::worktree_compat::rebase_preserving_locked_directories(
            repo,
            Some(&branch_annotated),
            Some(&onto_annotated),
            None,
            &mut opts,
        )?;
        let total = rebase.len();
        let sig = super::signature(repo)?;

        let conflict = self.advance_rebase(repo, &mut rebase, &sig)?;

        if let Some(current) = conflict {
            // 暂停：丢弃 rebase 和标注提交以释放不可变借用，再取快照
            drop(rebase);
            drop(branch_annotated);
            drop(onto_annotated);
            drop(head_ref);
            drop(onto_ref);
            let mut snapshot = self.snapshot_after_operation(repo)?;
            snapshot.rebase_in_progress = true;
            self.progress.emit(OperationEvent::Progress(format!(
                "变基暂停：正在重放提交 {current}/{total}，请解决冲突后继续"
            )));
            return Ok(RebaseOutcome::Conflicts {
                snapshot,
                current,
                total,
            });
        }

        rebase.finish(Some(&sig))?;
        drop(rebase);
        drop(branch_annotated);
        drop(onto_annotated);
        drop(head_ref);
        drop(onto_ref);
        self.progress
            .emit(OperationEvent::Finished(format!("已变基 {}", onto.0)));
        let mut snapshot = self.snapshot_after_operation(repo)?;
        snapshot.rebase_in_progress = false;
        Ok(RebaseOutcome::Completed(snapshot))
    }

    /// 变基遇到冲突后，用户已解决所有冲突，继续重放下一个提交。
    pub fn rebase_continue(&self, repo: &mut Repository) -> Result<RebaseOutcome> {
        self.progress
            .emit(OperationEvent::Started("正在继续变基".into()));

        let mut opts = RebaseOptions::new();
        let mut rebase =
            super::worktree_compat::open_rebase_preserving_locked_directories(repo, &mut opts)?;
        let total = rebase.len();
        let sig = super::signature(repo)?;

        // 用户已解决冲突，提交当前操作
        if repo.index()?.has_conflicts() {
            return Err(GitError::Message(
                "仍有冲突未解决，请先解决所有冲突再继续变基".into(),
            ));
        }
        rebase.commit(None, &sig, None)?;

        let conflict = self.advance_rebase(repo, &mut rebase, &sig)?;

        if let Some(current) = conflict {
            drop(rebase);
            let mut snapshot = self.snapshot_after_operation(repo)?;
            snapshot.rebase_in_progress = true;
            self.progress.emit(OperationEvent::Progress(format!(
                "变基暂停：正在重放提交 {current}/{total}，请解决冲突后继续"
            )));
            return Ok(RebaseOutcome::Conflicts {
                snapshot,
                current,
                total,
            });
        }

        rebase.finish(Some(&sig))?;
        drop(rebase);
        self.progress
            .emit(OperationEvent::Finished("变基已完成".into()));
        let mut snapshot = self.snapshot_after_operation(repo)?;
        snapshot.rebase_in_progress = false;
        Ok(RebaseOutcome::Completed(snapshot))
    }

    /// 跳过当前冲突提交：reset 到 HEAD 清理冲突状态后推进操作指针。
    pub fn rebase_skip(&self, repo: &mut Repository) -> Result<RebaseOutcome> {
        self.progress
            .emit(OperationEvent::Started("正在跳过变基提交".into()));

        let mut opts = RebaseOptions::new();
        let mut rebase =
            super::worktree_compat::open_rebase_preserving_locked_directories(repo, &mut opts)?;
        let total = rebase.len();
        let sig = super::signature(repo)?;

        // reset 到当前 HEAD，清除冲突状态
        {
            let head = repo.head()?.peel_to_commit()?;
            super::worktree_compat::reset_preserving_locked_directories(
                repo,
                head.as_object(),
                git2::ResetType::Hard,
            )?;
        }

        // 用干净索引提交当前操作（可能产生空提交，忽略错误以推进指针）
        let _ = rebase.commit(None, &sig, None);

        let conflict = self.advance_rebase(repo, &mut rebase, &sig)?;

        if let Some(current) = conflict {
            drop(rebase);
            let mut snapshot = self.snapshot_after_operation(repo)?;
            snapshot.rebase_in_progress = true;
            self.progress.emit(OperationEvent::Progress(format!(
                "变基暂停：正在重放提交 {current}/{total}，请解决冲突后继续"
            )));
            return Ok(RebaseOutcome::Conflicts {
                snapshot,
                current,
                total,
            });
        }

        rebase.finish(Some(&sig))?;
        drop(rebase);
        self.progress
            .emit(OperationEvent::Finished("变基已完成".into()));
        let mut snapshot = self.snapshot_after_operation(repo)?;
        snapshot.rebase_in_progress = false;
        Ok(RebaseOutcome::Completed(snapshot))
    }

    /// 中止变基，回到变基前的状态。
    pub fn rebase_abort(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started("正在中止变基".into()));

        let mut opts = RebaseOptions::new();
        let mut rebase =
            super::worktree_compat::open_rebase_preserving_locked_directories(repo, &mut opts)?;
        rebase.abort()?;
        drop(rebase);

        self.progress
            .emit(OperationEvent::Finished("变基已中止".into()));
        let mut snapshot = self.snapshot_after_operation(repo)?;
        snapshot.rebase_in_progress = false;
        Ok(snapshot)
    }

    /// pull --rebase：先 fetch 远端引用，再用 rebase 而非 merge 整合远端更新。
    /// 遇到冲突时返回 `Err(GitError::Conflicts)`，由 `with_repo` 走冲突处理路径。
    pub fn pull_branch_rebase(
        &self,
        repo: &mut Repository,
        remote: &RemoteName,
        remote_branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        super::validate_branch_name(&remote_branch.0)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在变基拉取 {}/{}",
            remote.0, remote_branch.0
        )));
        self.fetch_remote_refs(repo, remote)?;

        let remote_ref = self.remote_ref_for_remote_branch(repo, remote, &remote_branch.0)?;
        let upstream_annotated = repo.reference_to_annotated_commit(&remote_ref)?;
        let head_ref = repo.head()?;
        let branch_annotated = repo.reference_to_annotated_commit(&head_ref)?;

        let mut opts = RebaseOptions::new();
        let mut rebase = super::worktree_compat::rebase_preserving_locked_directories(
            repo,
            Some(&branch_annotated),
            Some(&upstream_annotated),
            None,
            &mut opts,
        )?;
        let sig = super::signature(repo)?;

        if self.advance_rebase(repo, &mut rebase, &sig)?.is_some() {
            // pull --rebase 遇到冲突：返回冲突错误让 with_repo 处理
            drop(rebase);
            drop(upstream_annotated);
            drop(branch_annotated);
            drop(head_ref);
            drop(remote_ref);
            let conflicts = self.conflicts(repo)?;
            return Err(GitError::Conflicts(conflicts));
        }

        rebase.finish(Some(&sig))?;
        drop(rebase);
        drop(upstream_annotated);
        drop(branch_annotated);
        drop(head_ref);
        drop(remote_ref);

        self.progress.emit(OperationEvent::Finished(format!(
            "已变基拉取 {}/{}",
            remote.0, remote_branch.0
        )));
        let mut snapshot = self.snapshot_after_operation(repo)?;
        snapshot.rebase_in_progress = false;
        Ok(snapshot)
    }

    /// 变基循环核心：逐个重放 Pick 操作，遇冲突返回 Some(current)（1-based 序号），全部完成返回 None。
    /// 调用方在返回 None 后负责调用 `rebase.finish()`。
    fn advance_rebase(
        &self,
        repo: &Repository,
        rebase: &mut git2::Rebase<'_>,
        sig: &git2::Signature<'_>,
    ) -> Result<Option<usize>> {
        while let Some(result) = rebase.next() {
            let op = result?;
            if matches!(op.kind(), Some(RebaseOperationType::Pick)) {
                if repo.index()?.has_conflicts() {
                    return Ok(Some(rebase.operation_current().unwrap_or(0) + 1));
                }
                rebase.commit(None, sig, None)?;
            }
            // 非 Pick 操作（Reword/Edit/Squash 等）在非交互式变基中不会出现，跳过
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "../tests/git/rebase.rs"]
mod tests;
