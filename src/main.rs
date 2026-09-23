#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai_view;
mod assets;
mod blame_view;
mod browse_compare_view;
mod browse_view;
mod chrome_view;
mod code_index_view;
mod code_palette_view;
mod commit_graph_view;
mod conflicts;
mod dialog_view;
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
mod repository_actions;
mod repository_core;
mod repository_credentials;
mod repository_events;
mod repository_operations;
mod repository_ui;
mod settings_center;
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
use std::ops::{Deref, DerefMut};
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
    App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, FocusHandle, Focusable,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollHandle, ScrollStrategy, TitlebarOptions, UniformListScrollHandle, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, canvas, div, img, point,
    prelude::*, px, size,
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
use text_input::TextFieldState;
use ui::theme::rgb;
use ui::{
    components::{
        AppToastKind, FeedbackMessage, ToastAction, app_shell_surface, bottom_progress_bar,
        danger_callout, dialog_actions, dialog_overlay, dialog_panel as ui_dialog_panel,
        dialog_panel_size, feedback_bubble, feedback_stack, glass_menu, segmented_button,
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

// Kit 输入（`Input` / `Textarea`）自管编辑、选区、IME 与剪贴板键位；
// 自绘输入的 16 个 text_* action 随 M7 清理删除。这里只保留 TextSubmit：
// 多行框的 Ctrl/Cmd+Enter 提交由它在 Kit 写入换行前截获（key context
// "Input" 由 Kit 输入组件内部声明）。
actions!(text_input, [TextSubmit]);

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
        ShortcutOpenCodeSearch,      // 符号搜索面板
    ]
);

/// 工作流快捷键的载荷动作：一条绑定自带模板文件名与执行模式，分发时
/// `on_action` 监听器直接读到载荷（gpui-ce 的 `KeyBinding` 携带 action 实例，
/// 故动态模板数量无需静态槽位）。`no_json`：不经 keymap JSON 构建——
/// 绑定只由本应用在运行时注册。
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = app_action, no_json)]
pub(crate) struct RunWorkflowShortcut {
    /// 模板文件名（如 "sync.json5"），绑定与触发的稳定标识。
    pub(crate) file: String,
    /// 「后台执行」勾选结果：true = 触发时不切换到工作流页。
    pub(crate) background: bool,
}

/// Kit 下拉菜单的有载荷动作：菜单项只负责派发选择，业务状态仍由 RepositoryView 持有。
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ui_action, no_json)]
pub(crate) struct SelectRemoteBranchOperationRemote {
    pub(crate) remote: String,
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ui_action, no_json)]
pub(crate) struct SelectTagPushRemote {
    pub(crate) remote: String,
}

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
    OpenCodeSearch,
}

impl ShortcutAction {
    /// 全部动作，按设置中心显示顺序。
    pub(crate) const ALL: [ShortcutAction; 13] = [
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
        ShortcutAction::OpenCodeSearch,
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
            ShortcutAction::OpenCodeSearch => "open_code_search",
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
            ShortcutAction::OpenCodeSearch => "符号搜索",
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
            ShortcutAction::OpenCodeSearch => "ctrl-p",
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

/// 快捷键录制目标：静态应用动作或某个工作流模板。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShortcutRecordingTarget {
    App(ShortcutAction),
    Workflow { file: String },
}

/// 快捷键冲突来源：静态应用动作或某个工作流模板绑定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShortcutConflict {
    App(ShortcutAction),
    Workflow(String),
}

impl ShortcutConflict {
    /// 冲突占用的功能描述（静态动作用中文标签，工作流用文件名）。
    pub(crate) fn describe(&self) -> String {
        match self {
            ShortcutConflict::App(action) => action.label().to_string(),
            ShortcutConflict::Workflow(file) => format!("工作流 {file}"),
        }
    }
}

/// 统一冲突检查：静态动作与工作流绑定共用一个键位空间，
/// 双向覆盖 静态↔工作流、工作流↔工作流，排除录制目标自身。
pub(crate) fn find_keystroke_conflict(
    app_bindings: &ShortcutBindings,
    workflow_bindings: &khaslana::WorkflowShortcutBindings,
    target: &ShortcutRecordingTarget,
    keystroke: &str,
) -> Option<ShortcutConflict> {
    for action in ShortcutAction::ALL {
        if matches!(target, ShortcutRecordingTarget::App(target_action) if *target_action == action)
        {
            continue;
        }
        if action.keystroke(app_bindings) == keystroke {
            return Some(ShortcutConflict::App(action));
        }
    }
    for (file, binding) in &workflow_bindings.bindings {
        if matches!(target, ShortcutRecordingTarget::Workflow { file: target_file } if target_file == file)
        {
            continue;
        }
        if binding.keystroke == keystroke {
            return Some(ShortcutConflict::Workflow(file.clone()));
        }
    }
    None
}

