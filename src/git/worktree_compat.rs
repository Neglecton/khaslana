#[cfg(windows)]
use git2::Binding;
use git2::build::CheckoutBuilder;
use git2::{
    AnnotatedCommit, CherrypickOptions, MergeOptions, Repository, ResetType, RevertOptions,
    Signature, StashApplyOptions, StashFlags,
};

#[cfg(windows)]
pub(super) fn raw_git_result(code: i32) -> std::result::Result<(), git2::Error> {
    if code < 0 {
        Err(git2::Error::last_error(code))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(super) fn checkout_options_preserving_locked_directories(
    checkout: &mut CheckoutBuilder<'_>,
) -> std::result::Result<libgit2_sys::git_checkout_options, git2::Error> {
    let mut raw_options = unsafe { std::mem::zeroed() };
    raw_git_result(unsafe {
        libgit2_sys::git_checkout_init_options(
            &mut raw_options,
            libgit2_sys::GIT_CHECKOUT_OPTIONS_VERSION,
        )
    })?;
    unsafe {
        checkout.configure(&mut raw_options);
    }
    // VS Code、终端或语言服务可能占用其打开的子目录。受 Git 管理的文件已正确
    // 删除后，只跳过 Windows 无法移除的空目录，与系统 Git 的行为保持一致。
    raw_options.checkout_strategy |= libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
    Ok(raw_options)
}

pub(super) fn checkout_tree_preserving_locked_directories(
    repo: &Repository,
    target: &git2::Object<'_>,
    checkout: &mut CheckoutBuilder<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let raw_options = checkout_options_preserving_locked_directories(checkout)?;
        raw_git_result(unsafe {
            libgit2_sys::git_checkout_tree(repo.raw(), target.raw(), &raw_options)
        })
    }

    #[cfg(not(windows))]
    {
        repo.checkout_tree(target, Some(checkout))
    }
}

pub(super) fn checkout_index_preserving_locked_directories(
    repo: &Repository,
    index: Option<&mut git2::Index>,
    checkout: &mut CheckoutBuilder<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let raw_options = checkout_options_preserving_locked_directories(checkout)?;
        let raw_index = index.map_or(std::ptr::null_mut(), |index| index.raw());
        raw_git_result(unsafe {
            libgit2_sys::git_checkout_index(repo.raw(), raw_index, &raw_options)
        })
    }

    #[cfg(not(windows))]
    {
        repo.checkout_index(index, Some(checkout))
    }
}

pub(super) fn checkout_head_preserving_locked_directories(
    repo: &Repository,
    checkout: &mut CheckoutBuilder<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let raw_options = checkout_options_preserving_locked_directories(checkout)?;
        raw_git_result(unsafe { libgit2_sys::git_checkout_head(repo.raw(), &raw_options) })
    }

    #[cfg(not(windows))]
    {
        repo.checkout_head(Some(checkout))
    }
}

pub(super) fn merge_preserving_locked_directories(
    repo: &Repository,
    annotated_commits: &[&AnnotatedCommit<'_>],
    merge_options: Option<&mut MergeOptions>,
    checkout: &mut CheckoutBuilder<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let raw_checkout = checkout_options_preserving_locked_directories(checkout)?;
        let mut commits = annotated_commits
            .iter()
            .map(|commit| commit.raw() as *const libgit2_sys::git_annotated_commit)
            .collect::<Vec<_>>();
        raw_git_result(unsafe {
            libgit2_sys::git_merge(
                repo.raw(),
                commits.as_mut_ptr(),
                commits.len(),
                merge_options.map_or(std::ptr::null(), |options| options.raw()),
                &raw_checkout,
            )
        })
    }

    #[cfg(not(windows))]
    {
        repo.merge(annotated_commits, merge_options, Some(checkout))
    }
}

pub(super) fn reset_preserving_locked_directories(
    repo: &Repository,
    target: &git2::Object<'_>,
    kind: ResetType,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        if kind != ResetType::Hard {
            return repo.reset(target, kind, None);
        }

        let mut raw_options = unsafe { std::mem::zeroed() };
        raw_git_result(unsafe {
            libgit2_sys::git_checkout_init_options(
                &mut raw_options,
                libgit2_sys::GIT_CHECKOUT_OPTIONS_VERSION,
            )
        })?;
        raw_options.checkout_strategy |= libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe {
            libgit2_sys::git_reset(
                repo.raw(),
                target.raw(),
                libgit2_sys::GIT_RESET_HARD,
                &raw_options,
            )
        })
    }

    #[cfg(not(windows))]
    {
        repo.reset(target, kind, None)
    }
}

pub(super) fn revert_preserving_locked_directories(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    options: &mut RevertOptions<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_options = options.raw();
        raw_options.checkout_opts.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe { libgit2_sys::git_revert(repo.raw(), commit.raw(), &raw_options) })
    }

    #[cfg(not(windows))]
    {
        repo.revert(commit, Some(options))
    }
}

