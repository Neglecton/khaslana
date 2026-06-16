use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use bstr::ByteSlice;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{BIG5, Encoding, GB18030, UTF_8};
use git2::build::{CheckoutBuilder, RepoBuilder};
use git2::{
    AnnotatedCommit, BranchType, Cred, CredentialType, Delta, DiffFormat, DiffOptions, ErrorCode,
    FetchOptions, FetchPrune, IndexAddOption, MergeAnalysis, MergeOptions, ProxyOptions,
    PushOptions, Reference, RemoteCallbacks, Repository, ResetType, RevertOptions, Signature, Sort,
    Status, StatusOptions,
};

use crate::credentials::{CredentialProvider, CredentialRequest, to_git_credential};
use crate::proxy::NetworkProxySettings;
use crate::types::{
    BranchInfo, BranchKind, BranchName, BranchSyncStatus, ChangeState, CloneOptions,
    CommitFileChange, CommitInfo, CommitMessage, CommitRefInfo, CommitRefKind, DiffEncodingChoice,
    DiffEncodingInfo, DiffLine, DiffLineKind, DiffScope, FileDiff, GitError, HistoryScope,
    OperationEvent, RemoteInfo, RemoteName, RepoPath, RepositorySnapshot, ResetMode, Result,
    StashInfo, TagInfo, TagName, WorktreeChange,
};
use smallvec::SmallVec;

mod browse;
mod conflicts;
mod rebase;
mod stash;
mod submodule;

// 重新导出浏览模式引用种类，供二进制 crate 使用
pub use browse::BrowseRefKind;

pub(crate) const DIFF_CONTEXT_LINES: u32 = 3;
const BRANCH_SYNC_UNPUSHED_OID_LIMIT: usize = 256;
const DIFF_ENCODING_SAMPLE_LIMIT: usize = 256 * 1024;
/// 全文差异视图使用的上下文行数：取一个足够大的安全值（远超字节预检阈值
/// `FULL_FILE_MAX_BYTES` 所允许的最大行数），让 libgit2 输出整文件作为上下文，
/// 改动行依旧按 Added/Removed 高亮，其余行作为 Context 展示。
/// 注意不能使用 `u32::MAX`：libgit2 内部对该值处理后会把上下文行数当作 0。
pub(crate) const FULL_FILE_CONTEXT_LINES: u32 = 10_000_000;
/// 全文差异视图的字节预检阈值：新旧侧文件体积超过该值则不生成全文差异，
/// 避免超大文件在分配逐行 String 时内存暴涨。UI 据此回退到紧凑差异。
pub(crate) const FULL_FILE_MAX_BYTES: u64 = 3 * 1024 * 1024;
/// 全文差异过大时返回的错误文案，UI 据此识别并回退到紧凑差异。
pub const FULL_FILE_TOO_LARGE_MESSAGE: &str = "文件过大，无法显示全文视图";

/// 全文差异的字节预检：仅在请求全文（`full_context`）时，于分配逐行 String 之前
/// 检查新旧侧文件体积，超过 `FULL_FILE_MAX_BYTES` 则直接返回错误。
fn guard_full_file_size(diff: &git2::Diff<'_>, full_context: bool) -> Result<()> {
    if !full_context {
        return Ok(());
    }
    let too_large = diff
        .deltas()
        .any(|delta| delta.old_file().size().max(delta.new_file().size()) > FULL_FILE_MAX_BYTES);
    if too_large {
        return Err(GitError::Message(FULL_FILE_TOO_LARGE_MESSAGE.into()));
    }
    Ok(())
}

/// 根据是否请求全文视图选择上下文行数。
fn diff_context_lines(full_context: bool) -> u32 {
    if full_context {
        FULL_FILE_CONTEXT_LINES
    } else {
        DIFF_CONTEXT_LINES
    }
}

pub trait ProgressEmitter: Send + Sync {
    fn emit(&self, event: OperationEvent);
}

#[derive(Clone, Default)]
pub struct NoopProgress;

impl ProgressEmitter for NoopProgress {
    fn emit(&self, _event: OperationEvent) {}
}

#[derive(Clone)]
pub struct GitService {
    credential_provider: Arc<dyn CredentialProvider>,
    progress: Arc<dyn ProgressEmitter>,
    remote_context: Arc<Mutex<Option<RemoteOperationContext>>>,
    next_remote_operation_id: Arc<AtomicU64>,
    proxy_settings: Arc<Mutex<NetworkProxySettings>>,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryRefsCache {
    pub(crate) starts: Vec<git2::Oid>,
    pub(crate) refs_by_oid: BTreeMap<git2::Oid, Vec<CommitRefInfo>>,
}

#[derive(Clone, Debug)]
struct RemoteOperationContext {
    repo_path: std::path::PathBuf,
    remote_name: String,
    // 区分同一次 libgit2 认证重试和工作流中的连续远端步骤。
    operation_id: u64,
}

pub(crate) struct RemoteContextGuard {
    context: Arc<Mutex<Option<RemoteOperationContext>>>,
}

impl Drop for RemoteContextGuard {
    fn drop(&mut self) {
        if let Ok(mut context) = self.context.lock() {
            *context = None;
        }
    }
}

impl GitService {
    pub fn new(
        credential_provider: Arc<dyn CredentialProvider>,
        progress: Arc<dyn ProgressEmitter>,
    ) -> Self {
        Self {
            credential_provider,
            progress,
            remote_context: Arc::new(Mutex::new(None)),
            next_remote_operation_id: Arc::new(AtomicU64::new(1)),
            proxy_settings: Arc::new(Mutex::new(NetworkProxySettings::default())),
        }
    }

    pub fn with_proxy_settings(self, proxy_settings: NetworkProxySettings) -> Self {
        match self.proxy_settings.lock() {
            Ok(mut current) => {
                *current = proxy_settings;
            }
            Err(_) => {
                // Mutex poisoned：说明持有该锁的线程曾 panic，逻辑状态已损坏。
                // 这里不 panic（构造期失败会直接让整个 RepositoryView 起不来），
                // 但必须留可观测信号，避免自定义代理「静默不生效」却无任何提示。
                //
                // 刻意不为此分支写单测：要构造 poisoned Mutex 必须先 panic 一次，
                // 会污染测试进程；且该分支仅发一条日志、无分支逻辑，测试价值低于
                // 引入 panic 测试的副作用成本。该分支的正确性由 match 结构保证。
                tracing::error!(
                    "proxy settings mutex poisoned; custom proxy may be inactive until restart"
                );
            }
        }
        self
    }

    pub fn open(&self, path: &RepoPath) -> Result<RepositorySnapshot> {
        let mut repo = Repository::open(&path.0)?;
        self.snapshot(&mut repo)
    }

    pub fn open_fast(&self, path: &RepoPath) -> Result<RepositorySnapshot> {
        let repo = Repository::open(&path.0)?;
        self.fast_snapshot(&repo)
    }

    pub fn clone_repo(&self, url: &str, into: &RepoPath) -> Result<RepositorySnapshot> {
        self.clone_repo_with_options(url, into, CloneOptions::default())
    }

    pub fn clone_repo_with_options(
        &self,
        url: &str,
        into: &RepoPath,
        options: CloneOptions,
    ) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started(format!("正在克隆 {url}")));

        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(self.remote_callbacks(None));
        self.apply_fetch_proxy(&mut fetch_options, Some(url))?;

        let mut checkout = CheckoutBuilder::new();
        checkout.progress(|path, current, total| {
            if let Some(path) = path {
                tracing::debug!("checkout {current}/{total}: {}", path.display());
            }
        });

        let mut repo = RepoBuilder::new()
            .fetch_options(fetch_options)
            .with_checkout(checkout)
            .clone(url, &into.0)?;

        if options.recursive_submodules {
            self.update_submodules_recursive(&repo)?;
        }

