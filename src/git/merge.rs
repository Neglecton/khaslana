//! 普通合并会话：开始、完成和中止。

use git2::build::CheckoutBuilder;
use git2::{AnnotatedCommit, MergeAnalysis, MergeOptions, Repository, RepositoryState, ResetType};

use super::worktree_compat::{
    checkout_tree_preserving_locked_directories, merge_preserving_locked_directories,
    reset_preserving_locked_directories,
};
use super::{GitService, signature};
use crate::types::{
    BranchName, CommitMessage, GitError, OperationEvent, RepositorySnapshot, Result,
};

impl GitService {
    pub fn merge_branch(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started(format!("正在合并 {}", branch.0)));
        let reference = self.find_branch_reference(repo, &branch.0)?;
        let annotated = repo.reference_to_annotated_commit(&reference)?;
        self.merge_annotated(repo, &annotated, &branch.0)?;
        drop(annotated);
        drop(reference);
        // 无冲突的正常非快进合并：自动提交（双父提交），不保留 merge 会话。
        if merge_in_progress(repo) {
            let message =
                merge_message(repo).unwrap_or_else(|| format!("Merge branch '{}'", branch.0));
            let mut merge_head_ids = Vec::new();
            repo.mergehead_foreach(|oid| {
                merge_head_ids.push(*oid);
                true
            })?;
            self.commit_merge(repo, &CommitMessage::new(message), &merge_head_ids)?;
        }
        let message = if merge_in_progress(repo) {
            format!("{} 的合并结果已写入暂存区", branch.0)
        } else {
            format!("已合并 {}", branch.0)
        };
        self.progress.emit(OperationEvent::Finished(message));
        self.snapshot_after_operation(repo)
    }

    pub(super) fn merge_annotated(
        &self,
        repo: &Repository,
        annotated: &AnnotatedCommit<'_>,
        label: &str,
    ) -> Result<()> {
        self.ensure_clean_for_merge(repo)?;
        let (analysis, _preference) = repo.merge_analysis(&[annotated])?;

        if analysis.contains(MergeAnalysis::ANALYSIS_UP_TO_DATE) {
            return Ok(());
        }
        if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
            fast_forward(repo, annotated)?;
            return Ok(());
        }
        if !analysis.contains(MergeAnalysis::ANALYSIS_NORMAL) {
            return Err(GitError::Message(format!(
                "无法合并 {label}：不支持的合并分析结果"
            )));
        }

        let mut merge_options = MergeOptions::new();
        let mut checkout = CheckoutBuilder::new();
        checkout
            .safe()
            .allow_conflicts(true)
            .conflict_style_merge(true);
        merge_preserving_locked_directories(
            repo,
            &[annotated],
            Some(&mut merge_options),
            &mut checkout,
        )?;

        if repo.index()?.has_conflicts() {
            // 保留 MERGE_HEAD、MERGE_MSG 和冲突 index，等待用户解决并完成合并。
            return Err(GitError::Conflicts(self.conflicts(repo)?));
        }

        // 无冲突的正常合并：由 merge_branch 自动提交（需要 &mut Repository 调用 mergehead_foreach）。
        Ok(())
    }

    pub fn finish_merge(
        &self,
        repo: &mut Repository,
        message: &CommitMessage,
    ) -> Result<RepositorySnapshot> {
        if !merge_in_progress(repo) {
            return Err(GitError::Message("当前没有正在进行的合并".into()));
        }
        self.progress
            .emit(OperationEvent::Started("正在完成合并".into()));
        let mut merge_head_ids = Vec::new();
        repo.mergehead_foreach(|oid| {
            merge_head_ids.push(*oid);
            true
        })?;
        self.commit_merge(repo, message, &merge_head_ids)?;
        self.progress
            .emit(OperationEvent::Finished("合并提交已创建".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn abort_merge(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        if !merge_in_progress(repo) {
            return Err(GitError::Message("当前没有正在进行的合并".into()));
        }

        self.progress
            .emit(OperationEvent::Started("正在中止合并".into()));
        let head = repo.head()?.peel_to_commit()?;
        reset_preserving_locked_directories(repo, head.as_object(), ResetType::Hard)?;
        drop(head);
        repo.cleanup_state()?;
        self.progress
            .emit(OperationEvent::Finished("合并已中止".into()));
        self.snapshot_after_operation(repo)
    }

    fn ensure_clean_for_merge(&self, repo: &Repository) -> Result<()> {
        if repo.state() != RepositoryState::Clean {
            return Err(GitError::Message(
                "仓库正在执行其他 Git 操作，请先完成或中止后再合并".into(),
            ));
        }
        if !self.status_full(repo)?.is_empty() || !self.conflicts(repo)?.is_empty() {
            return Err(GitError::Message(
                "当前存在未提交改动，请先提交或贮藏后再合并".into(),
            ));
        }
        Ok(())
    }

    fn commit_merge(
        &self,
        repo: &Repository,
        message: &CommitMessage,
        merge_head_ids: &[git2::Oid],
    ) -> Result<()> {
        if !merge_in_progress(repo) {
            return Err(GitError::Message("当前没有正在进行的合并".into()));
        }

        let message = message.0.trim();
        if message.is_empty() {
            return Err(GitError::EmptyCommitMessage);
        }

        let mut index = repo.index()?;
        if index.has_conflicts() {
            return Err(GitError::Conflicts(self.conflicts(repo)?));
        }
        let tree_id = index.write_tree()?;
        if merge_head_ids.is_empty() {
            return Err(GitError::Message(
                "合并状态缺少 MERGE_HEAD，无法完成合并".into(),
            ));
        }
        let tree = repo.find_tree(tree_id)?;
        let head = repo.head()?.peel_to_commit()?;
        let merge_heads = merge_head_ids
            .iter()
            .map(|oid| repo.find_commit(*oid))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut parents = Vec::with_capacity(merge_heads.len() + 1);
        parents.push(&head);
        parents.extend(merge_heads.iter());
        let signature = signature(repo)?;

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )?;
        // 只有提交成功后才清理状态，失败时用户仍可修改信息后重试或中止。
        repo.cleanup_state()?;
        Ok(())
    }
}

pub(super) fn merge_in_progress(repo: &Repository) -> bool {
    repo.state() == RepositoryState::Merge
}

pub(super) fn merge_message(repo: &Repository) -> Option<String> {
    merge_in_progress(repo)
        .then(|| repo.message().ok())
        .flatten()
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn fast_forward(repo: &Repository, annotated: &AnnotatedCommit<'_>) -> Result<()> {
    let refname = repo.head()?.name().map_err(GitError::from)?.to_string();
    let target = repo.find_object(annotated.id(), None)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    checkout_tree_preserving_locked_directories(repo, &target, &mut checkout)?;

    let mut reference = repo.find_reference(&refname)?;
    reference.set_target(annotated.id(), "khaslana fast-forward")?;
    repo.set_head(&refname)?;
    Ok(())
}
