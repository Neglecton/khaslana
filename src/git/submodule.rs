use std::path::Path;

#[cfg(windows)]
use git2::Binding;
#[cfg(not(windows))]
use git2::SubmoduleUpdateOptions;
use git2::build::CheckoutBuilder;
use git2::{
    BranchType, Direction, FetchOptions, Oid, Repository, SubmoduleIgnore, SubmoduleStatus,
};

use super::{GitService, remote_fetch_url};
use crate::{
    GitError, OperationEvent, RepositorySnapshot, Result, SubmoduleInfo, SubmoduleRemoteSyncStatus,
    SubmoduleState,
};

impl GitService {
    pub fn submodules(&self, repo: &Repository) -> Result<Vec<SubmoduleInfo>> {
        let mut modules = Vec::new();
        for submodule in repo.submodules()? {
            let name = submodule.name()?.to_string();
            let status = repo.submodule_status(&name, SubmoduleIgnore::None)?;
            modules.push(SubmoduleInfo {
                name,
                path: submodule.path().to_path_buf(),
                url: submodule.url()?.map(str::to_string),
                branch: submodule.branch()?.map(str::to_string),
                head_id: submodule.head_id().map(|oid| oid.to_string()),
                index_id: submodule.index_id().map(|oid| oid.to_string()),
                workdir_id: submodule.workdir_id().map(|oid| oid.to_string()),
                status: submodule_state(status, submodule.index_id(), submodule.workdir_id()),
            });
        }
        modules.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(modules)
    }

    pub fn submodule_remote_sync_statuses(
        &self,
        repo: &Repository,
    ) -> Result<Vec<(String, SubmoduleRemoteSyncStatus)>> {
        let mut statuses = Vec::new();
        for submodule in repo.submodules()? {
            let name = submodule.name()?.to_string();
            let status = match self.submodule_remote_sync_status(repo, &submodule, &name) {
                Ok(status) => status,
                Err(err) => {
                    tracing::warn!("submodule remote sync status skipped: {err}");
                    SubmoduleRemoteSyncStatus::Unavailable(err.to_string())
                }
            };
            statuses.push((name, status));
        }
        statuses.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(statuses)
    }