        self.progress
            .emit(OperationEvent::Finished(format!("已克隆 {url}")));
        self.snapshot(&mut repo)
    }

    pub fn snapshot(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        self.snapshot_details(repo)
    }

    pub fn snapshot_after_operation(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        let mut snapshot = self.snapshot_metadata(repo)?;
        snapshot.changes = self.status_fast(repo)?;
        Ok(snapshot)
    }

    pub fn fast_snapshot(&self, repo: &Repository) -> Result<RepositorySnapshot> {
        Ok(RepositorySnapshot {
            path: repo
                .workdir()
                .or_else(|| repo.path().parent())
                .unwrap_or_else(|| repo.path())
                .to_path_buf(),
            head: self.head_name(repo),
            branches: self.local_branches(repo)?,
            changes: Vec::new(),
            remotes: Vec::new(),
            tags: Vec::new(),
            stashes: Vec::new(),
            conflicts: Vec::new(),
            rebase_in_progress: false,
        })
    }

    pub fn snapshot_details(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        let branches_started = Instant::now();
        let branches = self.branches(repo)?;
        perf_log(
            "git.snapshot_details.branches",
            branches_started,
            format!("branches={}", branches.len()),
        );
        let status_started = Instant::now();
        let changes = self.status(repo)?;
        perf_log(
            "git.snapshot_details.status",
            status_started,
            format!("changes={}", changes.len()),
        );
        let remotes_started = Instant::now();
        let remotes = self.remotes(repo)?;
        perf_log(
            "git.snapshot_details.remotes",
            remotes_started,
            format!("remotes={}", remotes.len()),
        );
        let tags_started = Instant::now();
        let tags = self.tags(repo)?;
        perf_log(
            "git.snapshot_details.tags",
            tags_started,
            format!("tags={}", tags.len()),
        );
        let stashes_started = Instant::now();
        let stashes = self.stashes(repo)?;
        perf_log(
            "git.snapshot_details.stashes",
            stashes_started,
            format!("stashes={}", stashes.len()),
        );
        let conflicts_started = Instant::now();
        let conflicts = self.conflicts(repo)?;
        perf_log(
            "git.snapshot_details.conflicts",
            conflicts_started,
            format!("conflicts={}", conflicts.len()),
        );
        Ok(RepositorySnapshot {
            path: repo
                .workdir()
                .or_else(|| repo.path().parent())
                .unwrap_or_else(|| repo.path())
                .to_path_buf(),
            head: self.head_name(repo),
            branches,
            changes,
            remotes,
            tags,
            stashes,
            conflicts,
            rebase_in_progress: rebase_in_progress(repo),
        })
    }

    pub fn snapshot_metadata(&self, repo: &mut Repository) -> Result<RepositorySnapshot> {
        let branches_started = Instant::now();
        let branches = self.branches(repo)?;
        perf_log(
            "git.snapshot_metadata.branches",
            branches_started,
            format!("branches={}", branches.len()),
        );
        let remotes_started = Instant::now();
        let remotes = self.remotes(repo)?;
        perf_log(
            "git.snapshot_metadata.remotes",
            remotes_started,
            format!("remotes={}", remotes.len()),
        );
        let tags_started = Instant::now();
        let tags = self.tags(repo)?;
        perf_log(
            "git.snapshot_metadata.tags",
            tags_started,
            format!("tags={}", tags.len()),
        );
        let stashes_started = Instant::now();
        let stashes = self.stashes(repo)?;
        perf_log(
            "git.snapshot_metadata.stashes",
            stashes_started,
            format!("stashes={}", stashes.len()),
        );
        let conflicts_started = Instant::now();
        let conflicts = self.conflicts(repo)?;
        perf_log(
            "git.snapshot_metadata.conflicts",
            conflicts_started,
            format!("conflicts={}", conflicts.len()),
        );
        Ok(RepositorySnapshot {
            path: repo
                .workdir()
                .or_else(|| repo.path().parent())
                .unwrap_or_else(|| repo.path())
                .to_path_buf(),
            head: self.head_name(repo),
            branches,
            changes: Vec::new(),
            remotes,
            tags,
            stashes,
            conflicts,
            rebase_in_progress: rebase_in_progress(repo),
        })
    }

    pub fn current_branch(&self, repo: &Repository) -> Option<String> {
        self.head_name(repo)
    }

    pub fn branch_sync_status(
        &self,
        repo: &Repository,
        remote: &RemoteName,
    ) -> Result<Option<BranchSyncStatus>> {
        let Some(branch) = self.current_branch(repo) else {
            return Ok(None);
        };
        let Ok(local_branch) = repo.find_branch(&branch, BranchType::Local) else {
            return Ok(None);
        };
        let Some(local_oid) = local_branch.get().target() else {
            return Ok(None);
        };
        let Some((upstream, remote_oid)) =
            self.branch_sync_upstream(repo, remote, &local_branch)?
        else {
            return Ok(None);
        };

        let (ahead, behind) = repo.graph_ahead_behind(local_oid, remote_oid)?;
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        walk.push(local_oid)?;
        walk.hide(remote_oid)?;
        let mut unpushed_oids = Vec::with_capacity(ahead.min(BRANCH_SYNC_UNPUSHED_OID_LIMIT));
        for oid in walk {
            if unpushed_oids.len() >= BRANCH_SYNC_UNPUSHED_OID_LIMIT {
                break;
            }
            unpushed_oids.push(oid?.to_string());
        }

        Ok(Some(BranchSyncStatus {
            branch,
            upstream: Some(upstream),
            ahead,
            behind,
            unpushed_oids,
            unpushed_oids_truncated: ahead > BRANCH_SYNC_UNPUSHED_OID_LIMIT,
        }))
    }

    pub fn local_branches(&self, repo: &Repository) -> Result<Vec<BranchInfo>> {
        self.branches_by_type(repo, Some(BranchType::Local))
    }

    pub fn branches(&self, repo: &Repository) -> Result<Vec<BranchInfo>> {
        self.branches_by_type(repo, None)
    }

    fn branches_by_type(
        &self,
        repo: &Repository,
        branch_filter: Option<BranchType>,
    ) -> Result<Vec<BranchInfo>> {
        let mut branches = Vec::new();

        for branch in repo.branches(branch_filter)? {
            let (branch, branch_type) = branch?;
            let Some(name) = branch.name()? else {
                continue;
            };
            let upstream = if branch_type == BranchType::Local {
                branch
                    .upstream()
                    .ok()
                    .and_then(|upstream| upstream.name().ok().flatten().map(str::to_string))
            } else {
                None
            };
            branches.push(BranchInfo {
                name: name.to_string(),
                kind: match branch_type {
                    BranchType::Local => BranchKind::Local,
                    BranchType::Remote => BranchKind::Remote,
                },
                is_head: branch.is_head(),
                upstream,
            });
        }

        branches.sort_by(|a, b| {
            let kind = match (&a.kind, &b.kind) {
                (BranchKind::Local, BranchKind::Remote) => std::cmp::Ordering::Less,
                (BranchKind::Remote, BranchKind::Local) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            };
            kind.then_with(|| a.name.cmp(&b.name))
        });
        Ok(branches)
    }

    pub fn status(&self, repo: &Repository) -> Result<Vec<WorktreeChange>> {
        self.status_full(repo)
    }

    pub fn status_fast(&self, repo: &Repository) -> Result<Vec<WorktreeChange>> {
        self.status_with_options(repo, false, false)
    }

    pub fn status_full(&self, repo: &Repository) -> Result<Vec<WorktreeChange>> {
        self.status_with_options(repo, true, true)
    }

    fn status_with_options(
        &self,
        repo: &Repository,
        include_untracked: bool,
        recurse_untracked_dirs: bool,
    ) -> Result<Vec<WorktreeChange>> {
        let started = Instant::now();
        let mut options = StatusOptions::new();
        options
            .include_untracked(include_untracked)
            .recurse_untracked_dirs(recurse_untracked_dirs)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);

        let statuses = repo.statuses(Some(&mut options))?;
        let mut changes = BTreeMap::<String, WorktreeChange>::new();

        for entry in statuses.iter() {
            let path = entry.path()?.to_string();
            let status = entry.status();
            let change = changes
                .entry(path.clone())
                .or_insert_with(|| WorktreeChange {
                    path,
                    staged: None,
                    unstaged: None,
                });

            if let Some(state) = staged_state(status) {
                change.staged = Some(state);
            }
            if let Some(state) = unstaged_state(status) {
                change.unstaged = Some(state);
            }
        }

        let changes = changes.into_values().collect::<Vec<_>>();
        perf_log(
            if include_untracked {
                "git.status_full"
            } else {
                "git.status_fast"
            },
            started,
            format!(
                "changes={} include_untracked={} recurse_untracked_dirs={}",
                changes.len(),
                include_untracked,
                recurse_untracked_dirs
            ),
        );
        Ok(changes)
    }

    pub fn remotes(&self, repo: &Repository) -> Result<Vec<RemoteInfo>> {
        let remotes = repo.remotes()?;
        let mut infos = remotes.iter().try_fold(Vec::new(), |mut infos, name| {
            if let Some(name) = name? {
                let url = repo
                    .find_remote(name)
                    .ok()
                    .and_then(|remote| remote.url().ok().map(str::to_string))
                    .unwrap_or_default();
                infos.push(RemoteInfo {
                    name: name.to_string(),
                    url,
                    credential_record_id: None,
                });
            }
            Ok::<_, git2::Error>(infos)
        })?;
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(infos)
    }

    pub fn add_remote(
        &self,
        repo: &mut Repository,
        name: &RemoteName,
        url: &str,
    ) -> Result<RepositorySnapshot> {
        validate_remote_name(&name.0)?;
        validate_remote_url(url)?;
        if repo.find_remote(&name.0).is_ok() {
            return Err(GitError::Message(format!("远端名称已存在：{}", name.0)));
        }
        repo.remote(&name.0, url.trim())?;
        self.snapshot_after_operation(repo)
    }

    pub fn update_remote(
        &self,
        repo: &mut Repository,
        old_name: &RemoteName,
        new_name: &RemoteName,
        url: &str,
    ) -> Result<RepositorySnapshot> {
        validate_remote_name(&old_name.0)?;
        validate_remote_name(&new_name.0)?;
        validate_remote_url(url)?;
        if old_name.0 != new_name.0 {
            if repo.find_remote(&new_name.0).is_ok() {
                return Err(GitError::Message(format!("远端名称已存在：{}", new_name.0)));
            }
            repo.remote_rename(&old_name.0, &new_name.0)?;
        } else {
            repo.find_remote(&old_name.0)?;
        }
        repo.remote_set_url(&new_name.0, url.trim())?;
        repo.remote_set_pushurl(&new_name.0, Some(url.trim()))?;
        self.snapshot_after_operation(repo)
    }

    pub fn delete_remote(
        &self,
        repo: &mut Repository,
        name: &RemoteName,
    ) -> Result<RepositorySnapshot> {
        validate_remote_name(&name.0)?;
        repo.remote_delete(&name.0)?;
        self.snapshot_after_operation(repo)
    }

    pub fn tags(&self, repo: &Repository) -> Result<Vec<TagInfo>> {
        let tags = repo.tag_names(None)?;
        let mut tags = tags
            .iter()
            .flatten()
            .flatten()
            .map(|name| TagInfo {
                name: name.to_string(),
            })
            .collect::<Vec<_>>();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tags)
    }

    pub fn stashes(&self, repo: &mut Repository) -> Result<Vec<StashInfo>> {
        let mut stashes = Vec::new();
        repo.stash_foreach(|index, message, oid| {
            stashes.push(StashInfo {
                index,
                message: message.to_string(),
                oid: oid.to_string(),
            });
            true
        })?;
        Ok(stashes)
    }

    pub fn fetch(&self, repo: &mut Repository, remote: &RemoteName) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started(format!("正在获取 {}", remote.0)));
        self.fetch_remote_refs(repo, remote)?;
        self.progress
            .emit(OperationEvent::Finished(format!("已获取 {}", remote.0)));
        self.snapshot_after_operation(repo)
    }

    pub fn refresh(
        &self,
        repo: &mut Repository,
        remote: Option<&RemoteName>,
    ) -> Result<RepositorySnapshot> {
        if let Some(remote) = remote {
            self.progress
                .emit(OperationEvent::Started(format!("正在刷新 {}", remote.0)));
            self.fetch_remote_refs(repo, remote)?;
            self.progress
                .emit(OperationEvent::Finished(format!("已刷新 {}", remote.0)));
        }
        self.snapshot_after_operation(repo)
    }

    pub fn test_proxy(&self, repo: &Repository, remote: &RemoteName) -> Result<()> {
        let _remote_context = self.set_remote_context(repo, remote);
        let mut remote_handle = repo.find_remote(&remote.0)?;
        let remote_url = remote_fetch_url(&remote_handle);
        let proxy_options = self
            .proxy_settings()
            .proxy_options_for_remote(remote_url.as_deref())?;
        let connection = remote_handle.connect_auth(
            git2::Direction::Fetch,
            Some(self.remote_callbacks(Some(repo))),
            proxy_options,
        )?;
        connection.list()?;
        Ok(())
    }

    fn fetch_remote_refs(&self, repo: &mut Repository, remote: &RemoteName) -> Result<()> {
        let _remote_context = self.set_remote_context(repo, remote);
        let mut remote_handle = repo.find_remote(&remote.0)?;
        let mut options = FetchOptions::new();
        options.remote_callbacks(self.remote_callbacks(Some(repo)));
        let remote_url = remote_fetch_url(&remote_handle);
        self.apply_fetch_proxy(&mut options, remote_url.as_deref())?;
        // 刷新远端时同步清理已删除的远端跟踪分支，避免继续显示过期的拉取/推送状态。
        options.prune(FetchPrune::On);
        let result =
            remote_handle.fetch(&[] as &[&str], Some(&mut options), Some("khaslana fetch"));
        drop(remote_handle);
        drop(options);
        result?;
        Ok(())
    }

    pub fn pull(&self, repo: &mut Repository, remote: &RemoteName) -> Result<RepositorySnapshot> {
        self.progress
            .emit(OperationEvent::Started(format!("正在拉取 {}", remote.0)));
        self.fetch_remote_refs(repo, remote)?;

        let head = repo.head()?;
        let branch = head.shorthand().map_err(GitError::from)?.to_string();
        drop(head);

        let remote_ref = self.remote_ref_for_branch(repo, remote, &branch)?;
        let annotated = repo.reference_to_annotated_commit(&remote_ref)?;
        self.merge_annotated(repo, &annotated, &format!("{}/{}", remote.0, branch))?;
        drop(annotated);
        drop(remote_ref);

        self.progress
            .emit(OperationEvent::Finished(format!("已拉取 {}", remote.0)));
        self.snapshot_after_operation(repo)
    }

    pub fn pull_branch(
        &self,
        repo: &mut Repository,
        remote: &RemoteName,
        remote_branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&remote_branch.0)?;
        self.progress.emit(OperationEvent::Started(format!(
            "正在拉取 {}/{}",
            remote.0, remote_branch.0
        )));
        self.fetch_remote_refs(repo, remote)?;

        let remote_ref = self.remote_ref_for_remote_branch(repo, remote, &remote_branch.0)?;
        let annotated = repo.reference_to_annotated_commit(&remote_ref)?;
        self.merge_annotated(
            repo,
            &annotated,
            &format!("{}/{}", remote.0, remote_branch.0),
        )?;
        drop(annotated);
        drop(remote_ref);

        self.progress.emit(OperationEvent::Finished(format!(
            "已拉取 {}/{}",
            remote.0, remote_branch.0
        )));
        self.snapshot_after_operation(repo)
    }

    pub fn push(&self, repo: &mut Repository, remote: &RemoteName) -> Result<RepositorySnapshot> {
        let head = repo.head()?;
        let branch = head.shorthand().map_err(GitError::from)?.to_string();
        drop(head);
        self.push_branch(repo, remote, &BranchName::new(branch), true)
    }

    pub fn push_branch(
        &self,
        repo: &mut Repository,
        remote: &RemoteName,
        branch: &BranchName,
        set_upstream: bool,
    ) -> Result<RepositorySnapshot> {
        self.push_branch_to(repo, remote, branch, branch, set_upstream)
    }

    pub fn push_branch_to(
        &self,
        repo: &mut Repository,
        remote: &RemoteName,
        local_branch: &BranchName,
        remote_branch: &BranchName,
        set_upstream: bool,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&local_branch.0)?;
        validate_branch_name(&remote_branch.0)?;
        if repo
            .find_branch(&local_branch.0, BranchType::Local)
            .is_err()
        {
            return Err(GitError::Message(format!(
                "本地分支不存在：{}",
                local_branch.0
            )));
        }
        self.progress.emit(OperationEvent::Started(format!(
            "正在推送 {} 到 {}/{}",
            local_branch.0, remote.0, remote_branch.0
        )));
        let _remote_context = self.set_remote_context(repo, remote);
        let mut remote_handle = repo.find_remote(&remote.0)?;
        let mut options = PushOptions::new();
        options.remote_callbacks(self.remote_callbacks(Some(repo)));
        let remote_url = remote_push_url(&remote_handle);
        self.apply_push_proxy(&mut options, remote_url.as_deref())?;
        let refspec = format!(
            "refs/heads/{}:refs/heads/{}",
            local_branch.0, remote_branch.0
        );
        let result = remote_handle.push(&[refspec.as_str()], Some(&mut options));
        drop(remote_handle);
        drop(options);
        result?;

        if set_upstream && let Ok(mut local) = repo.find_branch(&local_branch.0, BranchType::Local)
        {
            let upstream = format!("{}/{}", remote.0, remote_branch.0);
            let _ = local.set_upstream(Some(&upstream));
        }

        self.progress.emit(OperationEvent::Finished(format!(
            "已推送 {} 到 {}/{}",
            local_branch.0, remote.0, remote_branch.0
        )));
        self.snapshot_after_operation(repo)
    }

    pub fn set_branch_upstream(
        &self,
        repo: &mut Repository,
        local_branch: &BranchName,
        remote: &RemoteName,
        remote_branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&local_branch.0)?;
        validate_remote_name(&remote.0)?;
        validate_branch_name(&remote_branch.0)?;

        let upstream = format!("{}/{}", remote.0, remote_branch.0);
        match repo.find_branch(&upstream, BranchType::Remote) {
            Ok(branch) if branch.get().target().is_some() => drop(branch),
            _ => return Err(GitError::Message(format!("远端分支不存在：{upstream}"))),
        }

        let mut branch = repo
            .find_branch(&local_branch.0, BranchType::Local)
            .map_err(|_| GitError::Message(format!("本地分支不存在：{}", local_branch.0)))?;
        branch.set_upstream(Some(&upstream))?;
        drop(branch);
        self.snapshot_after_operation(repo)
    }

    pub fn delete_remote_branch(
        &self,
        repo: &mut Repository,
        remote: &RemoteName,
        remote_branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        validate_remote_name(&remote.0)?;
        validate_branch_name(&remote_branch.0)?;
        let upstream = format!("{}/{}", remote.0, remote_branch.0);
        if repo.find_branch(&upstream, BranchType::Remote).is_err() {
            return Err(GitError::Message(format!("远端分支不存在：{upstream}")));
        }

        self.progress.emit(OperationEvent::Started(format!(
            "正在删除远端分支 {upstream}"
        )));
        let _remote_context = self.set_remote_context(repo, remote);
        let mut remote_handle = repo.find_remote(&remote.0)?;
        let mut options = PushOptions::new();
        options.remote_callbacks(self.remote_callbacks(Some(repo)));
        let remote_url = remote_push_url(&remote_handle);
        self.apply_push_proxy(&mut options, remote_url.as_deref())?;
        let refspec = format!(":refs/heads/{}", remote_branch.0);
        let result = remote_handle.push(&[refspec.as_str()], Some(&mut options));
        drop(remote_handle);
        drop(options);
        result?;
        drop(_remote_context);

        self.fetch_remote_refs(repo, remote)?;
        self.progress.emit(OperationEvent::Finished(format!(
            "已删除远端分支 {upstream}"
        )));
        self.snapshot_after_operation(repo)
    }

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
        self.progress
            .emit(OperationEvent::Finished(format!("已合并 {}", branch.0)));
        self.snapshot_after_operation(repo)
    }

    pub fn checkout_branch(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        let branch_handle = repo.find_branch(&branch.0, BranchType::Local)?;
        let reference = branch_handle.get();
        let target = reference
            .target()
            .ok_or_else(|| GitError::Message(format!("分支 {} 没有目标提交", branch.0)))?;
        let object = repo.find_object(target, None)?;

        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_tree(&object, Some(&mut checkout))?;
        let refname = reference.name().map_err(GitError::from)?;
        repo.set_head(refname)?;
        drop(object);
        drop(branch_handle);
        self.snapshot_after_operation(repo)
    }

    pub fn checkout_remote_branch(
        &self,
        repo: &mut Repository,
        remote_branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        let (remote, local_name) = remote_branch_name_parts(&remote_branch.0)?;
        validate_branch_name(local_name)?;

        let remote_branch_handle = repo.find_branch(&remote_branch.0, BranchType::Remote)?;
        let reference = remote_branch_handle.get();
        let target = reference.target().ok_or_else(|| {
            GitError::Message(format!("远端分支 {} 没有目标提交", remote_branch.0))
        })?;
        let commit = repo.find_commit(target)?;
        let upstream = format!("{remote}/{local_name}");

        if let Ok(mut local) = repo.find_branch(local_name, BranchType::Local) {
            local.set_upstream(Some(&upstream))?;
        } else {
            let mut local = repo.branch(local_name, &commit, false)?;
            local.set_upstream(Some(&upstream))?;
        }

        drop(commit);
        drop(remote_branch_handle);
        self.checkout_branch(repo, &BranchName::new(local_name))
            .map_err(|err| match err {
                GitError::Git(git_err) => GitError::Message(format!(
                    "无法切换到本地分支 {local_name}：{}",
                    git_err.message()
                )),
                other => other,
            })
    }

    pub fn checkout_tag(&self, repo: &mut Repository, tag: &TagName) -> Result<RepositorySnapshot> {
        let object = repo.revparse_single(&format!("refs/tags/{}", tag.0))?;
        let commit = object.peel_to_commit()?;
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_tree(commit.as_object(), Some(&mut checkout))?;
        repo.set_head_detached(commit.id())?;
        drop(commit);
        drop(object);
        self.snapshot_after_operation(repo)
    }

    pub fn create_branch(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        self.create_branch_from(repo, branch, None, false)
    }

    pub fn create_branch_from(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
        from: Option<&BranchName>,
        checkout: bool,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&branch.0)?;
        if repo.find_branch(&branch.0, BranchType::Local).is_ok() {
            return Err(GitError::Message(format!("分支名称已存在：{}", branch.0)));
        }
        let commit = if let Some(from) = from {
            self.find_branch_reference(repo, &from.0)?
                .peel_to_commit()?
        } else {
            repo.head()?.peel_to_commit()?
        };
        repo.branch(&branch.0, &commit, false)?;
        drop(commit);
        if checkout {
            return self.checkout_branch(repo, branch);
        }
        self.snapshot_after_operation(repo)
    }

    pub fn delete_branch(
        &self,
        repo: &mut Repository,
        branch: &BranchName,
    ) -> Result<RepositorySnapshot> {
        let mut branch_handle = repo.find_branch(&branch.0, BranchType::Local)?;
        branch_handle.delete()?;
        drop(branch_handle);
        self.snapshot_after_operation(repo)
    }

    pub fn rename_branch(
        &self,
        repo: &mut Repository,
        old: &BranchName,
        new: &BranchName,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&new.0)?;
        let mut branch = repo.find_branch(&old.0, BranchType::Local)?;
        branch.rename(&new.0, false)?;
        drop(branch);
        self.snapshot_after_operation(repo)
    }

    pub fn stage_path(&self, repo: &mut Repository, path: &Path) -> Result<RepositorySnapshot> {
        self.stage_paths(repo, [path])
    }

    pub fn stage_paths<'a, I>(&self, repo: &mut Repository, paths: I) -> Result<RepositorySnapshot>
    where
        I: IntoIterator<Item = &'a Path>,
    {
        let mut index = repo.index()?;
        for path in paths {
            if path == Path::new(".") {
                index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
            } else if repo
                .status_file(path)
                .map(|status| status.contains(Status::WT_DELETED))
                .unwrap_or(false)
            {
                index.remove_path(path)?;
            } else {
                index.add_path(path)?;
            }
        }
        index.write()?;
        self.snapshot_after_operation(repo)
    }

    pub fn unstage_path(&self, repo: &mut Repository, path: &Path) -> Result<RepositorySnapshot> {
        self.unstage_paths(repo, [path])
    }

    pub fn unstage_paths<'a, I>(
        &self,
        repo: &mut Repository,
        paths: I,
    ) -> Result<RepositorySnapshot>
    where
        I: IntoIterator<Item = &'a Path>,
    {
        // reset_default 需要 commit-ish；传 tree 会被 libgit2 再 peel 成 commit 而失败。
        let object = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let paths = paths.into_iter().collect::<Vec<_>>();
        repo.reset_default(object.as_ref().map(|commit| commit.as_object()), paths)?;
        drop(object);
        self.snapshot_after_operation(repo)
    }

    pub fn discard_unstaged_path(
        &self,
        repo: &mut Repository,
        path: &Path,
    ) -> Result<RepositorySnapshot> {
        self.discard_unstaged_paths(repo, [path])
    }

    pub fn discard_unstaged_paths<'a, I>(
        &self,
        repo: &mut Repository,
        paths: I,
    ) -> Result<RepositorySnapshot>
    where
        I: IntoIterator<Item = &'a Path>,
    {
        let paths = paths.into_iter().collect::<Vec<_>>();
        for path in &paths {
            self.ensure_path_not_conflicted(repo, path)?;
        }
        self.progress
            .emit(OperationEvent::Started("正在回滚未暂存更改".into()));

        let mut index = repo.index()?;
        for path in paths {
            let has_index_entry = index.get_path(path, 0).is_some();
            if has_index_entry {
                let mut checkout = CheckoutBuilder::new();
                checkout.force().path(path).disable_pathspec_match(true);
                repo.checkout_index(Some(&mut index), Some(&mut checkout))?;
            } else {
                remove_worktree_path(repo, path)?;
            }
        }
        drop(index);

        self.progress
            .emit(OperationEvent::Finished("已回滚未暂存更改".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn discard_all_path(
        &self,
        repo: &mut Repository,
        path: &Path,
    ) -> Result<RepositorySnapshot> {
        self.discard_all_paths(repo, [path])
    }

    pub fn discard_all_paths<'a, I>(
        &self,
        repo: &mut Repository,
        paths: I,
    ) -> Result<RepositorySnapshot>
    where
        I: IntoIterator<Item = &'a Path>,
    {
        let paths = paths.into_iter().collect::<Vec<_>>();
        for path in &paths {
            self.ensure_path_not_conflicted(repo, path)?;
        }
        self.progress
            .emit(OperationEvent::Started("正在回滚文件全部更改".into()));

        {
            let head_commit = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
            if let Some(head_commit) = head_commit {
                let head_tree = head_commit.tree()?;
                repo.reset_default(Some(head_commit.as_object()), paths.clone())?;

                for path in paths {
                    let head_has_path = head_tree.get_path(path).is_ok();
                    if head_has_path {
                        let mut checkout = CheckoutBuilder::new();
                        checkout.force().path(path).disable_pathspec_match(true);
                        repo.checkout_head(Some(&mut checkout))?;
                    } else {
                        let mut index = repo.index()?;
                        let _ = index.remove_path(path);
                        index.write()?;
                        remove_worktree_path(repo, path)?;
                    }
                }
            } else {
                let mut index = repo.index()?;
                for path in &paths {
                    if index.get_path(path, 0).is_none() {
                        return Err(GitError::Message(
                            "当前仓库还没有 HEAD，不能回滚该文件更改".into(),
                        ));
                    }
                }
                for path in paths {
                    index.remove_path(path)?;
                    remove_worktree_path(repo, path)?;
                }
                index.write()?;
            }
        }

        self.progress
            .emit(OperationEvent::Finished("已回滚文件全部更改".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn commit(
        &self,
        repo: &mut Repository,
        message: &CommitMessage,
    ) -> Result<RepositorySnapshot> {
        let message = message.0.trim();
        if message.is_empty() {
            return Err(GitError::EmptyCommitMessage);
        }

        let mut index = repo.index()?;
        if index.has_conflicts() {
            return Err(GitError::Conflicts(self.conflicts(repo)?));
        }
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let signature = signature(repo)?;
        let parent_commits = parents(repo)?;
        let parent_refs = parent_commits.iter().collect::<Vec<_>>();

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parent_refs,
        )?;

        repo.cleanup_state()?;
        drop(tree);
        drop(parent_commits);
        self.snapshot_after_operation(repo)
    }

    pub fn commit_and_push(
        &self,
        repo: &mut Repository,
        message: &CommitMessage,
        remote: &RemoteName,
    ) -> Result<std::result::Result<RepositorySnapshot, (RepositorySnapshot, GitError)>> {
        self.commit(repo, message)?;
        match self.push(repo, remote) {
            Ok(snapshot) => Ok(Ok(snapshot)),
            Err(err) => {
                let snapshot = self.snapshot_after_operation(repo)?;
                Ok(Err((snapshot, err)))
            }
        }
    }

    pub fn diff_for_path(
        &self,
        repo: &Repository,
        path: &Path,
        scope: DiffScope,
        full_context: bool,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let mut options = DiffOptions::new();
        options
            .pathspec(path)
            .include_untracked(true)
            .context_lines(diff_context_lines(full_context));

        let diff = match scope {
            DiffScope::Staged => {
                let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
                repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut options))?
            }
            DiffScope::Unstaged => repo.diff_index_to_workdir(None, Some(&mut options))?,
        };

        guard_full_file_size(&diff, full_context)?;
        self.file_diff_from_diff(diff, path_to_git(path), scope, encoding)
    }

    pub fn commit_history(
        &self,
        repo: &Repository,
        scope: HistoryScope,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        let (commits, _refs) = self.commit_history_with_refs(repo, scope, offset, limit, None)?;
        Ok(commits)
    }

    pub fn commit_history_with_refs(
        &self,
        repo: &Repository,
        scope: HistoryScope,
        offset: usize,
        limit: usize,
        refs_cache: Option<&HistoryRefsCache>,
    ) -> Result<(Vec<CommitInfo>, HistoryRefsCache)> {
        let refs_cache = match refs_cache {
            Some(cache) => cache.clone(),
            None => self.commit_graph_refs(repo)?,
        };
        match scope {
            HistoryScope::CurrentBranch => self
                .current_branch_commit_graph_with_refs(repo, offset, limit, &refs_cache)
                .map(|commits| (commits, refs_cache)),
            HistoryScope::AllRefs => self
                .commit_graph_with_refs(repo, offset, limit, &refs_cache)
                .map(|commits| (commits, refs_cache)),
        }
    }

    pub fn current_branch_commit_graph(
        &self,
        repo: &Repository,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        let refs = self.commit_graph_refs(repo)?;
        self.current_branch_commit_graph_with_refs(repo, offset, limit, &refs)
    }

    fn current_branch_commit_graph_with_refs(
        &self,
        repo: &Repository,
        offset: usize,
        limit: usize,
        refs: &HistoryRefsCache,
    ) -> Result<Vec<CommitInfo>> {
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        if let Err(err) = walk.push_head() {
            if is_empty_head_error(&err) {
                return Ok(Vec::new());
            }
            return Err(err.into());
        }
        self.collect_commit_infos(repo, walk.skip(offset).take(limit), &refs.refs_by_oid)
    }

    pub fn reset_to_commit(
        &self,
        repo: &mut Repository,
        commit_oid: &str,
        mode: ResetMode,
    ) -> Result<RepositorySnapshot> {
        if repo.head_detached()? {
            return Err(GitError::Git(git2::Error::from_str(
                "当前处于 detached HEAD，不能重置分支",
            )));
        }
        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let reset_type = match mode {
            ResetMode::Soft => ResetType::Soft,
            ResetMode::Mixed => ResetType::Mixed,
            ResetMode::Hard => ResetType::Hard,
        };
        self.progress
            .emit(OperationEvent::Started("正在重置分支".into()));
        repo.reset(commit.as_object(), reset_type, None)?;
        drop(commit);
        self.progress
            .emit(OperationEvent::Finished("分支已重置".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn uncommit_to_staged(
        &self,
        repo: &mut Repository,
        commit_oid: &str,
    ) -> Result<RepositorySnapshot> {
        if repo.head_detached()? {
            return Err(GitError::Git(git2::Error::from_str(
                "当前处于 detached HEAD，不能还原提交到暂存区",
            )));
        }

        let head = repo.head()?;
        if !head.is_branch() {
            return Err(GitError::Git(git2::Error::from_str(
                "当前 HEAD 未指向本地分支，不能还原提交到暂存区",
            )));
        }
        let Some(head_oid) = head.target() else {
            return Err(GitError::Git(git2::Error::from_str(
                "当前分支没有可还原的提交",
            )));
        };

        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        if commit.id() != head_oid {
            return Err(GitError::Git(git2::Error::from_str(
                "只能将当前最新提交还原到暂存区",
            )));
        }
        let parent_oid = match commit.parent_count() {
            0 => {
                return Err(GitError::Git(git2::Error::from_str(
                    "初始提交暂不支持还原到暂存区",
                )));
            }
            1 => commit.parent_id(0)?,
            _ => {
                return Err(GitError::Git(git2::Error::from_str(
                    "合并提交暂不支持还原到暂存区",
                )));
            }
        };

        drop(commit);
        drop(head);

        let parent = repo.find_commit(parent_oid)?;
        self.progress
            .emit(OperationEvent::Started("正在还原提交到暂存区".into()));
        repo.reset(parent.as_object(), ResetType::Soft, None)?;
        drop(parent);
        self.progress
            .emit(OperationEvent::Finished("提交已还原到暂存区".into()));
        self.snapshot_after_operation(repo)
    }

    fn ensure_clean_before_revert(&self, repo: &Repository, message: &str) -> Result<()> {
        if !self.status_full(repo)?.is_empty() || !self.conflicts(repo)?.is_empty() {
            return Err(GitError::Git(git2::Error::from_str(message)));
        }
        Ok(())
    }

    fn handle_revert_apply_error(&self, repo: &Repository, err: git2::Error) -> Result<()> {
        let conflicts = self.conflicts(repo)?;
        if !conflicts.is_empty() {
            // libgit2 的 revert 会把冲突写入仓库索引；清理 revert 状态文件后，
            // 保留索引冲突供现有冲突工作台继续处理。
            repo.cleanup_state()?;
            return Err(GitError::Conflicts(conflicts));
        }
        Err(err.into())
    }

    fn finish_revert_commit(
        &self,
        repo: &mut Repository,
        message: String,
        finished_label: &'static str,
    ) -> Result<RepositorySnapshot> {
        let mut index = repo.index()?;
        if index.has_conflicts() {
            let conflicts = self.conflicts(repo)?;
            // 成功进入冲突状态时同样清理 revert 状态文件，避免后续手动提交被状态文件干扰。
            repo.cleanup_state()?;
            return Err(GitError::Conflicts(conflicts));
        }

        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let signature = signature(repo)?;
        let head_commit = repo.head()?.peel_to_commit()?;
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &[&head_commit],
        )?;
        repo.cleanup_state()?;
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.checkout_head(Some(&mut checkout))?;
        drop(tree);
        drop(head_commit);
        self.progress
            .emit(OperationEvent::Finished(finished_label.into()));
        self.snapshot_after_operation(repo)
    }

    pub fn revert_commit(
        &self,
        repo: &mut Repository,
        commit_oid: &str,
    ) -> Result<RepositorySnapshot> {
        self.ensure_clean_before_revert(repo, "回滚提交前需要先提交、暂存或丢弃当前工作区修改")?;
        let revert_commit = self.find_commit_by_oid(repo, commit_oid)?;
        if revert_commit.parent_count() > 1 {
            return Err(GitError::Git(git2::Error::from_str("暂不支持回滚合并提交")));
        }
        let summary = revert_commit.summary().ok().flatten().unwrap_or("commit");
        let message = format!(
            "Revert \"{summary}\"\n\nThis reverts commit {}.",
            revert_commit.id()
        );
        self.progress
            .emit(OperationEvent::Started("正在回滚提交".into()));
        let mut options = RevertOptions::new();
        let revert_result = repo.revert(&revert_commit, Some(&mut options));
        drop(revert_commit);
        if let Err(err) = revert_result {
            self.handle_revert_apply_error(repo, err)?;
        }
        self.finish_revert_commit(repo, message, "回滚提交完成")
    }

    pub fn revert_merge_commit(
        &self,
        repo: &mut Repository,
        commit_oid: &str,
    ) -> Result<RepositorySnapshot> {
        self.ensure_clean_before_revert(repo, "撤销合并前需要先提交、暂存或丢弃当前工作区修改")?;
        let merge_commit = self.find_commit_by_oid(repo, commit_oid)?;
        if merge_commit.parent_count() <= 1 {
            return Err(GitError::Git(git2::Error::from_str(
                "该提交不是合并提交，请使用普通回滚提交",
            )));
        }
        let summary = merge_commit
            .summary()
            .ok()
            .flatten()
            .unwrap_or("merge commit");
        let message = format!(
            "Revert \"{summary}\"\n\nThis reverts merge commit {}, keeping the first parent side.",
            merge_commit.id()
        );
        self.progress
            .emit(OperationEvent::Started("正在撤销合并提交".into()));

        // 第一版固定使用 git revert -m 1 语义：保留合并提交的第一父提交主线侧。
        let mut options = RevertOptions::new();
        options.mainline(1);
        let revert_result = repo.revert(&merge_commit, Some(&mut options));
        drop(merge_commit);
        if let Err(err) = revert_result {
            self.handle_revert_apply_error(repo, err)?;
        }
        self.finish_revert_commit(repo, message, "撤销合并完成")
    }

    pub fn commit_graph(
        &self,
        repo: &Repository,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        let refs = self.commit_graph_refs(repo)?;
        self.commit_graph_with_refs(repo, offset, limit, &refs)
    }

    fn commit_graph_with_refs(
        &self,
        repo: &Repository,
        offset: usize,
        limit: usize,
        refs: &HistoryRefsCache,
    ) -> Result<Vec<CommitInfo>> {
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

        if refs.starts.is_empty() {
            if let Err(err) = walk.push_head() {
                if is_empty_head_error(&err) {
                    return Ok(Vec::new());
                }
                return Err(err.into());
            }
        } else {
            for oid in &refs.starts {
                walk.push(*oid)?;
            }
        }

        self.collect_commit_infos(repo, walk.skip(offset).take(limit), &refs.refs_by_oid)
    }

    fn collect_commit_infos<I>(
        &self,
        repo: &Repository,
        oids: I,
        refs_by_oid: &BTreeMap<git2::Oid, Vec<CommitRefInfo>>,
    ) -> Result<Vec<CommitInfo>>
    where
        I: IntoIterator<Item = std::result::Result<git2::Oid, git2::Error>>,
    {
        let mut commits = Vec::new();
        for oid in oids {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let author = commit.author();
            let author_name = author.name().unwrap_or("未知作者").to_string();
            let oid_string = oid.to_string();
            let parents = commit
                .parent_ids()
                .map(|parent| parent.to_string())
                .collect::<SmallVec<[String; 2]>>()
                .into_vec();
            commits.push(CommitInfo {
                oid: oid_string.clone(),
                short_oid: oid_string.chars().take(8).collect(),
                summary: commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or("(无提交信息)")
                    .to_string(),
                author: author_name,
                time: commit.time().seconds(),
                parents,
                refs: refs_by_oid.get(&oid).cloned().unwrap_or_default(),
            });
        }
        Ok(commits)
    }

    pub fn commit_history_refs(&self, repo: &Repository) -> Result<HistoryRefsCache> {
        self.commit_graph_refs(repo)
    }

    fn commit_graph_refs(&self, repo: &Repository) -> Result<HistoryRefsCache> {
        let started = Instant::now();
        let mut starts = Vec::<git2::Oid>::new();
        let mut refs_by_oid = BTreeMap::<git2::Oid, Vec<CommitRefInfo>>::new();

        let branches = match repo.branches(None) {
            Ok(branches) => Some(branches),
            Err(err) if is_empty_head_error(&err) => None,
            Err(err) => return Err(err.into()),
        };
        if let Some(branches) = branches {
            for branch in branches {
                let (branch, branch_type) = branch?;
                let Some(name) = branch.name()? else {
                    continue;
                };
                if branch_type == BranchType::Remote && name.ends_with("/HEAD") {
                    continue;
                }
                let Some(target) = branch.get().target() else {
                    continue;
                };
                if repo.find_commit(target).is_err() {
                    continue;
                }
                starts.push(target);
                refs_by_oid.entry(target).or_default().push(CommitRefInfo {
                    name: name.to_string(),
                    kind: match branch_type {
                        BranchType::Local => CommitRefKind::LocalBranch,
                        BranchType::Remote => CommitRefKind::RemoteBranch,
                    },
                });
            }
        }

        for name in repo.tag_names(None)?.iter().flatten().flatten() {
            let Ok(reference) = repo.find_reference(&format!("refs/tags/{name}")) else {
                continue;
            };
            let Ok(object) = reference.peel(git2::ObjectType::Commit) else {
                continue;
            };
            let Ok(commit) = object.into_commit() else {
                continue;
            };
            let oid = commit.id();
            refs_by_oid.entry(oid).or_default().push(CommitRefInfo {
                name: name.to_string(),
                kind: CommitRefKind::Tag,
            });
        }

        if let Ok(head) = repo.head()
            && let Ok(commit) = head.peel_to_commit()
        {
            let oid = commit.id();
            starts.push(oid);
            refs_by_oid.entry(oid).or_default().push(CommitRefInfo {
                name: "HEAD".to_string(),
                kind: CommitRefKind::Head,
            });
        }

        starts.sort();
        starts.dedup();
        for refs in refs_by_oid.values_mut() {
            refs.sort_by(|a, b| {
                ref_kind_order(&a.kind)
                    .cmp(&ref_kind_order(&b.kind))
                    .then_with(|| a.name.cmp(&b.name))
            });
            refs.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
        }

        perf_log(
            "git.history.refs",
            started,
            format!(
                "starts={} refs={}",
                starts.len(),
                refs_by_oid.values().map(Vec::len).sum::<usize>()
            ),
        );
        Ok(HistoryRefsCache {
            starts,
            refs_by_oid,
        })
    }

    pub fn commit_files(
        &self,
        repo: &Repository,
        commit_oid: &str,
    ) -> Result<Vec<CommitFileChange>> {
        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let diff = self.commit_diff(repo, &commit, None, false)?;
        let mut files = Vec::new();
        for delta in diff.deltas() {
            let Some(path) = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(path_to_git)
            else {
                continue;
            };
            let old_path = delta.old_file().path().map(path_to_git);
            files.push(CommitFileChange {
                path,
                old_path,
                status: change_state_from_delta(delta.status()),
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    pub fn commit_file_diff(
        &self,
        repo: &Repository,
        commit_oid: &str,
        path: &Path,
        full_context: bool,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let diff = self.commit_diff(repo, &commit, Some(path), full_context)?;
        guard_full_file_size(&diff, full_context)?;
        self.file_diff_from_diff(diff, path_to_git(path), DiffScope::Staged, encoding)
    }

    pub(crate) fn find_commit_by_oid<'repo>(
        &self,
        repo: &'repo Repository,
        commit_oid: &str,
    ) -> Result<git2::Commit<'repo>> {
        let oid = git2::Oid::from_str(commit_oid)
            .map_err(|err| GitError::Message(format!("提交 ID 无效：{}", err.message())))?;
        Ok(repo.find_commit(oid)?)
    }

    fn commit_diff<'repo>(
        &self,
        repo: &'repo Repository,
        commit: &git2::Commit<'repo>,
        path: Option<&Path>,
        full_context: bool,
    ) -> Result<git2::Diff<'repo>> {
        let tree = commit.tree()?;
        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };
        let mut options = DiffOptions::new();
        options.context_lines(diff_context_lines(full_context));
        if let Some(path) = path {
            options.pathspec(path);
        }
        Ok(repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?)
    }

    pub(crate) fn file_diff_from_diff(
        &self,
        diff: git2::Diff<'_>,
        path: String,
        scope: DiffScope,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let started = Instant::now();

        // 两趟遍历策略，省掉每行一次 Vec<u8> 堆分配：
        // 旧实现先把每行内容 `content.to_vec()` 存进 `RawDiffLine`（拷贝 1），
        // 再 decode 成 String（拷贝 2）。全文视图（FULL_FILE_CONTEXT_LINES 量级）
        // 下 N 行 × 2 次分配会显著拉高峰值内存。
        //
        // 新实现：
        //   第一趟 `diff.print`：只采集编码探测样本（有界 DIFF_ENCODING_SAMPLE_LIMIT）。
        //   确定编码后，第二趟 `diff.print`：直接 decode 成 String，每行只 1 次分配。
        // `git2::Diff::print` 接收 `&self`，可多次调用，每次重新遍历 deltas/hunks；
        // 多一次遍历的 CPU 成本 ≪ N 行 Vec<u8> 的内存+拷贝开销，净收益明显。

        let mut is_binary = false;
        for delta in diff.deltas() {
            if delta.flags().contains(git2::DiffFlags::BINARY) {
                is_binary = true;
            }
        }

        // 第一趟：采集编码样本。与旧实现完全一致地跳过 Header 行、按字节上限截断。
        let mut encoding_sample = Vec::new();
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let kind = match line.origin() {
                '+' => DiffLineKind::Added,
                '-' => DiffLineKind::Removed,
                'F' | 'H' => DiffLineKind::Header,
                _ => DiffLineKind::Context,
            };
            if kind != DiffLineKind::Header && encoding_sample.len() < DIFF_ENCODING_SAMPLE_LIMIT {
                let content = line.content();
                let remaining = DIFF_ENCODING_SAMPLE_LIMIT - encoding_sample.len();
                encoding_sample.extend_from_slice(&content[..content.len().min(remaining)]);
            }
            true
        })?;

        let (resolved_encoding, encoding_impl) = resolve_diff_encoding(encoding, &encoding_sample);

        // 第二趟：用确定的编码直接 decode 每行成 String，不再经过 Vec<u8> 中转。
        let mut lines = Vec::new();
        let mut lossy = false;
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let kind = match line.origin() {
                '+' => DiffLineKind::Added,
                '-' => DiffLineKind::Removed,
                'F' | 'H' => DiffLineKind::Header,
                _ => DiffLineKind::Context,
            };
            let (content, had_errors) = decode_diff_line(line.content(), encoding_impl);
            lossy |= had_errors;
            lines.push(DiffLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                content,
            });
            true
        })?;

        perf_log(
            "git.diff.decode",
            started,
            format!(
                "path={} lines={} sample_bytes={} binary={} encoding={}",
                path,
                lines.len(),
                encoding_sample.len(),
                is_binary,
                resolved_encoding.label()
            ),
        );

        Ok(FileDiff {
            path,
            scope,
            is_binary,
            encoding: DiffEncodingInfo {
                requested: encoding,
                resolved: resolved_encoding,
                lossy,
            },
            lines,
        })
    }

    fn head_name(&self, repo: &Repository) -> Option<String> {
        repo.head()
            .ok()
            .and_then(|head| head.shorthand().ok().map(str::to_string))
    }

    fn set_remote_context(&self, repo: &Repository, remote: &RemoteName) -> RemoteContextGuard {
        let repo_path = repo.path().parent().map(Path::to_path_buf);
        if let (Some(repo_path), Ok(mut context)) = (repo_path, self.remote_context.lock()) {
            let operation_id = self
                .next_remote_operation_id
                .fetch_add(1, Ordering::Relaxed);
            *context = Some(RemoteOperationContext {
                repo_path,
                remote_name: remote.0.clone(),
                operation_id,
            });
        }
        RemoteContextGuard {
            context: self.remote_context.clone(),
        }
    }

    pub(crate) fn set_submodule_context(
        &self,
        repo: &Repository,
        name: &str,
    ) -> RemoteContextGuard {
        let repo_path = repo.path().parent().map(Path::to_path_buf);
        if let (Some(repo_path), Ok(mut context)) = (repo_path, self.remote_context.lock()) {
            let operation_id = self
                .next_remote_operation_id
                .fetch_add(1, Ordering::Relaxed);
            *context = Some(RemoteOperationContext {
                repo_path,
                remote_name: name.to_string(),
                operation_id,
            });
        }
        RemoteContextGuard {
            context: self.remote_context.clone(),
        }
    }

    fn proxy_settings(&self) -> NetworkProxySettings {
        self.proxy_settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub(crate) fn apply_fetch_proxy<'a>(
        &self,
        options: &mut FetchOptions<'a>,
        remote_url: Option<&str>,
    ) -> Result<()> {
        self.proxy_settings()
            .apply_to_fetch_options(options, remote_url)
    }

    pub(crate) fn proxy_options_for_remote<'a>(
        &self,
        remote_url: Option<&str>,
    ) -> Result<Option<ProxyOptions<'a>>> {
        self.proxy_settings().proxy_options_for_remote(remote_url)
    }

    fn apply_push_proxy<'a>(
        &self,
        options: &mut PushOptions<'a>,
        remote_url: Option<&str>,
    ) -> Result<()> {
        self.proxy_settings()
            .apply_to_push_options(options, remote_url)
    }

    pub(crate) fn remote_callbacks<'a>(
        &'a self,
        repo: Option<&'a Repository>,
    ) -> RemoteCallbacks<'a> {
        let provider = self.credential_provider.clone();
        let progress = self.progress.clone();
        let config = repo.and_then(|repo| repo.config().ok());
        let remote_context = self.remote_context.clone();

        let mut callbacks = RemoteCallbacks::new();
        callbacks.transfer_progress(move |stats| {
            progress.emit(OperationEvent::Progress(format!(
                "已接收 {}/{} 个对象",
                stats.received_objects(),
                stats.total_objects()
            )));
            true
        });

        let provider_for_credentials = provider;
        callbacks.credentials(move |url, username_from_url, allowed_types| {
            let context = remote_context
                .lock()
                .ok()
                .and_then(|context| context.clone());
            credential_for_remote(
                config.as_ref(),
                provider_for_credentials.as_ref(),
                url,
                username_from_url,
                allowed_types,
                context,
            )
        });
        callbacks
    }

    fn branch_sync_upstream(
        &self,
        repo: &Repository,
        remote: &RemoteName,
        local_branch: &git2::Branch<'_>,
    ) -> Result<Option<(String, git2::Oid)>> {
        if let Ok(upstream) = local_branch.upstream() {
            let name = upstream
                .name()
                .ok()
                .flatten()
                .map(str::to_string)
                .or_else(|| upstream.get().name().ok().map(str::to_string))
                .unwrap_or_else(|| "upstream".to_string());
            if let Some(oid) = upstream.get().target() {
                return Ok(Some((name, oid)));
            }
        }

        let Some(local_name) = local_branch.name().ok().flatten() else {
            return Ok(None);
        };
        match repo.find_branch(&format!("{}/{}", remote.0, local_name), BranchType::Remote) {
            Ok(remote_branch) => {
                let name = remote_branch
                    .name()
                    .ok()
                    .flatten()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}/{}", remote.0, local_name));
                Ok(remote_branch.get().target().map(|oid| (name, oid)))
            }
            Err(err) if matches!(err.code(), ErrorCode::NotFound | ErrorCode::InvalidSpec) => {
                Ok(None)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn remote_ref_for_remote_branch<'repo>(
        &self,
        repo: &'repo Repository,
        remote: &RemoteName,
        branch: &str,
    ) -> Result<Reference<'repo>> {
        repo.find_reference(&format!("refs/remotes/{}/{}", remote.0, branch))
            .map_err(|err| {
                if matches!(err.code(), ErrorCode::NotFound | ErrorCode::InvalidSpec) {
                    GitError::Message(format!("远端分支不存在：{}/{}", remote.0, branch))
                } else {
                    GitError::from(err)
                }
            })
    }

    fn remote_ref_for_branch<'repo>(
        &self,
        repo: &'repo Repository,
        remote: &RemoteName,
        branch: &str,
    ) -> Result<Reference<'repo>> {
        if let Ok(local) = repo.find_branch(branch, BranchType::Local)
            && let Ok(upstream) = local.upstream()
        {
            return Ok(upstream.into_reference());
        }

        repo.find_reference(&format!("refs/remotes/{}/{}", remote.0, branch))
            .map_err(GitError::from)
    }

    fn find_branch_reference<'repo>(
        &self,
        repo: &'repo Repository,
        name: &str,
    ) -> Result<Reference<'repo>> {
        if let Ok(branch) = repo.find_branch(name, BranchType::Local) {
            return Ok(branch.into_reference());
        }
        if let Ok(branch) = repo.find_branch(name, BranchType::Remote) {
            return Ok(branch.into_reference());
        }
        repo.find_reference(name).map_err(GitError::from)
    }

    fn merge_annotated(
        &self,
        repo: &Repository,
        annotated: &AnnotatedCommit<'_>,
        label: &str,
    ) -> Result<()> {
        let (analysis, _preference) = repo.merge_analysis(&[annotated])?;

        if analysis.contains(MergeAnalysis::ANALYSIS_UP_TO_DATE) {
            return Ok(());
        }

        if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
            fast_forward(repo, annotated)?;
            return Ok(());
        }

        if analysis.contains(MergeAnalysis::ANALYSIS_NORMAL) {
            let head_commit = repo.head()?.peel_to_commit()?;
            let other_commit = repo.find_commit(annotated.id())?;
            let mut merge_options = MergeOptions::new();
            let mut index =
                repo.merge_commits(&head_commit, &other_commit, Some(&mut merge_options))?;

            if index.has_conflicts() {
                repo.checkout_index(
                    Some(&mut index),
                    Some(CheckoutBuilder::new().allow_conflicts(true)),
                )?;
                repo.cleanup_state()?;
                return Err(GitError::Conflicts(self.conflicts(repo)?));
            }

            let tree_id = index.write_tree_to(repo)?;
            let tree = repo.find_tree(tree_id)?;
            let signature = signature(repo)?;

            let mut repo_index = repo.index()?;
            repo_index.read_tree(&tree)?;
            repo_index.write()?;
            let mut checkout = CheckoutBuilder::new();
            checkout.safe();
            repo.checkout_index(Some(&mut repo_index), Some(&mut checkout))?;

            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                &format!("Merge branch '{label}'"),
                &tree,
                &[&head_commit, &other_commit],
            )?;
            repo.cleanup_state()?;
            return Ok(());
        }

        Err(GitError::Message(format!(
            "无法合并 {label}：不支持的合并分析结果"
        )))
    }

    fn conflicts(&self, repo: &Repository) -> Result<Vec<String>> {
        let mut conflicts = Vec::new();
        let index = repo.index()?;
        if !index.has_conflicts() {
            return Ok(conflicts);
        }

        let conflicts_iter = index.conflicts()?;
        for conflict in conflicts_iter {
            let conflict = conflict?;
            if let Some(path) = conflict
                .our
                .as_ref()
                .or(conflict.their.as_ref())
                .or(conflict.ancestor.as_ref())
                .and_then(|entry| std::str::from_utf8(&entry.path).ok())
            {
                conflicts.push(path.to_string());
            }
        }
        conflicts.sort();
        conflicts.dedup();
        Ok(conflicts)
    }

    fn ensure_path_not_conflicted(&self, repo: &Repository, path: &Path) -> Result<()> {
        let git_path = path_to_git(path);
        if self.conflicts(repo)?.iter().any(|path| path == &git_path) {
            return Err(GitError::Message(
                "该文件存在冲突，请先解决冲突后再回滚更改".into(),
            ));
        }
        Ok(())
    }
}