/// cherry-pick 的 Windows 兼容包装：与 revert 版同构（libgit2 里两者的
/// options 是同一结构），为 checkout 附加 SKIP_LOCKED_DIRECTORIES。
pub(super) fn cherrypick_preserving_locked_directories(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    options: &mut CherrypickOptions<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_options = options.raw();
        raw_options.checkout_opts.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe {
            libgit2_sys::git_cherrypick(repo.raw(), commit.raw(), &raw_options)
        })
    }

    #[cfg(not(windows))]
    {
        repo.cherrypick(commit, Some(options))
    }
}

pub(super) fn rebase_preserving_locked_directories<'repo>(
    repo: &'repo Repository,
    branch: Option<&AnnotatedCommit<'_>>,
    upstream: Option<&AnnotatedCommit<'_>>,
    onto: Option<&AnnotatedCommit<'_>>,
    options: &mut git2::RebaseOptions<'_>,
) -> std::result::Result<git2::Rebase<'repo>, git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_rebase = std::ptr::null_mut();
        let mut raw_options = unsafe { std::ptr::read(options.raw()) };
        raw_options.checkout_options.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe {
            libgit2_sys::git_rebase_init(
                &mut raw_rebase,
                repo.raw(),
                branch.map_or(std::ptr::null(), |commit| commit.raw()),
                upstream.map_or(std::ptr::null(), |commit| commit.raw()),
                onto.map_or(std::ptr::null(), |commit| commit.raw()),
                &raw_options,
            )
        })?;
        Ok(unsafe { git2::Rebase::from_raw(raw_rebase) })
    }

    #[cfg(not(windows))]
    {
        repo.rebase(branch, upstream, onto, Some(options))
    }
}

pub(super) fn open_rebase_preserving_locked_directories<'repo>(
    repo: &'repo Repository,
    options: &mut git2::RebaseOptions<'_>,
) -> std::result::Result<git2::Rebase<'repo>, git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_rebase = std::ptr::null_mut();
        let mut raw_options = unsafe { std::ptr::read(options.raw()) };
        raw_options.checkout_options.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe {
            libgit2_sys::git_rebase_open(&mut raw_rebase, repo.raw(), &raw_options)
        })?;
        Ok(unsafe { git2::Rebase::from_raw(raw_rebase) })
    }

    #[cfg(not(windows))]
    {
        repo.open_rebase(Some(options))
    }
}

pub(super) fn apply_stash_preserving_locked_directories(
    repo: &mut Repository,
    index: usize,
    options: &mut StashApplyOptions<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_options = unsafe { std::ptr::read(options.raw()) };
        raw_options.checkout_options.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe { libgit2_sys::git_stash_apply(repo.raw(), index, &raw_options) })
    }

    #[cfg(not(windows))]
    {
        repo.stash_apply(index, Some(options))
    }
}

pub(super) fn save_stash_preserving_locked_directories(
    repo: &mut Repository,
    signature: &Signature<'_>,
    message: &str,
    flags: StashFlags,
) -> std::result::Result<git2::Oid, git2::Error> {
    #[cfg(windows)]
    {
        // libgit2 的 stash save 不开放 checkout 选项。先只创建 stash，再用统一的
        // checkout 策略清理工作区，才能同样跳过被编辑器占用的空目录。
        let mut save_flags = flags;
        save_flags.insert(StashFlags::KEEP_ALL);
        let stash_oid = repo.stash_save(signature, message, Some(save_flags))?;
        let stash_commit = repo.find_commit(stash_oid)?;
        let target_oid = if flags.contains(StashFlags::KEEP_INDEX) {
            stash_commit.parent_id(1)?
        } else {
            stash_commit.parent_id(0)?
        };
        let target = repo.find_object(target_oid, None)?;
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        if flags.contains(StashFlags::INCLUDE_UNTRACKED) {
            checkout.remove_untracked(true);
        }
        if let Err(err) = checkout_tree_preserving_locked_directories(repo, &target, &mut checkout)
        {
            // stash 条目已创建成功，失败的是工作区清理：如实说明，用户重试前
            // 会知道已产生一条贮藏，避免盲目重试堆出重复贮藏。
            let short_oid = &stash_oid.to_string()[..7.min(stash_oid.to_string().len())];
            return Err(git2::Error::from_str(&format!(
                "贮藏已创建（{short_oid}），但清理工作区失败：{err}；可先处理占用后重试或手动丢弃工作区改动"
            )));
        }
        Ok(stash_oid)
    }

    #[cfg(not(windows))]
    {
        repo.stash_save(signature, message, Some(flags))
    }
}

pub(super) fn pop_stash_preserving_locked_directories(
    repo: &mut Repository,
    index: usize,
    options: &mut StashApplyOptions<'_>,
) -> std::result::Result<(), git2::Error> {
    #[cfg(windows)]
    {
        let mut raw_options = unsafe { std::ptr::read(options.raw()) };
        raw_options.checkout_options.checkout_strategy |=
            libgit2_sys::GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES as u32;
        raw_git_result(unsafe { libgit2_sys::git_stash_pop(repo.raw(), index, &raw_options) })
    }

    #[cfg(not(windows))]
    {
        repo.stash_pop(index, Some(options))
    }
}
