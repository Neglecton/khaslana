use std::path::Path;

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
        // 恢复：apply 后按冲突类型决定是否删除贮藏条目。
        let snapshot = match self.apply_stash_with_options(repo, 0, true) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                // 分支已经切换，必须把新状态回传给 UI；贮藏保留供用户重试。
                let snapshot = self.snapshot_after_operation(repo)?;
                return Ok(CarryingCheckoutOutcome {
                    snapshot,
                    notice: Some(format!(
                        "已切换到{target}，但恢复修改失败：{err}；原修改保留在贮藏列表第一条，请检查工作区后手动应用"
                    )),
                });
            }
        };
        if snapshot.conflicts.is_empty() {
            let snapshot = self.drop_stash(repo, 0)?;
            return Ok(CarryingCheckoutOutcome { snapshot, notice: None });
        }
        // 「贮藏新增、目标分支没有」型的索引冲突可自动落定；仅工作区标记
        // 的冲突（无索引阶段）不在此列，落定后仍留在 conflicts 中。
        let resolved_paths = self.resolve_stash_only_conflicts(repo)?;
        let remaining = snapshot
            .conflicts
            .iter()
            .filter(|path| !resolved_paths.contains(path))
            .count();
        if remaining == 0 {
            // 冲突全部自动落定，与无缝恢复等价，清除贮藏条目。
            let snapshot = self.drop_stash(repo, 0)?;
            return Ok(CarryingCheckoutOutcome { snapshot, notice: None });
        }
        let index_conflicts = self.conflicts(repo)?;
        let worktree_only = snapshot
            .conflicts
            .iter()
            .any(|path| !index_conflicts.contains(path));
        Ok(CarryingCheckoutOutcome {
            snapshot,
            notice: Some(if worktree_only {
                format!(
                    "已切换到{target}，但恢复修改时产生工作区冲突；贮藏已保留。请在「冲突」区域查看文件，并用外部编辑器处理冲突标记"
                )
            } else {
                format!(
                    "已切换到{target}，但未提交修改恢复时存在冲突；原修改已保留在贮藏列表第一条，请在冲突区域解决"
                )
            }),
        })
    }

    /// 落定「贮藏新增、目标分支没有」型的索引冲突：这类文件只存在于贮藏里
    /// （目标分支没有该路径），apply 已把贮藏内容写入工作区，只需清除
    /// 索引冲突阶段，文件仍作为未跟踪修改留在工作区。
    ///
    /// 返回成功落定的路径列表；存在目标分支也有改动的真冲突（our 阶段非空）
    /// 或写入失败时返回空列表（放弃自动落定，保留冲突与贮藏原样）。
    fn resolve_stash_only_conflicts(&self, repo: &mut Repository) -> Result<Vec<String>> {
        let mut index = repo.index()?;
        if !index.has_conflicts() {
            return Ok(Vec::new());
        }
        let mut stash_only_paths = Vec::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            match (conflict.our.as_ref(), conflict.their.as_ref()) {
                (None, Some(their)) => {
                    let path = std::str::from_utf8(&their.path).map_err(|_| {
                        GitError::Message("冲突文件路径不是有效 UTF-8，无法自动落定".into())
                    })?;
                    stash_only_paths.push(path.to_string());
                }
                // our 阶段存在：目标分支该文件也有内容，属真冲突，不自动处理。
                _ => return Ok(Vec::new()),
            }
        }
        let mut resolved = Vec::new();
        for path in &stash_only_paths {
            // 目标分支没有该文件；只清除冲突阶段，保留工作区文件为未跟踪，
            // 不用 add_path 把原本未暂存的修改悄悄放进暂存区。
            if index.conflict_remove(Path::new(path)).is_err() {
                return Ok(Vec::new());
            }
            resolved.push(path.clone());
        }
        index.write()?;
        Ok(resolved)
    }
}

fn is_blocked_by_local_changes(err: &GitError, repo: &Repository) -> bool {
    // libgit2 的 safe checkout 在会覆盖本地修改时返回 GIT_ECONFLICT；
    // 仓库处于合并、变基中等其他状态时不走自动携带，错误原样上报。
    matches!(err, GitError::Git(git_err) if git_err.code() == ErrorCode::Conflict)
        && repo.state() == RepositoryState::Clean
}