fn validate_branch_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.contains('\\')
        || name.starts_with('-')
        || !git2::Branch::name_is_valid(name)?
    {
        return Err(GitError::InvalidBranchName(name.to_string()));
    }
    Ok(())
}

fn validate_remote_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    let refname = format!("refs/remotes/{trimmed}/HEAD");
    if trimmed.is_empty()
        || trimmed.contains(char::is_whitespace)
        || trimmed.contains('\\')
        || trimmed.starts_with('-')
        || !git2::Reference::is_valid_name(&refname)
    {
        return Err(GitError::Message(format!("远端名称无效：{name}")));
    }
    Ok(())
}

fn validate_remote_url(url: &str) -> Result<()> {
    if url.trim().is_empty() {
        return Err(GitError::Message("远端地址不能为空".into()));
    }
    Ok(())
}

fn remote_fetch_url(remote: &git2::Remote<'_>) -> Option<String> {
    remote.url().ok().map(str::to_string)
}

fn remote_push_url(remote: &git2::Remote<'_>) -> Option<String> {
    remote
        .pushurl()
        .ok()
        .flatten()
        .or_else(|| remote.url().ok())
        .map(str::to_string)
}

fn remote_branch_name_parts(name: &str) -> Result<(&str, &str)> {
    let Some((remote, branch)) = name.split_once('/') else {
        return Err(GitError::InvalidBranchName(name.to_string()));
    };
    if remote.trim().is_empty() || branch.trim().is_empty() {
        return Err(GitError::InvalidBranchName(name.to_string()));
    }
    Ok((remote, branch))
}

