use git2::{ErrorCode, Repository, RepositoryState};

use crate::{
    GitService,
    types::{BranchName, GitError, RepositorySnapshot, Result, TagName},
};

/// 切换尝试的结果：要么成功切换，要么被未提交修改阻止（未做任何修改）。
pub enum CheckoutAttempt {
    Switched(RepositorySnapshot),
    /// 目标分支会覆盖本地修改：libgit2 safe checkout 失败时不会改动工作区
    /// 与 HEAD，调用方应向用户确认是否贮藏后重试。
    BlockedByLocalChanges,
}

/// 携带切换（贮藏 → 切换 → 恢复）的结果。
pub struct CarryingCheckoutOutcome {
    pub snapshot: RepositorySnapshot,
    /// 需要用户知晓的附加说明；自动恢复且无冲突时为 None。
    pub notice: Option<String>,
}

impl GitService {
    /// 切换本地分支；被未提交修改阻止时返回 `BlockedByLocalChanges`，
    /// 其余错误原样传播。
    pub fn checkout_branch_checked(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
    ) -> Result<CheckoutAttempt> {
        self.checkout_checked(repo, |service, repo| service.checkout_branch(repo, branch))
    }

    /// 检出远端分支（自动建/复用本地分支）的 checked 版本。
    pub fn checkout_remote_branch_checked(
        &self,
        repo: &mut Repository,
        remote_branch: &BranchName,
    ) -> Result<CheckoutAttempt> {
        self.checkout_checked(repo, |service, repo| {
            service.checkout_remote_branch(repo, remote_branch)
        })
    }

    /// 检出标签（detached HEAD）的 checked 版本。
    pub fn checkout_tag_checked(
        &self,
        repo: &mut Repository,
        tag: &TagName,
    ) -> Result<CheckoutAttempt> {
        self.checkout_checked(repo, |service, repo| service.checkout_tag(repo, tag))
    }

    /// 已确认携带：贮藏当前修改（含未跟踪文件）后切换。
    /// `auto_apply` 为 true 时自动恢复修改；为 false 时只贮藏并切换，
    /// 修改保留在贮藏中由用户稍后手动应用或弹出。
    pub fn checkout_branch_carrying_changes(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
        auto_apply: bool,
    ) -> Result<CarryingCheckoutOutcome> {
        let name = branch.0.clone();
        self.carrying_checkout(
            repo,
            &format!("分支 {name}"),
            auto_apply,
            move |service, repo| service.checkout_branch(repo, &BranchName::new(name)),
        )
    }

    /// 检出远端分支并携带未提交修改。
    pub fn checkout_remote_branch_carrying_changes(
        &self,
        repo: &mut Repository,
        remote_branch: &BranchName,
        auto_apply: bool,
    ) -> Result<CarryingCheckoutOutcome> {
        let name = remote_branch.0.clone();
        self.carrying_checkout(
            repo,
            &format!("远端分支 {name}"),
            auto_apply,
            move |service, repo| service.checkout_remote_branch(repo, &BranchName::new(name)),
        )
    }

    /// 检出标签并携带未提交修改。
    pub fn checkout_tag_carrying_changes(
        &self,
        repo: &mut Repository,
        tag: &TagName,
        auto_apply: bool,
    ) -> Result<CarryingCheckoutOutcome> {
        let name = tag.0.clone();
        self.carrying_checkout(
            repo,
            &format!("标签 {name}"),
            auto_apply,
            move |service, repo| service.checkout_tag(repo, &TagName::new(name)),
        )
    }

    fn checkout_checked<F>(
        &self,
        repo: &mut Repository,
        switch: F,
    ) -> Result<CheckoutAttempt>
    where
        F: FnOnce(&Self, &mut Repository) -> Result<RepositorySnapshot>,
    {
        match switch(self, repo) {
            Ok(snapshot) => Ok(CheckoutAttempt::Switched(snapshot)),
            Err(err) => {
                if is_blocked_by_local_changes(&err, repo) {
                    Ok(CheckoutAttempt::BlockedByLocalChanges)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn carrying_checkout<F>(
        &self,
        repo: &mut Repository,
        target: &str,
        auto_apply: bool,
        switch: F,
    ) -> Result<CarryingCheckoutOutcome>
    where
        F: FnOnce(&Self, &mut Repository) -> Result<RepositorySnapshot>,
    {
        // 已暂存/未暂存的划分在恢复时经 reinstantiate_index 还原，因此这里
        // 不保留 index（KEEP_INDEX 会把改动留在暂存区，阻碍 safe checkout）。
        let stash_message = format!("Khaslana 自动贮藏：切换至 {target}");
        self.save_stash(repo, &stash_message, true, false)?;
        // 贮藏后的切换快照不直接采用：恢复成功时以 apply 后的快照为准，
        // 恢复冲突时以带冲突索引的快照为准。
        if let Err(err) = switch(self, repo) {
            // 贮藏后仍无法切换：保留贮藏条目，把原修改交还用户处理，
            // 绝不丢弃。
            return Err(GitError::Message(format!(
                "已暂存未提交修改，但切换到{target}仍失败：{err}；修改已保留在贮藏中，可手动弹出或删除"
            )));
        }
        if !auto_apply {
            // 只贮藏并切换：修改留在贮藏列表第一条，稍后由用户手动应用或弹出。
            let snapshot = self.snapshot_after_operation(repo)?;
            return Ok(CarryingCheckoutOutcome {
                snapshot,
                notice: Some(format!(
                    "已切换到{target}，未提交修改已保留在贮藏列表第一条，需要时可在左侧「贮藏」区域应用或弹出"
                )),
            });
        }
        // 恢复分两步而非 pop：libgit2 的 git_stash_pop 在恢复冲突时也会删除
        // 贮藏条目（与 git 命令行“保留条目”不同），会丢掉用户的兜底拷贝。
        // 先 apply，仅在没有冲突时才显式 drop。
        match self.apply_stash_with_options(repo, 0, true) {
            Ok(snapshot) if snapshot.conflicts.is_empty() => {
                let snapshot = self.drop_stash(repo, 0)?;
                Ok(CarryingCheckoutOutcome { snapshot, notice: None })
            }
            Ok(snapshot) => Ok(CarryingCheckoutOutcome {
                snapshot,
                notice: Some(format!(
                    "已切换到{target}，但未提交修改恢复时存在冲突；原修改已保留在贮藏列表第一条，解决冲突后可手动删除该贮藏"
                )),
            }),
            Err(_) => {
                // apply 报错（恢复冲突）：冲突标记已落入工作区，贮藏条目仍在。
                let snapshot = self.snapshot_after_operation(repo)?;
                Ok(CarryingCheckoutOutcome {
                    snapshot,
                    notice: Some(format!(
                        "已切换到{target}，但未提交修改恢复时存在冲突；原修改已保留在贮藏列表第一条，解决冲突后可手动删除该贮藏"
                    )),
                })
            }
        }
    }
}

fn is_blocked_by_local_changes(err: &GitError, repo: &Repository) -> bool {
    // libgit2 的 safe checkout 在会覆盖本地修改时返回 GIT_ECONFLICT；
    // 仓库处于合并、变基中等其他状态时不走自动携带，错误原样上报。
    matches!(err, GitError::Git(git_err) if git_err.code() == ErrorCode::Conflict)
        && repo.state() == RepositoryState::Clean
}