/// 加载防御剪枝：keystroke 为空/不可解析、撞任一静态动作有效键、或同键位
/// 重复的工作流绑定丢弃（手改 DB / 异常数据兜底）；返回 (剪枝结果, 是否有变化)。
pub(crate) fn prune_workflow_shortcut_bindings(
    bindings: &khaslana::WorkflowShortcutBindings,
    app_bindings: &ShortcutBindings,
) -> (khaslana::WorkflowShortcutBindings, bool) {
    let static_keystrokes: Vec<&str> = ShortcutAction::ALL
        .iter()
        .map(|action| action.keystroke(app_bindings))
        .collect();
    let mut pruned = khaslana::WorkflowShortcutBindings::default();
    let mut seen_keystrokes: Vec<String> = Vec::new();
    for (file, binding) in &bindings.bindings {
        let keystroke = binding.keystroke.trim();
        if keystroke.is_empty()
            || gpui::Keystroke::parse(keystroke).is_err()
            || static_keystrokes.contains(&keystroke)
            || seen_keystrokes.iter().any(|seen| seen == keystroke)
        {
            continue;
        }
        seen_keystrokes.push(keystroke.to_string());
        pruned.bindings.insert(
            file.clone(),
            khaslana::WorkflowShortcutBinding {
                keystroke: keystroke.to_string(),
                background: binding.background,
            },
        );
    }
    let changed = pruned != *bindings;
    (pruned, changed)
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
pub(crate) const WORKFLOW_TEMPLATE_MENU_HEIGHT: f32 = 132.0;
const COMMIT_MENU_WIDTH: f32 = 230.0;
const COMMIT_MENU_HEIGHT: f32 = 320.0;
const COMMIT_UNPUSHED_MENU_HEIGHT: f32 = 355.0;
const ENCODING_MENU_WIDTH: f32 = 170.0;
const MENU_VIEWPORT_MARGIN: f32 = 8.0;
// 自绘窗口控制区（3 × 32px + 2 × 2px 间距）固定占宽；右边距由顶栏内边距给出。
// 壳层布局与本文件共用同一常量。
pub(crate) const WINDOW_CONTROLS_WIDTH: f32 =
    ui_theme::WINDOW_CONTROL_SIZE * 3.0 + ui_theme::WINDOW_CONTROL_GAP * 2.0;
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
    /// 设置中心「代码索引」页的仓库列表过滤框。
    CodeIndexFilter,
    /// 全局符号搜索面板（Ctrl+P）的输入框。
    CodePaletteSearch,
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
    (FieldId::CodeIndexFilter, |view: &RepositoryView| {
        &view.code_index_filter
    }),
    (FieldId::CodePaletteSearch, |view: &RepositoryView| {
        &view.code_palette_search
    }),
];