fn staged_state(status: Status) -> Option<ChangeState> {
    if status.contains(Status::CONFLICTED) {
        Some(ChangeState::Conflicted)
    } else if status.contains(Status::INDEX_RENAMED) {
        Some(ChangeState::Renamed)
    } else if status.contains(Status::INDEX_TYPECHANGE) {
        Some(ChangeState::Typechange)
    } else if status.contains(Status::INDEX_NEW) {
        Some(ChangeState::Added)
    } else if status.contains(Status::INDEX_MODIFIED) {
        Some(ChangeState::Modified)
    } else if status.contains(Status::INDEX_DELETED) {
        Some(ChangeState::Deleted)
    } else {
        None
    }
}

fn unstaged_state(status: Status) -> Option<ChangeState> {
    if status.contains(Status::CONFLICTED) {
        Some(ChangeState::Conflicted)
    } else if status.contains(Status::WT_RENAMED) {
        Some(ChangeState::Renamed)
    } else if status.contains(Status::WT_TYPECHANGE) {
        Some(ChangeState::Typechange)
    } else if status.contains(Status::WT_NEW) {
        Some(ChangeState::Untracked)
    } else if status.contains(Status::WT_MODIFIED) {
        Some(ChangeState::Modified)
    } else if status.contains(Status::WT_DELETED) {
        Some(ChangeState::Deleted)
    } else {
        None
    }
}

pub(crate) fn change_state_from_delta(delta: Delta) -> ChangeState {
    match delta {
        Delta::Added => ChangeState::Added,
        Delta::Deleted => ChangeState::Deleted,
        Delta::Renamed => ChangeState::Renamed,
        Delta::Typechange => ChangeState::Typechange,
        Delta::Conflicted => ChangeState::Conflicted,
        _ => ChangeState::Modified,
    }
}

fn ref_kind_order(kind: &CommitRefKind) -> u8 {
    match kind {
        CommitRefKind::Head => 0,
        CommitRefKind::LocalBranch => 1,
        CommitRefKind::RemoteBranch => 2,
        CommitRefKind::Tag => 3,
    }
}

fn is_empty_head_error(err: &git2::Error) -> bool {
    err.code() == ErrorCode::UnbornBranch
        || err.code() == ErrorCode::NotFound
        || err.message().contains("reference 'refs/heads/")
}

fn perf_log(stage: &'static str, started: Instant, details: impl AsRef<str>) {
    if std::env::var_os("KHASLANA_PERF_LOG").is_some() {
        tracing::info!(
            target: "khaslana::perf",
            stage,
            elapsed_ms = started.elapsed().as_millis(),
            "{}",
            details.as_ref()
        );
    }
}

fn resolve_diff_encoding(
    requested: DiffEncodingChoice,
    bytes: &[u8],
) -> (DiffEncodingChoice, &'static Encoding) {
    match requested {
        DiffEncodingChoice::Auto => detect_diff_encoding(bytes),
        DiffEncodingChoice::Utf8 => (DiffEncodingChoice::Utf8, UTF_8),
        DiffEncodingChoice::Gb18030 => (DiffEncodingChoice::Gb18030, GB18030),
        DiffEncodingChoice::Big5 => (DiffEncodingChoice::Big5, BIG5),
    }
}

fn detect_diff_encoding(bytes: &[u8]) -> (DiffEncodingChoice, &'static Encoding) {
    if bytes.find_byte(0).is_some() {
        return (DiffEncodingChoice::Utf8, UTF_8);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (DiffEncodingChoice::Utf8, UTF_8);
    }

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Deny);
    if encoding == GB18030 {
        (DiffEncodingChoice::Gb18030, GB18030)
    } else if encoding == BIG5 {
        (DiffEncodingChoice::Big5, BIG5)
    } else {
        let gb18030_score = chinese_decode_score(bytes, GB18030);
        let big5_score = chinese_decode_score(bytes, BIG5);
        if gb18030_score >= big5_score && gb18030_score > 0 {
            (DiffEncodingChoice::Gb18030, GB18030)
        } else if big5_score > 0 {
            (DiffEncodingChoice::Big5, BIG5)
        } else {
            (DiffEncodingChoice::Utf8, UTF_8)
        }
    }
}

fn chinese_decode_score(bytes: &[u8], encoding: &'static Encoding) -> usize {
    let (decoded, _encoding_used, had_errors) = encoding.decode(bytes);
    if had_errors {
        return 0;
    }
    decoded
        .chars()
        .filter(|ch| {
            matches!(
                *ch as u32,
                0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
            )
        })
        .count()
}

fn decode_diff_line(bytes: &[u8], encoding: &'static Encoding) -> (String, bool) {
    let without_lf = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let trimmed = without_lf.strip_suffix(b"\r").unwrap_or(without_lf);
    let (decoded, _encoding_used, had_errors) = encoding.decode(trimmed);
    (decoded.into_owned(), had_errors)
}

pub(crate) fn signature(repo: &Repository) -> Result<Signature<'static>> {
    repo.signature()
        .or_else(|_| Signature::now("Khaslana", "khaslana@example.invalid"))
        .map_err(GitError::from)
}

/// 检测是否有变基正在进行：检查 `.git/rebase-merge` 或 `rebase-apply` 目录是否存在。
pub(crate) fn rebase_in_progress(repo: &Repository) -> bool {
    let path = repo.path();
    path.join("rebase-merge").exists() || path.join("rebase-apply").exists()
}

fn parents(repo: &Repository) -> Result<Vec<git2::Commit<'_>>> {
    if let Ok(head) = repo.head() {
        if let Ok(commit) = head.peel_to_commit() {
            return Ok(vec![commit]);
        }
    }
    Ok(Vec::new())
}

fn fast_forward(repo: &Repository, annotated: &AnnotatedCommit<'_>) -> Result<()> {
    let refname = repo.head()?.name().map_err(GitError::from)?.to_string();
    let target = repo.find_object(annotated.id(), None)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_tree(&target, Some(&mut checkout))?;

    let mut reference = repo.find_reference(&refname)?;
    reference.set_target(annotated.id(), "khaslana fast-forward")?;
    repo.set_head(&refname)?;
    Ok(())
}

fn credential_for_remote(
    _config: Option<&git2::Config>,
    provider: &dyn CredentialProvider,
    url: &str,
    username_from_url: Option<&str>,
    allowed_types: CredentialType,
    context: Option<RemoteOperationContext>,
) -> std::result::Result<Cred, git2::Error> {
    let request = CredentialRequest {
        url: url.to_string(),
        username_from_url: username_from_url.map(str::to_string),
        allowed_types,
        repo_path: context.as_ref().map(|context| context.repo_path.clone()),
        remote_name: context.as_ref().map(|context| context.remote_name.clone()),
        operation_id: context.map(|context| context.operation_id),
    };
    match provider.credential_for(request.clone()) {
        Ok(Some(credential)) => to_git_credential(&request, credential),
        Ok(None) => Err(git2::Error::from_str(&format!("访问 {url} 需要身份验证"))),
        Err(err) => Err(git2::Error::from_str(&err.to_string())),
    }
}

pub(crate) fn path_to_git(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn ensure_worktree_relative_path(path: &Path, action: &str) -> Result<()> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Ok(());
    }

    Err(GitError::Message(format!("文件路径无效，{action}")))
}