    pub fn update_submodules(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started("正在同步子模块记录版本".into()));
        self.update_submodules_recursive(repo)?;
        self.progress
            .emit(OperationEvent::Finished("子模块已同步记录版本".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn update_submodules_to_remote_latest(
        &self,
        repo: &mut Repository,
    ) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started("正在更新子模块到远端最新".into()));
        self.update_submodules_to_remote_latest_recursive(repo)?;
        self.progress
            .emit(OperationEvent::Finished("子模块已更新到远端最新".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn update_submodule_to_remote_latest(
        &self,
        repo: &mut Repository,
        name: &str,
    ) -> Result<RepositorySnapshot> {
        self.progress.emit(OperationEvent::Started(format!(
            "正在更新子模块 {name} 到远端最新"
        )));
        self.update_one_submodule_to_remote_latest(repo, name)?;
        self.progress.emit(OperationEvent::Finished(format!(
            "子模块 {name} 已更新到远端最新"
        )));
        self.snapshot_after_operation(repo)
    }

    pub(crate) fn update_submodules_recursive(&self, repo: &Repository) -> Result<()> {
        self.update_submodules_in_repo(repo)?;
        for submodule in repo.submodules()? {
            let Ok(subrepo) = submodule.open() else {
                continue;
            };
            self.update_submodules_recursive(&subrepo)?;
        }
        Ok(())
    }

    fn update_submodules_in_repo(&self, repo: &Repository) -> Result<()> {
        for mut submodule in repo.submodules()? {
            let name = submodule.name()?.to_string();
            let path = submodule.path().to_path_buf();
            self.ensure_submodule_clean(repo, &name, &path)?;
            self.progress
                .emit(OperationEvent::Progress(format!("正在同步子模块 {name}")));
            self.checkout_submodule_recorded_commit(repo, &mut submodule, &name)?;
        }
        Ok(())
    }

    fn update_submodules_to_remote_latest_recursive(&self, repo: &Repository) -> Result<()> {
        let names = repo
            .submodules()?
            .into_iter()
            .map(|submodule| submodule.name().map(str::to_string).map_err(GitError::from))
            .collect::<Result<Vec<_>>>()?;
        for name in names {
            self.update_one_submodule_to_remote_latest(repo, &name)?;
        }
        Ok(())
    }

    fn update_one_submodule_to_remote_latest(&self, repo: &Repository, name: &str) -> Result<()> {
        let mut submodule = repo.find_submodule(name)?;
        let path = submodule.path().to_path_buf();
        self.ensure_submodule_clean(repo, name, &path)?;
        if submodule_needs_recorded_checkout(repo, name, &submodule)? {
            self.progress.emit(OperationEvent::Progress(format!(
                "正在同步子模块 {name} 的记录版本"
            )));
            self.checkout_submodule_recorded_commit(repo, &mut submodule, name)?;
        } else {
            submodule.init(true)?;
            submodule.sync()?;
        }
        drop(submodule);

        let subrepo = repo.find_submodule(name)?.open()?;
        self.ensure_submodule_repository_clean(&subrepo, &path)?;
        let target = self.fetch_submodule_remote_latest(repo, &subrepo, name)?;
        self.fast_forward_submodule_to(&subrepo, target, &path)?;
        self.update_submodules_to_remote_latest_recursive(&subrepo)
    }

    fn submodule_remote_sync_status(
        &self,
        repo: &Repository,
        submodule: &git2::Submodule<'_>,
        name: &str,
    ) -> Result<SubmoduleRemoteSyncStatus> {
        let parent_status = repo.submodule_status(name, SubmoduleIgnore::None)?;
        let state = submodule_state(parent_status, submodule.index_id(), submodule.workdir_id());
        if !state.initialized {
            return Ok(SubmoduleRemoteSyncStatus::Unavailable(
                "子模块未初始化".to_string(),
            ));
        }
        if !state.checked_out {
            return Ok(SubmoduleRemoteSyncStatus::Unavailable(
                "子模块未检出".to_string(),
            ));
        }

        let subrepo = submodule.open()?;
        let head = subrepo.head()?;
        let current = head
            .target()
            .or_else(|| head.peel_to_commit().ok().map(|commit| commit.id()))
            .ok_or_else(|| GitError::Message(format!("子模块 {name} 当前 HEAD 无法解析")))?;
        let target = self.fetch_submodule_remote_latest(repo, &subrepo, name)?;
        let (ahead, behind) = subrepo.graph_ahead_behind(current, target)?;
        Ok(SubmoduleRemoteSyncStatus::from_ahead_behind(ahead, behind))
    }

    fn checkout_submodule_recorded_commit(
        &self,
        repo: &Repository,
        submodule: &mut git2::Submodule<'_>,
        name: &str,
    ) -> Result<()> {
        submodule.init(true)?;
        submodule.sync()?;

        let _remote_context = self.set_submodule_context(repo, name);
        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(self.remote_callbacks(Some(repo)));
        let submodule_url = submodule.url()?.map(str::to_string);
        self.apply_fetch_proxy(&mut fetch_options, submodule_url.as_deref())?;

        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        #[cfg(windows)]
        {
            let mut raw_options = unsafe { std::mem::zeroed() };
            super::worktree_compat::raw_git_result(unsafe {
                libgit2_sys::git_submodule_update_init_options(
                    &mut raw_options,
                    libgit2_sys::GIT_SUBMODULE_UPDATE_OPTIONS_VERSION,
                )
            })?;
            raw_options.checkout_opts =
                super::worktree_compat::checkout_options_preserving_locked_directories(
                    &mut checkout,
                )?;
            raw_options.fetch_opts = fetch_options.raw();
            raw_options.allow_fetch = 1;
            super::worktree_compat::raw_git_result(unsafe {
                libgit2_sys::git_submodule_update(submodule.raw(), 1, &mut raw_options)
            })?;
        }
        #[cfg(not(windows))]
        {
            let mut options = SubmoduleUpdateOptions::new();
            options.fetch(fetch_options).checkout(checkout);
            submodule.update(true, Some(&mut options))?;
        }
        Ok(())
    }

    fn fetch_submodule_remote_latest(
        &self,
        parent_repo: &Repository,
        subrepo: &Repository,
        name: &str,
    ) -> Result<Oid> {
        let remote_name = submodule_remote_name(subrepo, name)?;
        let _remote_context = self.set_submodule_context(parent_repo, name);
        let mut remote = subrepo.find_remote(&remote_name)?;
        let remote_url = remote_fetch_url(&remote);
        let branch_name = self.submodule_target_branch(
            parent_repo,
            subrepo,
            name,
            &remote_name,
            remote_url.as_deref(),
        )?;
        let mut options = FetchOptions::new();
        options.remote_callbacks(self.remote_callbacks(Some(parent_repo)));
        self.apply_fetch_proxy(&mut options, remote_url.as_deref())?;
        let refspec = format!("+refs/heads/{branch_name}:refs/remotes/{remote_name}/{branch_name}");
        remote.fetch(
            &[refspec.as_str()],
            Some(&mut options),
            Some("khaslana submodule fetch"),
        )?;
        drop(remote);
        drop(options);

        let remote_ref = format!("refs/remotes/{remote_name}/{branch_name}");
        let reference = subrepo.find_reference(&remote_ref).map_err(|err| {
            GitError::Message(format!("子模块 {name} 未找到远端分支 {remote_ref}：{err}"))
        })?;
        reference
            .target()
            .ok_or_else(|| GitError::Message(format!("子模块 {name} 的远端分支不是直接引用")))
    }

    fn fast_forward_submodule_to(
        &self,
        subrepo: &Repository,
        target: Oid,
        path: &Path,
    ) -> Result<()> {
        let head = subrepo.head()?;
        let current = head
            .target()
            .or_else(|| head.peel_to_commit().ok().map(|commit| commit.id()))
            .ok_or_else(|| {
                GitError::Message(format!("子模块 {} 当前 HEAD 无法解析", path.display()))
            })?;
        if current == target {
            return Ok(());
        }
        if !subrepo.graph_descendant_of(target, current)? {
            return Err(GitError::Message(format!(
                "子模块 {} 不能快进到远端最新，请先手动处理分叉或本地提交",
                path.display()
            )));
        }

        let target_object = subrepo.find_object(target, None)?;
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        super::worktree_compat::checkout_tree_preserving_locked_directories(
            subrepo,
            &target_object,
            &mut checkout,
        )?;

        if head.is_branch()
            && let Ok(refname) = head.name()
        {
            let refname = refname.to_string();
            let mut reference = subrepo.find_reference(&refname)?;
            reference.set_target(target, "khaslana submodule fast-forward")?;
            subrepo.set_head(&refname)?;
        } else {
            subrepo.set_head_detached(target)?;
        }
        Ok(())
    }

    fn ensure_submodule_clean(&self, repo: &Repository, name: &str, path: &Path) -> Result<()> {
        let status = repo.submodule_status(name, SubmoduleIgnore::None)?;
        if submodule_has_local_workdir_changes(status) {
            return Err(GitError::Message(format!(
                "子模块 {} 有本地改动，请先在子模块中提交、贮藏或清理后再更新",
                path.display()
            )));
        }
        Ok(())
    }

    fn ensure_submodule_repository_clean(&self, subrepo: &Repository, path: &Path) -> Result<()> {
        if subrepo.statuses(None)?.iter().any(|entry| {
            let status = entry.status();
            status.is_wt_modified()
                || status.is_wt_new()
                || status.is_wt_deleted()
                || status.is_wt_renamed()
                || status.is_wt_typechange()
                || status.is_index_modified()
                || status.is_index_new()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange()
                || status.is_conflicted()
        }) {
            return Err(GitError::Message(format!(
                "子模块 {} 有本地改动，请先在子模块中提交、贮藏或清理后再更新",
                path.display()
            )));
        }
        Ok(())
    }

    fn submodule_target_branch(
        &self,
        parent_repo: &Repository,
        subrepo: &Repository,
        submodule_name: &str,
        remote_name: &str,
        remote_url: Option<&str>,
    ) -> Result<String> {
        let submodule = parent_repo.find_submodule(submodule_name)?;
        if let Some(branch) = submodule.branch()? {
            if branch == "." {
                return parent_current_branch(parent_repo).ok_or_else(|| {
                    GitError::Message(format!(
                        "子模块 {submodule_name} 配置 branch = .，但父仓库当前不是本地分支"
                    ))
                });
            }
            return Ok(branch.to_string());
        }
        self.submodule_remote_head_branch(subrepo, submodule_name, remote_name, remote_url)
    }

    fn submodule_remote_head_branch(
        &self,
        subrepo: &Repository,
        submodule_name: &str,
        remote_name: &str,
        remote_url: Option<&str>,
    ) -> Result<String> {
        if let Some(branch) = existing_remote_head_branch(subrepo, remote_name)? {
            return Ok(branch);
        }

        let mut remote = subrepo.find_remote(remote_name)?;
        let callbacks = self.remote_callbacks(None);
        let proxy_options = self.proxy_options_for_remote(remote_url)?;
        if let Ok(connection) =
            remote.connect_auth(Direction::Fetch, Some(callbacks), proxy_options)
            && let Ok(buf) = connection.default_branch()
            && let Ok(name) = buf.as_str()
            && let Some(branch) = name.strip_prefix("refs/heads/")
        {
            return Ok(branch.to_string());
        }
        drop(remote);

        if let Some(branch) = fallback_remote_branch(subrepo, remote_name)? {
            return Ok(branch);
        }

        Err(GitError::Message(format!(
            "子模块 {submodule_name} 未配置跟踪分支，且无法确定远端默认分支"
        )))
    }
}

fn submodule_state(
    status: SubmoduleStatus,
    index_id: Option<git2::Oid>,
    workdir_id: Option<git2::Oid>,
) -> SubmoduleState {
    let initialized = !status.is_wd_uninitialized();
    let checked_out = status.is_in_wd() && !status.is_wd_deleted() && workdir_id.is_some();
    let head_matches_index = index_id.is_some() && workdir_id.is_some() && index_id == workdir_id;
    SubmoduleState {
        initialized,
        checked_out,
        head_matches_index,
        workdir_modified: status.is_wd_wd_modified()
            || status.is_wd_added()
            || status.is_wd_deleted(),
        workdir_untracked: status.is_wd_untracked(),
    }
}

fn submodule_has_local_workdir_changes(status: SubmoduleStatus) -> bool {
    status.is_wd_wd_modified()
        || status.is_wd_untracked()
        || status.is_wd_added()
        || status.is_wd_deleted()
}

fn submodule_needs_recorded_checkout(
    repo: &Repository,
    name: &str,
    submodule: &git2::Submodule<'_>,
) -> Result<bool> {
    let status = repo.submodule_status(name, SubmoduleIgnore::None)?;
    Ok(status.is_wd_uninitialized()
        || !status.is_in_wd()
        || status.is_wd_deleted()
        || submodule.workdir_id().is_none())
}

fn submodule_remote_name(subrepo: &Repository, submodule_name: &str) -> Result<String> {
    if subrepo.find_remote("origin").is_ok() {
        return Ok("origin".to_string());
    }
    let remotes = subrepo.remotes()?;
    let names = remotes.iter().flatten().flatten().collect::<Vec<_>>();
    if names.len() == 1 {
        return Ok(names[0].to_string());
    }
    Err(GitError::Message(format!(
        "子模块 {submodule_name} 无法确定远端，请保留 origin 或只配置一个远端"
    )))
}

fn parent_current_branch(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().ok().map(str::to_string)
}

fn existing_remote_head_branch(subrepo: &Repository, remote_name: &str) -> Result<Option<String>> {
    let remote_head = format!("refs/remotes/{remote_name}/HEAD");
    let remote_prefix = format!("refs/remotes/{remote_name}/");
    if let Ok(head) = subrepo.find_reference(&remote_head)
        && let Ok(Some(symbolic)) = head.symbolic_target()
        && let Some(branch) = symbolic.strip_prefix(&remote_prefix)
    {
        return Ok(Some(branch.to_string()));
    }
    Ok(None)
}

fn fallback_remote_branch(subrepo: &Repository, remote_name: &str) -> Result<Option<String>> {
    for candidate in ["main", "master"] {
        if subrepo
            .find_branch(&format!("{remote_name}/{candidate}"), BranchType::Remote)
            .is_ok()
        {
            return Ok(Some(candidate.to_string()));
        }
    }

    let remote_prefix = format!("{remote_name}/");
    let mut remote_branches = Vec::new();
    for branch in subrepo.branches(Some(BranchType::Remote))? {
        let (branch, _) = branch?;
        let Some(name) = branch.name()?.map(str::to_string) else {
            continue;
        };
        let Some(short) = name.strip_prefix(&remote_prefix) else {
            continue;
        };
        if short != "HEAD" {
            remote_branches.push(short.to_string());
        }
    }
    if remote_branches.len() == 1 {
        return Ok(Some(remote_branches.remove(0)));
    }

    Ok(None)
}
