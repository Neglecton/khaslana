use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod browse;
mod conflicts;

pub use conflicts::{
    ConflictBlock, ConflictBlockResolution, ConflictBlockStatus, ConflictDraftStatus,
    ConflictFileKind, ConflictFileView, ConflictResolutionSide,
};

pub use browse::{
    BrowseCompareFile, BrowseEntry, BrowseEntryKind, BrowseFileContent, BrowseListMode,
    BrowseTarget,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoPath(pub PathBuf);

impl RepoPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BranchName(pub String);

impl BranchName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteName(pub String);

impl RemoteName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl Default for RemoteName {
    fn default() -> Self {
        Self("origin".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TagName(pub String);

impl TagName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommitMessage(pub String);

impl CommitMessage {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub kind: BranchKind,
    pub is_head: bool,
    pub upstream: Option<String>,
    /// 相对 upstream 需要推送的提交数；未关联 upstream 时为 None。
    pub ahead: Option<usize>,
    /// 相对 upstream 需要拉取的提交数；未关联 upstream 时为 None。
    pub behind: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    pub credential_record_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneOptions {
    pub recursive_submodules: bool,
}

impl Default for CloneOptions {
    fn default() -> Self {
        Self {
            recursive_submodules: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleInfo {
    pub name: String,
    pub path: PathBuf,
    pub url: Option<String>,
    pub branch: Option<String>,
    pub head_id: Option<String>,
    pub index_id: Option<String>,
    pub workdir_id: Option<String>,
    pub status: SubmoduleState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmoduleRemoteSyncStatus {
    Unknown,
    Checking,
    UpToDate,
    Behind(usize),
    Ahead(usize),
    Diverged { ahead: usize, behind: usize },
    Unavailable(String),
}

impl SubmoduleRemoteSyncStatus {
    pub fn from_ahead_behind(ahead: usize, behind: usize) -> Self {
        match (ahead, behind) {
            (0, 0) => Self::UpToDate,
            (0, behind) => Self::Behind(behind),
            (ahead, 0) => Self::Ahead(ahead),
            (ahead, behind) => Self::Diverged { ahead, behind },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleState {
    pub initialized: bool,
    pub checked_out: bool,
    pub head_matches_index: bool,
    pub workdir_modified: bool,
    pub workdir_untracked: bool,
}

impl SubmoduleState {
    pub fn is_ready(&self) -> bool {
        self.initialized
            && self.checked_out
            && self.head_matches_index
            && !self.workdir_modified
            && !self.workdir_untracked
    }

    pub fn label(&self) -> &'static str {
        if !self.initialized {
            "未初始化"
        } else if !self.checked_out {
            "未检出"
        } else if self.workdir_modified {
            "有改动"
        } else if self.workdir_untracked {
            "有未跟踪文件"
        } else if !self.head_matches_index {
            "需更新"
        } else {
            "已同步"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashInfo {
    pub index: usize,
    pub message: String,
    pub oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashFileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeState {
    Added,
    Modified,
    Deleted,
    Renamed,
    Typechange,
    Conflicted,
    Untracked,
}

impl ChangeState {
    pub fn label(&self) -> &'static str {
        match self {
            ChangeState::Added => "A",
            ChangeState::Modified => "M",
            ChangeState::Deleted => "D",
            ChangeState::Renamed => "R",
            ChangeState::Typechange => "T",
            ChangeState::Conflicted => "!",
            ChangeState::Untracked => "?",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeChange {
    pub path: String,
    pub staged: Option<ChangeState>,
    pub unstaged: Option<ChangeState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiffScope {
    Staged,
    Unstaged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Header,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffEncodingChoice {
    #[default]
    Auto,
    Utf8,
    Gb18030,
    Big5,
}

impl DiffEncodingChoice {
    pub fn label(self) -> &'static str {
        match self {
            DiffEncodingChoice::Auto => "自动",
            DiffEncodingChoice::Utf8 => "UTF-8",
            DiffEncodingChoice::Gb18030 => "GB18030/GBK",
            DiffEncodingChoice::Big5 => "Big5",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffEncodingInfo {
    pub requested: DiffEncodingChoice,
    pub resolved: DiffEncodingChoice,
    pub lossy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub scope: DiffScope,
    pub is_binary: bool,
    /// 旧侧文件大小（字节）；新增文件（Added delta）为 None。
    pub old_size: Option<u64>,
    /// 新侧文件大小（字节）；删除文件（Deleted delta）为 None。
    pub new_size: Option<u64>,
    pub encoding: DiffEncodingInfo,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub oid: String,
    pub short_oid: String,
    /// 仅首行摘要（列表行展示）。
    pub summary: String,
    /// 完整提交信息（含多行 body，供提交详情区展示）。
    pub message: String,
    pub author: String,
    pub author_email: Option<String>,
    /// 提交者（rebase/cherry-pick 后可能与作者不同，此时详情区单独展示）。
    pub committer: String,
    pub committer_email: Option<String>,
    pub time: i64,
    pub parents: Vec<String>,
    pub refs: Vec<CommitRefInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRefInfo {
    pub name: String,
    pub kind: CommitRefKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchSyncStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub unpushed_oids: Vec<String>,
    pub unpushed_oids_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitRefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
    Head,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryScope {
    #[default]
    CurrentBranch,
    AllRefs,
}

impl HistoryScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentBranch => "当前分支",
            Self::AllRefs => "所有分支",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositorySnapshot {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branches: Vec<BranchInfo>,
    pub changes: Vec<WorktreeChange>,
    pub remotes: Vec<RemoteInfo>,
    pub tags: Vec<TagInfo>,
    pub stashes: Vec<StashInfo>,
    pub conflicts: Vec<String>,
    /// 是否有普通合并正在进行（由 libgit2 的 RepositoryState::Merge 判定）。
    pub merge_in_progress: bool,
    /// 合并提交的默认信息；合并冲突解决后允许用户在提交框中修改。
    pub merge_message: Option<String>,
    /// 是否有变基正在进行（检测 .git/rebase-merge 或 rebase-apply 目录）。
    pub rebase_in_progress: bool,
}

/// 变基操作结果：要么全部完成，要么暂停在冲突处等待用户解决。
#[derive(Clone, Debug)]
pub enum RebaseOutcome {
    /// 变基全部完成，无冲突。
    Completed(RepositorySnapshot),
    /// 变基暂停：当前提交重放产生冲突，需要用户解决后继续。
    Conflicts {
        snapshot: RepositorySnapshot,
        /// 当前正在重放的提交序号（1-based）。
        current: usize,
        /// 待重放的提交总数。
        total: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationEvent {
    Started(String),
    Progress(String),
    Finished(String),
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git 错误：{0}")]
    Git(#[from] git2::Error),
    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("凭据错误：{0}")]
    Credential(String),
    #[error("访问 {url} 需要身份验证")]
    CredentialRequired { url: String },
    #[error("分支名称无效：{0}")]
    InvalidBranchName(String),
    #[error("提交信息不能为空")]
    EmptyCommitMessage,
    #[error("尚未打开仓库")]
    NoRepository,
    #[error("操作产生冲突：{0:?}")]
    Conflicts(Vec<String>),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, GitError>;
