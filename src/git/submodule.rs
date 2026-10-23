use crate::types::domain_ui_resources;
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
                Err(err) if crate::ssh_host_trust::is_security_error(&err) => return Err(err),
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
            .emit(OperationEvent::Started(crate::ui_i18n::text(domain_ui_resources::SYNCHRONIZING_RECORDED_SUBMODULE_REVISIONS).into()));
        self.update_submodules_recursive(repo)?;
        self.progress
            .emit(OperationEvent::Finished(crate::ui_i18n::text(domain_ui_resources::RECORDED_SUBMODULE_REVISIONS_SYNCHRONIZED).into()));
        self.snapshot_after_operation(repo)
    }

    pub fn update_submodules_to_remote_latest(
        &self,
        repo: &mut Repository,
    ) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started(crate::ui_i18n::text(domain_ui_resources::UPDATING_SUBMODULES_TO_THE_LATEST_REMOTE_REVISIONS).into()));
        self.update_submodules_to_remote_latest_recursive(repo)?;
        self.progress
            .emit(OperationEvent::Finished(crate::ui_i18n::text(domain_ui_resources::SUBMODULES_UPDATED_TO_THE_LATEST_REMOTE_REVISIONS).into()));
        self.snapshot_after_operation(repo)
    }

    pub fn update_submodule_to_remote_latest(
        &self,
        repo: &mut Repository,
        name: &str,
    ) -> Result<RepositorySnapshot> {
        self.progress.emit(OperationEvent::Started(crate::ui_i18n::format(domain_ui_resources::UPDATING_SUBMODULE_TO_THE_LATEST_REMOTE_REVISION, &[("name", &format!("{}", name))])));
        self.update_one_submodule_to_remote_latest(repo, name)?;
        self.progress.emit(OperationEvent::Finished(crate::ui_i18n::format(domain_ui_resources::SUBMODULE_UPDATED_TO_THE_LATEST_REMOTE_REVISION, &[("name", &format!("{}", name))])));
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
                .emit(OperationEvent::Progress(crate::ui_i18n::format(domain_ui_resources::SYNCHRONIZING_SUBMODULE, &[("name", &format!("{}", name))])));
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
            self.progress.emit(OperationEvent::Progress(crate::ui_i18n::format(domain_ui_resources::SYNCHRONIZING_THE_RECORDED_REVISION_OF_SUBMODULE, &[("name", &format!("{}", name))])));
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
                crate::ui_i18n::text(domain_ui_resources::SUBMODULE_IS_NOT_INITIALIZED).to_string(),
            ));
        }
        if !state.checked_out {
            return Ok(SubmoduleRemoteSyncStatus::Unavailable(
                crate::ui_i18n::text(domain_ui_resources::SUBMODULE_IS_NOT_CHECKED_OUT).to_string(),
            ));
        }

        let subrepo = submodule.open()?;
        let head = subrepo.head()?;
        let current = head
            .target()
            .or_else(|| head.peel_to_commit().ok().map(|commit| commit.id()))
            .ok_or_else(|| GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::CANNOT_RESOLVE_CURRENT_HEAD_OF_SUBMODULE, &[("name", &format!("{}", name))])))?;
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
        // init 已将相对 .gitmodules URL 解析到父仓库配置；不能拿相对路径猜测 SSH endpoint。
        let submodule_url = repo.config()?.get_string(&format!("submodule.{name}.url")).ok()
            .or(submodule.url()?.map(str::to_string));
        fetch_options.remote_callbacks(self.remote_callbacks_for_url(Some(repo), submodule_url.as_deref()));
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
        options.remote_callbacks(self.remote_callbacks_for_url(Some(parent_repo), remote_url.as_deref()));
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
            GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::REMOTE_BRANCH_NOT_FOUND_IN_SUBMODULE, &[("name", &format!("{}", name)), ("remote_ref", &format!("{}", remote_ref)), ("err", &format!("{}", err))]))
        })?;
        reference
            .target()
            .ok_or_else(|| GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::THE_REMOTE_BRANCH_OF_SUBMODULE_IS_NOT_A_DIRECT_REFERENCE, &[("name", &format!("{}", name))])))
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
                GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::CANNOT_RESOLVE_CURRENT_HEAD_OF_SUBMODULE_2, &[("value0", &format!("{}", path.display()))]))
            })?;
        if current == target {
            return Ok(());
        }
        if !subrepo.graph_descendant_of(target, current)? {
            return Err(GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::SUBMODULE_CANNOT_FAST_FORWARD_TO_THE_LATEST_REMOTE_REVISION_RESOLVE_DIVERGENCE_OR_LOC, &[("value0", &format!("{}", path.display()))])));
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
            return Err(GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::SUBMODULE_HAS_LOCAL_CHANGES_COMMIT_STASH_OR_DISCARD_THEM_IN_THE_SUBMODULE_BEFORE_UPDA, &[("value0", &format!("{}", path.display()))])));
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
            return Err(GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::SUBMODULE_HAS_LOCAL_CHANGES_COMMIT_STASH_OR_DISCARD_THEM_IN_THE_SUBMODULE_BEFORE_UPDA, &[("value0", &format!("{}", path.display()))])));
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
                    GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::SUBMODULE_HAS_BRANCH_BUT_THE_PARENT_REPOSITORY_IS_NOT_ON_A_LOCAL_BRANCH, &[("submodule_name", &format!("{}", submodule_name))]))
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
        let callbacks = self.remote_callbacks_for_url(None, remote_url);
        let proxy_options = self.proxy_options_for_remote(remote_url)?;
        {
            // 安全与认证失败必须向上传递，不得伪装成探测不到默认分支。
            let connection = remote.connect_auth(Direction::Fetch, Some(callbacks), proxy_options)?;
            if let Ok(buf) = connection.default_branch()
                && let Ok(name) = buf.as_str()
                && let Some(branch) = name.strip_prefix("refs/heads/")
            {
                return Ok(branch.to_string());
            }
        }
        drop(remote);

        if let Some(branch) = fallback_remote_branch(subrepo, remote_name)? {
            return Ok(branch);
        }

        Err(GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::SUBMODULE_HAS_NO_TRACKING_BRANCH_AND_ITS_REMOTE_DEFAULT_BRANCH_CANNOT_BE_DETERMINED, &[("submodule_name", &format!("{}", submodule_name))])))
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
    Err(GitError::Localized(crate::ui_i18n::LocalizedMessage::new(domain_ui_resources::CANNOT_DETERMINE_THE_REMOTE_FOR_SUBMODULE_KEEP_ORIGIN_OR_CONFIGURE_ONLY_ONE_REMOTE, &[("submodule_name", &format!("{}", submodule_name))])))
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