fn remove_worktree_path(repo: &Repository, path: &Path) -> Result<()> {
    ensure_worktree_relative_path(path, "不能回滚更改")?;

    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::Message("裸仓库没有工作区，不能回滚文件更改".into()))?;
    let full_path = workdir.join(path);
    let metadata = match fs::symlink_metadata(&full_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(GitError::Io(err)),
    };

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(full_path)?;
    } else {
        fs::remove_file(full_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git2::{BranchType, Oid, RepositoryInitOptions};
    use tempfile::TempDir;

    use super::*;
    use crate::credentials::PromptCredentialProvider;
    use crate::types::SubmoduleRemoteSyncStatus;

    fn service() -> GitService {
        GitService::new(
            Arc::new(PromptCredentialProvider::memory_only(|_| Ok(None))),
            Arc::new(NoopProgress),
        )
    }

    fn init_repo() -> (TempDir, Repository, GitService) {
        let dir = TempDir::new().unwrap();
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = Repository::init_opts(dir.path(), &options).unwrap();
        configure_user(&repo);
        (dir, repo, service())
    }

    fn configure_user(repo: &Repository) {
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config
            .set_str("user.email", "test@example.invalid")
            .unwrap();
    }

    fn write_file(root: &Path, path: &str, body: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, body).unwrap();
    }

    fn write_bytes(root: &Path, path: &str, body: &[u8]) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, body).unwrap();
    }

    fn assert_file_text(root: &Path, path: &str, expected: &str) {
        let actual = fs::read_to_string(root.join(path)).unwrap();
        assert_eq!(actual.replace("\r\n", "\n"), expected);
    }

    fn set_gitmodules_branch(root: &Path, submodule_path: &str, branch: &str) {
        let gitmodules_path = root.join(".gitmodules");
        let mut content = fs::read_to_string(&gitmodules_path).unwrap();
        let section = format!("[submodule \"{submodule_path}\"]");
        let start = content.find(&section).unwrap();
        let next_section = content[start + section.len()..]
            .find("\n[submodule ")
            .map(|offset| start + section.len() + offset)
            .unwrap_or(content.len());
        if content[start..next_section].contains("\n\tbranch = ") {
            let section_body = content[start..next_section].to_string();
            let replaced = section_body
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("branch = ") {
                        format!("\tbranch = {branch}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            content.replace_range(start..next_section, &replaced);
        } else {
            let insert_at = next_section;
            let prefix = if insert_at > 0 && content.as_bytes()[insert_at - 1] == b'\n' {
                ""
            } else {
                "\n"
            };
            content.insert_str(insert_at, &format!("{prefix}\tbranch = {branch}\n"));
        }
        fs::write(gitmodules_path, content).unwrap();
    }

    fn set_gitmodules_url(root: &Path, submodule_path: &str, url: &str) {
        let gitmodules_path = root.join(".gitmodules");
        let mut content = fs::read_to_string(&gitmodules_path).unwrap();
        let section = format!("[submodule \"{submodule_path}\"]");
        let start = content.find(&section).unwrap();
        let next_section = content[start + section.len()..]
            .find("\n[submodule ")
            .map(|offset| start + section.len() + offset)
            .unwrap_or(content.len());
        let section_body = content[start..next_section].to_string();
        let replaced = section_body
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("url = ") {
                    format!("\turl = {url}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        content.replace_range(start..next_section, &replaced);
        fs::write(gitmodules_path, content).unwrap();
    }

    fn commit_all(repo: &Repository, message: &str) -> Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = signature(repo).unwrap();
        let parents = parents(repo).unwrap();
        let parent_refs = parents.iter().collect::<Vec<_>>();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parent_refs,
        )
        .unwrap()
    }

    fn path_url(path: &Path) -> String {
        let normalized = path.display().to_string().replace('\\', "/");
        if cfg!(windows) {
            format!("file:///{normalized}")
        } else {
            format!("file://{normalized}")
        }
    }

    fn clone_repo_with_remote_feature()
    -> (TempDir, TempDir, std::path::PathBuf, Repository, GitService) {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), "README.md", "seed\n");
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();
        service
            .create_branch(&mut seed_repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut seed_repo, &BranchName::new("feature"))
            .unwrap();
        write_file(seed_dir.path(), "feature.txt", "feature\n");
        commit_all(&seed_repo, "feature");
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();

        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        service
            .clone_repo(&path_url(remote_dir.path()), &RepoPath::new(&clone_path))
            .unwrap();
        let clone_repo = Repository::open(&clone_path).unwrap();
        configure_user(&clone_repo);
        (remote_dir, clone_dir, clone_path, clone_repo, service)
    }

    fn create_bare_remote_with_seed(file_name: &str, body: &str) -> (TempDir, GitService) {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), file_name, body);
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();
        (remote_dir, service)
    }

    fn create_super_remote_with_submodule() -> (TempDir, TempDir, GitService) {
        let (sub_remote, service) = create_bare_remote_with_seed("sub.txt", "sub v1\n");
        let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");

        let work_dir = TempDir::new().unwrap();
        let work_path = work_dir.path().join("super-work");
        service
            .clone_repo(&path_url(super_remote.path()), &RepoPath::new(&work_path))
            .unwrap();
        let mut repo = Repository::open(&work_path).unwrap();
        configure_user(&repo);
        {
            let mut submodule = repo
                .submodule(&path_url(sub_remote.path()), Path::new("deps/sub"), true)
                .unwrap();
            submodule.clone(None).unwrap();
            submodule.add_finalize().unwrap();
        }
        commit_all(&repo, "add submodule");
        service.push(&mut repo, &RemoteName::new("origin")).unwrap();

        (super_remote, sub_remote, service)
    }

    struct SuperRemoteWithSubmodules {
        super_remote: TempDir,
        sub_remotes: Vec<TempDir>,
        service: GitService,
    }

    fn create_super_remote_with_named_submodules(
        modules: &[(&str, &str, Option<&str>)],
    ) -> SuperRemoteWithSubmodules {
        let service = service();
        let mut sub_remotes = Vec::new();
        let mut sub_urls = Vec::new();
        for (_name, initial_body, _branch) in modules {
            let remote_dir = TempDir::new().unwrap();
            let mut bare_opts = RepositoryInitOptions::new();
            bare_opts.bare(true).initial_head("main");
            Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

            let (work_dir, mut repo, _seed_service) = init_repo();
            write_file(work_dir.path(), "sub.txt", initial_body);
            commit_all(&repo, "seed submodule");
            repo.remote("origin", &path_url(remote_dir.path())).unwrap();
            service.push(&mut repo, &RemoteName::new("origin")).unwrap();
            sub_urls.push(path_url(remote_dir.path()));
            sub_remotes.push(remote_dir);
        }

        let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");
        let work_dir = TempDir::new().unwrap();
        let work_path = work_dir.path().join("super-work");
        service
            .clone_repo(&path_url(super_remote.path()), &RepoPath::new(&work_path))
            .unwrap();
        let mut repo = Repository::open(&work_path).unwrap();
        configure_user(&repo);
        for ((name, _initial_body, branch), url) in modules.iter().zip(sub_urls.iter()) {
            let path = format!("deps/{name}");
            {
                let mut submodule = repo.submodule(url, Path::new(&path), true).unwrap();
                submodule.clone(None).unwrap();
                submodule.add_finalize().unwrap();
            }
            if let Some(branch) = branch {
                set_gitmodules_branch(&work_path, &path, branch);
                let mut index = repo.index().unwrap();
                index.add_path(Path::new(".gitmodules")).unwrap();
                index.write().unwrap();
            }
        }
        commit_all(&repo, "add submodules");
        service.push(&mut repo, &RemoteName::new("origin")).unwrap();

        SuperRemoteWithSubmodules {
            super_remote,
            sub_remotes,
            service,
        }
    }

    fn create_super_remote_with_two_submodules() -> SuperRemoteWithSubmodules {
        create_super_remote_with_named_submodules(&[
            ("one", "one v1\n", None),
            ("two", "two v1\n", None),
        ])
    }

    fn clone_super_repo(
        fixture: &SuperRemoteWithSubmodules,
        recursive_submodules: bool,
    ) -> (TempDir, std::path::PathBuf, Repository) {
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        fixture
            .service
            .clone_repo_with_options(
                &path_url(fixture.super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules,
                },
            )
            .unwrap();
        let repo = Repository::open(&clone_path).unwrap();
        configure_user(&repo);
        (clone_dir, clone_path, repo)
    }

    fn advance_submodule_remote(
        remote_dir: &Path,
        service: &GitService,
        body: &str,
        branch: &str,
    ) -> Oid {
        let work_dir = TempDir::new().unwrap();
        let work_path = work_dir.path().join("sub-remote-work");
        service
            .clone_repo(&path_url(remote_dir), &RepoPath::new(&work_path))
            .unwrap();
        let mut repo = Repository::open(&work_path).unwrap();
        configure_user(&repo);
        if branch != "main" {
            service
                .create_branch(&mut repo, &BranchName::new(branch))
                .unwrap();
            service
                .checkout_branch(&mut repo, &BranchName::new(branch))
                .unwrap();
        }
        write_file(&work_path, "sub.txt", body);
        let oid = commit_all(&repo, "advance submodule");
        service
            .push_branch(
                &mut repo,
                &RemoteName::new("origin"),
                &BranchName::new(branch),
                false,
            )
            .unwrap();
        oid
    }

    fn submodule_head_oid(root: &Path, path: &str) -> Oid {
        let repo = Repository::open(root.join(path)).unwrap();
        repo.head().unwrap().target().unwrap()
    }

    fn single_submodule_remote_status(
        service: &GitService,
        repo: &Repository,
    ) -> SubmoduleRemoteSyncStatus {
        let statuses = service.submodule_remote_sync_statuses(repo).unwrap();
        assert_eq!(statuses.len(), 1);
        statuses[0].1.clone()
    }

    struct NestedSubmoduleFixture {
        super_remote: TempDir,
        _middle_remote: TempDir,
        leaf_remote: TempDir,
        service: GitService,
    }

    fn create_nested_submodule_fixture() -> NestedSubmoduleFixture {
        let service = service();
        let (leaf_remote, _leaf_service) = create_bare_remote_with_seed("leaf.txt", "leaf v1\n");
        let (middle_remote, _middle_service) =
            create_bare_remote_with_seed("middle.txt", "middle\n");

        let middle_work_dir = TempDir::new().unwrap();
        let middle_work_path = middle_work_dir.path().join("middle-work");
        service
            .clone_repo(
                &path_url(middle_remote.path()),
                &RepoPath::new(&middle_work_path),
            )
            .unwrap();
        let mut middle_repo = Repository::open(&middle_work_path).unwrap();
        configure_user(&middle_repo);
        {
            let leaf_url = path_url(leaf_remote.path());
            let mut submodule = middle_repo
                .submodule(&leaf_url, Path::new("deps/leaf"), true)
                .unwrap();
            submodule.clone(None).unwrap();
            submodule.add_finalize().unwrap();
            set_gitmodules_url(&middle_work_path, "deps/leaf", &leaf_url);
            let mut index = middle_repo.index().unwrap();
            index.add_path(Path::new(".gitmodules")).unwrap();
            index.write().unwrap();
        }
        commit_all(&middle_repo, "add nested leaf");
        service
            .push(&mut middle_repo, &RemoteName::new("origin"))
            .unwrap();

        let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");
        let super_work_dir = TempDir::new().unwrap();
        let super_work_path = super_work_dir.path().join("super-work");
        service
            .clone_repo(
                &path_url(super_remote.path()),
                &RepoPath::new(&super_work_path),
            )
            .unwrap();
        let mut super_repo = Repository::open(&super_work_path).unwrap();
        configure_user(&super_repo);
        {
            let middle_url = path_url(middle_remote.path());
            let mut submodule = super_repo
                .submodule(&middle_url, Path::new("deps/mid"), true)
                .unwrap();
            submodule.clone(None).unwrap();
            submodule.add_finalize().unwrap();
            set_gitmodules_url(&super_work_path, "deps/mid", &middle_url);
            let mut index = super_repo.index().unwrap();
            index.add_path(Path::new(".gitmodules")).unwrap();
            index.write().unwrap();
        }
        commit_all(&super_repo, "add middle submodule");
        service
            .push(&mut super_repo, &RemoteName::new("origin"))
            .unwrap();

        NestedSubmoduleFixture {
            super_remote,
            _middle_remote: middle_remote,
            leaf_remote,
            service,
        }
    }

    fn advance_remote_feature(remote_dir: &Path, service: &GitService) {
        let work_dir = TempDir::new().unwrap();
        let work_path = work_dir.path().join("remote-work");
        service
            .clone_repo(&path_url(remote_dir), &RepoPath::new(&work_path))
            .unwrap();
        let mut repo = Repository::open(&work_path).unwrap();
        configure_user(&repo);
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();
        service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();
        write_file(&work_path, "feature.txt", "feature\nremote update\n");
        commit_all(&repo, "remote feature update");
        service.push(&mut repo, &RemoteName::new("origin")).unwrap();
    }

    fn advance_remote_main(remote_dir: &Path, service: &GitService, path: &str, body: &str) -> Oid {
        let work_dir = TempDir::new().unwrap();
        let work_path = work_dir.path().join("remote-work");
        service
            .clone_repo(&path_url(remote_dir), &RepoPath::new(&work_path))
            .unwrap();
        let mut repo = Repository::open(&work_path).unwrap();
        configure_user(&repo);
        write_file(&work_path, path, body);
        let oid = commit_all(&repo, "remote main update");
        service.push(&mut repo, &RemoteName::new("origin")).unwrap();
        oid
    }

    #[test]
    fn branch_create_rename_delete() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "README.md", "hello");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        assert!(repo.find_branch("feature", BranchType::Local).is_ok());

        service
            .rename_branch(
                &mut repo,
                &BranchName::new("feature"),
                &BranchName::new("topic"),
            )
            .unwrap();
        assert!(repo.find_branch("feature", BranchType::Local).is_err());
        assert!(repo.find_branch("topic", BranchType::Local).is_ok());

        service
            .delete_branch(&mut repo, &BranchName::new("topic"))
            .unwrap();
        assert!(repo.find_branch("topic", BranchType::Local).is_err());
    }

    #[test]
    fn clone_repo_with_recursive_submodules_checks_out_submodule() {
        let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");

        service
            .clone_repo_with_options(
                &path_url(super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules: true,
                },
            )
            .unwrap();

        assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v1\n");
        let repo = Repository::open(&clone_path).unwrap();
        let submodules = service.submodules(&repo).unwrap();
        assert_eq!(submodules.len(), 1);
        assert!(submodules[0].status.is_ready());
    }

    #[test]
    fn update_submodules_initializes_non_recursive_clone() {
        let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");

        let mut repo = Repository::open(
            service
                .clone_repo_with_options(
                    &path_url(super_remote.path()),
                    &RepoPath::new(&clone_path),
                    CloneOptions {
                        recursive_submodules: false,
                    },
                )
                .unwrap()
                .path,
        )
        .unwrap();
        assert!(!clone_path.join("deps/sub/sub.txt").exists());

        service.update_submodules(&mut repo).unwrap();

        assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v1\n");
        let submodules = service.submodules(&repo).unwrap();
        assert_eq!(submodules.len(), 1);
        assert!(submodules[0].status.is_ready());
    }

    #[test]
    fn update_submodules_rejects_dirty_submodule_worktree() {
        let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        service
            .clone_repo_with_options(
                &path_url(super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules: true,
                },
            )
            .unwrap();
        let mut repo = Repository::open(&clone_path).unwrap();
        write_file(&clone_path, "deps/sub/local.txt", "local\n");

        let err = service.update_submodules(&mut repo).unwrap_err();

        assert!(err.to_string().contains("子模块"));
        assert!(err.to_string().contains("本地改动"));
    }

    #[test]
    fn open_fast_skips_submodule_scan_for_lazy_dialog_loading() {
        let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        service
            .clone_repo_with_options(
                &path_url(super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules: false,
                },
            )
            .unwrap();

        let snapshot = service.open_fast(&RepoPath::new(&clone_path)).unwrap();

        assert!(snapshot.changes.is_empty());
        let repo = Repository::open(&clone_path).unwrap();
        let submodules = service.submodules(&repo).unwrap();
        assert_eq!(submodules.len(), 1);
        assert_eq!(submodules[0].path, Path::new("deps/sub"));
    }

    #[test]
    fn update_submodules_to_remote_latest_advances_workdir_without_parent_commit() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        let target = advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "sub v2\n",
            "main",
        );

        fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap();

        let target_string = target.to_string();
        assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), target);
        assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v2\n");
        let submodules = fixture.service.submodules(&repo).unwrap();
        assert_eq!(submodules.len(), 1);
        assert_eq!(
            submodules[0].workdir_id.as_deref(),
            Some(target_string.as_str())
        );
        assert_ne!(submodules[0].index_id, submodules[0].workdir_id);
        assert_eq!(submodules[0].status.label(), "需更新");
        let changes = fixture.service.status(&repo).unwrap();
        assert!(
            changes.iter().any(|change| change.path == "deps/sub"
                && change.unstaged == Some(ChangeState::Modified))
        );
    }

    #[test]
    fn update_submodules_to_remote_latest_uses_gitmodules_branch() {
        let fixture =
            create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("develop"))]);
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        let _main_target = advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "main v2\n",
            "main",
        );
        let develop_target = advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "develop v2\n",
            "develop",
        );

        fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap();

        assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), develop_target);
        assert_file_text(&clone_path, "deps/sub/sub.txt", "develop v2\n");
    }

    #[test]
    fn update_submodules_to_remote_latest_branch_dot_uses_parent_branch() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        fixture
            .service
            .create_branch(&mut repo, &BranchName::new("release"))
            .unwrap();
        fixture
            .service
            .checkout_branch(&mut repo, &BranchName::new("release"))
            .unwrap();
        let target = advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "release v2\n",
            "release",
        );

        fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap();

        assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), target);
        assert_file_text(&clone_path, "deps/sub/sub.txt", "release v2\n");
    }

    #[test]
    fn update_submodules_to_remote_latest_branch_dot_rejects_detached_parent() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
        let (_clone_dir, _clone_path, mut repo) = clone_super_repo(&fixture, true);
        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "release v2\n",
            "release",
        );

        let err = fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap_err();

        assert!(err.to_string().contains("branch = ."));
        assert!(err.to_string().contains("父仓库当前不是本地分支"));
    }

    #[test]
    fn update_single_submodule_to_remote_latest_only_updates_named_module() {
        let fixture = create_super_remote_with_two_submodules();
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        let target_one = advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "one v2\n",
            "main",
        );
        let original_two = submodule_head_oid(&clone_path, "deps/two");
        advance_submodule_remote(
            fixture.sub_remotes[1].path(),
            &fixture.service,
            "two v2\n",
            "main",
        );

        fixture
            .service
            .update_submodule_to_remote_latest(&mut repo, "deps/one")
            .unwrap();

        assert_eq!(submodule_head_oid(&clone_path, "deps/one"), target_one);
        assert_eq!(submodule_head_oid(&clone_path, "deps/two"), original_two);
        assert_file_text(&clone_path, "deps/one/sub.txt", "one v2\n");
        assert_file_text(&clone_path, "deps/two/sub.txt", "two v1\n");
    }

    #[test]
    fn update_submodules_to_remote_latest_rejects_dirty_submodule() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "sub v2\n",
            "main",
        );
        write_file(&clone_path, "deps/sub/local.txt", "local\n");

        let err = fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap_err();

        assert!(err.to_string().contains("子模块"));
        assert!(err.to_string().contains("本地改动"));
    }

    #[test]
    fn update_submodules_to_remote_latest_rejects_non_fast_forward() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
        let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
        configure_user(&subrepo);
        write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
        commit_all(&subrepo, "local submodule commit");
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "remote v2\n",
            "main",
        );

        let err = fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap_err();

        assert!(err.to_string().contains("不能快进到远端最新"));
    }

    #[test]
    fn update_submodules_to_remote_latest_recurses_into_nested_submodules() {
        let fixture = create_nested_submodule_fixture();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        fixture
            .service
            .clone_repo_with_options(
                &path_url(fixture.super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules: true,
                },
            )
            .unwrap();
        let mut repo = Repository::open(&clone_path).unwrap();
        configure_user(&repo);
        let leaf_target = advance_submodule_remote(
            fixture.leaf_remote.path(),
            &fixture.service,
            "leaf v2\n",
            "main",
        );

        fixture
            .service
            .update_submodules_to_remote_latest(&mut repo)
            .unwrap();

        assert_eq!(
            submodule_head_oid(&clone_path, "deps/mid/deps/leaf"),
            leaf_target
        );
        assert_file_text(&clone_path, "deps/mid/deps/leaf/sub.txt", "leaf v2\n");
    }

    #[test]
    fn submodule_remote_sync_status_reports_up_to_date() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::UpToDate
        );
    }

    #[test]
    fn submodule_remote_sync_status_reports_behind() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "sub v2\n",
            "main",
        );

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Behind(1)
        );
    }

    #[test]
    fn submodule_remote_sync_status_reports_ahead() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, true);
        let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
        configure_user(&subrepo);
        write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
        commit_all(&subrepo, "local submodule commit");

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Ahead(1)
        );
    }

    #[test]
    fn submodule_remote_sync_status_reports_diverged() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, true);
        let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
        configure_user(&subrepo);
        write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
        commit_all(&subrepo, "local submodule commit");
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "remote v2\n",
            "main",
        );

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Diverged {
                ahead: 1,
                behind: 1,
            }
        );
    }

    #[test]
    fn submodule_remote_sync_status_uses_gitmodules_branch() {
        let fixture =
            create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("develop"))]);
        let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "develop v2\n",
            "develop",
        );
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "main v2\n",
            "main",
        );
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "main v3\n",
            "main",
        );

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Behind(1)
        );
    }

    #[test]
    fn submodule_remote_sync_status_branch_dot_uses_parent_branch() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
        let (_clone_dir, _clone_path, mut repo) = clone_super_repo(&fixture, true);
        fixture
            .service
            .create_branch(&mut repo, &BranchName::new("release"))
            .unwrap();
        fixture
            .service
            .checkout_branch(&mut repo, &BranchName::new("release"))
            .unwrap();
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "release v2\n",
            "release",
        );
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "main v2\n",
            "main",
        );
        advance_submodule_remote(
            fixture.sub_remotes[0].path(),
            &fixture.service,
            "main v3\n",
            "main",
        );

        assert_eq!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Behind(1)
        );
    }

    #[test]
    fn submodule_remote_sync_status_does_not_initialize_unchecked_out_submodule() {
        let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
        let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, false);
        assert!(!clone_path.join("deps/sub/sub.txt").exists());

        assert!(matches!(
            single_submodule_remote_status(&fixture.service, &repo),
            SubmoduleRemoteSyncStatus::Unavailable(_)
        ));
        assert!(!clone_path.join("deps/sub/sub.txt").exists());
    }

    #[test]
    fn stage_unstage_and_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "src/lib.rs", "pub fn value() -> i32 { 1 }\n");

        service
            .stage_path(&mut repo, Path::new("src/lib.rs"))
            .unwrap();
        let changes = service.status(&repo).unwrap();
        assert_eq!(changes[0].staged, Some(ChangeState::Added));

        service
            .unstage_path(&mut repo, Path::new("src/lib.rs"))
            .unwrap();
        let changes = service.status(&repo).unwrap();
        assert_eq!(changes[0].unstaged, Some(ChangeState::Untracked));

        service
            .stage_path(&mut repo, Path::new("src/lib.rs"))
            .unwrap();
        write_file(dir.path(), "src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let changes = service.status(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "src/lib.rs")
            .unwrap();
        assert_eq!(change.staged, Some(ChangeState::Added));
        assert_eq!(change.unstaged, Some(ChangeState::Modified));
        service
            .stage_path(&mut repo, Path::new("src/lib.rs"))
            .unwrap();
        service
            .commit(&mut repo, &CommitMessage::new("add library"))
            .unwrap();
        assert!(service.status(&repo).unwrap().is_empty());
    }

    #[test]
    fn batch_stage_and_unstage_paths() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "one.txt", "one\n");
        write_file(dir.path(), "two.txt", "two\n");

        let paths = [Path::new("one.txt"), Path::new("two.txt")];
        service.stage_paths(&mut repo, paths).unwrap();
        let changes = service.status(&repo).unwrap();
        assert!(changes.iter().any(|change| {
            change.path == "one.txt" && change.staged == Some(ChangeState::Added)
        }));
        assert!(changes.iter().any(|change| {
            change.path == "two.txt" && change.staged == Some(ChangeState::Added)
        }));

        service.unstage_paths(&mut repo, paths).unwrap();
        let changes = service.status(&repo).unwrap();
        assert!(changes.iter().any(|change| {
            change.path == "one.txt" && change.unstaged == Some(ChangeState::Untracked)
        }));
        assert!(changes.iter().any(|change| {
            change.path == "two.txt" && change.unstaged == Some(ChangeState::Untracked)
        }));
    }

    #[test]
    fn unstage_tracked_change_after_commit_keeps_worktree_change() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "initial\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "file.txt", "changed\n");

        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();
        let changes = service.status(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(change.staged, Some(ChangeState::Modified));
        assert_eq!(change.unstaged, None);

        service
            .unstage_path(&mut repo, Path::new("file.txt"))
            .unwrap();
        let changes = service.status(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(change.staged, None);
        assert_eq!(change.unstaged, Some(ChangeState::Modified));
        assert_file_text(dir.path(), "file.txt", "changed\n");
    }

    #[test]
    fn batch_stage_handles_deleted_files() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "keep.txt", "keep\n");
        write_file(dir.path(), "remove.txt", "remove\n");
        commit_all(&repo, "initial");

        fs::remove_file(dir.path().join("remove.txt")).unwrap();
        write_file(dir.path(), "keep.txt", "changed\n");

        let paths = [Path::new("keep.txt"), Path::new("remove.txt")];
        service.stage_paths(&mut repo, paths).unwrap();
        let changes = service.status(&repo).unwrap();
        assert!(changes.iter().any(|change| {
            change.path == "keep.txt" && change.staged == Some(ChangeState::Modified)
        }));
        assert!(changes.iter().any(|change| {
            change.path == "remove.txt" && change.staged == Some(ChangeState::Deleted)
        }));
    }

    #[test]
    fn discard_unstaged_keeps_staged_change() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");

        write_file(dir.path(), "file.txt", "staged\n");
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();
        write_file(dir.path(), "file.txt", "worktree\n");

        service
            .discard_unstaged_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        assert_file_text(dir.path(), "file.txt", "staged\n");
        let changes = service.status_full(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(change.staged, Some(ChangeState::Modified));
        assert_eq!(change.unstaged, None);
    }

    #[test]
    fn discard_all_removes_staged_and_unstaged_changes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");

        write_file(dir.path(), "file.txt", "staged\n");
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();
        write_file(dir.path(), "file.txt", "worktree\n");

        service
            .discard_all_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        assert_file_text(dir.path(), "file.txt", "base\n");
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_unstaged_removes_untracked_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "new.txt", "new\n");

        service
            .discard_unstaged_path(&mut repo, Path::new("new.txt"))
            .unwrap();

        assert!(!dir.path().join("new.txt").exists());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_all_removes_staged_added_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "new.txt", "new\n");
        service.stage_path(&mut repo, Path::new("new.txt")).unwrap();

        service
            .discard_all_path(&mut repo, Path::new("new.txt"))
            .unwrap();

        assert!(!dir.path().join("new.txt").exists());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_all_removes_staged_added_file_in_unborn_repo() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "new.txt", "new\n");
        service.stage_path(&mut repo, Path::new("new.txt")).unwrap();

        service
            .discard_all_path(&mut repo, Path::new("new.txt"))
            .unwrap();

        assert!(!dir.path().join("new.txt").exists());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_unstaged_restores_deleted_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");
        fs::remove_file(dir.path().join("file.txt")).unwrap();

        service
            .discard_unstaged_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        assert_file_text(dir.path(), "file.txt", "base\n");
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_all_restores_staged_deleted_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");
        fs::remove_file(dir.path().join("file.txt")).unwrap();
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        service
            .discard_all_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        assert_file_text(dir.path(), "file.txt", "base\n");
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_unstaged_paths_handles_multiple_tracked_changes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "one.txt", "one\n");
        write_file(dir.path(), "two.txt", "two\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "one.txt", "one changed\n");
        write_file(dir.path(), "two.txt", "two changed\n");

        service
            .discard_unstaged_paths(&mut repo, [Path::new("one.txt"), Path::new("two.txt")])
            .unwrap();

        assert_file_text(dir.path(), "one.txt", "one\n");
        assert_file_text(dir.path(), "two.txt", "two\n");
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_unstaged_paths_removes_multiple_untracked_files() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "one.txt", "one\n");
        write_file(dir.path(), "two.txt", "two\n");

        service
            .discard_unstaged_paths(&mut repo, [Path::new("one.txt"), Path::new("two.txt")])
            .unwrap();

        assert!(!dir.path().join("one.txt").exists());
        assert!(!dir.path().join("two.txt").exists());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_all_paths_removes_multiple_staged_changes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "modify.txt", "base\n");
        write_file(dir.path(), "delete.txt", "delete\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "modify.txt", "changed\n");
        fs::remove_file(dir.path().join("delete.txt")).unwrap();
        write_file(dir.path(), "new.txt", "new\n");
        service
            .stage_paths(
                &mut repo,
                [
                    Path::new("modify.txt"),
                    Path::new("delete.txt"),
                    Path::new("new.txt"),
                ],
            )
            .unwrap();

        service
            .discard_all_paths(
                &mut repo,
                [
                    Path::new("modify.txt"),
                    Path::new("delete.txt"),
                    Path::new("new.txt"),
                ],
            )
            .unwrap();

        assert_file_text(dir.path(), "modify.txt", "base\n");
        assert_file_text(dir.path(), "delete.txt", "delete\n");
        assert!(!dir.path().join("new.txt").exists());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_paths_respect_staged_and_unstaged_scope() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "same.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "same.txt", "staged\n");
        service
            .stage_path(&mut repo, Path::new("same.txt"))
            .unwrap();
        write_file(dir.path(), "same.txt", "worktree\n");

        service
            .discard_unstaged_paths(&mut repo, [Path::new("same.txt")])
            .unwrap();

        assert_file_text(dir.path(), "same.txt", "staged\n");
        let changes = service.status_full(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "same.txt")
            .unwrap();
        assert_eq!(change.staged, Some(ChangeState::Modified));
        assert_eq!(change.unstaged, None);

        service
            .discard_all_paths(&mut repo, [Path::new("same.txt")])
            .unwrap();

        assert_file_text(dir.path(), "same.txt", "base\n");
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn discard_rejects_conflicted_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "same.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "same.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "same.txt", "main\n");
        commit_all(&repo, "main");

        let _ = service.merge_branch(&mut repo, &BranchName::new("feature"));
        let err = service
            .discard_unstaged_path(&mut repo, Path::new("same.txt"))
            .unwrap_err();
        assert!(err.to_string().contains("存在冲突"));
    }

    #[test]
    fn discard_paths_reject_conflicts_before_touching_other_files() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "same.txt", "base\n");
        write_file(dir.path(), "safe.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "same.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "same.txt", "main\n");
        commit_all(&repo, "main");
        write_file(dir.path(), "safe.txt", "changed\n");

        let _ = service.merge_branch(&mut repo, &BranchName::new("feature"));
        let err = service
            .discard_unstaged_paths(&mut repo, [Path::new("safe.txt"), Path::new("same.txt")])
            .unwrap_err();

        assert!(err.to_string().contains("存在冲突"));
        assert_file_text(dir.path(), "safe.txt", "changed\n");
    }

    #[test]
    fn status_fast_skips_untracked_but_keeps_tracked_and_staged_changes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "tracked.txt", "one\n");
        write_file(dir.path(), "staged.txt", "one\n");
        commit_all(&repo, "initial");

        write_file(dir.path(), "tracked.txt", "one\ntwo\n");
        write_file(dir.path(), "staged.txt", "one\ntwo\n");
        service
            .stage_path(&mut repo, Path::new("staged.txt"))
            .unwrap();
        write_file(dir.path(), "untracked.txt", "new\n");

        let fast = service.status_fast(&repo).unwrap();
        assert!(fast.iter().any(|change| {
            change.path == "tracked.txt" && change.unstaged == Some(ChangeState::Modified)
        }));
        assert!(fast.iter().any(|change| {
            change.path == "staged.txt" && change.staged == Some(ChangeState::Modified)
        }));
        assert!(!fast.iter().any(|change| change.path == "untracked.txt"));

        let full = service.status_full(&repo).unwrap();
        assert!(full.iter().any(|change| {
            change.path == "untracked.txt" && change.unstaged == Some(ChangeState::Untracked)
        }));
    }

    #[test]
    fn metadata_snapshot_excludes_status_changes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "tracked.txt", "one\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "tracked.txt", "one\ntwo\n");
        write_file(dir.path(), "untracked.txt", "new\n");

        let metadata = service.snapshot_metadata(&mut repo).unwrap();
        assert_eq!(metadata.head.as_deref(), Some("main"));
        assert!(metadata.branches.iter().any(|branch| branch.name == "main"));
        assert!(metadata.changes.is_empty());

        let full = service.snapshot_details(&mut repo).unwrap();
        assert!(!full.changes.is_empty());
    }

    #[test]
    fn merge_success() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");

        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        assert!(dir.path().join("feature.txt").exists());
        assert!(service.conflicts(&repo).unwrap().is_empty());
    }

    #[test]
    fn fast_forward_merge_keeps_index_clean() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        let snapshot = service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();

        assert!(dir.path().join("feature.txt").exists());
        assert!(snapshot.changes.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn merge_conflict_detection() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "same.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "same.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "same.txt", "main\n");
        commit_all(&repo, "main");

        let err = service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap_err();
        assert!(matches!(err, GitError::Conflicts(paths) if paths == vec!["same.txt"]));
    }

    #[test]
    fn clone_fetch_push_against_local_bare_remote() {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), "README.md", "seed\n");
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();

        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");
        let snapshot = service
            .clone_repo(&path_url(remote_dir.path()), &RepoPath::new(&clone_path))
            .unwrap();
        assert_eq!(snapshot.head.as_deref(), Some("main"));

        let mut clone_repo = Repository::open(&clone_path).unwrap();
        configure_user(&clone_repo);
        write_file(&clone_path, "clone.txt", "clone\n");
        commit_all(&clone_repo, "clone");
        service
            .push(&mut clone_repo, &RemoteName::new("origin"))
            .unwrap();

        let other_dir = TempDir::new().unwrap();
        let other_path = other_dir.path().join("other");
        service
            .clone_repo(&path_url(remote_dir.path()), &RepoPath::new(&other_path))
            .unwrap();
        assert!(other_path.join("clone.txt").exists());
    }

    #[test]
    fn test_proxy_connects_to_current_remote_refs() {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), "README.md", "seed\n");
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();

        service
            .test_proxy(&seed_repo, &RemoteName::new("origin"))
            .unwrap();
    }

    #[test]
    fn commit_and_push_pushes_new_commit() {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), "README.md", "seed\n");
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();

        write_file(seed_dir.path(), "next.txt", "next\n");
        service
            .stage_path(&mut seed_repo, Path::new("next.txt"))
            .unwrap();
        let snapshot = service
            .commit_and_push(
                &mut seed_repo,
                &CommitMessage::new("next"),
                &RemoteName::new("origin"),
            )
            .unwrap()
            .unwrap();

        assert!(snapshot.changes.is_empty());
        let status = service
            .branch_sync_status(&seed_repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();
        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 0);

        let other_dir = TempDir::new().unwrap();
        let other_path = other_dir.path().join("other");
        service
            .clone_repo(&path_url(remote_dir.path()), &RepoPath::new(&other_path))
            .unwrap();
        assert!(other_path.join("next.txt").exists());
    }

    #[test]
    fn commit_and_push_keeps_local_commit_when_push_fails() {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (seed_dir, mut seed_repo, service) = init_repo();
        write_file(seed_dir.path(), "README.md", "seed\n");
        commit_all(&seed_repo, "seed");
        seed_repo
            .remote("origin", &path_url(remote_dir.path()))
            .unwrap();
        service
            .push(&mut seed_repo, &RemoteName::new("origin"))
            .unwrap();
        fs::remove_dir_all(remote_dir.path()).unwrap();

        write_file(seed_dir.path(), "local.txt", "local\n");
        service
            .stage_path(&mut seed_repo, Path::new("local.txt"))
            .unwrap();
        let result = service
            .commit_and_push(
                &mut seed_repo,
                &CommitMessage::new("local only"),
                &RemoteName::new("origin"),
            )
            .unwrap();

        let Err((snapshot, err)) = result else {
            panic!("push should fail");
        };
        assert!(err.to_string().contains("Git 错误"));
        assert!(snapshot.changes.is_empty());
        assert_eq!(
            seed_repo
                .head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .summary()
                .unwrap(),
            Some("local only")
        );
        let status = service
            .branch_sync_status(&seed_repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn open_fast_lists_only_local_branches() {
        let (_remote_dir, _clone_dir, clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        let fast = service.open_fast(&RepoPath::new(&clone_path)).unwrap();
        assert!(
            fast.branches
                .iter()
                .all(|branch| branch.kind == BranchKind::Local)
        );
        assert!(fast.branches.iter().any(|branch| branch.name == "main"));
        assert!(fast.remotes.is_empty());
        assert!(fast.changes.is_empty());
        assert!(fast.tags.is_empty());
        assert!(fast.stashes.is_empty());

        let details = service.snapshot_details(&mut repo).unwrap();
        assert!(details.remotes.iter().any(|remote| remote.name == "origin"));
        assert!(details.branches.iter().any(|branch| {
            branch.kind == BranchKind::Remote && branch.name == "origin/feature"
        }));
    }

    #[test]
    fn branch_sync_status_reports_local_ahead_commits() {
        let (_remote_dir, clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
        write_file(
            clone_dir.path().join("clone").as_path(),
            "local.txt",
            "local\n",
        );
        let oid = commit_all(&repo, "local ahead");

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();

        assert_eq!(status.branch, "main");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 0);
        assert_eq!(status.unpushed_oids, vec![oid.to_string()]);
        assert!(!status.unpushed_oids_truncated);
    }

    #[test]
    fn branch_sync_status_caps_unpushed_oid_list() {
        let (_remote_dir, clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
        for index in 0..(BRANCH_SYNC_UNPUSHED_OID_LIMIT + 1) {
            write_file(
                clone_dir.path().join("clone").as_path(),
                "local.txt",
                &format!("local {index}\n"),
            );
            commit_all(&repo, &format!("local ahead {index}"));
        }

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();

        assert_eq!(status.ahead, BRANCH_SYNC_UNPUSHED_OID_LIMIT + 1);
        assert_eq!(status.behind, 0);
        assert_eq!(status.unpushed_oids.len(), BRANCH_SYNC_UNPUSHED_OID_LIMIT);
        assert!(status.unpushed_oids_truncated);
    }

    #[test]
    fn branch_sync_status_reports_remote_behind_commits() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        advance_remote_main(remote_dir.path(), &service, "remote.txt", "remote\n");
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();

        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 1);
        assert!(status.unpushed_oids.is_empty());
        assert!(!status.unpushed_oids_truncated);
    }

    #[test]
    fn branch_sync_status_reports_diverged_branch() {
        let (remote_dir, clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        write_file(
            clone_dir.path().join("clone").as_path(),
            "local.txt",
            "local\n",
        );
        let local_oid = commit_all(&repo, "local ahead");
        advance_remote_main(remote_dir.path(), &service, "remote.txt", "remote\n");
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();

        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 1);
        assert_eq!(status.unpushed_oids, vec![local_oid.to_string()]);
        assert!(!status.unpushed_oids_truncated);
    }

    #[test]
    fn branch_sync_status_falls_back_to_remote_tracking_name() {
        let (_remote_dir, _clone_dir, _clone_path, repo, service) =
            clone_repo_with_remote_feature();
        let mut branch = repo.find_branch("main", BranchType::Local).unwrap();
        branch.set_upstream(None).unwrap();

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .unwrap();

        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn fetch_prunes_deleted_remote_branch_and_clears_sync_status() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();

        let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
        remote_repo
            .find_reference("refs/heads/feature")
            .unwrap()
            .delete()
            .unwrap();
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        assert!(
            repo.find_branch("origin/feature", BranchType::Remote)
                .is_err()
        );
        assert!(
            service
                .branch_sync_status(&repo, &RemoteName::new("origin"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn refresh_with_remote_prunes_deleted_current_upstream() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();

        let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
        remote_repo
            .find_reference("refs/heads/feature")
            .unwrap()
            .delete()
            .unwrap();
        let snapshot = service
            .refresh(&mut repo, Some(&RemoteName::new("origin")))
            .unwrap();

        assert!(!snapshot.branches.iter().any(|branch| {
            branch.kind == BranchKind::Remote && branch.name == "origin/feature"
        }));
        assert!(
            service
                .branch_sync_status(&repo, &RemoteName::new("origin"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn branch_sync_status_is_unknown_for_detached_head() {
        let (_remote_dir, _clone_dir, _clone_path, repo, service) =
            clone_repo_with_remote_feature();
        let oid = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(oid).unwrap();

        let status = service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap();

        assert!(status.is_none());
    }

    #[test]
    fn add_remote_returns_name_and_url() {
        let (dir, mut repo, service) = init_repo();
        let remote_dir = TempDir::new().unwrap();
        let snapshot = service
            .add_remote(
                &mut repo,
                &RemoteName::new("upstream"),
                &path_url(remote_dir.path()),
            )
            .unwrap();

        let remote = snapshot
            .remotes
            .iter()
            .find(|remote| remote.name == "upstream")
            .unwrap();
        assert_eq!(remote.url, path_url(remote_dir.path()));
        assert!(dir.path().join(".git").exists());
    }

    #[test]
    fn update_remote_renames_and_updates_fetch_and_push_url() {
        let (_dir, mut repo, service) = init_repo();
        let old_dir = TempDir::new().unwrap();
        let new_dir = TempDir::new().unwrap();
        service
            .add_remote(
                &mut repo,
                &RemoteName::new("origin"),
                &path_url(old_dir.path()),
            )
            .unwrap();

        let snapshot = service
            .update_remote(
                &mut repo,
                &RemoteName::new("origin"),
                &RemoteName::new("upstream"),
                &path_url(new_dir.path()),
            )
            .unwrap();

        assert!(
            snapshot.remotes.iter().any(|remote| {
                remote.name == "upstream" && remote.url == path_url(new_dir.path())
            })
        );
        assert!(
            snapshot
                .remotes
                .iter()
                .all(|remote| remote.name != "origin")
        );
        let remote = repo.find_remote("upstream").unwrap();
        assert_eq!(remote.url().unwrap(), path_url(new_dir.path()));
        assert_eq!(
            remote.pushurl().unwrap(),
            Some(path_url(new_dir.path()).as_str())
        );
    }

    #[test]
    fn delete_remote_removes_it_from_snapshot() {
        let (_dir, mut repo, service) = init_repo();
        let remote_dir = TempDir::new().unwrap();
        service
            .add_remote(
                &mut repo,
                &RemoteName::new("origin"),
                &path_url(remote_dir.path()),
            )
            .unwrap();

        let snapshot = service
            .delete_remote(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        assert!(
            snapshot
                .remotes
                .iter()
                .all(|remote| remote.name != "origin")
        );
        assert!(repo.find_remote("origin").is_err());
    }

    #[test]
    fn remote_validation_rejects_empty_url_and_duplicate_name() {
        let (_dir, mut repo, service) = init_repo();
        let remote_dir = TempDir::new().unwrap();

        assert!(
            service
                .add_remote(&mut repo, &RemoteName::new("origin"), "")
                .unwrap_err()
                .to_string()
                .contains("远端地址不能为空")
        );

        service
            .add_remote(
                &mut repo,
                &RemoteName::new("origin"),
                &path_url(remote_dir.path()),
            )
            .unwrap();
        assert!(
            service
                .add_remote(
                    &mut repo,
                    &RemoteName::new("origin"),
                    &path_url(remote_dir.path()),
                )
                .unwrap_err()
                .to_string()
                .contains("远端名称已存在")
        );
    }

    #[test]
    fn checkout_remote_branch_creates_tracks_and_switches() {
        let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();

        let snapshot = service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();

        assert_eq!(snapshot.head.as_deref(), Some("feature"));
        assert!(repo.find_branch("feature", BranchType::Local).is_ok());
        let branch = repo.find_branch("feature", BranchType::Local).unwrap();
        let upstream = branch.upstream().unwrap();
        assert_eq!(upstream.name().unwrap(), Some("origin/feature"));
        assert!(repo.workdir().unwrap().join("feature.txt").exists());
        assert!(snapshot.changes.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn checkout_remote_branch_reuses_existing_local_branch() {
        let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();
        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();

        let snapshot = service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();

        assert_eq!(snapshot.head.as_deref(), Some("feature"));
        let branch = repo.find_branch("feature", BranchType::Local).unwrap();
        let upstream = branch.upstream().unwrap();
        assert_eq!(upstream.name().unwrap(), Some("origin/feature"));
        assert!(snapshot.changes.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn set_branch_upstream_tracks_existing_remote_branch() {
        let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .create_branch(&mut repo, &BranchName::new("topic"))
            .unwrap();

        let snapshot = service
            .set_branch_upstream(
                &mut repo,
                &BranchName::new("topic"),
                &RemoteName::new("origin"),
                &BranchName::new("feature"),
            )
            .unwrap();

        let branch = repo.find_branch("topic", BranchType::Local).unwrap();
        assert_eq!(
            branch.upstream().unwrap().name().unwrap(),
            Some("origin/feature")
        );
        assert!(snapshot.branches.iter().any(|branch| {
            branch.kind == BranchKind::Local
                && branch.name == "topic"
                && branch.upstream.as_deref() == Some("origin/feature")
        }));
    }

    #[test]
    fn set_branch_upstream_rejects_missing_remote_branch() {
        let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();

        let err = service
            .set_branch_upstream(
                &mut repo,
                &BranchName::new("main"),
                &RemoteName::new("origin"),
                &BranchName::new("missing"),
            )
            .unwrap_err();

        assert!(err.to_string().contains("远端分支不存在"));
    }

    #[test]
    fn delete_remote_branch_removes_remote_ref_and_tracking_branch() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();

        let snapshot = service
            .delete_remote_branch(
                &mut repo,
                &RemoteName::new("origin"),
                &BranchName::new("feature"),
            )
            .unwrap();

        let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
        assert!(remote_repo.find_reference("refs/heads/feature").is_err());
        assert!(
            repo.find_branch("origin/feature", BranchType::Remote)
                .is_err()
        );
        assert!(!snapshot.branches.iter().any(|branch| {
            branch.kind == BranchKind::Remote && branch.name == "origin/feature"
        }));
    }

    #[test]
    fn fast_forward_pull_keeps_index_clean_after_branch_switch() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        service
            .fetch(&mut repo, &RemoteName::new("origin"))
            .unwrap();
        service
            .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
            .unwrap();

        advance_remote_feature(remote_dir.path(), &service);

        let snapshot = service.pull(&mut repo, &RemoteName::new("origin")).unwrap();

        assert_eq!(snapshot.head.as_deref(), Some("feature"));
        assert!(
            fs::read_to_string(repo.workdir().unwrap().join("feature.txt"))
                .unwrap()
                .contains("remote update")
        );
        assert!(snapshot.changes.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn pull_branch_fetches_and_merges_selected_remote_branch() {
        let (remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        advance_remote_feature(remote_dir.path(), &service);

        let snapshot = service
            .pull_branch(
                &mut repo,
                &RemoteName::new("origin"),
                &BranchName::new("feature"),
            )
            .unwrap();

        assert_eq!(snapshot.head.as_deref(), Some("main"));
        assert!(
            fs::read_to_string(repo.workdir().unwrap().join("feature.txt"))
                .unwrap()
                .contains("remote update")
        );
        assert!(snapshot.changes.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn pull_branch_reports_missing_remote_branch_in_chinese() {
        let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();

        let err = service
            .pull_branch(
                &mut repo,
                &RemoteName::new("origin"),
                &BranchName::new("missing"),
            )
            .unwrap_err();

        assert!(err.to_string().contains("远端分支不存在"));
    }

    #[test]
    fn push_branch_to_creates_different_remote_branch_and_sets_upstream() {
        let (remote_dir, clone_dir, _clone_path, mut repo, service) =
            clone_repo_with_remote_feature();
        write_file(
            clone_dir.path().join("clone").as_path(),
            "local.txt",
            "local\n",
        );
        commit_all(&repo, "local branch content");

        let snapshot = service
            .push_branch_to(
                &mut repo,
                &RemoteName::new("origin"),
                &BranchName::new("main"),
                &BranchName::new("published/main"),
                true,
            )
            .unwrap();

        assert!(snapshot.changes.is_empty());
        let branch = repo.find_branch("main", BranchType::Local).unwrap();
        assert_eq!(
            branch.upstream().unwrap().name().unwrap(),
            Some("origin/published/main")
        );
        let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
        assert!(
            remote_repo
                .find_reference("refs/heads/published/main")
                .is_ok()
        );
    }

    #[test]
    fn credential_provider_is_called_when_required() {
        struct CountingProvider(Arc<std::sync::atomic::AtomicUsize>);

        impl CredentialProvider for CountingProvider {
            fn credential_for(
                &self,
                _request: CredentialRequest,
            ) -> Result<Option<crate::credentials::GitCredential>> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            }
        }

        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let service = GitService::new(
            Arc::new(CountingProvider(count.clone())),
            Arc::new(NoopProgress),
        );
        let result = credential_for_remote(
            None,
            service.credential_provider.as_ref(),
            "https://example.invalid/repo.git",
            None,
            CredentialType::USER_PASS_PLAINTEXT,
            None,
        );
        assert!(result.is_err());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn credential_provider_credential_is_used_before_external_fallbacks() {
        struct StaticProvider;

        impl CredentialProvider for StaticProvider {
            fn credential_for(
                &self,
                request: CredentialRequest,
            ) -> Result<Option<crate::credentials::GitCredential>> {
                Ok(Some(crate::credentials::GitCredential::UserPass {
                    username: request.username_from_url.unwrap_or_else(|| "git".into()),
                    secret: "token".into(),
                    display_name: None,
                    save_to_keyring: false,
                    scope: crate::credentials::CredentialScope::RemoteUrl,
                }))
            }
        }

        let result = credential_for_remote(
            None,
            &StaticProvider,
            "https://example.invalid/repo.git",
            Some("alice"),
            CredentialType::USER_PASS_PLAINTEXT | CredentialType::DEFAULT,
            None,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn diff_for_staged_file() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "file.txt", "one\ntwo\n");
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        let diff = service
            .diff_for_path(
                &repo,
                Path::new("file.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        let added = diff
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Added && line.content.contains("two"))
            .unwrap();
        assert_eq!(added.old_lineno, None);
        assert_eq!(added.new_lineno, Some(2));
    }

    #[test]
    fn diff_uses_three_context_lines() {
        let (dir, mut repo, service) = init_repo();
        let original = (1..=12)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        write_file(dir.path(), "file.txt", &original);
        commit_all(&repo, "initial");

        let modified = (1..=12)
            .map(|line| {
                if line == 8 {
                    "line 8 changed\n".to_string()
                } else {
                    format!("line {line}\n")
                }
            })
            .collect::<String>();
        write_file(dir.path(), "file.txt", &modified);
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        let diff = service
            .diff_for_path(
                &repo,
                Path::new("file.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        let body = diff
            .lines
            .iter()
            .filter(|line| line.kind != DiffLineKind::Header)
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>();

        assert!(body.iter().any(|line| line.contains("line 5")));
        assert!(body.iter().any(|line| line.contains("line 11")));
        assert!(!body.iter().any(|line| line.contains("line 4")));
        assert!(!body.iter().any(|line| line.contains("line 12")));
    }

    #[test]
    fn diff_full_context_includes_entire_file() {
        let (dir, mut repo, service) = init_repo();
        let original = (1..=12)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        write_file(dir.path(), "file.txt", &original);
        commit_all(&repo, "initial");

        let modified = (1..=12)
            .map(|line| {
                if line == 8 {
                    "line 8 changed\n".to_string()
                } else {
                    format!("line {line}\n")
                }
            })
            .collect::<String>();
        write_file(dir.path(), "file.txt", &modified);
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();

        // 全文上下文应包含远离改动的整段未改行（紧凑 3 行上下文会排除它们）。
        let diff = service
            .diff_for_path(
                &repo,
                Path::new("file.txt"),
                DiffScope::Staged,
                true,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        let body: Vec<&str> = diff
            .lines
            .iter()
            .filter(|line| line.kind != DiffLineKind::Header)
            .map(|line| line.content.as_str())
            .collect();

        // "line 01" 这种独占前缀的断言避免与 "line 1X" 子串混淆。
        assert!(body.iter().any(|line| *line == "line 1"));
        assert!(body.iter().any(|line| *line == "line 12"));
        // 改动行依旧按 Added/Removed 高亮。
        assert!(
            diff.lines
                .iter()
                .any(|line| { line.kind == DiffLineKind::Removed && line.content == "line 8" })
        );
        assert!(
            diff.lines.iter().any(|line| {
                line.kind == DiffLineKind::Added && line.content == "line 8 changed"
            })
        );
    }

    #[test]
    fn diff_full_context_rejects_oversized_file() {
        let (dir, mut repo, service) = init_repo();
        // 生成一个超过全文阈值的文件并提交，再做一处改动后暂存。
        let big = "a".repeat((FULL_FILE_MAX_BYTES + 1024) as usize) + "\n";
        write_file(dir.path(), "big.txt", &big);
        commit_all(&repo, "initial");
        let mut modified = big.clone();
        modified.push_str("tail change\n");
        write_file(dir.path(), "big.txt", &modified);
        service.stage_path(&mut repo, Path::new("big.txt")).unwrap();

        // 全文视图应被字节预检拦截。
        let err = service
            .diff_for_path(
                &repo,
                Path::new("big.txt"),
                DiffScope::Staged,
                true,
                DiffEncodingChoice::Auto,
            )
            .unwrap_err();
        assert!(err.to_string().contains(FULL_FILE_TOO_LARGE_MESSAGE));

        // 紧凑差异在该文件上仍可正常生成。
        let compact = service
            .diff_for_path(
                &repo,
                Path::new("big.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        assert!(
            compact.lines.iter().any(
                |line| line.kind == DiffLineKind::Added && line.content.contains("tail change")
            )
        );
    }

    #[test]
    fn diff_binary_file_is_detected_and_decode_does_not_panic() {
        // 审查 #2：两趟遍历对二进制 diff 的行为锁定。
        //
        // 经验证，libgit2 的 diff_tree_to_index 路径对二进制文件并不设置
        // `DiffFlags::BINARY`，而是通过 `diff.print(Patch)` 输出一行形如
        // "Binary files a/blob.bin and b/blob.bin differ" 的**文本**提示行。
        // 因此本路径下 `FileDiff.is_binary` 为 false（这是一个已知的底层 flag
        // 与 patch 文案不一致的待办，不在本次改动范围内），但关键不变量是：
        // 第一趟采样与第二趟解码遍历的行集合一致（diff 是 &self 不可变借用），
        // 解码器对这行二进制提示文本**不会 panic**。此测试锁定该行为。
        let (dir, mut repo, service) = init_repo();
        // 用循环构造足够大的、含 NUL 字节的二进制体，确保新旧两侧都明显是二进制。
        let mut original = Vec::new();
        for _ in 0..64 {
            original.extend_from_slice(&[0u8, 1, 2, 3, 0, 255, 6, 7]);
        }
        write_bytes(dir.path(), "blob.bin", &original);
        commit_all(&repo, "add binary");
        // 改动二进制文件并暂存，制造一个二进制 diff。
        let mut modified = Vec::new();
        for _ in 0..64 {
            modified.extend_from_slice(&[0u8, 9, 9, 9, 0, 255, 8, 8]);
        }
        write_bytes(dir.path(), "blob.bin", &modified);
        service
            .stage_path(&mut repo, Path::new("blob.bin"))
            .unwrap();

        let diff = service
            .diff_for_path(
                &repo,
                Path::new("blob.bin"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        // 当前路径下 libgit2 不设 BINARY flag，锁定这一现状（防回归）。
        assert!(!diff.is_binary);
        // 解码器对二进制提示行不 panic，且能输出 "Binary files ... differ" 文本。
        // 不断言具体 index/oid 文案（libgit2 版本相关），只锁定关键词存在。
        let has_binary_hint = diff
            .lines
            .iter()
            .any(|line| line.content.contains("Binary files") && line.content.contains("differ"));
        assert!(
            has_binary_hint,
            "binary diff should emit a 'Binary files ... differ' line",
        );
    }

    #[test]
    fn diff_auto_detects_gb18030_text() {
        let (dir, mut repo, service) = init_repo();
        write_bytes(dir.path(), "cn.txt", b"hello\n");
        commit_all(&repo, "initial");
        write_bytes(dir.path(), "cn.txt", &[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
        service.stage_path(&mut repo, Path::new("cn.txt")).unwrap();

        let diff = service
            .diff_for_path(
                &repo,
                Path::new("cn.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();

        assert_eq!(diff.encoding.requested, DiffEncodingChoice::Auto);
        assert_eq!(diff.encoding.resolved, DiffEncodingChoice::Gb18030);
        assert!(
            diff.lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.content.contains("你好"))
        );
    }

    #[test]
    fn diff_auto_detection_uses_bounded_sample_for_large_diff() {
        let (dir, mut repo, service) = init_repo();
        write_bytes(dir.path(), "large-cn.txt", b"seed\n");
        commit_all(&repo, "initial");
        let mut body = Vec::new();
        for _ in 0..(DIFF_ENCODING_SAMPLE_LIMIT / 4) {
            body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
        }
        body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
        write_bytes(dir.path(), "large-cn.txt", &body);
        service
            .stage_path(&mut repo, Path::new("large-cn.txt"))
            .unwrap();

        let diff = service
            .diff_for_path(
                &repo,
                Path::new("large-cn.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();

        assert_eq!(diff.encoding.resolved, DiffEncodingChoice::Gb18030);
        assert!(diff.lines.iter().any(|line| line.content.contains("你好")));
    }

    #[test]
    fn diff_manual_big5_decodes_text() {
        let (dir, mut repo, service) = init_repo();
        write_bytes(dir.path(), "big5.txt", b"hello\n");
        commit_all(&repo, "initial");
        write_bytes(dir.path(), "big5.txt", &[0xa7, 0x41, 0xa6, 0x6e, b'\n']);
        service
            .stage_path(&mut repo, Path::new("big5.txt"))
            .unwrap();

        let utf8_diff = service
            .diff_for_path(
                &repo,
                Path::new("big5.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Utf8,
            )
            .unwrap();
        assert!(utf8_diff.encoding.lossy);

        let big5_diff = service
            .diff_for_path(
                &repo,
                Path::new("big5.txt"),
                DiffScope::Staged,
                false,
                DiffEncodingChoice::Big5,
            )
            .unwrap();

        assert_eq!(big5_diff.encoding.resolved, DiffEncodingChoice::Big5);
        assert!(
            big5_diff
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.content.contains("你好"))
        );
    }

    #[test]
    fn commit_history_pages_and_commit_diff() {
        let (dir, repo, service) = init_repo();
        write_file(dir.path(), "root.txt", "root\n");
        let root_oid = commit_all(&repo, "root commit");
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "add file");
        write_file(dir.path(), "file.txt", "one\ntwo\n");
        commit_all(&repo, "modify file");

        let first_page = service
            .commit_history(&repo, HistoryScope::CurrentBranch, 0, 2)
            .unwrap();
        assert_eq!(first_page.len(), 2);
        assert_eq!(first_page[0].summary, "modify file");
        assert_eq!(first_page[1].summary, "add file");

        let second_page = service
            .commit_history(&repo, HistoryScope::CurrentBranch, 2, 2)
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].summary, "root commit");
        assert_ne!(first_page[1].oid, second_page[0].oid);

        let files = service.commit_files(&repo, &first_page[0].oid).unwrap();
        assert!(
            files
                .iter()
                .any(|file| { file.path == "file.txt" && file.status == ChangeState::Modified })
        );
        let diff = service
            .commit_file_diff(
                &repo,
                &first_page[0].oid,
                Path::new("file.txt"),
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        assert!(diff.lines.iter().any(|line| {
            line.kind == DiffLineKind::Added
                && line.content.contains("two")
                && line.new_lineno == Some(2)
        }));

        let root_files = service.commit_files(&repo, &root_oid.to_string()).unwrap();
        assert!(
            root_files
                .iter()
                .any(|file| { file.path == "root.txt" && file.status == ChangeState::Added })
        );
        let root_diff = service
            .commit_file_diff(
                &repo,
                &root_oid.to_string(),
                Path::new("root.txt"),
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        assert!(
            root_diff
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.content.contains("root"))
        );
    }

    #[test]
    fn commit_history_scope_current_branch_excludes_other_branch_commits() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "base");
        service
            .create_branch(&mut repo, &BranchName::new("side"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("side"))
            .unwrap();
        write_file(dir.path(), "side.txt", "side\n");
        commit_all(&repo, "side only");
        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();

        let current = service
            .commit_history(&repo, HistoryScope::CurrentBranch, 0, 20)
            .unwrap();
        let all = service
            .commit_history(&repo, HistoryScope::AllRefs, 0, 20)
            .unwrap();

        assert!(!current.iter().any(|commit| commit.summary == "side only"));
        assert!(all.iter().any(|commit| commit.summary == "side only"));
    }

    #[test]
    fn commit_history_with_refs_reuses_reference_cache_for_pagination() {
        let (dir, repo, service) = init_repo();
        for index in 0..5 {
            write_file(dir.path(), "file.txt", &format!("value {index}\n"));
            commit_all(&repo, &format!("commit {index}"));
        }
        let head_oid = repo.head().unwrap().target().unwrap();
        repo.tag_lightweight(
            "v-head",
            repo.find_commit(head_oid).unwrap().as_object(),
            false,
        )
        .unwrap();

        let (first_page, refs_cache) = service
            .commit_history_with_refs(&repo, HistoryScope::CurrentBranch, 0, 2, None)
            .unwrap();
        let (second_page, reused_cache) = service
            .commit_history_with_refs(&repo, HistoryScope::CurrentBranch, 2, 2, Some(&refs_cache))
            .unwrap();
        let baseline_second_page = service
            .commit_history(&repo, HistoryScope::CurrentBranch, 2, 2)
            .unwrap();

        assert_eq!(first_page.len(), 2);
        assert_eq!(second_page, baseline_second_page);
        assert_eq!(reused_cache.starts, refs_cache.starts);
        assert_eq!(reused_cache.refs_by_oid, refs_cache.refs_by_oid);
    }

    #[test]
    fn commit_graph_lists_all_branch_reachable_commits_and_refs() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "root.txt", "root\n");
        let root_oid = commit_all(&repo, "root commit");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        let feature_oid = commit_all(&repo, "feature commit");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        let main_oid = commit_all(&repo, "main commit");

        repo.reference("refs/remotes/origin/feature", feature_oid, true, "test")
            .unwrap();
        let feature_commit = repo.find_commit(feature_oid).unwrap();
        repo.tag_lightweight("v-feature", feature_commit.as_object(), false)
            .unwrap();
        drop(feature_commit);

        let commits = service.commit_graph(&repo, 0, 20).unwrap();
        let summaries = commits
            .iter()
            .map(|commit| commit.summary.as_str())
            .collect::<Vec<_>>();
        assert!(summaries.contains(&"main commit"));
        assert!(summaries.contains(&"feature commit"));
        assert!(summaries.contains(&"root commit"));

        let feature = commits
            .iter()
            .find(|commit| commit.oid == feature_oid.to_string())
            .unwrap();
        assert!(feature.parents.contains(&root_oid.to_string()));
        assert!(feature.refs.iter().any(|reference| {
            reference.kind == CommitRefKind::LocalBranch && reference.name == "feature"
        }));
        assert!(feature.refs.iter().any(|reference| {
            reference.kind == CommitRefKind::RemoteBranch && reference.name == "origin/feature"
        }));
        assert!(feature.refs.iter().any(|reference| {
            reference.kind == CommitRefKind::Tag && reference.name == "v-feature"
        }));

        let main = commits
            .iter()
            .find(|commit| commit.oid == main_oid.to_string())
            .unwrap();
        assert!(
            main.refs
                .iter()
                .any(|reference| reference.kind == CommitRefKind::Head)
        );
    }

    #[test]
    fn commit_graph_records_merge_parents() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        let feature_oid = commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        let main_oid = commit_all(&repo, "main");

        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();

        let commits = service.commit_graph(&repo, 0, 20).unwrap();
        let merge = commits
            .iter()
            .find(|commit| commit.oid == merge_oid.to_string())
            .unwrap();
        assert_eq!(merge.parents.len(), 2);
        assert!(merge.parents.contains(&main_oid.to_string()));
        assert!(merge.parents.contains(&feature_oid.to_string()));
    }

    #[test]
    fn commit_graph_paginates_without_duplicates() {
        let (dir, repo, service) = init_repo();
        for index in 0..5 {
            write_file(dir.path(), "file.txt", &format!("{index}\n"));
            commit_all(&repo, &format!("commit {index}"));
        }

        let first_page = service.commit_graph(&repo, 0, 3).unwrap();
        let second_page = service.commit_graph(&repo, 3, 3).unwrap();
        assert_eq!(first_page.len(), 3);
        assert_eq!(second_page.len(), 2);
        let first_oids = first_page
            .iter()
            .map(|commit| commit.oid.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for commit in second_page {
            assert!(!first_oids.contains(commit.oid.as_str()));
        }
    }

    #[test]
    fn commit_graph_empty_repo_returns_empty() {
        let (_dir, repo, service) = init_repo();
        let commits = service.commit_graph(&repo, 0, 20).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn reset_to_commit_soft_keeps_changes_staged() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        commit_all(&repo, "two");

        service
            .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Soft)
            .unwrap();

        assert_eq!(repo.head().unwrap().target(), Some(first_oid));
        let changes = service.status_full(&repo).unwrap();
        let file = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(file.staged, Some(ChangeState::Modified));
        assert_eq!(file.unstaged, None);
    }

    #[test]
    fn reset_to_commit_mixed_keeps_changes_unstaged() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        commit_all(&repo, "two");

        service
            .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Mixed)
            .unwrap();

        assert_eq!(repo.head().unwrap().target(), Some(first_oid));
        let changes = service.status_full(&repo).unwrap();
        let file = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(file.staged, None);
        assert_eq!(file.unstaged, Some(ChangeState::Modified));
    }

    #[test]
    fn reset_to_commit_hard_updates_head_and_worktree() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        commit_all(&repo, "two");

        service
            .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Hard)
            .unwrap();

        assert_eq!(repo.head().unwrap().target(), Some(first_oid));
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn reset_to_commit_rejects_detached_head() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        repo.set_head_detached(first_oid).unwrap();

        let error = service
            .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Mixed)
            .unwrap_err()
            .to_string();
        assert!(error.contains("detached HEAD"));
    }

    #[test]
    fn uncommit_to_staged_soft_resets_head_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        let second_oid = commit_all(&repo, "two");

        service
            .uncommit_to_staged(&mut repo, &second_oid.to_string())
            .unwrap();

        assert_eq!(repo.head().unwrap().target(), Some(first_oid));
        let changes = service.status_full(&repo).unwrap();
        let file = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(file.staged, Some(ChangeState::Modified));
        assert_eq!(file.unstaged, None);
        assert_file_text(dir.path(), "file.txt", "two\n");
    }

    #[test]
    fn uncommit_to_staged_rejects_non_head_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        let second_oid = commit_all(&repo, "two");

        let error = service
            .uncommit_to_staged(&mut repo, &first_oid.to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("只能将当前最新提交还原到暂存区"));
        assert_eq!(repo.head().unwrap().target(), Some(second_oid));
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn uncommit_to_staged_rejects_initial_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let first_oid = commit_all(&repo, "one");

        let error = service
            .uncommit_to_staged(&mut repo, &first_oid.to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("初始提交暂不支持还原到暂存区"));
        assert_eq!(repo.head().unwrap().target(), Some(first_oid));
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn uncommit_to_staged_rejects_merge_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "shared.txt", "base\n");
        commit_all(&repo, "base");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");

        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();

        let error = service
            .uncommit_to_staged(&mut repo, &merge_oid.to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("合并提交暂不支持还原到暂存区"));
        assert_eq!(repo.head().unwrap().target(), Some(merge_oid));
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn revert_commit_creates_new_commit_and_restores_content() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        let second_oid = commit_all(&repo, "two");

        service
            .revert_commit(&mut repo, &second_oid.to_string())
            .unwrap();

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_ne!(head.id(), second_oid);
        assert!(head.summary().unwrap().unwrap().contains("Revert"));
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn revert_commit_rejects_dirty_worktree() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "one");
        write_file(dir.path(), "file.txt", "two\n");
        let second_oid = commit_all(&repo, "two");
        write_file(dir.path(), "scratch.txt", "dirty\n");

        let error = service
            .revert_commit(&mut repo, &second_oid.to_string())
            .unwrap_err()
            .to_string();
        assert!(error.contains("工作区修改"));
    }

    #[test]
    fn revert_commit_rejects_merge_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");

        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();

        let error = service
            .revert_commit(&mut repo, &merge_oid.to_string())
            .unwrap_err()
            .to_string();
        assert!(error.contains("合并提交"));
    }

    #[test]
    fn revert_merge_commit_with_first_parent_creates_revert_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");

        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();

        let snapshot = service
            .revert_merge_commit(&mut repo, &merge_oid.to_string())
            .unwrap();

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_ne!(head.id(), merge_oid);
        assert!(head.summary().unwrap().unwrap().contains("Revert"));
        assert!(!dir.path().join("feature.txt").exists());
        assert_file_text(dir.path(), "main.txt", "main\n");
        assert!(snapshot.conflicts.is_empty());
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn revert_merge_commit_rejects_non_merge_commit() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        let oid = commit_all(&repo, "one");

        let error = service
            .revert_merge_commit(&mut repo, &oid.to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("不是合并提交"));
        assert_eq!(repo.head().unwrap().target(), Some(oid));
        assert!(service.status_full(&repo).unwrap().is_empty());
    }

    #[test]
    fn revert_merge_commit_rejects_dirty_worktree() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");
        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();
        write_file(dir.path(), "scratch.txt", "dirty\n");

        let error = service
            .revert_merge_commit(&mut repo, &merge_oid.to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("工作区修改"));
        assert_eq!(repo.head().unwrap().target(), Some(merge_oid));
    }

    #[test]
    fn revert_merge_commit_reports_conflicts() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "same.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "same.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");
        service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        let merge_oid = repo.head().unwrap().target().unwrap();

        write_file(dir.path(), "same.txt", "main after merge\n");
        commit_all(&repo, "main after merge");

        let err = service
            .revert_merge_commit(&mut repo, &merge_oid.to_string())
            .unwrap_err();

        match err {
            GitError::Conflicts(paths) => assert_eq!(paths, vec!["same.txt"]),
            other => panic!("unexpected error: {other:?}"),
        }
        let snapshot = service.snapshot_after_operation(&mut repo).unwrap();
        assert_eq!(snapshot.conflicts, vec!["same.txt"]);
    }

    #[test]
    fn merge_commit_files_use_first_parent_diff() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");

        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "feature.txt", "feature\n");
        commit_all(&repo, "feature");

        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        write_file(dir.path(), "main.txt", "main\n");
        commit_all(&repo, "main");

        let snapshot = service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        assert_eq!(snapshot.head.as_deref(), Some("main"));

        let merge_oid = repo.head().unwrap().target().unwrap().to_string();
        let commit = repo
            .find_commit(git2::Oid::from_str(&merge_oid).unwrap())
            .unwrap();
        assert_eq!(commit.parent_count(), 2);

        let files = service.commit_files(&repo, &merge_oid).unwrap();
        assert!(
            files
                .iter()
                .any(|file| { file.path == "feature.txt" && file.status == ChangeState::Added })
        );
        assert!(!files.iter().any(|file| file.path == "main.txt"));
    }

    #[test]
    fn snapshot_lists_tags_and_stashes() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "initial");

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.tag_lightweight("v1.0.0", head.as_object(), false)
            .unwrap();
        drop(head);

        write_file(dir.path(), "scratch.txt", "stash me\n");
        let signature = signature(&repo).unwrap();
        repo.stash_save(
            &signature,
            "work in progress",
            Some(git2::StashFlags::INCLUDE_UNTRACKED),
        )
        .unwrap();

        let snapshot = service.snapshot(&mut repo).unwrap();
        assert!(snapshot.tags.iter().any(|tag| tag.name == "v1.0.0"));
        assert_eq!(snapshot.stashes.len(), 1);
        assert_eq!(snapshot.stashes[0].index, 0);
        assert!(snapshot.stashes[0].message.contains("work in progress"));
    }

    #[test]
    fn checkout_tag_detaches_head_and_updates_worktree() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "one\n");
        commit_all(&repo, "one");
        let first = repo.head().unwrap().peel_to_commit().unwrap();
        repo.tag_lightweight("v1", first.as_object(), false)
            .unwrap();
        drop(first);

        write_file(dir.path(), "file.txt", "two\n");
        commit_all(&repo, "two");

        service
            .checkout_tag(&mut repo, &TagName::new("v1"))
            .unwrap();

        assert!(repo.head_detached().unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
    }

    #[test]
    fn stash_apply_keeps_entry_and_pop_removes_it() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");

        write_file(dir.path(), "file.txt", "applied\n");
        let sig = signature(&repo).unwrap();
        repo.stash_save(&sig, "change file", None).unwrap();
        assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

        service.apply_stash(&mut repo, 0).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "applied\n"
        );
        assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

        let (pop_dir, mut pop_repo, pop_service) = init_repo();
        write_file(pop_dir.path(), "file.txt", "base\n");
        commit_all(&pop_repo, "initial");
        write_file(pop_dir.path(), "file.txt", "popped\n");
        let sig = signature(&pop_repo).unwrap();
        pop_repo.stash_save(&sig, "pop file", None).unwrap();

        pop_service.pop_stash(&mut pop_repo, 0).unwrap();
        assert_eq!(
            fs::read_to_string(pop_dir.path().join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "popped\n"
        );
        assert!(pop_service.stashes(&mut pop_repo).unwrap().is_empty());
    }

    #[test]
    fn save_stash_stashes_tracked_changes_and_lists_diff_files() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");

        write_file(dir.path(), "file.txt", "changed\n");
        let snapshot = service
            .save_stash(&mut repo, "tracked work", false, false)
            .unwrap();

        assert!(snapshot.changes.is_empty());
        assert_file_text(dir.path(), "file.txt", "base\n");
        assert_eq!(snapshot.stashes.len(), 1);
        assert!(snapshot.stashes[0].message.contains("tracked work"));

        let files = service
            .stash_files(&repo, &snapshot.stashes[0].oid)
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "file.txt");
        assert_eq!(files[0].status, ChangeState::Modified);

        let diff = service
            .stash_file_diff(
                &repo,
                &snapshot.stashes[0].oid,
                Path::new("file.txt"),
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        assert!(
            diff.lines
                .iter()
                .any(|line| line.content.contains("changed"))
        );
    }

    #[test]
    fn save_stash_include_untracked_records_untracked_files() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "base.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "new.txt", "new\n");

        let err = service
            .save_stash(&mut repo, "skip untracked", false, false)
            .unwrap_err();
        assert!(err.to_string().contains("包含未跟踪文件"));
        assert!(dir.path().join("new.txt").exists());

        let snapshot = service
            .save_stash(&mut repo, "with untracked", true, false)
            .unwrap();
        assert!(!dir.path().join("new.txt").exists());
        let files = service
            .stash_files(&repo, &snapshot.stashes[0].oid)
            .unwrap();
        assert!(
            files
                .iter()
                .any(|file| file.path == "new.txt" && file.status == ChangeState::Untracked)
        );

        let diff = service
            .stash_file_diff(
                &repo,
                &snapshot.stashes[0].oid,
                Path::new("new.txt"),
                false,
                DiffEncodingChoice::Auto,
            )
            .unwrap();
        assert!(diff.lines.iter().any(|line| line.content.contains("new")));
    }

    #[test]
    fn save_stash_keep_index_preserves_staged_content() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "file.txt", "staged\n");
        service
            .stage_path(&mut repo, Path::new("file.txt"))
            .unwrap();
        write_file(dir.path(), "file.txt", "worktree\n");

        service
            .save_stash(&mut repo, "keep staged", false, true)
            .unwrap();

        assert_file_text(dir.path(), "file.txt", "staged\n");
        let changes = service.status_full(&repo).unwrap();
        let change = changes
            .iter()
            .find(|change| change.path == "file.txt")
            .unwrap();
        assert_eq!(change.staged, Some(ChangeState::Modified));
        assert_eq!(change.unstaged, None);
    }

    #[test]
    fn drop_stash_removes_entry() {
        let (dir, mut repo, service) = init_repo();
        write_file(dir.path(), "file.txt", "base\n");
        commit_all(&repo, "initial");
        write_file(dir.path(), "file.txt", "changed\n");

        service
            .save_stash(&mut repo, "to drop", false, false)
            .unwrap();
        assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

        let snapshot = service.drop_stash(&mut repo, 0).unwrap();
        assert!(snapshot.stashes.is_empty());
        assert!(service.stashes(&mut repo).unwrap().is_empty());
    }
}
