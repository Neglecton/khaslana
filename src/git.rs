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
    BranchType, Cred, CredentialType, Delta, DiffFormat, DiffOptions, ErrorCode, FetchOptions,
    FetchPrune, IndexAddOption, ProxyOptions, PushOptions, Reference, RemoteCallbacks, Repository,
    ResetType, RevertOptions, Signature, Sort, Status, StatusOptions,
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
mod merge;
mod rebase;
mod stash;
mod submodule;
mod worktree_compat;

use worktree_compat::{
    checkout_head_preserving_locked_directories, checkout_index_preserving_locked_directories,
    checkout_tree_preserving_locked_directories, reset_preserving_locked_directories,
    revert_preserving_locked_directories,
};

#[cfg(test)]
pub(crate) mod test_support;

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

/// 推送被远端按 non-fast-forward 拒绝的统一文案（客户端预检查与服务器
/// 状态报告两条路径共用），引导用户先拉取。
pub(crate) const NON_FAST_FORWARD_PUSH_MESSAGE: &str =
    "推送被拒绝（non-fast-forward）：远端分支有新提交，请先拉取并解决后再推送";

/// 全文差异的字节预检：仅在请求全文（`full_context`）时，于分配逐行 String 之前
/// 检查新旧侧文件体积，超过 `FULL_FILE_MAX_BYTES` 则直接返回错误。
///
/// 尺寸不能直接用 delta 自带的 `size`：树对树 diff（历史/贮藏/分支比较）的
/// delta size 恒为 0（树条目不携带大小），预检会形同虚设，因此经
/// `delta_side_size` 读 ODB 对象头取真实大小。
fn guard_full_file_size(
    repo: &Repository,
    diff: &git2::Diff<'_>,
    full_context: bool,
) -> Result<()> {
    if !full_context {
        return Ok(());
    }
    let too_large = diff.deltas().any(|delta| {
        [delta.old_file(), delta.new_file()]
            .into_iter()
            .filter_map(|file| delta_side_size(repo, file))
            .any(|size| size > FULL_FILE_MAX_BYTES)
    });
    if too_large {
        return Err(GitError::Message(FULL_FILE_TOO_LARGE_MESSAGE.into()));
    }
    Ok(())
}

/// 读取 diff 单侧文件的真实大小。delta 自带的 `size` 不可靠（树条目不携带大小，
/// libgit2 只在加载过内容时才回填），因此：
/// - 零 oid（工作区文件）：`size` 来自 stat，直接采用；
/// - 有 oid（树/index blob）：读 ODB 对象头（不加载 blob 内容，避免大对象
///   为取尺寸先整体 inflate 到堆）；读不到时退回 delta size
///   （如空文件 oid 是空内容散列，但对象库里未必存在该 blob 对象）。
/// 缺失侧（Added/Untracked 的旧侧、Deleted 的新侧）由调用方按 delta 状态跳过。
fn delta_side_size(repo: &Repository, file: git2::DiffFile<'_>) -> Option<u64> {
    let id = file.id();
    if id.is_zero() {
        Some(file.size())
    } else {
        Some(
            repo.odb()
                .and_then(|odb| odb.read_header(id).map(|(size, _)| size.max(0) as u64))
                .unwrap_or(file.size()),
        )
    }
}

/// 未跟踪文件的二进制嗅探：工作区 diff 用 `include_untracked` 但不带
/// `show_untracked_content`，libgit2 不会加载内容，也就不会设置 BINARY 标志；
/// 手动读工作区文件前 8KB 查 NUL 字节（与 browse_file_content 的判定规则一致）。
fn workdir_file_is_binary(repo: &Repository, rel_path: &Path) -> bool {
    let Some(workdir) = repo.workdir() else {
        return false;
    };
    let Ok(mut file) = std::fs::File::open(workdir.join(rel_path)) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(read) = std::io::Read::read(&mut file, &mut buf) else {
        return false;
    };
    buf[..read].contains(&0)
}