/// 全部 FieldId 变体（与枚举同步维护；tests 断言 DEDICATED_FIELDS 全覆盖，
/// 防止「新增变体漏注册 → field() expect panic / 输入被静默丢弃」回归）。
#[cfg(test)]
pub(crate) const ALL_FIELD_IDS: &[FieldId] = &[
    FieldId::CloneUrl,
    FieldId::ClonePath,
    FieldId::BranchName,
    FieldId::BranchRename,
    FieldId::RemoteName,
    FieldId::RemoteUrl,
    FieldId::CommitMessage,
    FieldId::TagName,
    FieldId::TagMessage,
    FieldId::CredentialUsername,
    FieldId::CredentialSecret,
    FieldId::CredentialKeyPath,
    FieldId::CredentialPassphrase,
    FieldId::CredentialRemoteUrl,
    FieldId::CredentialDisplayName,
    FieldId::CredentialTestUrl,
    FieldId::ConflictEditor,
    FieldId::RemoteBranchName,
    FieldId::RemoteBranchSearch,
    FieldId::RepoSwitcherSearch,
    FieldId::CommitGraphSearch,
    FieldId::CommitGraphBranchSearch,
    FieldId::SidebarLocalBranchSearch,
    FieldId::SidebarRemoteBranchSearch,
    FieldId::ProxyHttpUrl,
    FieldId::ProxyHttpsUrl,
    FieldId::ProxySocks5Url,
    FieldId::AiBaseUrl,
    FieldId::AiApiKey,
    FieldId::AiModel,
    FieldId::ExternalMergeIntellijPath,
    FieldId::CodeIndexFilter,
    FieldId::CodePaletteSearch,
    FieldId::StashMessage,
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
    /// 代码索引：per-仓库开关、索引状态与重建/删除入口。
    CodeIndex,
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
    /// 删除仓库索引数据目录的确认弹窗（设置中心「代码索引」页；多仓库列表中
    /// 每个仓库都可发起删除，弹窗与删除动作按 repo_key 寻址，不再限定活动仓库）。
    ConfirmDeleteCodeIndex {
        repo_key: String,
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
    /// 工作流模板快捷键绑定弹窗（录制键位 + 后台执行开关）。
    WorkflowShortcutBinding {
        file: String,
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
    pub(crate) has_upstream: bool,
    pub(crate) height: f32,
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

/// 右键菜单的键盘动作表：渲染时按条目顺序登记（与视觉清单同源），
/// ↑/↓ 循环选择（跳过分隔线与禁用项）、Enter 执行选中项——键盘确认
/// 与鼠标点击走同一条执行路径（审查 R6）。
pub(crate) struct ContextMenuKeyboard {
    /// 当前登记的菜单身份（切换到别的菜单时重置选中与动作表）。
    menu_id: Option<String>,
    /// 当前选中的条目索引；None = 菜单刚打开，尚未经过鼠标或方向键。
    selected: Option<usize>,
    /// 按渲染顺序登记的条目（分隔线不登记）。
    actions: Vec<ContextMenuKeyAction>,
}

impl Default for ContextMenuKeyboard {
    fn default() -> Self {
        Self {
            menu_id: None,
            selected: None,
            actions: Vec::new(),
        }
    }
}

/// 右键菜单单个条目的键盘动作：稳定业务 id（不用中文 label 寻址）+
/// 可用性 + 打开菜单时构造的执行闭包。
pub(crate) struct ContextMenuKeyAction {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) action: Rc<dyn Fn(&mut RepositoryView, &mut Context<RepositoryView>)>,
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

/// 在途代码索引任务状态（全局单任务，Index 池单线程串行化）。
pub(crate) struct CodeIndexTaskState {
    pub repo_path: String,
    /// 置位后任务在文件/阶段边界退出，不落盘。
    pub cancel: Arc<AtomicBool>,
}

/// 设置中心「代码索引」页的仓库列表条目。`repo_key` 为规范化小写路径，
/// `name` 取自显示路径末段（保留原大小写，仅作展示）。
pub(crate) struct CodeIndexListEntry {
    pub repo_key: String,
    pub name: String,
    pub path: String,
}

/// 全局符号搜索面板（Ctrl+P）的会话状态；None = 关闭。
#[derive(Default)]
pub(crate) struct CodeSearchPaletteState {
    pub selected_index: usize,
    pub results: Vec<khaslana::code_index::SearchHit>,
    /// 选中符号的详情（直接调用关系 + 源码片段），随选中变化异步刷新。
    pub detail: Option<khaslana::code_index::SymbolDetail>,
    pub searching: bool,
    /// 详情请求在途（右栏展示「详情加载中」而非回落到提示文案）。
    pub detail_loading: bool,
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

/// 一次性 AI 生成任务（commit message / 冲突合并建议 / 工作流模板）的
/// 运行态。与弹窗可见态分离：「后台运行」只收起弹窗，任务继续跑，
/// 三业务互斥到任务真正完成/失败才释放（审查 R8）。
pub(crate) struct AiThinkingTask {
    /// 任务身份：增量/完成/失败事件按它寻址，迟到事件不影响其他任务。
    id: u64,
    /// 归属业务：共用失败事件只复位本业务的 loading 与回填目标。
    kind: AiThinkingTaskKind,
}

fn take_matching_ai_task(task: &mut Option<AiThinkingTask>, task_id: u64) -> Option<AiThinkingTask> {
    if task.as_ref()?.id != task_id {
        return None;
    }
    task.take()
}

/// 一次性 AI 生成任务归属的业务。
#[derive(Clone, Debug)]
pub(crate) enum AiThinkingTaskKind {
    CommitMessage,
    ConflictMerge { path: String },
    WorkflowTemplate { session_id: u64 },
}

impl AiThinkingTaskKind {
    fn owns_workflow_session(&self, current: Option<u64>) -> bool {
        matches!(self, Self::WorkflowTemplate { session_id } if Some(*session_id) == current)
    }
}

/// 浮层种类。实际层级由 overlay_focus_layers 按挂载顺序构建：普通
/// 右键菜单在设置之下，凭据菜单在对话框之上，不能只按枚举排序。
///
/// 渲染挂载顺序（`RepositoryView::render` 尾部）自底向上为：设置中心 →
/// 对话框 → 代码面板 → AI 思考窗 → 凭据菜单 → 操作遮罩 → 待输入凭据
/// 面板，因此这里的排序与之对应。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TopOverlayKind {
    /// 「需要凭据」提示面板（git 凭据回调期间挂在最顶层）。
    CredentialPrompt,
    /// 无遮罩弹层：各类右键菜单、编码菜单、仓库切换下拉、图谱分支下拉、
    /// 编辑器步骤下拉与窄窗导航覆盖层。
    PopupMenu,
    /// AI 思考弹窗（Esc = 后台运行语义，不终止任务）。
    AiThinking,
    /// 全局符号搜索面板（Ctrl+P）。
    CodePalette,
    /// AI 评审历史弹窗。
    ReviewHistory,
    /// 普通对话框（含工作流编辑器、凭据子窗等）。
    Dialog,
    /// 设置中心。
    Settings,
    #[default]
    None,
}

impl TopOverlayKind {
    /// 是否为带焦点圈的模态浮层（打开时焦点移入其中，Tab 在圈内循环）。
    fn is_modal(self) -> bool {
        matches!(
            self,
            Self::CredentialPrompt
                | Self::AiThinking
                | Self::CodePalette
                | Self::ReviewHistory
                | Self::Dialog
                | Self::Settings
        )
    }
}

/// 每一层保留自己的返回目标。同层替换继承旧层的返回目标，不能记录
/// 即将销毁的旧弹窗控件；同时关闭多层时返回最外一层的触发器。
struct OverlayFocusStack<K, H> {
    entries: Vec<(K, Option<H>)>,
}

impl<K, H> Default for OverlayFocusStack<K, H> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

struct OverlayFocusChange<H> {
    entering: bool,
    return_to: Option<H>,
}

impl<K: PartialEq + Clone, H: Clone> OverlayFocusStack<K, H> {
    fn reconcile(&mut self, layers: &[K], current: Option<H>) -> Option<OverlayFocusChange<H>> {
        let common = self.entries.iter().zip(layers)
            .take_while(|((old, _), new)| old == *new).count();
        if common == self.entries.len() && common == layers.len() {
            return None;
        }
        let return_to = if common < self.entries.len() {
            self.entries[common].1.clone()
        } else {
            current
        };
        self.entries.truncate(common);
        let entering = layers.len() > common;
        for (index, layer) in layers.iter().enumerate().skip(common) {
            self.entries.push((layer.clone(), if index == common { return_to.clone() } else { None }));
        }
        Some(OverlayFocusChange { entering, return_to })
    }
}

type OverlayFocusKey = (TopOverlayKind, Option<DialogState>);

#[derive(Default)]
pub(crate) struct OverlayFocusReturn {
    stack: OverlayFocusStack<OverlayFocusKey, gpui::WeakFocusHandle>,
    generation: u64,
    restore_pending: bool,
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
    /// 代码索引构建进度（按 repo_path 键控：索引中关闭仓库标签任务照常完成）。
    CodeIndexProgress {
        repo_path: String,
        message: String,
        done: usize,
        total: usize,
    },
    /// 代码索引完成（stats 为空 None 表示增量检查后无变化）。
    CodeIndexFinished {
        repo_path: String,
        stats: Option<khaslana::code_index::IndexRunStats>,
    },
    CodeIndexFailed {
        repo_path: String,
        error: String,
    },
    /// 全局符号搜索面板：查询结果（seq 守卫防乱序）。
    CodePaletteSearchFinished {
        seq: u64,
        hits: Vec<khaslana::code_index::SearchHit>,
    },
    /// 全局符号搜索面板：选中符号详情。
    CodePaletteDetailFinished {
        seq: u64,
        detail: Option<Box<khaslana::code_index::SymbolDetail>>,
    },
    /// 设置页打开/刷新时后台读库回填的索引统计。
    CodeIndexStatsLoaded {
        repo_path: String,
        stats: Option<khaslana::code_index::IndexStats>,
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
        /// 任务身份：与 `ai_thinking_task` 匹配才应用；不匹配说明是已收尾
        /// 任务的迟到事件，静默丢弃（审查 R8）。
        task_id: u64,
        message: String,
    },
    /// 工作流模板 AI 生成/编辑完成（JSON5 文本，经编辑器解析回填表单）。
    AiWorkflowTemplateGenerated {
        task_id: u64,
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
        task_id: u64,
        path: String,
        draft: String,
    },
    /// AI 思考弹窗的流式增量（思维链/正文），由公共执行器转发；
    /// `content_delta` 为 None 表示本片是思维链。携带任务身份：只进
    /// 所属任务且弹窗仍可见时增量，后台运行/迟到增量不写进别的窗口。
    AiThinkingDelta {
        task_id: u64,
        content_delta: Option<String>,
        reasoning_delta: String,
    },
    AiRequestFailed {
        task_id: u64,
        error: String,
    },
    /// AI 供应商连接测试失败（不属于一次性生成任务，无任务身份）。
    AiConnectionTestFailed {
        error: String,
    },
    AiConnectionTested {
        message: String,
    },
    // ── 更新事件 ──
    UpdateCheckFinished {
        manifest: Arc<UpdateManifest>,
        asset: UpdatePlatformAsset,
        trigger: UpdateCheckTrigger,
    },
    UpdateCheckFailed {
        error: String,
        /// 触发来源，决定结果反馈方式：手动（设置页「立即检查」）弹气泡、
        /// 启动自动仅状态栏、周期静默完全无反馈（发现新版本走可点击气泡）。
        trigger: UpdateCheckTrigger,
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
    pub(crate) settings_center_generation: u64,
    /// 用户自定义快捷键绑定（action_id → keystroke）。
    pub(crate) shortcut_bindings: ShortcutBindings,
    /// 工作流模板快捷键绑定（模板文件名 → 键位 + 后台执行），机器本地全局单份。
    pub(crate) workflow_shortcut_bindings: khaslana::WorkflowShortcutBindings,
    /// 正在录制的快捷键目标（静态动作或工作流模板）；None 表示非录制态。
    pub(crate) recording_shortcut: Option<ShortcutRecordingTarget>,
    /// 应用壳层的稳定焦点落点：关闭弹层后恢复到这里，再由 Tab 进入下一个控件。
    shell_focus: FocusHandle,
    /// 设置中心面板的焦点句柄，录制态时夺取焦点使 keydown dispatch_path 进入 overlay。
    settings_center_focus: FocusHandle,
    /// 工作流快捷键绑定弹窗的焦点句柄，录制态夺取焦点使按键先到根捕获层。
    pub(crate) workflow_shortcut_binding_focus: FocusHandle,
    /// 普通对话框的焦点圈句柄：dialog_overlay 经 `focus_trap` 挂载，
    /// 打开时焦点移入其中，Tab/Shift+Tab 在圈内循环不漏到遮罩下层。
    dialog_focus: FocusHandle,
    /// AI 思考弹窗的焦点圈句柄（同 dialog_focus 用途）。
    ai_thinking_focus: FocusHandle,
    /// 「需要凭据」提示面板的焦点圈句柄（同 dialog_focus 用途）。
    credential_prompt_focus: FocusHandle,
    /// 全局符号搜索面板的焦点圈句柄（同 dialog_focus 用途）。
    code_palette_focus: FocusHandle,
    review_history_focus: FocusHandle,
    /// 右键菜单容器的共享焦点圈句柄：菜单打开时焦点移入其中
    /// （maintain_overlay_focus），↑/↓/Enter 经根层 on_key_down 分发。
    context_menu_focus: FocusHandle,
    /// 顶层浮层焦点恢复记录（进入前焦点、待恢复标志），见
    /// [`OverlayFocusReturn`] 与 `maintain_overlay_focus`。
    overlay_focus_return: OverlayFocusReturn,
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
    /// 右键菜单键盘动作表（渲染时登记、↑/↓/Enter 按索引执行），见
    /// [`ContextMenuKeyboard`]（审查 R6）。
    pub(crate) context_menu_keyboard: RefCell<ContextMenuKeyboard>,
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
    // 普通壳层按钮的焦点由 Kit 基础按钮按元素 id 管理，无需在状态机单独持有句柄。
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
    /// 已迁移到 Kit 输入的字段宿主（`ui::fields`，按 `DEDICATED_FIELDS` 惰性创建）。
    kit_fields: Vec<(FieldId, ui::fields::KitField)>,
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
    ///
    /// 这只是**可见态**：「后台运行」只把它置 None，任务继续跑；三业务
    /// 互斥由 [`RepositoryView::ai_thinking_task`]（运行态）持有，任务
    /// 真正完成/失败才释放（审查 R8）。
    pub(crate) ai_thinking_overlay: Option<AiThinkingOverlayState>,
    /// 一次性 AI 生成任务的运行态（None = 无任务）：打开即占位，完成/失败
    /// 才清空。增量/成功/失败事件携带任务 id 按它寻址。
    pub(crate) ai_thinking_task: Option<AiThinkingTask>,
    /// 下一个 AI 思考任务 id（进程内自增，只用于身份区分）。
    next_ai_thinking_task_id: u64,
    /// 思考弹窗钉底跟随的跨帧状态（内容长度键），见 `AiThinkingFollowState`。
    pub(crate) ai_thinking_follow_state: std::rc::Rc<AiThinkingFollowState>,
    // ── 代码索引 ──
    /// 在途索引任务（全局单任务：Index 池单线程 + 此守卫双保险）。
    pub(crate) code_index_task: Option<CodeIndexTaskState>,
    /// 各仓库最近一次索引统计缓存（事件回填；设置页打开时也会后台查库刷新）。
    pub(crate) code_index_stats: HashMap<String, khaslana::code_index::IndexStats>,
    /// 设置页仓库列表过滤框（按仓库名称或路径过滤）。
    pub(crate) code_index_filter: TextFieldState,
    /// 设置页仓库列表（打开设置页时构建：已打开 tabs + 最近仓库 + 索引偏好记录）。
    pub(crate) code_index_list_entries: Vec<CodeIndexListEntry>,
    /// 在途索引任务的进度计数（进度条渲染；无任务时无意义）。
    pub(crate) code_index_progress_done: usize,
    pub(crate) code_index_progress_total: usize,
    /// 在途任务的最新进度文案（状态卡显示）。
    pub(crate) code_index_progress_message: String,
    /// 已启用索引的仓库键缓存（启动与设置页打开时从主库加载）。
    pub(crate) code_index_enabled_cache: std::collections::HashSet<String>,
    /// 全局符号搜索面板（Ctrl+P）；None = 关闭。
    pub(crate) code_search_palette: Option<CodeSearchPaletteState>,
    /// 面板输入框（面板关闭后保留输入内容，重开可续用）。
    pub(crate) code_palette_search: TextFieldState,
    /// 面板查询请求序号（每按键 +1，事件携带，乱序丢弃）。
    pub(crate) code_palette_search_seq: u64,
    /// 面板详情请求序号（随选中变化 +1）。
    pub(crate) code_palette_detail_seq: u64,
    // ── 更新状态 ──
    pub(crate) update_preferences: UpdatePreferences,
    pub(crate) update_checking: bool,
    /// 下一次周期静默更新检查的时刻（UiTick 到期触发；构造时 = 启动后 12h，
    /// 启动检查不重复计入）。
    next_periodic_update_check: Instant,
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
        // 先记录进入/退出的焦点关系，新元素树挂载后再应用初始焦点或返回目标。
        self.maintain_overlay_focus(window, cx);
        // 工作流模板编辑器：渲染前确保当前展示的文本框已创建
        //（field_mut 无 cx 不能惰性建框，text_input 的 paint 路径依赖它已存在）。
        self.ensure_workflow_editor_fields_inited(window, cx);
        // Kit 输入字段：渲染期只有 `&self`，宿主实体必须在这里先建好并对齐。
        self.ensure_kit_fields(window, cx);
        // 窗口圆角：外壳是「透明窗口 + 自绘圆角」的那张卡片，最大化时必须归零，
        // 否则四角会露出桌面。所有铺满窗口的色块（根背景、顶栏、对话框遮罩）
        // 都读这个值，保证圆角一致。
        ui_theme::set_window_radius(if window.is_maximized() {
            0.0
        } else {
            ui_theme::RADIUS_WINDOW
        });
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
            .track_focus(&self.shell_focus)
            .relative()
            .flex()
            .flex_col()
            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
            // 全局符号搜索面板的入口监听：元素级（区别于其他快捷键的
            // App::on_action），回调带 Window 以便聚焦面板输入框。
            .on_action(cx.listener(|this, _: &ShortcutOpenCodeSearch, window, cx| {
                if this.active_dialog.is_some() || this.settings_center.is_some() {
                    return;
                }
                this.toggle_code_search_palette(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(Self::text_submit))
            .on_action(cx.listener(
                |this, action: &SelectRemoteBranchOperationRemote, _window, cx| {
                    this.select_remote_branch_operation_remote(action.remote.clone());
                    cx.notify();
                },
            ))
            .on_action(cx.listener(
                |this, action: &SelectTagPushRemote, _window, cx| {
                    this.tag_push_remote = Some(action.remote.clone());
                    cx.notify();
                },
            ))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                // 设置中心焦点在 overlay 容器时用 ↑/↓ 快速切换分类；
                // 焦点在搜索框或内容控件时不拦截方向键。
                if matches!(key, "up" | "down")
                    && this.settings_center.is_some()
                    && window
                        .focused(cx)
                        .is_some_and(|handle| handle == this.settings_center_focus)
                    && {
                        this.cycle_settings_category(key == "down");
                        true
                    }
                {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if key == "escape"
                    && this.recording_shortcut.is_none()
                    && this.dismiss_topmost_cancellable_overlay()
                {
                    // 焦点归还进入浮层前的触发器（已销毁则父层/根），
                    // 由下一帧 maintain_overlay_focus 执行。
                    this.request_overlay_focus_restore();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // 右键菜单键盘模型（R6）：↑/↓ 循环选择（跳禁用项）、
                // Enter 执行选中项。仅最上层是受跟踪的右键菜单时响应；
                // 仓库切换下拉、编码菜单等有自己的输入/键盘语义。
                if matches!(key, "up" | "down" | "enter")
                    && this.handle_context_menu_key(key, cx)
                {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
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
                    // 鼠标点外部关闭菜单与 Esc 关闭同一策略：焦点还给打开
                    // 菜单前的元素（下一帧 maintain_overlay_focus 执行）。
                    this.request_overlay_focus_restore();
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
                    // 防御：工作流录制目标依赖绑定弹窗持有焦点，若弹窗已被
                    // 其它路径关闭（理论路径），取消录制避免快捷键长期失效。
                    if let Some(ShortcutRecordingTarget::Workflow { file }) =
                        this.recording_shortcut.as_ref()
                    {
                        let dialog_open = matches!(
                            &this.active_dialog,
                            Some(DialogState::WorkflowShortcutBinding { file: open }) if open == file
                        );
                        if !dialog_open {
                            this.recording_shortcut = None;
                            crate::register_all_key_bindings(
                                &mut cx.deref_mut(),
                                &this.shortcut_bindings,
                                &this.workflow_shortcut_bindings,
                                false,
                            );
                            cx.notify();
                            cx.stop_propagation();
                            return;
                        }
                    }
                    if let Some(target) = this.recording_shortcut.clone() {
                        let ks = shortcuts_view::keystroke_to_string(event);
                        // 统一冲突检查：静态动作与工作流绑定共用键位空间。
                        if let Some(conflict) = find_keystroke_conflict(
                            &this.shortcut_bindings,
                            &this.workflow_shortcut_bindings,
                            &target,
                            &ks,
                        ) {
                            this.recording_shortcut = None;
                            this.notify_warning(
                                format!(
                                    "快捷键 {} 已被「{}」占用",
                                    shortcuts_view::format_keystroke(&ks),
                                    conflict.describe()
                                ),
                                cx,
                            );
                        } else {
                            // 通过检查，按录制目标写入对应绑定表。
                            match &target {
                                ShortcutRecordingTarget::App(action) => {
                                    this.shortcut_bindings
                                        .bindings
                                        .insert(action.action_id().to_string(), ks);
                                    this.recording_shortcut = None;
                                    this.save_shortcut_bindings();
                                    crate::register_all_key_bindings(
                                        &mut cx.deref_mut(),
                                        &this.shortcut_bindings,
                                        &this.workflow_shortcut_bindings,
                                        false,
                                    );
                                    cx.notify();
                                }
                                ShortcutRecordingTarget::Workflow { file } => {
                                    // 保留已有的后台执行标志，仅更新键位。
                                    let background = this
                                        .workflow_shortcut_bindings
                                        .bindings
                                        .get(file)
                                        .map(|binding| binding.background)
                                        .unwrap_or(false);
                                    this.workflow_shortcut_bindings.bindings.insert(
                                        file.clone(),
                                        khaslana::WorkflowShortcutBinding {
                                            keystroke: ks,
                                            background,
                                        },
                                    );
                                    this.recording_shortcut = None;
                                    this.persist_workflow_shortcut_bindings(cx);
                                }
                            }
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
                    // 悬浮工作台：内容区四周留白，导航与页面各成一张抬起的面板。
                    // 面板之间不设 gap——拖拽区自身就是那段间隙（默认无可见分割线，
                    // 悬停/拖拽时才显示指示），收起窄条用右边距留出同样的间隙。
                    // 顶栏与主界面之间同样留出 SHELL_PADDING，避免顶栏贴住页面。
                    .px(px(chrome_view::SHELL_PADDING))
                    .pt(px(chrome_view::SHELL_PADDING))
                    .pb(px(chrome_view::SHELL_PADDING))
                    // 左侧列：Docked 展开完整导航器（模式按钮 + 分组列表）；
                    // 其余情况（收起偏好/窄窗/专用页面）一律渲染 48px 收起窄条
                    // （模式图标 + 展开箭头 + 设置），模式入口在任何页面都常驻。
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
                    // 页面容器本身不铺底色：各页面自行把分栏组合成独立的悬浮面板
                    // （floating_panel），面板之间的空隙直接露出环境底色，分割
                    // 靠面板自身的圆角与投影表达，而不是一整张卡被线条切开。
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
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
                                MainMode::Stash => {
                                    self.render_stash_preview_view(cx).into_any_element()
                                }
                                MainMode::Browse => self.render_browse_view(cx).into_any_element(),
                                MainMode::Blame => self.render_blame_view(cx).into_any_element(),
                                MainMode::CommitGraph => {
                                    self.render_commit_graph_view(window, cx).into_any_element()
                                }
                            }),
                    )
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
            .child(self.render_code_search_palette(window, cx))
            .child(self.render_ai_thinking_overlay(window, cx))
            .child(self.render_credential_context_menu(cx))
            .child(self.render_operation_blocker())
            .child(self.render_credentials(window, cx))
            .child(self.render_feedback_layer(cx))
    }
}

impl Focusable for RepositoryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.shell_focus.clone()
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

/// 更新检查的触发来源，决定结果反馈方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckTrigger {
    /// 应用启动时的自动检查：结果仅状态栏；发现新版本弹模态框（既有行为）。
    Startup,
    /// 设置页「立即检查」：结果即时气泡反馈；发现新版本弹模态框。
    Manual,
    /// 运行期周期检查（`PERIODIC_UPDATE_CHECK_INTERVAL`）：无新版本/失败
    /// 完全静默；发现新版本只弹可点击气泡直达更新设置，不弹模态框。
    Periodic,
}

/// 周期静默检查间隔：写死为常量、不暴露为设置。清单获取是一次约 1KB 的
/// HTTPS GET（CNB 主源 + GitHub 兜底、15s 分相超时、走全局代理），12 小时
/// 对天/周级发版节奏足够且最不打扰；首拍 = 启动后 12 小时（启动检查不重复
/// 计入）。系统睡眠期间 UiTick 线程暂停、单调钟照走，唤醒后到期即查。
const PERIODIC_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// 检查更新结果的气泡决策（纯函数，可单测）：手动检查需要即时反馈——
/// 「已是最新」弹成功气泡、真失败弹错误气泡；启动自动检查每次都跑，
/// 弹气泡会打扰，保持安静（发现新版本走弹窗不受此影响）；周期静默检查
/// 同样安静（发现新版本走可点击气泡，不在此决策）。
fn update_check_feedback(
    error: &str,
    trigger: UpdateCheckTrigger,
) -> Option<(AppToastKind, String)> {
    if trigger != UpdateCheckTrigger::Manual || error.is_empty() {
        return None;
    }
    if error == "当前已是最新版本" {
        Some((AppToastKind::Success, error.to_string()))
    } else {
        Some((AppToastKind::Error, format!("检查更新失败：{error}")))
    }
}

/// 周期检查发现新版本的气泡文案（纯函数，可单测）：发现版本与已知
/// `available_update` 版本相同（用户已看过提示，点 ✕ 或超时消失视为
/// 「稍后」）时不再重复打扰，返回 None；设置页常驻卡片仍可查。
fn periodic_update_found_toast(known_version: Option<&str>, found_version: &str) -> Option<String> {
    if known_version == Some(found_version) {
        return None;
    }
    Some(format!("发现新版本 v{found_version}，点击查看更新"))
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

/// 全量注册键盘绑定：先清空再重新注册全部（TextInput 基础键位 + 工作流快捷键 + 应用级快捷键）。
/// 在启动时和用户修改快捷键/工作流绑定后调用，保证绑定始终与持久化状态一致。
/// `skip_shortcuts` 为 true 时仅注册基础键位（录制态避免按键匹配到 action 导致 keydown 被吞）。
fn register_all_key_bindings(
    cx: &mut App,
    bindings: &ShortcutBindings,
    workflow_bindings: &khaslana::WorkflowShortcutBindings,
    skip_shortcuts: bool,
) {
    // Kit 在 init 时注册按钮、输入、菜单等组件键位。动态刷新应用快捷键时只清理
    // Khaslana 自己的绑定，不能把 Kit 的 Tab/Enter/Space/方向键/Esc 一并抹掉。
    let component_bindings = {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap
            .bindings()
            .filter(|binding| !is_khaslana_keybinding_action(binding.action().name()))
            .cloned()
            .collect::<Vec<_>>()
    };
    cx.clear_key_bindings();
    cx.bind_keys(component_bindings);
    // Kit Textarea 默认会在 secondary-enter 先插入换行再发 PressEnter；
    // 这里以应用提交动作覆盖该键位，确保提交前不会污染业务真值。
    //（key context "Input" 由 Kit 输入组件内部声明；自绘输入的
    // "TextInput" context 键位随自绘输入一并删除。）
    cx.bind_keys([KeyBinding::new(
        "secondary-enter",
        TextSubmit,
        Some("Input"),
    )]);
    // 应用级快捷键：全局生效（无 context 谓词）。
    if !skip_shortcuts {
        // 工作流绑定先于静态快捷键注册：同键位意外撞车时后注册的静态键胜出
        //（正常路径由统一冲突检查在写入前拦截，这只是防御顺序）。
        for (file, binding) in &workflow_bindings.bindings {
            // KeyBinding::new 对不可解析键位会 panic；加载层已剪枝，此处再防一次。
            if gpui::Keystroke::parse(&binding.keystroke).is_err() {
                tracing::warn!("skip unparseable workflow shortcut: {file}");
                continue;
            }
            cx.bind_keys([KeyBinding::new(
                &binding.keystroke,
                RunWorkflowShortcut {
                    file: file.clone(),
                    background: binding.background,
                },
                None,
            )]);
        }
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
                ShortcutAction::OpenCodeSearch => {
                    KeyBinding::new(keystroke, ShortcutOpenCodeSearch, None)
                }
            };
            cx.bind_keys([binding]);
        }
    }
}

fn is_khaslana_keybinding_action(action_name: &str) -> bool {
    action_name.starts_with("text_input::") || action_name.starts_with("app_action::")
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
    // 工作流快捷键：绑定携带模板文件名与后台执行标志（载荷 action）。
    // 设置中心或任意模态对话框打开时不触发（后台启动长任务不应躲在遮罩后）。
    cx.on_action({
        let weak = weak.clone();
        move |a: &RunWorkflowShortcut, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.settings_center.is_some() || this.active_dialog.is_some() {
                    return;
                }
                this.trigger_workflow_shortcut(a.file.clone(), a.background, cx);
                cx.notify();
            });
        }
    });
}

fn main() {
    // MCP 无头模式：`khaslana mcp [仓库路径]` 供外部 AI 工具（Claude Code /
    // Cursor / ZCode 等）挂载，查询本机已索引仓库的代码知识图谱。仓库路径
    // 可选：带=单仓库模式；不带=多仓库模式（工具按 repo 参数解析目标仓库）。
    // 必须先于 GUI 启动判断；此分支不初始化 stdout 版日志（tracing 默认写
    // stdout 会污染协议流），MCP 内部进度一律走 stderr。
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("mcp") {
        let repo = args.next();
        let code = khaslana::code_index::mcp::run(repo.as_deref().map(std::path::Path::new));
        std::process::exit(code);
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    // gpui-pre 由平台层提供入口函数，没有 `Application::new()`（gpui-ce 的写法）。
    gpui_kit::application()
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
            // Kit 初始化（组件层 + 键位）。Kit 不注册 AssetSource，图标资源由
            // `application().with_assets(...)` 提供；主题在窗口创建后按外观与
            // 强调色偏好重建（`apply_theme_for_appearance`）。
            gpui_kit::init(cx);
            // 组件层文案语言：Kit 的内置 locale 停在 `en` fallback，迁移前
            // Yororen `I18n` 提供的简体中文能力要在这一层继承（`zh-CN` 是
            // 组件自带 ui.yml 支持的 locale key）。
            gpui_kit::component::set_locale("zh-CN");
            let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
            // 注册全部键盘绑定：基础键位（TextInput）+ 工作流快捷键 + 应用级快捷键（从持久化加载）。
            let shortcut_bindings = khaslana::AppStorage::open_default()
                .ok()
                .map(|storage| RepositoryView::load_shortcut_bindings(&storage))
                .unwrap_or_else(default_shortcut_bindings);
            let workflow_shortcut_bindings = khaslana::AppStorage::open_default()
                .ok()
                .map(|storage| RepositoryView::load_workflow_shortcut_bindings(&storage))
                .unwrap_or_default();
            register_all_key_bindings(cx, &shortcut_bindings, &workflow_shortcut_bindings, false);
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
                    // 窗口背景透明：外壳自己就是那张圆角卡片（画板把外围桌面装饰去掉，
                    // 以圆角内容为边界）。系统边框由 `apply_window_chrome` 去掉，
                    // 否则 DWM 会沿顶边画一条线、并从圆角外的透明区域透出来。
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    // 去掉系统边框（创建后一次 + 下一帧一次，系统会在窗口显示后补回 caption）。
                    chrome_view::apply_window_chrome(window);
                    window.on_next_frame(|window, _cx| chrome_view::apply_window_chrome(window));
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
                    // `view.read(cx)` 与 `focus(.., cx)` 会在同一表达式里借 cx 两次，
                    // 先取出焦点句柄再调用。
                    let root_focus = view.read(cx).focus_handle(cx);
                    window.focus(&root_focus, cx);
                    // Kit `Root` 作为窗口根视图（M2 遗留缺口）：它是 Kit 各种弹层
                    //（Notification / Sheet / Dialog / Tooltip 宿主）的挂载点，同时提供
                    // 标准 Tab / Shift+Tab 焦点遍历。业务视图仍是 `RepositoryView`，
                    // Root 只包一层，不改业务状态。Linux 的 CSD 边框由 `bordered(false)` 关掉
                    //（Windows 侧本身不绘制）。
                    cx.new(|cx| {
                        // `Root` 自己会铺一层 `theme.tokens.background`，那会把外壳圆角
                        // 之外的那圈重新涂成实色（圆角随之消失）。这里显式覆盖成透明，
                        // 让窗口只在圆角卡片内部有像素——Kit 组件的底色仍走主题映射。
                        gpui_kit::component::Root::new(view, window, cx)
                            .bordered(false)
                            .bg(ui_theme::rgba(0x00000000))
                    })
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
