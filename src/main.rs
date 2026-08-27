#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai_view;
mod assets;
mod blame_view;
mod browse_compare_view;
mod browse_view;
mod chrome_view;
mod commit_graph_view;
mod conflicts;
mod diff_view;
mod external_merge_view;
mod history_view;
mod markdown_view;
mod merge_view;
mod oauth;
mod operation_blocker_view;
mod proxy_view;
mod rebase_view;
mod remote_branch_operation;
mod shortcuts_view;
mod sidebar_view;
mod ssh_credentials;
mod stash_view;
mod submodule_view;
mod system;
mod tasks;
mod text_input;
mod theme_view;
#[cfg(windows)]
mod tray;
mod ui;
mod ui_helpers;
mod workflow_editor;
mod workflow_view;
mod worktree_view;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut, Range};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_channel::{Receiver, Sender};
use git2::Repository;
use gpui::{
    App, Application, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, ListHorizontalSizingBehavior, ListSizingBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle,
    ScrollStrategy, TitlebarOptions, UTF16Selection, UniformListScrollHandle, WeakEntity, Window,
    WindowBounds, WindowOptions, actions, canvas, div, img, point, prelude::*, px, size,
    uniform_list,
};
use khaslana::{
    AiProviderSettings, AiReviewRecord, AiReviewResult, AiReviewStep, BlameView, BranchKind,
    BranchName, BranchSyncStatus, BrowseCompareFile, BrowseEntry, BrowseFileContent,
    BrowseListMode, BrowseRefKind, BrowseTarget, ChangeState, CommitFileChange, CommitInfo,
    CommitMessage, ConflictBlockResolution, ConflictFileKind, ConflictFileView, CredentialProvider,
    CredentialRecord, CredentialRequest, CredentialScope, CredentialStore, CustomProxySettings,
    DiffEncodingChoice, DiffEncodingInfo, DiffEncodingPreferences, DiffLineKind, DiffScope,
    ExternalMergeSettings, FileDiff, GitCredential, GitService, HistoryRefsCache, HistoryScope,
    KeyringCredentialStore, LineSelection, NetworkProxyMode, NetworkProxySettings, OperationEvent,
    ProgressEmitter, RemoteCredentialBinding, RemoteCredentialBindings, RemoteCredentialPolicy,
    RemoteInfo, RemoteName, RepoPath, RepositorySnapshot, ResetMode, SelectedDiffLine,
    SelectionSide, SessionState, ShortcutBindings, SubmoduleInfo, SubmoduleRemoteSyncStatus,
    TagName, ThemeMode, UpdatePreferences, credential_display_target, credential_key_filename,
    credential_kind_label, credential_record_is_compatible_with_url, credential_record_label,
    credential_record_matches_remote_url, credential_scope_label, normalize_remote_url,
    syntax::SyntaxSpans as SharedSyntaxSpans,
    test_credential_connection,
    update::{self, UpdateCheckResult, UpdateManifest, UpdatePlatformAsset},
};
use lru::LruCache;
use operation_blocker_view::OperationBlocker;
use remote_branch_operation::{
    RemoteBranchOperationKind, RemoteBranchOperationState, default_remote_branch_for,
    local_branch_by_name, remote_branch_dialog_defaults, remote_branch_exists,
};
use ssh_credentials::{SshCredentialDiscoveryState, SshDiscoveryResult};
use stash_view::StashPreviewState;
use submodule_view::{
    SubmoduleDialogState, operation_refreshes_submodule_dialog, submodule_remote_request_matches,
    submodule_request_matches,
};
use tasks::{TaskExecutor, TaskKind};
use text_input::{
    MULTILINE_LINE_HEIGHT, MULTILINE_MIN_LINES, MultiLineInputElement, SingleLineInputElement,
    TextFieldState,
};
use ui::theme::rgb;
use ui::{
    components::{
        AppToastKind, FeedbackMessage, InputFrameSize, app_shell_surface, bottom_progress_bar,
        danger_callout, dialog_actions, dialog_overlay, dialog_panel as ui_dialog_panel,
        feedback_bubble, feedback_stack, glass_menu, input_frame, segmented_button, toggle_box,
        tooltip_text,
    },
    icons::{OauthBrand, ToolbarIcon, toolbar_icon},
    theme as ui_theme,
};
use ui_helpers::*;
use workflow_editor::{WorkflowEditorState, workflow_editor_field_or_fallback};
use workflow_view::{
    WorkflowInputFieldState, WorkflowLogEntry, WorkflowTemplateItem, workflow_templates_dir,
};
use yororen_ui::{
    component::{init as init_yororen_components, select, select_option},
    i18n::{I18n, Locale},
    theme::GlobalTheme,
};

actions!(
    text_input,
    [
        TextBackspace,
        TextDelete,
        TextLeft,
        TextRight,
        TextUp,
        TextDown,
        TextSelectLeft,
        TextSelectRight,
        TextSelectUp,
        TextSelectDown,
        TextSelectAll,
        TextHome,
        TextEnd,
        TextPaste,
        TextCopy,
        TextCut,
        TextSubmit,
    ]
);

// 应用级快捷键动作：每个对应一个可配置快捷键的功能入口。
// bind_keys 把按键映射到这些 action，on_action 在根元素上监听并分发到 RepositoryView 方法。
// 命名加 Shortcut 前缀，避免与 ShortcutAction 枚举变体及其它类型冲突。
actions!(
    app_action,
    [
        ShortcutRefresh,             // 刷新
        ShortcutFetch,               // 获取
        ShortcutPull,                // 拉取
        ShortcutPush,                // 推送
        ShortcutOpenStash,           // 贮藏
        ShortcutOpenSubmodule,       // 子模块
        ShortcutOpenSettings,        // 设置
        ShortcutSwitchToWorktree,    // 工作区
        ShortcutSwitchToHistory,     // 提交记录
        ShortcutSwitchToWorkflow,    // 工作流
        ShortcutOpenInExplorer,      // 资源管理器打开仓库
        ShortcutOpenRemoteInBrowser, // 浏览器打开远端
    ]
);

/// 可配置快捷键的功能枚举，用于持久化与设置中心 UI。
/// action_id 是序列化键（存入 ShortcutBindings），default_keystroke 是内置默认组合。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShortcutAction {
    Refresh,
    Fetch,
    Pull,
    Push,
    OpenStash,
    OpenSubmodule,
    OpenSettings,
    SwitchToWorktree,
    SwitchToHistory,
    SwitchToWorkflow,
    OpenInExplorer,
    OpenRemoteInBrowser,
}

impl ShortcutAction {
    /// 全部动作，按设置中心显示顺序。
    pub(crate) const ALL: [ShortcutAction; 12] = [
        ShortcutAction::Refresh,
        ShortcutAction::Fetch,
        ShortcutAction::Pull,
        ShortcutAction::Push,
        ShortcutAction::OpenStash,
        ShortcutAction::OpenSubmodule,
        ShortcutAction::OpenSettings,
        ShortcutAction::SwitchToWorktree,
        ShortcutAction::SwitchToHistory,
        ShortcutAction::SwitchToWorkflow,
        ShortcutAction::OpenInExplorer,
        ShortcutAction::OpenRemoteInBrowser,
    ];

    /// 序列化键，存入 ShortcutBindings 的 BTreeMap key。
    pub(crate) fn action_id(&self) -> &'static str {
        match self {
            ShortcutAction::Refresh => "refresh",
            ShortcutAction::Fetch => "fetch",
            ShortcutAction::Pull => "pull",
            ShortcutAction::Push => "push",
            ShortcutAction::OpenStash => "open_stash",
            ShortcutAction::OpenSubmodule => "open_submodule",
            ShortcutAction::OpenSettings => "open_settings",
            ShortcutAction::SwitchToWorktree => "switch_to_worktree",
            ShortcutAction::SwitchToHistory => "switch_to_history",
            ShortcutAction::SwitchToWorkflow => "switch_to_workflow",
            ShortcutAction::OpenInExplorer => "open_in_explorer",
            ShortcutAction::OpenRemoteInBrowser => "open_remote_in_browser",
        }
    }

    /// 用户可见的中文标签。
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ShortcutAction::Refresh => "刷新",
            ShortcutAction::Fetch => "获取",
            ShortcutAction::Pull => "拉取",
            ShortcutAction::Push => "推送",
            ShortcutAction::OpenStash => "贮藏",
            ShortcutAction::OpenSubmodule => "子模块",
            ShortcutAction::OpenSettings => "设置",
            ShortcutAction::SwitchToWorktree => "工作区",
            ShortcutAction::SwitchToHistory => "提交记录",
            ShortcutAction::SwitchToWorkflow => "工作流",
            ShortcutAction::OpenInExplorer => "在资源管理器中打开仓库",
            ShortcutAction::OpenRemoteInBrowser => "以浏览器打开当前远端",
        }
    }

    /// 内置默认快捷键（GPUI keystroke 字符串格式）。
    pub(crate) fn default_keystroke(&self) -> &'static str {
        match self {
            ShortcutAction::Refresh => "f5",
            ShortcutAction::Fetch => "ctrl-shift-f",
            ShortcutAction::Pull => "ctrl-shift-l",
            ShortcutAction::Push => "ctrl-shift-p",
            ShortcutAction::OpenStash => "ctrl-shift-s",
            ShortcutAction::OpenSubmodule => "ctrl-shift-m",
            ShortcutAction::OpenSettings => "ctrl-,",
            ShortcutAction::SwitchToWorktree => "ctrl-1",
            ShortcutAction::SwitchToHistory => "ctrl-2",
            ShortcutAction::SwitchToWorkflow => "ctrl-3",
            ShortcutAction::OpenInExplorer => "ctrl-shift-o",
            ShortcutAction::OpenRemoteInBrowser => "ctrl-shift-b",
        }
    }

    /// 用 action_id 反查枚举。
    pub(crate) fn from_id(id: &str) -> Option<ShortcutAction> {
        Self::ALL
            .iter()
            .find(|action| action.action_id() == id)
            .copied()
    }

    /// 返回当前生效的 keystroke 的引用（优先用户绑定，回退默认 static）。
    /// 注意：返回值生命周期绑到 `bindings`，因为 default 是 `&'static str` 可安全兼容。
    pub(crate) fn keystroke<'a>(&self, bindings: &'a ShortcutBindings) -> &'a str {
        match bindings.bindings.get(self.action_id()) {
            Some(k) => k.as_str(),
            None => self.default_keystroke(),
        }
    }
}

/// 构造包含全部 12 条默认快捷键的 ShortcutBindings。
pub(crate) fn default_shortcut_bindings() -> ShortcutBindings {
    let mut bindings = BTreeMap::new();
    for action in ShortcutAction::ALL {
        bindings.insert(
            action.action_id().to_string(),
            action.default_keystroke().to_string(),
        );
    }
    ShortcutBindings { bindings }
}

/// 查找快捷键冲突：返回已占用该 keystroke 的其它动作（排除 target 自身）。
pub(crate) fn find_shortcut_conflict(
    bindings: &ShortcutBindings,
    target: ShortcutAction,
    keystroke: &str,
) -> Option<ShortcutAction> {
    ShortcutAction::ALL
        .iter()
        .find(|action| **action != target && action.keystroke(bindings) == keystroke)
        .copied()
}

const DEFAULT_SIDEBAR_WIDTH: f32 = 300.0;
const DEFAULT_CHANGES_WIDTH: f32 = 350.0;
const MIN_COLUMN_WIDTH: f32 = 240.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const CHANGE_ROW_HEIGHT: f32 = 36.0;
// 提交详情区高度（历史检查器上半部）：默认紧凑展示摘要+正文+元信息，可拖拽调整。
const DEFAULT_HISTORY_DETAILS_HEIGHT: f32 = 260.0;
const MIN_HISTORY_DETAILS_HEIGHT: f32 = 120.0;
const MAX_HISTORY_DETAILS_HEIGHT: f32 = 720.0;
const DEFAULT_HISTORY_FILES_WIDTH: f32 = 520.0;
const MIN_HISTORY_FILES_WIDTH: f32 = 260.0;
// 提交导航列上限放宽到 1080：宽屏下摘要 + ref 徽标有足够信息密度可铺更宽。
const MAX_HISTORY_FILES_WIDTH: f32 = 1080.0;
// 历史检查器内「提交文件 | 差异」分栏（四象限下半部）：默认沿用固定窄栏值。
const DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH: f32 = 370.0;
const MIN_HISTORY_INSPECTOR_FILES_WIDTH: f32 = 220.0;
const MAX_HISTORY_INSPECTOR_FILES_WIDTH: f32 = 720.0;
// 工作流模板导航列：模板名 + 描述需要比通用列更宽的上限，独立于提交导航约束。
const DEFAULT_WORKFLOW_TEMPLATES_WIDTH: f32 = 304.0;
const MIN_WORKFLOW_TEMPLATES_WIDTH: f32 = 260.0;
const MAX_WORKFLOW_TEMPLATES_WIDTH: f32 = 720.0;
const DEFAULT_BROWSE_TREE_WIDTH: f32 = 400.0;
const MIN_BROWSE_TREE_WIDTH: f32 = 240.0;
const MAX_BROWSE_TREE_WIDTH: f32 = 640.0;
// 提交图列宽：默认显示 6 条泳道，可拖拽调整；过窄时仅显示少量泳道，超出以省略号提示。
const DEFAULT_HISTORY_GRAPH_WIDTH: f32 = 96.0;
const MIN_HISTORY_GRAPH_WIDTH: f32 = 64.0;
const MAX_HISTORY_GRAPH_WIDTH: f32 = 480.0;
const HISTORY_PAGE_SIZE: usize = 50;
pub(crate) const BRANCH_MENU_WIDTH: f32 = 190.0;
pub(crate) const BRANCH_MENU_HEIGHT: f32 = 404.0;
pub(crate) const REMOTE_MENU_WIDTH: f32 = 170.0;
pub(crate) const REMOTE_MENU_HEIGHT: f32 = 80.0;
const CHANGE_MENU_WIDTH: f32 = 210.0;
// 两个菜单分支均新增「查看文件历史」「追溯此文件」两项（约 +34px/项），
// 未暂存分支还多一条分隔线。
const CHANGE_MENU_HEIGHT: f32 = 330.0;
const STAGED_CHANGE_MENU_HEIGHT: f32 = 395.0;
const FILE_PATH_MENU_WIDTH: f32 = 180.0;
// 提交文件右键菜单新增「查看文件历史」「追溯此文件」两项。
const FILE_PATH_MENU_HEIGHT: f32 = 140.0;
const CREDENTIAL_MENU_WIDTH: f32 = 180.0;
const CREDENTIAL_MENU_HEIGHT: f32 = 150.0;
pub(crate) const TAG_MENU_WIDTH: f32 = 170.0;
pub(crate) const TAG_MENU_HEIGHT: f32 = 200.0;
pub(crate) const STASH_MENU_WIDTH: f32 = 170.0;
pub(crate) const STASH_MENU_HEIGHT: f32 = 170.0;
pub(crate) const WORKFLOW_TEMPLATE_MENU_WIDTH: f32 = 150.0;
pub(crate) const WORKFLOW_TEMPLATE_MENU_HEIGHT: f32 = 76.0;
const COMMIT_MENU_WIDTH: f32 = 230.0;
const COMMIT_MENU_HEIGHT: f32 = 320.0;
const COMMIT_UNPUSHED_MENU_HEIGHT: f32 = 355.0;
const ENCODING_MENU_WIDTH: f32 = 170.0;
const MENU_VIEWPORT_MARGIN: f32 = 8.0;
// Windows 原生窗口控制区（3×44px）固定占宽；壳层布局与本文件共用同一常量。
pub(crate) const WINDOW_CONTROLS_WIDTH: f32 = 132.0;
// 仓库切换下拉尺寸：宽 320 容纳完整路径，高 480 内部滚动。
const REPO_SWITCHER_MENU_WIDTH: f32 = 320.0;
const REPO_SWITCHER_MENU_HEIGHT: f32 = 480.0;
const MAX_CONCURRENT_REPO_LOADS: usize = 2;
/// 同时进行的 AI 评审任务上限（含切目标后仍在后台跑的分离任务）；
/// 超出时阻止新开，提示等待完成或取消。
const MAX_CONCURRENT_AI_REVIEWS: usize = 3;
/// 评审历史弹窗一次加载的记录条数。
const AI_REVIEW_HISTORY_LIMIT: usize = 20;
const LARGE_DIFF_CACHE_LINE_LIMIT: usize = 20_000;
const DIFF_CACHE_CAPACITY: usize = 16;
const CONFLICT_OURS_SCROLL_HANDLE_ID: &str = "conflict-ours-scroll-handle";
const CONFLICT_RESULT_SCROLL_HANDLE_ID: &str = "conflict-result-scroll-handle";
const CONFLICT_THEIRS_SCROLL_HANDLE_ID: &str = "conflict-theirs-scroll-handle";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldId {
    CloneUrl,
    ClonePath,
    BranchName,
    BranchRename,
    RemoteName,
    RemoteUrl,
    CommitMessage,
    TagName,
    TagMessage,
    CredentialUsername,
    CredentialSecret,
    CredentialKeyPath,
    CredentialPassphrase,
    CredentialRemoteUrl,
    CredentialDisplayName,
    CredentialTestUrl,
    ConflictEditor,
    RemoteBranchName,
    RemoteBranchSearch,
    RepoSwitcherSearch,
    CommitGraphSearch,
    CommitGraphBranchSearch,
    SidebarLocalBranchSearch,
    SidebarRemoteBranchSearch,
    ProxyHttpUrl,
    ProxyHttpsUrl,
    ProxySocks5Url,
    AiBaseUrl,
    AiApiKey,
    AiModel,
    ExternalMergeIntellijPath,
    StashMessage,
    WorkflowInput(usize),
    /// 工作流模板编辑器的动态字段（模板名/文件名/步骤参数/变量行），
    /// 经 `workflow_editor_field_mut` 路由，不进 DEDICATED_FIELDS。
    WorkflowEditor(workflow_editor::WorkflowEditorFieldId),
}

/// 专用（非 WorkflowInput 动态索引）文本字段的注册表：FieldId → 状态访问器。
/// `field()` 与 `focused_field()` 共用同一份清单，避免两份手写列表漂移：
/// 漏注册的字段能正常渲染，但 `focused_field` 找不到聚焦字段会让
/// `EntityInputHandler` 静默丢弃全部键盘/粘贴/IME 输入（创建标签弹窗的
/// 标签名称与附注输入框曾因此完全无法输入）。新增 FieldId 时除补
/// `field_mut()` 的穷举 match 外，必须同步注册到这里。
type DedicatedFieldAccessor = fn(&RepositoryView) -> &TextFieldState;

const DEDICATED_FIELDS: &[(FieldId, DedicatedFieldAccessor)] = &[
    (FieldId::CloneUrl, |view: &RepositoryView| &view.clone_url),
    (FieldId::ClonePath, |view: &RepositoryView| &view.clone_path),
    (FieldId::BranchName, |view: &RepositoryView| {
        &view.branch_name
    }),
    (FieldId::BranchRename, |view: &RepositoryView| {
        &view.branch_rename
    }),
    (FieldId::RemoteName, |view: &RepositoryView| {
        &view.remote_name
    }),
    (FieldId::RemoteUrl, |view: &RepositoryView| &view.remote_url),
    (FieldId::CommitMessage, |view: &RepositoryView| {
        &view.commit_message
    }),
    (FieldId::StashMessage, |view: &RepositoryView| {
        &view.stash_message
    }),
    (FieldId::TagName, |view: &RepositoryView| &view.tag_name),
    (FieldId::TagMessage, |view: &RepositoryView| {
        &view.tag_message
    }),
    (FieldId::CredentialUsername, |view: &RepositoryView| {
        &view.credential_username
    }),
    (FieldId::CredentialSecret, |view: &RepositoryView| {
        &view.credential_secret
    }),
    (FieldId::CredentialKeyPath, |view: &RepositoryView| {
        &view.credential_key_path
    }),
    (FieldId::CredentialPassphrase, |view: &RepositoryView| {
        &view.credential_passphrase
    }),
    (FieldId::CredentialRemoteUrl, |view: &RepositoryView| {
        &view.credential_remote_url
    }),
    (FieldId::CredentialTestUrl, |view: &RepositoryView| {
        &view.credential_test_url
    }),
    (FieldId::CredentialDisplayName, |view: &RepositoryView| {
        &view.credential_display_name
    }),
    (FieldId::ConflictEditor, |view: &RepositoryView| {
        &view.conflict_editor
    }),
    (FieldId::RemoteBranchName, |view: &RepositoryView| {
        &view.remote_branch_name
    }),
    (FieldId::RemoteBranchSearch, |view: &RepositoryView| {
        &view.remote_branch_search
    }),
    (FieldId::RepoSwitcherSearch, |view: &RepositoryView| {
        &view.repo_switcher_search
    }),
    // 图谱页搜索词全局共享（与仓库切换下拉搜索同一模式），跨模式/跨 tab 保留；
    // 未注册时 focused_field 找不到字段，输入框会静默丢弃全部键盘/IME 输入。
    (FieldId::CommitGraphSearch, |view: &RepositoryView| {
        &view.commit_graph_search
    }),
    // 图谱页分支高亮下拉的菜单内搜索（打开菜单即清空并聚焦）。
    (FieldId::CommitGraphBranchSearch, |view: &RepositoryView| {
        &view.commit_graph_branch_search
    }),
    (
        FieldId::SidebarLocalBranchSearch,
        |view: &RepositoryView| &view.sidebar_local_branch_search,
    ),
    (
        FieldId::SidebarRemoteBranchSearch,
        |view: &RepositoryView| &view.sidebar_remote_branch_search,
    ),
    (FieldId::ProxyHttpUrl, |view: &RepositoryView| {
        &view.proxy_http_url
    }),
    (FieldId::ProxyHttpsUrl, |view: &RepositoryView| {
        &view.proxy_https_url
    }),
    (FieldId::ProxySocks5Url, |view: &RepositoryView| {
        &view.proxy_socks5_url
    }),
    (FieldId::AiBaseUrl, |view: &RepositoryView| {
        &view.ai_base_url
    }),
    (FieldId::AiApiKey, |view: &RepositoryView| &view.ai_api_key),
    (FieldId::AiModel, |view: &RepositoryView| &view.ai_model),
    (
        FieldId::ExternalMergeIntellijPath,
        |view: &RepositoryView| &view.external_merge_intellij_path,
    ),
];

#[derive(Clone, Debug)]
struct PendingCredential {
    tab_id: Option<RepoTabId>,
    request: CredentialRequest,
    response_tx: Arc<Mutex<Option<mpsc::Sender<khaslana::Result<Option<GitCredential>>>>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialFormMode {
    Https,
    Ssh,
}

/// OAuth 快速登录的服务商。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OAuthProvider {
    Github,
    Gitee,
}

impl OAuthProvider {
    fn label(self) -> &'static str {
        match self {
            OAuthProvider::Github => "GitHub",
            OAuthProvider::Gitee => "Gitee",
        }
    }
}

/// OAuth 快速登录的 UI 状态（仿 SshCredentialDiscoveryState，支持 GitHub/Gitee）。
#[derive(Clone, Debug, Default)]
struct OAuthLoginFlowState {
    loading: bool,
    /// 当前正在登录的服务商（loading=true 时有意义）。
    provider: Option<OAuthProvider>,
    /// 自增请求号，用于忽略过期/取消后的迟到事件。
    request_id: u64,
    /// GitHub Device Flow 的用户验证码（Gitee 授权码流没有）。
    user_code: Option<String>,
    verification_uri: Option<String>,
    error: Option<String>,
    /// 后台登录任务的取消标记；UI 取消登录时置位。
    cancel: Option<Arc<AtomicBool>>,
}

/// 设置中心的分类，独立于 DialogState 以避免凭据子弹窗叠加冲突。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    Credentials,
    Proxy,
    Ai,
    ExternalMerge,
    Theme,
    Update,
    Shortcuts,
    /// 「关于」页：无设置项，展示当前版本号、发布渠道与版本说明。
    About,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogState {
    CloneRepo,
    CreateBranch,
    RenameBranch {
        branch: String,
    },
    ConfirmReset {
        oid: String,
        summary: String,
        mode: ResetMode,
    },
    ConfirmRevert {
        oid: String,
        summary: String,
    },
    ConfirmRevertMerge {
        oid: String,
        summary: String,
    },
    ConfirmUncommitToStaged {
        oid: String,
        summary: String,
    },
    ConfirmAmendPushed {
        /// 确认后执行“修补提交”还是“修补提交并推送”。
        and_push: bool,
    },
    TagForm {
        /// 创建目标提交；`None` 表示 HEAD。
        target_oid: Option<String>,
        target_summary: String,
    },
    TagPush {
        tag: String,
    },
    ConfirmDeleteTag {
        tag: String,
    },
    ConfirmDeleteRemoteTag {
        remote: String,
        tag: String,
    },
    ConfirmDiscardChange {
        scope: DiffScope,
        target: DiscardTarget,
        paths: Vec<String>,
    },
    CredentialDetails {
        record_id: String,
    },
    CredentialForm {
        editing: Option<String>,
    },
    /// 凭据测试前的地址确认弹窗：预填记录远端地址，用户可改（如裸主机
    /// 换成真实仓库地址）再发起连接测试。
    TestCredential {
        record_id: String,
    },
    SubmoduleManager,
    RemoteManager,
    RemoteForm {
        editing: Option<String>,
    },
    ConfirmDeleteRemote {
        name: String,
    },
    ConfirmDeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    ConfirmDeleteCredential {
        record_id: String,
        label: String,
    },
    StashForm,
    /// 工作流模板可视化创建器（v1 仅新建）。
    WorkflowEditor,
    /// 编辑带注释的工作流模板前的确认弹窗（保存会丢失注释与排版）。
    ConfirmWorkflowEditComments,
    /// 删除工作流模板文件的确认弹窗。
    ConfirmDeleteWorkflowTemplate {
        path: PathBuf,
        display_name: String,
    },
    ConfirmDropStash {
        index: usize,
        message: String,
    },
    /// 弹出贮藏的确认弹窗（应用改动到工作区并从贮藏列表移除）。
    ConfirmPopStash {
        index: usize,
        message: String,
    },
    RemoteBranchOperation {
        kind: RemoteBranchOperationKind,
    },
    ConfirmConflictResolve,
    ConfirmAiConflictMerge {
        path: String,
    },
    ConfirmAbortMerge,
    ConfirmWindowClose,
    // ── 更新对话框 ──
    NewVersionAvailable {
        version: String,
        notes: String,
        published_at: String,
        size: u64,
    },
    ConfirmInstallUpdate {
        version: String,
    },
    UpdateNoWritePermission {
        version: String,
    },
    // ── 便携数据目录迁移提示 ──
    PortableMigrationPrompt,
    // ── 程序位置风险搬迁提示（exe 位于临时/聊天软件接收/下载目录） ──
    ExeRelocationPrompt,
}

#[derive(Clone, Debug)]
pub(crate) struct BranchContextMenu {
    pub(crate) branch: String,
    pub(crate) kind: BranchKind,
    pub(crate) is_head: bool,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteContextMenu {
    pub(crate) remote: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct TagContextMenu {
    pub(crate) tag: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct StashContextMenu {
    pub(crate) index: usize,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// 工作流模板列表行的右键菜单（编辑此模板 / 复制为副本）。
#[derive(Clone, Debug)]
pub(crate) struct WorkflowTemplateContextMenu {
    pub(crate) path: PathBuf,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct CommitContextMenu {
    pub(crate) oid: String,
    pub(crate) short_oid: String,
    pub(crate) summary: String,
    pub(crate) parent_count: usize,
    pub(crate) is_unpushed: bool,
    pub(crate) is_head: bool,
    pub(crate) height: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Debug)]
struct CredentialContextMenu {
    record_id: String,
    x: f32,
    y: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChangeSelection {
    staged: BTreeSet<String>,
    unstaged: BTreeSet<String>,
    staged_anchor: Option<String>,
    unstaged_anchor: Option<String>,
}

impl ChangeSelection {
    fn clear(&mut self) {
        self.staged.clear();
        self.unstaged.clear();
        self.staged_anchor = None;
        self.unstaged_anchor = None;
    }

    fn selected(&self, scope: &DiffScope) -> &BTreeSet<String> {
        match scope {
            DiffScope::Staged => &self.staged,
            DiffScope::Unstaged => &self.unstaged,
        }
    }

    fn selected_mut(&mut self, scope: &DiffScope) -> &mut BTreeSet<String> {
        match scope {
            DiffScope::Staged => &mut self.staged,
            DiffScope::Unstaged => &mut self.unstaged,
        }
    }

    fn anchor(&self, scope: &DiffScope) -> Option<&String> {
        match scope {
            DiffScope::Staged => self.staged_anchor.as_ref(),
            DiffScope::Unstaged => self.unstaged_anchor.as_ref(),
        }
    }

    fn set_anchor(&mut self, scope: &DiffScope, path: String) {
        match scope {
            DiffScope::Staged => self.staged_anchor = Some(path),
            DiffScope::Unstaged => self.unstaged_anchor = Some(path),
        }
    }
}

impl Default for ChangeSelection {
    fn default() -> Self {
        Self {
            staged: BTreeSet::new(),
            unstaged: BTreeSet::new(),
            staged_anchor: None,
            unstaged_anchor: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ChangeListIndexes {
    staged: Vec<usize>,
    unstaged: Vec<usize>,
}

impl ChangeListIndexes {
    fn rebuild(changes: &[khaslana::WorktreeChange]) -> Self {
        let mut indexes = Self::default();
        for (index, change) in changes.iter().enumerate() {
            if change.staged.is_some() {
                indexes.staged.push(index);
            }
            if change.unstaged.is_some() {
                indexes.unstaged.push(index);
            }
        }
        indexes
    }

    fn for_scope(&self, scope: &DiffScope) -> &[usize] {
        match scope {
            DiffScope::Staged => &self.staged,
            DiffScope::Unstaged => &self.unstaged,
        }
    }
}

#[derive(Clone, Debug)]
struct ChangeContextMenu {
    path: String,
    scope: DiffScope,
    x: f32,
    y: f32,
}

#[derive(Clone, Debug)]
struct FilePathContextMenu {
    path: String,
    x: f32,
    y: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiscardTarget {
    Single,
    Selected,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EncodingMenuTarget {
    Worktree,
    History,
    Stash,
    Browse,
    Blame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffRenderRow {
    HeaderToggle,
    DiffLine(usize),
    Empty,
}

fn discard_paths_preview(paths: &[String]) -> String {
    let mut preview = paths.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
    if paths.len() > 5 {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(&format!("... 以及另外 {} 个文件", paths.len() - 5));
    }
    preview
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffRenderModel {
    row_count: usize,
    header_count: usize,
    headers_expanded: bool,
    empty: bool,
}

impl DiffRenderModel {
    fn row_at(&self, row_index: usize) -> DiffRenderRow {
        if self.empty {
            return DiffRenderRow::Empty;
        }
        if self.header_count > 0 {
            if row_index == 0 {
                return DiffRenderRow::HeaderToggle;
            }
            if self.headers_expanded && row_index <= self.header_count {
                return DiffRenderRow::DiffLine(row_index - 1);
            }
        }
        let body_start = if self.header_count > 0 { 1 } else { 0 };
        let header_offset = if self.headers_expanded {
            self.header_count
        } else {
            0
        };
        DiffRenderRow::DiffLine(self.header_count + row_index - body_start - header_offset)
    }
}

fn diff_render_model_for(diff: Option<&FileDiff>, headers_expanded: bool) -> DiffRenderModel {
    let Some(diff) = diff else {
        return DiffRenderModel {
            row_count: 1,
            header_count: 0,
            headers_expanded,
            empty: true,
        };
    };
    // 可折叠头部只统计纯文件头（diff --git / index / --- / +++）。
    // 第一个 @@ hunk 头紧跟文件头且同为 Header kind，必须排除在外，
    // 否则折叠头部时会连首个 hunk 头一起吞掉：hunk 少一个「暂存此块」入口，行号也跳变。
    let header_count = diff
        .lines
        .iter()
        .take_while(|line| line.kind == DiffLineKind::Header && !line.content.starts_with("@@"))
        .count();
    let mut row_count = diff.lines.len().saturating_sub(header_count);
    if header_count > 0 {
        row_count += 1;
        if headers_expanded {
            row_count += header_count;
        }
    }
    DiffRenderModel {
        row_count: row_count.max(1),
        header_count,
        headers_expanded,
        empty: row_count == 0,
    }
}

/// 估算字符串在等宽字体下的显示列宽。
///
/// 仅用于比较 diff 行的相对宽度以选出最宽行：ASCII 字符计 1 列，
/// 其余字符（含中日韩、emoji 等）按全宽计 2 列。真实像素宽度仍由
/// gpui 通过 `with_width_from_item` 实测，这里不硬编码字体度量。
pub(crate) fn display_columns(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 在 diff 渲染模型中找出内容最宽的文本行对应的 model-row 索引。
///
/// `uniform_list` 通过 `with_width_from_item` 用单个被测量 item 的宽度决定
/// 整个列表的水平内容宽度。这里遍历所有实际会渲染的文本行（经 `row_at`
/// 映射，天然尊重头部展开/折叠），挑选显示列宽最大的一行作为测量基准，
/// 从而让长行也能驱动水平滚动条。无文本行时返回 `None`，由调用方回退。
fn widest_diff_row_index(diff: Option<&FileDiff>, model: &DiffRenderModel) -> Option<usize> {
    let diff = diff?;
    (0..model.row_count)
        .filter_map(|row_index| match model.row_at(row_index) {
            DiffRenderRow::DiffLine(line_index) => diff
                .lines
                .get(line_index)
                .map(|line| (row_index, display_columns(&line.content))),
            _ => None,
        })
        .max_by_key(|&(_, columns)| columns)
        .map(|(row_index, _)| row_index)
}

/// `widest_diff_row_index` 的单槽缓存键：diff 的 Arc 地址 + 行数 + 头部展开态。
/// 行数参与比较可排除 Arc 地址复用导致的误命中；最坏情况也只是水平宽度测量
/// 略有偏差（纯视觉量），换来的是大 diff 打开期间每帧省去 O(总字符) 扫描。
type WidestDiffRowKey = (usize, usize, bool);

#[derive(Default)]
struct WidestDiffRowCache {
    key: Option<WidestDiffRowKey>,
    value: Option<usize>,
}

/// 按 diff 身份缓存最宽行扫描结果；diff 变化或头部展开切换时重算。
fn cached_widest_diff_row_index(
    diff: Option<&Arc<FileDiff>>,
    headers_expanded: bool,
    model: &DiffRenderModel,
    cache: &RefCell<WidestDiffRowCache>,
) -> Option<usize> {
    let key = diff.map(|diff| {
        (
            Arc::as_ptr(diff) as usize,
            diff.lines.len(),
            headers_expanded,
        )
    });
    let mut cache = cache.borrow_mut();
    if cache.key != key {
        cache.key = key;
        cache.value = widest_diff_row_index(diff.map(|diff| diff.as_ref()), model);
    }
    cache.value
}

fn line_index_for_byte_offset(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    text[..clamped]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

fn conflict_workbench_scroll_handle_ids() -> [&'static str; 3] {
    [
        CONFLICT_OURS_SCROLL_HANDLE_ID,
        CONFLICT_RESULT_SCROLL_HANDLE_ID,
        CONFLICT_THEIRS_SCROLL_HANDLE_ID,
    ]
}

fn conflict_result_pane_uses_editor() -> bool {
    false
}

fn conflict_editor_should_store_draft(kind: ConflictFileKind) -> bool {
    kind == ConflictFileKind::Text && conflict_result_pane_uses_editor()
}

fn default_clone_recursive_submodules() -> bool {
    true
}

/// 仓库切换下拉的展开状态；x/y 为菜单左上角的窗口坐标，展开时按触发器按钮锚点计算。
#[derive(Clone, Debug)]
struct RepoSwitcherMenu {
    x: f32,
    y: f32,
}

/// 仓库切换下拉触发器按钮的窗口坐标矩形，由触发器 paint 时记录，
/// 用于把下拉菜单固定在按钮正下方，以及“点击外部/按钮关闭”的命中判定。
#[derive(Clone, Copy, Debug)]
struct RepoSwitcherAnchor {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// 由触发器按钮锚点计算下拉菜单左上角：水平对齐按钮左缘，垂直紧贴按钮下方，并钳制在视口内。
fn repo_switcher_menu_origin(
    anchor: &RepoSwitcherAnchor,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32) {
    let max_x = (viewport_width - REPO_SWITCHER_MENU_WIDTH - MENU_VIEWPORT_MARGIN)
        .max(MENU_VIEWPORT_MARGIN);
    let max_y = (viewport_height - REPO_SWITCHER_MENU_HEIGHT - MENU_VIEWPORT_MARGIN)
        .max(MENU_VIEWPORT_MARGIN);
    (
        anchor.x.clamp(MENU_VIEWPORT_MARGIN, max_x),
        (anchor.y + anchor.h).clamp(MENU_VIEWPORT_MARGIN, max_y),
    )
}

/// 判定坐标是否落在仓库切换下拉菜单或触发器按钮矩形内（用于点击外部关闭）。
fn point_in_repo_switcher(
    x: f32,
    y: f32,
    menu: &RepoSwitcherMenu,
    anchor: Option<&RepoSwitcherAnchor>,
) -> bool {
    // 命中判定比菜单绘制区域四周多留容差：菜单锚定在触发按钮正下方，左缘常与
    // 侧边栏分栏分割线重合，点在边框/阴影上（1-2px 偏差）不应被判为菜单外部
    // 而关闭菜单并触发分割线拖拽。
    const EDGE_TOLERANCE: f32 = 4.0;
    point_in_menu(
        x,
        y,
        menu.x - EDGE_TOLERANCE,
        menu.y - EDGE_TOLERANCE,
        REPO_SWITCHER_MENU_WIDTH + EDGE_TOLERANCE * 2.0,
        REPO_SWITCHER_MENU_HEIGHT + EDGE_TOLERANCE * 2.0,
    ) || anchor.is_some_and(|anchor| point_in_menu(x, y, anchor.x, anchor.y, anchor.w, anchor.h))
}

/// 分栏分割线是否响应鼠标：弹窗打开（有全屏遮罩）或任一弹出菜单/下拉打开时
/// 不响应。弹层无遮罩，菜单边缘容差区内的点击会物理落在分割线上，若仍响应
/// 会显示拖拽光标并可拖动，抢走本应属于弹层的交互。
fn column_splitter_accepts_mouse_events(active_dialog: bool, popup_menu_open: bool) -> bool {
    !active_dialog && !popup_menu_open
}

/// 遮挡层（弹窗或弹层菜单）打开时中止进行中的分割线拖拽，避免残留按下状态。
fn column_splitter_should_clear_resize(overlay_open: bool, resizing: bool) -> bool {
    overlay_open && resizing
}

#[cfg(test)]
fn dialog_parent_should_stop_mouse_event(event_name: &str) -> bool {
    event_name == "mouse_down"
}

fn multiline_input_should_scroll(id: FieldId, value: &str) -> bool {
    id == FieldId::ConflictEditor || visual_line_count(value) > MULTILINE_MIN_LINES
}

/// 多行输入字段的滚动容器句柄 id（提交信息框与冲突编辑器各一个）。
pub(crate) fn multiline_scroll_handle_id(id: FieldId) -> &'static str {
    if id == FieldId::ConflictEditor {
        CONFLICT_RESULT_SCROLL_HANDLE_ID
    } else {
        "commit-message-input-scroll"
    }
}

#[cfg(test)]
fn multiline_input_uses_input_frame(id: FieldId) -> bool {
    id != FieldId::ConflictEditor
}

#[cfg(test)]
fn diff_render_rows_for(diff: Option<&FileDiff>, headers_expanded: bool) -> Vec<DiffRenderRow> {
    let model = diff_render_model_for(diff, headers_expanded);
    (0..model.row_count)
        .map(|index| model.row_at(index))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingConflictResolve {
    path: String,
    unresolved_count: usize,
}

/// 语法高亮的槽位：标识一份「已落地的内容」来自哪个视图，
/// 调度与回填都按槽位路由（冲突视图单独走 `ConflictSyntaxPane`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyntaxSlot {
    WorktreeDiff,
    HistoryDiff,
    StashDiff,
    BrowseDiff,
    Blame,
    BrowseContent,
}

/// 语法高亮的后台任务源内容：把 Arc 带进闭包免克隆行数据，
/// anchor = (Arc 地址, 行数) 作为回填守卫（与 widest_line_cache 同一先例，
/// 行数参与比较可排除 Arc 地址复用误命中）。
enum SyntaxSource {
    Diff(Arc<FileDiff>),
    Blame(Arc<BlameView>),
    Content(Arc<BrowseFileContent>),
}

impl SyntaxSource {
    fn anchor(&self) -> (usize, usize) {
        match self {
            Self::Diff(diff) => (Arc::as_ptr(diff) as usize, diff.lines.len()),
            Self::Blame(view) => (Arc::as_ptr(view) as usize, view.lines.len()),
            Self::Content(content) => (Arc::as_ptr(content) as usize, content.lines.len()),
        }
    }
}

/// 冲突工作台的语法高亮分栏（ours/theirs 为只读，draft 随草稿重算）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConflictSyntaxPane {
    Ours,
    Theirs,
    Draft,
}

/// 单个冲突文件三栏的语法高亮缓存（key = 冲突文件路径）。
#[derive(Clone, Debug, Default)]
struct ConflictFileSyntax {
    ours: Option<Arc<SharedSyntaxSpans>>,
    theirs: Option<Arc<SharedSyntaxSpans>>,
    draft: Option<Arc<SharedSyntaxSpans>>,
    /// 草稿重算请求序号：按块接受/AI 生成连发时丢弃晚到的旧结果。
    draft_seq: u64,
}

#[derive(Clone, Debug, Default)]
struct ConflictWorkbenchState {
    selected_path: Option<String>,
    selected_block: usize,
    show_base: bool,
    pending_resolve: Option<PendingConflictResolve>,
    files: BTreeMap<String, ConflictFileView>,
    external_merge_auto_opened: BTreeSet<String>,
    /// 每个冲突文件的三栏语法高亮（选中文件才计算，见调度器）。
    syntax: BTreeMap<String, ConflictFileSyntax>,
}

impl ConflictWorkbenchState {
    fn request_resolve_confirmation(&mut self, path: String, unresolved_count: usize) -> bool {
        if unresolved_count == 0 {
            self.pending_resolve = None;
            return false;
        }
        self.pending_resolve = Some(PendingConflictResolve {
            path,
            unresolved_count,
        });
        true
    }

    fn clear_pending_resolve(&mut self) {
        self.pending_resolve = None;
    }

    fn mark_external_merge_auto_opened(&mut self, path: impl Into<String>) -> bool {
        self.external_merge_auto_opened.insert(path.into())
    }

    fn prune_external_merge_auto_opened(&mut self, conflict_paths: &[String]) {
        self.external_merge_auto_opened
            .retain(|path| conflict_paths.iter().any(|candidate| candidate == path));
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WorkflowState {
    pub(crate) definition: Option<khaslana::WorkflowDefinition>,
    pub(crate) preview: Option<khaslana::WorkflowPreview>,
    pub(crate) file_path: Option<PathBuf>,
    pub(crate) inputs: Vec<WorkflowInputFieldState>,
    pub(crate) selected_template_path: Option<PathBuf>,
    pub(crate) log: Vec<WorkflowLogEntry>,
}

fn sync_conflict_state_from_paths(
    main_mode: &mut MainMode,
    state: &mut ConflictWorkbenchState,
    conflict_paths: &[String],
    auto_open_conflict_mode: bool,
) {
    let entering_conflicts =
        !conflict_paths.is_empty() && state.selected_path.is_none() && state.files.is_empty();
    state
        .files
        .retain(|path, _| conflict_paths.iter().any(|candidate| candidate == path));
    state.prune_external_merge_auto_opened(conflict_paths);
    if state
        .pending_resolve
        .as_ref()
        .is_some_and(|pending| !conflict_paths.iter().any(|path| path == &pending.path))
    {
        state.pending_resolve = None;
    }

    if conflict_paths.is_empty() {
        *state = ConflictWorkbenchState::default();
        if *main_mode == MainMode::Conflict {
            *main_mode = MainMode::Worktree;
        }
        return;
    }

    if entering_conflicts {
        *main_mode = if auto_open_conflict_mode {
            MainMode::Conflict
        } else {
            // 普通合并冲突先在工作区展示合并状态，不主动打开冲突工作台。
            MainMode::Worktree
        };
    }
    if state
        .selected_path
        .as_ref()
        .is_none_or(|path| !conflict_paths.iter().any(|candidate| candidate == path))
    {
        state.selected_path = conflict_paths.first().cloned();
        state.selected_block = 0;
        state.show_base = false;
        state.pending_resolve = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RepoTabId(u64);

/// 浏览视图的模式：显示目标分支文件的原始内容，或与当前 HEAD 的差异。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum BrowseViewMode {
    #[default]
    Content,
    Diff,
}

/// 文件追溯视图的 per-repository 状态。
///
/// 进入追溯视图时记录目标路径并后台加载 `BlameView`；
/// 切换到其他主模式再切回时状态保留。
#[derive(Clone, Debug, Default)]
pub(crate) struct BlameState {
    /// 当前追溯的文件（git 风格相对路径）。
    pub path: Option<String>,
    pub loading: bool,
    /// 追溯数据，后台加载完成后填充。
    pub view: Option<Arc<BlameView>>,
    /// 内容列的语法高亮（索引与 view.lines 对齐；未提交行渲染时不使用）。
    pub syntax: Option<Arc<SharedSyntaxSpans>>,
    /// 内容视图最宽行扫描缓存：((Arc 地址, 行数), 最宽行索引)。
    /// 与 BrowseState::widest_line_cache 同一套模式，内容未变时每帧免扫描。
    pub widest_line_cache: RefCell<Option<((usize, usize), Option<usize>)>>,
}

impl BlameState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// 释放超大缓存，避免切仓库后内存占用过高。
    fn release_large_caches(&mut self) {
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.lines.len() > LARGE_DIFF_CACHE_LINE_LIMIT)
        {
            self.view = None;
        }
    }
}

/// 一次分支谱系追踪结果：高亮 OID 集合 + 是否截断。
/// 参数一致性由 `trace_seq` 代际守卫保证（分支/模式变化必先递增并清空旧集合）。
#[derive(Clone, Debug)]
pub(crate) struct CommitTrace {
    pub oids: Arc<HashSet<String>>,
    pub truncated: bool,
}

/// 提交图谱页的 per-repository 状态。
///
/// 切换模式（含经「在提交记录页查看」跳去主历史页再返回）**不重置**：
/// 高亮分支、开关、详情卡折叠与滚动位置全部保留（跳转无损往返的关键）；
/// 仅随 tab 销毁自然释放。搜索词在 RepositoryView 上全局共享（见
/// `commit_graph_search`），同样跨模式保留。
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitGraphState {
    /// 高亮追踪的本地分支名；None = 未启用高亮。
    pub highlight_branch: Option<String>,
    /// 高亮模式：true = 仅领先 HEAD 的提交（增量动向），false = 分支全谱系。
    pub highlight_ahead_only: bool,
    /// 淡化合并提交开关（未启用高亮时生效；高亮激活时谱系外一律淡化）。
    pub dim_merges: bool,
    /// 分支高亮下拉菜单展开状态。
    pub branch_menu_open: bool,
    /// 已加载的高亮 OID 集（计算中为 None，避免全表闪烁淡化）。
    pub trace: Option<CommitTrace>,
    /// 谱系请求代际：参数变化即递增，旧一代晚到的结果丢弃。
    pub trace_seq: u64,
    /// 谱系后台计算中（工具行提示）。
    pub trace_loading: bool,
    /// 详情卡折叠状态。
    pub details_collapsed: bool,
}

/// AI 评审历史弹窗状态（None = 关闭）。
pub(crate) struct AiReviewHistoryState {
    /// 后台加载记录中。
    pub loading: bool,
    pub records: Vec<AiReviewRecord>,
    pub error: Option<String>,
}

/// AI 思考弹窗状态：一次性 AI 请求（commit message / 冲突合并建议 /
/// 工作流模板生成）进行中的流式展示。思维链与正文增量实时追加，
/// 任务完成或失败后由对应事件处理关闭弹窗。
pub(crate) struct AiThinkingOverlayState {
    /// 弹窗标题（说明正在生成的业务，如「正在生成提交信息」）。
    pub title: String,
    /// 思维链流式累积文本。
    pub reasoning: String,
    /// 正文流式累积文本（非 reasoning 模型没有思维链，正文增量是
    /// 唯一的进度反馈）。
    pub content: String,
}

/// AI 思考弹窗的钉底跟随状态：prepaint 期按内容长度变化键门控滚动
/// （键不变不回弹，用户滚动不被抢夺）。Rc 共享给渲染闭包，弹窗打开时
/// 复位以强制首帧钉底。
pub(crate) struct AiThinkingFollowState {
    pub last_key: std::cell::Cell<(usize, usize)>,
}

/// 分支浏览模式的 per-repository 状态。
///
/// 维护已加载的文件树（按目录懒加载）、展开/选中状态，以及当前文件的只读内容或差异。
/// 切换到其他主模式再切回时状态保留，可直接回到上次位置。
#[derive(Clone, Debug, Default)]
pub(crate) struct BrowseState {
    /// 当前浏览的目标引用（显示名 + tip commit OID）。
    pub target: Option<BrowseTarget>,
    /// 左侧列表模式：完整文件树或仅差异文件。
    pub list_mode: BrowseListMode,
    /// 已加载的各目录条目，key 为 git 风格相对路径（根为 ""）。
    pub entries_by_dir: HashMap<PathBuf, Vec<BrowseEntry>>,
    /// 当前展开的目录路径集合。
    pub expanded: HashSet<PathBuf>,
    /// 当前选中的文件路径。
    pub selected_file: Option<PathBuf>,
    /// 比较模式下的差异文件列表。
    pub compare_files: Vec<BrowseCompareFile>,
    /// 比较模式下差异文件树的展开目录集合（git 风格相对路径）。
    /// 空集合表示默认全部展开；用户首次折叠时固化为显式集合。
    pub compare_expanded: HashSet<String>,
    /// 比较模式下当前选中的差异文件元数据。
    pub selected_compare_file: Option<BrowseCompareFile>,
    /// 只读内容视图的数据。
    pub content: Option<Arc<BrowseFileContent>>,
    /// 内容视图的语法高亮（索引与 content.lines 对齐）。
    pub content_syntax: Option<Arc<SharedSyntaxSpans>>,
    /// 与 HEAD 的差异。
    pub diff: Option<Arc<FileDiff>>,
    /// 差异视图的语法高亮（仅全文模式计算；索引与 diff.lines 对齐）。
    pub diff_syntax: Option<Arc<SharedSyntaxSpans>>,
    /// 当前视图模式。
    pub view_mode: BrowseViewMode,
    /// 差异头部是否展开。
    pub diff_headers_expanded: bool,
    pub loading_tree: bool,
    pub compare_loading: bool,
    pub loading_content: bool,
    pub loading_diff: bool,
    // 行级文本选区（拖选 + Ctrl+C / Ctrl+A）。
    pub selecting: bool,
    pub sel_start: Option<usize>,
    pub sel_end: Option<usize>,
    /// 内容视图最宽行扫描缓存：((Arc 地址, 行数), 最宽行索引)。
    /// 内容未变时每帧免 O(总字符) 扫描；行数参与比较可排除 Arc 地址复用误命中。
    pub widest_line_cache: RefCell<Option<((usize, usize), Option<usize>)>>,
}

impl BrowseState {
    /// 重置为初始状态（保留默认 view_mode）。
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// 根据当前路径返回目录的 git 风格 key（根为 ""）。
    fn dir_key(path: &Path) -> PathBuf {
        if path.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            path.to_path_buf()
        }
    }

    /// 当前行是否在选区内（sel_start..=sel_end，顺序无关）。
    fn is_row_selected(&self, index: usize) -> bool {
        match (self.sel_start, self.sel_end) {
            (Some(start), Some(end)) => {
                let (lo, hi) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                index >= lo && index <= hi
            }
            _ => false,
        }
    }

    /// 释放超大缓存，避免切仓库后内存占用过高。
    fn release_large_caches(&mut self) {
        if self
            .content
            .as_ref()
            .is_some_and(|content| content.lines.len() > LARGE_DIFF_CACHE_LINE_LIMIT)
        {
            self.content = None;
        }
        if self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.lines.len() > LARGE_DIFF_CACHE_LINE_LIMIT)
        {
            self.diff = None;
            self.diff_headers_expanded = false;
        }
    }
}

#[derive(Clone, Debug)]
struct RepoTabState {
    pub(crate) id: RepoTabId,
    pub(crate) repo_path: Option<PathBuf>,
    pub(crate) snapshot: Option<RepositorySnapshot>,
    pub(crate) selected_branch: Option<String>,
    pub(crate) selected_remote: Option<String>,
    pub(crate) change_selection: ChangeSelection,
    pub(crate) change_indexes: ChangeListIndexes,
    pub(crate) diff: Option<Arc<FileDiff>>,
    /// 工作区差异的语法高亮（仅全文模式计算；索引与 diff.lines 对齐）。
    pub(crate) diff_syntax: Option<Arc<SharedSyntaxSpans>>,
    pub(crate) diff_headers_expanded: bool,
    /// 工作区差异的按行选择（diff 行索引；仅 Added/Removed 行参与部分暂存，
    /// 范围选择中的上下文行在转换为行号选择时被忽略）。
    pub(crate) diff_line_selection: BTreeSet<usize>,
    diff_line_selection_anchor: Option<usize>,
    pub(crate) main_mode: MainMode,
    pub(crate) workflow_state: WorkflowState,
    pub(crate) history_commits: Vec<CommitInfo>,
    pub(crate) history_has_more: bool,
    pub(crate) history_selected_commit: Option<String>,
    pub(crate) history_files: Vec<CommitFileChange>,
    pub(crate) history_selected_file: Option<String>,
    pub(crate) history_diff: Option<Arc<FileDiff>>,
    /// 历史差异的语法高亮（仅全文模式计算；索引与 history_diff.lines 对齐）。
    pub(crate) history_diff_syntax: Option<Arc<SharedSyntaxSpans>>,
    pub(crate) history_diff_headers_expanded: bool,
    pub(crate) history_loading: HistoryLoading,
    /// 刷新历史时保留旧列表可见，等新数据就绪后直接替换
    pub(crate) history_refreshing: bool,
    pub(crate) history_scope: HistoryScope,
    /// 历史页的文件路径过滤：只显示改动过该文件的提交。
    /// 用户意图，`clear_history` 不清除（切 scope/切分支/刷新均保留），
    /// 仅显式点 chip 的 × 清除；随 tab 销毁自然释放。
    pub(crate) history_file_filter: Option<String>,
    pub(crate) history_refs_cache: Option<HistoryRefsCache>,
    /// 提交列表请求序号：每次发起加载递增，用于丢弃旧一代请求晚到的结果
    pub(crate) history_load_seq: u64,
    pub(crate) history_graph_rows: Vec<commit_graph_view::CommitGraphRow>,
    pub(crate) stash_preview: StashPreviewState,
    // 提交图谱页状态（模式切换不重置，跨页跳转无损保留）
    pub(crate) commit_graph: CommitGraphState,
    pub(crate) branch_sync_status: Option<BranchSyncStatus>,
    pub(crate) branch_sync_loading: bool,
    pub(crate) branch_sync_request_id: u64,
    pub(crate) submodule_dialog: SubmoduleDialogState,
    pub(crate) conflict_workbench: ConflictWorkbenchState,
    pub(crate) sidebar_sections: SidebarSectionState,
    // 是否以“全文视图”展示差异：开启后 diff 上下文行数拉满，展示整份文件并保留增删行高亮
    pub(crate) full_file_view: bool,
    // 分支浏览模式状态
    pub(crate) browse: BrowseState,
    // 文件追溯视图状态
    pub(crate) blame: BlameState,
    pub(crate) busy: bool,
    pub(crate) operation_blocker: OperationBlocker,
    /// 操作遮罩层开始时间；用于延迟显示遮罩层，避免快速完成时一闪而过。
    pub(crate) operation_blocker_started: Option<Instant>,
    operation_kind: OperationKind,
    pub(crate) loading: RepositoryLoading,
    pub(crate) repository_load_id: u64,
    pub(crate) status: String,
    pub(crate) last_error: Option<String>,
    /// 最后活动/打开时间（Unix 秒），用于仓库切换下拉排序。
    pub(crate) last_active_at: i64,
}

impl RepoTabState {
    fn new(id: RepoTabId, repo_path: Option<PathBuf>) -> Self {
        Self {
            id,
            repo_path,
            snapshot: None,
            selected_branch: None,
            selected_remote: None,
            change_selection: ChangeSelection::default(),
            change_indexes: ChangeListIndexes::default(),
            diff: None,
            diff_syntax: None,
            diff_headers_expanded: false,
            diff_line_selection: BTreeSet::new(),
            diff_line_selection_anchor: None,
            main_mode: MainMode::Worktree,
            workflow_state: WorkflowState::default(),
            history_commits: Vec::new(),
            history_has_more: false,
            history_selected_commit: None,
            history_files: Vec::new(),
            history_selected_file: None,
            history_diff: None,
            history_diff_syntax: None,
            history_diff_headers_expanded: false,
            history_loading: HistoryLoading::default(),
            history_refreshing: false,
            history_scope: HistoryScope::default(),
            history_file_filter: None,
            history_refs_cache: None,
            history_load_seq: 0,
            history_graph_rows: Vec::new(),
            stash_preview: StashPreviewState::default(),
            commit_graph: CommitGraphState::default(),
            branch_sync_status: None,
            branch_sync_loading: false,
            branch_sync_request_id: 0,
            submodule_dialog: SubmoduleDialogState::default(),
            conflict_workbench: ConflictWorkbenchState::default(),
            sidebar_sections: SidebarSectionState::default(),
            full_file_view: false,
            browse: BrowseState::default(),
            blame: BlameState::default(),
            busy: false,
            operation_blocker: OperationBlocker::None,
            operation_blocker_started: None,
            operation_kind: OperationKind::Local,
            loading: RepositoryLoading::default(),
            repository_load_id: 0,
            status: "就绪".to_string(),
            last_error: None,
            last_active_at: now_epoch_secs(),
        }
    }

    fn display_name(&self) -> String {
        self.repo_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "未命名仓库".to_string())
    }

    fn path_key(&self) -> Option<String> {
        self.repo_path
            .as_ref()
            .map(|path| normalize_repo_path(path))
    }

    fn release_large_diff_caches(&mut self) {
        if self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.lines.len() > LARGE_DIFF_CACHE_LINE_LIMIT)
        {
            self.diff = None;
            self.diff_headers_expanded = false;
        }
        if self
            .history_diff
            .as_ref()
            .is_some_and(|diff| diff.lines.len() > LARGE_DIFF_CACHE_LINE_LIMIT)
        {
            self.history_diff = None;
            self.history_diff_headers_expanded = false;
        }
        self.browse.release_large_caches();
        self.blame.release_large_caches();
    }

    /// 历史提交事件的应用守卫：load_id、scope 与路径过滤均与当前状态一致才落地。
    /// 抽成独立方法便于单测（过滤切换后旧请求晚到的结果不覆盖新数据）。
    fn history_commits_event_matches(
        &self,
        load_id: u64,
        scope: HistoryScope,
        path_filter: Option<&str>,
    ) -> bool {
        load_id == self.repository_load_id
            && scope == self.history_scope
            && path_filter == self.history_file_filter.as_deref()
    }

    /// 清空历史页的列表与选中状态。
    ///
    /// 注意：不清 `history_file_filter`——过滤器是用户意图，
    /// 切 scope/切分支/刷新均保留，仅显式点 chip 的 × 清除。
    fn clear_history(&mut self) {
        self.history_commits.clear();
        self.history_has_more = false;
        self.history_selected_commit = None;
        self.history_files.clear();
        self.history_selected_file = None;
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
        self.history_loading = HistoryLoading::default();
        self.history_refs_cache = None;
        self.history_graph_rows.clear();
        self.history_refreshing = false;
    }
}

/// HistoryFilesLoaded 自动选中的文件：默认取首个；过滤模式下若列表
/// 包含被过滤的路径则优先选它（提交差异立即可见）。
fn preferred_history_file(filter: Option<&str>, files: &[CommitFileChange]) -> Option<String> {
    filter
        .filter(|filter| files.iter().any(|file| file.path.as_str() == *filter))
        .map(str::to_string)
        .or_else(|| files.first().map(|file| file.path.clone()))
}

/// 未跟踪文件差异的展示行类型：整份文件以「新增」行输出，但渲染时
/// 白底显示（SourceTree 式，不标绿）——映射为 Context 的配色。
/// 仅影响显示，部分暂存等服务侧行为仍按原始 Added kind 判断。
fn display_diff_line_kind(kind: DiffLineKind, untracked: bool) -> DiffLineKind {
    if untracked && kind == DiffLineKind::Added {
        DiffLineKind::Context
    } else {
        kind
    }
}

#[derive(Clone, Copy, Debug)]
struct ResizeState {
    start_x: f32,
    start_y: f32,
    start_width: f32,
    start_height: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DiffCacheKind {
    Worktree { scope: DiffScope, path: String },
    History { commit_oid: String, path: String },
    Stash { stash_oid: String, path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DiffCacheKey {
    repo_key: String,
    load_id: u64,
    encoding: DiffEncodingChoice,
    kind: DiffCacheKind,
    // 全文视图与紧凑差异分别缓存，互不污染
    full_file: bool,
}

#[derive(Clone, Debug)]
struct RepositoryLoadRequest {
    tab_id: RepoTabId,
    path: PathBuf,
    started: &'static str,
    finished: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LoadPriority {
    Background,
    User,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OperationKind {
    #[default]
    Local,
    Network,
    LongRunning,
}

impl OperationKind {
    fn from_message(message: &str) -> Self {
        if message.contains("拉取")
            || message.contains("推送")
            || message.contains("克隆")
            || message.contains("刷新仓库")
            || message.contains("远端")
            || message.contains("凭据连接")
        {
            Self::Network
        } else if message.contains("工作流") {
            Self::LongRunning
        } else {
            Self::Local
        }
    }

    fn shows_progress(self) -> bool {
        matches!(self, Self::Network | Self::LongRunning)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeTarget {
    Sidebar,
    Changes,
    WorkflowTemplates,
    HistoryFiles,
    HistoryInspectorFiles,
    HistoryDetails,
    HistoryGraph,
    BrowseFiles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MainMode {
    Worktree,
    Conflict,
    History,
    Workflow,
    Stash,
    Browse,
    Blame,
    /// 提交图谱页（专用模式）：拓扑专注型，主历史页「图谱」按钮进入，
    /// 关闭/跳转返回 History。切换模式不重置图谱状态（无损往返）。
    CommitGraph,
}

/// Context Navigator 偏好：单一展开状态跨工作区/历史/工作流/图谱与**所有仓库**
/// 共享（存于 RepositoryView，非 per-tab），切换模式或切换仓库都不改变展开/收起，
/// 并经 layout_preferences 持久化、重启恢复。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextNavigatorPreferences {
    visible: bool,
}

impl Default for ContextNavigatorPreferences {
    fn default() -> Self {
        Self { visible: true }
    }
}

impl ContextNavigatorPreferences {
    pub(crate) const fn is_visible(self, mode: MainMode) -> bool {
        match mode {
            MainMode::Worktree | MainMode::History | MainMode::Workflow => self.visible,
            MainMode::Conflict
            | MainMode::Stash
            | MainMode::Browse
            | MainMode::Blame
            | MainMode::CommitGraph => false,
        }
    }

    pub(crate) fn toggle(&mut self, mode: MainMode) {
        match mode {
            MainMode::Worktree | MainMode::History | MainMode::Workflow => {
                self.visible = !self.visible
            }
            MainMode::Conflict
            | MainMode::Stash
            | MainMode::Browse
            | MainMode::Blame
            | MainMode::CommitGraph => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarSection {
    LocalBranches,
    Remotes,
    RemoteBranches,
    Tags,
    Stashes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidebarSectionState {
    local_branches: bool,
    remotes: bool,
    remote_branches: bool,
    tags: bool,
    stashes: bool,
}

impl Default for SidebarSectionState {
    fn default() -> Self {
        Self {
            local_branches: true,
            remotes: false,
            remote_branches: false,
            tags: false,
            stashes: false,
        }
    }
}

impl SidebarSectionState {
    pub(crate) fn is_expanded(self, section: SidebarSection) -> bool {
        match section {
            SidebarSection::LocalBranches => self.local_branches,
            SidebarSection::Remotes => self.remotes,
            SidebarSection::RemoteBranches => self.remote_branches,
            SidebarSection::Tags => self.tags,
            SidebarSection::Stashes => self.stashes,
        }
    }

    fn toggle(&mut self, section: SidebarSection) {
        match section {
            SidebarSection::LocalBranches => self.local_branches = !self.local_branches,
            SidebarSection::Remotes => self.remotes = !self.remotes,
            SidebarSection::RemoteBranches => self.remote_branches = !self.remote_branches,
            SidebarSection::Tags => self.tags = !self.tags,
            SidebarSection::Stashes => self.stashes = !self.stashes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffHeaderTarget {
    Worktree,
    History,
    Stash,
    Browse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HistoryLoading {
    commits: bool,
    files: bool,
    diff: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepositoryLoading {
    metadata: bool,
    status_fast: bool,
    status_full: bool,
}

impl RepositoryLoading {
    pub(crate) fn remote(self) -> bool {
        self.metadata
    }

    fn unstaged(self) -> bool {
        self.status_fast || self.status_full
    }

    fn staged(self) -> bool {
        self.status_fast
    }
}

#[derive(Clone, Debug)]
pub(crate) enum UiEvent {
    UiTick,
    OperationStarted {
        tab_id: Option<RepoTabId>,
        message: String,
    },
    OperationProgress {
        tab_id: Option<RepoTabId>,
        message: String,
    },
    RepositoryFastLoaded {
        tab_id: RepoTabId,
        message: String,
        snapshot: RepositorySnapshot,
        load_id: u64,
    },
    RepositoryMetadataLoaded {
        tab_id: RepoTabId,
        message: String,
        snapshot: RepositorySnapshot,
        load_id: u64,
    },
    RepositoryStatusFastLoaded {
        tab_id: RepoTabId,
        message: String,
        changes: Vec<khaslana::WorktreeChange>,
        load_id: u64,
    },
    RepositoryStatusFullLoaded {
        tab_id: RepoTabId,
        message: String,
        changes: Vec<khaslana::WorktreeChange>,
        load_id: u64,
    },
    RepositoryLoadStageFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
    },
    RepositoryLoadFinished {
        tab_id: RepoTabId,
        load_id: u64,
    },
    OperationFinished {
        tab_id: Option<RepoTabId>,
        message: String,
        snapshot: Option<RepositorySnapshot>,
        diff: Option<FileDiff>,
    },
    DiscardChangeFinished {
        tab_id: RepoTabId,
        message: String,
        snapshot: RepositorySnapshot,
        changes: Vec<khaslana::WorktreeChange>,
        load_id: u64,
    },
    HistoryCommitsLoaded {
        tab_id: RepoTabId,
        commits: Vec<CommitInfo>,
        refs_cache: HistoryRefsCache,
        append: bool,
        has_more: bool,
        scope: HistoryScope,
        /// 发起请求时生效的文件路径过滤；与当前过滤器比对，
        /// 防止切换过滤后旧请求的结果覆盖新数据。
        path_filter: Option<String>,
        load_id: u64,
        seq: u64,
    },
    HistoryFilesLoaded {
        tab_id: RepoTabId,
        commit_oid: String,
        files: Vec<CommitFileChange>,
        load_id: u64,
    },
    HistoryDiffLoaded {
        tab_id: RepoTabId,
        commit_oid: String,
        path: String,
        diff: FileDiff,
        load_id: u64,
    },
    StashFilesLoaded {
        tab_id: RepoTabId,
        stash_oid: String,
        files: Vec<khaslana::StashFileChange>,
        load_id: u64,
    },
    StashDiffLoaded {
        tab_id: RepoTabId,
        stash_oid: String,
        path: String,
        diff: FileDiff,
        load_id: u64,
    },
    HistoryLoadFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
    },
    // 提交图谱页：分支谱系高亮集合计算完成（seq 代际守卫，参数变化即作废旧结果）
    CommitTraceLoaded {
        tab_id: RepoTabId,
        branch: String,
        ahead_only: bool,
        oids: Vec<String>,
        truncated: bool,
        load_id: u64,
        seq: u64,
    },
    // 提交图谱页：分支谱系计算失败
    CommitTraceLoadFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
        seq: u64,
    },
    BranchSyncStatusLoaded {
        tab_id: RepoTabId,
        status: Option<BranchSyncStatus>,
        load_id: u64,
        request_id: u64,
    },
    BranchSyncStatusFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
        request_id: u64,
    },
    SubmodulesLoaded {
        tab_id: RepoTabId,
        items: Vec<SubmoduleInfo>,
        load_id: u64,
        request_id: u64,
    },
    SubmodulesLoadFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
        request_id: u64,
    },
    SubmoduleRemoteStatusesLoaded {
        tab_id: RepoTabId,
        statuses: Vec<(String, SubmoduleRemoteSyncStatus)>,
        load_id: u64,
        request_id: u64,
    },
    SubmoduleRemoteStatusesLoadFailed {
        tab_id: RepoTabId,
        error: String,
        load_id: u64,
        request_id: u64,
    },
    // 文件追溯视图：后台加载完成
    BlameLoaded {
        tab_id: RepoTabId,
        path: String,
        view: khaslana::BlameView,
        load_id: u64,
    },
    // 文件追溯视图：后台加载失败
    BlameLoadFailed {
        tab_id: RepoTabId,
        path: String,
        error: String,
        load_id: u64,
    },
    // 语法高亮：后台补算完成（Arc 槽位，anchor = 源内容 Arc 地址 + 行数防复用误命中）
    SyntaxHighlighted {
        tab_id: RepoTabId,
        slot: SyntaxSlot,
        anchor: usize,
        anchor_len: usize,
        spans: Option<Arc<SharedSyntaxSpans>>,
    },
    // 语法高亮：冲突工作台分栏补算完成（draft 带 seq 防乱序）
    ConflictSyntaxHighlighted {
        tab_id: RepoTabId,
        path: String,
        pane: ConflictSyntaxPane,
        seq: u64,
        spans: Option<Arc<SharedSyntaxSpans>>,
    },
    // 分支浏览模式：目标引用解析完成
    BrowseTargetResolved {
        tab_id: RepoTabId,
        target: BrowseTarget,
        load_id: u64,
    },
    // 分支浏览模式：目录树加载完成
    BrowseTreeLoaded {
        tab_id: RepoTabId,
        dir_path: PathBuf,
        entries: Vec<BrowseEntry>,
        load_id: u64,
    },
    // 分支比较模式：差异文件列表加载完成
    BrowseCompareFilesLoaded {
        tab_id: RepoTabId,
        target_oid: String,
        files: Vec<BrowseCompareFile>,
        load_id: u64,
    },
    // 分支浏览模式：文件只读内容加载完成
    BrowseFileContentLoaded {
        tab_id: RepoTabId,
        path: String,
        content: BrowseFileContent,
        load_id: u64,
    },
    // 分支浏览模式：文件与 HEAD 差异加载完成
    BrowseFileDiffLoaded {
        tab_id: RepoTabId,
        path: String,
        diff: FileDiff,
        load_id: u64,
    },
    OperationFailed {
        tab_id: Option<RepoTabId>,
        error: String,
    },
    CredentialRecordsLoaded {
        records: Vec<CredentialRecord>,
        message: String,
    },
    CredentialRequested {
        tab_id: Option<RepoTabId>,
        request: CredentialRequest,
        response_tx: Arc<Mutex<Option<mpsc::Sender<khaslana::Result<Option<GitCredential>>>>>>,
    },
    SshCredentialsDiscovered {
        request_id: u64,
        result: SshDiscoveryResult,
    },
    SshCredentialDiscoveryFailed {
        request_id: u64,
        error: String,
    },
    CredentialSshKeyFileSelected {
        path: Option<PathBuf>,
    },
    ProxyTestFinished {
        message: String,
    },
    WorkflowProgress {
        tab_id: RepoTabId,
        entry: WorkflowLogEntry,
    },
    WorkflowFinished {
        tab_id: RepoTabId,
        message: String,
        snapshot: RepositorySnapshot,
        log: Vec<WorkflowLogEntry>,
    },
    /// 工作流模板目录后台刷新结果（目录 IO/JSON5 解析不占 UI 线程）。
    WorkflowTemplatesLoaded {
        result: Result<Vec<WorkflowTemplateItem>, String>,
    },
    OpenRepositoryFolderSelected {
        path: Option<PathBuf>,
    },
    CloneTargetFolderSelected {
        path: Option<PathBuf>,
    },
    ExternalMergeExecutableSelected {
        path: Option<PathBuf>,
    },
    AiCommitMessageGenerated {
        message: String,
    },
    /// 工作流模板 AI 生成/编辑完成（JSON5 文本，经编辑器解析回填表单）。
    AiWorkflowTemplateGenerated {
        content: String,
    },
    AiReviewGenerated {
        /// 任务代际：与 `ai_review_active_generation` 匹配才应用到面板，
        /// 不匹配说明 UI 已分离（切目标/取消），仅做后台完成提示。
        generation: u64,
        review: AiReviewResult,
        /// 记录是否成功落盘到评审记录目录。
        saved: bool,
    },
    /// agent 评审过程新增一个步骤（思维链或工具调用完成）。
    AiReviewStepAdded {
        generation: u64,
        step: AiReviewStep,
    },
    /// agent 评审的进度文案更新（如「第 2 轮 · 已执行工具 5 次」）。
    AiReviewProgress {
        generation: u64,
        message: String,
    },
    /// agent 评审当前轮的流式增量（正文/思考链），驱动时间线 live 区。
    AiReviewDelta {
        generation: u64,
        content_delta: Option<String>,
        reasoning_delta: Option<String>,
    },
    /// agent 评审失败（从共用的 AiRequestFailed 拆出：携带代际，旧任务的
    /// 失败不会误复位新任务的状态）。
    AiReviewFailed {
        generation: u64,
        error: String,
    },
    /// agent 评审被取消后在轮次边界退出（UI 已在取消时复位，这里只做
    /// 在途任务计数归位；无需携带代际，取消只影响计数）。
    AiReviewCancelled,
    /// 评审历史记录加载完成（历史弹窗）。
    AiReviewHistoryLoaded {
        records: Vec<AiReviewRecord>,
    },
    /// 评审历史记录加载失败。
    AiReviewHistoryLoadFailed {
        error: String,
    },
    AiConflictMergeProgress {
        path: String,
        segment: usize,
        total: usize,
    },
    AiConflictMergeGenerated {
        path: String,
        draft: String,
    },
    /// AI 思考弹窗的流式增量（思维链/正文），由公共执行器转发；
    /// `content_delta` 为 None 表示本片是思维链。
    AiThinkingDelta {
        content_delta: Option<String>,
        reasoning_delta: String,
    },
    AiRequestFailed {
        error: String,
    },
    AiConnectionTested {
        message: String,
    },
    // ── 更新事件 ──
    UpdateCheckFinished {
        manifest: Arc<UpdateManifest>,
        asset: UpdatePlatformAsset,
    },
    UpdateCheckFailed {
        error: String,
        /// 手动检查（设置页「立即检查」）：结果弹气泡；自动检查保持安静
        ///（每次启动都弹「已是最新」会打扰）。
        manual: bool,
    },
    UpdateDownloadProgress {
        downloaded: u64,
        total: u64,
    },
    UpdateReadyToInstall {
        staging_dir: PathBuf,
        manifest: Arc<UpdateManifest>,
    },
    UpdateInstallFailed {
        error: String,
    },
    // ── 后台任务异常兜底 ──
    /// 后台任务 panic（TaskExecutor catch_unwind 捕获）。
    /// rayon 会静默吞掉 panic，若不兜底，对应 tab 的 busy/加载标志和仓库
    /// 加载槽位会永久卡死；UI 收到此事件后统一复位。
    BackgroundTaskPanicked {
        message: String,
    },
    /// 修补开关的 HEAD 提交信息预填结果（历史未加载时由后台任务读取）。
    AmendPrefillLoaded {
        tab_id: RepoTabId,
        message: Option<String>,
    },
    // ── OAuth 快速登录（GitHub Device Flow / Gitee 授权码流）──
    OAuthLoginReady {
        request_id: u64,
        provider: OAuthProvider,
        /// 浏览器要打开的地址（GitHub：设备验证页；Gitee：授权页）。
        url: String,
        /// GitHub 的用户验证码（Gitee 为 None）。
        user_code: Option<String>,
    },
    OAuthLoginSucceeded {
        request_id: u64,
        provider: OAuthProvider,
        username: String,
        token: String,
        /// Gitee 专属：自动续期材料（refresh_token + 过期时间）；GitHub/旧 broker 为 None。
        gitee_refresh: Option<(String, i64)>,
    },
    OAuthLoginFailed {
        request_id: u64,
        error: String,
    },
    // Gitee 令牌自动续期结果（后台任务线程回传，成功仅状态栏、失败加 toast）
    GiteeTokenRefreshed {
        success: bool,
        message: String,
    },
}

#[derive(Clone)]
struct TabProgress {
    pub(crate) tx: Sender<UiEvent>,
    tab_id: RepoTabId,
}

impl ProgressEmitter for TabProgress {
    fn emit(&self, event: OperationEvent) {
        let event = match event {
            OperationEvent::Started(message) => UiEvent::OperationStarted {
                tab_id: Some(self.tab_id),
                message,
            },
            OperationEvent::Progress(message) => UiEvent::OperationProgress {
                tab_id: Some(self.tab_id),
                message,
            },
            OperationEvent::Finished(message) => UiEvent::OperationProgress {
                tab_id: Some(self.tab_id),
                message,
            },
        };
        send_ui_event(&self.tx, event);
    }
}

#[derive(Clone)]
struct TabCredentialProvider {
    store: Arc<dyn khaslana::CredentialStore>,
    storage: Arc<khaslana::AppStorage>,
    remote_bindings: Arc<Mutex<RemoteCredentialBindings>>,
    tx: Sender<UiEvent>,
    rejected_record_ids: Arc<Mutex<Vec<String>>>,
    last_stored_attempt: Arc<Mutex<Option<StoredCredentialAttempt>>>,
    tab_id: RepoTabId,
    /// Gitee 令牌自动续期用的代理设置（刷新经 broker，与 git 操作同一代理策略）。
    proxy_settings: NetworkProxySettings,
}

const STORED_CREDENTIAL_REUSE_LIMIT_PER_OPERATION: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredCredentialAttempt {
    url: String,
    record_id: String,
    operation_id: Option<u64>,
    repo_path: Option<PathBuf>,
    remote_name: Option<String>,
    use_count: usize,
}

impl StoredCredentialAttempt {
    fn from_request(request: &CredentialRequest, record_id: String) -> Self {
        Self {
            url: request.url.clone(),
            record_id,
            operation_id: request.operation_id,
            repo_path: request.repo_path.clone(),
            remote_name: request.remote_name.clone(),
            use_count: 1,
        }
    }

    fn is_retry_for(&self, request: &CredentialRequest) -> bool {
        self.operation_id.is_some()
            && self.operation_id == request.operation_id
            && self.url == request.url
            && self.repo_path == request.repo_path
            && self.remote_name == request.remote_name
    }

    fn mark_used_again(&mut self) {
        self.use_count = self.use_count.saturating_add(1);
    }
}

impl TabCredentialProvider {
    fn new(
        store: Arc<dyn khaslana::CredentialStore>,
        storage: Arc<khaslana::AppStorage>,
        remote_bindings: Arc<Mutex<RemoteCredentialBindings>>,
        tx: Sender<UiEvent>,
        tab_id: RepoTabId,
        proxy_settings: NetworkProxySettings,
    ) -> Self {
        Self {
            store,
            storage,
            remote_bindings,
            tx,
            rejected_record_ids: Arc::new(Mutex::new(Vec::new())),
            last_stored_attempt: Arc::new(Mutex::new(None)),
            tab_id,
            proxy_settings,
        }
    }

    /// 是否为 Gitee 的 HTTPS 凭据记录（自动续期只对 Gitee OAuth 令牌生效）。
    /// `record.host` 是 host_key 形态（协议 + 小写主机，如 `https://gitee.com`）。
    fn is_gitee_https_record(record: &khaslana::credentials::CredentialRecord) -> bool {
        record.host == "https://gitee.com"
    }

    /// Gitee OAuth 令牌惰性续期：命中已存凭据时检查过期时间，距过期不足
    /// 提前量（或已过期）则经 broker 刷新，成功后把新令牌写回 Keyring 并
    /// 返回续期后的凭据；失败时沿用旧令牌（认证若失败会走正常的凭据
    /// 重试/提示流程），仅 toast 提示重新登录。
    fn maybe_refresh_gitee_token(
        &self,
        stored: &khaslana::credentials::StoredCredential,
    ) -> GitCredential {
        if !Self::is_gitee_https_record(&stored.record)
            || stored.record.kind != khaslana::StoredCredentialKind::HttpsUserPass
            || !matches!(stored.credential, GitCredential::UserPass { .. })
        {
            return stored.credential.clone();
        }
        let Some(payload) = khaslana::credentials::load_gitee_refresh_payload(&stored.record.id)
        else {
            return stored.credential.clone();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if !oauth::gitee_needs_refresh(payload.expires_at, now) {
            return stored.credential.clone();
        }

        // single-flight：凭据回调可能同时发生在多个 long 线程（多仓库并发
        // fetch/push），双发同一 refresh_token 会浪费一次轮换甚至双双失败；
        // 已有刷新在途时本次直接沿用旧令牌（下次操作会再触发）。
        static GITEE_REFRESH_IN_FLIGHT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let Ok(_flight_guard) = GITEE_REFRESH_IN_FLIGHT.try_lock() else {
            return stored.credential.clone();
        };

        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target("https://gitee.com/");
        match oauth::gitee_refresh_via_broker(proxy_url, &payload.refresh_token) {
            Ok(refreshed) => {
                let new_token = refreshed.access_token;
                // 写回新令牌 + 更新续期材料（refresh_token 可能轮换，以响应为准；
                // 响应未带新值时沿用旧的——Gitee 刷新不总是轮换）。
                let next_refresh = refreshed
                    .refresh_token
                    .clone()
                    .unwrap_or(payload.refresh_token);
                // 响应缺 expires_in 时不能沿用旧值（已在提前量内会立刻再次
                // 触发刷新）：兜底前移 12h（Gitee 典型 24h 的一半）。
                let next_expires = refreshed.expires_at.unwrap_or_else(|| now + 12 * 3600);
                if let Err(err) = self.store.update_secret(&stored.record.id, &new_token) {
                    tracing::warn!("Gitee 令牌续期后写回失败：{err}");
                }
                // 轮换出的新 refresh_token 若写回失败，存储里留下的是已被
                // 消费的旧 token，自动续期从此静默失效——必须提示用户重新
                // 登录，不能只记日志。
                if let Err(err) = khaslana::credentials::save_gitee_refresh_payload(
                    &stored.record.id,
                    &next_refresh,
                    next_expires,
                ) {
                    send_ui_event(
                        &self.tx,
                        UiEvent::GiteeTokenRefreshed {
                            success: false,
                            message: format!(
                                "Gitee 令牌已续期，但续期材料保存失败（{err}）；请重新登录 Gitee 恢复自动续期"
                            ),
                        },
                    );
                    // 本操作仍用新令牌（内存中已刷新）；仅持久化受损。
                    return match stored.credential.clone() {
                        GitCredential::UserPass {
                            username,
                            secret: _,
                            display_name,
                            save_to_keyring,
                            scope,
                        } => GitCredential::UserPass {
                            username,
                            secret: new_token,
                            display_name,
                            save_to_keyring,
                            scope,
                        },
                        other => other,
                    };
                }
                send_ui_event(
                    &self.tx,
                    UiEvent::GiteeTokenRefreshed {
                        success: true,
                        message: "Gitee 令牌已自动续期".into(),
                    },
                );
                match stored.credential.clone() {
                    GitCredential::UserPass {
                        username,
                        secret: _,
                        display_name,
                        save_to_keyring,
                        scope,
                    } => GitCredential::UserPass {
                        username,
                        secret: new_token,
                        display_name,
                        save_to_keyring,
                        scope,
                    },
                    other => other,
                }
            }
            Err(error) => {
                tracing::warn!("Gitee 令牌自动续期失败：{error}");
                send_ui_event(
                    &self.tx,
                    UiEvent::GiteeTokenRefreshed {
                        success: false,
                        message: format!(
                            "Gitee 令牌自动续期失败：{error}；请重新登录 Gitee 更新凭据"
                        ),
                    },
                );
                stored.credential.clone()
            }
        }
    }
}

impl CredentialProvider for TabCredentialProvider {
    fn credential_for(
        &self,
        request: CredentialRequest,
    ) -> khaslana::Result<Option<GitCredential>> {
        if let Ok(mut last) = self.last_stored_attempt.lock()
            && let Some(attempt) = last.clone()
            && attempt.is_retry_for(&request)
            && attempt.use_count >= STORED_CREDENTIAL_REUSE_LIMIT_PER_OPERATION
        {
            if let Ok(mut rejected) = self.rejected_record_ids.lock()
                && !rejected.contains(&attempt.record_id)
            {
                rejected.push(attempt.record_id.clone());
            }
            *last = None;
        }

        let rejected_record_ids = self
            .rejected_record_ids
            .lock()
            .map(|rejected| rejected.clone())
            .unwrap_or_default();

        let binding_policy = remote_binding_for_request(&self.remote_bindings, &request);
        let stored = match binding_policy {
            RemoteCredentialPolicy::NoCredential => Ok(None),
            RemoteCredentialPolicy::Record(record_id) => {
                if rejected_record_ids.contains(&record_id) {
                    Ok(None)
                } else {
                    match self.store.credential_for_record(&record_id) {
                        Ok(Some(credential)) => {
                            let touched = self.store.touch_record(&record_id)?;
                            let Some(record) = touched else {
                                return Ok(None);
                            };
                            if !credential_record_matches_remote_url(&record, &request.url) {
                                return Ok(None);
                            }
                            Ok(Some(khaslana::credentials::StoredCredential {
                                record,
                                credential,
                            }))
                        }
                        Ok(None) => Ok(None),
                        Err(err) => Err(err),
                    }
                }
            }
            RemoteCredentialPolicy::AutoMatch => {
                self.store.get_stored(&request, &rejected_record_ids)
            }
        };
        match stored {
            Ok(Some(stored)) => {
                if let Ok(mut last) = self.last_stored_attempt.lock() {
                    if let Some(attempt) = last.as_mut()
                        && attempt.is_retry_for(&request)
                        && attempt.record_id == stored.record.id
                    {
                        attempt.mark_used_again();
                    } else {
                        *last = Some(StoredCredentialAttempt::from_request(
                            &request,
                            stored.record.id.clone(),
                        ));
                    }
                }
                // Gitee OAuth 凭据：按需惰性续期（距过期 <2h 或已过期时刷新）。
                return Ok(Some(self.maybe_refresh_gitee_token(&stored)));
            }
            Ok(None) => {}
            Err(err) => tracing::warn!("keyring read skipped: {err}"),
        }

        let (response_tx, response_rx) = mpsc::channel();
        let response_tx = Arc::new(Mutex::new(Some(response_tx)));
        send_ui_event(
            &self.tx,
            UiEvent::CredentialRequested {
                tab_id: Some(self.tab_id),
                request: request.clone(),
                response_tx: response_tx.clone(),
            },
        );
        let credential = response_rx
            .recv()
            .map_err(|_| khaslana::GitError::Credential("凭据输入已取消".into()))??;

        if let Some(credential) = credential {
            if credential.should_save() {
                match self.store.save_record(&request, &credential) {
                    Ok(record) => {
                        if let Ok(mut rejected) = self.rejected_record_ids.lock() {
                            rejected.retain(|record_id| record_id != &record.id);
                        }
                        if let Ok(mut last) = self.last_stored_attempt.lock() {
                            *last = Some(StoredCredentialAttempt::from_request(
                                &request,
                                record.id.clone(),
                            ));
                        }
                        set_remote_binding_for_request(
                            &self.remote_bindings,
                            &request,
                            RemoteCredentialPolicy::Record(record.id),
                        );
                        if let Ok(bindings) = self.remote_bindings.lock() {
                            if let Err(err) =
                                self.storage.save_remote_credential_bindings(&bindings)
                            {
                                tracing::warn!("remote credential bindings write skipped: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!("keyring save skipped: {err}");
                        if let Ok(mut last) = self.last_stored_attempt.lock() {
                            *last = None;
                        }
                    }
                }
            } else if let Ok(mut last) = self.last_stored_attempt.lock() {
                *last = None;
            }
            return Ok(Some(credential));
        }

        Ok(None)
    }
}

fn remote_binding_key(repo_path: &Path, remote_name: &str) -> (String, String) {
    (normalize_repo_path(repo_path), remote_name.to_string())
}

fn remote_binding_for_request(
    bindings: &Arc<Mutex<RemoteCredentialBindings>>,
    request: &CredentialRequest,
) -> RemoteCredentialPolicy {
    let (Some(repo_path), Some(remote_name)) = (&request.repo_path, request.remote_name.as_ref())
    else {
        return RemoteCredentialPolicy::AutoMatch;
    };
    let (repo_key, remote_key) = remote_binding_key(repo_path, remote_name);
    bindings
        .lock()
        .ok()
        .and_then(|bindings| {
            bindings
                .remotes
                .iter()
                .find(|binding| {
                    binding.repo_path == repo_key
                        && binding.remote_name == remote_key
                        && normalize_remote_url(&binding.remote_url)
                            == normalize_remote_url(&request.url)
                })
                .map(|binding| binding.policy.clone())
        })
        .unwrap_or(RemoteCredentialPolicy::AutoMatch)
}

fn set_remote_binding_for_request(
    bindings: &Arc<Mutex<RemoteCredentialBindings>>,
    request: &CredentialRequest,
    policy: RemoteCredentialPolicy,
) {
    let (Some(repo_path), Some(remote_name)) = (&request.repo_path, request.remote_name.as_ref())
    else {
        return;
    };
    let (repo_key, remote_key) = remote_binding_key(repo_path, remote_name);
    let Ok(mut bindings) = bindings.lock() else {
        return;
    };
    if let Some(binding) = bindings
        .remotes
        .iter_mut()
        .find(|binding| binding.repo_path == repo_key && binding.remote_name == remote_key)
    {
        binding.remote_url = request.url.clone();
        binding.policy = policy;
    } else {
        bindings.remotes.push(RemoteCredentialBinding {
            repo_path: repo_key,
            remote_name: remote_key,
            remote_url: request.url.clone(),
            policy,
        });
    }
}

fn send_credential_response(
    pending: &PendingCredential,
    response: khaslana::Result<Option<GitCredential>>,
) -> bool {
    let Ok(mut response_tx) = pending.response_tx.lock() else {
        return false;
    };
    let Some(response_tx) = response_tx.take() else {
        return false;
    };
    response_tx.send(response).is_ok()
}

/// 凭据测试地址校验（纯函数，可单测）：非空 → 协议族（HTTPS 记录须
/// http(s)、SSH 记录须 SSH 地址）→ HTTPS 记录再做同站点校验（令牌不
/// 通用，跨站点测试只会得到误导性的认证失败；真实操作中凭据也只会在
/// 同站点被命中）。SSH 私钥主机无关（同一把钥匙可部署多个平台），
/// 不做站点限制。
fn validate_credential_test_url(
    kind: khaslana::StoredCredentialKind,
    record_host: &str,
    url: &str,
) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("需要填写测试地址".to_string());
    }
    let url = url.trim();
    let inferred_mode = credential_form_mode_for_request(&CredentialRequest {
        url: url.to_string(),
        username_from_url: None,
        allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT | git2::CredentialType::SSH_KEY,
        repo_path: None,
        remote_name: None,
        operation_id: None,
    });
    let expected_mode = match kind {
        khaslana::StoredCredentialKind::HttpsUserPass => CredentialFormMode::Https,
        khaslana::StoredCredentialKind::SshKey => CredentialFormMode::Ssh,
    };
    if inferred_mode != expected_mode {
        return Err(match expected_mode {
            CredentialFormMode::Https => {
                "该记录是 HTTPS 凭据，测试地址必须是 http(s) 地址".to_string()
            }
            CredentialFormMode::Ssh => {
                "该记录是 SSH 凭据，测试地址必须是 SSH 地址（git@主机:仓库 或 ssh://）".to_string()
            }
        });
    }
    if kind == khaslana::StoredCredentialKind::HttpsUserPass {
        match khaslana::credentials::remote_host_key(url) {
            Some(host_key) if host_key == record_host => Ok(()),
            Some(host_key) => Err(format!(
                "该凭据绑定 {record_host}，令牌不通用，不能用其它站点（{host_key}）的地址测试"
            )),
            None => Err("无法解析测试地址".to_string()),
        }
    } else {
        Ok(())
    }
}

fn credential_form_mode_for_request(request: &CredentialRequest) -> CredentialFormMode {
    let lower = request.url.to_ascii_lowercase();
    if lower.starts_with("ssh://")
        || lower.starts_with("git@")
        || (!lower.starts_with("http://")
            && !lower.starts_with("https://")
            && request
                .allowed_types
                .contains(git2::CredentialType::SSH_KEY))
    {
        CredentialFormMode::Ssh
    } else {
        CredentialFormMode::Https
    }
}

pub(crate) fn send_ui_event(tx: &Sender<UiEvent>, event: UiEvent) {
    let _ = tx.try_send(event);
}

/// 在系统默认浏览器中打开 URL。
fn open_url(url: &str) {
    // raw_arg 会绕过 Rust 的参数转义：URL 位于 cmd 的双引号内时 `&` 等符号
    // 是字面量，但 URL 内出现 `"` 会提前闭合引号、向 cmd 注入命令分隔符，
    // 控制字符同理。remote 配置可写入任意 URL，这里统一拒绝。
    if url.contains(['"', '\r', '\n', '\t']) {
        tracing::warn!(target: "khaslana", "拒绝打开含特殊字符的 URL：{url}");
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不弹出黑色 cmd 控制台窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // raw_arg 绕过 Rust 对参数的自动引号转义，避免内层引号被二次转义导致 start 报错；
        // URL 整体加双引号，让 cmd 不把查询串里的 & 当命令分隔符（&code=...&state=...）；
        // 空标题 "" 防止 start 把引号包裹的目标当成窗口标题而被吞掉。
        let _ = std::process::Command::new("cmd")
            .raw_arg(format!("/C start \"\" \"{url}\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub(crate) fn perf_log(stage: &'static str, started: Instant, details: impl AsRef<str>) {
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

fn optional_display_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn started_message_for_label(label: &'static str) -> &'static str {
    match label {
        "拉取远程引用完成" => "正在拉取远程引用",
        "拉取完成" => "正在拉取",
        "推送完成" => "正在推送",
        "提交并推送完成" => "正在提交并推送",
        "远端分支已拉取到本地" => "正在拉取远端分支",
        "克隆完成" => "正在克隆仓库",
        "已刷新" => "正在刷新仓库",
        "合并操作已完成" => "正在合并分支",
        "合并已完成" => "正在完成合并",
        "合并已中止" => "正在中止合并",
        "变基完成" => "正在变基分支",
        "变基已中止" => "正在中止变基",
        "变基拉取完成" => "正在变基拉取",
        "切换分支完成" => "正在切换分支",
        "提交完成" => "正在提交",
        "修补提交完成" => "正在修补提交",
        "拣选提交完成" => "正在拣选提交",
        "已暂存选中改动" => "正在暂存选中改动",
        "已取消暂存选中改动" => "正在取消暂存选中改动",
        "标签已创建" => "正在创建标签",
        "标签已删除" => "正在删除标签",
        "标签已推送" => "正在推送标签",
        "远端标签已删除" => "正在删除远端标签",
        "分支已创建" => "正在创建分支",
        "分支已重命名" => "正在重命名分支",
        "分支已删除" => "正在删除分支",
        "检出标签完成" => "正在检出标签",
        "应用贮藏完成" => "正在应用贮藏",
        "弹出贮藏完成" => "正在弹出贮藏",
        "分支已重置" => "正在重置分支",
        "回滚提交完成" => "正在回滚提交",
        "远端已更新" => "正在更新远端",
        "远端已新增" => "正在新增远端",
        "远端已删除" => "正在删除远端",
        "远端已刷新" => "正在刷新远端",
        "冲突已标记为解决" => "正在标记冲突解决",
        "IntelliJ IDEA 合并结果已应用" => "正在等待 IntelliJ IDEA 合并完成",
        "子模块已同步记录版本" => "正在同步子模块记录版本",
        "子模块已更新到远端最新" => "正在更新子模块到远端最新",
        _ => label,
    }
}

fn started_message_for_label_text(label: &str) -> String {
    if label.starts_with("子模块 ") && label.ends_with(" 已更新到远端最新") {
        return "正在更新子模块到远端最新".to_string();
    }
    match label {
        "子模块已同步记录版本" => "正在同步子模块记录版本".to_string(),
        "子模块已更新到远端最新" => "正在更新子模块到远端最新".to_string(),
        _ => label.to_string(),
    }
}

/// 将 Git 的仓库相对路径转换为可复制、可交给系统文件管理器的绝对路径。
fn repository_file_absolute_path(repo_path: &Path, file_path: &str) -> PathBuf {
    let repo_path = if repo_path.is_absolute() {
        repo_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(repo_path))
            .unwrap_or_else(|_| repo_path.to_path_buf())
    };
    repo_path.join(file_path).components().collect()
}

pub(crate) struct RepositoryView {
    tx: Sender<UiEvent>,
    rx: Receiver<UiEvent>,
    tasks: TaskExecutor,
    storage: Arc<khaslana::AppStorage>,
    credential_store: Arc<KeyringCredentialStore>,
    remote_credential_bindings: Arc<Mutex<RemoteCredentialBindings>>,
    credential_records: Vec<CredentialRecord>,
    pub(crate) workflow_templates: Vec<WorkflowTemplateItem>,
    pub(crate) workflow_template_dir: Option<PathBuf>,
    /// 工作流模板创建器状态（仅弹窗打开期间存在）。
    pub(crate) workflow_editor: Option<WorkflowEditorState>,
    /// 注释丢失确认前的暂存（编辑带注释模板时，确认后据此进入编辑器）。
    pub(crate) pending_workflow_edit: Option<workflow_editor::PendingWorkflowEdit>,
    diff_encoding_preferences: DiffEncodingPreferences,
    diff_cache: RefCell<LruCache<DiffCacheKey, Arc<FileDiff>>>,
    pub(crate) proxy_settings: NetworkProxySettings,
    pub(crate) theme_mode: ThemeMode,
    /// 当前激活的主题色预设索引（0 = 靛蓝默认）。
    pub(crate) theme_accent: usize,
    tabs: Vec<RepoTabState>,
    active_tab: Option<RepoTabId>,
    /// 全局测试类操作（代理/凭据/AI 连接测试）借用 busy 的来源 tab。
    /// 这些操作不属于任何 tab，busy 却经 DerefMut 落在发起时的活动 tab 上；
    /// 记录来源 tab 并在完成事件中定向复位，避免测试期间切换仓库把复位
    /// 写到错误的 tab、原 tab 的工具栏永久禁用。
    global_busy_tab: Option<RepoTabId>,
    next_tab_id: u64,
    fallback_tab: RepoTabState,
    restoring_session: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) changes_width: f32,
    pub(crate) workflow_templates_width: f32,
    pub(crate) history_files_width: f32,
    /// 历史检查器内「提交文件 | 差异」分栏宽度（四象限下半部，视图偏好不持久化）。
    pub(crate) history_inspector_files_width: f32,
    /// 提交详情区高度与折叠状态（视图偏好，不持久化）：`None` 表示未手动
    /// 调整过，检查器使用默认详情高度。
    pub(crate) history_details_height: Option<f32>,
    pub(crate) history_details_collapsed: bool,
    /// 历史检查器顶部窗口坐标（1px 标记 canvas 每帧记录）：首次拖拽时，
    /// 用分割条点击位置减去该坐标推导详情区实际高度并固化。
    history_details_top_hint: Arc<Cell<f32>>,
    pub(crate) browse_tree_width: f32,
    pub(crate) history_graph_width: f32,
    resizing_sidebar_width: Option<ResizeState>,
    resizing_changes_width: Option<ResizeState>,
    resizing_workflow_templates_width: Option<ResizeState>,
    resizing_history_files_width: Option<ResizeState>,
    resizing_history_inspector_files_width: Option<ResizeState>,
    resizing_history_details_height: Option<ResizeState>,
    resizing_browse_tree_width: Option<ResizeState>,
    resizing_history_graph_width: Option<ResizeState>,
    scroll_handles: RefCell<HashMap<String, ScrollHandle>>,
    uniform_scroll_handles: RefCell<HashMap<String, UniformListScrollHandle>>,
    /// 差异区域最宽行扫描的单槽缓存（见 `cached_widest_diff_row_index`）。
    widest_diff_row_cache: RefCell<WidestDiffRowCache>,
    pub(crate) scrollbar_drag: Option<ScrollbarDragState>,
    pending_credential: Option<PendingCredential>,
    pending_credentials: VecDeque<PendingCredential>,
    repository_load_queue: VecDeque<RepositoryLoadRequest>,
    active_repository_loads: usize,
    feedbacks: VecDeque<FeedbackMessage>,
    next_feedback_id: u64,
    progress_phase: u64,
    pub(crate) active_dialog: Option<DialogState>,
    /// 设置中心当前分类；独立于 active_dialog，凭据子弹窗可叠加其上。
    pub(crate) settings_center: Option<SettingsCategory>,
    /// 用户自定义快捷键绑定（action_id → keystroke）。
    pub(crate) shortcut_bindings: ShortcutBindings,
    /// 快捷键设置页中正在录制的动作；None 表示非录制态。
    pub(crate) recording_shortcut: Option<ShortcutAction>,
    /// 设置中心面板的焦点句柄，录制态时夺取焦点使 keydown dispatch_path 进入 overlay。
    settings_center_focus: FocusHandle,
    dialog_before_window_close: Option<DialogState>,
    exit_requested: bool,
    #[cfg(windows)]
    tray: Option<tray::TrayController>,
    #[cfg(windows)]
    tray_error: Option<String>,
    pub(crate) branch_context_menu: Option<BranchContextMenu>,
    pub(crate) remote_context_menu: Option<RemoteContextMenu>,
    change_context_menu: Option<ChangeContextMenu>,
    file_path_context_menu: Option<FilePathContextMenu>,
    credential_context_menu: Option<CredentialContextMenu>,
    pub(crate) tag_context_menu: Option<TagContextMenu>,
    pub(crate) stash_context_menu: Option<StashContextMenu>,
    pub(crate) workflow_template_context_menu: Option<WorkflowTemplateContextMenu>,
    pub(crate) commit_context_menu: Option<CommitContextMenu>,
    pub(crate) encoding_menu_target: Option<EncodingMenuTarget>,
    encoding_menu_closed_by_capture: Option<EncodingMenuTarget>,
    /// 图谱页分支高亮下拉菜单：根层捕获点击关闭后的「同次点击不再打开」标记
    ///（与 encoding_menu_closed_by_capture 同一套防重开模式）。
    commit_graph_branch_menu_closed_by_capture: bool,
    repo_switcher_menu: Option<RepoSwitcherMenu>,
    /// 窄窗口下 Context Navigator 的临时覆盖态，不改写宽屏停靠偏好。
    context_navigator_overlay_open: bool,
    /// Context Navigator 展开偏好（全局单值，跨模式/跨仓库共享，经布局偏好持久化）。
    context_navigator_preferences: ContextNavigatorPreferences,
    // 壳层按钮均为纯鼠标交互，不再持有焦点句柄（键盘白名单见 AGENTS.md §8）。
    /// 仓库切换下拉触发器按钮的窗口坐标矩形，paint 时记录，供菜单锚定与点击外部关闭。
    repo_switcher_anchor: Option<RepoSwitcherAnchor>,
    /// 仓库切换下拉展开时缓存的最近仓库列表（toggle 时同步加载，渲染时纯读）。
    repo_switcher_recent: Vec<(PathBuf, i64)>,
    /// 仓库切换下拉顶部的搜索框，输入即过滤打开/最近项目列表。
    repo_switcher_search: TextFieldState,
    /// 提交图谱页的谱系搜索框（按摘要/作者/短 SHA 过滤已加载提交）。
    /// 全局共享一份搜索词，跨模式跳转与 tab 切换均保留。
    commit_graph_search: TextFieldState,
    /// 提交图谱页分支高亮下拉的菜单内搜索框（打开菜单即清空并聚焦）。
    commit_graph_branch_search: TextFieldState,
    /// 搜索框是否展开：默认只显示「搜索仓库」按钮，点击后替换为输入框 + 小叉。
    repo_switcher_search_open: bool,
    save_credential: bool,
    credential_scope: CredentialScope,
    credential_form_mode: CredentialFormMode,
    credential_use_ssh_agent: bool,
    pub(crate) ssh_credential_discovery: SshCredentialDiscoveryState,
    oauth_login_flow: OAuthLoginFlowState,
    /// Gitee 登录成功后刚保存的凭据记录 id：`save_credential_form` 写入，
    /// `OAuthLoginSucceeded` 处理器取走并把 refresh_token 等续期材料落到
    /// 独立的 Keyring 条目（见 `credentials::save_gitee_refresh_payload`）。
    pending_gitee_refresh_record: Option<String>,
    clone_url: TextFieldState,
    clone_path: TextFieldState,
    clone_recursive_submodules: bool,
    branch_name: TextFieldState,
    create_branch_checkout: bool,
    branch_rename: TextFieldState,
    commit_message: TextFieldState,
    /// 修补提交模式：开启后主提交按钮变“修补提交”，以当前暂存区重写 HEAD。
    amend_mode: bool,
    /// 修补开关预填的提交信息：关闭开关时仅当输入框未被用户修改才清除。
    amend_prefill: Option<String>,
    stash_message: TextFieldState,
    tag_name: TextFieldState,
    tag_message: TextFieldState,
    /// 创建标签时是否带附注（附注标签记录 tagger 与信息，发布场景推荐）。
    tag_annotated: bool,
    /// 标签推送对话框选中的远端。
    tag_push_remote: Option<String>,
    stash_include_untracked: bool,
    stash_keep_index: bool,
    credential_username: TextFieldState,
    credential_secret: TextFieldState,
    credential_key_path: TextFieldState,
    credential_passphrase: TextFieldState,

    credential_remote_url: TextFieldState,
    /// 凭据测试弹窗的「测试地址」输入（预填记录远端地址，可改）。
    credential_test_url: TextFieldState,
    /// 凭据测试弹窗的内联校验错误（地址非法/跨站点），关窗即清。
    credential_test_error: Option<String>,
    credential_display_name: TextFieldState,
    conflict_editor: TextFieldState,
    remote_name: TextFieldState,
    remote_url: TextFieldState,
    remote_credential_policy: RemoteCredentialPolicy,
    pub(crate) remote_branch_name: TextFieldState,
    pub(crate) remote_branch_search: TextFieldState,
    pub(crate) sidebar_local_branch_search: TextFieldState,
    pub(crate) sidebar_remote_branch_search: TextFieldState,
    pub(crate) sidebar_local_branch_search_open: bool,
    pub(crate) sidebar_remote_branch_search_open: bool,
    pub(crate) remote_branch_operation: RemoteBranchOperationState,
    proxy_mode: NetworkProxyMode,
    proxy_http_url: TextFieldState,
    proxy_https_url: TextFieldState,
    proxy_socks5_url: TextFieldState,
    pub(crate) ai_settings: AiProviderSettings,
    pub(crate) external_merge_settings: ExternalMergeSettings,
    pub(crate) external_merge_enabled_form: bool,
    pub(crate) external_merge_auto_open_form: bool,
    external_merge_intellij_path: TextFieldState,
    external_merge_detection: Option<(ExternalMergeSettings, bool)>,
    pub(crate) ai_enabled_form: bool,
    ai_base_url: TextFieldState,
    ai_api_key: TextFieldState,
    ai_model: TextFieldState,
    pub(crate) ai_commit_loading: bool,
    pub(crate) ai_review: Option<Arc<AiReviewResult>>,
    pub(crate) ai_review_loading: bool,
    /// agent 评审实时累积的执行轨迹（思维链 + 工具调用），生成期间与
    /// 完成后共用（完成后与 review.steps 同源）。
    pub(crate) ai_review_steps: Vec<AiReviewStep>,
    /// agent 评审的进度文案（生成中展示在标题栏）。
    pub(crate) ai_review_progress: Option<String>,
    /// 时间线上展开详情的步骤下标集合（其余行只显示一行摘要）。
    pub(crate) ai_review_step_expanded: BTreeSet<usize>,
    /// 当前轮流式思维链的实时累积（「思考中…」live 区，轮次落定后清空）。
    pub(crate) ai_review_live_reasoning: String,
    /// 最终正文的流式实时累积（边生成边按 Markdown 渲染，完成定格）。
    pub(crate) ai_review_live_content: String,
    pub(crate) ai_review_expanded: bool,
    /// 评审代际计数器：每次 generate 递增，事件携带代际做守卫。
    ai_review_next_generation: u64,
    /// 当前展示附着的任务代际；None = 无附着（后台任务可能仍在跑，
    /// 其事件只用于完成提示，不进面板）。
    ai_review_active_generation: Option<u64>,
    /// 在途评审任务数（含切目标后分离的），上限 MAX_CONCURRENT_AI_REVIEWS。
    ai_review_running_tasks: usize,
    /// 附着任务的取消标志（置位后任务在轮次边界退出，不落盘不提示失败）。
    ai_review_cancel: Option<Arc<AtomicBool>>,
    /// 面板当前展示的是历史记录时的标签（如「历史 · 08-18 14:30 · feature/x」）。
    pub(crate) ai_review_loaded_label: Option<String>,
    /// 评审历史弹窗状态（None = 关闭）。
    pub(crate) ai_review_history: Option<AiReviewHistoryState>,
    /// 冲突工作台三栏同步滚动的上帧 offset 记录（[ours, result, theirs]，
    /// 跨帧供连线 canvas paint 判定滚动源；paint 闭包拿不到实体，经 Rc
    /// 共享）。选中冲突文件时重置。
    pub(crate) conflict_pane_scroll_sync: Rc<RefCell<Option<[f32; 3]>>>,
    /// 冲突工作台「AI 合并建议」生成中标志（不借用 busy，生成期间其它
    /// 冲突操作保持可用，与 commit message 生成同一模式）。
    pub(crate) ai_conflict_loading: bool,
    /// AI 思考弹窗状态（None = 关闭）：公共执行器发起的一次性 AI 请求
    /// （commit message / 冲突合并建议 / 工作流模板生成）期间展示思维链
    /// 流式输出，任务完成或失败后自动关闭弹窗。
    pub(crate) ai_thinking_overlay: Option<AiThinkingOverlayState>,
    /// 思考弹窗钉底跟随的跨帧状态（内容长度键），见 `AiThinkingFollowState`。
    pub(crate) ai_thinking_follow_state: std::rc::Rc<AiThinkingFollowState>,
    // ── 更新状态 ──
    pub(crate) update_preferences: UpdatePreferences,
    pub(crate) update_checking: bool,
    pub(crate) update_downloading: bool,
    pub(crate) available_update: Option<Arc<UpdateManifest>>,
    pub(crate) update_download_progress: Option<String>,
    pub(crate) update_error: Option<String>,
    pub(crate) staging_dir_for_install: Option<PathBuf>,
}

/// 主模式继承判定（纯函数）：切换/打开/克隆仓库时，仅主模式
/// （工作区/提交记录/工作流/图谱）跟随切换带过去，保持「当前区域」不变；
/// 专用模式（Conflict/Stash/Browse/Blame）绑定 per-repo 状态，不继承。
fn inheritable_main_mode(previous: Option<MainMode>) -> Option<MainMode> {
    match previous {
        mode @ Some(
            MainMode::Worktree | MainMode::History | MainMode::Workflow | MainMode::CommitGraph,
        ) => mode,
        _ => None,
    }
}

impl RepositoryView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let (storage, storage_status, storage_error) = Self::open_storage();
        let credential_store = Arc::new(KeyringCredentialStore::with_storage(storage.clone()));
        let ai_settings = Self::load_ai_provider_settings(&storage);
        let remote_credential_bindings =
            Arc::new(Mutex::new(Self::load_remote_credential_bindings(&storage)));
        let proxy_settings = Self::load_proxy_settings(&storage);
        let external_merge_settings = Self::load_external_merge_settings(&storage);
        let theme_mode = Self::load_theme_mode(&storage);
        let theme_accent = Self::load_theme_accent(&storage);
        let layout_preferences = Self::load_layout_preferences(&storage);
        let proxy_custom = proxy_settings.custom.normalized();
        #[cfg(windows)]
        let (tray, tray_error) = match tray::TrayController::new() {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(error)),
        };
        Self::spawn_event_pump(rx.clone(), cx);
        Self::spawn_ui_tick(tx.clone());
        let tasks = TaskExecutor::new(tx.clone());

        Self {
            tx,
            rx,
            tasks,
            storage: storage.clone(),
            credential_store,
            remote_credential_bindings,
            credential_records: Vec::new(),
            workflow_templates: Vec::new(),
            workflow_template_dir: workflow_templates_dir(),
            workflow_editor: None,
            pending_workflow_edit: None,
            workflow_template_context_menu: None,
            diff_encoding_preferences: Self::load_diff_encoding_preferences(&storage),
            diff_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(DIFF_CACHE_CAPACITY)
                    .expect("diff cache capacity must be nonzero"),
            )),
            proxy_settings: proxy_settings.clone(),
            theme_mode,
            theme_accent,
            tabs: Vec::new(),
            active_tab: None,
            global_busy_tab: None,
            next_tab_id: 1,
            fallback_tab: {
                let mut tab = RepoTabState::new(RepoTabId(0), None);
                if let Some(status) = storage_status {
                    tab.status = status;
                    tab.last_error = storage_error;
                }
                tab
            },
            restoring_session: false,
            // 布局偏好：启动时从 layout_preferences 恢复（空库回默认常量），
            // 宽度按 MIN/MAX 钳制——防手改 DB 或常量演进导致越界布局。
            context_navigator_preferences: ContextNavigatorPreferences {
                visible: layout_preferences.navigator_visible.unwrap_or(true),
            },
            sidebar_width: layout_preferences
                .sidebar_width
                .map(|width| width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH))
                .unwrap_or(DEFAULT_SIDEBAR_WIDTH),
            changes_width: layout_preferences
                .changes_width
                .map(|width| width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH))
                .unwrap_or(DEFAULT_CHANGES_WIDTH),
            workflow_templates_width: layout_preferences
                .workflow_templates_width
                .map(|width| {
                    width.clamp(MIN_WORKFLOW_TEMPLATES_WIDTH, MAX_WORKFLOW_TEMPLATES_WIDTH)
                })
                .unwrap_or(DEFAULT_WORKFLOW_TEMPLATES_WIDTH),
            history_files_width: layout_preferences
                .history_files_width
                .map(|width| width.clamp(MIN_HISTORY_FILES_WIDTH, MAX_HISTORY_FILES_WIDTH))
                .unwrap_or(DEFAULT_HISTORY_FILES_WIDTH),
            history_inspector_files_width: layout_preferences
                .history_inspector_files_width
                .map(|width| {
                    width.clamp(
                        MIN_HISTORY_INSPECTOR_FILES_WIDTH,
                        MAX_HISTORY_INSPECTOR_FILES_WIDTH,
                    )
                })
                .unwrap_or(DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH),
            history_details_height: layout_preferences
                .history_details_height
                .map(|height| height.clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT)),
            history_details_collapsed: layout_preferences.history_details_collapsed,
            history_details_top_hint: Arc::new(Cell::new(0.0)),
            browse_tree_width: layout_preferences
                .browse_tree_width
                .map(|width| width.clamp(MIN_BROWSE_TREE_WIDTH, MAX_BROWSE_TREE_WIDTH))
                .unwrap_or(DEFAULT_BROWSE_TREE_WIDTH),
            history_graph_width: layout_preferences
                .history_graph_width
                .map(|width| width.clamp(MIN_HISTORY_GRAPH_WIDTH, MAX_HISTORY_GRAPH_WIDTH))
                .unwrap_or(DEFAULT_HISTORY_GRAPH_WIDTH),
            resizing_sidebar_width: None,
            resizing_changes_width: None,
            resizing_workflow_templates_width: None,
            resizing_history_files_width: None,
            resizing_history_inspector_files_width: None,
            resizing_history_details_height: None,
            resizing_browse_tree_width: None,
            resizing_history_graph_width: None,
            scroll_handles: RefCell::new(HashMap::new()),
            uniform_scroll_handles: RefCell::new(HashMap::new()),
            widest_diff_row_cache: RefCell::new(WidestDiffRowCache::default()),
            scrollbar_drag: None,
            pending_credential: None,
            pending_credentials: VecDeque::new(),
            repository_load_queue: VecDeque::new(),
            active_repository_loads: 0,
            feedbacks: VecDeque::new(),
            next_feedback_id: 0,
            progress_phase: 0,
            active_dialog: None,
            settings_center: None,
            shortcut_bindings: Self::load_shortcut_bindings(&storage),
            recording_shortcut: None,
            settings_center_focus: cx.focus_handle(),
            dialog_before_window_close: None,
            exit_requested: false,
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            tray_error,
            branch_context_menu: None,
            remote_context_menu: None,
            change_context_menu: None,
            file_path_context_menu: None,
            credential_context_menu: None,
            tag_context_menu: None,
            stash_context_menu: None,
            commit_context_menu: None,
            encoding_menu_target: None,
            encoding_menu_closed_by_capture: None,
            commit_graph_branch_menu_closed_by_capture: false,
            repo_switcher_menu: None,
            context_navigator_overlay_open: false,
            repo_switcher_anchor: None,
            repo_switcher_recent: Vec::new(),
            repo_switcher_search: TextFieldState::new(cx, "搜索仓库"),
            commit_graph_search: TextFieldState::new(cx, "搜索提交/作者/SHA"),
            commit_graph_branch_search: TextFieldState::new(cx, "搜索分支"),
            repo_switcher_search_open: false,
            save_credential: false,
            credential_scope: CredentialScope::RemoteUrl,
            credential_form_mode: CredentialFormMode::Https,
            credential_use_ssh_agent: false,
            ssh_credential_discovery: SshCredentialDiscoveryState::default(),
            oauth_login_flow: OAuthLoginFlowState::default(),
            pending_gitee_refresh_record: None,
            clone_url: TextFieldState::new(cx, "远程仓库 URL"),
            clone_path: TextFieldState::new(cx, "克隆到父文件夹"),
            clone_recursive_submodules: default_clone_recursive_submodules(),
            branch_name: TextFieldState::new(cx, "新分支名称"),
            create_branch_checkout: true,
            branch_rename: TextFieldState::new(cx, "重命名为"),
            commit_message: TextFieldState::new(cx, "提交信息"),
            amend_mode: false,
            amend_prefill: None,
            stash_message: TextFieldState::new(cx, "贮藏说明（可选）"),
            tag_name: TextFieldState::new(cx, "标签名称，如 v1.0.0"),
            tag_message: TextFieldState::new(cx, "标签附注信息（可选）"),
            tag_annotated: true,
            tag_push_remote: None,
            stash_include_untracked: false,
            stash_keep_index: false,
            credential_username: TextFieldState::new(cx, "用户名"),
            credential_secret: TextFieldState::new(cx, "密码或 PAT").secret(),
            credential_key_path: TextFieldState::new(cx, "SSH 私钥路径"),
            credential_passphrase: TextFieldState::new(cx, "SSH 密码短语").secret(),

            credential_remote_url: TextFieldState::new(cx, "适用远端 URL"),
            credential_test_url: TextFieldState::new(cx, "测试地址"),
            credential_test_error: None,
            credential_display_name: TextFieldState::new(cx, "凭据名称（可选）"),
            conflict_editor: TextFieldState::new(cx, "冲突结果"),
            remote_name: TextFieldState::new(cx, "远端名称"),
            remote_url: TextFieldState::new(cx, "远端地址"),
            remote_credential_policy: RemoteCredentialPolicy::AutoMatch,
            remote_branch_name: TextFieldState::new(cx, "远程分支"),
            remote_branch_search: TextFieldState::new(cx, "搜索远端分支"),
            sidebar_local_branch_search: TextFieldState::new(cx, "搜索本地分支"),
            sidebar_remote_branch_search: TextFieldState::new(cx, "搜索远端分支"),
            sidebar_local_branch_search_open: false,
            sidebar_remote_branch_search_open: false,
            remote_branch_operation: RemoteBranchOperationState::default(),
            proxy_mode: proxy_settings.mode,
            external_merge_enabled_form: external_merge_settings.enabled,
            external_merge_auto_open_form: external_merge_settings.auto_open_intellij,
            external_merge_settings: external_merge_settings.clone(),
            ai_enabled_form: ai_settings.enabled,
            ai_settings,
            ai_base_url: TextFieldState::new(cx, "Base URL，例如 https://api.openai.com/v1"),
            ai_api_key: TextFieldState::new(cx, "API Key").secret(),
            ai_model: TextFieldState::new(cx, "模型名称，例如 gpt-4o-mini"),
            external_merge_intellij_path: TextFieldState::new(cx, "IntelliJ IDEA 路径（可选）")
                .with_value(external_merge_settings.normalized_intellij_path()),
            external_merge_detection: None,
            ai_commit_loading: false,
            ai_review: None,
            ai_review_loading: false,
            ai_review_steps: Vec::new(),
            ai_review_progress: None,
            ai_review_step_expanded: BTreeSet::new(),
            ai_review_live_reasoning: String::new(),
            ai_review_live_content: String::new(),
            ai_review_expanded: false,
            ai_review_next_generation: 1,
            ai_review_active_generation: None,
            ai_review_running_tasks: 0,
            ai_review_cancel: None,
            ai_review_loaded_label: None,
            ai_review_history: None,
            conflict_pane_scroll_sync: Rc::new(RefCell::new(None)),
            ai_conflict_loading: false,
            ai_thinking_overlay: None,
            ai_thinking_follow_state: std::rc::Rc::new(AiThinkingFollowState {
                last_key: std::cell::Cell::new((usize::MAX, usize::MAX)),
            }),
            // ── 更新状态 ──
            update_preferences: Self::load_update_preferences(&storage),
            update_checking: false,
            update_downloading: false,
            available_update: None,
            update_download_progress: None,
            update_error: None,
            staging_dir_for_install: None,
            proxy_http_url: TextFieldState::new(cx, "HTTP 代理 URL")
                .with_value(proxy_custom.http_proxy),
            proxy_https_url: TextFieldState::new(cx, "HTTPS 代理 URL")
                .with_value(proxy_custom.https_proxy),
            proxy_socks5_url: TextFieldState::new(cx, "SOCKS5 代理 URL")
                .with_value(proxy_custom.socks5_proxy),
        }
    }

    fn spawn_ui_tick(tx: Sender<UiEvent>) {
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(420));
                if tx.try_send(UiEvent::UiTick).is_err() {
                    break;
                }
            }
        });
    }

    fn new_with_session(cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(cx);
        view.restore_session();
        // 启动时自动检查更新（manual=false：结果不弹气泡，仅状态栏；
        // 发现新版本仍会弹窗）
        if view.update_preferences.auto_check {
            view.start_update_check(false);
        }
        // 老用户首次进入便携版本时，提示把数据从 C 盘迁移到程序同级目录。
        view.maybe_prompt_portable_migration();
        // exe 位于临时/聊天软件接收/下载目录时，提示移动程序到安全目录。
        view.maybe_prompt_exe_relocation();
        view
    }

    /// 检测当前是否需要提供「迁移到便携目录」入口。
    /// 仅当应用当前仍在旧目录（C 盘）运行、且未在排队待迁移时返回真；
    /// 一旦已切换到便携目录（新机器或迁移完成后），即返回假，设置中心入口随之隐藏。
    /// 不检查「不再提示」标记：即使用户曾选择保持现状，仍可从设置中心手动触发迁移。
    fn portable_migration_available(&self) -> bool {
        let (Some(active), Some(portable)) = (
            khaslana::default_database_path(),
            khaslana::portable_database_path(),
        ) else {
            return false;
        };
        // 当前已启用便携目录（新机器或已完成迁移）→ 无需迁移入口。
        if active == portable {
            return false;
        }
        // 已排队待迁移（下次启动搬运）→ 不重复显示。
        if khaslana::portable_pending_marker().is_some_and(|p| p.exists()) {
            return false;
        }
        // 当前激活路径（旧目录）的库确实存在 → 可迁移。
        active.exists()
    }

    /// 检测是否应提示用户把数据从旧目录（C 盘）迁移到便携目录。
    fn maybe_prompt_portable_migration(&mut self) {
        if !self.portable_migration_available() {
            return;
        }
        if self.storage.portable_migration_dismissed() {
            return;
        }
        self.active_dialog = Some(DialogState::PortableMigrationPrompt);
    }

    /// 用户确认迁移：写入待迁移标记后重启应用，下次启动在打开数据库前完成搬运。
    fn confirm_portable_migration(&mut self) {
        let _ = self
            .storage
            .set_meta_value("pending_portable_migration", "1");
        if let Some(marker) = khaslana::portable_pending_marker() {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, []);
        }
        self.active_dialog = None;
        Self::relaunch_app();
    }

    /// 用户保持现状：永久忽略便携迁移提示。
    fn dismiss_portable_migration(&mut self) {
        let _ = self.storage.mark_portable_migration_dismissed();
        self.active_dialog = None;
    }

    /// 重启应用：启动新实例后立即退出当前进程。
    fn relaunch_app() {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("khaslana.exe"));
        let exe_str = exe.to_string_lossy().to_string();
        let _ = std::process::Command::new(&exe_str).spawn();
        std::process::exit(0);
    }

    // ── 程序位置风险搬迁（exe 位于临时/聊天软件接收/下载目录） ──

    /// 当前是否应提供「移动程序到安全目录」入口。
    /// dismiss 标记只抑制启动弹窗，不影响设置中心手动入口。
    fn exe_relocation_available(&self) -> bool {
        if khaslana::current_exe_location_risk() == khaslana::ExeLocationRisk::Safe {
            return false;
        }
        // 已排队待搬迁 → 不重复提示。
        if khaslana::exe_relocation_pending_marker().is_some_and(|marker| marker.exists()) {
            return false;
        }
        true
    }

    /// 检测是否应提示用户把程序（连同数据）移出风险目录。
    fn maybe_prompt_exe_relocation(&mut self) {
        if !self.exe_relocation_available() {
            return;
        }
        if self.storage.exe_relocation_dismissed() {
            return;
        }
        // 便携迁移提示优先；两者都满足时本轮只弹一个，位置风险下一轮再提示。
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(DialogState::ExeRelocationPrompt);
    }

    /// 用户确认搬迁：写入待搬迁标记（固定目录下）后重启，
    /// 下次启动最早期把程序与数据搬到安全目录并从新位置运行。
    fn confirm_exe_relocation(&mut self) {
        self.active_dialog = None;
        let Some(target) = khaslana::exe_relocation_target_dir() else {
            self.last_error = Some("无法定位搬迁目标目录".to_string());
            return;
        };
        if let Err(err) = khaslana::request_exe_relocation(&target) {
            self.last_error = Some(err.to_string());
            return;
        }
        Self::relaunch_app();
    }

    /// 用户保持现状：永久忽略程序位置风险提示（数据本身已受解析规则保护）。
    fn dismiss_exe_relocation(&mut self) {
        let _ = self.storage.mark_exe_relocation_dismissed();
        self.active_dialog = None;
    }

    pub(crate) fn scroll_handle(&self, id: &'static str) -> ScrollHandle {
        let scoped_id = self
            .active_tab
            .map(|tab_id| format!("tab-{}:{id}", tab_id.0))
            .unwrap_or_else(|| format!("global:{id}"));
        self.scroll_handles
            .borrow_mut()
            .entry(scoped_id)
            .or_default()
            .clone()
    }

    fn scroll_local_branch_to_current(&self) {
        // 分组标题钉住后，本地分支是独立虚拟列表：在过滤后的条目模型里定位
        // HEAD 行号，交给该列表的滚动目标机制。分组折叠时不渲染列表，跳过定位。
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if !self
            .sidebar_sections
            .is_expanded(SidebarSection::LocalBranches)
        {
            return;
        }
        let Some(head_branch_index) = snapshot
            .branches
            .iter()
            .position(|branch| branch.kind == BranchKind::Local && branch.is_head)
        else {
            return;
        };
        let local_query = if self.sidebar_local_branch_search_open {
            self.sidebar_local_branch_search.value.trim()
        } else {
            ""
        };
        let entries = sidebar_view::sidebar_local_branch_entries(&snapshot.branches, local_query);
        let Some(row) = entries.iter().position(|item| {
            matches!(item, sidebar_view::SidebarNavItem::Branch(index) if *index == head_branch_index)
        }) else {
            return;
        };
        self.uniform_scroll_handle(sidebar_view::sidebar_section_scroll_id(
            SidebarSection::LocalBranches,
        ))
        .scroll_to_item(row, ScrollStrategy::Center);
    }

    pub(crate) fn uniform_scroll_handle(&self, id: &'static str) -> UniformListScrollHandle {
        let scoped_id = self
            .active_tab
            .map(|tab_id| format!("tab-{}:{id}", tab_id.0))
            .unwrap_or_else(|| format!("global:{id}"));
        self.uniform_scroll_handles
            .borrow_mut()
            .entry(scoped_id)
            .or_insert_with(UniformListScrollHandle::new)
            .clone()
    }

    pub(crate) fn reset_uniform_scroll(&self, id: &'static str) {
        let handle = self.uniform_scroll_handle(id);
        handle
            .0
            .borrow_mut()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    pub(crate) fn active_tab_id(&self) -> Option<RepoTabId> {
        self.active_tab
    }

    fn active_tab(&self) -> Option<&RepoTabState> {
        let id = self.active_tab?;
        self.tabs.iter().find(|tab| tab.id == id)
    }

    fn tab_mut(&mut self, tab_id: RepoTabId) -> Option<&mut RepoTabState> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    fn tab(&self, tab_id: RepoTabId) -> Option<&RepoTabState> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
    }

    fn ensure_tab_for_path(&mut self, path: PathBuf) -> RepoTabId {
        // 记录切换前的主模式：打开/切换仓库保持「当前区域」不变
        //（主模式跟随；专用页面绑定 per-repo 状态不继承）。
        let previous_mode = self.current_tab_main_mode();
        let key = normalize_repo_path(&path);
        let existing_id = self
            .tabs
            .iter()
            .find(|tab| tab.path_key().as_deref() == Some(key.as_str()))
            .map(|tab| tab.id);
        if let Some(id) = existing_id {
            if let Some(tab) = self.tab_mut(id) {
                tab.last_active_at = now_epoch_secs();
            }
            self.active_tab = Some(id);
            self.inherit_main_mode(previous_mode);
            self.save_session();
            return id;
        }

        let id = RepoTabId(self.next_tab_id);
        self.next_tab_id = self.next_tab_id.wrapping_add(1).max(1);
        self.tabs.push(RepoTabState::new(id, Some(path)));
        self.active_tab = Some(id);
        self.inherit_main_mode(previous_mode);
        self.save_session();
        id
    }

    fn activate_tab(&mut self, tab_id: RepoTabId) {
        if self.active_tab == Some(tab_id) || self.tab(tab_id).is_none() {
            return;
        }
        let previous_mode = self.current_tab_main_mode();
        if self.active_dialog == Some(DialogState::SubmoduleManager) {
            self.close_dialog();
        }
        self.close_popups();
        if let Some(active) = self.active_tab
            && let Some(tab) = self.tab_mut(active)
        {
            tab.release_large_diff_caches();
            tab.submodule_dialog.invalidate();
        }
        self.active_tab = Some(tab_id);
        if let Some(tab) = self.tab_mut(tab_id) {
            tab.last_active_at = now_epoch_secs();
            // 记录最近打开时间，供仓库切换下拉排序。
            if let Some(path) = tab.repo_path.clone() {
                let _ = self.storage.upsert_recent_repo(&path);
            }
        }
        // 切换仓库保持当前区域：主模式（工作区/提交记录/工作流/图谱）跟随切换
        // 带过去；专用页面（追溯/浏览/冲突等）不继承，落回目标 tab 自身模式。
        self.inherit_main_mode(previous_mode);
        self.ensure_history_loaded();
        self.sync_conflict_mode_with_snapshot();
        self.save_session();
        // 切换仓库后自动本地刷新
        self.refresh();
    }

    /// 当前激活 tab 的主模式（无激活 tab 时 None）。
    fn current_tab_main_mode(&self) -> Option<MainMode> {
        self.active_tab
            .and_then(|id| self.tab(id))
            .map(|tab| tab.main_mode)
    }

    /// 主模式继承：切换/打开/克隆仓库时把切换前所在的主页面写到新激活的 tab。
    /// 专用模式（Conflict/Stash/Browse/Blame）绑定 per-repo 状态，不继承。
    /// 必须在 `active_tab` 已指向目标 tab 之后调用（经 Deref 写入该 tab）。
    fn inherit_main_mode(&mut self, previous: Option<MainMode>) {
        if let Some(mode) = inheritable_main_mode(previous) {
            self.main_mode = mode;
        }
    }

    fn close_tab(&mut self, tab_id: RepoTabId) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if self.active_tab == Some(tab_id)
            && self.active_dialog == Some(DialogState::SubmoduleManager)
        {
            self.close_dialog();
        }
        self.close_popups();
        self.tabs.remove(index);
        // 清理该 tab 的滚动句柄：句柄按 `tab-{id}:` 前缀入表，不随 tab 关闭
        // 移除会长期缓慢积累（频繁开合仓库时无上限）。
        {
            let prefix = format!("tab-{}:", tab_id.0);
            self.scroll_handles
                .borrow_mut()
                .retain(|key, _| !key.starts_with(&prefix));
            self.uniform_scroll_handles
                .borrow_mut()
                .retain(|key, _| !key.starts_with(&prefix));
        }
        let mut retained = VecDeque::new();
        while let Some(pending) = self.pending_credentials.pop_front() {
            if pending.tab_id == Some(tab_id) {
                send_credential_response(&pending, Ok(None));
            } else {
                retained.push_back(pending);
            }
        }
        self.pending_credentials = retained;
        self.repository_load_queue
            .retain(|request| request.tab_id != tab_id);
        if self
            .pending_credential
            .as_ref()
            .and_then(|pending| pending.tab_id)
            == Some(tab_id)
        {
            if let Some(pending) = self.pending_credential.as_ref() {
                send_credential_response(pending, Ok(None));
            }
            self.show_next_credential_request();
        }
        if self.active_tab == Some(tab_id) {
            self.active_tab = self
                .tabs
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.tabs.get(prev)))
                .map(|tab| tab.id);
        }
        self.save_session();
    }

    fn open_storage() -> (Arc<khaslana::AppStorage>, Option<String>, Option<String>) {
        match khaslana::AppStorage::open_default() {
            Ok(storage) => (Arc::new(storage), None, None),
            Err(first_err) => {
                tracing::warn!("local config database open failed, recreating: {first_err}");
                match khaslana::AppStorage::recreate_default_after_failure() {
                    Ok(storage) => (
                        Arc::new(storage),
                        Some("本地配置数据库已重建".to_string()),
                        Some(format!("原数据库打开失败，已创建空数据库：{first_err}")),
                    ),
                    Err(second_err) => {
                        tracing::warn!(
                            "local config database recreate failed, using memory database: {second_err}"
                        );
                        let storage =
                            khaslana::AppStorage::open_in_memory().unwrap_or_else(|err| {
                                panic!("无法创建临时配置数据库：{err}");
                            });
                        (
                            Arc::new(storage),
                            Some("正在使用临时配置数据库".to_string()),
                            Some(format!("本地配置数据库不可用：{second_err}")),
                        )
                    }
                }
            }
        }
    }

    fn load_session_state(&self) -> Option<SessionState> {
        match self.storage.load_session_state() {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!("session load skipped: {err}");
                None
            }
        }
    }

    fn load_diff_encoding_preferences(storage: &khaslana::AppStorage) -> DiffEncodingPreferences {
        storage
            .load_diff_encoding_preferences()
            .inspect_err(|err| tracing::warn!("diff encoding preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_remote_credential_bindings(storage: &khaslana::AppStorage) -> RemoteCredentialBindings {
        storage
            .load_remote_credential_bindings()
            .inspect_err(|err| tracing::warn!("remote credential bindings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_proxy_settings(storage: &khaslana::AppStorage) -> NetworkProxySettings {
        storage
            .load_proxy_settings()
            .inspect_err(|err| tracing::warn!("network proxy settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_ai_provider_settings(storage: &khaslana::AppStorage) -> AiProviderSettings {
        storage
            .load_ai_provider_settings()
            .inspect_err(|err| tracing::warn!("ai provider settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_external_merge_settings(storage: &khaslana::AppStorage) -> ExternalMergeSettings {
        storage
            .load_external_merge_settings()
            .inspect_err(|err| tracing::warn!("external merge settings load skipped: {err}"))
            .unwrap_or_default()
    }

    fn load_update_preferences(storage: &khaslana::AppStorage) -> UpdatePreferences {
        storage
            .load_update_preferences()
            .inspect_err(|err| tracing::warn!("update preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    /// 加载快捷键绑定；存储为空或出错时回退全部默认值。
    fn load_shortcut_bindings(storage: &khaslana::AppStorage) -> ShortcutBindings {
        let stored = storage
            .load_shortcut_bindings()
            .inspect_err(|err| tracing::warn!("shortcut bindings load skipped: {err}"))
            .unwrap_or_default();
        // 合并：存储的绑定优先，缺失的动作用默认值补齐，保证新动作一定有快捷键。
        let mut result = default_shortcut_bindings();
        for (id, keystroke) in stored.bindings {
            if ShortcutAction::from_id(&id).is_some() {
                result.bindings.insert(id, keystroke);
            }
        }
        result
    }

    /// 保存当前快捷键绑定到数据库。
    fn save_shortcut_bindings(&self) {
        if let Err(err) = self.storage.save_shortcut_bindings(&self.shortcut_bindings) {
            tracing::warn!("shortcut bindings write skipped: {err}");
        }
    }

    /// 加载布局偏好；存储为空或出错时回退全部默认值。
    fn load_layout_preferences(storage: &khaslana::AppStorage) -> khaslana::LayoutPreferences {
        storage
            .load_layout_preferences()
            .inspect_err(|err| tracing::warn!("layout preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    /// 保存布局偏好（导航器展开 + 全部分割线位置）。
    ///
    /// 仅在离散用户动作后调用（拖拽结束/双击复位/导航器开合/详情卡折叠），
    /// UI 线程同步写：单行 <200B 的本地 SQLite 写无感知卡顿，同步保证操作
    /// 顺序落库且「改完即关应用」不丢最后一次修改（与主题/快捷键保存同模式）。
    pub(crate) fn save_layout_preferences(&self) {
        let preferences = khaslana::LayoutPreferences {
            navigator_visible: Some(self.context_navigator_preferences.visible),
            sidebar_width: Some(self.sidebar_width),
            changes_width: Some(self.changes_width),
            workflow_templates_width: Some(self.workflow_templates_width),
            history_files_width: Some(self.history_files_width),
            history_inspector_files_width: Some(self.history_inspector_files_width),
            history_graph_width: Some(self.history_graph_width),
            browse_tree_width: Some(self.browse_tree_width),
            history_details_height: self.history_details_height,
            history_details_collapsed: self.history_details_collapsed,
        };
        if let Err(err) = self.storage.save_layout_preferences(&preferences) {
            tracing::warn!("layout preferences write skipped: {err}");
        }
    }

    fn save_diff_encoding_preferences(&self) {
        if let Err(err) = self
            .storage
            .save_diff_encoding_preferences(&self.diff_encoding_preferences)
        {
            tracing::warn!("diff encoding preferences write skipped: {err}");
        }
    }

    fn save_remote_credential_bindings(&self) {
        let Ok(bindings) = self.remote_credential_bindings.lock() else {
            tracing::warn!("remote credential bindings state read skipped");
            return;
        };
        if let Err(err) = self.storage.save_remote_credential_bindings(&bindings) {
            tracing::warn!("remote credential bindings write skipped: {err}");
        }
    }

    pub(crate) fn save_ai_provider_settings(&self) {
        if let Err(err) = self.storage.save_ai_provider_settings(&self.ai_settings) {
            tracing::warn!("ai provider settings write skipped: {err}");
        }
    }

    pub(crate) fn save_external_merge_settings(&self) {
        if let Err(err) = self
            .storage
            .save_external_merge_settings(&self.external_merge_settings)
        {
            tracing::warn!("external merge settings write skipped: {err}");
        }
    }

    pub(crate) fn save_proxy_settings(&self) {
        if let Err(err) = self.storage.save_proxy_settings(&self.proxy_settings) {
            tracing::warn!("network proxy settings write skipped: {err}");
        }
    }

    pub(crate) fn diff_encoding_choice_for_path(&self, path: &Path) -> DiffEncodingChoice {
        self.diff_encoding_preferences
            .repositories
            .get(&normalize_repo_path(path))
            .copied()
            .unwrap_or_default()
    }

    fn current_diff_encoding_choice(&self) -> DiffEncodingChoice {
        self.repo_path
            .as_ref()
            .map(|path| self.diff_encoding_choice_for_path(path))
            .unwrap_or_default()
    }

    pub(crate) fn set_current_diff_encoding(&mut self, encoding: DiffEncodingChoice) {
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let key = normalize_repo_path(&repo_path);
        if encoding == DiffEncodingChoice::Auto {
            self.diff_encoding_preferences.repositories.remove(&key);
        } else {
            self.diff_encoding_preferences
                .repositories
                .insert(key, encoding);
        }
        self.save_diff_encoding_preferences();
        self.status = format!("差异编码已切换为 {}", encoding.label());
        self.reload_visible_diffs_after_encoding_change();
    }

    fn reload_visible_diffs_after_encoding_change(&mut self) {
        self.diff_cache.borrow_mut().clear();
        if let Some(diff) = self.diff.clone() {
            self.load_diff(diff.path.clone(), diff.scope.clone());
        }
        if self.main_mode == MainMode::History
            && let Some(path) = self.history_selected_file.clone()
        {
            self.select_history_file_with_reload(path, true);
        }
        if self.main_mode == MainMode::Stash
            && let Some(path) = self.stash_preview.selected_file.clone()
        {
            self.select_stash_file(path, true);
        }
        if self.main_mode == MainMode::Browse {
            self.reload_browse_on_encoding_change();
        }
        if self.main_mode == MainMode::Blame {
            self.reload_blame_on_encoding_change();
        }
    }

    fn save_session(&self) {
        if self.restoring_session {
            return;
        }
        let repo_paths = dedupe_repo_paths(
            self.tabs
                .iter()
                .filter_map(|tab| tab.repo_path.clone())
                .collect::<Vec<_>>(),
        );
        let active_repo_path = self
            .active_tab()
            .and_then(|tab| tab.repo_path.as_ref())
            .cloned();
        let state = SessionState {
            repo_paths,
            active_repo_path,
        };
        if let Err(err) = self.storage.save_session_state(&state) {
            tracing::warn!("session write skipped: {err}");
        }
    }

    fn restore_session(&mut self) {
        let Some(session) = self.load_session_state() else {
            return;
        };
        self.restoring_session = true;
        let mut restored = Vec::new();
        let mut failed = 0usize;
        let mut seen = BTreeSet::new();

        for path in session.repo_paths {
            let key = normalize_repo_path(&path);
            if !seen.insert(key) {
                continue;
            }
            if !path.exists() || Repository::open(&path).is_err() {
                failed += 1;
                continue;
            }
            restored.push(path);
        }

        if restored.is_empty() {
            if failed > 0 {
                self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
                self.fallback_tab.status = "会话恢复失败".to_string();
            }
            self.restoring_session = false;
            self.save_session();
            return;
        }

        let active_key = session
            .active_repo_path
            .as_ref()
            .map(|path| normalize_repo_path(path));
        let mut active = None;
        for path in restored {
            let id = self.ensure_tab_for_path(path.clone());
            if active_key.as_deref() == Some(normalize_repo_path(&path).as_str()) {
                active = Some(id);
            }
        }
        if let Some(active) = active.or(self.active_tab) {
            self.active_tab = Some(active);
        }
        if failed > 0 {
            self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
        }

        let tabs = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        for tab_id in tabs {
            if let Some(path) = self.tab(tab_id).and_then(|tab| tab.repo_path.clone()) {
                self.queue_repository_load(
                    tab_id,
                    path,
                    "正在恢复仓库",
                    "仓库已恢复",
                    LoadPriority::Background,
                );
            }
        }
        self.restoring_session = false;
        self.save_session();
    }

    pub(crate) fn active_tab_state(&self) -> &RepoTabState {
        self.active_tab().unwrap_or_else(|| &self.fallback_tab)
    }

    pub(crate) fn active_tab_state_mut(&mut self) -> &mut RepoTabState {
        let id = self.active_tab;
        if let Some(id) = id
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            return &mut self.tabs[index];
        }
        &mut self.fallback_tab
    }

    pub(crate) fn service_for_tab(&self, tab_id: RepoTabId) -> GitService {
        GitService::new(
            Arc::new(TabCredentialProvider::new(
                self.credential_store.clone(),
                self.storage.clone(),
                self.remote_credential_bindings.clone(),
                self.tx.clone(),
                tab_id,
                self.proxy_settings.clone(),
            )),
            Arc::new(TabProgress {
                tx: self.tx.clone(),
                tab_id,
            }),
        )
        .with_proxy_settings(self.proxy_settings.clone())
    }

    pub(crate) fn with_tab_context<R>(
        &mut self,
        tab_id: RepoTabId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        self.tab(tab_id)?;
        let previous = self.active_tab;
        self.active_tab = Some(tab_id);
        let result = f(self);
        self.active_tab = previous
            .filter(|id| self.tab(*id).is_some())
            .or_else(|| self.tabs.first().map(|tab| tab.id));
        Some(result)
    }

    pub(crate) fn apply_status_event(
        &mut self,
        tab_id: Option<RepoTabId>,
        f: impl FnOnce(&mut Self),
    ) {
        if let Some(tab_id) = tab_id {
            let _ = self.with_tab_context(tab_id, f);
        } else {
            f(self);
        }
    }

    /// 全局测试类操作开始借用 busy：记录发起时的活动 tab 并置 busy。
    fn begin_global_test_busy(&mut self, status: &str) {
        self.global_busy_tab = self.active_tab;
        self.busy = true;
        self.status = status.into();
        self.last_error = None;
    }

    /// 复位全局测试借用的 tab busy；tab 已在测试期间关闭时静默忽略。
    fn end_global_test_busy(&mut self) {
        if let Some(tab_id) = self.global_busy_tab.take() {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.busy = false;
            }
        }
    }

    fn disabled_reason(&self, enabled: bool, fallback: &'static str) -> Option<&'static str> {
        if enabled {
            None
        } else if self.busy {
            Some("当前操作运行中")
        } else if self.repo_path.is_none() {
            Some("请先打开仓库")
        } else {
            Some(fallback)
        }
    }

    /// 入队凭据请求。返回 `true` 表示该请求被提升为当前待处理（此前没有
    /// pending），`false` 表示已排队（当前表单保持不动，用户输入不丢失）。
    fn enqueue_credential_request(&mut self, pending: PendingCredential) -> bool {
        if pending
            .tab_id
            .is_some_and(|tab_id| self.tab(tab_id).is_none())
        {
            return false;
        }
        if self.pending_credential.is_none() {
            self.pending_credential = Some(pending);
            true
        } else {
            self.pending_credentials.push_back(pending);
            false
        }
    }

    fn show_next_credential_request(&mut self) {
        self.pending_credential = None;
        while let Some(pending) = self.pending_credentials.pop_front() {
            if pending
                .tab_id
                .is_none_or(|tab_id| self.tab(tab_id).is_some())
            {
                self.pending_credential = Some(pending);
                self.prepare_current_credential_prompt();
                break;
            }
        }
    }

    fn prepare_current_credential_prompt(&mut self) {
        let Some(pending) = self.pending_credential.as_ref() else {
            return;
        };
        let request = pending.request.clone();
        self.save_credential = true;
        self.credential_scope = CredentialScope::RemoteUrl;
        self.credential_form_mode = credential_form_mode_for_request(&request);
        self.credential_use_ssh_agent = false;
        self.credential_username.set_value(
            request
                .username_from_url
                .clone()
                .unwrap_or_else(|| "git".to_string()),
        );
        self.credential_secret.clear();
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        self.credential_display_name.clear();
        self.credential_remote_url.set_value(request.url);
    }

    fn spawn_event_pump(rx: Receiver<UiEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |weak: WeakEntity<RepositoryView>, cx| {
            while let Ok(event) = rx.recv().await {
                if weak
                    .update(cx, |this, cx| {
                        this.handle_ui_event(event, cx);
                        this.drain_pending_events(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                let _ = cx.refresh();
            }
        })
        .detach();
    }

    fn drain_pending_events(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = self.rx.try_recv() {
            self.handle_ui_event(event, cx);
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::UiTick => {
                self.progress_phase = self.progress_phase.wrapping_add(1);
                let now = Instant::now();
                let feedbacks_before = self.feedbacks.len();
                self.feedbacks.retain(|feedback| !feedback.is_expired(now));
                let feedbacks_expired = self.feedbacks.len() != feedbacks_before;
                self.sync_conflict_editor_into_state();
                self.handle_tray_action(cx);
                // 空闲期不做全窗口重绘（UiTick 每 420ms 一次，无条件 notify 会让
                // 应用常驻 2.4Hz 重绘并触发渲染路径里的重复计算）：只在时态内容
                // 变化时通知——底部进度线在动画（有加载中操作）、操作遮罩到延迟
                // 阈值需要显示、或有过期反馈被移除。托盘动作无需重绘。
                if self.has_active_loading()
                    || self.active_operation_blocker_message().is_some()
                    || feedbacks_expired
                {
                    cx.notify();
                }
                // UiTick 不走事件末尾的统一 notify。
                return;
            }
            UiEvent::OperationStarted { tab_id, message } => {
                self.apply_status_event(tab_id, |this| {
                    this.busy = true;
                    this.operation_kind = OperationKind::from_message(&message);
                    this.status = message;
                    this.last_error = None;
                });
            }
            UiEvent::OperationProgress { tab_id, message } => {
                self.apply_status_event(tab_id, |this| {
                    this.status = message;
                });
            }
            UiEvent::RepositoryFastLoaded {
                tab_id,
                message,
                snapshot,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        let was_merge_in_progress = this.merge_in_progress();
                        let merge_in_progress = snapshot.merge_in_progress;
                        let merge_message = snapshot.merge_message.clone();
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.loading = RepositoryLoading {
                            metadata: true,
                            status_fast: true,
                            status_full: true,
                        };
                        this.status = message;
                        this.last_error = None;
                        this.diff = None;
                        this.branch_sync_status = None;
                        this.branch_sync_loading = false;
                        this.refresh_history();
                        this.change_selection.clear();
                        this.repo_path = Some(snapshot.path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        this.snapshot = Some(snapshot);
                        this.sync_merge_message_transition(
                            was_merge_in_progress,
                            merge_in_progress,
                            merge_message,
                        );
                        this.sync_conflict_mode_with_snapshot();
                        this.scroll_local_branch_to_current();
                        // 仓库（重）加载后历史引用已变：已有历史列表时不受视图限制地后台刷新，
                        // 覆盖切换分支/拉取/推送等引用类操作；初始打开（列表为空）不预加载。
                        this.reload_history_after_change();
                    }
                });
            }
            UiEvent::RepositoryMetadataLoaded {
                tab_id,
                message,
                snapshot,
                load_id,
            } => {
                let mut sync_request = None;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.loading.metadata = false;
                        this.status = message;
                        this.merge_metadata_snapshot(snapshot);
                        this.scroll_local_branch_to_current();
                        sync_request = this.prepare_branch_sync_status_request();
                    }
                });
                if let Some((tab_id, path, remote, load_id, request_id)) = sync_request {
                    self.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
                }
            }
            UiEvent::RepositoryStatusFastLoaded {
                tab_id,
                message,
                changes,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading.status_fast = false;
                        this.status = message;
                        this.replace_changes(changes);
                    }
                });
            }
            UiEvent::RepositoryStatusFullLoaded {
                tab_id,
                message,
                changes,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading.status_full = false;
                        this.status = message;
                        this.replace_changes(changes);
                    }
                });
            }
            UiEvent::RepositoryLoadStageFailed {
                tab_id,
                error,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.loading = RepositoryLoading::default();
                        this.operation_kind = OperationKind::Local;
                        this.status = "仓库已打开，后台加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::RepositoryLoadFinished { tab_id, load_id } => {
                self.active_repository_loads = self.active_repository_loads.saturating_sub(1);
                if self
                    .tab(tab_id)
                    .is_some_and(|tab| tab.repository_load_id == load_id)
                {
                    self.apply_status_event(Some(tab_id), |this| {
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.operation_kind = OperationKind::Local;
                    });
                }
                self.start_queued_repository_loads();
            }
            UiEvent::OperationFinished {
                tab_id,
                message,
                snapshot,
                diff,
            } => {
                let toast_message = message.clone();
                let keeps_remote_branch_dialog = matches!(
                    self.active_dialog.as_ref(),
                    Some(DialogState::RemoteBranchOperation { .. })
                );
                let should_refresh_repository =
                    operation_requires_repository_refresh(&message) && !keeps_remote_branch_dialog;
                let affects_commit_history = operation_affects_commit_history(&message);
                let should_refresh_submodules = operation_refreshes_submodule_dialog(&message)
                    && self.active_dialog == Some(DialogState::SubmoduleManager);
                let has_snapshot = snapshot.is_some();
                let has_diff = diff.is_some();
                let mut full_status_request = None;
                let mut sync_request = None;
                let mut repository_refresh_request = None;
                self.apply_status_event(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.remote_branch_operation.refreshing = false;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = message;
                    if let Some(snapshot) = snapshot {
                        let was_merge_in_progress = this.merge_in_progress();
                        let merge_in_progress = snapshot.merge_in_progress;
                        let merge_message = snapshot.merge_message.clone();
                        this.repo_path = (!snapshot.path.as_os_str().is_empty())
                            .then(|| snapshot.path.clone())
                            .or_else(|| this.repo_path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        if !snapshot.conflicts.is_empty() {
                            this.diff = None;
                            this.diff_headers_expanded = false;
                            this.reset_uniform_scroll("diff-scroll");
                        }
                        this.snapshot = Some(snapshot);
                        this.sync_merge_message_transition(
                            was_merge_in_progress,
                            merge_in_progress,
                            merge_message,
                        );
                        this.prune_stash_preview();
                        this.prune_change_selection();
                        this.sync_conflict_mode_with_snapshot();
                        this.refresh_history();
                        this.scroll_local_branch_to_current();
                        if affects_commit_history {
                            // 创建/移动提交或 HEAD 的操作：无论当前视图都后台刷新
                            // 提交记录及其 HEAD/分支/标签徽章，不再需要人工刷新。
                            this.reload_history_after_change();
                        } else {
                            this.reload_history_if_active();
                        }
                        if let Some(tab_id) = tab_id {
                            if should_refresh_repository {
                                repository_refresh_request =
                                    this.repo_path.clone().map(|path| (tab_id, path));
                            } else {
                                full_status_request = this
                                    .repo_path
                                    .clone()
                                    .map(|path| (tab_id, path, this.repository_load_id));
                                this.loading.status_full = true;
                            }
                        }
                        if !should_refresh_repository {
                            sync_request = this.prepare_branch_sync_status_request();
                        }
                    }
                    if let Some(diff) = diff {
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::Worktree {
                                    scope: diff.scope.clone(),
                                    path: diff.path.clone(),
                                },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.diff = Some(diff);
                        this.diff_headers_expanded = false;
                        this.diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::WorktreeDiff);
                        this.reset_uniform_scroll("diff-scroll");
                    }
                });
                // 暂存/取消暂存（整文件或按块/按行）后差异面板跟随刷新。
                // 必须先于全量状态补全/分支同步请求执行：load_diff 会经
                // spawn_operation 递增 repository_load_id（diff 缓存失效机制），
                // 这些后台请求若沿用闭包内捕获的旧代际，结果到达时会因代际
                // 守卫不匹配被丢弃，变更列表将停留在不含未跟踪文件的操作快照上。
                if operation_refreshes_worktree_diff(&toast_message) {
                    self.refresh_diff_after_stage_change();
                }
                if let Some((tab_id, path, _)) = full_status_request {
                    // 代际需在差异重载之后取最新值（见上），否则结果会被守卫丢弃。
                    if let Some(load_id) = self.tab(tab_id).map(|tab| tab.repository_load_id) {
                        self.load_full_status_for_tab(
                            tab_id,
                            path,
                            load_id,
                            "变更已补全".to_string(),
                        );
                    }
                }
                if let Some((tab_id, path, remote, _, request_id)) = sync_request {
                    if let Some(load_id) = self.tab(tab_id).map(|tab| tab.repository_load_id) {
                        self.load_branch_sync_status_for_tab(
                            tab_id, path, remote, load_id, request_id,
                        );
                    }
                }
                if let Some((tab_id, path)) = repository_refresh_request {
                    // 分支引用变化后重新走完整仓库加载，避免只应用操作快照时遗漏引用或状态更新。
                    self.queue_repository_load(
                        tab_id,
                        path,
                        "正在刷新分支状态",
                        "分支状态已刷新",
                        LoadPriority::Background,
                    );
                }
                if should_notify_operation_finished(&toast_message, has_snapshot, has_diff) {
                    self.notify_completion(&toast_message, cx);
                }
                if should_refresh_submodules {
                    self.load_submodules();
                }
            }
            UiEvent::DiscardChangeFinished {
                tab_id,
                message,
                snapshot,
                changes,
                load_id,
            } => {
                let toast_message = message.clone();
                let mut should_notify = false;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        should_notify = true;
                        this.busy = false;
                        this.operation_blocker = OperationBlocker::None;
                        this.operation_blocker_started = None;
                        this.operation_kind = OperationKind::Local;
                        this.loading = RepositoryLoading::default();
                        this.status = message;
                        this.last_error = None;
                        this.repo_path = Some(snapshot.path.clone());
                        this.sync_selected_remote(&snapshot);
                        this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                        this.snapshot = Some(snapshot);
                        this.prune_stash_preview();
                        this.sync_conflict_mode_with_snapshot();
                        this.replace_changes(changes);
                        this.diff = None;
                        this.diff_headers_expanded = false;
                        this.reset_uniform_scroll("diff-scroll");
                        this.refresh_history();
                        this.scroll_local_branch_to_current();
                        this.reload_history_if_active();
                    }
                });
                if should_notify {
                    self.notify_success(toast_message, cx);
                }
            }
            UiEvent::CredentialRecordsLoaded { records, message } => {
                let toast_message = message.clone();
                self.end_global_test_busy();
                self.operation_blocker = OperationBlocker::None;
                self.operation_blocker_started = None;
                self.credential_records = records;
                self.status = message;
                self.last_error = None;
                if Self::should_toast_completion(&toast_message) {
                    self.notify_completion(&toast_message, cx);
                }
            }
            UiEvent::SshCredentialsDiscovered { request_id, result } => {
                if request_id == self.ssh_credential_discovery.request_id {
                    let key_count = result.keys.len();
                    let agent_count = result.agent_identities.len();
                    self.ssh_credential_discovery.loading = false;
                    self.ssh_credential_discovery.result = Some(result);
                    self.ssh_credential_discovery.error = None;
                    self.status =
                        format!("已发现 {key_count} 个 SSH 私钥，Agent 中有 {agent_count} 个身份");
                }
            }
            UiEvent::SshCredentialDiscoveryFailed { request_id, error } => {
                if request_id == self.ssh_credential_discovery.request_id {
                    self.ssh_credential_discovery.loading = false;
                    self.ssh_credential_discovery.error = Some(error.clone());
                    self.status = "本机 SSH 检测失败".into();
                    self.last_error = Some(error);
                }
            }
            UiEvent::OAuthLoginReady {
                request_id,
                provider,
                url,
                user_code,
            } => {
                if request_id == self.oauth_login_flow.request_id {
                    self.oauth_login_flow.user_code = user_code;
                    self.oauth_login_flow.verification_uri = Some(url.clone());
                    open_url(&url);
                    self.status = format!("请在浏览器中完成{}登录", provider.label());
                }
            }
            UiEvent::OAuthLoginSucceeded {
                request_id,
                provider,
                username,
                token,
                gitee_refresh,
            } => {
                if request_id == self.oauth_login_flow.request_id {
                    let (host_url, note) = match provider {
                        OAuthProvider::Github => ("github.com", "GitHub 登录成功，凭据已保存"),
                        OAuthProvider::Gitee => (
                            "gitee.com",
                            if gitee_refresh.is_some() {
                                "Gitee 登录成功，凭据已保存（令牌将自动续期）"
                            } else {
                                // 旧版 broker 未透传 refresh_token：保持手动续期提示。
                                "Gitee 登录成功，凭据已保存（令牌约 1 天过期，过期后重新登录）"
                            },
                        ),
                    };
                    let provider_label = provider.label();
                    self.oauth_login_flow.loading = false;
                    self.oauth_login_flow.provider = None;
                    self.oauth_login_flow.user_code = None;
                    self.oauth_login_flow.verification_uri = None;
                    self.oauth_login_flow.cancel = None;
                    // 把令牌当作 PAT 填入 HTTPS 凭据表单并直接保存，用户无需手动录入。
                    self.credential_form_mode = CredentialFormMode::Https;
                    self.credential_username.set_value(username.clone());
                    self.credential_secret.set_value(token);
                    if self.credential_display_name.value.trim().is_empty()
                        || !self.credential_display_name.value.contains(provider_label)
                    {
                        self.credential_display_name
                            .set_value(format!("{provider_label} · {username}"));
                    }
                    if !self
                        .credential_remote_url
                        .value
                        .to_ascii_lowercase()
                        .contains(host_url)
                    {
                        self.credential_remote_url
                            .set_value(format!("https://{host_url}"));
                    }
                    self.credential_scope = CredentialScope::Host;
                    self.save_credential_form();
                    // Gitee：把自动续期材料落到独立 Keyring 条目（绑定刚保存的记录）。
                    if let (OAuthProvider::Gitee, Some(record_id)) =
                        (provider, self.pending_gitee_refresh_record.take())
                        && let Some((refresh_token, expires_at)) = gitee_refresh
                    {
                        if let Err(err) = khaslana::credentials::save_gitee_refresh_payload(
                            &record_id,
                            &refresh_token,
                            expires_at,
                        ) {
                            tracing::warn!("Gitee 续期材料保存失败：{err}");
                            self.notify_warning(
                                "Gitee 令牌已保存，但自动续期材料写入失败，令牌过期后需重新登录",
                                cx,
                            );
                        }
                    } else {
                        self.pending_gitee_refresh_record = None;
                    }
                    self.notify_success(note, cx);
                }
            }
            UiEvent::GiteeTokenRefreshed { success, message } => {
                if success {
                    self.status = message;
                } else {
                    // 续期失败不阻断当前操作（旧令牌继续尝试），提示重新登录即可恢复。
                    self.notify_warning(message, cx);
                }
            }
            UiEvent::OAuthLoginFailed { request_id, error } => {
                if request_id == self.oauth_login_flow.request_id {
                    let label = self
                        .oauth_login_flow
                        .provider
                        .map(|p| p.label())
                        .unwrap_or("OAuth");
                    self.oauth_login_flow.loading = false;
                    self.oauth_login_flow.provider = None;
                    self.oauth_login_flow.user_code = None;
                    self.oauth_login_flow.verification_uri = None;
                    self.oauth_login_flow.cancel = None;
                    self.oauth_login_flow.error = Some(error.clone());
                    self.last_error = Some(error.clone());
                    self.notify_error(format!("{label} 登录失败：{error}"), cx);
                }
            }
            UiEvent::CredentialSshKeyFileSelected { path } => {
                if let Some(path) = path {
                    self.use_discovered_ssh_key(path);
                } else {
                    self.status = "已取消选择 SSH 私钥".into();
                }
            }
            UiEvent::HistoryCommitsLoaded {
                tab_id,
                commits,
                refs_cache,
                append,
                has_more,
                scope,
                path_filter,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅最新一代请求（seq 匹配）复位加载标志并应用数据；
                    // 旧一代晚到的结果直接丢弃，避免覆盖新数据。
                    // seq 匹配但 load_id 失配（操作作废了本次请求）也要复位标志，
                    // 否则 history_loading.commits 永久为 true 会吞掉后续所有历史加载。
                    if seq == this.history_load_seq {
                        this.history_loading.commits = false;
                        if this.history_commits_event_matches(
                            load_id,
                            scope,
                            path_filter.as_deref(),
                        ) {
                            this.history_refs_cache = Some(refs_cache);
                            this.history_has_more = has_more;
                            if append {
                                this.history_commits.extend(commits);
                            } else {
                                this.history_commits = commits;
                                let was_refreshing = this.history_refreshing;
                                if was_refreshing {
                                    // 刷新时保留选中提交（若仍存在于新列表）。
                                    // 选中的文件列表与差异按 oid 不可变，一并保留，
                                    // 避免出现“详情显示选中提交、文件/差异永远空占位”
                                    // 的不一致（历史上这里的清空没有配套重载）。
                                    let still_exists =
                                        this.history_selected_commit.as_ref().is_some_and(|oid| {
                                            this.history_commits
                                                .iter()
                                                .any(|c| c.oid == oid.as_str())
                                        });
                                    if !still_exists {
                                        this.history_selected_commit = None;
                                        this.history_files.clear();
                                        this.history_selected_file = None;
                                        this.history_diff = None;
                                    }
                                } else {
                                    // 非刷新（scope 切换/初始加载）：全部重置
                                    this.history_selected_commit = None;
                                    this.history_files.clear();
                                    this.history_selected_file = None;
                                    this.history_diff = None;
                                }
                                this.history_refreshing = false;
                            }
                            // 过滤模式下隐藏提交图形列：过滤后中间提交缺失，
                            // 泳道线会断裂，跳过泳道计算并让行渲染不画图形。
                            this.history_graph_rows = if this.history_file_filter.is_none() {
                                commit_graph_view::commit_graph_rows(&this.history_commits)
                            } else {
                                Vec::new()
                            };

                            if this.history_selected_commit.is_none() {
                                if let Some(commit) = this.history_commits.first() {
                                    this.select_history_commit(commit.oid.clone());
                                } else {
                                    this.status = "当前分支暂无提交记录".to_string();
                                }
                            } else {
                                this.status = "提交记录已加载".to_string();
                                // 自愈：选中保留但文件列表为空（旧状态冻结、或此前的
                                // 文件加载被仓库重载作废）时重新拉取；非空时
                                // select_history_commit 幂等跳过，不影响现有展示。
                                if let Some(oid) = this.history_selected_commit.clone() {
                                    this.select_history_commit(oid);
                                }
                            }
                        }
                    }
                });
            }
            UiEvent::HistoryFilesLoaded {
                tab_id,
                commit_oid,
                files,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.history_selected_commit.as_deref() == Some(commit_oid.as_str())
                    {
                        this.history_loading.files = false;
                        this.history_files = files;
                        this.history_selected_file = None;
                        this.history_diff = None;
                        this.history_diff_headers_expanded = false;

                        if let Some(preferred) = preferred_history_file(
                            this.history_file_filter.as_deref(),
                            &this.history_files,
                        ) {
                            this.select_history_file(preferred);
                        } else {
                            this.status = "该提交没有文件变更".to_string();
                        }
                    }
                });
            }
            UiEvent::HistoryDiffLoaded {
                tab_id,
                commit_oid,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.history_selected_commit.as_deref() == Some(commit_oid.as_str())
                        && this.history_selected_file.as_deref() == Some(path.as_str())
                    {
                        this.history_loading.diff = false;
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::History { commit_oid, path },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.history_diff = Some(diff);
                        this.history_diff_headers_expanded = false;
                        this.history_diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::HistoryDiff);
                        this.reset_uniform_scroll("history-diff-scroll");
                        this.status = "提交差异已加载".to_string();
                    }
                });
            }
            UiEvent::StashFilesLoaded {
                tab_id,
                stash_oid,
                files,
                load_id,
            } => {
                let mut first_path = None;
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.stash_preview.stash_oid.as_deref() == Some(stash_oid.as_str())
                    {
                        this.stash_preview.loading_files = false;
                        this.stash_preview.files = files;
                        this.stash_preview.selected_file = None;
                        this.stash_preview.diff = None;
                        this.stash_preview.diff_headers_expanded = false;
                        first_path = this
                            .stash_preview
                            .files
                            .first()
                            .map(|file| file.path.clone());
                        if first_path.is_none() {
                            this.status = "该贮藏没有文件变更".to_string();
                        }
                    }
                });
                if let Some(path) = first_path
                    && self.active_tab == Some(tab_id)
                {
                    self.select_stash_file(path, false);
                }
            }
            UiEvent::StashDiffLoaded {
                tab_id,
                stash_oid,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.stash_preview.stash_oid.as_deref() == Some(stash_oid.as_str())
                        && this.stash_preview.selected_file.as_deref() == Some(path.as_str())
                    {
                        this.stash_preview.loading_diff = false;
                        let diff = Arc::new(diff);
                        if let Some(repo_path) = this.repo_path.as_deref() {
                            let cache_key = this.diff_cache_key(
                                DiffCacheKind::Stash { stash_oid, path },
                                repo_path,
                            );
                            this.cache_diff(cache_key, diff.clone());
                        }
                        this.stash_preview.diff = Some(diff);
                        this.stash_preview.diff_headers_expanded = false;
                        this.stash_preview.diff_syntax = None;
                        this.schedule_syntax_highlight(SyntaxSlot::StashDiff);
                        this.reset_uniform_scroll("stash-diff-scroll");
                        this.status = "贮藏差异已加载".to_string();
                    }
                });
            }
            UiEvent::HistoryLoadFailed {
                tab_id,
                error,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 失败（包括被 load_id 作废的陈旧失败）都要复位加载标志，
                    // 否则操作开始 bump load_id 后若操作失败，标志永久卡住，
                    // 后续所有历史/贮藏预览加载都会被在飞守卫静默吞掉。
                    this.history_loading = HistoryLoading::default();
                    this.history_refreshing = false;
                    this.stash_preview.loading_files = false;
                    this.stash_preview.loading_diff = false;
                    if load_id == this.repository_load_id {
                        this.status = if this.main_mode == MainMode::Stash {
                            "贮藏预览加载失败".to_string()
                        } else {
                            "提交记录加载失败".to_string()
                        };
                        this.last_error = Some(error);
                        // 全文视图过大时自动回退到紧凑差异
                        this.revert_full_file_if_too_large_error();
                    }
                });
            }
            UiEvent::CommitTraceLoaded {
                tab_id,
                branch,
                ahead_only,
                oids,
                truncated,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅最新代际（seq 匹配）应用：参数（分支/模式）变化必先递增 seq，
                    // 旧一代晚到的结果直接丢弃；仓库重载（load_id 失配）同样作废。
                    if seq != this.commit_graph.trace_seq || load_id != this.repository_load_id {
                        return;
                    }
                    this.commit_graph.trace_loading = false;
                    this.commit_graph.trace = Some(CommitTrace {
                        oids: Arc::new(oids.into_iter().collect()),
                        truncated,
                    });
                    let mode_label = if ahead_only {
                        "仅领先 HEAD"
                    } else {
                        "全谱系"
                    };
                    this.status = format!("已高亮分支 {branch}（{mode_label}）");
                });
            }
            UiEvent::CommitTraceLoadFailed {
                tab_id,
                error,
                load_id,
                seq,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if seq != this.commit_graph.trace_seq {
                        return;
                    }
                    this.commit_graph.trace_loading = false;
                    if load_id == this.repository_load_id {
                        this.status = "分支谱系计算失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::BranchSyncStatusLoaded {
                tab_id,
                status,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && request_id == this.branch_sync_request_id
                    {
                        this.branch_sync_loading = false;
                        this.branch_sync_status = status;
                    }
                });
            }
            UiEvent::BranchSyncStatusFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && request_id == this.branch_sync_request_id
                    {
                        this.branch_sync_loading = false;
                        this.branch_sync_status = None;
                        tracing::warn!("branch sync status skipped: {error}");
                    }
                });
            }
            UiEvent::SubmodulesLoaded {
                tab_id,
                items,
                load_id,
                request_id,
            } => {
                let mut should_load_remote_statuses = false;
                self.with_tab_context(tab_id, |this| {
                    if submodule_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.items = items;
                        this.submodule_dialog.remote_statuses.clear();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.loading = false;
                        this.submodule_dialog.loaded = true;
                        this.submodule_dialog.error = None;
                        this.submodule_dialog.remote_error = None;
                        should_load_remote_statuses = !this.submodule_dialog.items.is_empty()
                            && this.active_dialog == Some(DialogState::SubmoduleManager);
                        this.status = "子模块列表已加载".to_string();
                    }
                });
                if should_load_remote_statuses {
                    let _ = self.with_tab_context(tab_id, |this| {
                        this.load_submodule_remote_statuses();
                    });
                }
            }
            UiEvent::SubmodulesLoadFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.items.clear();
                        this.submodule_dialog.remote_statuses.clear();
                        this.submodule_dialog.loading = false;
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.loaded = false;
                        this.submodule_dialog.error = Some(error.clone());
                        this.submodule_dialog.remote_error = None;
                        this.status = "子模块列表加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
            }
            UiEvent::SubmoduleRemoteStatusesLoaded {
                tab_id,
                statuses,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_remote_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.remote_statuses = statuses.into_iter().collect();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.remote_error = None;
                        this.status = "子模块远端状态已检查".to_string();
                    }
                });
            }
            UiEvent::SubmoduleRemoteStatusesLoadFailed {
                tab_id,
                error,
                load_id,
                request_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if submodule_remote_request_matches(
                        &this.submodule_dialog,
                        this.repository_load_id,
                        load_id,
                        request_id,
                    ) {
                        this.submodule_dialog.remote_statuses = this
                            .submodule_dialog
                            .items
                            .iter()
                            .map(|module| {
                                (
                                    module.name.clone(),
                                    SubmoduleRemoteSyncStatus::Unavailable(error.clone()),
                                )
                            })
                            .collect();
                        this.submodule_dialog.remote_loading = false;
                        this.submodule_dialog.remote_error = Some(error.clone());
                        this.status = "子模块远端状态检查失败".to_string();
                        tracing::warn!("submodule remote status skipped: {error}");
                    }
                });
            }
            UiEvent::BlameLoaded {
                tab_id,
                path,
                view,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // load_id 与路径双校验：仓库重载或切换追溯文件后，
                    // 旧请求的结果不落地。
                    if load_id == this.repository_load_id
                        && this.blame.path.as_deref() == Some(path.as_str())
                    {
                        this.blame.loading = false;
                        this.blame.syntax = None;
                        this.blame.view = Some(Arc::new(view));
                        this.status = "文件追溯已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::Blame);
                    }
                });
            }
            UiEvent::BlameLoadFailed {
                tab_id,
                path,
                error,
                load_id,
            } => {
                let toast_message = error.clone();
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.blame.path.as_deref() == Some(path.as_str())
                    {
                        this.blame.loading = false;
                        this.status = "文件追溯加载失败".to_string();
                        this.last_error = Some(error);
                    }
                });
                self.notify_error(toast_message, cx);
            }
            UiEvent::SyntaxHighlighted {
                tab_id,
                slot,
                anchor,
                anchor_len,
                spans,
            } => {
                self.with_tab_context(tab_id, |this| {
                    // Arc 身份守卫：内容已被替换（或 Arc 地址被复用但行数不同）
                    // 时丢弃，避免旧高亮错配新内容。
                    let current = match slot {
                        SyntaxSlot::WorktreeDiff => this
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::HistoryDiff => this
                            .history_diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::StashDiff => this
                            .stash_preview
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::BrowseDiff => this
                            .browse
                            .diff
                            .as_ref()
                            .map(|diff| (Arc::as_ptr(diff) as usize, diff.lines.len())),
                        SyntaxSlot::Blame => this
                            .blame
                            .view
                            .as_ref()
                            .map(|view| (Arc::as_ptr(view) as usize, view.lines.len())),
                        SyntaxSlot::BrowseContent => this
                            .browse
                            .content
                            .as_ref()
                            .map(|content| (Arc::as_ptr(content) as usize, content.lines.len())),
                    };
                    if current != Some((anchor, anchor_len)) {
                        return;
                    }
                    match slot {
                        SyntaxSlot::WorktreeDiff => this.diff_syntax = spans,
                        SyntaxSlot::HistoryDiff => this.history_diff_syntax = spans,
                        SyntaxSlot::StashDiff => this.stash_preview.diff_syntax = spans,
                        SyntaxSlot::BrowseDiff => this.browse.diff_syntax = spans,
                        SyntaxSlot::Blame => this.blame.syntax = spans,
                        SyntaxSlot::BrowseContent => this.browse.content_syntax = spans,
                    }
                });
            }
            UiEvent::ConflictSyntaxHighlighted {
                tab_id,
                path,
                pane,
                seq,
                spans,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if !this.conflict_workbench.files.contains_key(&path) {
                        return;
                    }
                    let Some(entry) = this.conflict_workbench.syntax.get_mut(&path) else {
                        return;
                    };
                    match pane {
                        ConflictSyntaxPane::Ours => entry.ours = spans,
                        ConflictSyntaxPane::Theirs => entry.theirs = spans,
                        // 草稿已再次变更（更新的调度在飞）时丢弃旧结果
                        ConflictSyntaxPane::Draft => {
                            if entry.draft_seq == seq {
                                entry.draft = spans;
                            }
                        }
                    }
                });
            }
            UiEvent::BrowseTargetResolved {
                tab_id,
                target,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.browse.target = Some(target);
                        match this.browse.list_mode {
                            BrowseListMode::Tree => {
                                // 浏览模式自动加载根目录树。
                                this.load_browse_tree(PathBuf::new());
                            }
                            BrowseListMode::Compare => {
                                // 比较模式只加载差异文件列表，避免遍历完整文件树。
                                this.load_browse_compare_files();
                            }
                        }
                    }
                });
            }
            UiEvent::BrowseTreeLoaded {
                tab_id,
                dir_path,
                entries,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id {
                        this.browse.loading_tree = false;
                        this.browse.entries_by_dir.insert(dir_path, entries);
                    }
                });
            }
            UiEvent::BrowseCompareFilesLoaded {
                tab_id,
                target_oid,
                files,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    let target_matches = this
                        .browse
                        .target
                        .as_ref()
                        .is_some_and(|target| target.commit_oid == target_oid);
                    if load_id == this.repository_load_id
                        && target_matches
                        && this.browse.list_mode == BrowseListMode::Compare
                    {
                        this.browse.compare_loading = false;
                        this.browse.compare_files = files;
                        if let Some(first) = this.browse.compare_files.first().cloned() {
                            this.status =
                                format!("已加载 {} 个差异文件", this.browse.compare_files.len());
                            this.select_browse_compare_file(first);
                        } else {
                            this.browse.selected_file = None;
                            this.browse.selected_compare_file = None;
                            this.browse.content = None;
                            this.browse.diff = None;
                            this.browse.diff_headers_expanded = false;
                            this.status = "该分支与当前分支没有差异".to_string();
                        }
                    }
                });
            }
            UiEvent::BrowseFileContentLoaded {
                tab_id,
                path,
                content,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.browse.selected_file.as_deref() == Some(std::path::Path::new(&path))
                        && this.browse.view_mode == BrowseViewMode::Content
                    {
                        this.browse.loading_content = false;
                        this.browse.content_syntax = None;
                        this.browse.content = Some(Arc::new(content));
                        this.status = "文件内容已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::BrowseContent);
                    }
                });
            }
            UiEvent::BrowseFileDiffLoaded {
                tab_id,
                path,
                diff,
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id
                        && this.browse.selected_file.as_deref() == Some(std::path::Path::new(&path))
                        && this.browse.view_mode == BrowseViewMode::Diff
                    {
                        this.browse.loading_diff = false;
                        this.browse.diff_syntax = None;
                        this.browse.diff = Some(Arc::new(diff));
                        this.browse.diff_headers_expanded = false;
                        this.status = "文件差异已加载".to_string();
                        this.schedule_syntax_highlight(SyntaxSlot::BrowseDiff);
                    }
                });
            }
            UiEvent::OperationFailed { tab_id, error } => {
                let toast_message = error.clone();
                // 全局测试失败路径带 tab_id=None：定向复位借用的来源 tab。
                if tab_id.is_none() {
                    self.end_global_test_busy();
                }
                self.apply_status_event(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.remote_branch_operation.refreshing = false;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = "操作失败".to_string();
                    this.last_error = Some(error);
                    // 全文视图过大时自动回退到紧凑差异
                    this.revert_full_file_if_too_large_error();
                });
                self.notify_error(toast_message, cx);
            }
            UiEvent::CredentialRequested {
                tab_id,
                request,
                response_tx,
            } => {
                if tab_id.is_some_and(|tab_id| self.tab(tab_id).is_none()) {
                    if let Ok(mut response_tx) = response_tx.lock()
                        && let Some(response_tx) = response_tx.take()
                    {
                        let _ = response_tx.send(Ok(None));
                    }
                    return;
                }
                self.apply_status_event(tab_id, |this| {
                    this.status = "需要凭据".to_string();
                });
                self.notify_toast(AppToastKind::Info, "远端操作需要凭据，请在右上角填写", cx);
                let pending = PendingCredential {
                    tab_id,
                    request,
                    response_tx,
                };
                // 仅当请求被提升为当前待处理时才准备表单：排队中的请求不能
                // 重置表单——那会用旧请求的参数覆盖用户正在输入的内容。
                if self.enqueue_credential_request(pending) {
                    self.prepare_current_credential_prompt();
                }
            }
            UiEvent::ProxyTestFinished { message } => {
                let toast_message = message.clone();
                self.end_global_test_busy();
                self.operation_blocker = OperationBlocker::None;
                self.operation_blocker_started = None;
                self.status = message;
                self.last_error = None;
                self.notify_success(toast_message, cx);
            }
            UiEvent::WorkflowProgress { tab_id, entry } => {
                self.with_tab_context(tab_id, |this| {
                    this.status = entry.message.clone();
                    this.workflow_state.log.push(entry);
                });
            }
            UiEvent::WorkflowTemplatesLoaded { result } => {
                self.apply_workflow_templates(result);
            }
            UiEvent::WorkflowFinished {
                tab_id,
                message,
                snapshot,
                log,
            } => {
                let toast_message = message.clone();
                let mut full_status_request = None;
                let mut sync_request = None;
                self.with_tab_context(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = message;
                    this.last_error = None;
                    this.workflow_state.log = log;
                    this.repo_path = Some(snapshot.path.clone());
                    this.sync_selected_remote(&snapshot);
                    this.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
                    this.snapshot = Some(snapshot);
                    this.prune_stash_preview();
                    this.sync_conflict_mode_with_snapshot();
                    this.prune_change_selection();
                    this.diff = None;
                    this.diff_headers_expanded = false;
                    this.reset_uniform_scroll("diff-scroll");
                    this.refresh_history();
                    this.scroll_local_branch_to_current();
                    // 工作流可能执行拉取/提交等操作，历史列表存在时后台刷新
                    this.reload_history_after_change();
                    full_status_request = this
                        .repo_path
                        .clone()
                        .map(|path| (tab_id, path, this.repository_load_id));
                    this.loading.status_full = true;
                    sync_request = this.prepare_branch_sync_status_request();
                });
                if let Some((tab_id, path, load_id)) = full_status_request {
                    self.load_full_status_for_tab(tab_id, path, load_id, "变更已补全".to_string());
                }
                if let Some((tab_id, path, remote, load_id, request_id)) = sync_request {
                    self.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
                }
                self.notify_completion(&toast_message, cx);
            }
            UiEvent::OpenRepositoryFolderSelected { path } => {
                if let Some(path) = path {
                    self.open_repo(path);
                } else {
                    self.status = "已取消选择仓库文件夹".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::CloneTargetFolderSelected { path } => {
                if let Some(path) = path {
                    self.clone_path.set_value(path.display().to_string());
                    self.last_error = None;
                } else {
                    self.status = "已取消选择克隆父文件夹".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::ExternalMergeExecutableSelected { path } => {
                self.external_merge_detection = None;
                if let Some(path) = path {
                    self.external_merge_intellij_path
                        .set_value(path.display().to_string());
                    self.external_merge_enabled_form = true;
                    self.status = format!("已选择 IntelliJ IDEA：{}", path.display());
                    self.last_error = None;
                } else {
                    self.status = "已取消选择 IntelliJ IDEA 程序".to_string();
                    self.last_error = None;
                }
            }
            UiEvent::AiWorkflowTemplateGenerated { content } => {
                self.handle_ai_workflow_template_generated(content, cx);
            }
            UiEvent::AiCommitMessageGenerated { message } => {
                self.ai_commit_loading = false;
                // 思考弹窗随完成自动关闭。
                self.ai_thinking_overlay = None;
                // 兜底守卫：空结果不覆盖输入框（避免清掉用户草稿）并显式提示。
                // 正常路径已在生成任务里按空正文报错，这里防御未来回归。
                if message.trim().is_empty() {
                    self.status = "AI 未返回提交信息".into();
                    self.last_error = Some("AI 返回的提交信息为空".into());
                    self.notify_error("AI 返回的提交信息为空", cx);
                } else {
                    // 流式期间已逐段填入输入框，这里用最终结果做一次干净覆盖，
                    // 确保最终的 trim 和换行规范化。
                    self.commit_message.set_value(message);
                    self.status = "AI 已生成提交信息".into();
                    self.last_error = None;
                }
            }
            UiEvent::AiReviewGenerated {
                generation,
                review,
                saved,
            } => {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                if self.ai_review_active_generation == Some(generation) {
                    self.ai_review_loading = false;
                    self.ai_review_cancel = None;
                    self.ai_review_progress = None;
                    // live 缓冲定格为正式结果（同一数据源），清空避免重复展示。
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    self.ai_review_loaded_label = None;
                    self.ai_review = Some(Arc::new(review));
                    self.status = if saved {
                        "AI 评审已生成并保存到评审记录".into()
                    } else {
                        "AI 评审已生成（保存记录失败）".into()
                    };
                    self.last_error = None;
                } else {
                    // 后台分离任务完成：落盘已在任务线程做完，提示可去历史查看。
                    let message = if saved {
                        "后台 AI 评审完成，已保存到评审记录".to_string()
                    } else {
                        "后台 AI 评审完成（保存记录失败，历史弹窗中将看不到本次记录）".to_string()
                    };
                    self.status = message.clone();
                    self.notify_completion(&message, cx);
                }
            }
            UiEvent::AiReviewStepAdded { generation, step } => {
                // 代际守卫：UI 已分离（切目标/取消）的旧任务事件不进面板。
                if self.ai_review_active_generation == Some(generation) {
                    // 保留已完成评审的轨迹直到新评审开始覆盖（generate 时清空）。
                    // 思维链/中间正文步骤落定后，对应 live 区让位于正式时间线行。
                    match &step {
                        AiReviewStep::Reasoning { .. } => self.ai_review_live_reasoning.clear(),
                        AiReviewStep::Message { .. } => self.ai_review_live_content.clear(),
                        AiReviewStep::ToolCall { .. } => {}
                    }
                    self.ai_review_steps.push(step);
                }
            }
            UiEvent::AiReviewProgress {
                generation,
                message,
            } => {
                if self.ai_review_active_generation == Some(generation) {
                    // 新一轮开始：清上一轮的 live 思维链（正文 live 只属于末轮，
                    // 也在换轮时清空避免中轮正文残留）。
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    // 进度同时落状态栏，与其它 AI 任务一致。
                    self.status = message.clone();
                    self.ai_review_progress = Some(message);
                }
            }
            UiEvent::AiReviewDelta {
                generation,
                content_delta,
                reasoning_delta,
            } => {
                if self.ai_review_active_generation == Some(generation) {
                    if let Some(delta) = content_delta {
                        self.ai_review_live_content.push_str(&delta);
                    }
                    if let Some(delta) = reasoning_delta {
                        self.ai_review_live_reasoning.push_str(&delta);
                    }
                }
            }
            UiEvent::AiReviewFailed { generation, error } => {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                if self.ai_review_active_generation == Some(generation) {
                    self.ai_review_loading = false;
                    self.ai_review_cancel = None;
                    self.ai_review_progress = None;
                    self.ai_review_live_reasoning.clear();
                    self.ai_review_live_content.clear();
                    // 失败保留已产生的轨迹（可检查模型做了什么）。
                    self.status = "AI 请求失败".into();
                    self.last_error = Some(error.clone());
                    self.notify_error(format!("AI 请求失败：{error}"), cx);
                } else {
                    // 后台分离任务失败：不打断用户，仅状态栏可见。
                    self.status = format!("后台 AI 评审失败：{error}");
                }
            }
            UiEvent::AiReviewCancelled => {
                // 取消时 UI 已即时复位，这里只做在途计数归位。
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
            }
            UiEvent::AiReviewHistoryLoaded { records } => {
                if let Some(state) = self.ai_review_history.as_mut() {
                    state.loading = false;
                    state.records = records;
                }
            }
            UiEvent::AiReviewHistoryLoadFailed { error } => {
                if let Some(state) = self.ai_review_history.as_mut() {
                    state.loading = false;
                    state.error = Some(error);
                }
            }
            UiEvent::AiConflictMergeProgress {
                path,
                segment,
                total,
            } => {
                // 分段模式下的进度提示：仅当前选中的冲突文件更新状态栏，
                // 避免生成期间切换文件后被旧任务的进度占据。整文件模式
                // 只有一段，不发送该事件。
                if self.conflict_workbench.selected_path.as_deref() == Some(path.as_str()) {
                    self.status = format!("正在生成 AI 合并建议（第 {segment}/{total} 段）");
                }
            }
            UiEvent::AiConflictMergeGenerated { path, draft } => {
                self.ai_conflict_loading = false;
                // 思考弹窗随完成自动关闭。
                self.ai_thinking_overlay = None;
                match self.conflict_workbench.files.get_mut(&path) {
                    Some(view) if view.kind == ConflictFileKind::Text => {
                        // Merged 写入：被覆盖块标记「已合并」（绿色），不再
                        // 计入未处理，也不触发手工修改横幅。
                        view.set_merged_draft(draft);
                        self.sync_conflict_editor_from_state();
                        // 草稿整体替换后重算结果区语法高亮
                        self.schedule_conflict_syntax_for_selected(&[ConflictSyntaxPane::Draft]);
                        self.status = "已填入 AI 合并建议，请检查后应用".into();
                        self.last_error = None;
                        self.notify_success("已填入 AI 合并建议，请检查后应用", cx);
                    }
                    // 生成期间冲突被解决、文件被移出列表或切换了标签页：
                    // 结果无处安放，仅状态栏说明，不弹错误。
                    _ => {
                        self.status = "AI 合并建议已返回，但该文件已不在冲突列表".into();
                    }
                }
            }
            UiEvent::AiRequestFailed { error } => {
                self.ai_commit_loading = false;
                self.ai_conflict_loading = false;
                // 思考弹窗随失败自动关闭（无论哪路业务失败）。
                self.ai_thinking_overlay = None;
                // 工作流模板编辑器的 AI 生成失败：错误写进编辑器内错误条
                // （弹窗仍开着，用户可直接改需求重试），并复位其 loading。
                self.handle_ai_workflow_template_failed(&error);
                // 评审失败走带代际的 AiReviewFailed（旧任务的失败不能
                // 误复位当前附着任务的状态）。
                // 测试连接失败时也要解锁借用的 busy，否则按钮永久禁用。
                self.end_global_test_busy();
                // 失败提示走右下角 toast + 状态栏双通道，仅靠状态栏小字极易被错过。
                self.status = "AI 请求失败".into();
                self.last_error = Some(error.clone());
                self.notify_error(format!("AI 请求失败：{error}"), cx);
            }
            UiEvent::AiThinkingDelta {
                content_delta,
                reasoning_delta,
            } => {
                // 思考弹窗已关闭（后台运行）时丢弃增量。
                let Some(overlay) = self.ai_thinking_overlay.as_mut() else {
                    return;
                };
                overlay.reasoning.push_str(&reasoning_delta);
                if let Some(delta) = content_delta {
                    overlay.content.push_str(&delta);
                }
                // 钉底跟随不在这里做：事件时机的 max_offset 是上一帧的，
                // 会恒落后一帧；改由弹窗内容末位 canvas 的 prepaint 按内容
                // 长度键门控执行（见 render_ai_thinking_overlay）。
            }
            UiEvent::AiConnectionTested { message } => {
                self.end_global_test_busy();
                self.status = message.clone();
                self.last_error = None;
                self.notify_completion(&message, cx);
            }
            // ── 更新事件 ──
            UiEvent::UpdateCheckFinished { manifest, asset } => {
                self.update_checking = false;
                self.available_update = Some(manifest.clone());
                self.status = format!("发现新版本 v{}", manifest.version);
                self.last_error = None;
                self.active_dialog = Some(DialogState::NewVersionAvailable {
                    version: manifest.version.clone(),
                    notes: manifest.notes.clone(),
                    published_at: manifest.published_at.clone(),
                    size: asset.size,
                });
            }
            UiEvent::UpdateCheckFailed { error, manual } => {
                self.update_checking = false;
                if !error.is_empty() {
                    self.update_error = Some(error.clone());
                    self.status = error.clone();
                    // 手动检查弹气泡反馈（成功发现最新=成功气泡、真失败=
                    // 错误气泡）；自动检查保持安静，仅状态栏与设置页错误条。
                    if let Some((kind, message)) = update_check_toast(&error, manual) {
                        self.notify_toast(kind, message, cx);
                    }
                }
            }
            UiEvent::UpdateDownloadProgress { downloaded, total } => {
                let mb_down = downloaded as f64 / 1_048_576.0;
                let mb_total = total as f64 / 1_048_576.0;
                self.update_download_progress =
                    Some(format!("{:.1} MB / {:.1} MB", mb_down, mb_total));
            }
            UiEvent::UpdateReadyToInstall {
                staging_dir,
                manifest,
            } => {
                self.update_downloading = false;
                self.update_download_progress = None;
                self.status = format!("更新 v{} 已准备就绪", manifest.version);
                self.last_error = None;
                self.active_dialog = Some(DialogState::ConfirmInstallUpdate {
                    version: manifest.version.clone(),
                });
                // 保存 staging_dir 以便安装
                self.available_update = Some(manifest);
                self.staging_dir_for_install = Some(staging_dir);
            }
            UiEvent::UpdateInstallFailed { error } => {
                self.update_downloading = false;
                self.update_download_progress = None;
                self.update_error = Some(error.clone());
                self.status = "更新失败".into();
                self.notify_toast(AppToastKind::Error, format!("更新失败：{error}"), cx);
            }
            UiEvent::BackgroundTaskPanicked { message } => {
                // 无法定位具体是哪个任务 panic：保守复位所有 tab 的 busy/加载
                // 标志与仓库加载槽位（序号守卫会丢弃迟到的旧结果，复位是安全的）。
                for tab in self.tabs.iter_mut() {
                    tab.busy = false;
                    tab.loading = RepositoryLoading::default();
                    tab.history_loading = HistoryLoading::default();
                }
                self.active_repository_loads = 0;
                // 全局一次性标志同样复位：AI 生成（commit/冲突合并）、思考
                // 弹窗、全局 busy 槽位、更新下载与工作流编辑器的 AI 生成
                // 标志——任何一项卡死都会永久阻断对应入口（按钮禁用 /
                // 「已有操作正在运行」）。
                self.ai_commit_loading = false;
                self.ai_conflict_loading = false;
                self.ai_thinking_overlay = None;
                self.global_busy_tab = None;
                self.update_downloading = false;
                self.reset_ai_loading_after_panic();
                self.status = "后台任务异常".into();
                self.notify_toast(
                    AppToastKind::Error,
                    format!("后台任务异常退出，已复位操作状态：{message}"),
                    cx,
                );
            }
            UiEvent::AmendPrefillLoaded { tab_id, message } => {
                self.with_tab_context(tab_id, |this| {
                    // 仅当仍处于修补模式且输入框仍为空时填入：期间用户可能
                    // 已关闭开关或手动输入了内容。
                    if this.amend_mode
                        && this.commit_message.value.trim().is_empty()
                        && let Some(message) = message.filter(|m| !m.trim().is_empty())
                    {
                        this.amend_prefill = Some(message.clone());
                        this.commit_message.set_value(message);
                        this.commit_message.caret = 0;
                    }
                });
            }
        }
        cx.notify();
    }

    fn merge_metadata_snapshot(&mut self, snapshot: RepositorySnapshot) {
        let mut merged = self.snapshot.take().unwrap_or_default();
        let was_merge_in_progress = merged.merge_in_progress;
        let merge_in_progress = snapshot.merge_in_progress;
        let merge_message = snapshot.merge_message.clone();
        merged.path = snapshot.path;
        merged.head = snapshot.head;
        merged.branches = snapshot.branches;
        merged.remotes = snapshot.remotes;
        merged.tags = snapshot.tags;
        merged.stashes = snapshot.stashes;
        merged.conflicts = snapshot.conflicts;
        merged.merge_in_progress = merge_in_progress;
        merged.merge_message = snapshot.merge_message;
        merged.rebase_in_progress = snapshot.rebase_in_progress;
        self.repo_path = Some(merged.path.clone());
        self.sync_selected_remote(&merged);
        self.change_indexes = ChangeListIndexes::rebuild(&merged.changes);
        self.snapshot = Some(merged);
        self.sync_merge_message_transition(was_merge_in_progress, merge_in_progress, merge_message);
        self.sync_conflict_mode_with_snapshot();
    }

    fn replace_changes(&mut self, changes: Vec<khaslana::WorktreeChange>) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.changes = changes;
            self.change_indexes = ChangeListIndexes::rebuild(&snapshot.changes);
        } else {
            self.change_indexes = ChangeListIndexes::default();
        }
        self.prune_change_selection();
    }

    fn sync_conflict_mode_with_snapshot(&mut self) {
        let conflict_paths = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.conflicts.clone())
            .unwrap_or_default();
        let auto_open_conflict_mode = !self.merge_in_progress();
        let tab = self.active_tab_state_mut();
        sync_conflict_state_from_paths(
            &mut tab.main_mode,
            &mut tab.conflict_workbench,
            &conflict_paths,
            auto_open_conflict_mode,
        );

        if conflict_paths.is_empty() {
            self.conflict_editor.clear();
            return;
        }
        self.ensure_conflict_views_loaded();
        self.sync_conflict_editor_from_state();
        self.maybe_auto_open_external_merge_for_selected_conflict();
    }

    fn ensure_conflict_views_loaded(&mut self) {
        let paths = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.conflicts.clone())
            .unwrap_or_default();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        for path in paths {
            if self.conflict_workbench.files.contains_key(&path) {
                continue;
            }
            match Repository::open(&repo_path)
                .map_err(khaslana::GitError::from)
                .and_then(|repo| service.conflict_file_view(&repo, Path::new(&path)))
            {
                Ok(view) => {
                    self.conflict_workbench.files.insert(path, view);
                }
                Err(err) => {
                    self.last_error = Some(err.to_string());
                }
            }
        }
    }

    fn sync_conflict_editor_from_state(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.conflict_editor.clear();
            return;
        };
        let Some((kind, draft)) = self
            .conflict_workbench
            .files
            .get(&path)
            .map(|view| (view.kind, view.draft.clone()))
        else {
            self.conflict_editor.clear();
            return;
        };
        if kind != ConflictFileKind::Text {
            self.conflict_editor.clear();
            return;
        }
        if !conflict_editor_should_store_draft(kind) {
            self.conflict_editor.clear();
            self.scroll_conflict_panes_to_selected_block(
                &draft,
                self.selected_conflict_block_start(),
            );
            return;
        }
        if self.conflict_editor.value != draft {
            self.conflict_editor.set_value(draft);
        }
        self.highlight_selected_conflict_block();
    }

    fn selected_conflict_block_start(&self) -> usize {
        let Some(path) = self.conflict_workbench.selected_path.as_ref() else {
            return 0;
        };
        self.conflict_workbench
            .files
            .get(path)
            .and_then(|view| {
                view.blocks
                    .get(
                        self.conflict_workbench
                            .selected_block
                            .min(view.blocks.len().saturating_sub(1)),
                    )
                    .map(|block| block.start)
            })
            .unwrap_or(0)
    }

    fn highlight_selected_conflict_block(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        let Some(block) = view
            .blocks
            .get(
                self.conflict_workbench
                    .selected_block
                    .min(view.blocks.len().saturating_sub(1)),
            )
            .cloned()
        else {
            return;
        };
        let draft = view.draft.clone();
        if conflict_editor_should_store_draft(view.kind) {
            self.conflict_editor.move_caret_to(block.start, false);
            self.conflict_editor.move_caret_to(block.end, true);
        }
        self.scroll_conflict_panes_to_selected_block(&draft, block.start);
    }

    fn scroll_conflict_panes_to_selected_block(&self, text: &str, offset: usize) {
        let line_index = line_index_for_byte_offset(text, offset);
        for handle_id in conflict_workbench_scroll_handle_ids() {
            self.uniform_scroll_handle(handle_id)
                .scroll_to_item_strict_with_offset(line_index, ScrollStrategy::Top, 4);
        }
    }

    fn sync_conflict_editor_into_state(&mut self) {
        if !conflict_result_pane_uses_editor() {
            return;
        }
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let new_value = self.conflict_editor.value.clone();
        let Some(view) = self.conflict_workbench.files.get_mut(&path) else {
            return;
        };
        if view.kind == ConflictFileKind::Text && view.draft != new_value {
            let max_index = view.blocks.len().saturating_sub(1);
            view.set_draft(new_value);
            self.conflict_workbench.selected_block =
                self.conflict_workbench.selected_block.min(max_index);
        }
    }

    fn select_conflict_file(&mut self, path: String) {
        self.sync_conflict_editor_into_state();
        self.conflict_workbench.selected_path = Some(path.clone());
        self.conflict_workbench.selected_block = 0;
        self.conflict_workbench.show_base = false;
        self.conflict_workbench.clear_pending_resolve();
        // 换文件后三栏 offset 全变，清掉同步滚动的上帧记录避免误判源栏。
        self.conflict_pane_scroll_sync.borrow_mut().take();
        self.ensure_conflict_views_loaded();
        if self.conflict_workbench.files.contains_key(&path) {
            self.sync_conflict_editor_from_state();
            self.maybe_auto_open_external_merge_for_selected_conflict();
            // 切换冲突文件后为新的选中文件补算三栏语法高亮
            let panes = [
                ConflictSyntaxPane::Ours,
                ConflictSyntaxPane::Theirs,
                ConflictSyntaxPane::Draft,
            ];
            self.schedule_conflict_syntax_for_selected(&panes);
        }
    }

    fn select_conflict_block(&mut self, index: usize) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        if view.blocks.is_empty() {
            self.conflict_workbench.selected_block = 0;
        } else {
            self.conflict_workbench.selected_block = index.min(view.blocks.len() - 1);
        }
        self.conflict_workbench.clear_pending_resolve();
        self.sync_conflict_editor_from_state();
    }

    fn step_conflict_block(&mut self, delta: isize) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            return;
        };
        if view.blocks.is_empty() {
            return;
        }
        let current = self.conflict_workbench.selected_block as isize;
        let target = (current + delta).clamp(0, view.blocks.len() as isize - 1) as usize;
        self.select_conflict_block(target);
    }

    fn apply_selected_conflict_resolution(&mut self, resolution: ConflictBlockResolution) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let selected_block = self.conflict_workbench.selected_block;
        if let Some(view) = self.conflict_workbench.files.get_mut(&path) {
            view.apply_block_resolution(selected_block, resolution);
        }
        self.sync_conflict_editor_from_state();
        // 草稿文本已变，重算结果区语法高亮（seq 守卫丢弃乱序旧结果）
        self.schedule_conflict_syntax_for_selected(&[ConflictSyntaxPane::Draft]);
    }

    fn ignore_selected_conflict_block(&mut self) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let selected_block = self.conflict_workbench.selected_block;
        if let Some(view) = self.conflict_workbench.files.get_mut(&path) {
            view.ignore_block(selected_block);
        }
        self.conflict_workbench.clear_pending_resolve();
        self.sync_conflict_editor_from_state();
    }

    fn apply_selected_conflict_draft(&mut self, resolve: bool) {
        self.sync_conflict_editor_into_state();
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        let Some(view) = self.conflict_workbench.files.get_mut(&path) else {
            self.last_error = Some("冲突文件详情尚未加载".into());
            return;
        };
        let unresolved_count = view.unresolved_block_count();
        let draft = view.draft.clone();
        if !resolve {
            view.mark_applied();
        } else if self
            .conflict_workbench
            .request_resolve_confirmation(path.clone(), unresolved_count)
        {
            self.active_dialog = Some(DialogState::ConfirmConflictResolve);
            return;
        }
        self.apply_conflict_draft_operation(path, draft, resolve);
    }

    fn confirm_pending_conflict_resolve(&mut self) {
        self.sync_conflict_editor_into_state();
        let Some(pending) = self.conflict_workbench.pending_resolve.clone() else {
            self.active_dialog = None;
            return;
        };
        let Some(draft) = self
            .conflict_workbench
            .files
            .get(&pending.path)
            .map(|view| view.draft.clone())
        else {
            self.conflict_workbench.clear_pending_resolve();
            self.active_dialog = None;
            self.last_error = Some("冲突文件详情尚未加载".into());
            return;
        };
        self.conflict_workbench.clear_pending_resolve();
        self.active_dialog = None;
        self.apply_conflict_draft_operation(pending.path, draft, true);
    }

    fn cancel_pending_conflict_resolve(&mut self) {
        self.conflict_workbench.clear_pending_resolve();
        self.active_dialog = None;
    }

    fn apply_conflict_draft_operation(&mut self, path: String, draft: String, resolve: bool) {
        let path_for_op = path.clone();
        let label = if resolve {
            "冲突结果已应用并标记解决"
        } else {
            "冲突草稿已应用到工作区"
        };
        self.with_repo(label, move |service, repo| {
            if resolve {
                service.apply_conflict_draft_and_resolve(repo, Path::new(&path_for_op), &draft)
            } else {
                service.apply_conflict_draft(repo, Path::new(&path_for_op), &draft)
            }
        });
    }

    fn prune_change_selection(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.change_selection.clear();
            return;
        };
        let staged = snapshot
            .changes
            .iter()
            .filter(|change| change.staged.is_some())
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>();
        let unstaged = snapshot
            .changes
            .iter()
            .filter(|change| change.unstaged.is_some())
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>();
        self.change_selection
            .staged
            .retain(|path| staged.contains(path));
        self.change_selection
            .unstaged
            .retain(|path| unstaged.contains(path));
        if self
            .change_selection
            .staged_anchor
            .as_ref()
            .is_some_and(|path| !staged.contains(path))
        {
            self.change_selection.staged_anchor = None;
        }
        if self
            .change_selection
            .unstaged_anchor
            .as_ref()
            .is_some_and(|path| !unstaged.contains(path))
        {
            self.change_selection.unstaged_anchor = None;
        }
    }

    fn submit_focused_field(&mut self, field: FieldId) {
        if matches!(field, FieldId::CommitMessage) {
            self.commit();
        } else if matches!(field, FieldId::ConflictEditor) {
            self.apply_selected_conflict_draft(false);
        } else if matches!(field, FieldId::CloneUrl | FieldId::ClonePath) {
            if self.active_dialog == Some(DialogState::CloneRepo) {
                self.clone_repo();
            }
        } else if matches!(field, FieldId::BranchName) {
            if self.active_dialog == Some(DialogState::CreateBranch) {
                self.create_branch();
            }
        } else if matches!(field, FieldId::BranchRename) {
            if let Some(DialogState::RenameBranch { branch }) = self.active_dialog.clone() {
                self.rename_branch(branch);
            }
        } else if matches!(field, FieldId::StashMessage) {
            if self.active_dialog == Some(DialogState::StashForm) {
                self.save_stash();
            }
        } else if matches!(field, FieldId::TagName) {
            if matches!(self.active_dialog, Some(DialogState::TagForm { .. })) {
                self.create_tag();
            }
        } else if matches!(field, FieldId::RemoteName | FieldId::RemoteUrl) {
            if let Some(DialogState::RemoteForm { editing }) = self.active_dialog.clone() {
                self.save_remote(editing);
            }
        } else if matches!(field, FieldId::RemoteBranchName) {
            if let Some(DialogState::RemoteBranchOperation { kind }) = self.active_dialog.clone() {
                self.confirm_remote_branch_operation(kind);
            }
        } else if matches!(field, FieldId::RemoteBranchSearch) {
            self.remote_branch_operation.branch_dropdown_open = false;
        } else if matches!(
            field,
            FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
        ) {
            if self.settings_center == Some(SettingsCategory::Proxy) {
                self.save_network_proxy_settings();
            }
        } else if matches!(field, FieldId::ExternalMergeIntellijPath) {
            if self.settings_center == Some(SettingsCategory::ExternalMerge) {
                self.save_external_merge_settings_from_form_and_resume();
            }
        } else if matches!(
            field,
            FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
        ) {
            if self.settings_center == Some(SettingsCategory::Ai) {
                self.save_ai_provider_settings_from_form();
            }
        } else if matches!(field, FieldId::CredentialTestUrl) {
            if matches!(self.active_dialog, Some(DialogState::TestCredential { .. })) {
                self.confirm_test_credential();
            }
        } else if matches!(
            field,
            FieldId::CredentialSecret
                | FieldId::CredentialPassphrase
                | FieldId::CredentialUsername
                | FieldId::CredentialKeyPath
                | FieldId::CredentialRemoteUrl
                | FieldId::CredentialDisplayName
        ) {
            if matches!(self.active_dialog, Some(DialogState::CredentialForm { .. })) {
                self.save_credential_form();
            } else {
                self.use_credentials();
            }
        }
    }

    fn notify_text_field_changed(&mut self, field: FieldId) {
        if matches!(field, FieldId::WorkflowInput(_)) {
            self.workflow_input_changed();
        }
        // 编辑器文本框值变化即同步回纯数据层（预览/保存校验都读数据层）
        if let FieldId::WorkflowEditor(editor_id) = field {
            self.workflow_editor_field_changed(editor_id);
        }
    }

    fn focused_text_field(&self, window: &Window, cx: &App) -> Option<FieldId> {
        let field = self.focused_field(window, cx)?;
        if self.active_operation_blocker_message().is_some()
            && !self.operation_blocker_allows_text_field(field)
        {
            return None;
        }
        Some(field)
    }

    fn operation_blocker_allows_text_field(&self, field: FieldId) -> bool {
        self.pending_credential.is_some()
            && matches!(
                field,
                FieldId::CredentialUsername
                    | FieldId::CredentialSecret
                    | FieldId::CredentialKeyPath
                    | FieldId::CredentialPassphrase
            )
    }

    fn text_backspace(&mut self, _: &TextBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_backward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn text_delete(&mut self, _: &TextDelete, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_forward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn text_left(&mut self, _: &TextLeft, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(false);
            cx.notify();
        }
    }

    fn text_right(&mut self, _: &TextRight, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(false);
            cx.notify();
        }
    }

    fn text_up(&mut self, _: &TextUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_vertical(-1, false);
                cx.notify();
            }
        }
    }

    fn text_down(&mut self, _: &TextDown, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_vertical(1, false);
                cx.notify();
            }
        }
    }

    fn text_select_left(
        &mut self,
        _: &TextSelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(true);
            cx.notify();
        }
    }

    fn text_select_right(
        &mut self,
        _: &TextSelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(true);
            cx.notify();
        }
    }

    fn text_select_up(&mut self, _: &TextSelectUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(-1, true);
            cx.notify();
        }
    }

    fn text_select_down(
        &mut self,
        _: &TextSelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(1, true);
            cx.notify();
        }
    }

    fn text_select_all(&mut self, _: &TextSelectAll, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).select_all();
            cx.notify();
        }
    }

    fn text_home(&mut self, _: &TextHome, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_start(false);
            } else {
                self.field_mut(field).move_caret_to(0, false);
            }
            cx.notify();
        }
    }

    fn text_end(&mut self, _: &TextEnd, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_end(false);
            } else {
                let end = self.field(field).value.len();
                self.field_mut(field).move_caret_to(end, false);
            }
            cx.notify();
        }
    }

    fn text_paste(&mut self, _: &TextPaste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.field_mut(field).replace_text_in_utf16_range_with_mode(
                None,
                &text,
                Self::is_multiline_field(field),
            );
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn text_copy(&mut self, _: &TextCopy, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn text_cut(&mut self, _: &TextCut, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.field_mut(field).delete_selection();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn text_submit(&mut self, _: &TextSubmit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if field == FieldId::CommitMessage {
                self.commit();
                cx.notify();
            } else if field == FieldId::ConflictEditor {
                self.apply_selected_conflict_draft(false);
                cx.notify();
            } else if matches!(
                field,
                FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
            ) && self.settings_center == Some(SettingsCategory::Proxy)
            {
                self.save_network_proxy_settings();
                cx.notify();
            } else if matches!(
                field,
                FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
            ) && self.settings_center == Some(SettingsCategory::Ai)
            {
                self.save_ai_provider_settings_from_form();
                cx.notify();
            } else if field == FieldId::ExternalMergeIntellijPath
                && self.settings_center == Some(SettingsCategory::ExternalMerge)
            {
                self.save_external_merge_settings_from_form();
                cx.notify();
            }
        }
    }

    fn focused_field(&self, window: &Window, _cx: &App) -> Option<FieldId> {
        DEDICATED_FIELDS
            .iter()
            .find_map(|(id, access)| access(self).focus.is_focused(window).then_some(*id))
            .or_else(|| self.focused_workflow_input(window))
            .or_else(|| self.workflow_editor_focused_field(window))
    }

    fn field(&self, id: FieldId) -> &TextFieldState {
        match id {
            FieldId::WorkflowInput(index) => self.workflow_input_field(index),
            // 编辑器字段经独立寻址（渲染前 ensure 已初始化；弹窗关闭瞬间
            // 的在途渲染回落到静态字段兜底）。
            FieldId::WorkflowEditor(editor_id) => self
                .workflow_editor_field_ref(editor_id)
                .unwrap_or(&self.branch_name),
            // 经 DEDICATED_FIELDS 单一注册表查找：与 focused_field 共用一份
            // 清单，漏注册会在此处 panic（首次渲染即暴露）而非静默丢输入。
            _ => DEDICATED_FIELDS
                .iter()
                .find_map(|(field_id, access)| (*field_id == id).then(|| access(self)))
                .expect("FieldId 未注册到 DEDICATED_FIELDS"),
        }
    }

    fn field_mut(&mut self, id: FieldId) -> &mut TextFieldState {
        match id {
            FieldId::CloneUrl => &mut self.clone_url,
            FieldId::ClonePath => &mut self.clone_path,
            FieldId::BranchName => &mut self.branch_name,
            FieldId::BranchRename => &mut self.branch_rename,
            FieldId::RemoteName => &mut self.remote_name,
            FieldId::RemoteUrl => &mut self.remote_url,
            FieldId::CommitMessage => &mut self.commit_message,
            FieldId::StashMessage => &mut self.stash_message,
            FieldId::TagName => &mut self.tag_name,
            FieldId::TagMessage => &mut self.tag_message,
            FieldId::CredentialUsername => &mut self.credential_username,
            FieldId::CredentialSecret => &mut self.credential_secret,
            FieldId::CredentialKeyPath => &mut self.credential_key_path,
            FieldId::CredentialPassphrase => &mut self.credential_passphrase,
            FieldId::CredentialRemoteUrl => &mut self.credential_remote_url,
            FieldId::CredentialTestUrl => &mut self.credential_test_url,
            FieldId::CredentialDisplayName => &mut self.credential_display_name,
            FieldId::ConflictEditor => &mut self.conflict_editor,
            FieldId::RemoteBranchName => &mut self.remote_branch_name,
            FieldId::RemoteBranchSearch => &mut self.remote_branch_search,
            FieldId::RepoSwitcherSearch => &mut self.repo_switcher_search,
            FieldId::CommitGraphSearch => &mut self.commit_graph_search,
            FieldId::CommitGraphBranchSearch => &mut self.commit_graph_branch_search,
            FieldId::SidebarLocalBranchSearch => &mut self.sidebar_local_branch_search,
            FieldId::SidebarRemoteBranchSearch => &mut self.sidebar_remote_branch_search,
            FieldId::ProxyHttpUrl => &mut self.proxy_http_url,
            FieldId::ProxyHttpsUrl => &mut self.proxy_https_url,
            FieldId::ProxySocks5Url => &mut self.proxy_socks5_url,
            FieldId::AiBaseUrl => &mut self.ai_base_url,
            FieldId::AiApiKey => &mut self.ai_api_key,
            FieldId::AiModel => &mut self.ai_model,
            FieldId::ExternalMergeIntellijPath => &mut self.external_merge_intellij_path,
            FieldId::WorkflowInput(index) => self.workflow_input_field_mut(index),
            // 编辑器字段惰性初始化需要 Context（focus_handle）；
            // 编辑器未打开而字段仍被寻址（弹窗关闭瞬间的在途事件）时
            // 退回一个无关紧要的静态字段，保证返回值满足借用契约。
            FieldId::WorkflowEditor(editor_id) => {
                return workflow_editor_field_or_fallback(self, editor_id);
            }
        }
    }

    fn browse_open(&mut self) {
        self.status = "正在选择仓库文件夹".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new().pick_folder();
            send_ui_event(&tx, UiEvent::OpenRepositoryFolderSelected { path });
        });
    }

    fn browse_clone_target(&mut self) {
        self.status = "正在选择克隆父文件夹".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new().pick_folder();
            send_ui_event(&tx, UiEvent::CloneTargetFolderSelected { path });
        });
    }

    fn open_clone_dialog(&mut self, window: &mut Window) {
        self.close_popups();
        self.clone_url.clear();
        self.clone_path.clear();
        self.clone_recursive_submodules = default_clone_recursive_submodules();
        self.active_dialog = Some(DialogState::CloneRepo);
        self.last_error = None;
        window.focus(&self.clone_url.focus);
    }

    pub(crate) fn open_create_branch_dialog(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.branch_name.clear();
        self.create_branch_checkout = true;
        self.active_dialog = Some(DialogState::CreateBranch);
        self.last_error = None;
    }

    pub(crate) fn open_rename_branch_dialog(&mut self, branch: String) {
        self.close_popups();
        self.branch_rename.set_value(branch.clone());
        self.active_dialog = Some(DialogState::RenameBranch { branch });
        self.last_error = None;
    }

    pub(crate) fn close_popups(&mut self) {
        self.active_dialog = None;
        self.ai_review_history = None;
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.credential_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.workflow_template_context_menu = None;
        self.encoding_menu_target = None;
        self.encoding_menu_closed_by_capture = None;
        self.commit_graph_branch_menu_closed_by_capture = false;
        self.active_tab_state_mut().commit_graph.branch_menu_open = false;
        self.commit_graph_branch_search.clear();
        self.context_navigator_overlay_open = false;
        self.close_repo_switcher();
        // 工作流编辑器的下拉（类型/守卫）在弹窗关闭时一并收起，防陈旧态。
        self.workflow_editor_close_menus();
    }

    /// 切换仓库切换下拉的展开/收起；展开时菜单固定在触发器按钮正下方（按记录的锚点定位）。
    pub(crate) fn toggle_repo_switcher(&mut self, window: &Window) {
        if self.repo_switcher_menu.is_some() {
            self.close_repo_switcher();
            return;
        }
        // 仓库切换与窄窗导航覆盖层互斥。
        self.context_navigator_overlay_open = false;
        // 展开时同步加载最近仓库列表（SQLite 本地查询 < 1ms），渲染时纯读缓存。
        self.repo_switcher_recent = self.storage.load_recent_repos().unwrap_or_default();
        let viewport_size = window.viewport_size();
        // 锚点由触发器 paint 时记录；首帧尚未记录时回退到视口左上角。
        let (x, y) = self
            .repo_switcher_anchor
            .map(|anchor| {
                repo_switcher_menu_origin(
                    &anchor,
                    f32::from(viewport_size.width),
                    f32::from(viewport_size.height),
                )
            })
            .unwrap_or((MENU_VIEWPORT_MARGIN, MENU_VIEWPORT_MARGIN));
        self.repo_switcher_menu = Some(RepoSwitcherMenu { x, y });
        // 搜索默认收起为「搜索仓库」按钮；清掉上一次的搜索词。
        self.repo_switcher_search_open = false;
        self.repo_switcher_search.clear();
    }

    pub(crate) fn close_repo_switcher(&mut self) {
        self.repo_switcher_menu = None;
        self.repo_switcher_search_open = false;
        self.repo_switcher_search.clear();
    }

    /// 是否有任一弹出菜单（仓库切换下拉、各类右键菜单、编码菜单）或窄窗导航覆盖层打开。
    /// 弹层没有全屏遮罩，期间分栏分割线等底层交互应暂停，避免抢走弹层边缘的点击。
    fn any_popup_menu_open(&self) -> bool {
        self.context_navigator_overlay_open
            || self.repo_switcher_menu.is_some()
            || self.branch_context_menu.is_some()
            || self.remote_context_menu.is_some()
            || self.change_context_menu.is_some()
            || self.file_path_context_menu.is_some()
            || self.credential_context_menu.is_some()
            || self.tag_context_menu.is_some()
            || self.stash_context_menu.is_some()
            || self.commit_graph.branch_menu_open
            || self.commit_context_menu.is_some()
            || self.workflow_template_context_menu.is_some()
            || self.encoding_menu_target.is_some()
            || self.workflow_editor_menu_open()
    }

    pub(crate) fn toggle_sidebar_section(&mut self, section: SidebarSection) {
        self.close_popups();
        self.sidebar_sections.toggle(section);
    }

    pub(crate) fn close_dialog(&mut self) {
        if self.active_dialog == Some(DialogState::ConfirmWindowClose) {
            self.cancel_window_close();
            return;
        }
        // 如果没有 active_dialog 但设置中心打开，关闭设置中心。
        if self.active_dialog.is_none() && self.settings_center.is_some() {
            self.close_settings_center();
            return;
        }
        let closing_submodule_manager = self.active_dialog == Some(DialogState::SubmoduleManager);
        self.active_dialog = None;
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        self.credential_context_menu = None;
        if closing_submodule_manager {
            self.submodule_dialog.invalidate();
        }
        self.last_error = None;
    }

    fn request_window_close(&mut self) {
        if self.active_dialog == Some(DialogState::ConfirmWindowClose) {
            return;
        }
        // 关闭确认临时覆盖已有弹窗；用户取消时恢复，避免丢失尚未保存的表单。
        let previous_dialog = self.active_dialog.take();
        self.close_popups();
        self.dialog_before_window_close = previous_dialog;
        self.active_dialog = Some(DialogState::ConfirmWindowClose);
        self.last_error = None;
    }

    fn cancel_window_close(&mut self) {
        self.active_dialog = self.dialog_before_window_close.take();
        self.last_error = None;
    }

    fn should_close_window(&mut self) -> bool {
        if self.exit_requested {
            return true;
        }
        self.request_window_close();
        false
    }

    fn exit_application(&mut self, cx: &mut Context<Self>) {
        self.exit_requested = true;
        self.dialog_before_window_close = None;
        cx.quit();
    }

    fn minimize_to_tray(&mut self, window: &Window, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let result = self
                .tray
                .as_mut()
                .ok_or_else(|| {
                    self.tray_error
                        .clone()
                        .unwrap_or_else(|| "系统托盘不可用".to_string())
                })
                .and_then(|tray| tray.hide_window(window));
            match result {
                Ok(()) => {
                    self.active_dialog = None;
                    self.dialog_before_window_close = None;
                    self.last_error = None;
                }
                Err(error) => {
                    self.last_error = Some(error);
                    self.notify_error("无法缩小到系统托盘", cx);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            self.last_error = Some("当前平台暂不支持缩小到系统托盘".to_string());
            self.notify_error("无法缩小到系统托盘", cx);
        }
    }

    #[cfg(windows)]
    fn attach_window_to_tray(&mut self, window: &Window) {
        let result = self.tray.as_mut().map(|tray| tray.attach_window(window));
        if let Some(Err(error)) = result {
            self.tray = None;
            self.tray_error = Some(error);
        }
    }

    #[cfg(not(windows))]
    fn attach_window_to_tray(&mut self, _window: &Window) {}

    fn handle_tray_action(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let action = self.tray.as_ref().and_then(|tray| tray.next_action());
            match action {
                Some(tray::TrayAction::Show) => {
                    if let Some(tray) = self.tray.as_ref() {
                        tray.show_window();
                    }
                }
                Some(tray::TrayAction::Exit) => self.exit_application(cx),
                None => {}
            }
        }
        #[cfg(not(windows))]
        let _ = cx;
    }

    fn close_credential_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.credential_context_menu.is_some() {
            self.credential_context_menu = None;
            cx.notify();
        }
    }

    /// 打开设置中心，默认显示凭据管理分类。
    pub(crate) fn open_settings_center(&mut self) {
        self.close_popups();
        self.settings_center = Some(SettingsCategory::Credentials);
        self.reload_credential_records("凭据列表已加载");
    }

    /// 切换设置中心的分类。
    pub(crate) fn select_settings_category(&mut self, category: SettingsCategory) {
        self.settings_center = Some(category);
        match category {
            SettingsCategory::Credentials => {
                self.reload_credential_records("凭据列表已加载");
            }
            SettingsCategory::Proxy => {
                self.reset_proxy_form_from_settings();
            }
            SettingsCategory::Ai => {
                self.reset_ai_form_from_settings();
            }
            SettingsCategory::ExternalMerge => {
                self.reset_external_merge_form_from_settings();
            }
            SettingsCategory::Theme
            | SettingsCategory::Update
            | SettingsCategory::Shortcuts
            | SettingsCategory::About => {}
        }
    }

    /// 关闭设置中心。
    pub(crate) fn close_settings_center(&mut self) {
        self.settings_center = None;
        // 关闭设置中心时清掉可能残留的「保存并继续」待处理冲突路径，
        // 等价于原外部合并「取消」按钮的清理职责。
        external_merge_view::clear_pending_external_merge_path();
    }

    /// 设置页保存后按 `last_error` 提示成功/失败（各 save 失败置 Some、成功置 None）。
    /// 成功不关闭页面，失败把具体错误带进 toast。
    fn notify_settings_save(
        &mut self,
        success_msg: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        if let Some(err) = self.last_error.clone() {
            self.notify_error(format!("保存失败：{err}"), cx);
        } else {
            self.notify_success(success_msg, cx);
        }
    }

    fn open_credential_manager(&mut self) {
        self.open_settings_center();
        self.select_settings_category(SettingsCategory::Credentials);
    }

    pub(crate) fn reset_proxy_form_from_settings(&mut self) {
        let custom = self.proxy_settings.custom.normalized();
        self.proxy_mode = self.proxy_settings.mode;
        self.proxy_http_url.set_value(custom.http_proxy);
        self.proxy_https_url.set_value(custom.https_proxy);
        self.proxy_socks5_url.set_value(custom.socks5_proxy);
    }

    pub(crate) fn proxy_form_settings(&self) -> NetworkProxySettings {
        NetworkProxySettings {
            mode: self.proxy_mode,
            custom: CustomProxySettings {
                http_proxy: self.proxy_http_url.value.trim().to_string(),
                https_proxy: self.proxy_https_url.value.trim().to_string(),
                socks5_proxy: self.proxy_socks5_url.value.trim().to_string(),
            },
        }
    }

    pub(crate) fn set_proxy_mode(&mut self, mode: NetworkProxyMode) {
        self.proxy_mode = mode;
        self.last_error = None;
    }

    pub(crate) fn save_network_proxy_settings(&mut self) {
        let settings = self.proxy_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.proxy_settings = settings;
        self.save_proxy_settings();
        self.status = "代理设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn open_external_merge_settings(&mut self) {
        self.open_settings_center();
        self.select_settings_category(SettingsCategory::ExternalMerge);
        self.status = "合并工具设置已打开".into();
        self.last_error = None;
    }

    pub(crate) fn reset_external_merge_form_from_settings(&mut self) {
        self.external_merge_enabled_form = self.external_merge_settings.enabled;
        self.external_merge_auto_open_form = self.external_merge_settings.auto_open_intellij;
        self.external_merge_intellij_path
            .set_value(self.external_merge_settings.normalized_intellij_path());
        self.external_merge_detection = None;
    }

    pub(crate) fn external_merge_form_settings(&self) -> ExternalMergeSettings {
        ExternalMergeSettings {
            enabled: self.external_merge_enabled_form,
            auto_open_intellij: self.external_merge_auto_open_form,
            intellij_path: self.external_merge_intellij_path.value.trim().to_string(),
        }
    }

    pub(crate) fn set_external_merge_enabled_form(&mut self, enabled: bool) {
        self.external_merge_enabled_form = enabled;
        if !enabled {
            self.external_merge_auto_open_form = false;
        }
        self.last_error = None;
    }

    pub(crate) fn set_external_merge_auto_open_form(&mut self, enabled: bool) {
        self.external_merge_auto_open_form = enabled;
        if enabled {
            self.external_merge_enabled_form = true;
        }
        self.last_error = None;
    }

    pub(crate) fn save_external_merge_settings_from_form(&mut self) {
        self.external_merge_settings = self.external_merge_form_settings();
        self.save_external_merge_settings();
        self.status = "合并工具设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn test_external_merge_settings_from_form(&mut self) {
        let settings = self.external_merge_form_settings();
        match khaslana::external_merge::resolve_intellij_idea_command_with_settings(&settings) {
            Ok(path) => {
                self.external_merge_detection = Some((settings.clone(), true));
                self.external_merge_settings = settings;
                self.save_external_merge_settings();
                self.status = format!("已找到 IntelliJ IDEA：{}", path.display());
                self.last_error = None;
            }
            Err(err) => {
                self.external_merge_detection = Some((settings, false));
                self.status = "IntelliJ IDEA 检测失败".into();
                self.last_error = Some(err.to_string());
            }
        }
    }

    // ── 更新方法 ──────────────────────────────────────────────────────────

    pub(crate) fn start_update_check(&mut self, manual: bool) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        self.update_error = None;
        self.status = "检查更新中".into();

        let tx = self.tx.clone();
        let preferences = self.update_preferences.clone();
        let proxy_settings = self.proxy_settings.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let sources = update::manifest_sources_for(&preferences);
            match update::check_for_update(&sources, &preferences, &proxy_settings) {
                Ok(UpdateCheckResult::UpdateAvailable { manifest, asset }) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFinished {
                            manifest: Arc::new(manifest),
                            asset,
                        },
                    );
                }
                Ok(UpdateCheckResult::UpToDate) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: "当前已是最新版本".into(),
                            manual,
                        },
                    );
                }
                Ok(UpdateCheckResult::SkippedVersion) => {
                    // 用户跳过了此版本，静默忽略
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: String::new(),
                            manual,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateCheckFailed {
                            error: err.to_string(),
                            manual,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn start_update_download(&mut self) {
        let Some(manifest) = self.available_update.clone() else {
            return;
        };
        let asset = manifest.platforms.get("windows-x86_64").cloned();
        let Some(asset) = asset else {
            self.update_error = Some("缺少下载信息".into());
            return;
        };
        self.update_downloading = true;
        self.update_download_progress = None;
        self.update_error = None;
        self.status = "下载更新中".into();

        let tx = self.tx.clone();
        let config_dir = khaslana::default_database_path()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let proxy_settings = self.proxy_settings.clone();

        self.tasks.spawn(TaskKind::Long, move || {
            // 进度回调：发送 UpdateDownloadProgress 事件
            let on_progress = |downloaded: u64, total: u64| {
                let _ = tx.try_send(UiEvent::UpdateDownloadProgress { downloaded, total });
            };

            // 下载
            match update::download_update(&asset, &config_dir, &proxy_settings, Some(&on_progress))
            {
                Ok((zip_path, computed_sha256)) => {
                    // SHA-256 校验
                    if computed_sha256 != asset.sha256 {
                        send_ui_event(
                            &tx,
                            UiEvent::UpdateInstallFailed {
                                error: "更新包 SHA-256 校验失败，文件可能被篡改".into(),
                            },
                        );
                        return;
                    }
                    // 解压 staging
                    let version = manifest.version.clone();
                    match update::prepare_staging(&zip_path, &version, &config_dir) {
                        Ok(staging_dir) => {
                            send_ui_event(
                                &tx,
                                UiEvent::UpdateReadyToInstall {
                                    staging_dir,
                                    manifest,
                                },
                            );
                        }
                        Err(err) => {
                            send_ui_event(
                                &tx,
                                UiEvent::UpdateInstallFailed {
                                    error: format!("更新包解压失败：{err}"),
                                },
                            );
                        }
                    }
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::UpdateInstallFailed {
                            error: format!("更新包下载失败：{err}"),
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn install_update(
        &mut self,
        staging_dir: &Path,
        _version: &str,
        cx: &mut Context<Self>,
    ) {
        // 检查写入权限
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("khaslana.exe"));
        let exe_dir = current_exe
            .parent()
            .unwrap_or_else(|| Path::as_ref(Path::new(".")));

        // 尝试在 exe 目录创建临时文件来验证写入权限
        let test_file = exe_dir.join(".khaslana_update_test");
        let writable = fs::File::create(&test_file).is_ok();
        let _ = fs::remove_file(&test_file);

        if !writable {
            let version = self
                .available_update
                .as_ref()
                .map(|m| m.version.clone())
                .unwrap_or_default();
            self.active_dialog = Some(DialogState::UpdateNoWritePermission { version });
            return;
        }

        // 构造 updater 命令
        let new_exe = staging_dir.join("khaslana.exe");
        let new_updater = staging_dir.join("khaslana_updater.exe");
        let config_dir = khaslana::default_database_path()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let backup_dir = config_dir.join("updates").join("backup");
        let pid = std::process::id();

        // 先转为字符串，避免 Command::new move PathBuf 后无法引用
        let new_exe_str = new_exe.to_string_lossy().to_string();
        let new_updater_str = new_updater.to_string_lossy().to_string();
        let current_exe_str = current_exe.to_string_lossy().to_string();
        let backup_dir_str = backup_dir.to_string_lossy().to_string();
        let pid_str = pid.to_string();

        if let Err(err) = Command::new(&new_updater_str)
            .args([
                "--pid",
                &pid_str,
                "--target-exe",
                &current_exe_str,
                "--new-exe",
                &new_exe_str,
                "--new-updater",
                &new_updater_str,
                "--backup-dir",
                &backup_dir_str,
                "--restart",
            ])
            .spawn()
        {
            // updater 启动失败（杀软拦截、staging 目录被清理等）：留在当前
            // 版本并提示，不退出应用——静默退出会让用户误以为更新已安装。
            self.active_dialog = None;
            self.update_error = Some(format!("更新器启动失败：{err}"));
            self.status = "更新失败".into();
            self.notify_toast(
                AppToastKind::Error,
                format!("更新器启动失败：{err}，已保留当前版本"),
                cx,
            );
            return;
        }

        std::process::exit(0);
    }

    pub(crate) fn skip_version(&mut self, version: &str) {
        self.update_preferences.skipped_version = Some(version.to_string());
        self.save_update_preferences();
        self.active_dialog = None;
    }

    pub(crate) fn clear_skipped_version(&mut self) {
        self.update_preferences.skipped_version = None;
        self.save_update_preferences();
        self.status = "已清除跳过版本".into();
    }

    fn save_update_preferences(&self) {
        if let Err(err) = self
            .storage
            .save_update_preferences(&self.update_preferences)
        {
            tracing::warn!("update preferences write skipped: {err}");
        }
    }

    pub(crate) fn reset_ai_form_from_settings(&mut self) {
        self.ai_enabled_form = self.ai_settings.enabled;
        self.ai_base_url
            .set_value(self.ai_settings.base_url.clone());
        self.ai_api_key.set_value(self.ai_settings.api_key.clone());
        self.ai_model.set_value(self.ai_settings.model.clone());
    }

    pub(crate) fn ai_form_settings(&self) -> AiProviderSettings {
        let mut settings = self.ai_settings.clone();
        settings.enabled = self.ai_enabled_form;
        settings.base_url = self.ai_base_url.value.trim().to_string();
        settings.api_key = self.ai_api_key.value.trim().to_string();
        settings.model = self.ai_model.value.trim().to_string();
        settings
    }

    pub(crate) fn set_ai_enabled_form(&mut self, enabled: bool) {
        self.ai_enabled_form = enabled;
        self.last_error = None;
    }

    pub(crate) fn save_ai_provider_settings_from_form(&mut self) {
        let settings = self.ai_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.ai_settings = settings;
        self.save_ai_provider_settings();
        self.status = "AI 设置已保存".into();
        self.last_error = None;
    }

    pub(crate) fn test_network_proxy_settings(&mut self) {
        if self.busy || self.global_busy_tab.is_some() {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端，无法测试代理".into());
            return;
        };
        let settings = self.proxy_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }

        self.proxy_settings = settings;
        self.save_proxy_settings();
        self.begin_global_test_busy("正在测试代理连接");
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let result = (|| -> khaslana::Result<()> {
                let repo = Repository::open(repo_path)?;
                service.test_proxy(&repo, &RemoteName::new(remote))?;
                Ok(())
            })();
            match result {
                Ok(()) => send_ui_event(
                    &tx,
                    UiEvent::ProxyTestFinished {
                        message: "代理测试通过".into(),
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: None,
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    fn open_credential_form(&mut self) {
        self.credential_context_menu = None;
        let suggested_remote_url = self.snapshot.as_ref().and_then(|snapshot| {
            self.selected_remote
                .as_deref()
                .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
                .or_else(|| {
                    snapshot
                        .remotes
                        .iter()
                        .find(|remote| remote.name == "origin")
                })
                .or_else(|| snapshot.remotes.first())
                .map(|remote| remote.url.clone())
        });
        self.reset_credential_form();
        if let Some(url) = suggested_remote_url {
            self.credential_remote_url.set_value(url.clone());
            let mode = credential_form_mode_for_request(&CredentialRequest {
                url,
                username_from_url: None,
                allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                    | git2::CredentialType::SSH_KEY,
                repo_path: None,
                remote_name: None,
                operation_id: None,
            });
            self.credential_form_mode = mode;
            if mode == CredentialFormMode::Ssh {
                self.credential_username.set_value(
                    ssh_credentials::ssh_username_from_url(&self.credential_remote_url.value)
                        .unwrap_or_else(|| "git".into()),
                );
                self.discover_ssh_credentials_if_needed();
            }
        }
        self.active_dialog = Some(DialogState::CredentialForm { editing: None });
        self.last_error = None;
    }

    fn reset_credential_form(&mut self) {
        if self.ssh_credential_discovery.loading {
            // 关闭表单后忽略尚未完成的扫描结果，避免异步结果覆盖其他界面状态。
            self.ssh_credential_discovery.request_id = self
                .ssh_credential_discovery
                .request_id
                .wrapping_add(1)
                .max(1);
            self.ssh_credential_discovery.loading = false;
        }
        // 作废可能进行中的 OAuth 登录：递增请求号让迟到事件被忽略，并停止后台任务。
        if self.oauth_login_flow.loading {
            self.oauth_login_flow.request_id =
                self.oauth_login_flow.request_id.wrapping_add(1).max(1);
            self.oauth_login_flow.loading = false;
        }
        if let Some(cancel) = self.oauth_login_flow.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.oauth_login_flow.provider = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        self.oauth_login_flow.error = None;
        self.credential_form_mode = CredentialFormMode::Https;
        self.credential_scope = CredentialScope::RemoteUrl;
        self.credential_use_ssh_agent = false;
        self.credential_remote_url.clear();
        self.credential_username.clear();
        self.credential_secret.clear();
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        self.credential_display_name.clear();
    }

    fn close_credential_form(&mut self) {
        self.reset_credential_form();
        self.open_credential_manager();
        self.last_error = None;
        self.feedbacks
            .retain(|feedback| feedback.kind != AppToastKind::Error);
    }

    fn set_credential_form_mode(&mut self, mode: CredentialFormMode) {
        self.credential_form_mode = mode;
        self.last_error = None;
        if mode == CredentialFormMode::Ssh {
            if self.credential_username.value.trim().is_empty() {
                self.credential_username.set_value("git");
            }
            if let Some(ssh_url) = ssh_credentials::http_remote_to_ssh(
                &self.credential_remote_url.value,
                &self.credential_username.value,
            ) {
                self.credential_remote_url.set_value(ssh_url.clone());
                self.status = format!("已将适用远端地址切换为 SSH：{ssh_url}");
            }
            self.discover_ssh_credentials_if_needed();
        }
    }

    fn discover_ssh_credentials_if_needed(&mut self) {
        if self.ssh_credential_discovery.result.is_none() && !self.ssh_credential_discovery.loading
        {
            self.discover_ssh_credentials();
        }
    }

    pub(crate) fn discover_ssh_credentials(&mut self) {
        if self.ssh_credential_discovery.loading {
            return;
        }
        self.ssh_credential_discovery.request_id = self
            .ssh_credential_discovery
            .request_id
            .wrapping_add(1)
            .max(1);
        let request_id = self.ssh_credential_discovery.request_id;
        self.ssh_credential_discovery.loading = true;
        self.ssh_credential_discovery.error = None;
        self.status = "正在检测本机 SSH 身份".into();
        self.last_error = None;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            match ssh_credentials::discover_local_ssh_credentials() {
                Ok(result) => send_ui_event(
                    &tx,
                    UiEvent::SshCredentialsDiscovered { request_id, result },
                ),
                Err(error) => send_ui_event(
                    &tx,
                    UiEvent::SshCredentialDiscoveryFailed { request_id, error },
                ),
            }
        });
    }

    pub(crate) fn use_discovered_ssh_agent(&mut self) {
        self.credential_use_ssh_agent = true;
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        if self.credential_display_name.value.trim().is_empty()
            || self.credential_display_name.value.starts_with("SSH · ")
        {
            self.credential_display_name.set_value("本机 SSH Agent");
        }
        self.status = "已选择本机 SSH Agent".into();
        self.last_error = None;
    }

    pub(crate) fn use_discovered_ssh_key(&mut self, path: PathBuf) {
        if self.credential_key_path.value.trim() != path.to_string_lossy() {
            self.credential_passphrase.clear();
        }
        self.credential_use_ssh_agent = false;
        self.credential_key_path
            .set_value(path.display().to_string());
        if (self.credential_display_name.value.trim().is_empty()
            || self.credential_display_name.value == "本机 SSH Agent")
            && let Some(name) = path.file_name().and_then(|name| name.to_str())
        {
            self.credential_display_name
                .set_value(format!("SSH · {name}"));
        }
        self.status = format!("已选择 SSH 私钥：{}", path.display());
        self.last_error = None;
    }

    fn browse_credential_ssh_key(&mut self) {
        self.status = "正在选择 SSH 私钥...".into();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title("选择 SSH 私钥")
                .pick_file();
            send_ui_event(&tx, UiEvent::CredentialSshKeyFileSelected { path });
        });
    }

    /// OAuth 登录公共前置：递增 request_id、置 loading、建取消标记。返回 (request_id, cancel)。
    fn begin_oauth_login(&mut self, provider: OAuthProvider) -> (u64, Arc<AtomicBool>) {
        self.oauth_login_flow.request_id = self.oauth_login_flow.request_id.wrapping_add(1).max(1);
        let request_id = self.oauth_login_flow.request_id;
        self.oauth_login_flow.loading = true;
        self.oauth_login_flow.provider = Some(provider);
        self.oauth_login_flow.error = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.oauth_login_flow.cancel = Some(cancel.clone());
        self.last_error = None;
        (request_id, cancel)
    }

    /// 启动 GitHub OAuth Device Flow 登录：后台请求设备码 → 显示并打开浏览器 → 轮询令牌。
    pub(crate) fn start_github_login(&mut self) {
        if self.oauth_login_flow.loading {
            return;
        }
        if !oauth::is_configured() {
            self.last_error = Some("尚未配置 GitHub OAuth Client ID，请联系维护者".into());
            return;
        }
        let (request_id, cancel) = self.begin_oauth_login(OAuthProvider::Github);
        self.status = "正在请求 GitHub 设备码...".into();
        // 复用全局代理设置，与 AI/更新等网络请求保持一致。
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target("https://github.com/");
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            // 1. 请求设备码。
            let device = match oauth::request_device_code(proxy_url.clone()) {
                Ok(d) => d,
                Err(error) => {
                    send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error });
                    return;
                }
            };
            let user_code = device.user_code.clone();
            let verification_uri = device.verification_url().to_string();
            let device_code = device.device_code.clone();
            let interval = device.interval.max(1);
            let expires_in = device.expires_in;
            send_ui_event(
                &tx,
                UiEvent::OAuthLoginReady {
                    request_id,
                    provider: OAuthProvider::Github,
                    url: verification_uri,
                    user_code: Some(user_code),
                },
            );
            // 2. 轮询令牌（取消由 UI 通过 cancel 标记触发）。
            match oauth::poll_for_token(
                proxy_url.clone(),
                device_code,
                interval,
                expires_in,
                cancel.as_ref(),
            ) {
                Ok(token) => {
                    // 3. 用令牌换取登录名，作为 git 认证用户名。令牌不写日志。
                    match oauth::fetch_login(proxy_url, &token) {
                        Ok(username) => send_ui_event(
                            &tx,
                            UiEvent::OAuthLoginSucceeded {
                                request_id,
                                provider: OAuthProvider::Github,
                                username,
                                token,
                                gitee_refresh: None,
                            },
                        ),
                        Err(error) => {
                            send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error })
                        }
                    }
                }
                Err(error) => send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error }),
            }
        });
    }

    /// 启动 Gitee OAuth 授权码流登录：本地回调收 code → broker 换 token → 取登录名。
    pub(crate) fn start_gitee_login(&mut self) {
        if self.oauth_login_flow.loading {
            return;
        }
        if !oauth::is_gitee_configured() {
            self.last_error = Some("尚未配置 Gitee 登录服务（broker URL），详见 AGENTS.md".into());
            return;
        }
        let (request_id, cancel) = self.begin_oauth_login(OAuthProvider::Gitee);
        self.status = "正在等待浏览器完成 Gitee 授权...".into();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target("https://gitee.com/");
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let result = oauth::gitee_run_code_flow(proxy_url, cancel.as_ref(), |url| {
                // 本地回调监听已就绪，通知 UI 打开浏览器（验证码仅 GitHub 有，Gitee 为 None）。
                send_ui_event(
                    &tx,
                    UiEvent::OAuthLoginReady {
                        request_id,
                        provider: OAuthProvider::Gitee,
                        url: url.to_string(),
                        user_code: None,
                    },
                );
            });
            match result {
                Ok(grant) => send_ui_event(
                    &tx,
                    UiEvent::OAuthLoginSucceeded {
                        request_id,
                        provider: OAuthProvider::Gitee,
                        username: grant.username,
                        token: grant.access_token,
                        gitee_refresh: grant.refresh_token.zip(grant.expires_at),
                    },
                ),
                Err(error) => send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error }),
            }
        });
    }

    /// 取消进行中的 OAuth 登录。
    pub(crate) fn cancel_oauth_login(&mut self) {
        if let Some(cancel) = self.oauth_login_flow.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.oauth_login_flow.loading = false;
        self.oauth_login_flow.provider = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        self.status = "已取消登录".into();
    }

    fn save_credential_form(&mut self) {
        let url = self.credential_remote_url.value.trim().to_string();
        if url.is_empty() {
            self.last_error = Some("需要填写适用远端 URL".into());
            return;
        }
        let inferred_mode = credential_form_mode_for_request(&CredentialRequest {
            url: url.clone(),
            username_from_url: None,
            allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                | git2::CredentialType::SSH_KEY,
            repo_path: None,
            remote_name: None,
            operation_id: None,
        });
        if inferred_mode != self.credential_form_mode {
            self.last_error = Some(match self.credential_form_mode {
                CredentialFormMode::Ssh => {
                    "当前“适用远端 URL”不是 SSH 地址，请使用 git@主机:仓库路径 或 ssh:// 地址"
                        .into()
                }
                CredentialFormMode::Https => {
                    "当前“适用远端 URL”不是 HTTP(S) 地址，请切换到 SSH 凭据或修改地址".into()
                }
            });
            return;
        }
        let username = self
            .credential_username
            .value
            .trim()
            .to_string()
            .if_empty_then(|| "git".into());
        let display_name = optional_display_name(&self.credential_display_name.value);
        let credential = match self.credential_form_mode {
            CredentialFormMode::Https => {
                if self.credential_secret.value.is_empty() {
                    self.last_error = Some("需要填写密码或 PAT".into());
                    return;
                }
                GitCredential::UserPass {
                    username,
                    secret: self.credential_secret.value.clone(),
                    display_name,
                    save_to_keyring: true,
                    scope: self.credential_scope,
                }
            }
            CredentialFormMode::Ssh => {
                let key_path = self.credential_key_path.value.trim().to_string();
                if !self.credential_use_ssh_agent && key_path.is_empty() {
                    self.last_error = Some("需要填写 SSH 私钥路径或选择使用 SSH agent".into());
                    return;
                }
                if !self.credential_use_ssh_agent
                    && let Err(error) =
                        ssh_credentials::validate_ssh_private_key_path(Path::new(&key_path))
                {
                    self.last_error = Some(error);
                    return;
                }
                GitCredential::SshPassphrase {
                    username,
                    private_key_path: (!self.credential_use_ssh_agent).then_some(key_path),
                    passphrase: (!self.credential_passphrase.value.is_empty())
                        .then(|| self.credential_passphrase.value.clone()),
                    display_name,
                    save_to_keyring: true,
                    scope: self.credential_scope,
                }
            }
        };
        let request = CredentialRequest {
            url,
            username_from_url: Some(credential.username().to_string()),
            allowed_types: match self.credential_form_mode {
                CredentialFormMode::Https => git2::CredentialType::USER_PASS_PLAINTEXT,
                CredentialFormMode::Ssh => git2::CredentialType::SSH_KEY,
            },
            repo_path: None,
            remote_name: None,
            operation_id: None,
        };
        match self.credential_store.save_record(&request, &credential) {
            Ok(record) => {
                self.reset_credential_form();
                self.open_credential_manager();
                self.last_error = None;
                self.reload_credential_records("凭据已添加");
                self.pending_gitee_refresh_record = Some(record.id);
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }
    }

    pub(crate) fn open_remote_manager(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.active_dialog = Some(DialogState::RemoteManager);
        self.reload_credential_records("远端管理已打开");
    }

    fn open_remote_form(&mut self, editing: Option<String>) {
        let remote = match editing.as_ref() {
            Some(name) => {
                let Some(snapshot) = self.snapshot.as_ref() else {
                    self.last_error = Some("请先打开一个仓库".into());
                    return;
                };
                let Some(remote) = snapshot.remotes.iter().find(|remote| remote.name == *name)
                else {
                    self.last_error = Some("远端不存在".into());
                    return;
                };
                Some(remote.clone())
            }
            None => None,
        };

        self.remote_credential_policy = RemoteCredentialPolicy::AutoMatch;
        if let Some(remote) = remote {
            self.remote_name.set_value(remote.name.clone());
            self.remote_url.set_value(remote.url.clone());
            if let Some(repo_path) = self.repo_path.as_ref() {
                self.remote_credential_policy =
                    self.remote_credential_policy_for_remote(repo_path, &remote.name, &remote.url);
            }
        } else {
            self.remote_name.clear();
            self.remote_url.clear();
        }
        self.active_dialog = Some(DialogState::RemoteForm { editing });
        self.last_error = None;
    }

    fn open_delete_remote_confirm(&mut self, name: String) {
        self.active_dialog = Some(DialogState::ConfirmDeleteRemote { name });
        self.last_error = None;
    }

    pub(crate) fn open_delete_remote_branch_confirm(&mut self, remote_branch: String) {
        let Some((remote, branch)) = remote_branch.split_once('/') else {
            self.last_error = Some(format!("远端分支名称无效：{remote_branch}"));
            return;
        };
        self.branch_context_menu = None;
        self.active_dialog = Some(DialogState::ConfirmDeleteRemoteBranch {
            remote: remote.to_string(),
            branch: branch.to_string(),
        });
        self.last_error = None;
    }

    fn save_remote(&mut self, editing: Option<String>) {
        let name = self.remote_name.value.trim().to_string();
        let url = self.remote_url.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("需要填写远端名称".into());
            return;
        }
        if url.is_empty() {
            self.last_error = Some("需要填写远端地址".into());
            return;
        }
        if name.contains(char::is_whitespace) || name.contains('\\') || name.starts_with('-') {
            self.last_error = Some(format!("远端名称无效：{name}"));
            return;
        }

        let existing_remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        if editing.as_deref() != Some(name.as_str())
            && existing_remotes.iter().any(|remote| remote.name == name)
        {
            self.last_error = Some(format!("远端名称已存在：{name}"));
            return;
        }

        let selected_record =
            if let RemoteCredentialPolicy::Record(id) = &self.remote_credential_policy {
                let Some(record) = self
                    .credential_records
                    .iter()
                    .find(|record| record.id == *id)
                    .cloned()
                else {
                    self.last_error = Some("所选凭据记录不存在".into());
                    return;
                };
                Some(record)
            } else {
                None
            };
        if let Some(record) = selected_record.as_ref() {
            let compatible = match record.scope {
                CredentialScope::RemoteUrl => {
                    credential_record_is_compatible_with_url(record, &url)
                }
                CredentialScope::Host => credential_record_matches_remote_url(record, &url),
            };
            if !compatible {
                self.last_error = Some("所选凭据与远端地址协议或站点不匹配".into());
                return;
            }
            if record.scope == CredentialScope::RemoteUrl
                && let Err(err) = self
                    .credential_store
                    .update_record_remote_url(&record.id, &url)
            {
                self.last_error = Some(err.to_string());
                return;
            }
            if record.scope == CredentialScope::Host
                && let Err(err) = self.credential_store.touch_record(&record.id)
            {
                self.last_error = Some(err.to_string());
                return;
            }
            self.reload_credential_records("远端凭据绑定已更新");
        }

        if let Some(repo_path) = self.repo_path.as_ref() {
            let request = CredentialRequest {
                url: url.clone(),
                username_from_url: None,
                allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                    | git2::CredentialType::SSH_KEY,
                repo_path: Some(repo_path.clone()),
                remote_name: Some(name.clone()),
                operation_id: None,
            };
            set_remote_binding_for_request(
                &self.remote_credential_bindings,
                &request,
                self.remote_credential_policy.clone(),
            );
            self.save_remote_credential_bindings();
        }

        let old_selected = self.selected_remote.clone();
        if let Some(old_name) = editing.as_ref() {
            if old_selected.as_deref() == Some(old_name.as_str()) {
                self.selected_remote = Some(name.clone());
            }
        }
        let new_name = name.clone();
        match editing {
            Some(old_name) => {
                self.with_repo("远端已更新", move |service, repo| {
                    service.update_remote(
                        repo,
                        &RemoteName::new(old_name),
                        &RemoteName::new(new_name),
                        &url,
                    )
                });
            }
            None => {
                self.selected_remote = Some(name.clone());
                self.with_repo("远端已新增", move |service, repo| {
                    service.add_remote(repo, &RemoteName::new(name), &url)
                });
            }
        }
    }

    fn delete_remote(&mut self, name: String) {
        if self.selected_remote.as_deref() == Some(name.as_str()) {
            self.selected_remote = None;
        }
        self.with_repo("远端已删除", move |service, repo| {
            service.delete_remote(repo, &RemoteName::new(name))
        });
    }

    fn delete_remote_branch(&mut self, remote: String, branch: String) {
        self.with_repo("远端分支已删除", move |service, repo| {
            service.delete_remote_branch(repo, &RemoteName::new(remote), &BranchName::new(branch))
        });
    }

    fn reload_credential_records(&mut self, message: &'static str) {
        self.credential_context_menu = None;
        match self.credential_store.list_records() {
            Ok(records) => {
                self.credential_records = records;
                self.status = message.to_string();
                self.last_error = None;
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn open_delete_credential_confirm(&mut self, record_id: String, label: String) {
        self.active_dialog = Some(DialogState::ConfirmDeleteCredential { record_id, label });
        self.credential_context_menu = None;
        self.last_error = None;
    }

    fn open_credential_details(&mut self, record_id: String) {
        self.credential_context_menu = None;
        self.active_dialog = Some(DialogState::CredentialDetails { record_id });
        self.last_error = None;
    }

    fn open_credential_context_menu(
        &mut self,
        record_id: String,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.branch_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        let (x, y) =
            clamped_menu_position(event, window, CREDENTIAL_MENU_WIDTH, CREDENTIAL_MENU_HEIGHT);
        self.credential_context_menu = Some(CredentialContextMenu { record_id, x, y });
    }

    fn copy_credential_text(
        &mut self,
        text: Option<String>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            self.last_error = Some(format!("{label}为空，无法复制"));
            self.credential_context_menu = None;
            self.notify_warning(format!("{label}为空，无法复制"), cx);
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status = format!("已复制{label}");
        self.last_error = None;
        self.credential_context_menu = None;
        self.notify_success(self.status.clone(), cx);
    }

    fn delete_credential_record(&mut self, record_id: String) {
        match self.credential_store.delete_record(&record_id) {
            Ok(()) => {
                self.open_credential_manager();
                self.credential_context_menu = None;
                self.reload_credential_records("凭据已删除");
            }
            Err(err) => {
                self.open_credential_manager();
                self.last_error = Some(err.to_string());
            }
        }
    }

    /// 打开凭据测试地址确认弹窗：预填记录保存的远端地址，用户可改成
    /// 其它地址（典型：裸主机地址换成真实仓库地址）再开始测试。
    fn open_test_credential_dialog(&mut self, record_id: String) {
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        self.credential_test_error = None;
        self.credential_test_url
            .set_value(record.remote_url.clone());
        self.active_dialog = Some(DialogState::TestCredential { record_id });
    }

    /// 凭据测试弹窗的「开始测试」：非空 → 协议族 → HTTPS 同站点校验，
    /// 全过才关窗发起连接；任一失败写弹窗内错误条、不关窗。
    pub(crate) fn confirm_test_credential(&mut self) {
        let Some(DialogState::TestCredential { record_id }) = self.active_dialog.clone() else {
            return;
        };
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            // 记录列表已刷新导致记录消失：直接关窗提示。
            self.close_dialog();
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        let url = self.credential_test_url.value.trim().to_string();
        if let Err(error) = validate_credential_test_url(record.kind, &record.host, &url) {
            self.credential_test_error = Some(error);
            return;
        }
        self.credential_test_error = None;
        self.close_dialog();
        self.test_credential_record(record_id, url);
    }

    fn test_credential_record(&mut self, record_id: String, url: String) {
        if self.busy || self.global_busy_tab.is_some() {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let Some(mut record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        // 连接目标用弹窗确认的地址（记录本体保持原值，测试不改凭据）。
        record.remote_url = url;
        self.begin_global_test_busy("正在测试凭据连接");
        let store: Arc<dyn CredentialStore> = self.credential_store.clone();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            match test_credential_connection(store.as_ref(), &record) {
                Ok(()) => {
                    let records = store.list_records().unwrap_or_default();
                    send_ui_event(
                        &tx,
                        UiEvent::CredentialRecordsLoaded {
                            records,
                            message: "凭据测试通过".to_string(),
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::OperationFailed {
                            tab_id: None,
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    fn matching_credential_for_remote_url(&self, url: &str) -> Option<&CredentialRecord> {
        self.credential_records
            .iter()
            .filter(|record| credential_record_matches_remote_url(record, url))
            .max_by(|a, b| {
                let a_scope = match a.scope {
                    CredentialScope::RemoteUrl => 1,
                    CredentialScope::Host => 0,
                };
                let b_scope = match b.scope {
                    CredentialScope::RemoteUrl => 1,
                    CredentialScope::Host => 0,
                };
                a_scope
                    .cmp(&b_scope)
                    .then_with(|| a.last_used.unwrap_or(0).cmp(&b.last_used.unwrap_or(0)))
                    .then_with(|| a.updated_at.cmp(&b.updated_at))
            })
    }

    fn remote_credential_policy_for_remote(
        &self,
        repo_path: &Path,
        remote_name: &str,
        remote_url: &str,
    ) -> RemoteCredentialPolicy {
        let (repo_key, remote_key) = remote_binding_key(repo_path, remote_name);
        self.remote_credential_bindings
            .lock()
            .ok()
            .and_then(|bindings| {
                bindings
                    .remotes
                    .iter()
                    .find(|binding| {
                        binding.repo_path == repo_key
                            && binding.remote_name == remote_key
                            && normalize_remote_url(&binding.remote_url)
                                == normalize_remote_url(remote_url)
                    })
                    .map(|binding| binding.policy.clone())
            })
            .unwrap_or(RemoteCredentialPolicy::AutoMatch)
    }

    fn open_repo(&mut self, path: PathBuf) {
        if Repository::open(&path).is_err() {
            self.status = "打开仓库失败".to_string();
            self.last_error = Some("该目录不是 Git 仓库".to_string());
            return;
        }
        // 记录最近打开时间，供仓库切换下拉排序。
        let _ = self.storage.upsert_recent_repo(&path);
        let tab_id = self.ensure_tab_for_path(path.clone());
        self.queue_repository_load(
            tab_id,
            path,
            "正在打开仓库",
            "仓库已打开",
            LoadPriority::User,
        );
    }

    fn clone_repo(&mut self) {
        let url = self.clone_url.value.trim().to_string();
        let path_text = self.clone_path.value.trim().to_string();
        if url.is_empty() || path_text.is_empty() {
            self.last_error = Some("需要填写远程仓库 URL 和克隆到父文件夹".into());
            return;
        }
        if infer_clone_directory_name(&url).is_none() {
            self.last_error = Some("无法从远程仓库 URL 推导仓库文件夹名".into());
            return;
        };
        let Some(path) = infer_clone_target_path(&url, &path_text) else {
            self.last_error = Some("需要填写远程仓库 URL 和克隆到父文件夹".into());
            return;
        };
        if path.exists() {
            self.last_error = Some("目标仓库文件夹已存在".into());
            return;
        }
        let key = normalize_repo_path(&path);
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.path_key().as_deref() == Some(key.as_str()))
        {
            let previous_mode = self.current_tab_main_mode();
            self.active_tab = Some(tab.id);
            self.inherit_main_mode(previous_mode);
            self.last_error = Some("该仓库已经打开".into());
            self.save_session();
            return;
        }

        let tab_id = self.ensure_tab_for_path(path.clone());
        let service = self.service_for_tab(tab_id);
        let options = khaslana::CloneOptions {
            recursive_submodules: self.clone_recursive_submodules,
        };
        self.spawn_operation_for_tab(Some(tab_id), "正在克隆仓库", move || {
            service
                .clone_repo_with_options(&url, &RepoPath::new(path), options)
                .map(|snapshot| UiEvent::OperationFinished {
                    tab_id: Some(tab_id),
                    message: "克隆完成".to_string(),
                    snapshot: Some(snapshot),
                    diff: None,
                })
        });
    }

    pub(crate) fn refresh(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.queue_repository_load(tab_id, path, "正在刷新仓库", "已刷新", LoadPriority::User);
    }

    fn queue_repository_load(
        &mut self,
        tab_id: RepoTabId,
        path: PathBuf,
        started: &'static str,
        finished: &'static str,
        priority: LoadPriority,
    ) {
        let load_id = {
            let Some(tab) = self.tab_mut(tab_id) else {
                return;
            };
            let load_id = tab.repository_load_id.wrapping_add(1);
            tab.repository_load_id = load_id;
            tab.repo_path = Some(path.clone());
            tab.busy = true;
            tab.operation_blocker = OperationBlocker::None;
            tab.operation_blocker_started = None;
            tab.operation_kind = OperationKind::from_message(started);
            tab.loading = RepositoryLoading::default();
            tab.branch_sync_status = None;
            tab.branch_sync_loading = false;
            tab.branch_sync_request_id = tab.branch_sync_request_id.wrapping_add(1).max(1);
            tab.submodule_dialog.invalidate();
            tab.status = started.to_string();
            tab.last_error = None;
            load_id
        };
        self.close_popups();
        self.save_session();
        self.repository_load_queue
            .retain(|request| request.tab_id != tab_id);
        let request = RepositoryLoadRequest {
            tab_id,
            path,
            started,
            finished,
        };
        if priority == LoadPriority::User {
            self.repository_load_queue.push_front(request);
        } else {
            self.repository_load_queue.push_back(request);
        }
        self.start_queued_repository_loads();
        if let Some(tab) = self.tab(tab_id)
            && tab.repository_load_id == load_id
            && self
                .repository_load_queue
                .iter()
                .any(|request| request.tab_id == tab_id)
        {
            self.apply_status_event(Some(tab_id), |this| {
                this.status = "等待加载仓库".to_string();
            });
        }
    }

    fn start_queued_repository_loads(&mut self) {
        while self.active_repository_loads < MAX_CONCURRENT_REPO_LOADS {
            let Some(request) = self.repository_load_queue.pop_front() else {
                break;
            };
            if self.tab(request.tab_id).is_none() {
                continue;
            }
            self.active_repository_loads += 1;
            self.spawn_repository_load(request);
        }
    }

    fn spawn_repository_load(&mut self, request: RepositoryLoadRequest) {
        let tab_id = request.tab_id;
        let path = request.path;
        let started = request.started;
        let finished = request.finished;
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self
            .tab(tab_id)
            .map(|tab| tab.repository_load_id)
            .unwrap_or_default();
        let load_started = Instant::now();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id: Some(tab_id),
                message: started.to_string(),
            },
        );
        self.tasks.spawn(TaskKind::Short, move || {
            let stage_started = Instant::now();
            let repo_path = RepoPath::new(path);
            let fast = match service.open_fast(&repo_path) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::OperationFailed {
                            tab_id: Some(tab_id),
                            error: err.to_string(),
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            };
            perf_log(
                "repo.open_fast",
                stage_started,
                format!("tab={} branches={}", tab_id.0, fast.branches.len()),
            );
            send_ui_event(
                &tx,
                UiEvent::RepositoryFastLoaded {
                    tab_id,
                    message: "本地分支已加载，正在加载仓库详情".to_string(),
                    snapshot: fast,
                    load_id,
                },
            );

            let mut repo = match Repository::open(&repo_path.0) {
                Ok(repo) => repo,
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            };

            let stage_started = Instant::now();
            match service.snapshot_metadata(&mut repo) {
                Ok(snapshot) => {
                    perf_log(
                        "repo.metadata",
                        stage_started,
                        format!(
                            "tab={} branches={} remotes={} tags={} stashes={} conflicts={}",
                            tab_id.0,
                            snapshot.branches.len(),
                            snapshot.remotes.len(),
                            snapshot.tags.len(),
                            snapshot.stashes.len(),
                            snapshot.conflicts.len()
                        ),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryMetadataLoaded {
                            tab_id,
                            message: "仓库信息已加载".to_string(),
                            snapshot,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            }

            let stage_started = Instant::now();
            match service.status_fast(&repo) {
                Ok(changes) => {
                    perf_log(
                        "repo.status_fast",
                        stage_started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFastLoaded {
                            tab_id,
                            message: "快速变更已加载，正在补全未跟踪文件".to_string(),
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                    send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
                    return;
                }
            }

            let stage_started = Instant::now();
            match service.status_full(&repo) {
                Ok(changes) => {
                    perf_log(
                        "repo.status_full",
                        stage_started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFullLoaded {
                            tab_id,
                            message: finished.to_string(),
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
            perf_log(
                "repo.load_total",
                load_started,
                format!("tab={} load_id={}", tab_id.0, load_id),
            );
            send_ui_event(&tx, UiEvent::RepositoryLoadFinished { tab_id, load_id });
        });
    }

    fn load_full_status_for_tab(
        &self,
        tab_id: RepoTabId,
        path: PathBuf,
        load_id: u64,
        message: String,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<Vec<khaslana::WorktreeChange>> {
                let repo = Repository::open(path)?;
                service.status_full(&repo)
            })();
            match result {
                Ok(changes) => {
                    perf_log(
                        "repo.status_full.operation",
                        started,
                        format!("tab={} changes={}", tab_id.0, changes.len()),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryStatusFullLoaded {
                            tab_id,
                            message,
                            changes,
                            load_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::RepositoryLoadStageFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn prepare_branch_sync_status_request(
        &mut self,
    ) -> Option<(RepoTabId, PathBuf, String, u64, u64)> {
        let tab_id = self.active_tab_id()?;
        let path = self.repo_path.clone()?;
        let Some(remote) = self.current_remote() else {
            self.branch_sync_status = None;
            self.branch_sync_loading = false;
            self.branch_sync_request_id = self.branch_sync_request_id.wrapping_add(1).max(1);
            return None;
        };
        let load_id = self.repository_load_id;
        self.branch_sync_request_id = self.branch_sync_request_id.wrapping_add(1).max(1);
        self.branch_sync_loading = true;
        Some((tab_id, path, remote, load_id, self.branch_sync_request_id))
    }

    pub(crate) fn load_branch_sync_status_for_tab(
        &self,
        tab_id: RepoTabId,
        path: PathBuf,
        remote: String,
        load_id: u64,
        request_id: u64,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<Option<BranchSyncStatus>> {
                let repo = Repository::open(path)?;
                service.branch_sync_status(&repo, &RemoteName::new(remote))
            })();
            match result {
                Ok(status) => {
                    perf_log(
                        "branch.sync_status",
                        started,
                        format!(
                            "tab={} ahead={} behind={}",
                            tab_id.0,
                            status.as_ref().map(|status| status.ahead).unwrap_or(0),
                            status.as_ref().map(|status| status.behind).unwrap_or(0)
                        ),
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::BranchSyncStatusLoaded {
                            tab_id,
                            status,
                            load_id,
                            request_id,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::BranchSyncStatusFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                            request_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn with_repo<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_with_blocker(label, OperationBlocker::None, f);
    }

    pub(crate) fn with_repo_blocking<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_with_blocker(label, OperationBlocker::Modal, f);
    }

    fn with_repo_with_blocker<F>(&mut self, label: &'static str, blocker: OperationBlocker, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let service = self.service_for_tab(tab_id);
        let snapshot_service = service.clone();
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            started_message_for_label(label),
            blocker,
            move || {
                let mut repo = Repository::open(path)?;
                match f(service, &mut repo) {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: label.to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err(err) => {
                        let snapshot = snapshot_service.snapshot_after_operation(&mut repo).ok();
                        if let Some(snapshot) = snapshot
                            && !snapshot.conflicts.is_empty()
                        {
                            return Ok(UiEvent::OperationFinished {
                                tab_id: Some(tab_id),
                                message: conflicts::conflict_status_message(
                                    label,
                                    snapshot.conflicts.len(),
                                ),
                                snapshot: Some(snapshot),
                                diff: None,
                            });
                        }
                        Err(err)
                    }
                }
            },
        );
    }

    fn with_repo_keep_dialog<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label.to_string(), OperationBlocker::None, f)
    }

    fn with_repo_keep_dialog_blocking<F>(&mut self, label: &'static str, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label.to_string(), OperationBlocker::Modal, f)
    }

    fn with_repo_keep_dialog_owned_blocking<F>(&mut self, label: String, f: F)
    where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        self.with_repo_keep_dialog_owned_with_blocker(label, OperationBlocker::Modal, f)
    }

    fn with_repo_keep_dialog_owned_with_blocker<F>(
        &mut self,
        label: String,
        blocker: OperationBlocker,
        f: F,
    ) where
        F: FnOnce(GitService, &mut Repository) -> khaslana::Result<RepositorySnapshot>
            + Send
            + 'static,
    {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        if self.busy {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let service = self.service_for_tab(tab_id);
        let started = started_message_for_label_text(&label);
        self.apply_status_event(Some(tab_id), |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = blocker;
            this.operation_blocker_started = if blocker.blocks_interaction() {
                Some(Instant::now())
            } else {
                None
            };
            this.operation_kind = OperationKind::from_message(&started);
            this.status = started.clone();
            this.last_error = None;
        });
        let tx = self.tx.clone();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id: Some(tab_id),
                message: started,
            },
        );
        self.tasks.spawn(TaskKind::Long, move || {
            match Repository::open(path)
                .map_err(khaslana::GitError::from)
                .and_then(|mut repo| f(service, &mut repo))
            {
                Ok(snapshot) => send_ui_event(
                    &tx,
                    UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: label,
                        snapshot: Some(snapshot),
                        diff: None,
                    },
                ),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    pub(crate) fn current_remote(&self) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        self.selected_remote
            .as_ref()
            .filter(|remote| snapshot.remotes.iter().any(|info| info.name == **remote))
            .cloned()
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name.as_str() == "origin")
                    .map(|remote| remote.name.clone())
            })
            .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()))
    }

    fn sync_selected_remote(&mut self, snapshot: &RepositorySnapshot) {
        if snapshot.remotes.is_empty() {
            self.selected_remote = None;
            return;
        }

        if self
            .selected_remote
            .as_ref()
            .is_some_and(|remote| snapshot.remotes.iter().any(|info| info.name == *remote))
        {
            return;
        }

        self.selected_remote = snapshot
            .remotes
            .iter()
            .find(|remote| remote.name.as_str() == "origin")
            .map(|remote| remote.name.clone())
            .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()));
    }

    pub(crate) fn fetch(&mut self) {
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        self.with_repo("拉取远程引用完成", move |service, repo| {
            service.fetch(repo, &RemoteName::new(remote))
        });
    }

    pub(crate) fn refresh_remote(&mut self, remote: String) {
        self.remote_context_menu = None;
        self.selected_remote = Some(remote.clone());
        self.with_repo("远端已刷新", move |service, repo| {
            service.refresh(repo, Some(&RemoteName::new(remote)))
        });
    }

    pub(crate) fn open_remote_branch_operation(&mut self, kind: RemoteBranchOperationKind) {
        if matches!(
            kind,
            RemoteBranchOperationKind::Pull | RemoteBranchOperationKind::Push
        ) && !self.ensure_no_merge_in_progress("拉取或推送")
        {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let defaults = match remote_branch_dialog_defaults(snapshot, self.current_remote()) {
            Ok(defaults) => defaults,
            Err(message) => {
                self.last_error = Some(message);
                return;
            }
        };
        self.close_popups();
        self.remote_branch_operation.clear();
        self.remote_branch_operation.local_branch = Some(defaults.local_branch);
        self.remote_branch_operation.selected_remote = Some(defaults.remote);
        self.remote_branch_name.set_value(defaults.remote_branch);
        self.remote_branch_search.clear();
        self.active_dialog = Some(DialogState::RemoteBranchOperation { kind });
        self.last_error = None;
    }

    pub(crate) fn open_set_branch_upstream_dialog(&mut self, branch: String) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(local_branch) = local_branch_by_name(snapshot, &branch) else {
            self.last_error = Some(format!("本地分支不存在：{branch}"));
            return;
        };
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let remote_branch = default_remote_branch_for(local_branch, &remote);
        self.close_popups();
        self.remote_branch_operation.clear();
        self.remote_branch_operation.local_branch = Some(branch);
        self.remote_branch_operation.selected_remote = Some(remote);
        self.remote_branch_name.set_value(remote_branch);
        self.remote_branch_search.clear();
        self.active_dialog = Some(DialogState::RemoteBranchOperation {
            kind: RemoteBranchOperationKind::SetUpstream,
        });
        self.last_error = None;
    }

    pub(crate) fn select_remote_branch_operation_remote(&mut self, remote: String) {
        self.remote_branch_operation.selected_remote = Some(remote.clone());
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        let default_branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.remote_branch_operation
                    .local_branch
                    .as_deref()
                    .and_then(|name| local_branch_by_name(snapshot, name))
                    .or_else(|| remote_branch_operation::current_local_branch(snapshot))
            })
            .map(|local_branch| default_remote_branch_for(local_branch, &remote));
        if let Some(default_branch) = default_branch {
            self.remote_branch_name.set_value(default_branch);
        }
        self.last_error = None;
    }

    pub(crate) fn refresh_remote_branch_operation(&mut self) {
        let Some(remote) = self.remote_branch_operation.selected_remote.clone() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_operation.refreshing = true;
        self.with_repo_keep_dialog("拉取远程引用完成", move |service, repo| {
            service.fetch(repo, &RemoteName::new(remote))
        });
    }

    pub(crate) fn confirm_remote_branch_operation(&mut self, kind: RemoteBranchOperationKind) {
        if matches!(
            kind,
            RemoteBranchOperationKind::Pull | RemoteBranchOperationKind::Push
        ) && !self.ensure_no_merge_in_progress("拉取或推送")
        {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(local_branch) = self
            .remote_branch_operation
            .local_branch
            .as_deref()
            .and_then(|name| local_branch_by_name(snapshot, name))
            .or_else(|| remote_branch_operation::current_local_branch(snapshot))
            .map(|branch| branch.name.clone())
        else {
            self.last_error = Some("当前不是本地分支，无法拉取、推送或设置 upstream".into());
            return;
        };
        let Some(remote) = self.remote_branch_operation.selected_remote.clone() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let remote_branch = self.remote_branch_name.value.trim().to_string();
        if remote_branch.is_empty() {
            self.last_error = Some("需要填写远程分支".into());
            return;
        }
        if kind.requires_existing_remote_branch()
            && !remote_branch_exists(snapshot, &remote, &remote_branch)
        {
            self.last_error = Some("远端分支不存在，请点击刷新或选择已有分支".into());
            return;
        }

        let use_rebase = self.remote_branch_operation.use_rebase;
        self.active_dialog = None;
        self.remote_branch_operation.refreshing = false;
        self.remote_branch_operation.branch_dropdown_open = false;
        match kind {
            RemoteBranchOperationKind::Pull => {
                if use_rebase {
                    // 用变基代替合并
                    self.with_repo_blocking("变基拉取完成", move |service, repo| {
                        service.pull_branch_rebase(
                            repo,
                            &RemoteName::new(remote),
                            &BranchName::new(remote_branch),
                        )
                    });
                } else {
                    self.with_repo_blocking("拉取完成", move |service, repo| {
                        service.pull_branch(
                            repo,
                            &RemoteName::new(remote),
                            &BranchName::new(remote_branch),
                        )
                    });
                }
            }
            RemoteBranchOperationKind::Push => {
                self.with_repo_blocking("推送完成", move |service, repo| {
                    service.push_branch_to(
                        repo,
                        &RemoteName::new(remote),
                        &BranchName::new(local_branch),
                        &BranchName::new(remote_branch),
                        true,
                    )
                });
            }
            RemoteBranchOperationKind::SetUpstream => {
                self.with_repo("upstream 已设置", move |service, repo| {
                    service.set_branch_upstream(
                        repo,
                        &BranchName::new(local_branch),
                        &RemoteName::new(remote),
                        &BranchName::new(remote_branch),
                    )
                });
            }
        }
    }

    pub(crate) fn checkout(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("切换分支") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("切换分支完成", move |service, repo| {
            service.checkout_branch(repo, &BranchName::new(name))
        });
    }

    fn create_branch(&mut self) {
        let name = self.branch_name.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("需要填写分支名称".into());
            return;
        }
        let checkout = self.create_branch_checkout;
        self.with_repo("分支已创建", move |service, repo| {
            service.create_branch_from(repo, &BranchName::new(name), None, checkout)
        });
    }

    fn rename_branch(&mut self, old: String) {
        let new = self.branch_rename.value.trim().to_string();
        if new.is_empty() {
            self.last_error = Some("需要填写新的分支名称".into());
            return;
        }
        self.with_repo("分支已重命名", move |service, repo| {
            service.rename_branch(repo, &BranchName::new(old), &BranchName::new(new))
        });
    }

    pub(crate) fn delete_branch(&mut self, name: String) {
        self.with_repo("分支已删除", move |service, repo| {
            service.delete_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn merge_branch(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("再次合并") {
            return;
        }
        self.with_repo_blocking("合并操作已完成", move |service, repo| {
            service.merge_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_remote_branch(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("切换远端分支") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("远端分支已拉取到本地", move |service, repo| {
            service.checkout_remote_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_tag(&mut self, name: String) {
        if !self.ensure_no_merge_in_progress("检出标签") {
            return;
        }
        self.close_browse_if_comparing();
        self.with_repo_blocking("检出标签完成", move |service, repo| {
            service.checkout_tag(repo, &TagName::new(name))
        });
    }

    // ── 标签管理 ──────────────────────────────────────────────

    /// 打开创建标签对话框；`target_oid` 为 None 时目标为 HEAD。
    pub(crate) fn open_tag_form_dialog(
        &mut self,
        target_oid: Option<String>,
        target_summary: String,
    ) {
        self.close_popups();
        self.tag_name.clear();
        self.tag_message.clear();
        self.tag_annotated = true;
        self.active_dialog = Some(DialogState::TagForm {
            target_oid,
            target_summary,
        });
        self.last_error = None;
    }

    fn create_tag(&mut self) {
        let target_oid = match self.active_dialog.clone() {
            Some(DialogState::TagForm { target_oid, .. }) => target_oid,
            _ => return,
        };
        let name = self.tag_name.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("请填写标签名称".into());
            return;
        }
        let message = if self.tag_annotated {
            Some(self.tag_message.value.trim().to_string())
        } else {
            None
        };
        self.tag_name.clear();
        self.tag_message.clear();
        self.with_repo("标签已创建", move |service, repo| {
            service.create_tag(
                repo,
                &TagName::new(name.clone()),
                target_oid.as_deref(),
                message.as_deref(),
            )
        });
    }

    pub(crate) fn open_tag_push_dialog(&mut self, tag: String) {
        self.close_popups();
        self.tag_push_remote = self.current_remote();
        self.active_dialog = Some(DialogState::TagPush { tag });
        self.last_error = None;
    }

    fn push_tag(&mut self, tag: String) {
        let Some(remote) = self
            .tag_push_remote
            .clone()
            .filter(|remote| !remote.trim().is_empty())
            .or_else(|| self.current_remote())
        else {
            self.last_error = Some("当前仓库没有远端，无法推送标签".into());
            return;
        };
        self.with_repo_blocking("标签已推送", move |service, repo| {
            service.push_tag(
                repo,
                &RemoteName::new(remote.clone()),
                &TagName::new(tag.clone()),
            )
        });
    }

    pub(crate) fn open_delete_tag_confirm(&mut self, tag: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmDeleteTag { tag });
        self.last_error = None;
    }

    fn delete_tag(&mut self, tag: String) {
        self.with_repo("标签已删除", move |service, repo| {
            service.delete_tag(repo, &TagName::new(tag.clone()))
        });
    }

    pub(crate) fn open_delete_remote_tag_confirm(&mut self, remote: String, tag: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmDeleteRemoteTag { remote, tag });
        self.last_error = None;
    }

    fn delete_remote_tag(&mut self, remote: String, tag: String) {
        self.with_repo_blocking("远端标签已删除", move |service, repo| {
            service.delete_remote_tag(
                repo,
                &RemoteName::new(remote.clone()),
                &TagName::new(tag.clone()),
            )
        });
    }

    pub(crate) fn apply_stash(&mut self, index: usize) {
        if !self.ensure_no_merge_in_progress("应用贮藏") {
            return;
        }
        self.with_repo_blocking("应用贮藏完成", move |service, repo| {
            service.apply_stash(repo, index)
        });
    }

    pub(crate) fn pop_stash(&mut self, index: usize) {
        if !self.ensure_no_merge_in_progress("弹出贮藏") {
            return;
        }
        self.with_repo_blocking("弹出贮藏完成", move |service, repo| {
            service.pop_stash(repo, index)
        });
    }

    pub(crate) fn open_reset_confirm_dialog(
        &mut self,
        oid: String,
        summary: String,
        mode: ResetMode,
    ) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmReset { oid, summary, mode });
        self.last_error = None;
    }

    pub(crate) fn open_revert_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmRevert { oid, summary });
        self.last_error = None;
    }

    pub(crate) fn open_revert_merge_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmRevertMerge { oid, summary });
        self.last_error = None;
    }

    pub(crate) fn open_uncommit_to_staged_confirm_dialog(&mut self, oid: String, summary: String) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmUncommitToStaged { oid, summary });
        self.last_error = None;
    }

    fn open_discard_change_confirm_dialog(
        &mut self,
        paths: Vec<String>,
        scope: DiffScope,
        target: DiscardTarget,
    ) {
        if paths.is_empty() {
            self.last_error = Some("没有可回滚的文件".into());
            self.change_context_menu = None;
            return;
        }
        self.close_popups();
        match target {
            DiscardTarget::Single => {
                if let Some(path) = paths.first() {
                    self.select_only_change(path.clone(), scope.clone(), false);
                }
            }
            DiscardTarget::Selected | DiscardTarget::All => {
                self.clear_opposite_change_selection(&scope);
            }
        }
        self.active_dialog = Some(DialogState::ConfirmDiscardChange {
            scope,
            target,
            paths,
        });
        self.last_error = None;
    }

    /// 丢弃全部未暂存变更 — 先弹出确认弹窗
    fn confirm_discard_all(&mut self) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            let paths = self
                .change_indexes
                .unstaged
                .iter()
                .filter_map(|i| snapshot.changes.get(*i))
                .map(|c| c.path.clone())
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                self.open_discard_change_confirm_dialog(
                    paths,
                    DiffScope::Unstaged,
                    DiscardTarget::All,
                );
            }
        }
    }

    fn reset_to_commit(&mut self, oid: String, mode: ResetMode) {
        if !self.ensure_no_merge_in_progress("重置提交") {
            return;
        }
        self.with_repo_blocking("分支已重置", move |service, repo| {
            service.reset_to_commit(repo, &oid, mode)
        });
    }

    fn revert_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("回滚提交") {
            return;
        }
        self.with_repo_blocking("回滚提交完成", move |service, repo| {
            service.revert_commit(repo, &oid)
        });
    }

    fn revert_merge_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("撤销合并提交") {
            return;
        }
        self.with_repo_blocking("撤销合并完成", move |service, repo| {
            service.revert_merge_commit(repo, &oid)
        });
    }

    fn uncommit_to_staged(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("还原提交到暂存区") {
            return;
        }
        self.with_repo_blocking("提交已还原到暂存区", move |service, repo| {
            service.uncommit_to_staged(repo, &oid)
        });
    }

    fn discard_change(&mut self, paths: Vec<String>, scope: DiffScope, target: DiscardTarget) {
        if !self.ensure_no_merge_in_progress("回滚工作区更改") {
            return;
        }
        let message = match scope {
            DiffScope::Staged => match target {
                DiscardTarget::Single => "已回滚文件全部更改",
                DiscardTarget::Selected => "已回滚选定文件全部更改",
                DiscardTarget::All => "已回滚暂存区全部更改",
            },
            DiffScope::Unstaged => match target {
                DiscardTarget::Single => "已回滚未暂存更改",
                DiscardTarget::Selected => "已回滚选定未暂存更改",
                DiscardTarget::All => "已回滚修改区全部更改",
            },
        };
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.close_dialog();
        self.change_context_menu = None;
        let path_set = paths.iter().cloned().collect::<BTreeSet<_>>();
        self.change_selection
            .selected_mut(&scope)
            .retain(|path| !path_set.contains(path));
        self.clear_change_anchor_if_empty(&scope);
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");

        let service = self.service_for_tab(tab_id);
        let load_id = {
            let Some(tab) = self.tab_mut(tab_id) else {
                return;
            };
            tab.repository_load_id = tab.repository_load_id.wrapping_add(1);
            tab.repository_load_id
        };
        self.spawn_operation_without_load_bump_with_blocker(
            Some(tab_id),
            "正在回滚文件更改",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(repo_path)?;
                let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
                let path_refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                let snapshot = match scope {
                    DiffScope::Staged => service.discard_all_paths(&mut repo, path_refs)?,
                    DiffScope::Unstaged => service.discard_unstaged_paths(&mut repo, path_refs)?,
                };
                let changes = service.status_full(&repo)?;
                Ok(UiEvent::DiscardChangeFinished {
                    tab_id,
                    message: message.to_string(),
                    snapshot,
                    changes,
                    load_id,
                })
            },
        );
    }

    fn spawn_operation_without_load_bump_with_blocker<F>(
        &mut self,
        tab_id: Option<RepoTabId>,
        started: &'static str,
        blocker: OperationBlocker,
        f: F,
    ) where
        F: FnOnce() -> khaslana::Result<UiEvent> + Send + 'static,
    {
        if let Some(tab_id) = tab_id
            && self.tab(tab_id).is_none()
        {
            return;
        }
        let busy = tab_id
            .and_then(|id| self.tab(id).map(|tab| tab.busy))
            .unwrap_or(self.busy);
        if busy {
            self.apply_status_event(tab_id, |this| {
                this.last_error = Some("已有操作正在运行".into());
            });
            return;
        }
        self.close_popups();
        self.apply_status_event(tab_id, |this| {
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = blocker;
            this.operation_blocker_started = if blocker.blocks_interaction() {
                Some(Instant::now())
            } else {
                None
            };
            this.operation_kind = OperationKind::from_message(started);
            this.status = started.to_string();
            this.last_error = None;
        });
        let tx = self.tx.clone();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id,
                message: started.to_string(),
            },
        );
        self.tasks.spawn(TaskKind::Short, move || match f() {
            Ok(event) => {
                send_ui_event(&tx, event);
            }
            Err(err) => {
                send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id,
                        error: err.to_string(),
                    },
                );
            }
        });
    }

    pub(crate) fn copy_commit_sha(&mut self, oid: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(oid));
        self.commit_context_menu = None;
        self.status = "已复制提交 SHA".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    fn copy_file_absolute_path(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        let absolute_path = repository_file_absolute_path(repo_path, &path);
        cx.write_to_clipboard(ClipboardItem::new_string(
            absolute_path.to_string_lossy().into_owned(),
        ));
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.status = "已复制文件绝对路径".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    fn open_file_parent_directory(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        let absolute_path = repository_file_absolute_path(repo_path, &path);
        let Some(parent) = absolute_path.parent() else {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning("无法确定文件所在目录", cx);
            return;
        };
        if !parent.is_dir() {
            self.change_context_menu = None;
            self.file_path_context_menu = None;
            self.notify_warning(format!("文件所在目录不存在：{}", parent.display()), cx);
            return;
        }
        match system::open_directory(parent) {
            Ok(()) => {
                self.status = "已打开文件所在目录".into();
                self.last_error = None;
                self.notify_success(self.status.clone(), cx);
            }
            Err(err) => {
                let message = format!("打开文件所在目录失败：{err}");
                self.last_error = Some(message.clone());
                self.notify_error(message, cx);
            }
        }
        self.change_context_menu = None;
        self.file_path_context_menu = None;
    }

    /// 在系统资源管理器中打开当前仓库根目录（快捷键入口）。
    pub(crate) fn open_repo_in_explorer(&mut self, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.as_deref().map(PathBuf::from) else {
            self.notify_warning("当前没有打开的仓库", cx);
            return;
        };
        self.close_popups();
        match system::open_directory(&repo_path) {
            Ok(()) => {
                self.status = "已在资源管理器中打开仓库".into();
                self.last_error = None;
                self.notify_success(self.status.clone(), cx);
            }
            Err(err) => {
                let message = format!("打开仓库目录失败：{err}");
                self.last_error = Some(message.clone());
                self.notify_error(message, cx);
            }
        }
    }

    /// 以默认浏览器打开当前远端的 URL（快捷键入口）。
    pub(crate) fn open_remote_in_browser(&mut self, cx: &mut Context<Self>) {
        let Some(remote_name) = self.current_remote() else {
            self.notify_warning("当前仓库没有远端", cx);
            return;
        };
        // 先把 url 提取为 owned String，避免后续 self 操作与不可变借用冲突。
        let url = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .remotes
                .iter()
                .find(|r| r.name == remote_name)
                .map(|r| r.url.clone())
        });
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            self.notify_warning("远端 URL 为空或未找到", cx);
            return;
        };
        self.close_popups();
        open_url(&url);
        self.status = format!("已在浏览器中打开 {url}");
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn copy_branch_name(&mut self, branch: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(branch));
        self.branch_context_menu = None;
        self.status = "已复制分支名称".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn copy_remote_checkout_command(&mut self, branch: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(format!(
            "git checkout --track {branch}"
        )));
        self.branch_context_menu = None;
        self.status = "已复制 checkout 命令".into();
        self.last_error = None;
        self.notify_success(self.status.clone(), cx);
    }

    fn toggle_encoding_menu(&mut self, target: EncodingMenuTarget) {
        if self.encoding_menu_closed_by_capture == Some(target) {
            self.encoding_menu_closed_by_capture = None;
            self.encoding_menu_target = None;
            return;
        }
        self.encoding_menu_closed_by_capture = None;
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.active_dialog = None;
        self.commit_graph.branch_menu_open = false;
        self.commit_graph_branch_search.clear();
        self.encoding_menu_target = if self.encoding_menu_target == Some(target) {
            None
        } else {
            Some(target)
        };
    }

    fn choose_diff_encoding(&mut self, encoding: DiffEncodingChoice) {
        self.encoding_menu_target = None;
        self.encoding_menu_closed_by_capture = None;
        self.set_current_diff_encoding(encoding);
    }

    fn change_paths(&self, scope: DiffScope) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .changes
                    .iter()
                    .filter(|change| match scope {
                        DiffScope::Staged => change.staged.is_some(),
                        DiffScope::Unstaged => change.unstaged.is_some(),
                    })
                    .map(|change| change.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selected_change_paths(&self, scope: DiffScope) -> Vec<String> {
        self.change_selection
            .selected(&scope)
            .iter()
            .cloned()
            .collect()
    }

    fn is_change_selected(&self, scope: &DiffScope, path: &str) -> bool {
        self.change_selection.selected(scope).contains(path)
    }

    pub(crate) fn has_local_branch_for_remote(&self, remote_branch: &str) -> bool {
        let Some((_, local_name)) = remote_branch.split_once('/') else {
            return false;
        };
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.branches.iter().any(|branch| {
                branch.kind == BranchKind::Local && branch.name.as_str() == local_name
            })
        })
    }

    fn clear_opposite_change_selection(&mut self, scope: &DiffScope) {
        match scope {
            DiffScope::Staged => {
                self.change_selection.unstaged.clear();
                self.change_selection.unstaged_anchor = None;
            }
            DiffScope::Unstaged => {
                self.change_selection.staged.clear();
                self.change_selection.staged_anchor = None;
            }
        }
    }

    fn clear_change_anchor_if_empty(&mut self, scope: &DiffScope) {
        if !self.change_selection.selected(scope).is_empty() {
            return;
        }
        match scope {
            DiffScope::Staged => self.change_selection.staged_anchor = None,
            DiffScope::Unstaged => self.change_selection.unstaged_anchor = None,
        }
    }

    fn select_only_change(&mut self, path: String, scope: DiffScope, load_diff: bool) {
        self.change_selection.clear();
        self.change_selection
            .selected_mut(&scope)
            .insert(path.clone());
        self.change_selection.set_anchor(&scope, path.clone());
        if load_diff {
            self.load_diff(path, scope);
        }
    }

    fn select_change_from_mouse(&mut self, path: String, scope: DiffScope, event: &MouseDownEvent) {
        self.clear_opposite_change_selection(&scope);
        let multi = event.modifiers.control || event.modifiers.platform;
        if event.modifiers.shift {
            self.select_change_range(path.clone(), scope.clone());
        } else if multi {
            let selected = self.change_selection.selected_mut(&scope);
            if selected.contains(&path) {
                selected.remove(&path);
                self.clear_change_anchor_if_empty(&scope);
            } else {
                selected.insert(path.clone());
                self.change_selection.set_anchor(&scope, path.clone());
                self.load_diff(path.clone(), scope.clone());
            }
        } else if self.is_change_selected(&scope, &path) {
            self.change_selection.selected_mut(&scope).remove(&path);
            self.clear_change_anchor_if_empty(&scope);
        } else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
        }
    }

    fn select_change_range(&mut self, path: String, scope: DiffScope) {
        self.clear_opposite_change_selection(&scope);
        let paths = self.change_paths(scope.clone());
        let Some(current_index) = paths.iter().position(|candidate| candidate == &path) else {
            return;
        };
        let Some(anchor) = self.change_selection.anchor(&scope).cloned() else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
            return;
        };
        let Some(anchor_index) = paths.iter().position(|candidate| candidate == &anchor) else {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
            return;
        };
        let (start, end) = if anchor_index <= current_index {
            (anchor_index, current_index)
        } else {
            (current_index, anchor_index)
        };
        let selected = self.change_selection.selected_mut(&scope);
        selected.clear();
        selected.extend(paths[start..=end].iter().cloned());
        self.load_diff(path, scope);
    }

    fn ensure_change_context_selection(&mut self, path: String, scope: DiffScope) {
        self.clear_opposite_change_selection(&scope);
        if !self.is_change_selected(&scope, &path) {
            self.change_selection.selected_mut(&scope).clear();
            self.change_selection
                .selected_mut(&scope)
                .insert(path.clone());
            self.change_selection.set_anchor(&scope, path.clone());
            self.load_diff(path, scope);
        }
    }

    fn open_change_context_menu(
        &mut self,
        path: String,
        scope: DiffScope,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.ensure_change_context_selection(path.clone(), scope.clone());
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.file_path_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let menu_height = if scope == DiffScope::Staged {
            STAGED_CHANGE_MENU_HEIGHT
        } else {
            CHANGE_MENU_HEIGHT
        };
        let (x, y) = clamped_menu_position(event, window, CHANGE_MENU_WIDTH, menu_height);
        self.change_context_menu = Some(ChangeContextMenu { path, scope, x, y });
    }

    pub(crate) fn open_file_path_context_menu(
        &mut self,
        path: String,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.credential_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let (x, y) =
            clamped_menu_position(event, window, FILE_PATH_MENU_WIDTH, FILE_PATH_MENU_HEIGHT);
        self.file_path_context_menu = Some(FilePathContextMenu { path, x, y });
    }

    fn mouse_down_inside_context_menu(&self, event: &MouseDownEvent) -> bool {
        let x: f32 = event.position.x.into();
        let y: f32 = event.position.y.into();
        self.branch_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, BRANCH_MENU_WIDTH, BRANCH_MENU_HEIGHT)
        }) || self.remote_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, REMOTE_MENU_WIDTH, REMOTE_MENU_HEIGHT)
        }) || self.change_context_menu.as_ref().is_some_and(|menu| {
            let height = if menu.scope == DiffScope::Staged {
                STAGED_CHANGE_MENU_HEIGHT
            } else {
                CHANGE_MENU_HEIGHT
            };
            point_in_menu(x, y, menu.x, menu.y, CHANGE_MENU_WIDTH, height)
        }) || self.file_path_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(
                x,
                y,
                menu.x,
                menu.y,
                FILE_PATH_MENU_WIDTH,
                FILE_PATH_MENU_HEIGHT,
            )
        }) || self.credential_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(
                x,
                y,
                menu.x,
                menu.y,
                CREDENTIAL_MENU_WIDTH,
                CREDENTIAL_MENU_HEIGHT,
            )
        }) || self.tag_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, TAG_MENU_WIDTH, TAG_MENU_HEIGHT)
        }) || self.stash_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, STASH_MENU_WIDTH, STASH_MENU_HEIGHT)
        }) || self
            .workflow_template_context_menu
            .as_ref()
            .is_some_and(|menu| {
                point_in_menu(
                    x,
                    y,
                    menu.x,
                    menu.y,
                    WORKFLOW_TEMPLATE_MENU_WIDTH,
                    WORKFLOW_TEMPLATE_MENU_HEIGHT,
                )
            })
            || self.commit_context_menu.as_ref().is_some_and(|menu| {
                point_in_menu(x, y, menu.x, menu.y, COMMIT_MENU_WIDTH, menu.height)
            })
            || self.repo_switcher_menu.as_ref().is_some_and(|menu| {
                point_in_repo_switcher(x, y, menu, self.repo_switcher_anchor.as_ref())
            })
    }

    pub(crate) fn open_commit_context_menu(
        &mut self,
        oid: String,
        short_oid: String,
        summary: String,
        parent_count: usize,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.select_history_commit(oid.clone());
        self.branch_context_menu = None;
        self.change_context_menu = None;
        self.file_path_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let is_unpushed = self
            .branch_sync_status
            .as_ref()
            .is_some_and(|status| status.unpushed_oids.iter().any(|id| id == &oid));
        let height = if is_unpushed {
            COMMIT_UNPUSHED_MENU_HEIGHT
        } else {
            COMMIT_MENU_HEIGHT
        };
        let (x, y) = clamped_menu_position(event, window, COMMIT_MENU_WIDTH, height);
        let is_head = self
            .history_commits
            .iter()
            .find(|commit| commit.oid == oid)
            .is_some_and(|commit| {
                commit
                    .refs
                    .iter()
                    .any(|reference| reference.kind == khaslana::CommitRefKind::Head)
            });
        self.commit_context_menu = Some(CommitContextMenu {
            oid,
            short_oid,
            summary,
            parent_count,
            is_unpushed,
            is_head,
            height,
            x,
            y,
        });
    }

    fn start_resize_column(&mut self, target: ResizeTarget, event: &MouseDownEvent) {
        // 注意：不要在这里 close_popups——根层 capture_any_mouse_down 已对真正的
        // 菜单外部点击统一关闭弹层；菜单边缘容差区内的点击命中判定视为菜单内部
        //（菜单保持打开），此处若再关会把容差区内落在分割线上的点击重新误杀。
        // 详情区对半分模式（height = None）：点击处即分割条顶部，用左列顶部
        // 坐标推导当前实际高度并固化为绝对值，后续拖拽增量才有基准。
        if target == ResizeTarget::HistoryDetails && self.history_details_height.is_none() {
            let click_y: f32 = event.position.y.into();
            let column_top = self.history_details_top_hint.get();
            let derived = if column_top > 0.0 {
                (click_y - column_top - 4.0)
                    .clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT)
            } else {
                DEFAULT_HISTORY_DETAILS_HEIGHT
            };
            self.history_details_height = Some(derived);
        }
        let state = ResizeState {
            start_x: event.position.x.into(),
            start_y: event.position.y.into(),
            start_width: self.column_width(target),
            start_height: self.row_height(target),
        };
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width = Some(state),
            ResizeTarget::Changes => self.resizing_changes_width = Some(state),
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width = Some(state),
            ResizeTarget::HistoryFiles => self.resizing_history_files_width = Some(state),
            ResizeTarget::HistoryInspectorFiles => {
                self.resizing_history_inspector_files_width = Some(state)
            }
            ResizeTarget::HistoryDetails => self.resizing_history_details_height = Some(state),
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = Some(state),
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width = Some(state),
        }
    }

    fn update_resize_column(&mut self, target: ResizeTarget, event: &MouseMoveEvent) {
        let Some(resize) = self.resize_state(target) else {
            return;
        };
        let current_x: f32 = event.position.x.into();
        let delta = current_x - resize.start_x;
        match target {
            ResizeTarget::HistoryDetails => {
                let current_y: f32 = event.position.y.into();
                let delta = current_y - resize.start_y;
                let height = (resize.start_height + delta)
                    .clamp(MIN_HISTORY_DETAILS_HEIGHT, MAX_HISTORY_DETAILS_HEIGHT);
                self.set_row_height(target, height);
            }
            ResizeTarget::HistoryFiles => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_HISTORY_FILES_WIDTH, MAX_HISTORY_FILES_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::WorkflowTemplates => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_WORKFLOW_TEMPLATES_WIDTH, MAX_WORKFLOW_TEMPLATES_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::HistoryInspectorFiles => {
                let width = (resize.start_width + delta).clamp(
                    MIN_HISTORY_INSPECTOR_FILES_WIDTH,
                    MAX_HISTORY_INSPECTOR_FILES_WIDTH,
                );
                self.set_column_width(target, width);
            }
            ResizeTarget::BrowseFiles => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_BROWSE_TREE_WIDTH, MAX_BROWSE_TREE_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::HistoryGraph => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_HISTORY_GRAPH_WIDTH, MAX_HISTORY_GRAPH_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::Sidebar | ResizeTarget::Changes => {
                let width = (resize.start_width + delta).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
                self.set_column_width(target, width);
            }
        }
    }

    fn finish_resize_column(&mut self, target: ResizeTarget) {
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width = None,
            ResizeTarget::Changes => self.resizing_changes_width = None,
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width = None,
            ResizeTarget::HistoryFiles => self.resizing_history_files_width = None,
            ResizeTarget::HistoryInspectorFiles => {
                self.resizing_history_inspector_files_width = None
            }
            ResizeTarget::HistoryDetails => self.resizing_history_details_height = None,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = None,
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width = None,
        }
        // 拖拽结束：布局已定型，同步落库（重启恢复）。
        self.save_layout_preferences();
    }

    fn reset_resize_target(&mut self, target: ResizeTarget) {
        self.finish_resize_column(target);
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = DEFAULT_SIDEBAR_WIDTH,
            ResizeTarget::Changes => self.changes_width = DEFAULT_CHANGES_WIDTH,
            ResizeTarget::WorkflowTemplates => {
                self.workflow_templates_width = DEFAULT_WORKFLOW_TEMPLATES_WIDTH
            }
            ResizeTarget::HistoryFiles => self.history_files_width = DEFAULT_HISTORY_FILES_WIDTH,
            ResizeTarget::HistoryInspectorFiles => {
                self.history_inspector_files_width = DEFAULT_HISTORY_INSPECTOR_FILES_WIDTH
            }
            // 双击复位：回到检查器的默认详情高度。
            ResizeTarget::HistoryDetails => self.history_details_height = None,
            ResizeTarget::BrowseFiles => self.browse_tree_width = DEFAULT_BROWSE_TREE_WIDTH,
            ResizeTarget::HistoryGraph => self.history_graph_width = DEFAULT_HISTORY_GRAPH_WIDTH,
        }
        // finish_resize_column 已保存一次；复位改写了默认值后再保存最终状态。
        self.save_layout_preferences();
    }

    fn column_width(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width,
            ResizeTarget::Changes => self.changes_width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width,
            ResizeTarget::HistoryFiles => self.history_files_width,
            ResizeTarget::HistoryInspectorFiles => self.history_inspector_files_width,
            ResizeTarget::HistoryDetails => 0.0,
            ResizeTarget::BrowseFiles => self.browse_tree_width,
            ResizeTarget::HistoryGraph => self.history_graph_width,
        }
    }

    fn set_column_width(&mut self, target: ResizeTarget, width: f32) {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = width,
            ResizeTarget::Changes => self.changes_width = width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width = width,
            ResizeTarget::HistoryFiles => self.history_files_width = width,
            ResizeTarget::HistoryInspectorFiles => self.history_inspector_files_width = width,
            ResizeTarget::HistoryDetails => {}
            ResizeTarget::BrowseFiles => self.browse_tree_width = width,
            ResizeTarget::HistoryGraph => self.history_graph_width = width,
        }
    }

    fn row_height(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::HistoryDetails => self
                .history_details_height
                .unwrap_or(DEFAULT_HISTORY_DETAILS_HEIGHT),
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::HistoryInspectorFiles
            | ResizeTarget::BrowseFiles
            | ResizeTarget::HistoryGraph => 0.0,
        }
    }

    fn set_row_height(&mut self, target: ResizeTarget, height: f32) {
        match target {
            ResizeTarget::HistoryDetails => self.history_details_height = Some(height),
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::HistoryInspectorFiles
            | ResizeTarget::BrowseFiles
            | ResizeTarget::HistoryGraph => {}
        }
    }

    fn resize_state(&self, target: ResizeTarget) -> Option<ResizeState> {
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width,
            ResizeTarget::Changes => self.resizing_changes_width,
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width,
            ResizeTarget::HistoryFiles => self.resizing_history_files_width,
            ResizeTarget::HistoryInspectorFiles => self.resizing_history_inspector_files_width,
            ResizeTarget::HistoryDetails => self.resizing_history_details_height,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width,
            ResizeTarget::HistoryGraph => self.resizing_history_graph_width,
        }
    }

    fn toggle_diff_headers(&mut self) {
        self.diff_headers_expanded = !self.diff_headers_expanded;
        self.reset_uniform_scroll("diff-scroll");
    }

    fn toggle_history_diff_headers(&mut self) {
        self.history_diff_headers_expanded = !self.history_diff_headers_expanded;
        self.reset_uniform_scroll("history-diff-scroll");
    }

    fn toggle_browse_diff_headers(&mut self) {
        self.browse.diff_headers_expanded = !self.browse.diff_headers_expanded;
        self.reset_uniform_scroll("browse-diff-scroll");
    }

    pub(crate) fn set_main_mode(&mut self, mode: MainMode) {
        self.main_mode = mode;
        // Navigator 偏好由 tab 内的各模式独立保存，切换模式不重置用户选择。
        self.close_popups();
        if self.main_mode == MainMode::Conflict {
            self.ensure_conflict_views_loaded();
            self.sync_conflict_editor_from_state();
            // 进入冲突工作台即为选中文件补算三栏语法高亮
            let panes = [
                ConflictSyntaxPane::Ours,
                ConflictSyntaxPane::Theirs,
                ConflictSyntaxPane::Draft,
            ];
            self.schedule_conflict_syntax_for_selected(&panes);
        }
        if self.main_mode == MainMode::Workflow {
            self.refresh_workflow_templates();
        }
        if self.main_mode == MainMode::History || self.main_mode == MainMode::CommitGraph {
            self.ensure_history_loaded();
        }
    }

    /// 刷新历史时保留旧列表可见，等新数据就绪后直接替换
    fn refresh_history(&mut self) {
        // 保留：commits、graph_rows、has_more、refs_cache、selected_commit。
        // 选中提交的文件列表与差异按 oid 不可变，同样保留展示（与提交列表
        // 同一套 stale-while-revalidate 策略）；若新列表丢弃选中提交，
        // 由 HistoryCommitsLoaded 统一清空文件与差异。
        self.history_refreshing = true;
        self.history_loading = HistoryLoading::default();
    }

    pub(crate) fn diff_cache_key(&self, kind: DiffCacheKind, repo_path: &Path) -> DiffCacheKey {
        DiffCacheKey {
            repo_key: normalize_repo_path(repo_path),
            load_id: self.repository_load_id,
            encoding: self.diff_encoding_choice_for_path(repo_path),
            kind,
            full_file: self.full_file_view,
        }
    }

    pub(crate) fn cached_diff(&self, key: &DiffCacheKey) -> Option<Arc<FileDiff>> {
        self.diff_cache.borrow_mut().get(key).cloned()
    }

    pub(crate) fn cache_diff(&self, key: DiffCacheKey, diff: Arc<FileDiff>) {
        self.diff_cache.borrow_mut().put(key, diff);
    }

    // ===== 提交图谱页 =====

    /// 从主历史页「图谱」按钮进入图谱页。不重置任何图谱状态：
    /// 高亮分支、开关、搜索词与滚动位置跨跳转无损保留。
    pub(crate) fn open_commit_graph(&mut self) {
        self.set_main_mode(MainMode::CommitGraph);
    }

    /// 关闭图谱页返回主历史页。「在提交记录页查看」跳转复用同一出口：
    /// 选中提交与已预加载的文件/差异在四象限直接就位。
    pub(crate) fn close_commit_graph(&mut self) {
        self.set_main_mode(MainMode::History);
        self.status = "已返回提交记录页".to_string();
    }

    /// 图谱页分支高亮下拉的展开/收起（与编码菜单同一套防重开模式）。
    fn toggle_commit_graph_branch_menu(&mut self, window: &mut Window) {
        if self.commit_graph_branch_menu_closed_by_capture {
            self.commit_graph_branch_menu_closed_by_capture = false;
            self.commit_graph.branch_menu_open = false;
            self.commit_graph_branch_search.clear();
            return;
        }
        // 互斥关闭其余弹层菜单
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        self.active_dialog = None;
        self.commit_graph.branch_menu_open = !self.commit_graph.branch_menu_open;
        if self.commit_graph.branch_menu_open {
            // 打开即清空上次搜索词并聚焦搜索框，输入即过滤（仓库切换下拉同款）。
            self.commit_graph_branch_search.clear();
            window.focus(&self.commit_graph_branch_search.focus);
        }
    }

    /// 设置/清除分支动向高亮；Some 时后台计算谱系 OID 集合。
    pub(crate) fn set_commit_graph_highlight(&mut self, branch: Option<String>) {
        self.close_popups();
        self.commit_graph.highlight_branch = branch.clone();
        self.commit_graph.trace = None;
        match branch {
            Some(branch) => {
                self.status = format!("正在计算分支谱系：{branch}");
                self.refresh_commit_graph_trace();
            }
            None => {
                self.commit_graph.trace_loading = false;
                self.status = "已关闭分支高亮".to_string();
            }
        }
    }

    /// 切换「仅领先 HEAD」模式：重新计算谱系集合（全谱系 ↔ 增量动向）。
    fn toggle_commit_graph_ahead_only(&mut self) {
        if self.commit_graph.highlight_branch.is_none() {
            return;
        }
        self.commit_graph.highlight_ahead_only = !self.commit_graph.highlight_ahead_only;
        self.refresh_commit_graph_trace();
    }

    /// 切换「淡化合并提交」：纯渲染开关，无需后台计算。
    fn toggle_commit_graph_dim_merges(&mut self) {
        self.commit_graph.dim_merges = !self.commit_graph.dim_merges;
    }

    /// 后台计算当前高亮分支的谱系 OID 集合（Short 任务池，纯本地 revwalk）。
    fn refresh_commit_graph_trace(&mut self) {
        let Some(branch) = self.commit_graph.highlight_branch.clone() else {
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let ahead_only = self.commit_graph.highlight_ahead_only;
        let load_id = self.repository_load_id;
        self.commit_graph.trace_seq += 1;
        let seq = self.commit_graph.trace_seq;
        self.commit_graph.trace = None;
        self.commit_graph.trace_loading = true;

        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let (oids, truncated) = service.branch_commit_oids(&repo, &branch, ahead_only)?;
                Ok(UiEvent::CommitTraceLoaded {
                    tab_id,
                    branch,
                    ahead_only,
                    oids,
                    truncated,
                    load_id,
                    seq,
                })
            })();
            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::CommitTraceLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                            seq,
                        },
                    );
                }
            }
        });
    }

    // ===== 分支浏览模式 =====

    /// 从侧边栏分支右键菜单进入浏览模式。
    pub(crate) fn open_browse_branch(&mut self, branch: String, kind: BranchKind) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let ref_kind = match kind {
            BranchKind::Local => BrowseRefKind::LocalBranch,
            BranchKind::Remote => BrowseRefKind::RemoteBranch,
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Tree;
        self.main_mode = MainMode::Browse;
        self.status = format!("正在解析分支 {branch}");
        self.open_browse_resolve(repo_path, tab_id, branch, ref_kind);
    }

    /// 从侧边栏分支右键菜单进入比较模式。
    pub(crate) fn open_compare_branch(&mut self, branch: String, kind: BranchKind) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let ref_kind = match kind {
            BranchKind::Local => BrowseRefKind::LocalBranch,
            BranchKind::Remote => BrowseRefKind::RemoteBranch,
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Compare;
        self.browse.view_mode = BrowseViewMode::Diff;
        // 切换比较目标时作废旧评审。
        self.reset_ai_review_state();
        self.main_mode = MainMode::Browse;
        self.status = format!("正在准备分支比较 {branch}");
        self.open_browse_resolve(repo_path, tab_id, branch, ref_kind);
    }

    /// 从侧边栏标签右键菜单进入浏览模式。
    pub(crate) fn open_browse_tag(&mut self, tag: String) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        self.browse.reset();
        self.browse.list_mode = BrowseListMode::Tree;
        self.main_mode = MainMode::Browse;
        self.status = format!("正在解析标签 {tag}");
        self.open_browse_resolve(repo_path, tab_id, tag, BrowseRefKind::Tag);
    }

    /// 后台解析目标引用为 BrowseTarget。
    fn open_browse_resolve(
        &mut self,
        repo_path: PathBuf,
        tab_id: RepoTabId,
        name: String,
        ref_kind: BrowseRefKind,
    ) {
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let target = service.resolve_browse_target(&repo, &name, ref_kind)?;
                Ok(UiEvent::BrowseTargetResolved {
                    tab_id,
                    target,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 后台加载某个目录的文件树条目。
    pub(crate) fn load_browse_tree(&mut self, dir: PathBuf) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        let prefix = if dir.as_os_str().is_empty() {
            None
        } else {
            Some(dir.clone())
        };
        self.browse.loading_tree = true;
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let entries = service.browse_tree_entries(&repo, &commit_oid, prefix.as_deref())?;
                Ok(UiEvent::BrowseTreeLoaded {
                    tab_id,
                    dir_path: dir,
                    entries,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 后台加载目标分支与当前 HEAD 的差异文件列表。
    pub(crate) fn load_browse_compare_files(&mut self) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        self.browse.compare_loading = true;
        self.browse.compare_files.clear();
        // 重新加载差异时清空展开状态，让新比较默认全部展开。
        self.browse.compare_expanded.clear();
        self.browse.selected_file = None;
        self.browse.selected_compare_file = None;
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.status = "正在加载分支差异".to_string();

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let files = service.browse_compare_files(&repo, &commit_oid)?;
                Ok(UiEvent::BrowseCompareFilesLoaded {
                    tab_id,
                    target_oid: commit_oid,
                    files,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 展开/折叠比较差异文件树中的目录。
    /// 第一次操作时把默认全展开固化成显式集合，再增删目标目录，
    /// 避免其它目录意外折叠。
    pub(crate) fn toggle_compare_dir(&mut self, dir: String) {
        if self.browse.compare_expanded.is_empty() {
            self.browse.compare_expanded =
                browse_compare_view::all_compare_dirs(&self.browse.compare_files);
        }
        if self.browse.compare_expanded.contains(&dir) {
            self.browse.compare_expanded.remove(&dir);
        } else {
            self.browse.compare_expanded.insert(dir);
        }
    }

    /// 展开/折叠目录；展开时按需懒加载子树。
    pub(crate) fn toggle_browse_dir(&mut self, path: PathBuf) {
        let already_loaded = self
            .browse
            .entries_by_dir
            .contains_key(&BrowseState::dir_key(&path));
        if self.browse.expanded.contains(&path) {
            self.browse.expanded.remove(&path);
        } else {
            self.browse.expanded.insert(path.clone());
            if !already_loaded {
                self.load_browse_tree(path);
            }
        }
    }

    /// 选中文件树文件并按当前模式加载内容或差异。
    pub(crate) fn select_browse_file(&mut self, path: PathBuf) {
        if self.browse.selected_file.as_ref() == Some(&path)
            && (self.browse.content.is_some() || self.browse.diff.is_some())
        {
            return;
        }
        self.browse.selected_file = Some(path.clone());
        self.browse.selected_compare_file = None;
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    /// 选中比较模式中的差异文件，并保留旧路径/状态供 diff 与全文视图判断。
    pub(crate) fn select_browse_compare_file(&mut self, file: BrowseCompareFile) {
        let path = PathBuf::from(&file.path);
        if self.browse.selected_file.as_ref() == Some(&path)
            && self.browse.selected_compare_file.as_ref() == Some(&file)
            && (self.browse.content.is_some() || self.browse.diff.is_some())
        {
            return;
        }
        self.browse.selected_file = Some(path);
        self.browse.selected_compare_file = Some(file);
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    fn clear_browse_current_views(&mut self) {
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.clear_browse_selection();
        self.reset_uniform_scroll("browse-content-scroll");
        self.reset_uniform_scroll("browse-diff-scroll");
    }

    /// 切换内容/差异视图模式，并按需重新加载。
    pub(crate) fn set_browse_view_mode(&mut self, mode: BrowseViewMode) {
        if self.browse.view_mode == mode {
            return;
        }
        self.browse.view_mode = mode;
        self.clear_browse_current_views();
        self.load_browse_current();
    }

    /// 根据当前选中的文件和视图模式触发后台加载。
    fn load_browse_current(&mut self) {
        let Some(path) = self.browse.selected_file.clone() else {
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(target) = self.browse.target.clone() else {
            return;
        };
        let commit_oid = target.commit_oid.clone();
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let mode = self.browse.view_mode;
        let compare_file = self.browse.selected_compare_file.clone();
        let old_path = compare_file
            .as_ref()
            .and_then(|file| file.old_path.as_ref())
            .map(PathBuf::from);

        if mode == BrowseViewMode::Content
            && compare_file
                .as_ref()
                .is_some_and(|file| file.status == ChangeState::Deleted)
        {
            self.browse.loading_content = false;
            self.status = "目标分支中不存在该文件".to_string();
            return;
        }

        match mode {
            BrowseViewMode::Content => {
                self.browse.loading_content = true;
                self.status = "正在加载文件内容".to_string();
            }
            BrowseViewMode::Diff => {
                self.browse.loading_diff = true;
                self.status = "正在加载文件差异".to_string();
            }
        }

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                match mode {
                    BrowseViewMode::Content => {
                        let content =
                            service.browse_file_content(&repo, &commit_oid, &path, encoding)?;
                        Ok(UiEvent::BrowseFileContentLoaded {
                            tab_id,
                            path: path.to_string_lossy().to_string(),
                            content,
                            load_id,
                        })
                    }
                    BrowseViewMode::Diff => {
                        let diff = service.browse_file_diff_for_compare(
                            &repo,
                            &commit_oid,
                            &path,
                            old_path.as_deref(),
                            full_context,
                            encoding,
                        )?;
                        Ok(UiEvent::BrowseFileDiffLoaded {
                            tab_id,
                            path: path.to_string_lossy().to_string(),
                            diff,
                            load_id,
                        })
                    }
                }
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id: Some(tab_id),
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 关闭浏览模式，回到工作区。
    pub(crate) fn close_browse(&mut self) {
        self.main_mode = MainMode::Worktree;
        // 评审针对具体比较目标，退出浏览时整体作废，避免下次进入比较模式
        // 时残留旧评审直接占满右侧区域。
        self.reset_ai_review_state();
        self.status = "已退出分支浏览".to_string();
    }

    /// 分离当前评审展示（**不终止在途任务**）：切目标/退出浏览时调用。
    /// 后台任务继续执行，完成后落盘并 toast 提示；其后续事件因代际
    /// 不匹配被丢弃，不会污染新目标的展示状态。
    fn reset_ai_review_state(&mut self) {
        self.ai_review = None;
        self.ai_review_steps.clear();
        self.ai_review_step_expanded.clear();
        self.ai_review_progress = None;
        self.ai_review_live_reasoning.clear();
        self.ai_review_live_content.clear();
        self.ai_review_expanded = false;
        self.ai_review_loaded_label = None;
        self.ai_review_active_generation = None;
        self.ai_review_loading = false;
        // 取消标志不动：任务继续后台执行直到结束落盘。
    }

    /// 取消当前附着的评审任务：置位取消标志并分离展示；任务在轮次
    /// 边界自行退出（不落盘、不提示失败、不占并发名额的时间超过必要）。
    pub(crate) fn cancel_ai_review(&mut self) {
        if let Some(flag) = self.ai_review_cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.reset_ai_review_state();
        self.status = "已取消 AI 评审".into();
    }

    /// 打开评审历史弹窗并后台加载当前仓库的最近记录。
    pub(crate) fn open_ai_review_history(&mut self) {
        if self.ai_review_history.is_some() {
            return;
        }
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.last_error = Some("无法定位数据目录".into());
            return;
        };
        self.ai_review_history = Some(AiReviewHistoryState {
            loading: true,
            records: Vec::new(),
            error: None,
        });
        self.status = "正在加载评审记录".into();
        let tx = self.tx.clone();
        self.tasks.spawn(crate::TaskKind::Short, move || {
            let repo_path_string = repo_path.display().to_string();
            let result = khaslana::ai::list_review_records(
                &data_dir,
                &repo_path_string,
                AI_REVIEW_HISTORY_LIMIT,
            );
            match result {
                Ok(records) => {
                    crate::send_ui_event(&tx, crate::UiEvent::AiReviewHistoryLoaded { records })
                }
                Err(err) => crate::send_ui_event(
                    &tx,
                    crate::UiEvent::AiReviewHistoryLoadFailed {
                        error: err.to_string(),
                    },
                ),
            }
        });
    }

    /// 关闭评审历史弹窗。
    pub(crate) fn close_ai_review_history(&mut self) {
        self.ai_review_history = None;
    }

    /// 把一条历史记录载入评审面板（若有生成中的附着任务先分离，其继续
    /// 后台执行并落盘）。
    pub(crate) fn open_ai_review_record(&mut self, record: AiReviewRecord) {
        self.close_ai_review_history();
        self.reset_ai_review_state();
        let date =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(record.created_at_millis as i64)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "时间未知".to_string());
        self.ai_review_loaded_label =
            Some(format!("历史 · {date} · {}", record.target_display_name));
        self.ai_review = Some(Arc::new(record.result.clone()));
        self.ai_review_steps = record.result.steps;
        self.ai_review_expanded = true;
        self.status = "已打开历史评审记录".into();
        self.last_error = None;
    }

    /// 对比视图依赖具体分支的差异；切换分支/检出后旧差异会失效，
    /// 命中对比视图时关闭它，回到工作区，让新 HEAD 的状态正常展示。
    fn close_browse_if_comparing(&mut self) {
        if self.main_mode == MainMode::Browse && self.browse.list_mode == BrowseListMode::Compare {
            self.close_browse();
        }
        // 追溯视图同样基于 HEAD：检出后内容失效，一并关闭。
        if self.main_mode == MainMode::Blame {
            self.close_blame();
        }
    }

    // ===== 文件追溯（blame）视图 =====

    /// 打开某文件的追溯视图：置状态、切主模式并后台加载。
    ///
    /// 历史页提交文件右键入口对 HEAD 版本追溯（v1 不支持对任意提交 blame，
    /// `BlameOptions::newest_commit` 留作后续）；工作区入口同时纳入未提交
    /// 改动（服务端经 blame_buffer 处理）。
    pub(crate) fn open_blame_file(&mut self, path: String) {
        self.close_popups();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let load_id = self.repository_load_id;
        self.blame.reset();
        self.blame.path = Some(path.clone());
        self.blame.loading = true;
        self.main_mode = MainMode::Blame;
        self.status = format!("正在加载文件追溯：{path}");
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(&repo_path)?;
                let view = service.blame_file(&repo, Path::new(&path), encoding)?;
                Ok(UiEvent::BlameLoaded {
                    tab_id,
                    path: path.clone(),
                    view,
                    load_id,
                })
            })();
            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::BlameLoadFailed {
                        tab_id,
                        path,
                        error: err.to_string(),
                        load_id,
                    },
                ),
            }
        });
    }

    /// 关闭追溯视图，回到工作区（仿浏览模式）。
    pub(crate) fn close_blame(&mut self) {
        self.blame.reset();
        self.main_mode = MainMode::Worktree;
        self.status = "已退出文件追溯".to_string();
    }

    /// 编码切换时重新加载当前追溯文件。
    pub(crate) fn reload_blame_on_encoding_change(&mut self) {
        if self.main_mode != MainMode::Blame {
            return;
        }
        if let Some(path) = self.blame.path.clone() {
            self.open_blame_file(path);
        }
    }

    /// 右键菜单「查看文件历史」入口：设置历史页路径过滤并切换过去；
    /// 已在历史页时仅设置过滤器。
    pub(crate) fn view_file_history(&mut self, path: String) {
        self.close_popups();
        let in_history = self.main_mode == MainMode::History;
        self.set_history_file_filter(Some(path));
        if !in_history {
            self.set_main_mode(MainMode::History);
        }
    }

    // ===== 语法高亮调度（统一后台补算 + Arc 身份守卫） =====

    /// 为当前活动 tab 的某个槽位调度后台语法高亮。
    ///
    /// 调度只发生在「内容落位」之后（各加载事件处理分支 / 缓存命中路径），
    /// 计算是纯 CPU 任务（不碰 git），结果经 `SyntaxHighlighted` 回填；
    /// diff 槽位仅在全文模式且非二进制时调度（紧凑差异块不高亮）。
    pub(crate) fn schedule_syntax_highlight(&mut self, slot: SyntaxSlot) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let source = match slot {
            SyntaxSlot::WorktreeDiff => self.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::HistoryDiff => self.history_diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::StashDiff => self.stash_preview.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::BrowseDiff => self.browse.diff.clone().map(SyntaxSource::Diff),
            SyntaxSlot::Blame => self.blame.view.clone().map(SyntaxSource::Blame),
            SyntaxSlot::BrowseContent => self.browse.content.clone().map(SyntaxSource::Content),
        };
        let Some(source) = source else {
            return;
        };
        if let SyntaxSource::Diff(diff) = &source
            && (!self.full_file_view || diff.is_binary)
        {
            return;
        }
        if let SyntaxSource::Content(content) = &source
            && content.is_binary
        {
            return;
        }
        let (anchor, anchor_len) = source.anchor();
        let dark = ui::theme::active_variant().is_dark();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let spans = match &source {
                SyntaxSource::Diff(diff) => khaslana::syntax::highlight_diff_lines(diff, dark),
                SyntaxSource::Blame(view) => {
                    khaslana::syntax::highlight(&view.path, &view.lines, dark)
                }
                SyntaxSource::Content(content) => {
                    khaslana::syntax::highlight(&content.path, &content.lines, dark)
                }
            };
            send_ui_event(
                &tx,
                UiEvent::SyntaxHighlighted {
                    tab_id,
                    slot,
                    anchor,
                    anchor_len,
                    spans: spans.map(Arc::new),
                },
            );
        });
    }

    /// 为当前选中的冲突文件调度指定分栏的语法高亮。
    ///
    /// 只计算选中文件（渲染也只看选中文件）；draft 每次调度递增 seq，
    /// 晚到的旧结果在回填时被丢弃。二进制冲突文件无文本栏，直接跳过。
    fn schedule_conflict_syntax_for_selected(&mut self, panes: &[ConflictSyntaxPane]) {
        let panes: Vec<ConflictSyntaxPane> = panes.to_vec();
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        let (ours, theirs, draft) = match self.conflict_workbench.files.get(&path) {
            Some(view) if view.kind == ConflictFileKind::Text => (
                view.ours_text.clone(),
                view.theirs_text.clone(),
                view.draft.clone(),
            ),
            _ => return,
        };
        let dark = ui::theme::active_variant().is_dark();
        for pane in panes {
            let (text, seq) = match pane {
                ConflictSyntaxPane::Ours => (ours.clone(), 0),
                ConflictSyntaxPane::Theirs => (theirs.clone(), 0),
                ConflictSyntaxPane::Draft => {
                    let entry = self
                        .conflict_workbench
                        .syntax
                        .entry(path.clone())
                        .or_default();
                    entry.draft_seq += 1;
                    (draft.clone(), entry.draft_seq)
                }
            };
            let task_path = path.clone();
            let tx = self.tx.clone();
            self.tasks.spawn(TaskKind::Short, move || {
                let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
                let spans = khaslana::syntax::highlight(&task_path, &lines, dark);
                send_ui_event(
                    &tx,
                    UiEvent::ConflictSyntaxHighlighted {
                        tab_id,
                        path: task_path,
                        pane,
                        seq,
                        spans: spans.map(Arc::new),
                    },
                );
            });
        }
    }

    /// 主题深浅切换后：清空全部语法高亮并按新变体重新调度。
    ///
    /// 只是从各槽位现存的 Arc/文本补算，不做任何 git 重载；
    /// 含非活动 tab（调度是读 Arc 的轻量任务）。
    fn invalidate_and_refresh_syntax_highlights(&mut self) {
        let slots_by_tab: Vec<(RepoTabId, Vec<SyntaxSlot>)> = self
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.id,
                    vec![
                        SyntaxSlot::WorktreeDiff,
                        SyntaxSlot::HistoryDiff,
                        SyntaxSlot::StashDiff,
                        SyntaxSlot::BrowseDiff,
                        SyntaxSlot::Blame,
                        SyntaxSlot::BrowseContent,
                    ],
                )
            })
            .collect();
        for (tab_id, slots) in slots_by_tab {
            self.with_tab_context(tab_id, |this| {
                this.diff_syntax = None;
                this.history_diff_syntax = None;
                this.stash_preview.diff_syntax = None;
                this.browse.diff_syntax = None;
                this.browse.content_syntax = None;
                this.blame.syntax = None;
                this.conflict_workbench.syntax.clear();
                for slot in slots {
                    this.schedule_syntax_highlight(slot);
                }
                let panes = [
                    ConflictSyntaxPane::Ours,
                    ConflictSyntaxPane::Theirs,
                    ConflictSyntaxPane::Draft,
                ];
                this.schedule_conflict_syntax_for_selected(&panes);
            });
        }
    }

    /// 将鼠标 Y 坐标映射到内容行索引（基于 uniform_list 滚动偏移与行高）。
    fn browse_row_for_mouse_y(&self, y: Pixels, line_count: usize) -> usize {
        let scroll = self.uniform_scroll_handle("browse-content-scroll");
        let state = scroll.0.borrow();
        let bounds = state.base_handle.bounds();
        let offset_y = f32::from(state.base_handle.offset().y);
        let row = ((f32::from(y) - f32::from(bounds.top()) - offset_y)
            / crate::browse_view::BROWSE_ROW_HEIGHT)
            .floor()
            .max(0.0) as usize;
        row.min(line_count.saturating_sub(1))
    }

    /// 清空行级选区。
    pub(crate) fn clear_browse_selection(&mut self) {
        self.browse.selecting = false;
        self.browse.sel_start = None;
        self.browse.sel_end = None;
    }

    /// 编码切换时重新加载当前浏览文件。
    pub(crate) fn reload_browse_on_encoding_change(&mut self) {
        if self.main_mode != MainMode::Browse {
            return;
        }
        self.browse.content = None;
        self.browse.diff = None;
        self.browse.diff_headers_expanded = false;
        self.load_browse_current();
    }

    pub(crate) fn set_history_scope(&mut self, scope: HistoryScope) {
        if self.history_scope == scope {
            return;
        }
        self.history_scope = scope;
        // 过滤器是用户意图：切 scope 保留，仅显式清除（chip 的 ×）。
        self.clear_history();
        self.status = format!("提交记录范围已切换为{}", scope.label());
        self.load_history_page(false);
    }

    /// 设置/清除历史页的文件路径过滤（None 为清除）。
    ///
    /// 仿 `set_history_scope`：设字段 -> 清列表（`clear_history` 不清过滤器）
    /// -> 全量重载。切换分支、刷新等操作同样保留过滤器，per-tab 生命周期
    /// 随 tab 自然销毁。
    pub(crate) fn set_history_file_filter(&mut self, path: Option<String>) {
        if self.history_file_filter == path {
            return;
        }
        let label = path
            .as_deref()
            .map(|path| format!("提交记录已按文件 {path} 过滤"))
            .unwrap_or_else(|| "已清除文件过滤".to_string());
        self.history_file_filter = path;
        self.clear_history();
        self.status = label;
        self.load_history_page(false);
    }

    fn reload_history_if_active(&mut self) {
        if self.main_mode == MainMode::History {
            self.load_history_page(false);
        }
    }

    /// 历史可能已过时：正在查看历史页或已有历史列表时立即后台刷新，
    /// 不受当前视图限制；历史列表为空且不在历史页（用户从未看过历史）时
    /// 保持现状，等进入历史页时由 ensure_history_loaded 拉取，避免预加载。
    fn reload_history_after_change(&mut self) {
        if self.repo_path.is_some()
            && !self.history_loading.commits
            && (matches!(self.main_mode, MainMode::History | MainMode::CommitGraph)
                || !self.history_commits.is_empty())
        {
            self.load_history_page(false);
        }
    }

    fn ensure_history_loaded(&mut self) {
        if matches!(self.main_mode, MainMode::History | MainMode::CommitGraph)
            && self.repo_path.is_some()
            // 列表为空时首载；被标记陈旧（操作后未在后台刷新）时重新拉取
            && (self.history_commits.is_empty() || self.history_refreshing)
            && !self.history_loading.commits
        {
            self.load_history_page(false);
        }
    }

    fn load_history_page(&mut self, append: bool) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        if self.history_loading.commits {
            return;
        }

        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let scope = self.history_scope;
        // 文件路径过滤：非空时分派到 file_history（只返回改动过该文件的提交）
        let path_filter = self.history_file_filter.clone();
        // 仅分页（append）复用 refs 缓存；全量刷新传 None 重建，
        // 保证切换分支、提交等操作后 HEAD/分支/标签徽章与最新仓库状态一致。
        let refs_cache = if append {
            self.history_refs_cache.clone()
        } else {
            None
        };
        let offset = if append {
            self.history_commits.len()
        } else {
            0
        };
        let load_id = self.repository_load_id;
        self.history_load_seq += 1;
        let seq = self.history_load_seq;
        self.history_loading.commits = true;
        self.status = if append {
            "正在加载更多提交记录".to_string()
        } else {
            "正在加载提交记录".to_string()
        };
        self.last_error = None;

        // 图谱页高亮激活时随全量历史刷新同步重算谱系：提交/拉取/切换分支等
        // 操作可能移动分支 tip，旧 OID 集合会静默失真。append 分页不改 tip，
        // 无需重算。
        if !append && self.commit_graph.highlight_branch.is_some() {
            self.refresh_commit_graph_trace();
        }

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let (mut commits, refs_cache) = match path_filter.as_deref() {
                    Some(path) => service.file_history(
                        &repo,
                        scope,
                        path,
                        offset,
                        HISTORY_PAGE_SIZE + 1,
                        refs_cache.as_ref(),
                    )?,
                    None => service.commit_history_with_refs(
                        &repo,
                        scope,
                        offset,
                        HISTORY_PAGE_SIZE + 1,
                        refs_cache.as_ref(),
                    )?,
                };
                let has_more = commits.len() > HISTORY_PAGE_SIZE;
                commits.truncate(HISTORY_PAGE_SIZE);
                perf_log(
                    "history.commits",
                    started,
                    format!(
                        "tab={} scope={} append={} offset={} commits={} has_more={} path_filter={:?}",
                        tab_id.0,
                        scope.label(),
                        append,
                        offset,
                        commits.len(),
                        has_more,
                        path_filter
                    ),
                );
                Ok(UiEvent::HistoryCommitsLoaded {
                    tab_id,
                    commits,
                    refs_cache,
                    append,
                    has_more,
                    scope,
                    path_filter,
                    load_id,
                    seq,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn load_more_history(&mut self) {
        if !self.history_has_more {
            return;
        }
        self.load_history_page(true);
    }

    pub(crate) fn select_history_commit(&mut self, oid: String) {
        if self.history_selected_commit.as_deref() == Some(oid.as_str())
            && !self.history_files.is_empty()
        {
            return;
        }

        self.history_selected_commit = Some(oid.clone());
        self.history_files.clear();
        self.history_selected_file = None;
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
        self.reset_uniform_scroll("history-diff-scroll");
        self.history_loading.diff = false;

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        // 加载标志与状态文字在守卫之后设置：守卫早退时不应挂起
        // “提交文件加载中...”（文件区会永久显示加载占位）。
        self.history_loading.files = true;
        self.status = "正在加载提交文件".to_string();
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let files = service.commit_files(&repo, &oid)?;
                perf_log(
                    "history.files",
                    started,
                    format!("tab={} files={}", tab_id.0, files.len()),
                );
                Ok(UiEvent::HistoryFilesLoaded {
                    tab_id,
                    commit_oid: oid,
                    files,
                    load_id,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn select_history_file(&mut self, path: String) {
        self.select_history_file_with_reload(path, false);
    }

    pub(crate) fn select_history_file_with_reload(&mut self, path: String, force_reload: bool) {
        let Some(commit_oid) = self.history_selected_commit.clone() else {
            return;
        };
        if !force_reload
            && self.history_selected_file.as_deref() == Some(path.as_str())
            && self.history_diff.is_some()
        {
            return;
        }

        self.history_selected_file = Some(path.clone());
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
        self.reset_uniform_scroll("history-diff-scroll");
        self.history_loading.diff = true;
        self.status = "正在加载提交差异".to_string();

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let cache_key = self.diff_cache_key(
            DiffCacheKind::History {
                commit_oid: commit_oid.clone(),
                path: path.clone(),
            },
            &repo_path,
        );
        if !force_reload && let Some(diff) = self.cached_diff(&cache_key) {
            self.history_loading.diff = false;
            self.history_diff_syntax = None;
            self.history_diff = Some(diff);
            self.history_diff_headers_expanded = false;
            self.status = "提交差异已加载".to_string();
            // 缓存命中不走事件落位，语法高亮在此手动调度
            self.schedule_syntax_highlight(SyntaxSlot::HistoryDiff);
            return;
        }
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let diff = service.commit_file_diff(
                    &repo,
                    &commit_oid,
                    Path::new(&path),
                    full_context,
                    encoding,
                )?;
                perf_log(
                    "history.diff",
                    started,
                    format!("tab={} lines={}", tab_id.0, diff.lines.len()),
                );
                Ok(UiEvent::HistoryDiffLoaded {
                    tab_id,
                    commit_oid,
                    path,
                    diff,
                    load_id,
                })
            })();

            match result {
                Ok(event) => {
                    send_ui_event(&tx, event);
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::HistoryLoadFailed {
                            tab_id,
                            error: err.to_string(),
                            load_id,
                        },
                    );
                }
            }
        });
    }

    fn stage_selected(&mut self) {
        let paths = self.selected_change_paths(DiffScope::Unstaged);
        if paths.is_empty() {
            self.last_error = Some("请先在修改区选择文件".into());
            return;
        }
        self.stage_paths(paths, "已暂存选定文件");
    }

    fn stage_all(&mut self) {
        let paths = self.change_paths(DiffScope::Unstaged);
        if paths.is_empty() {
            self.last_error = Some("修改区没有可暂存文件".into());
            return;
        }
        self.stage_paths(paths, "已暂存所有文件");
    }

    fn stage_paths(&mut self, paths: Vec<String>, label: &'static str) {
        self.with_repo(label, move |service, repo| {
            let path_bufs = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            service.stage_paths(repo, path_bufs.iter().map(|path| path.as_path()))
        });
    }

    fn unstage_selected(&mut self) {
        let paths = self.selected_change_paths(DiffScope::Staged);
        if paths.is_empty() {
            self.last_error = Some("请先在暂存区选择文件".into());
            return;
        }
        self.unstage_paths(paths, "已取消暂存选定文件");
    }

    fn unstage_all(&mut self) {
        let paths = self.change_paths(DiffScope::Staged);
        if paths.is_empty() {
            self.last_error = Some("暂存区没有可取消暂存文件".into());
            return;
        }
        self.unstage_paths(paths, "已取消暂存所有文件");
    }

    fn unstage_paths(&mut self, paths: Vec<String>, label: &'static str) {
        self.with_repo(label, move |service, repo| {
            let path_bufs = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            service.unstage_paths(repo, path_bufs.iter().map(|path| path.as_path()))
        });
    }

    fn commit(&mut self) {
        if self.merge_in_progress() {
            self.finish_merge();
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        if message.is_empty() {
            self.last_error = Some("需要填写提交信息".into());
            return;
        }
        self.commit_message.clear();
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.with_repo_blocking("提交完成", move |service, repo| {
            service.commit(repo, &CommitMessage::new(message))
        });
    }

    /// 修补最后一次提交：以当前暂存区为树重写 HEAD。
    /// 输入框为空时保留原提交信息（只补文件不改信息的场景）。
    fn amend(&mut self) {
        if !self.ensure_no_merge_in_progress("修补提交") {
            return;
        }
        if self.amend_needs_push_warning() {
            self.open_amend_pushed_confirm_dialog(false);
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        self.perform_amend(message);
    }

    /// 修补最后一次提交并推送当前分支。
    fn amend_and_push(&mut self) {
        if !self.ensure_no_merge_in_progress("修补提交并推送") {
            return;
        }
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        if self.amend_needs_push_warning() {
            self.open_amend_pushed_confirm_dialog(true);
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        self.perform_amend_and_push(message, remote);
    }

    fn perform_amend(&mut self, message: String) {
        let message = (!message.is_empty()).then(|| CommitMessage::new(message));
        self.commit_message.clear();
        self.amend_mode = false;
        self.amend_prefill = None;
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.with_repo_blocking("修补提交完成", move |service, repo| {
            service.amend_commit(repo, message.as_ref())
        });
    }

    /// 修补后推送：组合错误处理与 commit_and_push 一致——修补成功但推送
    /// 失败时保留修补结果并给出组合提示。
    fn perform_amend_and_push(&mut self, message: String, remote: String) {
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let message = (!message.is_empty()).then(|| CommitMessage::new(message));
        self.commit_message.clear();
        self.amend_mode = false;
        self.amend_prefill = None;
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            "正在修补提交并推送",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(path)?;
                let snapshot = service.amend_commit(&mut repo, message.as_ref())?;
                match service.push(&mut repo, &RemoteName::new(remote)) {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: "修补提交并推送完成".to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err(err) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: format!("修补已完成，但推送失败：{err}"),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                }
            },
        );
    }

    /// 修补的 HEAD 是否已推送（据 branch_sync_status 判断，数据可能略陈旧：
    /// 误判为已推送只会多一次确认，安全方向）。无 upstream 视为未推送。
    /// 预填修补模式的 HEAD 提交信息：优先用内存中的历史数据（已加载过
    /// 提交记录时即时命中）；否则后台读取 HEAD（不依赖进入过历史页），
    /// 经 `AmendPrefillLoaded` 事件回填。
    fn prefill_amend_message(&mut self) {
        // 同步路径仅在历史数据确定新鲜时使用：刷新中（提交/推送等操作后
        // 的后台重载还在飞行）旧列表里的 HEAD 徽章已过时，会预填旧提交的
        // 信息，此时直接走后台读 HEAD 的兜底路径。
        if !self.history_refreshing
            && let Some(message) = self
                .history_commits
                .iter()
                .find(|commit| {
                    commit
                        .refs
                        .iter()
                        .any(|reference| reference.kind == khaslana::CommitRefKind::Head)
                })
                .map(|commit| commit.message.clone())
        {
            self.amend_prefill = Some(message.clone());
            self.commit_message.set_value(message);
            // caret 归零：预填信息通常从首行（主题）开始编辑，避免滚动到底。
            self.commit_message.caret = 0;
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let message = (|| -> khaslana::Result<Option<String>> {
                let repo = Repository::open(path)?;
                service.head_commit_message(&repo)
            })()
            // 仓库打开失败等视作无预填，不打断用户。
            .unwrap_or(None);
            send_ui_event(&tx, UiEvent::AmendPrefillLoaded { tab_id, message });
        });
    }

    fn amend_needs_push_warning(&self) -> bool {
        let Some(status) = self.branch_sync_status.as_ref() else {
            return false;
        };
        let Some(head) = self.snapshot.as_ref().map(|snapshot| snapshot.head.clone()) else {
            return false;
        };
        status.branch == head.unwrap_or_default() && status.upstream.is_some() && status.ahead == 0
    }

    fn open_amend_pushed_confirm_dialog(&mut self, and_push: bool) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmAmendPushed { and_push });
        self.last_error = None;
    }

    /// 拣选提交到当前分支（历史页右键菜单入口）。
    fn cherry_pick_commit(&mut self, oid: String) {
        if !self.ensure_no_merge_in_progress("拣选提交") {
            return;
        }
        // 工作区脏时提前拦截，避免后台任务失败后才反馈。
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.changes.is_empty())
        {
            self.last_error = Some("工作区有未提交修改，请先提交、暂存或丢弃后再拣选".into());
            return;
        }
        self.with_repo_blocking("拣选提交完成", move |service, repo| {
            service.cherry_pick_commit(repo, &oid)
        });
    }

    fn commit_and_push(&mut self) {
        if !self.ensure_no_merge_in_progress("提交并推送") {
            return;
        }
        let message = self.commit_message.value.trim().to_string();
        if message.is_empty() {
            self.last_error = Some("需要填写提交信息".into());
            return;
        }
        let Some(remote) = self.current_remote() else {
            self.last_error = Some("当前仓库没有远端".into());
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        self.commit_message.clear();
        self.scroll_handle("commit-message-input-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab_with_blocker(
            Some(tab_id),
            "正在提交并推送",
            OperationBlocker::Modal,
            move || {
                let mut repo = Repository::open(path)?;
                match service.commit_and_push(
                    &mut repo,
                    &CommitMessage::new(message),
                    &RemoteName::new(remote),
                )? {
                    Ok(snapshot) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: "提交并推送完成".to_string(),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                    Err((snapshot, err)) => Ok(UiEvent::OperationFinished {
                        tab_id: Some(tab_id),
                        message: format!("提交已完成，但推送失败：{err}"),
                        snapshot: Some(snapshot),
                        diff: None,
                    }),
                }
            },
        );
    }

    pub(crate) fn load_diff(&mut self, path: String, scope: DiffScope) {
        self.reset_uniform_scroll("diff-scroll");
        // 差异内容变化后行索引失效：清空按行选择。
        self.diff_line_selection.clear();
        self.diff_line_selection_anchor = None;
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let cache_key = self.diff_cache_key(
            DiffCacheKind::Worktree {
                scope: scope.clone(),
                path: path.clone(),
            },
            &repo_path,
        );
        if let Some(diff) = self.cached_diff(&cache_key) {
            self.diff = Some(diff);
            self.diff_headers_expanded = false;
            self.diff_syntax = None;
            self.status = "差异已加载".to_string();
            // 缓存命中不走事件落位，语法高亮在此手动调度
            self.schedule_syntax_highlight(SyntaxSlot::WorktreeDiff);
            return;
        }
        let is_conflicted_path = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.conflicts.iter().any(|conflict| conflict == &path));
        let service = self.service_for_tab(tab_id);
        self.spawn_operation_for_tab(Some(tab_id), "正在加载差异", move || {
            let started = Instant::now();
            let repo = Repository::open(repo_path)?;
            let diff = service
                .diff_for_path(&repo, Path::new(&path), scope, full_context, encoding)
                .map_err(|err| {
                    if is_conflicted_path {
                        khaslana::GitError::Message(
                            "该文件存在冲突，请选择版本或手动编辑后标记解决".into(),
                        )
                    } else {
                        err
                    }
                })?;
            perf_log(
                "worktree.diff",
                started,
                format!("tab={} lines={}", tab_id.0, diff.lines.len()),
            );
            Ok(UiEvent::OperationFinished {
                tab_id: Some(tab_id),
                message: "差异已加载".to_string(),
                snapshot: None,
                diff: Some(diff),
            })
        });
    }

    /// 暂存/取消暂存（整文件或按块/按行）完成后刷新差异面板：
    /// - 当前差异的 (path, scope) 仍有改动 → 原位重载，反映最新暂存状态；
    /// - 已失效（整文件被挪到对侧列表）→ 清空差异面板，避免残留旧内容
    ///   与失效的「暂存此块/取消暂存此块」按钮。
    /// 存在性按操作后的快照判定（见 `diff_scope_still_present`）。
    fn refresh_diff_after_stage_change(&mut self) {
        let Some(diff) = self.diff.clone() else {
            return;
        };
        let path = diff.path.clone();
        let scope = diff.scope.clone();
        let still_present = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| diff_scope_still_present(&snapshot.changes, &path, &scope));
        if still_present {
            self.load_diff(path, scope);
        } else {
            self.diff = None;
            self.diff_headers_expanded = false;
            self.diff_line_selection.clear();
            self.diff_line_selection_anchor = None;
            self.reset_uniform_scroll("diff-scroll");
        }
    }

    // ── 按行/按块部分暂存（工作区差异视图）─────────────────────────

    /// 切换差异行的选中态。与变更列表同一套语义：普通点击单选（再点取消）、
    /// Ctrl/Cmd 多选、Shift 从锚点范围选择（替换现有选择）。
    fn toggle_diff_line_selection(&mut self, index: usize, multi: bool, shift: bool) {
        // 两个字段经 Deref 落在 RepoTabState 上，需先取出同一可变引用。
        let state = self.active_tab_state_mut();
        toggle_index_selection(
            &mut state.diff_line_selection,
            &mut state.diff_line_selection_anchor,
            index,
            multi,
            shift,
        );
    }

    /// 把选中的 diff 行索引转换为服务层的行号选择
    ///（Added 用 new_lineno、Removed 用 old_lineno；上下文行忽略）。
    fn diff_line_indices_to_selection(
        &self,
        indices: impl Iterator<Item = usize>,
    ) -> Option<LineSelection> {
        let diff = self.diff.as_ref()?;
        let mut selection = LineSelection::new();
        for index in indices {
            let Some(line) = diff.lines.get(index) else {
                continue;
            };
            match line.kind {
                DiffLineKind::Added => {
                    if let Some(lineno) = line.new_lineno {
                        selection.insert(SelectedDiffLine {
                            side: SelectionSide::Added,
                            lineno,
                        });
                    }
                }
                DiffLineKind::Removed => {
                    if let Some(lineno) = line.old_lineno {
                        selection.insert(SelectedDiffLine {
                            side: SelectionSide::Removed,
                            lineno,
                        });
                    }
                }
                _ => {}
            }
        }
        (!selection.is_empty()).then_some(selection)
    }

    /// 当前选中行对应的部分暂存选择。
    fn selected_diff_lines_selection(&self) -> Option<LineSelection> {
        let indices = self.diff_line_selection.iter().copied().collect::<Vec<_>>();
        self.diff_line_indices_to_selection(indices.into_iter())
    }

    /// 指定 hunk 的全部 +/- 行对应的部分暂存选择。
    fn diff_hunk_selection(&self, hunk_index: usize) -> Option<LineSelection> {
        let diff = self.diff.as_ref()?;
        let indices = diff
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.hunk_index == hunk_index)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        self.diff_line_indices_to_selection(indices.into_iter())
    }

    /// 执行部分暂存/取消暂存：按当前差异的 scope 决定方向
    ///（未暂存 → 暂存选中改动；已暂存 → 取消暂存选中改动）。
    fn apply_partial_stage(&mut self, selection: LineSelection) {
        let Some(diff) = self.diff.clone() else {
            return;
        };
        if diff.is_binary {
            self.last_error = Some("二进制文件不支持部分暂存".into());
            return;
        }
        let path = diff.path.clone();
        let is_stage = diff.scope == DiffScope::Unstaged;
        let label: &'static str = if is_stage {
            "已暂存选中改动"
        } else {
            "已取消暂存选中改动"
        };
        // 选区随差异刷新清空（load_diff 开头统一处理，这里立即清掉按钮态）。
        self.diff_line_selection.clear();
        self.diff_line_selection_anchor = None;
        self.with_repo(label, move |service, repo| {
            if is_stage {
                service.stage_lines(repo, Path::new(&path), &selection)
            } else {
                service.unstage_lines(repo, Path::new(&path), &selection)
            }
        });
    }

    /// 工具栏按钮：暂存/取消暂存当前选中的行。
    fn apply_selected_partial_stage(&mut self) {
        let Some(selection) = self.selected_diff_lines_selection() else {
            self.last_error = Some("请先点击差异中的 +/- 行选择要暂存的改动".into());
            return;
        };
        self.apply_partial_stage(selection);
    }

    /// hunk 头按钮：暂存/取消暂存整块。
    fn apply_hunk_partial_stage(&mut self, hunk_index: usize) {
        let Some(selection) = self.diff_hunk_selection(hunk_index) else {
            self.last_error = Some("该块没有可暂存的改动".into());
            return;
        };
        self.apply_partial_stage(selection);
    }

    fn use_credentials(&mut self) {
        let Some(pending) = self.pending_credential.clone() else {
            return;
        };

        let username = self
            .credential_username
            .value
            .trim()
            .to_string()
            .if_empty_then(|| {
                pending
                    .request
                    .username_from_url
                    .clone()
                    .unwrap_or_else(|| "git".into())
            });
        let secret = self.credential_secret.value.clone();
        let key_path = self.credential_key_path.value.trim().to_string();
        let passphrase = self.credential_passphrase.value.clone();
        let display_name = self
            .save_credential
            .then(|| optional_display_name(&self.credential_display_name.value))
            .flatten();

        let credential = if self.credential_form_mode == CredentialFormMode::Ssh {
            GitCredential::SshPassphrase {
                username,
                private_key_path: (!self.credential_use_ssh_agent && !key_path.is_empty())
                    .then_some(key_path),
                passphrase: (!passphrase.is_empty()).then_some(passphrase),
                display_name,
                save_to_keyring: self.save_credential,
                scope: self.credential_scope,
            }
        } else {
            GitCredential::UserPass {
                username,
                secret,
                display_name,
                save_to_keyring: self.save_credential,
                scope: self.credential_scope,
            }
        };

        if !send_credential_response(&pending, Ok(Some(credential))) {
            self.last_error = Some("凭据请求已失效".into());
            return;
        }
        self.show_next_credential_request();
        self.apply_status_event(pending.tab_id, |this| {
            this.status = "凭据已提交，正在继续操作".into();
            this.last_error = None;
        });
        self.reload_credential_records("凭据已提交");
        self.save_remote_credential_bindings();
    }

    fn cancel_credential_request(&mut self) {
        let Some(pending) = self.pending_credential.clone() else {
            return;
        };
        let _ = send_credential_response(
            &pending,
            Err(khaslana::GitError::Credential("已取消凭据输入".into())),
        );
        self.show_next_credential_request();
        self.apply_status_event(pending.tab_id, |this| {
            this.status = "凭据输入已取消".into();
            this.last_error = None;
        });
    }

    fn spawn_operation_for_tab<F>(&mut self, tab_id: Option<RepoTabId>, started: &'static str, f: F)
    where
        F: FnOnce() -> khaslana::Result<UiEvent> + Send + 'static,
    {
        self.spawn_operation_for_tab_with_blocker(tab_id, started, OperationBlocker::None, f);
    }

    fn spawn_operation_for_tab_with_blocker<F>(
        &mut self,
        tab_id: Option<RepoTabId>,
        started: &'static str,
        blocker: OperationBlocker,
        f: F,
    ) where
        F: FnOnce() -> khaslana::Result<UiEvent> + Send + 'static,
    {
        if let Some(tab_id) = tab_id
            && self.tab(tab_id).is_none()
        {
            return;
        }
        let busy = tab_id
            .and_then(|id| self.tab(id).map(|tab| tab.busy))
            .unwrap_or(self.busy);
        if busy {
            self.apply_status_event(tab_id, |this| {
                this.last_error = Some("已有操作正在运行".into());
            });
            return;
        }
        self.close_popups();
        self.apply_status_event(tab_id, |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = blocker;
            this.operation_blocker_started = if blocker.blocks_interaction() {
                Some(Instant::now())
            } else {
                None
            };
            this.operation_kind = OperationKind::from_message(started);
            this.status = started.to_string();
            this.last_error = None;
        });
        let tx = self.tx.clone();
        send_ui_event(
            &tx,
            UiEvent::OperationStarted {
                tab_id,
                message: started.to_string(),
            },
        );
        self.tasks.spawn(TaskKind::Long, move || match f() {
            Ok(event) => {
                send_ui_event(&tx, event);
            }
            Err(err) => {
                send_ui_event(
                    &tx,
                    UiEvent::OperationFailed {
                        tab_id,
                        error: err.to_string(),
                    },
                );
            }
        });
    }

    fn credential_scope_button(
        &self,
        label: &'static str,
        scope: CredentialScope,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.credential_scope == scope;
        segmented_button(format!("credential-scope-{label}"), selected, enabled)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.credential_scope = scope;
                    cx.notify();
                }
            }))
            .child(label)
    }

    fn credential_kind_button(
        &self,
        label: &'static str,
        mode: CredentialFormMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.credential_form_mode == mode;
        segmented_button(format!("credential-kind-{label}"), selected, true)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.set_credential_form_mode(mode);
                cx.notify();
            }))
            .child(label)
    }

    pub(crate) fn toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .child(toggle_box(checked))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(label),
            )
    }

    pub(crate) fn input(
        &self,
        id: FieldId,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if id == FieldId::ConflictEditor {
            return self.conflict_editor_input(window, cx).into_any_element();
        }
        if Self::is_multiline_field(id) {
            return self.multi_line_input(id, window, cx).into_any_element();
        }
        self.single_line_input(id, compact, window, cx)
            .into_any_element()
    }

    fn is_multiline_field(id: FieldId) -> bool {
        matches!(
            id,
            FieldId::CommitMessage
                | FieldId::ConflictEditor
                | FieldId::TagMessage
                // 工作流模板 AI 功能需求描述（编辑器弹窗内多行输入）。
                | FieldId::WorkflowEditor(workflow_editor::WorkflowEditorFieldId::AiDescription)
        )
    }

    fn single_line_input(
        &self,
        id: FieldId,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let field = self.field(id);
        let focused = field.focus.is_focused(window);
        input_frame(
            format!("field-{id:?}"),
            focused,
            if compact {
                InputFrameSize::Compact
            } else {
                InputFrameSize::Regular
            },
        )
        .track_focus(&field.focus)
        .key_context("TextInput")
        .on_action(cx.listener(Self::text_backspace))
        .on_action(cx.listener(Self::text_delete))
        .on_action(cx.listener(Self::text_left))
        .on_action(cx.listener(Self::text_right))
        .on_action(cx.listener(Self::text_up))
        .on_action(cx.listener(Self::text_down))
        .on_action(cx.listener(Self::text_select_left))
        .on_action(cx.listener(Self::text_select_right))
        .on_action(cx.listener(Self::text_select_up))
        .on_action(cx.listener(Self::text_select_down))
        .on_action(cx.listener(Self::text_select_all))
        .on_action(cx.listener(Self::text_home))
        .on_action(cx.listener(Self::text_end))
        .on_action(cx.listener(Self::text_paste))
        .on_action(cx.listener(Self::text_copy))
        .on_action(cx.listener(Self::text_cut))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
            if this.active_operation_blocker_message().is_some()
                && !this.operation_blocker_allows_text_field(id)
            {
                cx.stop_propagation();
                return;
            }
            // 单行框 Enter 提交所在表单（用户确认保留的文本框内行为）。
            if event.keystroke.key.as_str() == "enter" {
                this.submit_focused_field(id);
                cx.stop_propagation();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.field(id).focus);
                let position = this.field(id).index_for_mouse_position(event.position);
                let field = this.field_mut(id);
                field.is_selecting = true;
                if event.modifiers.shift {
                    field.select_to(position);
                } else {
                    field.move_to(position);
                }
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.field_mut(id).is_selecting = false;
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.field_mut(id).is_selecting = false;
                cx.notify();
            }),
        )
        .on_mouse_move(
            cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if !this.field(id).is_selecting {
                    return;
                }
                let position = this.field(id).index_for_mouse_position(event.position);
                this.field_mut(id).select_to(position);
                cx.notify();
            }),
        )
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .child(SingleLineInputElement {
            field_id: id,
            entity: cx.entity(),
        })
    }

    fn multi_line_input(
        &self,
        id: FieldId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let field = self.field(id);
        let focused = field.focus.is_focused(window);
        // 溢出判定综合逻辑行数与上帧自动换行行数（长行换行后同样超高）。
        let multiline_overflows = multiline_input_should_scroll(id, &field.value)
            || field.last_wrapped_line_count > MULTILINE_MIN_LINES;
        input_frame(format!("field-{id:?}"), focused, InputFrameSize::Multiline)
            .track_focus(&field.focus)
            .key_context("TextInput")
            .on_action(cx.listener(Self::text_backspace))
            .on_action(cx.listener(Self::text_delete))
            .on_action(cx.listener(Self::text_left))
            .on_action(cx.listener(Self::text_right))
            .on_action(cx.listener(Self::text_select_left))
            .on_action(cx.listener(Self::text_select_right))
            .on_action(cx.listener(Self::text_select_all))
            .on_action(cx.listener(Self::text_home))
            .on_action(cx.listener(Self::text_end))
            .on_action(cx.listener(Self::text_paste))
            .on_action(cx.listener(Self::text_copy))
            .on_action(cx.listener(Self::text_cut))
            .on_action(cx.listener(Self::text_submit))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                if this.active_operation_blocker_message().is_some()
                    && !this.operation_blocker_allows_text_field(id)
                {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key.as_str() == "enter"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                {
                    this.field_mut(id).insert_text("\n", true);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.field(id).focus);
                    let position = this.field(id).index_for_mouse_position(event.position);
                    let field = this.field_mut(id);
                    field.is_selecting = true;
                    if event.modifiers.shift {
                        field.select_to(position);
                    } else {
                        field.move_to(position);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(id).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(id).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !this.field(id).is_selecting {
                        return;
                    }
                    let position = this.field(id).index_for_mouse_position(event.position);
                    this.field_mut(id).select_to(position);
                    cx.notify();
                }),
            )
            .px_2()
            .py_2()
            .overflow_hidden()
            .child({
                let handle = self.scroll_handle(multiline_scroll_handle_id(id));
                let scroll_id = if id == FieldId::ConflictEditor {
                    "conflict-editor-scroll"
                } else {
                    "commit-message-input-scroll"
                };
                let content = div()
                    .id(scroll_id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(MultiLineInputElement {
                        field_id: id,
                        entity: cx.entity(),
                    })
                    .into_any_element();
                let frame = scrollable_frame_when(
                    scroll_id,
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    multiline_overflows,
                    cx,
                );
                if id == FieldId::ConflictEditor {
                    // 冲突编辑器随冲突面板高度伸缩。
                    frame.into_any_element()
                } else {
                    // 提交信息框固定可视高度（约 MULTILINE_MIN_LINES 行），
                    // 内容超出后滚动，不再随内容无限撑高。
                    div()
                        .flex()
                        .flex_col()
                        .h(px(MULTILINE_LINE_HEIGHT * MULTILINE_MIN_LINES as f32))
                        .child(frame)
                        .into_any_element()
                }
            })
    }

    fn conflict_editor_input(&self, _window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = self.field(FieldId::ConflictEditor);
        let multiline_overflows =
            multiline_input_should_scroll(FieldId::ConflictEditor, &field.value);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .track_focus(&field.focus)
            .key_context("TextInput")
            .on_action(cx.listener(Self::text_backspace))
            .on_action(cx.listener(Self::text_delete))
            .on_action(cx.listener(Self::text_left))
            .on_action(cx.listener(Self::text_right))
            .on_action(cx.listener(Self::text_select_left))
            .on_action(cx.listener(Self::text_select_right))
            .on_action(cx.listener(Self::text_select_all))
            .on_action(cx.listener(Self::text_home))
            .on_action(cx.listener(Self::text_end))
            .on_action(cx.listener(Self::text_paste))
            .on_action(cx.listener(Self::text_copy))
            .on_action(cx.listener(Self::text_cut))
            .on_action(cx.listener(Self::text_submit))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                if this.active_operation_blocker_message().is_some()
                    && !this.operation_blocker_allows_text_field(FieldId::ConflictEditor)
                {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key.as_str() == "enter"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                {
                    this.field_mut(FieldId::ConflictEditor)
                        .insert_text("\n", true);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.field(FieldId::ConflictEditor).focus);
                    let position = this
                        .field(FieldId::ConflictEditor)
                        .index_for_mouse_position(event.position);
                    let field = this.field_mut(FieldId::ConflictEditor);
                    field.is_selecting = true;
                    if event.modifiers.shift {
                        field.select_to(position);
                    } else {
                        field.move_to(position);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(FieldId::ConflictEditor).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(FieldId::ConflictEditor).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !this.field(FieldId::ConflictEditor).is_selecting {
                        return;
                    }
                    let position = this
                        .field(FieldId::ConflictEditor)
                        .index_for_mouse_position(event.position);
                    this.field_mut(FieldId::ConflictEditor).select_to(position);
                    cx.notify();
                }),
            )
            .p_2()
            .child({
                let handle = self.scroll_handle(CONFLICT_RESULT_SCROLL_HANDLE_ID);
                let content = div()
                    .id("conflict-editor-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(MultiLineInputElement {
                        field_id: FieldId::ConflictEditor,
                        entity: cx.entity(),
                    })
                    .into_any_element();
                scrollable_frame_when(
                    "conflict-editor-scroll",
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    multiline_overflows,
                    cx,
                )
            })
    }

    /// 仓库切换下拉触发器按钮：显示当前仓库头像 + 名称 + ▾。
    fn render_repo_switcher_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let name = self.display_name();
        // 按钮内名称截断后，悬浮仍可确认仓库完整路径。
        let repo_tooltip = self
            .repo_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| name.clone());
        let enabled = !self.busy;
        let text_color = if enabled {
            ui_theme::FOREGROUND
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        div()
            .id("repo-switcher-trigger")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .ml(px(12.0))
            .rounded(px(ui_theme::RADIUS_XS))
            // 纯鼠标触发器：不可聚焦、无键盘激活（键盘白名单见 AGENTS.md §8）。
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed())
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.5))
            .text_color(rgb(text_color))
            .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                if this.busy {
                    return;
                }
                // 已展开时点击按钮应关闭；close_popups 会清掉菜单，故先记录原状态，
                // 仅在原本未展开时才重新打开，避免“点按钮关不掉”。
                let was_open = this.repo_switcher_menu.is_some();
                this.close_popups();
                if !was_open {
                    this.toggle_repo_switcher(window);
                }
                cx.notify();
            }))
            .child(repo_avatar(&name))
            .child(
                div()
                    .id("repo-switcher-name")
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .max_w(px(120.0))
                    .min_w(px(0.0))
                    .truncate()
                    .tooltip(move |_window, cx| tooltip_text(repo_tooltip.clone(), cx))
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("▾"),
            )
            // paint 时记录按钮的窗口坐标矩形，供下拉菜单锚定与“点击外部关闭”命中判定。
            // 纯记录、不注册鼠标事件，故不拦截按钮自身的点击；不 notify，避免重渲染循环。
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, _window, cx| {
                        entity.update(cx, |this, _cx| {
                            this.repo_switcher_anchor = Some(RepoSwitcherAnchor {
                                x: bounds.origin.x.into(),
                                y: bounds.origin.y.into(),
                                w: bounds.size.width.into(),
                                h: bounds.size.height.into(),
                            });
                        });
                    },
                )
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0)),
            )
    }

    /// 仓库切换下拉 overlay：IDEA 式三区结构（功能 / 打开项目 / 最近项目）。
    /// 组装并按当前搜索词过滤仓库切换下拉的分区数据（渲染与键盘导航共用）。
    fn repo_switcher_filtered_sections(&self) -> RepoSwitcherSections {
        let active_key = self
            .active_tab
            .and_then(|id| self.tab(id))
            .and_then(|tab| tab.path_key());

        let tabs: Vec<RepoSwitcherTabInput> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let path = tab.repo_path.as_ref()?;
                Some(RepoSwitcherTabInput {
                    key: normalize_repo_path(path),
                    name: tab.display_name(),
                    full_path: path.to_string_lossy().to_string(),
                    last_active: tab.last_active_at,
                    tab_id: tab.id,
                })
            })
            .collect();

        let recent: Vec<RepoSwitcherRecentInput> = self
            .repo_switcher_recent
            .iter()
            .map(|(path, ts)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                RepoSwitcherRecentInput {
                    key: normalize_repo_path(path),
                    name,
                    full_path: path.to_string_lossy().to_string(),
                    last_opened: *ts,
                }
            })
            .collect();

        let sections = build_repo_switcher_sections(active_key.as_deref(), tabs, recent);
        filter_repo_switcher_sections(sections, self.repo_switcher_search.value.as_str())
    }

    fn render_repo_switcher_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(menu) = self.repo_switcher_menu.as_ref() else {
            return div().into_any_element();
        };
        let menu = menu.clone();

        let query_active = !self.repo_switcher_search.value.trim().is_empty();
        let sections = self.repo_switcher_filtered_sections();

        // 下拉内容区的滚动句柄，内容超出最大高度时滚动并绘制滚动条。
        let switcher_handle = self.scroll_handle("repo-switcher-scroll");
        let switcher_content = div()
            .id("repo-switcher-scroll")
            .flex()
            .flex_col()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&switcher_handle)
            // ── 功能区：克隆 / 打开 / 搜索仓库 ──
            .child(self.repo_switcher_action_item(
                "repo-switcher-clone",
                ToolbarIcon::Clone,
                "克隆仓库…",
                |this, window, _cx| {
                    this.close_repo_switcher();
                    this.open_clone_dialog(window);
                },
                cx,
            ))
            .child(self.repo_switcher_action_item(
                "repo-switcher-open",
                ToolbarIcon::Open,
                "打开仓库…",
                |this, _window, _cx| {
                    this.close_repo_switcher();
                    this.browse_open();
                },
                cx,
            ))
            // 搜索仓库：默认为按钮，点击展开输入框 + 小叉
            .when(!self.repo_switcher_search_open, |this| {
                this.child(self.repo_switcher_action_item(
                    "repo-switcher-search-toggle",
                    ToolbarIcon::Search,
                    "搜索仓库",
                    |this, window, _cx| {
                        this.repo_switcher_search_open = true;
                        window.focus(&this.repo_switcher_search.focus);
                    },
                    cx,
                ))
            })
            .when(self.repo_switcher_search_open, |this| {
                this.child(
                    div()
                        .id("repo-switcher-search-row")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(div().flex_1().min_w(px(0.0)).child(self.input(
                            FieldId::RepoSwitcherSearch,
                            false,
                            window,
                            cx,
                        )))
                        .child(
                            div()
                                .id("repo-switcher-search-close")
                                .flex_none()
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(ui_theme::RADIUS_XS))
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    // 收起输入框，恢复「搜索仓库」按钮并取消过滤
                                    this.repo_switcher_search_open = false;
                                    this.repo_switcher_search.clear();
                                    cx.notify();
                                }))
                                .child("✕"),
                        ),
                )
            })
            // ── 打开项目区 ──
            .when(!sections.open.is_empty(), |this| {
                this.child(self.repo_switcher_section_header("打开项目"))
                    .children(sections.open.iter().map(|repo| {
                        self.repo_switcher_repo_item(repo.clone(), cx)
                            .into_any_element()
                    }))
            })
            // ── 最近项目区 ──
            .when(!sections.recent.is_empty(), |this| {
                this.child(self.repo_switcher_section_header("最近的项目"))
                    .children(sections.recent.iter().map(|repo| {
                        self.repo_switcher_repo_item(repo.clone(), cx)
                            .into_any_element()
                    }))
            })
            // ── 搜索无结果占位 ──
            .when(
                query_active && sections.open.is_empty() && sections.recent.is_empty(),
                |this| {
                    this.child(
                        div()
                            .px_3()
                            .py_4()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("没有匹配的仓库"),
                    )
                },
            )
            .into_any_element();

        // 外层仅做定位与最大高度约束，滚动与滚动条交给 scrollable_frame_when。
        glass_menu()
            .id("repo-switcher-menu")
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(REPO_SWITCHER_MENU_WIDTH))
            .max_h(px(REPO_SWITCHER_MENU_HEIGHT))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(scrollable_frame_when(
                "repo-switcher-scroll",
                ScrollbarMode::Vertical,
                switcher_content,
                switcher_handle,
                true,
                cx,
            ))
            .into_any_element()
    }

    /// 下拉功能区的一项（克隆/打开）。
    fn repo_switcher_action_item(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        label: &'static str,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .text_size(px(12.0))
            .text_color(rgb(ui_theme::FOREGROUND))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .child(toolbar_icon(icon, ui_theme::MUTED_FOREGROUND))
            .child(label)
    }

    /// 下拉分区小标题。
    fn repo_switcher_section_header(&self, label: &'static str) -> impl IntoElement {
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .text_size(px(11.0))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(label)
    }

    /// 下拉仓库行：头像 + 名称 + 完整路径；已打开项 hover 显示关闭按钮；
    /// 活动仓库使用选中色优先。
    fn repo_switcher_repo_item(
        &self,
        repo: RepoSwitcherRepo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let path_for_click = repo.full_path.clone();
        let tab_id = repo.tab_id;
        let is_active = repo.active;
        let can_close = repo.tab_id.is_some();
        let close_tab_id = repo.tab_id;

        div()
            .id(format!("repo-switcher-item-{}", repo.path_key))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(is_active, |this| this.bg(rgb(ui_theme::ACCENT)))
            .when(!is_active, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.close_repo_switcher();
                if let Some(id) = tab_id {
                    this.activate_tab(id);
                } else {
                    this.open_repo(PathBuf::from(&path_for_click));
                }
                cx.notify();
            }))
            .child(repo_avatar(&repo.name))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(1.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .truncate()
                                    .child(repo.name),
                            )
                            .when(is_active, |this| {
                                this.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child("✓"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .truncate()
                            .child(repo.full_path),
                    ),
            )
            .when(can_close, |this| {
                this.child(
                    div()
                        .id(format!("repo-switcher-close-{}", repo.path_key))
                        .flex_none()
                        .size(px(20.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(ui_theme::RADIUS_XS))
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::DESTRUCTIVE)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            if let Some(id) = close_tab_id {
                                this.close_tab(id);
                            }
                            cx.notify();
                        }))
                        .child("✕"),
                )
            })
    }

    /// 设置中心 overlay：左导航 + 右内容面板。
    fn render_settings_center_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(category) = self.settings_center else {
            return div().into_any_element();
        };

        let categories = [
            (
                SettingsCategory::Credentials,
                ToolbarIcon::Credentials,
                "凭据管理",
            ),
            (SettingsCategory::Proxy, ToolbarIcon::Proxy, "网络代理"),
            (SettingsCategory::Ai, ToolbarIcon::Ai, "AI 设置"),
            (
                SettingsCategory::ExternalMerge,
                ToolbarIcon::Workflow,
                "合并工具",
            ),
            (SettingsCategory::Theme, ToolbarIcon::Globe, "外观"),
            (SettingsCategory::Update, ToolbarIcon::Update, "更新设置"),
            (SettingsCategory::Shortcuts, ToolbarIcon::Keyboard, "快捷键"),
            (SettingsCategory::About, ToolbarIcon::Info, "关于"),
        ];

        // 右侧内容面板根据当前分类渲染对应 body。
        let body: gpui::AnyElement = match category {
            SettingsCategory::Credentials => {
                self.render_credential_manager_dialog(cx).into_any_element()
            }
            SettingsCategory::Proxy => self
                .render_network_proxy_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Ai => self
                .render_ai_provider_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::ExternalMerge => self
                .render_external_merge_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Theme => self
                .render_theme_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Update => self.render_update_settings_dialog(cx).into_any_element(),
            SettingsCategory::Shortcuts => self.render_shortcuts_settings(cx).into_any_element(),
            SettingsCategory::About => self.render_about_settings(cx).into_any_element(),
        };
        // 右侧内容区的滚动句柄，供内容超出固定高度时滚动并绘制滚动条。
        let settings_content_handle = self.scroll_handle("settings-center-content");

        // 遮罩不承载关闭：点击遮罩背景、遮罩上方的通知气泡（含其关闭按钮）
        // 都不关闭设置中心——唯一关闭入口是弹窗右上角的「✕」（Ctrl+, 快捷键
        // 保留 toggle 语义）。遮罩自身 occlude() 挡住下层 UI 的点击。
        dialog_overlay()
            .child(
                div()
                    .id("settings-center-panel")
                    .track_focus(&self.settings_center_focus)
                    .w(px(900.0))
                    // 固定高度，弹窗大小不随分类内容多少变化；内容超出由右侧内容区滚动。
                    .h(px(640.0))
                    .min_w(px(0.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    // 顶栏
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child("设置中心"),
                            )
                            .child(
                                div()
                                    .id("settings-center-close")
                                    .size(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .cursor_pointer()
                                    .text_size(px(14.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.close_settings_center();
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    // 主体：左导航 + 右内容
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            // 左导航
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_none()
                                    .w(px(160.0))
                                    .border_r_1()
                                    .border_color(rgb(ui_theme::BORDER))
                                    .py_2()
                                    .children(categories.iter().map(|(cat, icon, label)| {
                                        let is_active = *cat == category;
                                        let cat = *cat;
                                        div()
                                            .id(format!("settings-nav-{label}"))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .text_size(px(12.0))
                                            .cursor_pointer()
                                            .when(is_active, |this| {
                                                this.bg(rgb(ui_theme::ACCENT))
                                                    .text_color(rgb(ui_theme::PRIMARY))
                                            })
                                            .when(!is_active, |this| {
                                                this.text_color(rgb(ui_theme::FOREGROUND))
                                                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                            })
                                            .on_click(cx.listener(
                                                move |this, _event, _window, cx| {
                                                    this.select_settings_category(cat);
                                                    cx.notify();
                                                },
                                            ))
                                            .child(toolbar_icon(
                                                *icon,
                                                if is_active {
                                                    ui_theme::PRIMARY
                                                } else {
                                                    ui_theme::MUTED_FOREGROUND
                                                },
                                            ))
                                            .child(*label)
                                            .into_any_element()
                                    })),
                            )
                            // 右内容：固定高度内滚动，叠加滚动条。
                            .child(scrollable_frame_when(
                                "settings-center-content",
                                ScrollbarMode::Vertical,
                                div()
                                    .id("settings-center-content")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .p_4()
                                    .overflow_y_scroll()
                                    .track_scroll(&settings_content_handle)
                                    .child(body)
                                    .into_any_element(),
                                settings_content_handle,
                                true,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_tag_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.tag_context_menu.clone() else {
            return div().into_any_element();
        };
        let has_remotes = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.remotes.is_empty());

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(TAG_MENU_WIDTH))
            .child(context_menu_item(
                "检出标签",
                !self.busy && !self.merge_in_progress(),
                {
                    let tag = menu.tag.clone();
                    move |this| this.checkout_tag(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "浏览此标签",
                !self.busy,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_browse_tag(tag.clone())
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                "推送到远端...",
                !self.busy && has_remotes,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_tag_push_dialog(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除标签",
                !self.busy,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_delete_tag_confirm(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除远端标签...",
                !self.busy && has_remotes,
                {
                    let tag = menu.tag.clone();
                    let remote = self.current_remote().unwrap_or_default();
                    move |this| this.open_delete_remote_tag_confirm(remote.clone(), tag.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_stash_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.stash_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(STASH_MENU_WIDTH))
            .child(self.render_stash_context_menu_content(menu.index, cx))
            .into_any_element()
    }

    fn render_workflow_template_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.workflow_template_context_menu.clone() else {
            return div().into_any_element();
        };
        let edit_path = menu.path.clone();
        let copy_path = menu.path.clone();
        // 删除确认弹窗展示用名称：优先解析出的显示名不可得（坏模板也允许删），退回文件名主干
        let delete_path = menu.path.clone();
        let delete_display_name = menu
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "该模板".to_string());

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(WORKFLOW_TEMPLATE_MENU_WIDTH))
            .child(context_menu_item_with_context(
                "编辑此模板",
                !self.busy,
                move |this, cx| {
                    this.workflow_template_context_menu = None;
                    let path = edit_path.clone();
                    this.open_workflow_editor_for_path(path, false, cx);
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                "复制为副本",
                !self.busy,
                move |this, cx| {
                    this.workflow_template_context_menu = None;
                    let path = copy_path.clone();
                    this.open_workflow_editor_for_path(path, true, cx);
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item_with_context(
                "删除模板...",
                !self.busy,
                move |this, _cx| {
                    this.workflow_template_context_menu = None;
                    let path = delete_path.clone();
                    let name = delete_display_name.clone();
                    this.open_delete_workflow_template_confirm(path, name);
                },
                cx,
            ))
            .into_any_element()
    }

    /// 打开「删除工作流模板」确认弹窗。
    pub(crate) fn open_delete_workflow_template_confirm(
        &mut self,
        path: PathBuf,
        display_name: String,
    ) {
        self.active_dialog =
            Some(DialogState::ConfirmDeleteWorkflowTemplate { path, display_name });
        self.last_error = None;
    }

    /// 删除工作流模板文件（纯本地 IO，小文件同步执行）；若它是当前加载的
    /// 工作流则同时清空详情区，避免残留失效引用。
    pub(crate) fn delete_workflow_template(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match fs::remove_file(&path) {
            Ok(()) => {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                if self.workflow_state.selected_template_path.as_ref() == Some(&path) {
                    self.clear_workflow_file();
                    self.workflow_state.selected_template_path = None;
                }
                self.refresh_workflow_templates();
                self.status = format!("工作流模板已删除：{file_name}");
                self.notify_success(format!("工作流模板已删除：{file_name}"), cx);
            }
            Err(err) => {
                self.last_error = Some(format!("工作流模板删除失败：{err}"));
            }
        }
    }

    fn render_change_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.change_context_menu.clone() else {
            return div().into_any_element();
        };
        let selected_count = self.change_selection.selected(&menu.scope).len();
        let selected_paths = self.selected_change_paths(menu.scope.clone());
        let all_paths = self.change_paths(menu.scope.clone());
        let all_count = all_paths.len();
        let can_discard = !self.busy && !self.merge_in_progress();

        let mut menu_el = glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(CHANGE_MENU_WIDTH))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.credential_context_menu.is_some() {
                        this.credential_context_menu = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            });

        menu_el = match menu.scope {
            DiffScope::Staged => menu_el
                .child(context_menu_item_with_context(
                    "复制绝对路径",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item_with_context(
                    "打开文件所在目录",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.open_file_parent_directory(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "追溯此文件",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.open_blame_file(path.clone())
                    },
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "取消暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.unstage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    "取消暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.unstage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "回滚更改...",
                    can_discard,
                    {
                        let path = menu.path.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                vec![path.clone()],
                                scope.clone(),
                                DiscardTarget::Single,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚指定更改...",
                    selected_count > 0 && can_discard,
                    {
                        let paths = selected_paths.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::Selected,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚全部更改...",
                    all_count > 0 && can_discard,
                    {
                        let paths = all_paths.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::All,
                            )
                        }
                    },
                    cx,
                )),
            DiffScope::Unstaged => menu_el
                .child(context_menu_item(
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "追溯此文件",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.open_blame_file(path.clone())
                    },
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.stage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    "暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.stage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "回滚更改...",
                    can_discard,
                    {
                        let path = menu.path.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                vec![path.clone()],
                                scope.clone(),
                                DiscardTarget::Single,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚指定更改...",
                    selected_count > 0 && can_discard,
                    {
                        let paths = selected_paths;
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::Selected,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚全部更改...",
                    all_count > 0 && can_discard,
                    {
                        let paths = all_paths;
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::All,
                            )
                        }
                    },
                    cx,
                )),
        };

        menu_el.into_any_element()
    }

    fn render_file_path_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.file_path_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(FILE_PATH_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(context_menu_item_with_context(
                "复制绝对路径",
                true,
                {
                    let path = menu.path.clone();
                    move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                "打开文件所在目录",
                true,
                {
                    let path = menu.path.clone();
                    move |this, cx| this.open_file_parent_directory(path.clone(), cx)
                },
                cx,
            ))
            // 「追溯此文件」对 HEAD 版本追溯（v1 不支持对任意提交 blame）
            .child(context_menu_item(
                "查看文件历史",
                true,
                {
                    let path = menu.path.clone();
                    move |this| this.view_file_history(path.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "追溯此文件",
                true,
                {
                    let path = menu.path.clone();
                    move |this| this.open_blame_file(path.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_commit_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.commit_context_menu.clone() else {
            return div().into_any_element();
        };
        let is_merge_commit = menu.parent_count > 1;
        let revert_label = if is_merge_commit {
            "撤销合并提交..."
        } else {
            "回滚提交"
        };
        let can_change_repository = !self.busy && !self.merge_in_progress();

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(COMMIT_MENU_WIDTH))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.credential_context_menu.is_some() {
                        this.credential_context_menu = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("提交 {}", menu.short_oid)),
            )
            .child(menu_separator())
            .when(menu.is_unpushed, |this| {
                let can_uncommit = can_change_repository && menu.is_head;
                let label = if menu.is_head {
                    "还原到暂存区..."
                } else {
                    "还原到暂存区（仅支持最新提交）"
                };
                this.child(context_menu_item(
                    label,
                    can_uncommit,
                    {
                        let oid = menu.oid.clone();
                        let summary = menu.summary.clone();
                        move |this| {
                            this.open_uncommit_to_staged_confirm_dialog(
                                oid.clone(),
                                summary.clone(),
                            )
                        }
                    },
                    cx,
                ))
                .child(menu_separator())
            })
            .child(context_menu_item(
                "软重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Soft,
                        )
                    }
                },
                cx,
            ))
            .child(context_menu_item(
                "混合重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Mixed,
                        )
                    }
                },
                cx,
            ))
            .child(context_menu_item(
                "强制重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Hard,
                        )
                    }
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                revert_label,
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        if is_merge_commit {
                            this.open_revert_merge_confirm_dialog(oid.clone(), summary.clone())
                        } else {
                            this.open_revert_confirm_dialog(oid.clone(), summary.clone())
                        }
                    }
                },
                cx,
            ))
            // 拣选提交：合并提交暂不支持（需要 -m mainline 语义，后续迭代）。
            .child(context_menu_item(
                if is_merge_commit {
                    "拣选提交（暂不支持合并提交）"
                } else {
                    "拣选提交到当前分支"
                },
                can_change_repository && !is_merge_commit,
                {
                    let oid = menu.oid.clone();
                    move |this| this.cherry_pick_commit(oid.clone())
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                "在此提交上创建标签...",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| this.open_tag_form_dialog(Some(oid.clone()), summary.clone())
                },
                cx,
            ))
            .child(self.commit_copy_sha_menu_item(menu.oid.clone(), cx))
            .into_any_element()
    }

    fn render_credential_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.credential_context_menu.clone() else {
            return div().into_any_element();
        };
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == menu.record_id)
            .cloned()
        else {
            return div().into_any_element();
        };

        let name = Some(credential_record_label(&record));
        let target = Some(credential_display_target(&record));
        let username = Some(record.username.clone());
        let key_path = record.key_path.clone();

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(CREDENTIAL_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(self.credential_copy_menu_item("复制名称", name, "凭据名称", cx))
            .child(self.credential_copy_menu_item("复制站点/远端", target, "站点/远端", cx))
            .child(self.credential_copy_menu_item("复制用户名", username, "用户名", cx))
            .child(self.credential_copy_menu_item(
                "复制 SSH Key 路径",
                key_path,
                "SSH Key 路径",
                cx,
            ))
            .into_any_element()
    }

    fn credential_copy_menu_item(
        &self,
        label: &'static str,
        text: Option<String>,
        status_label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = text
            .as_ref()
            .is_some_and(|text| !text.is_empty() && text != "-");
        div()
            .id(format!("credential-context-menu-{label}"))
            .px_3()
            .py_1()
            .text_color(if enabled {
                rgb(ui_theme::FOREGROUND)
            } else {
                rgb(ui_theme::MUTED_FOREGROUND)
            })
            .bg(rgb(ui_theme::CARD))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                if enabled {
                    this.copy_credential_text(text.clone(), status_label, cx);
                    cx.notify();
                }
            }))
            .child(label)
    }

    fn commit_copy_sha_menu_item(&self, oid: String, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("context-menu-copy-commit-sha")
            .px_3()
            .py_1()
            .text_color(rgb(ui_theme::FOREGROUND))
            .bg(rgb(ui_theme::CARD))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.copy_commit_sha(oid.clone(), cx);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child("复制 SHA 到剪贴板")
    }

    pub(crate) fn render_encoding_dropdown(
        &self,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.encoding_menu_target != Some(target) {
            return div().into_any_element();
        }
        let current = self.current_diff_encoding_choice();
        let title = match target {
            EncodingMenuTarget::Worktree => "工作区差异编码",
            EncodingMenuTarget::History => "提交差异编码",
            EncodingMenuTarget::Stash => "贮藏差异编码",
            EncodingMenuTarget::Browse => "浏览编码",
            EncodingMenuTarget::Blame => "追溯编码",
        };

        glass_menu()
            .absolute()
            .top(px(38.0))
            .right(px(12.0))
            .w(px(ENCODING_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(title),
            )
            .child(menu_separator())
            .child(self.encoding_menu_item(DiffEncodingChoice::Auto, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Utf8, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Gb18030, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Big5, current, cx))
            .into_any_element()
    }

    fn encoding_menu_item(
        &self,
        choice: DiffEncodingChoice,
        current: DiffEncodingChoice,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = choice == current;
        let label = if selected {
            format!("✓ {}", choice.label())
        } else {
            format!("  {}", choice.label())
        };
        div()
            .id(format!("context-menu-encoding-{}", choice.label()))
            .px_3()
            .py_1()
            .text_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::FOREGROUND)
            })
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::CARD)
            })
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.choose_diff_encoding(choice);
                cx.notify();
            }))
            .child(label)
    }

    pub(crate) fn render_column_splitter(
        &self,
        target: ResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let active = self.resize_state(target).is_some();
        let horizontal = target == ResizeTarget::HistoryDetails;
        // 弹窗或弹层菜单打开期间分割线不响应：不显示拖拽光标、不高亮、不响应鼠标，
        // 避免弹层边缘容差区内的悬停/点击被分割线抢走。
        let interactive = column_splitter_accepts_mouse_events(
            self.active_dialog.is_some(),
            self.any_popup_menu_open(),
        );

        div()
            .flex_none()
            .relative()
            .map(|this| {
                if horizontal {
                    this.h(px(8.0)).w_full()
                } else {
                    this.w(px(8.0)).h_full()
                }
            })
            .when(interactive, |this| {
                this.cursor(if horizontal {
                    CursorStyle::ResizeRow
                } else {
                    CursorStyle::ResizeColumn
                })
                .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            })
            .bg(if active {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::CARD)
            })
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseUpEvent, _window, cx| {
                    if this.resize_state(target).is_some() {
                        this.finish_resize_column(target);
                        cx.notify();
                    }
                }),
            )
            .child(if horizontal {
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .top(px(3.0))
                    .h(px(1.0))
                    .bg(if active {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .into_any_element()
            } else {
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(px(1.0))
                    .bg(if active {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .into_any_element()
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseDownEvent, _, _, cx| {
                                if !bounds.contains(&event.position) {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if !column_splitter_accepts_mouse_events(
                                        this.active_dialog.is_some(),
                                        this.any_popup_menu_open(),
                                    ) {
                                        this.finish_resize_column(target);
                                        cx.notify();
                                        return;
                                    }
                                    if event.click_count >= 2 {
                                        this.reset_resize_target(target);
                                    } else {
                                        this.start_resize_column(target, event);
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseMoveEvent, _, _, cx| {
                                let (resizing, active_dialog, popup_open) = {
                                    let view = entity.read(cx);
                                    (
                                        view.resize_state(target).is_some(),
                                        view.active_dialog.is_some(),
                                        view.any_popup_menu_open(),
                                    )
                                };
                                if column_splitter_should_clear_resize(
                                    active_dialog || popup_open,
                                    resizing,
                                ) {
                                    entity.update(cx, |this, cx| {
                                        this.finish_resize_column(target);
                                        cx.notify();
                                    });
                                    return;
                                }
                                if !resizing
                                    || !event.dragging()
                                    || !column_splitter_accepts_mouse_events(
                                        active_dialog,
                                        popup_open,
                                    )
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    this.update_resize_column(target, event);
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                            let (resizing, active_dialog, popup_open) = {
                                let view = entity.read(cx);
                                (
                                    view.resize_state(target).is_some(),
                                    view.active_dialog.is_some(),
                                    view.any_popup_menu_open(),
                                )
                            };
                            if !resizing {
                                return;
                            }
                            if !column_splitter_accepts_mouse_events(active_dialog, popup_open)
                                && !column_splitter_should_clear_resize(
                                    active_dialog || popup_open,
                                    resizing,
                                )
                            {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                this.finish_resize_column(target);
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0)),
            )
    }

    pub(crate) fn render_virtual_diff(
        &self,
        scroll_id: &'static str,
        diff: Option<Arc<FileDiff>>,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 二进制文件不渲染逐行 diff（也不显示「Binary files ... differ」原始行），
        // 直接显示信息占位卡片，含文件大小/新增删除信息。例外：Office 文档
        // （docx/xlsx/pptx）提取出的文本差异行——is_binary 保持 true（沿用
        // 全部二进制门控）但携带 lines，按普通行渲染文本化预览。
        if let Some(diff) = diff
            .as_deref()
            .filter(|diff| diff.is_binary && diff.lines.is_empty())
        {
            return binary_diff_placeholder(diff).into_any_element();
        }
        let model = diff_render_model_for(diff.as_deref(), headers_expanded);
        let row_count = model.row_count;
        let content_present = diff.is_some() && row_count > 0;
        // 以内容最宽的文本行作为列表水平宽度的测量基准，保证长行也能左右滚动。
        // 结果按 diff 身份缓存：大 diff（上限 2 万行）每帧重算是 O(总字符) 扫描。
        let width_measure_index = cached_widest_diff_row_index(
            diff.as_ref(),
            headers_expanded,
            &model,
            &self.widest_diff_row_cache,
        )
        .or_else(|| row_count.checked_sub(1));
        let handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = handle.clone();
        let model_for_list = model.clone();
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let diff = diff.as_deref();
                        range
                            .map(|index| {
                                this.render_diff_row(
                                    diff,
                                    model_for_list.row_at(index),
                                    headers_expanded,
                                    header_target,
                                    &empty_message,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_width_from_item(width_measure_index)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Both,
            content,
            handle,
            content_present,
            cx,
        )
        .into_any_element()
    }

    /// 按差异视图上下文取对应槽位的语法高亮结果（带主题变体守卫）。
    fn syntax_spans_for_diff(&self, target: DiffHeaderTarget) -> Option<&SharedSyntaxSpans> {
        let spans = match target {
            DiffHeaderTarget::Worktree => &self.diff_syntax,
            DiffHeaderTarget::History => &self.history_diff_syntax,
            DiffHeaderTarget::Stash => &self.stash_preview.diff_syntax,
            DiffHeaderTarget::Browse => &self.browse.diff_syntax,
        };
        spans
            .as_deref()
            .filter(|spans| spans.dark == ui::theme::active_variant().is_dark())
    }

    fn render_diff_row(
        &self,
        diff: Option<&FileDiff>,
        row: DiffRenderRow,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 仅工作区差异视图提供部分暂存交互（历史/贮藏/浏览只读）；Office
        // 文档的文本化差异是提取合成的，不能按块/按行回写（部分暂存守卫在
        // 服务层同样拒绝），这里直接不显示按钮。
        let partial_stage_enabled =
            header_target == DiffHeaderTarget::Worktree && !diff.is_some_and(|d| d.is_binary);
        match row {
            DiffRenderRow::HeaderToggle => {
                let summary = if headers_expanded {
                    "Diff 元信息（点击折叠）"
                } else {
                    "Diff 元信息（点击展开）"
                };
                diff_header_toggle(summary, header_target, cx).into_any_element()
            }
            DiffRenderRow::DiffLine(index) => {
                let Some(line) = diff.and_then(|diff| diff.lines.get(index)) else {
                    return diff_line(DiffLineKind::Context, None, None, String::new(), None)
                        .into_any_element();
                };
                // 按差异上下文取对应槽位的语法高亮（仅全文模式计算过）。
                let syntax_spans = self
                    .syntax_spans_for_diff(header_target)
                    .and_then(|spans| spans.lines.get(index).map(Vec::as_slice));
                if line.kind == DiffLineKind::Header {
                    let is_hunk_header = line.content.starts_with("@@");
                    let row_element =
                        diff_line(line.kind.clone(), None, None, line.content.clone(), None);
                    if partial_stage_enabled && is_hunk_header {
                        // hunk 分隔行右侧提供整块暂存/取消暂存按钮。
                        let is_stage = diff
                            .map(|diff| diff.scope == DiffScope::Unstaged)
                            .unwrap_or(true);
                        let label: &'static str = if is_stage {
                            "暂存此块"
                        } else {
                            "取消暂存此块"
                        };
                        let hunk_index = line.hunk_index;
                        return div()
                            .relative()
                            .child(row_element)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_1()
                                    .flex()
                                    .items_center()
                                    .child(diff_hunk_action_button(
                                        hunk_index,
                                        label,
                                        move |this| {
                                            this.apply_hunk_partial_stage(hunk_index);
                                        },
                                        cx,
                                    )),
                            )
                            .into_any_element();
                    }
                    return row_element.into_any_element();
                }
                let row_element = diff_line(
                    display_diff_line_kind(line.kind.clone(), diff.is_some_and(|d| d.untracked)),
                    line.old_lineno,
                    line.new_lineno,
                    line.content.clone(),
                    syntax_spans,
                );
                if !partial_stage_enabled {
                    return row_element.into_any_element();
                }
                let selectable = matches!(line.kind, DiffLineKind::Added | DiffLineKind::Removed);
                if !selectable {
                    return row_element.into_any_element();
                }
                // +/- 行：点击选择（Ctrl/Cmd 多选、Shift 范围）。
                // 高亮层必须放在 row_element 之后：GPUI 按子元素顺序绘制，
                // 放在前面会被行自身的不透明背景完全盖住，视觉上不可见。
                let selected = self.diff_line_selection.contains(&index);
                div()
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            let multi = event.modifiers.control || event.modifiers.platform;
                            let shift = event.modifiers.shift;
                            this.toggle_diff_line_selection(index, multi, shift);
                            cx.notify();
                        }),
                    )
                    .child(row_element)
                    .when(selected, |this| {
                        this.child(
                            // 整行半透明主题色打底：复用输入选区 token（自带 alpha，跟随主题色），
                            // 叠加在 +/- 行背景色之上仍能清晰辨认选中范围。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .bg(ui_theme::rgba(ui_theme::INPUT_SELECTION)),
                        )
                        .child(
                            // 左缘 2px 主题色实线条作为第二重视觉信号。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(2.0))
                                .bg(rgb(ui_theme::PRIMARY)),
                        )
                    })
                    .into_any_element()
            }
            DiffRenderRow::Empty => {
                let message = diff
                    .map(|diff| {
                        if diff.is_binary {
                            "二进制文件仅显示元信息"
                        } else {
                            "没有可显示的文本差异"
                        }
                    })
                    .unwrap_or(empty_message);
                diff_line(DiffLineKind::Context, None, None, message.to_string(), None)
                    .into_any_element()
            }
        }
    }

    pub(crate) fn diff_section_header(
        &self,
        title: String,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let diff = match target {
            EncodingMenuTarget::Worktree => self.diff.as_deref(),
            EncodingMenuTarget::History => self.history_diff.as_deref(),
            EncodingMenuTarget::Stash => self.stash_preview.diff.as_deref(),
            EncodingMenuTarget::Browse => self.browse.diff.as_deref(),
            // 追溯视图没有 FileDiff；该 target 不经此头部渲染
            EncodingMenuTarget::Blame => None,
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // 二进制文件没有全文/编码差异可言，隐藏这两个工具按钮
                    .when(!diff.is_some_and(|diff| diff.is_binary), |this| {
                        this.child(self.full_file_toggle_button(target, cx))
                            .child(self.encoding_button(diff, target, cx))
                    })
                    // 按行选择非空时的部分暂存入口（仅工作区差异视图）。
                    .when(
                        target == EncodingMenuTarget::Worktree
                            && !self.diff_line_selection.is_empty(),
                        |this| {
                            let count = self.diff_line_selection.len();
                            let is_stage = self
                                .diff
                                .as_ref()
                                .map(|diff| diff.scope == DiffScope::Unstaged)
                                .unwrap_or(true);
                            let label = if is_stage {
                                format!("暂存选中行({count})")
                            } else {
                                format!("取消暂存选中行({count})")
                            };
                            this.child(
                                // 尺寸规格与「全文/编码」工具按钮一致（px 8 / py 2 /
                                // RADIUS_XS / 11px），避免撑高差异区标题栏。
                                div()
                                    .id("stage-selected-diff-lines-button")
                                    .flex_none()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::ACCENT))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .cursor_pointer()
                                    .hover(|hover| hover.bg(rgb(ui_theme::SECONDARY)))
                                    .child(label)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.apply_selected_partial_stage();
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    // 工作区差异的「追溯」入口：打开该文件的追溯视图
                    //（规格严格复用「全文/编码」工具按钮；二进制文件不提供）。
                    .when(
                        target == EncodingMenuTarget::Worktree
                            && self.diff.as_ref().is_some_and(|diff| !diff.is_binary),
                        |this| {
                            let path = self
                                .diff
                                .as_ref()
                                .map(|diff| diff.path.clone())
                                .unwrap_or_default();
                            this.child(
                                div()
                                    .id("worktree-diff-blame-button")
                                    .flex_none()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::ACCENT))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .cursor_pointer()
                                    .hover(|hover| hover.bg(rgb(ui_theme::SECONDARY)))
                                    .child("追溯")
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_blame_file(path.clone());
                                        cx.notify();
                                    })),
                            )
                        },
                    ),
            )
    }

    fn encoding_button(
        &self,
        diff: Option<&FileDiff>,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let requested = self.current_diff_encoding_choice();
        let label = diff
            .map(diff_encoding_label)
            .unwrap_or_else(|| format!("编码：{}", requested.label()));
        div()
            .id(match target {
                EncodingMenuTarget::Worktree => "worktree-diff-encoding",
                EncodingMenuTarget::History => "history-diff-encoding",
                EncodingMenuTarget::Stash => "stash-diff-encoding",
                EncodingMenuTarget::Browse => "browse-encoding",
                EncodingMenuTarget::Blame => "blame-encoding",
            })
            .relative()
            .flex_none()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::ACCENT))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.toggle_encoding_menu(target);
                    cx.notify();
                }),
            )
            .child(label)
    }

    fn render_status(&self) -> impl IntoElement {
        let status_label = if self.busy { "运行中" } else { "就绪" };
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_deref())
            .unwrap_or("未打开仓库");
        let staged_count = self.change_indexes.staged.len();
        let unstaged_count = self.change_indexes.unstaged.len();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(chrome_view::STATUS_BAR_HEIGHT))
            .px(px(16.0))
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .text_size(px(10.0))
            .child(
                div()
                    .flex_none()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(if self.busy {
                        ui_theme::PRIMARY
                    } else {
                        ui_theme::GIT_ADDED
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(status_label),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(branch.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(if self.busy {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .child(if self.busy {
                        format!("{}...", self.status)
                    } else {
                        self.status.clone()
                    }),
            )
            .when_some(self.last_error.clone(), |this, error| {
                this.child(
                    div()
                        .max_w(px(360.0))
                        .truncate()
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(format!("错误：{error}")),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("{unstaged_count} 未暂存 · {staged_count} 已暂存")),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
    }

    fn has_active_loading(&self) -> bool {
        let tab = self.active_tab_state();
        tab.operation_kind.shows_progress()
            && (tab.busy || tab.loading != RepositoryLoading::default())
    }

    fn render_feedback_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 所有通知气泡统一在右下角堆叠（按队列顺序），不再按重要度分左右；
        // 每个气泡（成功/信息/警告/错误）都带关闭按钮（feedback_bubble 内置）。
        let mut stack = feedback_stack();
        for feedback in self.feedbacks.iter() {
            stack = stack.child(feedback_bubble(feedback, cx));
        }

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .child(stack)
            // 状态文字已在底栏展示，操作期间只保留轻量进度线，避免重复悬浮框。
            .when(self.has_active_loading(), |this| {
                this.child(bottom_progress_bar(self.progress_phase))
            })
    }

    fn render_credentials(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(pending) = self.pending_credential.as_ref() else {
            return div().into_any_element();
        };

        div()
            .absolute()
            .top(px(70.0))
            .right(px(18.0))
            .w(px(420.0))
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_2()
            .cursor(CursorStyle::Arrow)
            .occlude()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("需要凭据"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("远端：{}", pending.request.url)),
            )
            .child(self.input(FieldId::CredentialUsername, true, window, cx))
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.input(FieldId::CredentialSecret, true, window, cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Ssh,
                |this| {
                    this.child(self.toggle_row(
                        "credential-use-ssh-agent",
                        "使用 SSH agent",
                        self.credential_use_ssh_agent,
                        |this, _, _| this.credential_use_ssh_agent = !this.credential_use_ssh_agent,
                        cx,
                    ))
                    .when(!self.credential_use_ssh_agent, |this| {
                        this.child(self.input(FieldId::CredentialKeyPath, true, window, cx))
                    })
                    .child(self.input(
                        FieldId::CredentialPassphrase,
                        true,
                        window,
                        cx,
                    ))
                },
            )
            .child(self.toggle_row(
                "save-credential",
                "保存到系统凭据管理器",
                self.save_credential,
                |this, _, _| this.save_credential = !this.save_credential,
                cx,
            ))
            .when(self.save_credential, |this| {
                this.child(self.input(FieldId::CredentialDisplayName, true, window, cx))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!self.save_credential, |this| this.opacity(0.55))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("复用范围"),
                    )
                    .child(self.credential_scope_button(
                        "仅此远端",
                        CredentialScope::RemoteUrl,
                        self.save_credential,
                        cx,
                    ))
                    .child(self.credential_scope_button(
                        "同站点",
                        CredentialScope::Host,
                        self.save_credential,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(self.primary_button(
                        "使用凭据",
                        true,
                        |this, _, _| this.use_credentials(),
                        cx,
                    ))
                    .child(self.button(
                        "取消",
                        true,
                        |this, _, _| this.cancel_credential_request(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_dialogs(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(dialog) = self.active_dialog.clone() else {
            return div().into_any_element();
        };

        let content = match dialog {
            DialogState::CloneRepo => self.render_clone_dialog(window, cx).into_any_element(),
            DialogState::CreateBranch => self
                .render_create_branch_dialog(window, cx)
                .into_any_element(),
            DialogState::RenameBranch { branch } => self
                .render_rename_branch_dialog(branch, window, cx)
                .into_any_element(),
            DialogState::ConfirmReset { oid, summary, mode } => self
                .render_confirm_reset_dialog(oid, summary, mode, cx)
                .into_any_element(),
            DialogState::ConfirmRevert { oid, summary } => self
                .render_confirm_revert_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmRevertMerge { oid, summary } => self
                .render_confirm_revert_merge_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmUncommitToStaged { oid, summary } => self
                .render_confirm_uncommit_to_staged_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmAmendPushed { and_push } => self
                .render_confirm_amend_pushed_dialog(and_push, cx)
                .into_any_element(),
            DialogState::TagForm {
                target_oid,
                target_summary,
            } => self
                .render_tag_form_dialog(target_oid, target_summary, window, cx)
                .into_any_element(),
            DialogState::TagPush { tag } => self
                .render_tag_push_dialog(tag, window, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteTag { tag } => self
                .render_confirm_delete_tag_dialog(tag, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemoteTag { remote, tag } => self
                .render_confirm_delete_remote_tag_dialog(remote, tag, cx)
                .into_any_element(),
            DialogState::ConfirmDiscardChange {
                scope,
                target,
                paths,
            } => self
                .render_confirm_discard_change_dialog(scope, target, paths, cx)
                .into_any_element(),
            DialogState::CredentialDetails { record_id } => self
                .render_credential_details_dialog(record_id, cx)
                .into_any_element(),
            DialogState::CredentialForm { editing } => self
                .render_credential_form_dialog(editing, window, cx)
                .into_any_element(),
            DialogState::TestCredential { record_id } => self
                .render_test_credential_dialog(record_id, window, cx)
                .into_any_element(),
            DialogState::SubmoduleManager => {
                self.render_submodule_manager_dialog(cx).into_any_element()
            }
            DialogState::RemoteManager => self.render_remote_manager_dialog(cx).into_any_element(),
            DialogState::RemoteForm { editing } => self
                .render_remote_form_dialog(editing, window, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemote { name } => self
                .render_confirm_delete_remote_dialog(name, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemoteBranch { remote, branch } => self
                .render_confirm_delete_remote_branch_dialog(remote, branch, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteCredential { record_id, label } => self
                .render_confirm_delete_credential_dialog(record_id, label, cx)
                .into_any_element(),
            DialogState::StashForm => self.render_stash_form_dialog(window, cx).into_any_element(),
            DialogState::WorkflowEditor => self
                .render_workflow_editor_dialog(window, cx)
                .into_any_element(),
            DialogState::ConfirmWorkflowEditComments => self
                .render_confirm_workflow_edit_comments(cx)
                .into_any_element(),
            DialogState::ConfirmDeleteWorkflowTemplate { path, display_name } => self
                .render_confirm_delete_workflow_template_dialog(path, display_name, cx)
                .into_any_element(),
            DialogState::ConfirmDropStash { index, message } => self
                .render_confirm_drop_stash_dialog(index, message, cx)
                .into_any_element(),
            DialogState::ConfirmPopStash { index, message } => self
                .render_confirm_pop_stash_dialog(index, message, cx)
                .into_any_element(),
            DialogState::RemoteBranchOperation { kind } => self
                .render_remote_branch_operation_dialog(kind, window, cx)
                .into_any_element(),
            DialogState::ConfirmConflictResolve => self
                .render_confirm_conflict_resolve_dialog(cx)
                .into_any_element(),
            DialogState::ConfirmAiConflictMerge { path } => self
                .render_confirm_ai_conflict_merge_dialog(path, cx)
                .into_any_element(),
            DialogState::ConfirmAbortMerge => self
                .render_confirm_abort_merge_dialog(cx)
                .into_any_element(),
            DialogState::ConfirmWindowClose => self
                .render_confirm_window_close_dialog(cx)
                .into_any_element(),
            // ── 更新对话框 ──
            DialogState::NewVersionAvailable {
                version,
                notes,
                published_at,
                size,
            } => self
                .render_new_version_dialog(&version, &notes, &published_at, size, cx)
                .into_any_element(),
            DialogState::ConfirmInstallUpdate { version } => self
                .render_confirm_install_dialog(&version, cx)
                .into_any_element(),
            DialogState::UpdateNoWritePermission { version } => self
                .render_no_write_permission_dialog(&version, cx)
                .into_any_element(),
            DialogState::PortableMigrationPrompt => {
                self.render_portable_migration_dialog(cx).into_any_element()
            }
            DialogState::ExeRelocationPrompt => {
                self.render_exe_relocation_dialog(cx).into_any_element()
            }
        };

        dialog_overlay()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.close_credential_context_menu(cx);
                    cx.stop_propagation();
                }),
            )
            .child(content)
            .into_any_element()
    }

    fn render_clone_dialog(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = infer_clone_target_path(&self.clone_url.value, &self.clone_path.value)
            .map(|path| path.display().to_string());
        self.dialog_panel("克隆仓库", cx)
            .child(self.input(FieldId::CloneUrl, false, window, cx))
            .child(self.input(FieldId::ClonePath, false, window, cx))
            .child(self.toggle_row(
                "clone-recursive-submodules",
                "递归克隆子模块",
                self.clone_recursive_submodules,
                |this, _, _| this.clone_recursive_submodules = !this.clone_recursive_submodules,
                cx,
            ))
            .child(
                div()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(rgb(if preview.is_some() {
                        ui_theme::MUTED_FOREGROUND
                    } else {
                        ui_theme::MUTED_FOREGROUND
                    }))
                    .child(preview.unwrap_or_else(|| {
                        "填写远程仓库 URL 和父文件夹后显示最终代码路径".to_string()
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .child(self.button(
                        "选择目录",
                        !self.busy,
                        |this, _, _| this.browse_clone_target(),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "取消",
                                !self.busy,
                                |this, _, _| this.close_dialog(),
                                cx,
                            ))
                            .child(self.primary_button(
                                "克隆",
                                !self.busy,
                                |this, _, _| this.clone_repo(),
                                cx,
                            )),
                    ),
            )
    }

    fn render_confirm_window_close_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel("关闭 Khaslana", cx)
            .w(px(520.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("要直接退出应用，还是让 Khaslana 继续在系统托盘中运行？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("缩小到托盘后，可点击托盘图标恢复主窗口，或从托盘菜单退出。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| this.cancel_window_close(), cx))
                    .child(self.primary_button(
                        "缩小到托盘",
                        true,
                        |this, window, cx| this.minimize_to_tray(window, cx),
                        cx,
                    ))
                    .child(self.danger_button(
                        "直接退出",
                        true,
                        |this, _, cx| this.exit_application(cx),
                        cx,
                    )),
            )
    }

    fn render_portable_migration_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel("迁移到便携目录", cx)
            .w(px(540.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("检测到应用数据当前存放在 C 盘系统目录，是否迁移到程序所在目录？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(
                        "迁移后，数据库、更新缓存和工作流模板将统一保存在可执行文件同级的 \
                         data/ 目录，便于整体备份并减少 C 盘占用。点击「迁移并重启」后应用将关闭，\
                         并在下次启动时自动完成数据搬运。若选择「保持现状」，后续可在「设置」-「更新设置」中手动执行迁移。",
                    ),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        "保持现状",
                        true,
                        |this, _, _| this.dismiss_portable_migration(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "迁移并重启",
                        true,
                        |this, _, _| this.confirm_portable_migration(),
                        cx,
                    )),
            )
    }

    /// 程序位置风险搬迁弹窗：exe 位于临时/聊天软件接收/下载目录时建议
    /// 把程序与数据一起移到安全目录。文案区分风险级别，并说明数据当前
    /// 是否已在安全位置（新用户经解析规则直接落固定目录）。
    fn render_exe_relocation_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let risk = khaslana::current_exe_location_risk();
        let risk_text = match risk {
            khaslana::ExeLocationRisk::Volatile => {
                "检测到程序当前位于可能被自动清理的目录（临时目录或聊天软件的接收文件目录）。\
                 这类目录会被系统或聊天软件的清理功能定期清空，届时程序与其中的数据都会丢失。"
            }
            _ => {
                "检测到程序当前位于下载文件夹。下载文件夹可能被系统「存储感知」\
                或清理工具定期清空，届时程序与其中的数据都会丢失。"
            }
        };
        let data_at_risk = khaslana::portable_database_path().is_some_and(|path| path.exists());
        let data_note = if data_at_risk {
            "数据目前也存放在该目录中，强烈建议立即移动。"
        } else {
            "你的数据已保存在安全位置，仅程序本体存在丢失风险。"
        };
        let target_label = khaslana::exe_relocation_target_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "未知".to_string());
        self.dialog_panel("移动到安全目录", cx)
            .w(px(540.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(risk_text),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("{data_note}点击「移动并重启」后应用将关闭，程序与数据会被搬到 {target_label} 并从新位置重新启动。若选择「保持现状」，之后可在「设置」-「更新设置」中手动执行移动。")),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        "保持现状",
                        true,
                        |this, _, _| this.dismiss_exe_relocation(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "移动并重启",
                        true,
                        |this, _, _| this.confirm_exe_relocation(),
                        cx,
                    )),
            )
    }

    fn render_create_branch_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("新建分支", cx)
            .child(self.input(FieldId::BranchName, false, window, cx))
            .child(self.toggle_row(
                "create-branch-checkout",
                "创建成功后切换到新分支",
                self.create_branch_checkout,
                |this, _, _| this.create_branch_checkout = !this.create_branch_checkout,
                cx,
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "创建",
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.create_branch(),
                        cx,
                    )),
            )
    }

    fn render_rename_branch_dialog(
        &self,
        branch: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("重命名分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("当前分支：{branch}")),
            )
            .child(self.input(FieldId::BranchRename, false, window, cx))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "重命名",
                        !self.busy,
                        {
                            let branch = branch.clone();
                            move |this, _, _| this.rename_branch(branch.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_reset_dialog(
        &self,
        oid: String,
        summary: String,
        mode: ResetMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mode_label = reset_mode_label(mode);
        let mode_help = reset_mode_help(mode);
        self.dialog_panel("确认重置分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("将当前分支重置到该提交。{mode_label}：{mode_help}")),
            )
            .when(mode == ResetMode::Hard, |this| {
                this.child(danger_callout(
                    "强制重置会移动当前分支，目标提交之后的已提交代码会从分支历史中移除。确认前请确保目标提交正确。",
                ))
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认重置",
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.reset_to_commit(oid.clone(), mode)
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_revert_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("确认回滚提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("确认后会创建一个新的提交，用于撤销该提交引入的修改。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认回滚",
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.revert_commit(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_revert_merge_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("确认撤销合并提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("确认后会创建一个新的提交，用于撤销这次合并相对主线引入的修改。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("该操作不会删除原合并提交，也不会重写分支历史；若产生冲突，请在冲突解决中心处理后手动提交。"),
            )
            .child(danger_callout(
                "这等价于 git revert -m 1，表示保留合并提交的第一父提交一侧。后续再次合并同一分支时，Git 会认为这次合并的改动曾被主动撤销。",
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认撤销合并",
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.revert_merge_commit(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_uncommit_to_staged_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("确认还原到暂存区", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("确认后会撤销该提交记录，并把该提交引入的修改保留在暂存区。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("该操作只支持当前分支最新且尚未推送的普通提交。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认还原",
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.uncommit_to_staged(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_amend_pushed_dialog(
        &self,
        and_push: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("修补已推送的提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("当前最新提交已推送到远端。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("修补会重写这条提交，之后必须用强制推送才能覆盖远端历史；当前版本暂不支持强推，其他协作者的本地历史会与远端分叉。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("建议仅对尚未推送的提交使用修补。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        if and_push { "仍要修补并推送" } else { "仍要修补" },
                        !self.busy,
                        move |this, _, _| {
                            let message = this.commit_message.value.trim().to_string();
                            if and_push {
                                let Some(remote) = this.current_remote() else {
                                    this.last_error = Some("当前仓库没有远端".into());
                                    return;
                                };
                                this.perform_amend_and_push(message, remote);
                            } else {
                                this.perform_amend(message);
                            }
                        },
                        cx,
                    )),
            )
    }

    /// 创建标签对话框：名称 + 附注开关 + 附注信息（多行）+ 目标提交展示。
    fn render_tag_form_dialog(
        &self,
        target_oid: Option<String>,
        target_summary: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target_label = match (&target_oid, &target_summary) {
            (Some(oid), summary) => {
                format!("目标提交：{} {}", short_oid(oid), summary)
            }
            (None, _) => "目标提交：HEAD（当前分支最新提交）".to_string(),
        };
        self.dialog_panel("创建标签", cx)
            .child(self.input(FieldId::TagName, false, window, cx))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(target_label),
            )
            .child(self.toggle_row(
                "tag-annotated-toggle",
                "创建附注标签（记录标签信息与创建者，发布推荐）",
                self.tag_annotated,
                |this, _, _| this.tag_annotated = !this.tag_annotated,
                cx,
            ))
            .when(self.tag_annotated, |this| {
                this.child(self.input(FieldId::TagMessage, false, window, cx))
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "创建",
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.create_tag(),
                        cx,
                    )),
            )
    }

    /// 推送标签对话框：选择远端后推送。
    fn render_tag_push_dialog(
        &self,
        tag: String,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        let selected_remote = self
            .tag_push_remote
            .clone()
            .or_else(|| remotes.first().map(|remote| remote.name.clone()));
        let options = remotes
            .iter()
            .map(|remote| {
                select_option()
                    .value(remote.name.clone())
                    .label(remote.name.clone())
            })
            .collect::<Vec<_>>();
        let entity = cx.entity();
        self.dialog_panel("推送标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("标签：{tag}")),
            )
            .child(
                div().w_full().text_size(px(12.0)).child(
                    select("tag-push-remote-select")
                        .w_full()
                        .h(px(34.0))
                        .options(options)
                        .placeholder("选择远端")
                        .value(selected_remote.unwrap_or_default())
                        .disabled(remotes.is_empty() || self.busy)
                        .menu_width(px(320.0))
                        .on_change(move |value, _window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.tag_push_remote = Some(value.to_string());
                                cx.notify();
                            });
                        }),
                ),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "推送",
                        !remotes.is_empty() && !self.busy,
                        {
                            let tag = tag.clone();
                            move |this, _, _| this.push_tag(tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_tag_dialog(
        &self,
        tag: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("标签：{tag}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("确认后删除本地标签，不影响远端标签。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        {
                            let tag = tag.clone();
                            move |this, _, _| this.delete_tag(tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_remote_tag_dialog(
        &self,
        remote: String,
        tag: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除远端标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("远端标签：{remote}/{tag}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("确认后从远端删除该标签，已发布的版本引用将不可再用，删除后无法恢复。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        {
                            let remote = remote.clone();
                            let tag = tag.clone();
                            move |this, _, _| this.delete_remote_tag(remote.clone(), tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_discard_change_dialog(
        &self,
        scope: DiffScope,
        target: DiscardTarget,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = paths.len();
        let target_label = match target {
            DiscardTarget::Single => "目标文件".to_string(),
            DiscardTarget::Selected => format!("选定文件（{count} 个）"),
            DiscardTarget::All => match scope {
                DiffScope::Staged => format!("暂存区全部文件（{count} 个）"),
                DiffScope::Unstaged => format!("修改区全部文件（{count} 个）"),
            },
        };
        let preview = discard_paths_preview(&paths);
        let help = match scope {
            DiffScope::Staged => {
                "将丢弃这些文件全部未提交更改，包括暂存区和工作区。新增文件会被删除，删除文件会被恢复。此操作无法从 Khaslana 内撤销。"
            }
            DiffScope::Unstaged => {
                "将仅丢弃这些文件尚未暂存的更改，已暂存内容会保留。未跟踪新增文件会被删除。此操作无法从 Khaslana 内撤销。"
            }
        };
        self.dialog_panel("确认回滚更改", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(target_label),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(preview),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(help),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认回滚",
                        !self.busy,
                        {
                            let paths = paths.clone();
                            move |this, _, _| {
                                this.discard_change(paths.clone(), scope.clone(), target.clone())
                            }
                        },
                        cx,
                    )),
            )
    }

    /// AI 合并建议覆盖确认：草稿已有块处理或手工编辑时，确认后才生成并覆盖。
    fn render_confirm_ai_conflict_merge_dialog(
        &self,
        path: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("覆盖现有冲突处理？", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(path.clone()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("该文件的草稿已有块处理或手工修改，生成 AI 合并建议会覆盖这些内容。"),
            )
            .child(danger_callout(
                "覆盖后已接受的块、忽略操作和手工编辑都会丢失，需要重新处理。",
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        "返回保留现状",
                        !self.busy,
                        |this, _, _| this.close_dialog(),
                        cx,
                    ))
                    .child(self.danger_button(
                        "覆盖并生成",
                        !self.busy,
                        move |this, _, _| {
                            this.close_dialog();
                            this.start_ai_conflict_merge(path.clone());
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_conflict_resolve_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.conflict_workbench.pending_resolve.clone();
        let unresolved_count = pending
            .as_ref()
            .map(|item| item.unresolved_count)
            .unwrap_or(0);
        let path = pending
            .as_ref()
            .map(|item| item.path.clone())
            .unwrap_or_else(|| "当前冲突文件".to_string());

        self.dialog_panel("仍有未处理代码块", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(path),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!(
                        "还有 {unresolved_count} 个代码块未处理，是否继续标记已解决？"
                    )),
            )
            .child(danger_callout(
                "继续后会直接把当前结果写入工作区并从索引中移除冲突标记。",
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        "返回继续处理",
                        !self.busy,
                        |this, _, _| this.cancel_pending_conflict_resolve(),
                        cx,
                    ))
                    .child(self.danger_button(
                        "继续解决",
                        !self.busy,
                        |this, _, _| this.confirm_pending_conflict_resolve(),
                        cx,
                    )),
            )
    }

    // ── 更新对话框渲染 ──────────────────────────────────────────────────

    fn render_update_settings_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let auto_check = self.update_preferences.auto_check;
        let include_beta = self.update_preferences.include_beta;
        let skipped = self.update_preferences.skipped_version.clone();
        // 新版本卡片数据：版本号 / 发布时间 / 版本说明 / 包大小。
        let available_update_display = self.available_update.as_ref().map(|manifest| {
            let size = manifest
                .platforms
                .get("windows-x86_64")
                .map(|asset| format_byte_size(asset.size))
                .unwrap_or_default();
            let notes = if manifest.notes.trim().is_empty() {
                "（此版本未附版本说明）".to_string()
            } else {
                manifest.notes.clone()
            };
            (
                manifest.version.clone(),
                manifest.published_at.clone(),
                notes,
                size,
            )
        });
        // 仅当存在可迁移的旧库时，在更新设置中常驻「迁移到便携目录」入口；
        // dismiss 标记只抑制启动时的自动弹窗，不影响此处手动入口。
        let migration_available = self.portable_migration_available();
        let current_db_label = khaslana::default_database_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "未知".to_string());
        let migrate_btn = self.primary_button(
            "迁移到便携目录",
            true,
            |this, _, _| {
                this.active_dialog = Some(DialogState::PortableMigrationPrompt);
            },
            cx,
        );
        // 程序位于临时/聊天软件接收/下载目录时，常驻「移动到安全目录」入口；
        // dismiss 标记只抑制启动时的自动弹窗，不影响此处手动入口。
        let relocation_available = self.exe_relocation_available();
        // 第一个 when 闭包会 move current_db_label，这里单独克隆一份。
        let relocation_db_label = current_db_label.clone();
        let relocate_btn = self.primary_button(
            "移动到安全目录",
            true,
            |this, _, _| {
                this.active_dialog = Some(DialogState::ExeRelocationPrompt);
            },
            cx,
        );

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("当前版本"),
                    )
                    .child(
                        div()
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .child(
                div()
                    .id("auto_check_update")
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.update_preferences.auto_check = !this.update_preferences.auto_check;
                        this.save_update_preferences();
                        cx.notify();
                    }))
                    .child(toggle_box(auto_check))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("自动检查更新"),
                    ),
            )
            // 测试版（Beta）更新渠道：勾选后检测/安装所有版本（含预发布），
            // 未勾选只走正式版清单（与旧版本行为一致）。切换后下次检查生效。
            .child(
                div()
                    .id("include_beta_updates")
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.update_preferences.include_beta =
                            !this.update_preferences.include_beta;
                        this.save_update_preferences();
                        cx.notify();
                    }))
                    .child(toggle_box(include_beta))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child("接收测试版（Beta）更新"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("勾选后同时检测并安装测试版；测试版可能不稳定"),
                            ),
                    ),
            )
            // 检测到新版本：页内常驻展示版本信息 + 版本说明 + 立即更新入口。
            .when_some(available_update_display, |container, update| {
                let (update_version, update_published_at, update_notes, update_size) = update;
                container.child(
                    div()
                        .id("available-update-card")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p(px(ui_theme::SPACE_3))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .border_1()
                        .border_color(rgb(ui_theme::PRIMARY))
                        .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .text_size(px(12.0))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child(format!("发现新版本 v{update_version}")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_size(px(11.0))
                                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                        .child(format!("发布于 {update_published_at}"))
                                        .child(format!("包大小 {update_size}")),
                                ),
                        )
                        .child(
                            div()
                                .id("available-update-notes")
                                .max_h(px(180.0))
                                .overflow_y_scroll()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(ui_theme::FOREGROUND))
                                // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                                .children(
                                    update_notes
                                        .lines()
                                        .map(|line| div().child(if line.is_empty() { " ".to_string() } else { line.to_string() })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.primary_button(
                                    "立即更新",
                                    !self.update_downloading,
                                    |this, _window, cx| {
                                        this.start_update_download();
                                        cx.notify();
                                    },
                                    cx,
                                ))
                                .when(self.update_downloading, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                            .child(
                                                self.update_download_progress
                                                    .clone()
                                                    .unwrap_or_else(|| "准备下载...".into()),
                                            ),
                                    )
                                }),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("已跳过版本"),
                    )
                    .child(
                        div().text_color(rgb(ui_theme::MUTED_FOREGROUND)).child(
                            skipped
                                .map(|v| format!("v{v}"))
                                .unwrap_or_else(|| "无".to_string()),
                        ),
                    ),
            )
            .when(migration_available, move |container| {
                container.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.0))
                        .child(
                            div()
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .child("数据目录"),
                        )
                        .child(
                            div()
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(format!("当前：{current_db_label}")),
                        )
                        .child(
                            div()
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(
                                    "可将数据从 C 盘系统目录迁移到程序所在目录，便于整体备份并减少 C 盘占用。",
                                ),
                        )
                        .child(migrate_btn),
                )
            })
            .when(relocation_available, move |container| {
                container.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.0))
                        .child(
                            div()
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .child("程序位置"),
                        )
                        .child(
                            div()
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(format!("当前：{relocation_db_label}")),
                        )
                        .child(
                            div().text_color(rgb(ui_theme::MUTED_FOREGROUND)).child(
                                "程序当前位于可能被清理的目录（临时/聊天软件接收/下载目录），\
                                 建议把程序与数据移动到独立的安全目录。",
                            ),
                        )
                        .child(relocate_btn),
                )
            })
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        "立即检查",
                        !self.update_checking && !self.busy,
                        |this, _, _| this.start_update_check(true),
                        cx,
                    ))
                    .child(self.button(
                        "清除跳过",
                        self.update_preferences.skipped_version.is_some(),
                        |this, _, _| this.clear_skipped_version(),
                        cx,
                    )),
            )
    }

    /// 「关于」页：无设置项，展示当前版本号、发布渠道与版本说明
    /// （版本说明由发版流水线经 KHASLANA_RELEASE_NOTES 编译期嵌入；本地
    /// 开发构建或未配置时显示占位文案）。
    fn render_about_settings(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION");
        let channel = update::current_channel();
        let notes = update::current_release_notes();
        let notes_display = if notes.trim().is_empty() {
            "（此版本未附版本说明）".to_string()
        } else {
            notes.to_string()
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_PAGE_TITLE))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(format!("Khaslana v{version}")),
                    )
                    .child(
                        // 渠道徽标：按版本号预发布段推断（正式版 / 测试版）。
                        div()
                            .id("about-channel-badge")
                            .flex_none()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(ui_theme::RADIUS_PILL))
                            .bg(rgb(if channel == "测试版" {
                                ui_theme::FEEDBACK_WARNING_BG
                            } else {
                                ui_theme::PRIMARY_SUBTLE
                            }))
                            .text_size(px(10.0))
                            .text_color(rgb(if channel == "测试版" {
                                ui_theme::FEEDBACK_WARNING_TEXT
                            } else {
                                ui_theme::PRIMARY
                            }))
                            .child(channel),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("版本说明"),
                    )
                    .child(
                        div()
                            .id("about-release-notes")
                            .max_h(px(280.0))
                            .overflow_y_scroll()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                            .children(notes_display.lines().map(|line| {
                                div().child(if line.is_empty() {
                                    " ".to_string()
                                } else {
                                    line.to_string()
                                })
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("更新渠道与自动检查可在「更新设置」中配置"),
            )
    }

    fn render_new_version_dialog(
        &self,
        version: &str,
        notes: &str,
        published_at: &str,
        size: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let size_mb = size as f64 / 1_048_576.0;
        let version_owned = version.to_string();

        self.dialog_panel(format!("发现新版本 v{version}"), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("发布于 {published_at}")),
            )
            .child(
                // 版本说明：多行可滚动（内容多时可翻看，不再 120px 硬截断）。
                div()
                    .id("new-version-notes")
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                    .children(notes.lines().map(|line| {
                        div().child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    })),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("包大小：{:.1} MB", size_mb)),
            )
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        "立即更新",
                        !self.update_downloading && !self.busy,
                        |this, _, _| {
                            this.active_dialog = None;
                            this.start_update_download();
                        },
                        cx,
                    ))
                    .child(self.button(
                        "跳过此版本",
                        !self.update_downloading,
                        move |this, _, _| this.skip_version(&version_owned),
                        cx,
                    ))
                    .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_confirm_install_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staging_dir = self.staging_dir_for_install.clone();
        let version_owned = version.to_string();

        self.dialog_panel("更新准备就绪", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!(
                        "版本 v{version} 已下载并校验通过，应用将重启以完成安装。"
                    )),
            )
            .child(danger_callout(
                "安装过程中应用会自动退出并重启，请确保没有未保存的工作。",
            ))
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        "立即重启",
                        true,
                        move |this, _, cx| {
                            if let Some(dir) = staging_dir.clone() {
                                this.install_update(&dir, &version_owned, cx);
                            } else {
                                this.update_error = Some("staging 目录丢失".into());
                            }
                        },
                        cx,
                    ))
                    .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_no_write_permission_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("无法自动更新", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!(
                        "当前目录没有写入权限，无法自动安装新版本（v{version}）。请手动下载新版本："
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "打开 CNB 下载页",
                        true,
                        |_, _, _| {
                            open_url("https://cnb.cool/suhoan/khaslana-release");
                        },
                        cx,
                    ))
                    .child(self.button(
                        "打开 GitHub Release",
                        true,
                        |_, _, _| {
                            open_url("https://github.com/FuturePrayer/khaslana/releases");
                        },
                        cx,
                    )),
            )
            .child(dialog_actions().child(self.button(
                "关闭",
                true,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }

    fn render_remote_manager_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        let rows = if remotes.is_empty() {
            vec![placeholder_row("暂无远端。可以点击“新增远端”添加。").into_any_element()]
        } else {
            remotes
                .into_iter()
                .map(|remote| self.remote_manager_row(remote, cx).into_any_element())
                .collect::<Vec<_>>()
        };

        div()
            .id("dialog-远端管理")
            .w(px(820.0))
            .max_h(px(620.0))
            .p_4()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .cursor(CursorStyle::Arrow)
            .occlude()
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                if this.mouse_down_inside_context_menu(event) {
                    return;
                }
                if this.credential_context_menu.is_some() {
                    this.credential_context_menu = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("远端管理"),
                    )
                    .child(self.primary_button(
                        "新增远端",
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.open_remote_form(None),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("远端地址会同时作为 fetch 和 push URL；凭据只从已保存凭据中选择。"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h(px(0.0))
                    .max_h(px(420.0))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .rounded_sm()
                    .child(self.remote_manager_header())
                    .child({
                        let handle = self.scroll_handle("remote-manager-list");
                        let content = div()
                            .id("remote-manager-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_0()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&handle)
                            .children(rows)
                            .into_any_element();
                        scrollable_frame_when(
                            "remote-manager-list",
                            ScrollbarMode::Vertical,
                            content,
                            handle,
                            self.snapshot
                                .as_ref()
                                .is_some_and(|snapshot| !snapshot.remotes.is_empty()),
                            cx,
                        )
                    }),
            )
            .child(div().flex().justify_end().child(self.button(
                "关闭",
                !self.busy,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }

    fn remote_manager_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(div().flex_none().w(px(104.0)).child("名称"))
            .child(div().flex_1().min_w(px(0.0)).child("地址"))
            .child(div().flex_none().w(px(180.0)).child("凭据"))
            .child(div().flex_none().w(px(106.0)).child("操作"))
    }

    fn remote_manager_row(&self, remote: RemoteInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let edit_name = remote.name.clone();
        let delete_name = remote.name.clone();
        let policy = self
            .repo_path
            .as_ref()
            .map(|repo_path| {
                self.remote_credential_policy_for_remote(repo_path, &remote.name, &remote.url)
            })
            .unwrap_or(RemoteCredentialPolicy::AutoMatch);
        let credential_label = match policy {
            RemoteCredentialPolicy::NoCredential => "无凭据".to_string(),
            RemoteCredentialPolicy::Record(record_id) => self
                .credential_records
                .iter()
                .find(|record| record.id == record_id)
                .map(credential_record_label)
                .unwrap_or_else(|| "凭据不存在".to_string()),
            RemoteCredentialPolicy::AutoMatch => self
                .matching_credential_for_remote_url(&remote.url)
                .map(|record| format!("自动：{}", credential_record_label(record)))
                .unwrap_or_else(|| "自动匹配".to_string()),
        };

        div()
            .id(format!("remote-manager-row-{}", remote.name))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .child(
                div()
                    .flex_none()
                    .w(px(104.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(remote.name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(remote.url),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(180.0))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(credential_label),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(106.0))
                    .flex()
                    .gap_1()
                    .child(self.button(
                        "编辑",
                        !self.busy,
                        move |this, _, _| this.open_remote_form(Some(edit_name.clone())),
                        cx,
                    ))
                    .child(self.danger_button(
                        "删除",
                        !self.busy,
                        move |this, _, _| this.open_delete_remote_confirm(delete_name.clone()),
                        cx,
                    )),
            )
    }

    fn render_remote_form_dialog(
        &self,
        editing: Option<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = if editing.is_some() {
            "编辑远端"
        } else {
            "新增远端"
        };
        self.dialog_panel(title, cx)
            .w(px(560.0))
            .child(self.input(FieldId::RemoteName, false, window, cx))
            .child(self.input(FieldId::RemoteUrl, false, window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("绑定凭据"),
                    )
                    .child(self.remote_credential_picker(cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        move |this, _, _| this.save_remote(editing.clone()),
                        cx,
                    )),
            )
    }

    fn remote_credential_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.remote_url.value.trim().to_string();
        let mut rows = Vec::new();
        rows.push(
            self.remote_credential_option(
                RemoteCredentialPolicy::AutoMatch,
                "自动匹配保存凭据".to_string(),
                true,
                cx,
            )
            .into_any_element(),
        );
        rows.push(
            self.remote_credential_option(
                RemoteCredentialPolicy::NoCredential,
                "无凭据".to_string(),
                true,
                cx,
            )
            .into_any_element(),
        );
        rows.extend(
            self.credential_records
                .iter()
                .cloned()
                .map(|record| {
                    let compatible = if url.is_empty() {
                        true
                    } else {
                        match record.scope {
                            CredentialScope::RemoteUrl => {
                                credential_record_is_compatible_with_url(&record, &url)
                            }
                            CredentialScope::Host => {
                                credential_record_matches_remote_url(&record, &url)
                            }
                        }
                    };
                    let mut label = credential_record_label(&record);
                    if record.scope == CredentialScope::Host {
                        label = format!("{label} ({})", credential_display_target(&record));
                    }
                    if !compatible {
                        label.push_str("（不匹配）");
                    }
                    self.remote_credential_option(
                        RemoteCredentialPolicy::Record(record.id),
                        label,
                        compatible,
                        cx,
                    )
                    .into_any_element()
                })
                .collect::<Vec<_>>(),
        );

        div()
            .flex()
            .flex_col()
            .max_h(px(168.0))
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded_sm()
            .bg(rgb(ui_theme::CARD))
            .children(rows)
    }

    fn remote_credential_option(
        &self,
        policy: RemoteCredentialPolicy,
        label: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.remote_credential_policy == policy;
        let id_label = match &policy {
            RemoteCredentialPolicy::AutoMatch => "auto",
            RemoteCredentialPolicy::NoCredential => "none",
            RemoteCredentialPolicy::Record(record_id) => record_id.as_str(),
        };
        div()
            .id(format!("remote-credential-option-{id_label}"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::CARD)
            })
            .text_size(px(12.0))
            .text_color(if !enabled {
                rgb(ui_theme::MUTED_FOREGROUND)
            } else if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::FOREGROUND)
            })
            .cursor_pointer()
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            })
            .child(
                div()
                    .flex_none()
                    .size(px(10.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .bg(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::CARD)
                    }),
            )
            .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.remote_credential_policy = policy.clone();
                    cx.notify();
                }
            }))
    }

    fn render_confirm_delete_remote_dialog(
        &self,
        name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除远端", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除远端：{name}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("这只会删除当前仓库的远端配置，不会删除任何已保存凭据。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        move |this, _, _| this.delete_remote(name.clone()),
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_remote_branch_dialog(
        &self,
        remote: String,
        branch: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let full_name = format!("{remote}/{branch}");
        self.dialog_panel("删除远端分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除远端分支：{full_name}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(
                        "这会删除远端仓库上的分支，并刷新本地远端分支列表；不会删除同名本地分支。",
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        move |this, _, _| this.delete_remote_branch(remote.clone(), branch.clone()),
                        cx,
                    )),
            )
    }

    /// OAuth 品牌矩形按钮：显示带文字的品牌 logo（按当前主题选浅/深变体），点击触发登录。
    /// 两个按钮用统一固定宽度，避免 logo 长宽比不同导致不等宽。
    fn oauth_brand_button(
        &self,
        brand: OauthBrand,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon_h = 18.0_f32;
        let icon_w = icon_h * brand.aspect();
        // 按当前主题取适配的 logo 变体。GPUI 的 svg() 只能按 text_color 做单色 alpha 蒙版，
        // 会丢掉品牌色（Gitee 红、彩色文字）；改用 img() 走 render_single_frame，保留 SVG 原始 fill。
        div()
            .id(brand.id_str())
            .flex()
            .items_center()
            .justify_center()
            .w(px(140.0))
            .h(px(36.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                img(brand.lockup_path())
                    .h(px(icon_h))
                    .w(px(icon_w))
                    .flex_none(),
            )
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        on_click(this, window, cx);
                        cx.notify();
                    }))
            })
            .when(!enabled, |this| this.opacity(0.5))
    }

    /// OAuth 快速登录区：品牌按钮 + 进行中的验证码/取消面板 + 错误提示。
    fn render_oauth_login_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let flow = &self.oauth_login_flow;
        let github_enabled = !flow.loading && !self.busy;
        let gitee_enabled = github_enabled && oauth::is_gitee_configured();
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("快速登录"),
                    )
                    .child(self.oauth_brand_button(
                        OauthBrand::Github,
                        github_enabled,
                        |this, _, _| this.start_github_login(),
                        cx,
                    ))
                    .child(self.oauth_brand_button(
                        OauthBrand::Gitee,
                        gitee_enabled,
                        |this, _, _| this.start_gitee_login(),
                        cx,
                    )),
            );

        if flow.loading {
            let provider_label = flow.provider.map(|p| p.label()).unwrap_or("OAuth");
            if let Some(code) = flow.user_code.clone() {
                // GitHub Device Flow：显示用户验证码 + 取消。
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(format!(
                                    "已在浏览器打开{provider_label}，请确认验证码后完成登录："
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .font_family("Consolas, monospace")
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child(code),
                                )
                                .child(self.button(
                                    "取消",
                                    !self.busy,
                                    |this, _, _| this.cancel_oauth_login(),
                                    cx,
                                )),
                        ),
                );
            } else {
                // Gitee 授权码流（或设备码尚未到位）：等待浏览器授权。
                panel = panel.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(format!("请在浏览器中完成{provider_label}登录...")),
                );
            }
        } else if !oauth::is_gitee_configured() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(
                        "Gitee 登录需由维护者部署令牌交换服务（见 AGENTS.md）；GitHub 可直接使用。",
                    ),
            );
        }

        if let Some(error) = flow.error.clone() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::DESTRUCTIVE))
                    .child(error),
            );
        }

        panel
    }

    /// 凭据测试地址确认弹窗：预填记录远端地址；说明裸主机地址的局限。
    fn render_test_credential_dialog(
        &self,
        record_id: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record_label = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .map(|record| {
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.username.clone())
            })
            .unwrap_or_else(|| "未知记录".to_string());
        let testing = self.busy || self.global_busy_tab.is_some();
        self.dialog_panel("测试凭据连接", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("凭据：{record_label}")),
            )
            .child(self.input(FieldId::CredentialTestUrl, false, window, cx))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(
                        "将使用此地址发起一次 Git 连接验证凭据。建议填写真实仓库地址；                         裸站点地址（如 https://gitee.com）可能因服务器不发起认证而无法验证凭据。",
                    ),
            )
            .when_some(self.credential_test_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(error),
                )
            })
            .child(
                dialog_actions()
                    .child(
                        self.button("取消", true, |this, _, _| this.close_dialog(), cx),
                    )
                    .child(self.primary_button(
                        "开始测试",
                        !testing,
                        |this, _, _| this.confirm_test_credential(),
                        cx,
                    )),
            )
    }

    fn render_credential_form_dialog(
        &self,
        _editing: Option<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("添加凭据", cx)
            .w(px(680.0))
            .max_h(px(760.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("类型"),
                    )
                    .child(self.credential_kind_button("HTTPS", CredentialFormMode::Https, cx))
                    .child(self.credential_kind_button("SSH", CredentialFormMode::Ssh, cx)),
            )
            .child(self.input(FieldId::CredentialDisplayName, false, window, cx))
            .child(self.input(FieldId::CredentialRemoteUrl, false, window, cx))
            .child(self.input(FieldId::CredentialUsername, false, window, cx))
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.input(FieldId::CredentialSecret, false, window, cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.render_oauth_login_section(cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Ssh,
                |this| {
                    this.child(self.render_ssh_credential_discovery(cx))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("推荐优先使用 SSH Agent；使用私钥文件时，应用只保存路径，密码短语仍存入系统 Keyring。"),
                    )
                    .child(self.toggle_row(
                        "credential-form-use-ssh-agent",
                        "使用 SSH Agent（不保存私钥路径）",
                        self.credential_use_ssh_agent,
                        |this, _, _| {
                            this.credential_use_ssh_agent = !this.credential_use_ssh_agent;
                            if this.credential_use_ssh_agent {
                                this.credential_key_path.clear();
                                this.credential_passphrase.clear();
                            }
                        },
                        cx,
                    ))
                    .when(!self.credential_use_ssh_agent, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(self.input(
                                            FieldId::CredentialKeyPath,
                                            false,
                                            window,
                                            cx,
                                        )),
                                )
                                .child(self.button(
                                    "选择私钥文件",
                                    !self.busy,
                                    |this, _, _| this.browse_credential_ssh_key(),
                                    cx,
                                )),
                        )
                        .child(self.input(
                            FieldId::CredentialPassphrase,
                            false,
                            window,
                            cx,
                        ))
                    })
                },
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("复用范围"),
                    )
                    .child(self.credential_scope_button(
                        "仅此远端",
                        CredentialScope::RemoteUrl,
                        true,
                        cx,
                    ))
                    .child(self.credential_scope_button("同站点", CredentialScope::Host, true, cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| this.close_credential_form(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        |this, _, _| this.save_credential_form(),
                        cx,
                    )),
            )
    }

    fn render_credential_manager_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = if self.credential_records.is_empty() {
            vec![
                placeholder_row("暂无已保存凭据。远程操作时勾选保存后会出现在这里。")
                    .into_any_element(),
            ]
        } else {
            self.credential_records
                .iter()
                .cloned()
                .map(|record| self.credential_record_row(record, cx).into_any_element())
                .collect::<Vec<_>>()
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div().flex().items_center().justify_between().child(
                    div()
                        .flex()
                        .gap_2()
                        .child(self.primary_button(
                            "添加凭据",
                            !self.busy,
                            |this, _, _| this.open_credential_form(),
                            cx,
                        ))
                        .child(self.button(
                            "刷新",
                            !self.busy,
                            |this, _, _| this.reload_credential_records("凭据列表已刷新"),
                            cx,
                        )),
                ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(
                        "密文仅保存在系统凭据管理器；这里不显示、不复制密码、PAT 或 SSH 密码短语。",
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .max_h(px(440.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .rounded_sm()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.credential_manager_header())
                    .child({
                        let handle = self.scroll_handle("credential-record-list");
                        let content = div()
                            .id("credential-record-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .w_full()
                            .gap_0()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&handle)
                            .children(rows)
                            .into_any_element();
                        scrollable_frame_when(
                            "credential-record-list",
                            ScrollbarMode::Vertical,
                            content,
                            handle,
                            !self.credential_records.is_empty(),
                            cx,
                        )
                    }),
            )
    }

    fn render_credential_details_dialog(
        &self,
        record_id: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            return self
                .dialog_panel("凭据详情", cx)
                .w(px(560.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("凭据记录不存在，可能已经被删除。"),
                )
                .child(div().flex().justify_end().child(self.button(
                    "关闭",
                    !self.busy,
                    |this, _, _| this.open_credential_manager(),
                    cx,
                )));
        };

        let display_name = record
            .display_name
            .clone()
            .unwrap_or_else(|| credential_record_label(&record));
        let target = credential_display_target(&record);
        let key_path = record.key_path.clone().unwrap_or_else(|| "-".to_string());
        let last_used = record
            .last_used
            .map(timestamp_label)
            .unwrap_or_else(|| "-".to_string());

        self.dialog_panel("凭据详情", cx)
            .w(px(640.0))
            .max_h(px(620.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.0))
                    .child(self.credential_detail_row("名称", display_name))
                    .child(self.credential_detail_row(
                        "类型",
                        credential_kind_label(record.kind).to_string(),
                    ))
                    .child(self.credential_detail_row(
                        "复用范围",
                        credential_scope_label(record.scope).to_string(),
                    ))
                    .child(self.credential_detail_row("站点 / 远端", target))
                    .child(self.credential_detail_row("用户名", record.username))
                    .child(self.credential_detail_row("SSH Key 路径", key_path))
                    .child(
                        self.credential_detail_row("创建时间", timestamp_label(record.created_at)),
                    )
                    .child(
                        self.credential_detail_row("更新时间", timestamp_label(record.updated_at)),
                    )
                    .child(self.credential_detail_row("最后使用时间", last_used)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("密码、PAT 和 SSH 密码短语不会在这里显示。"),
            )
            .child(div().flex().justify_end().child(self.button(
                "关闭",
                !self.busy,
                |this, _, _| this.open_credential_manager(),
                cx,
            )))
    }

    fn credential_detail_row(&self, label: &'static str, value: String) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .w(px(96.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(value),
            )
    }

    fn credential_manager_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(div().flex_none().w(px(112.0)).truncate().child("名称"))
            .child(div().flex_none().w(px(88.0)).truncate().child("类型"))
            .child(div().flex_none().w(px(64.0)).truncate().child("范围"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .child("站点 / 远端"),
            )
            .child(div().flex_none().w(px(72.0)).truncate().child("用户名"))
            .child(div().flex_none().w(px(68.0)).truncate().child("SSH Key"))
            .child(div().flex_none().w(px(108.0)).truncate().child("更新时间"))
            .child(div().flex_none().w(px(112.0)).truncate().child("操作"))
    }

    fn credential_record_row(
        &self,
        record: CredentialRecord,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record_id = record.id.clone();
        let detail_id = record.id.clone();
        let menu_id = record.id.clone();
        let delete_id = record.id.clone();
        let actions_id = record.id.clone();
        let label = credential_record_label(&record);
        let target = credential_display_target(&record);
        let key_file = credential_key_filename(&record);
        let display_name = record.display_name.clone().unwrap_or_else(|| label.clone());
        div()
            .id(format!("credential-record-{}", record.id))
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_credential_details(detail_id.clone());
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.open_credential_context_menu(menu_id.clone(), event, window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(format!("credential-record-actions-{actions_id}"))
                    .flex_none()
                    .w(px(112.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(display_name),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(88.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(credential_kind_label(record.kind)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(64.0))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(credential_scope_label(record.scope)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(target),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(72.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(record.username),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(68.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(key_file),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(108.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(timestamp_label(record.updated_at)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(112.0))
                    .flex()
                    .justify_end()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.button(
                        "测试",
                        !self.busy,
                        move |this, _, cx| {
                            cx.stop_propagation();
                            this.open_test_credential_dialog(record_id.clone());
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "删除",
                        !self.busy,
                        move |this, _, cx| {
                            cx.stop_propagation();
                            this.open_delete_credential_confirm(delete_id.clone(), label.clone());
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_credential_dialog(
        &self,
        record_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除凭据", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除凭据：{label}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("删除会同时移除非敏感索引和系统凭据管理器中的密文。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.open_credential_manager();
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        move |this, _, _| this.delete_credential_record(record_id.clone()),
                        cx,
                    )),
            )
    }

    pub(crate) fn dialog_panel(
        &self,
        title: impl Into<gpui::SharedString>,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        ui_dialog_panel(title)
    }
}

impl Deref for RepositoryView {
    type Target = RepoTabState;

    fn deref(&self) -> &Self::Target {
        self.active_tab_state()
    }
}

impl DerefMut for RepositoryView {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.active_tab_state_mut()
    }
}

impl Render for RepositoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drain_pending_events(cx);
        // 工作流模板编辑器：渲染前确保当前展示的文本框已创建
        //（field_mut 无 cx 不能惰性建框，text_input 的 paint 路径依赖它已存在）。
        self.ensure_workflow_editor_fields_inited(window, cx);
        let shell_policy = self.shell_layout_policy(window);
        let shell_content_height =
            chrome_view::shell_content_height(window.viewport_size().height.into());
        let context_toggle_is_overlay = shell_policy.band == chrome_view::LayoutBand::Narrow;
        // 窄窗覆盖态不写回停靠偏好；窗口恢复到标准宽度后立即清理，避免继续阻断分割线。
        if !context_toggle_is_overlay {
            self.context_navigator_overlay_open = false;
        }
        // 对话框/设置中心带自身 overlay；不允许遗留无遮罩菜单或导航覆盖层于其下方。
        if self.active_dialog.is_some() || self.settings_center.is_some() {
            self.context_navigator_overlay_open = false;
        }
        let context_presentation = self.context_navigator_presentation(window);

        app_shell_surface()
            .id("app-root")
            .relative()
            .flex()
            .flex_col()
            .text_color(rgb(ui_theme::FOREGROUND))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                this.encoding_menu_closed_by_capture = None;
                this.commit_graph_branch_menu_closed_by_capture = false;
                if this.mouse_down_inside_context_menu(event) {
                    return;
                }
                if this.branch_context_menu.is_some()
                    || this.remote_context_menu.is_some()
                    || this.change_context_menu.is_some()
                    || this.file_path_context_menu.is_some()
                    || this.credential_context_menu.is_some()
                    || this.tag_context_menu.is_some()
                    || this.stash_context_menu.is_some()
                    || this.commit_context_menu.is_some()
                    || this.workflow_template_context_menu.is_some()
                    || this.encoding_menu_target.is_some()
                    || this.repo_switcher_menu.is_some()
                    || this.commit_graph.branch_menu_open
                {
                    let closed_encoding_menu = this.encoding_menu_target;
                    let closed_branch_menu = this.commit_graph.branch_menu_open;
                    this.branch_context_menu = None;
                    this.remote_context_menu = None;
                    this.change_context_menu = None;
                    this.file_path_context_menu = None;
                    this.credential_context_menu = None;
                    this.tag_context_menu = None;
                    this.stash_context_menu = None;
                    this.commit_context_menu = None;
                    this.workflow_template_context_menu = None;
                    this.encoding_menu_target = None;
                    this.encoding_menu_closed_by_capture = closed_encoding_menu;
                    this.commit_graph.branch_menu_open = false;
                    this.commit_graph_branch_search.clear();
                    this.commit_graph_branch_menu_closed_by_capture = closed_branch_menu;
                    this.close_repo_switcher();
                    cx.notify();
                }
            }))
            // 录制态时在 capture 阶段截获全部按键：stop_propagation 阻止 action dispatch，
            // 使快捷键录制逻辑能在按键到达 action listener 之前处理；
            // capture 从根向下传播，不依赖焦点路径。
            .when(self.recording_shortcut.is_some(), |this| {
                this.capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                    // Esc 取消录制。
                    if event.keystroke.key.as_str() == "escape" {
                        this.recording_shortcut = None;
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    if let Some(action) = this.recording_shortcut {
                        let ks = shortcuts_view::keystroke_to_string(event);
                        // 冲突检查：若已被其它动作占用则拒绝并提示。
                        if let Some(conflict) =
                            find_shortcut_conflict(&this.shortcut_bindings, action, &ks)
                        {
                            this.recording_shortcut = None;
                            this.notify_warning(
                                format!(
                                    "快捷键 {} 已被「{}」占用",
                                    shortcuts_view::format_keystroke(&ks),
                                    conflict.label()
                                ),
                                cx,
                            );
                        } else {
                            // 通过检查，更新绑定。
                            this.shortcut_bindings
                                .bindings
                                .insert(action.action_id().to_string(), ks);
                            this.recording_shortcut = None;
                            this.save_shortcut_bindings();
                            crate::register_all_key_bindings(
                                &mut cx.deref_mut(),
                                &this.shortcut_bindings,
                                false,
                            );
                            cx.notify();
                        }
                    }
                    cx.stop_propagation();
                }))
            })
            .child(self.render_chrome_titlebar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w_full()
                    .h(px(shell_content_height))
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .relative()
                    // 左侧列：Docked 展开完整导航器（标题 + 模式按钮 + 分组列表）；
                    // 其余情况（收起偏好/窄窗/专用页面）一律渲染 48px 收起窄条
                    // （展开箭头 + 模式图标），模式入口在任何页面都常驻。
                    .child(
                        if context_presentation == chrome_view::ContextNavigatorPresentation::Docked
                        {
                            self.render_context_navigator(window, false, cx)
                                .into_any_element()
                        } else {
                            self.render_navigator_collapsed_strip(context_toggle_is_overlay, cx)
                                .into_any_element()
                        },
                    )
                    .when(
                        context_presentation == chrome_view::ContextNavigatorPresentation::Docked,
                        |this| this.child(self.render_column_splitter(ResizeTarget::Sidebar, cx)),
                    )
                    .child(match self.main_mode {
                        MainMode::Worktree => {
                            self.render_worktree_view(window, cx).into_any_element()
                        }
                        MainMode::Conflict => self
                            .render_conflict_workbench(window, cx)
                            .into_any_element(),
                        MainMode::History => self.render_history_view(cx).into_any_element(),
                        MainMode::Workflow => {
                            self.render_workflow_view(window, cx).into_any_element()
                        }
                        MainMode::Stash => self.render_stash_preview_view(cx).into_any_element(),
                        MainMode::Browse => self.render_browse_view(cx).into_any_element(),
                        MainMode::Blame => self.render_blame_view(cx).into_any_element(),
                        MainMode::CommitGraph => {
                            self.render_commit_graph_view(window, cx).into_any_element()
                        }
                    })
                    // 窄窗 Navigator 覆盖层最后挂载（盖在主体内容之上）。
                    .when(
                        context_presentation == chrome_view::ContextNavigatorPresentation::Overlay,
                        |this| this.child(self.render_context_navigator_overlay(window, cx)),
                    ),
            )
            .child(self.render_status())
            .child(self.render_branch_context_menu(cx))
            .child(self.render_remote_context_menu(cx))
            .child(self.render_change_context_menu(cx))
            .child(self.render_file_path_context_menu(cx))
            .child(self.render_commit_context_menu(cx))
            .child(self.render_tag_context_menu(cx))
            .child(self.render_stash_context_menu(cx))
            .child(self.render_workflow_template_context_menu(cx))
            .child(self.render_repo_switcher_menu(window, cx))
            .child(self.render_settings_center_overlay(window, cx))
            .child(self.render_dialogs(window, cx))
            // AI 思考弹窗：一次性生成类请求的思维链流式展示，层级在
            // 普通对话框之上（工作流编辑器弹窗内触发时覆盖其上）。
            .child(self.render_ai_thinking_overlay(cx))
            .child(self.render_credential_context_menu(cx))
            .child(self.render_operation_blocker())
            .child(self.render_credentials(window, cx))
            .child(self.render_feedback_layer(cx))
    }
}

impl Focusable for RepositoryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.clone_url.focus.clone()
    }
}

impl gpui::EntityInputHandler for RepositoryView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.focused_text_field(window, cx)?;
        let field_state = self.field(field);
        let range = field_state.range_from_utf16(&range_utf16);
        adjusted_range.replace(field_state.range_to_utf16(&range));
        Some(field_state.text_for_utf16_range(&range_utf16))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.focused_text_field(window, cx)?;
        let field_state = self.field(field);
        Some(UTF16Selection {
            range: field_state.range_to_utf16(&field_state.input_range()),
            reversed: field_state.selection_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let field = self.focused_text_field(window, cx)?;
        let field_state = self.field(field);
        field_state
            .marked_range
            .as_ref()
            .map(|range| field_state.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).marked_range = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).replace_text_in_utf16_range_with_mode(
                range_utf16,
                text,
                field == FieldId::CommitMessage,
            );
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field)
                .replace_and_mark_text_in_utf16_range_with_mode(
                    range_utf16,
                    new_text,
                    new_selected_range_utf16,
                    field == FieldId::CommitMessage,
                );
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let field = self.focused_text_field(window, cx)?;
        let field_state = self.field(field);
        field_state.bounds_for_utf16_range(&range_utf16, bounds)
    }

    fn character_index_for_point(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let field = self.focused_text_field(window, cx)?;
        let field_state = self.field(field);
        Some(field_state.offset_to_utf16(field_state.index_for_mouse_position(position)))
    }
}

trait EmptyStringExt {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String;
}

impl EmptyStringExt for String {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String {
        if self.is_empty() { f() } else { self }
    }
}

/// 当前 Unix 时间戳（秒），用于 RepoTabState.last_active_at 等内存排序。
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `normalize_repo_path` 的进程级缓存。canonicalize 是磁盘 IO（Windows 上为
/// 打开文件句柄的 GetFinalPathNameByHandle），仓库切换下拉打开期间每帧都会对
/// 全部 tab + 最近仓库逐个调用，不缓存会造成持续磁盘访问与下拉卡顿。
/// 键为原始路径，条目数以实际访问过的仓库路径为上界，无需淘汰。
static REPO_PATH_CACHE: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

fn normalize_repo_path(path: &Path) -> String {
    let mut guard = REPO_PATH_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(cached) = cache.get(path) {
        return cached.clone();
    }
    let normalized = fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase();
    cache.insert(path.to_path_buf(), normalized.clone());
    normalized
}

fn infer_clone_directory_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .trim_end_matches('/');
    let path_part = if let Some((_, rest)) = without_query.split_once("://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else {
        without_query
    };
    let last_segment = path_part
        .rsplit(['/', ':'])
        .find(|segment| !segment.trim().is_empty())?;
    let name = last_segment
        .strip_suffix(".git")
        .unwrap_or(last_segment)
        .trim();
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|ch| {
            matches!(ch, '<' | '>' | '"' | '|' | '?' | '*' | '\\')
                || ch.is_control()
                || ch == std::path::MAIN_SEPARATOR
        });
    (!invalid).then(|| name.to_string())
}

fn infer_clone_target_path(url: &str, parent_path: &str) -> Option<PathBuf> {
    let parent_path = parent_path.trim();
    if parent_path.is_empty() {
        return None;
    }
    infer_clone_directory_name(url).map(|name| PathBuf::from(parent_path).join(name))
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

fn reset_mode_label(mode: ResetMode) -> &'static str {
    match mode {
        ResetMode::Soft => "软重置",
        ResetMode::Mixed => "混合重置",
        ResetMode::Hard => "强制重置",
    }
}

fn reset_mode_help(mode: ResetMode) -> &'static str {
    match mode {
        ResetMode::Soft => "保留暂存区和工作区修改",
        ResetMode::Mixed => "重置暂存区，保留工作区修改",
        ResetMode::Hard => "重置暂存区和工作区，丢弃未提交修改",
    }
}

pub(crate) fn encoding_info_label(info: &DiffEncodingInfo) -> String {
    let base = if info.requested == DiffEncodingChoice::Auto {
        format!("编码：自动({})", info.resolved.label())
    } else {
        format!("编码：{}", info.requested.label())
    };
    if info.lossy {
        format!("{base}，有替换")
    } else {
        base
    }
}

pub(crate) fn diff_encoding_label(diff: &FileDiff) -> String {
    encoding_info_label(&diff.encoding)
}

fn timestamp_label(seconds: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

fn visual_line_count(value: &str) -> usize {
    value.chars().filter(|ch| *ch == '\n').count() + 1
}

pub(crate) fn clamped_menu_position(
    event: &MouseDownEvent,
    window: &Window,
    width: f32,
    height: f32,
) -> (f32, f32) {
    let position_x: f32 = event.position.x.into();
    let position_y: f32 = event.position.y.into();
    let viewport_size = window.viewport_size();
    context_menu_position(
        position_x,
        position_y,
        f32::from(viewport_size.width),
        f32::from(viewport_size.height),
        width,
        height,
    )
}

fn context_menu_position(
    mouse_x: f32,
    mouse_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    menu_width: f32,
    menu_height: f32,
) -> (f32, f32) {
    let max_x = (viewport_width - menu_width - MENU_VIEWPORT_MARGIN).max(MENU_VIEWPORT_MARGIN);
    let max_y = (viewport_height - menu_height - MENU_VIEWPORT_MARGIN).max(MENU_VIEWPORT_MARGIN);
    let x = if mouse_x + menu_width + MENU_VIEWPORT_MARGIN > viewport_width {
        mouse_x - menu_width
    } else {
        mouse_x
    };
    let y = mouse_y;

    (
        x.clamp(MENU_VIEWPORT_MARGIN, max_x),
        y.clamp(MENU_VIEWPORT_MARGIN, max_y),
    )
}

fn point_in_menu(x: f32, y: f32, menu_x: f32, menu_y: f32, width: f32, height: f32) -> bool {
    x >= menu_x && x <= menu_x + width && y >= menu_y && y <= menu_y + height
}

fn should_notify_operation_finished(message: &str, has_snapshot: bool, has_diff: bool) -> bool {
    !(message == "差异已加载" && !has_snapshot && has_diff)
}

/// 这些操作会改变 HEAD、本地/远端分支引用或 upstream，需要在操作快照之外再完整刷新一次。
fn operation_requires_repository_refresh(message: &str) -> bool {
    matches!(
        message,
        "切换分支完成"
            | "检出标签完成"
            | "远端分支已拉取到本地"
            | "分支已创建"
            | "分支已重命名"
            | "分支已删除"
            | "远端分支已删除"
            | "远端已删除"
            | "拉取远程引用完成"
            | "远端已刷新"
            | "拉取完成"
            | "变基拉取完成"
            | "分支拉取完成"
            | "推送完成"
            | "标签已推送"
            | "远端标签已删除"
            | "upstream 已设置"
    )
}

/// 暂存/取消暂存类操作（整文件或按块/按行，含行内 +/- 按钮路径）：
/// 完成后差异面板需跟随刷新（原位重载或清空），见
/// `RepositoryView::refresh_diff_after_stage_change`。
fn operation_refreshes_worktree_diff(message: &str) -> bool {
    matches!(
        message,
        "暂存"
            | "取消暂存"
            | "已暂存选定文件"
            | "已暂存所有文件"
            | "已取消暂存选定文件"
            | "已取消暂存所有文件"
            | "已暂存选中改动"
            | "已取消暂存选中改动"
    )
}

/// (path, scope) 在变更列表中是否仍有可展示的改动。
/// 操作快照来自 fast 状态（不含未跟踪文件）：scope 为未暂存且路径完全
/// 缺失时视为仍存在——未跟踪文件只出现在未暂存侧，不能因快照缺失被清空。
fn diff_scope_still_present(
    changes: &[khaslana::WorktreeChange],
    path: &str,
    scope: &DiffScope,
) -> bool {
    let mut any_entry = false;
    let mut side_present = false;
    for change in changes {
        if change.path != path {
            continue;
        }
        any_entry = true;
        side_present |= match scope {
            DiffScope::Staged => change.staged.is_some(),
            DiffScope::Unstaged => change.unstaged.is_some(),
        };
    }
    side_present || (matches!(scope, DiffScope::Unstaged) && !any_entry)
}

/// 行索引选择的通用切换语义（差异行选择与变更列表一致）：
/// 普通点击单选（再点同一行取消）、Ctrl/Cmd 切换多选、Shift 从锚点做
/// 范围选择（替换现有选择；无锚点时等价普通选择并记录锚点）。
/// 范围内的上下文行/块头索引不产生实际选择（转换时只取 +/- 行）。
fn toggle_index_selection(
    selection: &mut BTreeSet<usize>,
    anchor: &mut Option<usize>,
    index: usize,
    multi: bool,
    shift: bool,
) {
    if shift {
        let Some(anchor) = *anchor else {
            selection.insert(index);
            *anchor = Some(index);
            return;
        };
        let (lo, hi) = if anchor < index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        selection.clear();
        selection.extend(lo..=hi);
    } else if multi {
        if selection.contains(&index) {
            selection.remove(&index);
        } else {
            selection.insert(index);
            *anchor = Some(index);
        }
    } else if selection.len() == 1 && selection.contains(&index) {
        selection.clear();
    } else {
        selection.clear();
        selection.insert(index);
        *anchor = Some(index);
    }
}

/// 这些操作会创建/移动提交或 HEAD，但不触发完整仓库重载；
/// 完成后需不受当前视图限制地后台刷新提交记录及其 HEAD/分支/标签徽章。
/// 引用类操作（切换分支、拉取、推送等）见 `operation_requires_repository_refresh`，
/// 它们走完整仓库重载路径，由 `RepositoryFastLoaded` 统一刷新历史。
fn operation_affects_commit_history(message: &str) -> bool {
    matches!(
        message,
        "提交完成"
            | "提交并推送完成"
            | "合并操作已完成"
            | "合并已完成"
            | "合并已中止"
            | "变基完成"
            | "变基已中止"
            | "分支已重置"
            | "回滚提交完成"
            | "撤销合并完成"
            | "提交已还原到暂存区"
            | "修补提交完成"
            | "修补提交并推送完成"
            | "拣选提交完成"
            | "标签已创建"
            | "标签已删除"
    )
}

/// 检查更新结果的气泡决策（纯函数，可单测）：手动检查需要即时反馈——
/// 「已是最新」弹成功气泡、真失败弹错误气泡；自动检查每次启动都跑，
/// 弹气泡会打扰，保持安静（发现新版本走弹窗不受此影响）。
fn update_check_toast(error: &str, manual: bool) -> Option<(AppToastKind, String)> {
    if !manual || error.is_empty() {
        return None;
    }
    if error == "当前已是最新版本" {
        Some((AppToastKind::Success, error.to_string()))
    } else {
        Some((AppToastKind::Error, format!("检查更新失败：{error}")))
    }
}

fn dedupe_repo_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(normalize_repo_path(path)))
        .collect()
}

/// 仓库切换下拉顶部的固定功能项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepoSwitcherAction {
    /// 克隆仓库，排第一。
    Clone,
    /// 打开本地仓库，排第二。
    Open,
}

/// 仓库切换下拉里的一个仓库行（“打开项目”或“最近的项目”区共用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoSwitcherRepo {
    /// 归一化路径键，用于元素 id 与去重，不直接展示。
    pub path_key: String,
    /// 显示名（路径末段）。
    pub name: String,
    /// 完整路径，展示于次行；最近项点击时据此重新打开仓库。
    pub full_path: String,
    /// 已打开 tab 的 id；最近项为 None。
    pub tab_id: Option<RepoTabId>,
    /// 是否为当前活动仓库。
    pub active: bool,
}

/// 仓库切换下拉的纯函数输入：一个已打开 tab（归一化键由调用方算好，避免纯函数访问磁盘）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoSwitcherTabInput {
    pub key: String,
    pub name: String,
    pub full_path: String,
    pub last_active: i64,
    pub tab_id: RepoTabId,
}

/// 仓库切换下拉的纯函数输入：一条最近打开记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoSwitcherRecentInput {
    pub key: String,
    pub name: String,
    pub full_path: String,
    pub last_opened: i64,
}

/// 仓库切换下拉的三区结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepoSwitcherSections {
    /// 固定功能区：克隆、打开。
    pub actions: Vec<RepoSwitcherAction>,
    /// “打开项目”区：活动仓库置顶，其余已打开 tab 按最后活动时间倒序。
    pub open: Vec<RepoSwitcherRepo>,
    /// “最近的项目”区：未打开的历史仓库，按最后打开时间倒序。
    pub recent: Vec<RepoSwitcherRepo>,
}

/// 构造仓库切换下拉的三区结构（纯函数，不含磁盘 IO）。
///
/// `active_key` 为当前活动仓库的归一化键；`tabs` 为已打开 tab；`recent` 为最近打开记录
/// （调用方应保证已按 last_opened 倒序）。已打开 tab 与最近记录按归一化键去重，已打开优先，
/// 因此“最近的项目”区只含当前未打开者。
pub(crate) fn build_repo_switcher_sections(
    active_key: Option<&str>,
    mut tabs: Vec<RepoSwitcherTabInput>,
    recent: Vec<RepoSwitcherRecentInput>,
) -> RepoSwitcherSections {
    let actions = vec![RepoSwitcherAction::Clone, RepoSwitcherAction::Open];

    // 活动仓库置顶，其余按最后活动时间倒序。
    tabs.sort_by(|a, b| {
        let a_active = active_key == Some(a.key.as_str());
        let b_active = active_key == Some(b.key.as_str());
        b_active
            .cmp(&a_active)
            .then_with(|| b.last_active.cmp(&a.last_active))
    });
    let open = tabs
        .iter()
        .map(|tab| RepoSwitcherRepo {
            active: active_key == Some(tab.key.as_str()),
            path_key: tab.key.clone(),
            name: tab.name.clone(),
            full_path: tab.full_path.clone(),
            tab_id: Some(tab.tab_id),
        })
        .collect();

    // 最近区排除已打开者，保持 recent 原序（已按时间倒序）。
    let tab_keys: std::collections::HashSet<&str> =
        tabs.iter().map(|tab| tab.key.as_str()).collect();
    let recent = recent
        .into_iter()
        .filter(|item| !tab_keys.contains(item.key.as_str()))
        .map(|item| RepoSwitcherRepo {
            active: false,
            path_key: item.key,
            name: item.name,
            full_path: item.full_path,
            tab_id: None,
        })
        .collect();

    RepoSwitcherSections {
        actions,
        open,
        recent,
    }
}

/// 仓库切换下拉的搜索匹配：query trim + 小写后对名称和完整路径做子串匹配；
/// 空 query 恒匹配（等价于不过滤）。
pub(crate) fn repo_switcher_repo_matches_query(repo: &RepoSwitcherRepo, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    repo.name.to_lowercase().contains(&query) || repo.full_path.to_lowercase().contains(&query)
}

/// 名称是否命中搜索词（用于排序：名称命中排在仅路径命中的前面）。
pub(crate) fn repo_switcher_repo_name_matches_query(repo: &RepoSwitcherRepo, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    !query.is_empty() && repo.name.to_lowercase().contains(&query)
}

/// 按搜索词过滤仓库切换下拉的打开/最近两区，区内名称命中排在仅路径命中之前
///（稳定排序，同类内保持原有顺序）；功能区不参与过滤。
pub(crate) fn filter_repo_switcher_sections(
    sections: RepoSwitcherSections,
    query: &str,
) -> RepoSwitcherSections {
    let query_trimmed = query.trim();
    if query_trimmed.is_empty() {
        return sections;
    }
    let mut open = sections
        .open
        .iter()
        .filter(|repo| repo_switcher_repo_matches_query(repo, query_trimmed))
        .cloned()
        .collect::<Vec<_>>();
    let mut recent = sections
        .recent
        .iter()
        .filter(|repo| repo_switcher_repo_matches_query(repo, query_trimmed))
        .cloned()
        .collect::<Vec<_>>();
    open.sort_by_key(|repo| !repo_switcher_repo_name_matches_query(repo, query_trimmed));
    recent.sort_by_key(|repo| !repo_switcher_repo_name_matches_query(repo, query_trimmed));
    RepoSwitcherSections {
        actions: sections.actions,
        open,
        recent,
    }
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod app_tests;

/// 全量注册键盘绑定：先清空再重新注册全部（TextInput/BrowseContent 基础键位 + 应用级快捷键）。
/// 在启动时和用户修改快捷键后调用，保证绑定始终与 self.shortcut_bindings 一致。
/// 全量注册键盘绑定：先清空再重新注册全部（TextInput/BrowseContent 基础键位 + 应用级快捷键）。
/// 在启动时和用户修改快捷键后调用，保证绑定始终与 self.shortcut_bindings 一致。
/// `skip_shortcuts` 为 true 时仅注册基础键位（录制态避免按键匹配到 action 导致 keydown 被吞）。
fn register_all_key_bindings(cx: &mut App, bindings: &ShortcutBindings, skip_shortcuts: bool) {
    cx.clear_key_bindings();
    // 基础键位：文本输入框和浏览内容区。
    cx.bind_keys([
        KeyBinding::new("backspace", TextBackspace, Some("TextInput")),
        KeyBinding::new("delete", TextDelete, Some("TextInput")),
        KeyBinding::new("left", TextLeft, Some("TextInput")),
        KeyBinding::new("right", TextRight, Some("TextInput")),
        KeyBinding::new("up", TextUp, Some("TextInput")),
        KeyBinding::new("down", TextDown, Some("TextInput")),
        KeyBinding::new("shift-left", TextSelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", TextSelectRight, Some("TextInput")),
        KeyBinding::new("shift-up", TextSelectUp, Some("TextInput")),
        KeyBinding::new("shift-down", TextSelectDown, Some("TextInput")),
        KeyBinding::new("home", TextHome, Some("TextInput")),
        KeyBinding::new("end", TextEnd, Some("TextInput")),
        KeyBinding::new("cmd-enter", TextSubmit, Some("TextInput")),
        KeyBinding::new("ctrl-enter", TextSubmit, Some("TextInput")),
        KeyBinding::new("cmd-a", TextSelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", TextCopy, Some("TextInput")),
        KeyBinding::new("cmd-v", TextPaste, Some("TextInput")),
        KeyBinding::new("cmd-x", TextCut, Some("TextInput")),
        KeyBinding::new("ctrl-a", TextSelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-c", TextCopy, Some("TextInput")),
        KeyBinding::new("ctrl-v", TextPaste, Some("TextInput")),
        KeyBinding::new("ctrl-x", TextCut, Some("TextInput")),
    ]);
    // 应用级快捷键：全局生效（无 context 谓词）。
    if !skip_shortcuts {
        for action in ShortcutAction::ALL {
            let keystroke = action.keystroke(bindings);
            let binding = match action {
                ShortcutAction::Refresh => KeyBinding::new(keystroke, ShortcutRefresh, None),
                ShortcutAction::Fetch => KeyBinding::new(keystroke, ShortcutFetch, None),
                ShortcutAction::Pull => KeyBinding::new(keystroke, ShortcutPull, None),
                ShortcutAction::Push => KeyBinding::new(keystroke, ShortcutPush, None),
                ShortcutAction::OpenStash => KeyBinding::new(keystroke, ShortcutOpenStash, None),
                ShortcutAction::OpenSubmodule => {
                    KeyBinding::new(keystroke, ShortcutOpenSubmodule, None)
                }
                ShortcutAction::OpenSettings => {
                    KeyBinding::new(keystroke, ShortcutOpenSettings, None)
                }
                ShortcutAction::SwitchToWorktree => {
                    KeyBinding::new(keystroke, ShortcutSwitchToWorktree, None)
                }
                ShortcutAction::SwitchToHistory => {
                    KeyBinding::new(keystroke, ShortcutSwitchToHistory, None)
                }
                ShortcutAction::SwitchToWorkflow => {
                    KeyBinding::new(keystroke, ShortcutSwitchToWorkflow, None)
                }
                ShortcutAction::OpenInExplorer => {
                    KeyBinding::new(keystroke, ShortcutOpenInExplorer, None)
                }
                ShortcutAction::OpenRemoteInBrowser => {
                    KeyBinding::new(keystroke, ShortcutOpenRemoteInBrowser, None)
                }
            };
            cx.bind_keys([binding]);
        }
    }
}

/// 注册全局快捷键 action 监听器，通过 weak entity 在回调中安全更新 RepositoryView。
/// 使用 App::on_action（全局监听器），不依赖焦点路径，保证快捷键在任何非输入框焦点下都生效。
/// 设置中心打开时（含快捷键录制态），除「设置」外的全部快捷键都不触发主视图动作。
fn register_shortcut_listeners(cx: &mut App, weak: WeakEntity<RepositoryView>) {
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutRefresh, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.refresh();
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutFetch, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.fetch();
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutPull, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_remote_branch_operation(RemoteBranchOperationKind::Pull);
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutPush, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_remote_branch_operation(RemoteBranchOperationKind::Push);
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutOpenStash, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_stash_dialog();
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutOpenSubmodule, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_submodule_manager();
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutOpenSettings, cx| {
            // 设置中心已打开时按设置快捷键 -> 关闭（toggle 语义）；否则打开。
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    this.close_settings_center();
                } else {
                    this.open_settings_center();
                }
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutSwitchToWorktree, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.set_main_mode(MainMode::Worktree);
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutSwitchToHistory, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.set_main_mode(MainMode::History);
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutSwitchToWorkflow, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.set_main_mode(MainMode::Workflow);
                cx.notify();
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutOpenInExplorer, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_repo_in_explorer(cx);
            });
        }
    });
    cx.on_action({
        let weak = weak.clone();
        move |_a: &ShortcutOpenRemoteInBrowser, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() {
                    return;
                }
                this.open_remote_in_browser(cx);
            });
        }
    });
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    Application::new()
        .with_assets(assets::AppAssets::new())
        .run(|cx: &mut App| {
            // 启动最早期执行待处理的便携迁移（若用户上次已同意迁移）；
            // 必须在打开任何数据库连接之前完成文件搬运。
            let _ = khaslana::apply_pending_portable_migration();
            // 程序搬迁（exe 位于危险目录时用户已同意移动）：把程序与数据
            // 搬到安全目录后从新位置重启；成功路径内部直接退出进程。
            let _ = khaslana::apply_pending_exe_relocation();
            // 记录「上次数据目录」指针：exe 被手动挪走或旧位置副本再次
            // 运行时按指针延续旧数据（指针失效即忽略）。
            if let Some(data_dir) = khaslana::storage::active_data_dir() {
                khaslana::record_last_data_home(&data_dir);
            }
            init_yororen_components(cx);
            cx.set_global(GlobalTheme::new(cx.window_appearance()));
            cx.set_global(I18n::with_embedded(
                Locale::new("zh-CN").expect("zh-CN locale is valid"),
            ));
            let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
            // 注册全部键盘绑定：基础键位（TextInput/BrowseContent）+ 应用级快捷键（从持久化加载）。
            let shortcut_bindings = khaslana::AppStorage::open_default()
                .ok()
                .map(|storage| RepositoryView::load_shortcut_bindings(&storage))
                .unwrap_or_else(default_shortcut_bindings);
            register_all_key_bindings(cx, &shortcut_bindings, false);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // GPUI 0.3.3 将该值传给原生窗口，确保最小化/最大化/关闭控制区始终可达。
                    window_min_size: Some(size(
                        px(chrome_view::MIN_WINDOW_WIDTH),
                        px(chrome_view::MIN_WINDOW_HEIGHT),
                    )),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Khaslana".into()),
                        // 隐藏 Windows 原生标题栏，由主工具栏承载拖动和窗口控制。
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(RepositoryView::new_with_session);
                    view.update(cx, |this, cx| {
                        this.attach_window_to_tray(window);
                        this.apply_theme_for_appearance(window.appearance(), cx);
                        // 跟随系统模式在系统深浅色变化时即时刷新；固定模式也会保持所选色板。
                        cx.observe_window_appearance(window, |this, window, cx| {
                            this.apply_theme_for_appearance(window.appearance(), cx);
                            window.refresh();
                        })
                        .detach();
                    });
                    let weak_view = view.downgrade();
                    window.on_window_should_close(cx, move |_window, cx| {
                        weak_view
                            .update(cx, |this, cx| {
                                let should_close = this.should_close_window();
                                cx.notify();
                                should_close
                            })
                            .unwrap_or(true)
                    });
                    // 注册全局快捷键监听器：不依赖焦点路径，在 action 冒泡到顶层时触发。
                    register_shortcut_listeners(cx, view.downgrade());
                    window.focus(&view.read(cx).focus_handle(cx));
                    view
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