/// 已知二进制格式的扩展名兜底：内容检测（NUL 嗅探 / libgit2 BINARY 标志）对
/// 空文件无能为力（新建即空的 .docx 等占位文件），按扩展名判定更符合直觉。
/// 有内容时内容检测总是先命中，这里只补空文件和极少数无 NUL 二进制的场景。
fn path_has_binary_extension(path: &str) -> bool {
    const BINARY_EXTENSIONS: &[&str] = &[
        // 压缩包 / Office 文档
        "zip", "docx", "xlsx", "pptx", "doc", "xls", "ppt", "pdf", "7z", "rar", "gz", "jar",
        // 图片（svg 是文本，不在列）
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tif", "tiff", // 音视频
        "mp3", "mp4", "avi", "mov", "wmv", "flac", "ogg", "wav", "mkv",
        // 可执行 / 库 / 字体
        "exe", "dll", "so", "dylib", "msi", "ttf", "otf", "woff", "woff2",
    ];
    let Some(ext) = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
    else {
        return false;
    };
    BINARY_EXTENSIONS.contains(&ext.as_str())
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
        if let Ok(mut current) = self.proxy_settings.lock() {
            *current = proxy_settings;
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
            merge_in_progress: merge_in_progress(repo),
            merge_message: merge_message(repo),
            rebase_in_progress: rebase_in_progress(repo),
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
            merge_in_progress: merge_in_progress(repo),
            merge_message: merge_message(repo),
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
            merge_in_progress: merge_in_progress(repo),
            merge_message: merge_message(repo),
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
            // 只解析一次 upstream：既取显示名，也取 ahead/behind 的目标提交。
            let upstream = if branch_type == BranchType::Local {
                branch.upstream().ok()
            } else {
                None
            };
            let upstream_name = upstream
                .as_ref()
                .and_then(|u| u.name().ok().flatten().map(str::to_string));
            let (ahead, behind) = match (
                branch.get().target(),
                upstream.as_ref().and_then(|u| u.get().target()),
            ) {
                (Some(local_oid), Some(upstream_oid)) => {
                    // 单个分支的 upstream 元数据异常（对象库缺提交、悬空引用等）
                    // 不应让整个仓库无法打开：降级为无 ahead/behind 并记录告警。
                    match repo.graph_ahead_behind(local_oid, upstream_oid) {
                        Ok((ahead, behind)) => (Some(ahead), Some(behind)),
                        Err(err) => {
                            tracing::warn!(
                                target: "khaslana::git",
                                "分支 {name} 的 upstream ahead/behind 计算失败：{err}"
                            );
                            (None, None)
                        }
                    }
                }
                _ => (None, None),
            };
            branches.push(BranchInfo {
                name: name.to_string(),
                kind: match branch_type {
                    BranchType::Local => BranchKind::Local,
                    BranchType::Remote => BranchKind::Remote,
                },
                is_head: branch.is_head(),
                upstream: upstream_name,
                ahead,
                behind,
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
            // 非 UTF-8 文件名不能让整个仓库状态加载失败：按字节读取并做
            // 有损转换（无法显示的字节替换为 U+FFFD 占位）。
            let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
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
        // 非快进干净合并自动提交，不保留待确认会话。
        self.complete_clean_merge(repo, &format!("{}/{}", remote.0, branch))?;

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
        // 非快进干净合并自动提交，不保留待确认会话。
        self.complete_clean_merge(repo, &format!("{}/{}", remote.0, remote_branch.0))?;

        self.progress.emit(OperationEvent::Finished(format!(
            "已拉取 {}/{}",
            remote.0, remote_branch.0
        )));
        self.snapshot_after_operation(repo)
    }

    /// 拉取指定本地分支。当前分支执行正常合并，非当前分支仅允许安全快进。
    pub fn pull_local_branch(
        &self,
        repo: &mut Repository,
        local_branch_name: &BranchName,
    ) -> Result<RepositorySnapshot> {
        validate_branch_name(&local_branch_name.0)?;
        let local_ref_name = format!("refs/heads/{}", local_branch_name.0);
        let remote = repo
            .branch_upstream_remote(&local_ref_name)?
            .as_str()
            .map_err(|_| GitError::Message("upstream 远端名称不是有效 UTF-8".into()))?
            .to_string();
        let upstream_ref_name = repo
            .branch_upstream_name(&local_ref_name)?
            .as_str()
            .map_err(|_| GitError::Message("upstream 分支名称不是有效 UTF-8".into()))?
            .to_string();
        let remote_prefix = format!("refs/remotes/{remote}/");
        let remote_branch = upstream_ref_name
            .strip_prefix(&remote_prefix)
            .ok_or_else(|| GitError::Message("无法识别本地分支关联的远程分支".into()))?
            .to_string();

        self.progress.emit(OperationEvent::Started(format!(
            "正在拉取本地分支 {}（{remote}/{remote_branch}）",
            local_branch_name.0
        )));
        self.fetch_remote_refs(repo, &RemoteName::new(remote.clone()))?;

        let local_branch = repo.find_branch(&local_branch_name.0, BranchType::Local)?;
        let is_head = local_branch.is_head();
        let local_oid = local_branch
            .get()
            .target()
            .ok_or_else(|| GitError::Message("本地分支没有可更新的提交".into()))?;
        let upstream_branch = local_branch.upstream()?;
        let upstream_oid = upstream_branch
            .get()
            .target()
            .ok_or_else(|| GitError::Message("远程分支没有可拉取的提交".into()))?;
        let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;

        if behind == 0 {
            self.progress.emit(OperationEvent::Finished(format!(
                "本地分支 {} 已是最新",
                local_branch_name.0
            )));
            drop(upstream_branch);
            drop(local_branch);
            return self.snapshot_after_operation(repo);
        }

        if !is_head {
            if ahead > 0 {
                return Err(GitError::Message(format!(
                    "分支 {} 与 {remote}/{remote_branch} 已分叉，请先切换到该分支再拉取",
                    local_branch_name.0
                )));
            }
            drop(upstream_branch);
            drop(local_branch);
            repo.find_reference(&local_ref_name)?.set_target(
                upstream_oid,
                &format!("pull: fast-forward {remote}/{remote_branch}"),
            )?;
        } else {
            let annotated = repo.find_annotated_commit(upstream_oid)?;
            drop(upstream_branch);
            drop(local_branch);
            self.merge_annotated(repo, &annotated, &format!("{remote}/{remote_branch}"))?;
            drop(annotated);
            // 非快进干净合并自动提交，不保留待确认会话。
            self.complete_clean_merge(repo, &format!("{remote}/{remote_branch}"))?;
        }

        self.progress.emit(OperationEvent::Finished(format!(
            "已拉取本地分支 {}",
            local_branch_name.0
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
        // 常规 non-fast-forward（未拉取远端新提交、或已 fetch 未合并）会被
        // libgit2 在传输前的客户端检查拦下，返回英文 NotFastForward 错误；
        // 统一映射为中文引导，与 push_update_reference 回调的文案一致。
        match result {
            Ok(()) => {}
            Err(err) if err.code() == git2::ErrorCode::NotFastForward => {
                return Err(GitError::Message(NON_FAST_FORWARD_PUSH_MESSAGE.into()));
            }
            Err(err) => return Err(err.into()),
        }

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
        checkout_tree_preserving_locked_directories(repo, &object, &mut checkout)?;
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
        checkout_tree_preserving_locked_directories(repo, commit.as_object(), &mut checkout)?;
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

    /// 批量删除本地分支（仅本地，不涉及远端）。
    ///
    /// 复用 `delete_branch` 的 libgit2 路径，不绕开 `GitService`。
    /// 供工作流 `deleteBranches` 步骤使用：循环内只做删除，快照由调用方统一刷新一次。
    /// 某个分支不存在或无法删除时立即返回错误，已删除的分支不会被回滚。
    pub fn delete_local_branches(&self, repo: &mut Repository, names: &[BranchName]) -> Result<()> {
        for name in names {
            let mut branch_handle = repo.find_branch(&name.0, BranchType::Local)?;
            branch_handle.delete()?;
            drop(branch_handle);
        }
        Ok(())
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
                checkout_index_preserving_locked_directories(
                    repo,
                    Some(&mut index),
                    &mut checkout,
                )?;
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
                        checkout_head_preserving_locked_directories(repo, &mut checkout)?;
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
        if merge::merge_in_progress(repo) {
            return self.finish_merge(repo, message);
        }
        // 变基进行中不允许普通提交：会以单亲 HEAD 创建提交挪走分支，
        // 随后的 cleanup_state 还会删除 rebase-merge 状态目录，剩余待重放
        // 提交直接丢失。变基期间应通过 rebase_continue 推进。
        if rebase_in_progress(repo) {
            return Err(GitError::Message(
                "变基进行中，请通过变基状态条继续/跳过/中止，不能直接提交".into(),
            ));
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

        guard_full_file_size(repo, &diff, full_context)?;
        self.file_diff_from_diff(repo, diff, path_to_git(path), scope, encoding)
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
        reset_preserving_locked_directories(repo, commit.as_object(), reset_type)?;
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
        reset_preserving_locked_directories(repo, parent.as_object(), ResetType::Soft)?;
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
        checkout_head_preserving_locked_directories(repo, &mut checkout)?;
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
        let revert_result =
            revert_preserving_locked_directories(repo, &revert_commit, &mut options);
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
        let revert_result = revert_preserving_locked_directories(repo, &merge_commit, &mut options);
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
            let committer = commit.committer();
            let committer_name = committer.name().unwrap_or("未知提交者").to_string();
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
                // message() 对非 UTF-8 编码返回 Err，统一走字节读取 + 有损转换。
                message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
                author: author_name,
                author_email: author.email().ok().map(str::to_string),
                committer: committer_name,
                committer_email: committer.email().ok().map(str::to_string),
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
        guard_full_file_size(repo, &diff, full_context)?;
        self.file_diff_from_diff(repo, diff, path_to_git(path), DiffScope::Staged, encoding)
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
        repo: &Repository,
        diff: git2::Diff<'_>,
        path: String,
        scope: DiffScope,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let started = Instant::now();
        struct RawDiffLine {
            kind: DiffLineKind,
            old_lineno: Option<u32>,
            new_lineno: Option<u32>,
            content: Vec<u8>,
        }

        let mut raw_lines = Vec::new();
        let mut encoding_sample = Vec::new();
        let mut is_binary = false;
        let mut old_size = None;
        let mut new_size = None;
        for delta in diff.deltas() {
            let status = delta.status();
            // 未跟踪文件不加载内容，需手动嗅探是否二进制。
            if status == git2::Delta::Untracked {
                if let Some(path) = delta.new_file().path() {
                    is_binary = is_binary || workdir_file_is_binary(repo, path);
                }
            }
            // 按状态区分缺失侧：Added/Untracked 无旧文件，Deleted 无新文件。
            if status != git2::Delta::Added && status != git2::Delta::Untracked {
                old_size = delta_side_size(repo, delta.old_file());
            }
            if status != git2::Delta::Deleted {
                new_size = delta_side_size(repo, delta.new_file());
            }
        }

        diff.print(DiffFormat::Patch, |delta, _hunk, line| {
            // 二进制标记在补丁生成时才可靠：libgit2 只有加载内容后才在 delta 上
            // 回填 BINARY 标志（树→index diff 创建阶段不会读取 blob）。
            // 'B' origin 是无 show_binary 时的 "Binary files ... differ" 行，同样视为二进制并跳过该行。
            if delta.flags().contains(git2::DiffFlags::BINARY) || line.origin() == 'B' {
                is_binary = true;
            }
            if line.origin() == 'B' {
                return true;
            }
            let kind = match line.origin() {
                '+' => DiffLineKind::Added,
                '-' => DiffLineKind::Removed,
                'F' | 'H' => DiffLineKind::Header,
                _ => DiffLineKind::Context,
            };
            let content = line.content();
            if kind != DiffLineKind::Header && encoding_sample.len() < DIFF_ENCODING_SAMPLE_LIMIT {
                let remaining = DIFF_ENCODING_SAMPLE_LIMIT - encoding_sample.len();
                encoding_sample.extend_from_slice(&content[..content.len().min(remaining)]);
            }
            raw_lines.push(RawDiffLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                content: content.to_vec(),
            });
            true
        })?;

        let (resolved_encoding, encoding_impl) = resolve_diff_encoding(encoding, &encoding_sample);
        let mut lossy = false;
        let lines = raw_lines
            .into_iter()
            .map(|line| {
                let (content, had_errors) = decode_diff_line(&line.content, encoding_impl);
                lossy |= had_errors;
                DiffLine {
                    kind: line.kind,
                    old_lineno: line.old_lineno,
                    new_lineno: line.new_lineno,
                    content,
                }
            })
            .collect::<Vec<_>>();

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

        // 扩展名兜底：空文件（如新建即空的 .docx）内容检测无能为力，按已知二进制扩展名判定
        let is_binary = is_binary || path_has_binary_extension(&path);
        Ok(FileDiff {
            path,
            scope,
            is_binary,
            old_size,
            new_size,
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

        // libgit2 对“服务器按引用拒绝”的推送（non-fast-forward、分支保护、
        // hook 拒绝、权限不足）不返回错误，拒绝原因只经本回调的 status 暴露；
        // 不注册则被拒推送会静默“成功”。回调仅在 push 时触发，fetch/clone 不受影响。
        callbacks.push_update_reference(|refname, status| match status {
            None => Ok(()),
            Some(msg) if msg.contains("non-fast-forward") || msg.contains("fetch first") => {
                Err(git2::Error::from_str(NON_FAST_FORWARD_PUSH_MESSAGE))
            }
            Some(msg) => {
                let branch = refname.strip_prefix("refs/heads/").unwrap_or(refname);
                Err(git2::Error::from_str(&format!(
                    "远端拒绝推送 {branch}：{msg}"
                )))
            }
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

/// 检测是否有普通合并正在进行。
pub(crate) fn merge_in_progress(repo: &Repository) -> bool {
    merge::merge_in_progress(repo)
}

/// 读取 libgit2 为当前合并准备的默认提交信息。
fn merge_message(repo: &Repository) -> Option<String> {
    merge::merge_message(repo)
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
#[path = "tests/git.rs"]
mod tests;
