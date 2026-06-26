use git2::Repository;
use serde::Deserialize;

use crate::{GitError, Result};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RemoteBranchGuardAction {
    Fail,
    Continue,
}

impl RemoteBranchGuardAction {
    pub(super) fn should_fail(self) -> bool {
        matches!(self, Self::Fail)
    }
}

pub(super) fn default_on_exists() -> RemoteBranchGuardAction {
    RemoteBranchGuardAction::Fail
}

pub(super) fn default_on_missing() -> RemoteBranchGuardAction {
    RemoteBranchGuardAction::Continue
}

pub(super) fn default_guard_fetch() -> bool {
    true
}

pub(super) fn validate_remote_branch_name(remote: &str, branch: &str) -> Result<()> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(GitError::Message("远端分支名不能为空".into()));
    }
    if branch
        .strip_prefix(remote)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err(GitError::Message(format!(
            "远端分支名不要带远端名前缀，请填写不含 {remote}/ 的分支名：{branch}"
        )));
    }
    if branch.starts_with('/') || branch.ends_with('/') || branch.contains("//") {
        return Err(GitError::Message(format!("远端分支名无效：{branch}")));
    }
    Ok(())
}

pub(super) fn remote_branch_exists(repo: &Repository, remote: &str, branch: &str) -> Result<bool> {
    validate_remote_branch_name(remote, branch)?;
    let refname = format!("refs/remotes/{remote}/{branch}");
    match repo.find_reference(&refname) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(err) => Err(GitError::Git(err)),
    }
}

pub(super) fn guard_remote_branch(
    repo: &Repository,
    remote: &str,
    branch: &str,
    on_exists: RemoteBranchGuardAction,
    on_missing: RemoteBranchGuardAction,
) -> Result<()> {
    let exists = remote_branch_exists(repo, remote, branch)?;
    if exists && on_exists.should_fail() {
        return Err(GitError::Message(format!(
            "远端分支已存在：{remote}/{branch}"
        )));
    }
    if !exists && on_missing.should_fail() {
        return Err(GitError::Message(format!(
            "远端分支不存在：{remote}/{branch}"
        )));
    }
    Ok(())
}

pub(super) fn guard_summary(
    remote: &str,
    branch: &str,
    fetch: bool,
    on_exists: RemoteBranchGuardAction,
    on_missing: RemoteBranchGuardAction,
) -> String {
    let refresh = if fetch {
        "刷新后"
    } else {
        "基于本地引用"
    };
    let exists = match on_exists {
        RemoteBranchGuardAction::Fail => "存在则停止",
        RemoteBranchGuardAction::Continue => "存在则继续",
    };
    let missing = match on_missing {
        RemoteBranchGuardAction::Fail => "不存在则停止",
        RemoteBranchGuardAction::Continue => "不存在则继续",
    };
    format!("检查远端分支 {remote}/{branch}（{refresh}，{exists}，{missing}）")
}

#[cfg(test)]
#[path = "../tests/workflow/remote_branch_guard.rs"]
mod tests;
