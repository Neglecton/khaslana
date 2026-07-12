#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai_view;
mod assets;
mod browse_compare_view;
mod browse_view;
mod conflicts;
mod diff_view;
mod external_merge_view;
mod history_view;
mod operation_blocker_view;
mod proxy_view;
mod rebase_view;
mod remote_branch_operation;
mod sidebar_view;
mod stash_view;
mod ssh_credentials;
mod submodule_view;
mod system;
mod tasks;
mod text_input;
mod ui;
mod ui_helpers;
mod workflow_view;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut, Range};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use git2::Repository;
use gpui::{
    App, Application, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, ListHorizontalSizingBehavior, ListSizingBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle,
    ScrollStrategy, TitlebarOptions, UTF16Selection, UniformListScrollHandle, WeakEntity, Window,
    WindowBounds, WindowControlArea, WindowOptions, actions, canvas, div, point, prelude::*, px,
    rgb, rgba, size, uniform_list,
};
use khaslana::{
    AiProviderSettings, AiReviewResult, BranchKind, BranchName, BranchSyncStatus,
    BrowseCompareFile, BrowseEntry, BrowseFileContent, BrowseListMode, BrowseRefKind, BrowseTarget,
    ChangeState, CommitFileChange, CommitInfo, CommitMessage, ConflictBlockResolution,
    ConflictFileKind, ConflictFileView, CredentialProvider, CredentialRecord, CredentialRequest,
    CredentialScope, CredentialStore, CustomProxySettings, DiffEncodingChoice, DiffEncodingInfo,
    DiffEncodingPreferences, DiffLineKind, DiffScope, ExternalMergeSettings, FileDiff,
    GitCredential, GitService, HistoryRefsCache, HistoryScope, KeyringCredentialStore,
    NetworkProxyMode, NetworkProxySettings, OperationEvent, ProgressEmitter,
    RemoteCredentialBinding, RemoteCredentialBindings, RemoteCredentialPolicy, RemoteInfo,
    RemoteName, RepoPath, RepositorySnapshot, ResetMode, SessionState, SubmoduleInfo,
    SubmoduleRemoteSyncStatus, TagName, UpdatePreferences, credential_display_target,
    credential_key_filename, credential_kind_label, credential_record_is_compatible_with_url,
    credential_record_label, credential_record_matches_remote_url, credential_scope_label,
    normalize_remote_url, test_credential_connection,
    update::{self, UpdateCheckResult, UpdateManifest, UpdatePlatformAsset},
};
use lru::LruCache;
use operation_blocker_view::OperationBlocker;
use remote_branch_operation::{
    RemoteBranchOperationKind, RemoteBranchOperationState, default_remote_branch_for,
    local_branch_by_name, remote_branch_dialog_defaults, remote_branch_exists,
};
use stash_view::StashPreviewState;
use ssh_credentials::{SshCredentialDiscoveryState, SshDiscoveryResult};
use submodule_view::{
    SubmoduleDialogState, operation_refreshes_submodule_dialog, submodule_remote_request_matches,
    submodule_request_matches,
};
use tasks::{TaskExecutor, TaskKind};
use text_input::{MultiLineInputElement, SingleLineInputElement, TextFieldState};
use ui::{
    components::{
        AppToastKind, FeedbackMessage, InputFrameSize, app_panel, app_shell_surface,
        bottom_progress_bar, danger_callout, dialog_actions, dialog_overlay,
        dialog_panel as ui_dialog_panel, feedback_bubble, feedback_stack, glass_menu, hero_toolbar,
        input_frame, list_row_surface, mode_pill, segmented_button, toggle_box, tooltip_text,
    },
    icons::{ToolbarIcon, toolbar_icon, toolbar_icon_with_size},
    theme as ui_theme,
};
use ui_helpers::*;
use workflow_view::{WorkflowInputFieldState, WorkflowTemplateItem, workflow_templates_dir};
use yororen_ui::{
    component::init as init_yororen_components,
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

actions!(browse_content, [BrowseContentCopy, BrowseContentSelectAll,]);

const DEFAULT_SIDEBAR_WIDTH: f32 = 220.0;
const DEFAULT_CHANGES_WIDTH: f32 = 330.0;
const MIN_COLUMN_WIDTH: f32 = 240.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const CHANGE_ROW_HEIGHT: f32 = 36.0;
const DEFAULT_HISTORY_TOP_HEIGHT: f32 = 430.0;
const MIN_HISTORY_TOP_HEIGHT: f32 = 180.0;
const MAX_HISTORY_TOP_HEIGHT: f32 = 760.0;
const DEFAULT_HISTORY_FILES_WIDTH: f32 = 520.0;
const MIN_HISTORY_FILES_WIDTH: f32 = 260.0;
const MAX_HISTORY_FILES_WIDTH: f32 = 720.0;
const DEFAULT_BROWSE_TREE_WIDTH: f32 = 400.0;
const MIN_BROWSE_TREE_WIDTH: f32 = 240.0;
const MAX_BROWSE_TREE_WIDTH: f32 = 640.0;
const HISTORY_PAGE_SIZE: usize = 50;
pub(crate) const BRANCH_MENU_WIDTH: f32 = 190.0;
pub(crate) const BRANCH_MENU_HEIGHT: f32 = 404.0;
pub(crate) const REMOTE_MENU_WIDTH: f32 = 170.0;
pub(crate) const REMOTE_MENU_HEIGHT: f32 = 80.0;
const CHANGE_MENU_WIDTH: f32 = 210.0;
const CHANGE_MENU_HEIGHT: f32 = 255.0;
const CREDENTIAL_MENU_WIDTH: f32 = 180.0;
const CREDENTIAL_MENU_HEIGHT: f32 = 150.0;
pub(crate) const TAG_MENU_WIDTH: f32 = 170.0;
pub(crate) const TAG_MENU_HEIGHT: f32 = 80.0;
pub(crate) const STASH_MENU_WIDTH: f32 = 170.0;
pub(crate) const STASH_MENU_HEIGHT: f32 = 170.0;
const COMMIT_MENU_WIDTH: f32 = 230.0;
const COMMIT_MENU_HEIGHT: f32 = 230.0;
const COMMIT_UNPUSHED_MENU_HEIGHT: f32 = 265.0;
const ENCODING_MENU_WIDTH: f32 = 170.0;
const MENU_VIEWPORT_MARGIN: f32 = 8.0;
const TOOLBAR_FULL_LAYOUT_MIN_WIDTH: f32 = 1760.0;
const TOOLBAR_MORE_MENU_WIDTH: f32 = 190.0;
const TOOLBAR_MORE_MENU_HEIGHT: f32 = 256.0;
const TOOLBAR_MORE_BUTTON_ANCHOR_WIDTH: f32 = 76.0;
const TOOLBAR_MORE_MENU_VERTICAL_OFFSET: f32 = 20.0;
const MAX_CONCURRENT_REPO_LOADS: usize = 2;
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
    CredentialUsername,
    CredentialSecret,
    CredentialKeyPath,
    CredentialPassphrase,
    CredentialRemoteUrl,
    CredentialDisplayName,
    ConflictEditor,
    RemoteBranchName,
    RemoteBranchSearch,
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
}

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
    ConfirmDiscardChange {
        scope: DiffScope,
        target: DiscardTarget,
        paths: Vec<String>,
    },
    CredentialManager,
    CredentialDetails {
        record_id: String,
    },
    CredentialForm {
        editing: Option<String>,
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
    NetworkProxySettings,
    AiProviderSettings,
    ExternalMergeSettings,
    StashForm,
    ConfirmDropStash {
        index: usize,
        message: String,
    },
    RemoteBranchOperation {
        kind: RemoteBranchOperationKind,
    },
    ConfirmConflictResolve,
    // ── 更新对话框 ──
    UpdateSettings,
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
}

#[derive(Clone, Debug)]
struct ChangeContextMenu {
    path: String,
    scope: DiffScope,
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
    let header_count = diff
        .lines
        .iter()
        .take_while(|line| line.kind == DiffLineKind::Header)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarMoreAction {
    Clone,
    Stash,
    Submodule,
    Credentials,
    Proxy,
    AiSettings,
    ExternalMergeSettings,
    UpdateSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarLayoutMode {
    Compact,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ToolbarMoreActionPlacement {
    // 宽屏时把“更多”动作平铺到工具栏，窄屏时收进“更多”菜单。
    show_inline_actions: bool,
    show_more_button: bool,
}

#[derive(Clone, Debug)]
struct ToolbarMoreMenu {
    x: f32,
    y: f32,
    button_x: f32,
    button_y: f32,
}

fn toolbar_more_action_enabled(action: ToolbarMoreAction, repo_open: bool, busy: bool) -> bool {
    match action {
        ToolbarMoreAction::Stash | ToolbarMoreAction::Submodule => repo_open && !busy,
        ToolbarMoreAction::Clone
        | ToolbarMoreAction::Credentials
        | ToolbarMoreAction::Proxy
        | ToolbarMoreAction::AiSettings
        | ToolbarMoreAction::ExternalMergeSettings
        | ToolbarMoreAction::UpdateSettings => !busy,
    }
}

fn toolbar_layout_mode(viewport_width: f32) -> ToolbarLayoutMode {
    if viewport_width >= TOOLBAR_FULL_LAYOUT_MIN_WIDTH {
        ToolbarLayoutMode::Full
    } else {
        ToolbarLayoutMode::Compact
    }
}

fn toolbar_more_action_placement(layout: ToolbarLayoutMode) -> ToolbarMoreActionPlacement {
    match layout {
        ToolbarLayoutMode::Compact => ToolbarMoreActionPlacement {
            show_inline_actions: false,
            show_more_button: true,
        },
        ToolbarLayoutMode::Full => ToolbarMoreActionPlacement {
            show_inline_actions: true,
            show_more_button: false,
        },
    }
}

fn toolbar_more_menu_position(
    click_x: f32,
    click_y: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32) {
    // 以“更多”按钮右侧作为锚点，根层渲染下拉菜单，避免被工具栏裁剪。
    let anchor_right = click_x + TOOLBAR_MORE_BUTTON_ANCHOR_WIDTH / 2.0;
    let raw_x = anchor_right - TOOLBAR_MORE_MENU_WIDTH;
    let raw_y = click_y + TOOLBAR_MORE_MENU_VERTICAL_OFFSET;
    let max_x =
        (viewport_width - TOOLBAR_MORE_MENU_WIDTH - MENU_VIEWPORT_MARGIN).max(MENU_VIEWPORT_MARGIN);
    let max_y = (viewport_height - TOOLBAR_MORE_MENU_HEIGHT - MENU_VIEWPORT_MARGIN)
        .max(MENU_VIEWPORT_MARGIN);

    (
        raw_x.clamp(MENU_VIEWPORT_MARGIN, max_x),
        raw_y.clamp(MENU_VIEWPORT_MARGIN, max_y),
    )
}

fn point_in_toolbar_more_menu(x: f32, y: f32, menu: &ToolbarMoreMenu) -> bool {
    point_in_menu(
        x,
        y,
        menu.x,
        menu.y,
        TOOLBAR_MORE_MENU_WIDTH,
        TOOLBAR_MORE_MENU_HEIGHT,
    ) || point_in_menu(
        x,
        y,
        menu.button_x,
        menu.button_y,
        TOOLBAR_MORE_BUTTON_ANCHOR_WIDTH,
        44.0,
    )
}

fn column_splitter_accepts_mouse_events(active_dialog: bool) -> bool {
    !active_dialog
}

fn column_splitter_should_clear_resize(active_dialog: bool, resizing: bool) -> bool {
    active_dialog && resizing
}

#[cfg(test)]
fn dialog_parent_should_stop_mouse_event(event_name: &str) -> bool {
    event_name == "mouse_down"
}

fn multiline_input_should_scroll(id: FieldId, value: &str) -> bool {
    id == FieldId::ConflictEditor || visual_line_count(value) > 4
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

#[derive(Clone, Debug, Default)]
struct ConflictWorkbenchState {
    selected_path: Option<String>,
    selected_block: usize,
    show_base: bool,
    pending_resolve: Option<PendingConflictResolve>,
    files: BTreeMap<String, ConflictFileView>,
    external_merge_auto_opened: BTreeSet<String>,
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
    pub(crate) log: Vec<String>,
}

fn sync_conflict_state_from_paths(
    main_mode: &mut MainMode,
    state: &mut ConflictWorkbenchState,
    conflict_paths: &[String],
) {
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

    if *main_mode != MainMode::Conflict {
        *main_mode = MainMode::Conflict;
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
    /// 与 HEAD 的差异。
    pub diff: Option<Arc<FileDiff>>,
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
    pub(crate) diff_headers_expanded: bool,
    pub(crate) main_mode: MainMode,
    pub(crate) workflow_state: WorkflowState,
    pub(crate) history_commits: Vec<CommitInfo>,
    pub(crate) history_has_more: bool,
    pub(crate) history_selected_commit: Option<String>,
    pub(crate) history_files: Vec<CommitFileChange>,
    pub(crate) history_selected_file: Option<String>,
    pub(crate) history_diff: Option<Arc<FileDiff>>,
    pub(crate) history_diff_headers_expanded: bool,
    pub(crate) history_loading: HistoryLoading,
    /// 刷新历史时保留旧列表可见，等新数据就绪后直接替换
    pub(crate) history_refreshing: bool,
    pub(crate) history_scope: HistoryScope,
    pub(crate) history_refs_cache: Option<HistoryRefsCache>,
    pub(crate) history_graph_rows: Vec<history_view::CommitGraphRow>,
    pub(crate) stash_preview: StashPreviewState,
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
    pub(crate) busy: bool,
    pub(crate) operation_blocker: OperationBlocker,
    /// 操作遮罩层开始时间；用于延迟显示遮罩层，避免快速完成时一闪而过。
    pub(crate) operation_blocker_started: Option<Instant>,
    operation_kind: OperationKind,
    pub(crate) loading: RepositoryLoading,
    pub(crate) repository_load_id: u64,
    pub(crate) status: String,
    pub(crate) last_error: Option<String>,
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
            diff_headers_expanded: false,
            main_mode: MainMode::Worktree,
            workflow_state: WorkflowState::default(),
            history_commits: Vec::new(),
            history_has_more: false,
            history_selected_commit: None,
            history_files: Vec::new(),
            history_selected_file: None,
            history_diff: None,
            history_diff_headers_expanded: false,
            history_loading: HistoryLoading::default(),
            history_refreshing: false,
            history_scope: HistoryScope::default(),
            history_refs_cache: None,
            history_graph_rows: Vec::new(),
            stash_preview: StashPreviewState::default(),
            branch_sync_status: None,
            branch_sync_loading: false,
            branch_sync_request_id: 0,
            submodule_dialog: SubmoduleDialogState::default(),
            conflict_workbench: ConflictWorkbenchState::default(),
            sidebar_sections: SidebarSectionState::default(),
            full_file_view: false,
            browse: BrowseState::default(),
            busy: false,
            operation_blocker: OperationBlocker::None,
            operation_blocker_started: None,
            operation_kind: OperationKind::Local,
            loading: RepositoryLoading::default(),
            repository_load_id: 0,
            status: "就绪".to_string(),
            last_error: None,
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
    HistoryTop,
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
        load_id: u64,
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
        message: String,
    },
    WorkflowFinished {
        tab_id: RepoTabId,
        message: String,
        snapshot: RepositorySnapshot,
        log: Vec<String>,
    },
    WorkflowFileSelected {
        path: Option<PathBuf>,
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
    AiCommitMessageDelta {
        delta: String,
    },
    AiReviewGenerated {
        review: AiReviewResult,
    },
    AiReviewDelta {
        content_delta: Option<String>,
        reasoning_delta: Option<String>,
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
    ) -> Self {
        Self {
            store,
            storage,
            remote_bindings,
            tx,
            rejected_record_ids: Arc::new(Mutex::new(Vec::new())),
            last_stored_attempt: Arc::new(Mutex::new(None)),
            tab_id,
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
                return Ok(Some(stored.credential));
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
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", url])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("open")
        .arg(url)
        .spawn();
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
        "合并完成" => "正在合并分支",
        "变基完成" => "正在变基分支",
        "变基已中止" => "正在中止变基",
        "变基拉取完成" => "正在变基拉取",
        "切换分支完成" => "正在切换分支",
        "提交完成" => "正在提交",
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
    diff_encoding_preferences: DiffEncodingPreferences,
    diff_cache: RefCell<LruCache<DiffCacheKey, Arc<FileDiff>>>,
    proxy_settings: NetworkProxySettings,
    tabs: Vec<RepoTabState>,
    active_tab: Option<RepoTabId>,
    next_tab_id: u64,
    fallback_tab: RepoTabState,
    restoring_session: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) changes_width: f32,
    pub(crate) workflow_templates_width: f32,
    pub(crate) history_top_height: f32,
    pub(crate) history_files_width: f32,
    pub(crate) browse_tree_width: f32,
    resizing_sidebar_width: Option<ResizeState>,
    resizing_changes_width: Option<ResizeState>,
    resizing_workflow_templates_width: Option<ResizeState>,
    resizing_history_files_width: Option<ResizeState>,
    resizing_history_top_height: Option<ResizeState>,
    resizing_browse_tree_width: Option<ResizeState>,
    scroll_handles: RefCell<HashMap<String, ScrollHandle>>,
    uniform_scroll_handles: RefCell<HashMap<String, UniformListScrollHandle>>,
    pub(crate) scrollbar_drag: Option<ScrollbarDragState>,
    pending_credential: Option<PendingCredential>,
    pending_credentials: VecDeque<PendingCredential>,
    repository_load_queue: VecDeque<RepositoryLoadRequest>,
    active_repository_loads: usize,
    feedbacks: VecDeque<FeedbackMessage>,
    next_feedback_id: u64,
    progress_phase: u64,
    pub(crate) active_dialog: Option<DialogState>,
    pub(crate) branch_context_menu: Option<BranchContextMenu>,
    pub(crate) remote_context_menu: Option<RemoteContextMenu>,
    change_context_menu: Option<ChangeContextMenu>,
    credential_context_menu: Option<CredentialContextMenu>,
    pub(crate) tag_context_menu: Option<TagContextMenu>,
    pub(crate) stash_context_menu: Option<StashContextMenu>,
    pub(crate) commit_context_menu: Option<CommitContextMenu>,
    pub(crate) encoding_menu_target: Option<EncodingMenuTarget>,
    encoding_menu_closed_by_capture: Option<EncodingMenuTarget>,
    toolbar_more_menu: Option<ToolbarMoreMenu>,
    save_credential: bool,
    credential_scope: CredentialScope,
    credential_form_mode: CredentialFormMode,
    credential_use_ssh_agent: bool,
    pub(crate) ssh_credential_discovery: SshCredentialDiscoveryState,
    browse_content_focus: FocusHandle,
    clone_url: TextFieldState,
    clone_path: TextFieldState,
    clone_recursive_submodules: bool,
    branch_name: TextFieldState,
    create_branch_checkout: bool,
    branch_rename: TextFieldState,
    commit_message: TextFieldState,
    stash_message: TextFieldState,
    stash_include_untracked: bool,
    stash_keep_index: bool,
    credential_username: TextFieldState,
    credential_secret: TextFieldState,
    credential_key_path: TextFieldState,
    credential_passphrase: TextFieldState,
    credential_remote_url: TextFieldState,
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
    /// commit message 流式生成时的实时缓冲。
    pub(crate) ai_commit_buffer: String,
    pub(crate) ai_review: Option<Arc<AiReviewResult>>,
    pub(crate) ai_review_loading: bool,
    /// review 流式生成时的实时正文缓冲。
    pub(crate) ai_review_buffer: String,
    /// review 流式生成时的实时思考链缓冲。
    pub(crate) ai_review_reasoning_buffer: String,
    pub(crate) ai_review_expanded: bool,
    // ── 更新状态 ──
    pub(crate) update_preferences: UpdatePreferences,
    pub(crate) update_checking: bool,
    pub(crate) update_downloading: bool,
    pub(crate) available_update: Option<Arc<UpdateManifest>>,
    pub(crate) update_download_progress: Option<String>,
    pub(crate) update_error: Option<String>,
    pub(crate) staging_dir_for_install: Option<PathBuf>,
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
        let proxy_custom = proxy_settings.custom.normalized();
        Self::spawn_event_pump(rx.clone(), cx);
        Self::spawn_ui_tick(tx.clone());
        let tasks = TaskExecutor::new();

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
            diff_encoding_preferences: Self::load_diff_encoding_preferences(&storage),
            diff_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(DIFF_CACHE_CAPACITY)
                    .expect("diff cache capacity must be nonzero"),
            )),
            proxy_settings: proxy_settings.clone(),
            tabs: Vec::new(),
            active_tab: None,
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
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            changes_width: DEFAULT_CHANGES_WIDTH,
            workflow_templates_width: DEFAULT_CHANGES_WIDTH,
            history_top_height: DEFAULT_HISTORY_TOP_HEIGHT,
            history_files_width: DEFAULT_HISTORY_FILES_WIDTH,
            browse_tree_width: DEFAULT_BROWSE_TREE_WIDTH,
            resizing_sidebar_width: None,
            resizing_changes_width: None,
            resizing_workflow_templates_width: None,
            resizing_history_files_width: None,
            resizing_history_top_height: None,
            resizing_browse_tree_width: None,
            scroll_handles: RefCell::new(HashMap::new()),
            uniform_scroll_handles: RefCell::new(HashMap::new()),
            scrollbar_drag: None,
            pending_credential: None,
            pending_credentials: VecDeque::new(),
            repository_load_queue: VecDeque::new(),
            active_repository_loads: 0,
            feedbacks: VecDeque::new(),
            next_feedback_id: 0,
            progress_phase: 0,
            active_dialog: None,
            branch_context_menu: None,
            remote_context_menu: None,
            change_context_menu: None,
            credential_context_menu: None,
            tag_context_menu: None,
            stash_context_menu: None,
            commit_context_menu: None,
            encoding_menu_target: None,
            encoding_menu_closed_by_capture: None,
            toolbar_more_menu: None,
            save_credential: false,
            credential_scope: CredentialScope::RemoteUrl,
            credential_form_mode: CredentialFormMode::Https,
            credential_use_ssh_agent: false,
            ssh_credential_discovery: SshCredentialDiscoveryState::default(),
            browse_content_focus: cx.focus_handle(),
            clone_url: TextFieldState::new(cx, "远程仓库 URL"),
            clone_path: TextFieldState::new(cx, "克隆到父文件夹"),
            clone_recursive_submodules: default_clone_recursive_submodules(),
            branch_name: TextFieldState::new(cx, "新分支名称"),
            create_branch_checkout: true,
            branch_rename: TextFieldState::new(cx, "重命名为"),
            commit_message: TextFieldState::new(cx, "提交信息"),
            stash_message: TextFieldState::new(cx, "贮藏说明（可选）"),
            stash_include_untracked: false,
            stash_keep_index: false,
            credential_username: TextFieldState::new(cx, "用户名"),
            credential_secret: TextFieldState::new(cx, "密码或 PAT").secret(),
            credential_key_path: TextFieldState::new(cx, "SSH 私钥路径"),
            credential_passphrase: TextFieldState::new(cx, "SSH 密码短语").secret(),
            credential_remote_url: TextFieldState::new(cx, "适用远端 URL"),
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
            ai_commit_buffer: String::new(),
            ai_review: None,
            ai_review_loading: false,
            ai_review_buffer: String::new(),
            ai_review_reasoning_buffer: String::new(),
            ai_review_expanded: false,
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
        // 启动时自动检查更新
        if view.update_preferences.auto_check {
            view.start_update_check();
        }
        view
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
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(index) = snapshot
            .branches
            .iter()
            .filter(|branch| branch.kind == BranchKind::Local)
            .position(|branch| branch.is_head)
        else {
            return;
        };

        let handle = self.scroll_handle("local-branch-list");
        let bounds = handle.bounds();
        let max_offset = f32::from(handle.max_offset().height).max(0.0);
        if max_offset <= 1.0 || f32::from(bounds.size.height) <= 1.0 {
            handle.scroll_to_top_of_item(index);
            return;
        }

        let item_top = handle
            .bounds_for_item(index)
            .map(|item_bounds| f32::from(item_bounds.top() - bounds.top()))
            .unwrap_or_else(|| {
                let list_padding = 8.0;
                let row_gap = 4.0;
                list_padding + index as f32 * (NAV_ROW_HEIGHT + row_gap)
            });
        let row_height = handle
            .bounds_for_item(index)
            .map(|item_bounds| f32::from(item_bounds.size.height))
            .unwrap_or(NAV_ROW_HEIGHT);
        let viewport_height = f32::from(bounds.size.height);
        let target_scroll =
            (item_top - viewport_height / 2.0 + row_height / 2.0).clamp(0.0, max_offset);
        handle.set_offset(point(px(0.0), px(-target_scroll)));
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
        let key = normalize_repo_path(&path);
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.path_key().as_deref() == Some(key.as_str()))
        {
            self.active_tab = Some(tab.id);
            self.save_session();
            return tab.id;
        }

        let id = RepoTabId(self.next_tab_id);
        self.next_tab_id = self.next_tab_id.wrapping_add(1).max(1);
        self.tabs.push(RepoTabState::new(id, Some(path)));
        self.active_tab = Some(id);
        self.save_session();
        id
    }

    fn activate_tab(&mut self, tab_id: RepoTabId) {
        if self.active_tab == Some(tab_id) || self.tab(tab_id).is_none() {
            return;
        }
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
        // 切换仓库后默认打开提交记录
        self.main_mode = MainMode::History;
        self.ensure_history_loaded();
        self.sync_conflict_mode_with_snapshot();
        self.save_session();
        // 切换仓库后自动本地刷新
        self.refresh();
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

    fn enqueue_credential_request(&mut self, pending: PendingCredential) {
        if pending
            .tab_id
            .is_some_and(|tab_id| self.tab(tab_id).is_none())
        {
            return;
        }
        if self.pending_credential.is_none() {
            self.pending_credential = Some(pending);
        } else {
            self.pending_credentials.push_back(pending);
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
                self.feedbacks.retain(|feedback| !feedback.is_expired(now));
                self.sync_conflict_editor_into_state();
                // 操作遮罩层有延迟显示阈值；UiTick 到达阈值时触发重绘让遮罩层出现。
                if self.active_operation_blocker_message().is_some() {
                    cx.notify();
                }
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
                        this.sync_conflict_mode_with_snapshot();
                        this.scroll_local_branch_to_current();
                        this.reload_history_if_active();
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
                let should_refresh_submodules = operation_refreshes_submodule_dialog(&message)
                    && self.active_dialog == Some(DialogState::SubmoduleManager);
                let has_snapshot = snapshot.is_some();
                let has_diff = diff.is_some();
                let mut full_status_request = None;
                let mut sync_request = None;
                self.apply_status_event(tab_id, |this| {
                    this.busy = false;
                    this.operation_blocker = OperationBlocker::None;
                    this.operation_blocker_started = None;
                    this.remote_branch_operation.refreshing = false;
                    this.operation_kind = OperationKind::Local;
                    this.loading = RepositoryLoading::default();
                    this.status = message;
                    if let Some(snapshot) = snapshot {
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
                        this.prune_stash_preview();
                        this.prune_change_selection();
                        this.sync_conflict_mode_with_snapshot();
                        this.refresh_history();
                        this.scroll_local_branch_to_current();
                        this.reload_history_if_active();
                        if let Some(tab_id) = tab_id {
                            full_status_request = this
                                .repo_path
                                .clone()
                                .map(|path| (tab_id, path, this.repository_load_id));
                            this.loading.status_full = true;
                        }
                        sync_request = this.prepare_branch_sync_status_request();
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
                        this.reset_uniform_scroll("diff-scroll");
                    }
                });
                if let Some((tab_id, path, load_id)) = full_status_request {
                    self.load_full_status_for_tab(tab_id, path, load_id, "变更已补全".to_string());
                }
                if let Some((tab_id, path, remote, load_id, request_id)) = sync_request {
                    self.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
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
                self.busy = false;
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
                    self.status = format!(
                        "已发现 {key_count} 个 SSH 私钥，Agent 中有 {agent_count} 个身份"
                    );
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
                load_id,
            } => {
                self.with_tab_context(tab_id, |this| {
                    if load_id == this.repository_load_id && scope == this.history_scope {
                        this.history_loading.commits = false;
                        this.history_refs_cache = Some(refs_cache);
                        this.history_has_more = has_more;
                        if append {
                            this.history_commits.extend(commits);
                        } else {
                            this.history_commits = commits;
                            let was_refreshing = this.history_refreshing;
                            if was_refreshing {
                                // 刷新时保留选中提交（若仍存在于新列表）
                                let still_exists =
                                    this.history_selected_commit.as_ref().is_some_and(|oid| {
                                        this.history_commits.iter().any(|c| c.oid == oid.as_str())
                                    });
                                if !still_exists {
                                    this.history_selected_commit = None;
                                }
                                // 详情可能已过时，清空以重新加载
                                this.history_files.clear();
                                this.history_selected_file = None;
                                this.history_diff = None;
                            } else {
                                // 非刷新（scope 切换/初始加载）：全部重置
                                this.history_selected_commit = None;
                                this.history_files.clear();
                                this.history_selected_file = None;
                                this.history_diff = None;
                            }
                            this.history_refreshing = false;
                        }
                        this.history_graph_rows =
                            history_view::commit_graph_rows(&this.history_commits);

                        if this.history_selected_commit.is_none() {
                            if let Some(commit) = this.history_commits.first() {
                                this.select_history_commit(commit.oid.clone());
                            } else {
                                this.status = "当前分支暂无提交记录".to_string();
                            }
                        } else {
                            this.status = "提交记录已加载".to_string();
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

                        if let Some(file) = this.history_files.first() {
                            this.select_history_file(file.path.clone());
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
                    if load_id == this.repository_load_id {
                        this.history_loading = HistoryLoading::default();
                        this.history_refreshing = false;
                        this.stash_preview.loading_files = false;
                        this.stash_preview.loading_diff = false;
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
                        this.browse.content = Some(Arc::new(content));
                        this.status = "文件内容已加载".to_string();
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
                        this.browse.diff = Some(Arc::new(diff));
                        this.browse.diff_headers_expanded = false;
                        this.status = "文件差异已加载".to_string();
                    }
                });
            }
            UiEvent::OperationFailed { tab_id, error } => {
                let toast_message = error.clone();
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
                self.enqueue_credential_request(PendingCredential {
                    tab_id,
                    request,
                    response_tx,
                });
                self.prepare_current_credential_prompt();
            }
            UiEvent::ProxyTestFinished { message } => {
                let toast_message = message.clone();
                self.busy = false;
                self.operation_blocker = OperationBlocker::None;
                self.operation_blocker_started = None;
                self.status = message;
                self.last_error = None;
                self.notify_success(toast_message, cx);
            }
            UiEvent::WorkflowProgress { tab_id, message } => {
                self.with_tab_context(tab_id, |this| {
                    this.status = message.clone();
                    this.workflow_state.log.push(message);
                });
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
                    this.reload_history_if_active();
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
            UiEvent::WorkflowFileSelected { path } => {
                if let Some(path) = path {
                    self.set_main_mode(MainMode::Workflow);
                    self.load_workflow_file(path, cx);
                } else {
                    self.status = "已取消选择工作流文件".to_string();
                    self.last_error = None;
                }
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
            UiEvent::AiCommitMessageGenerated { message } => {
                self.ai_commit_loading = false;
                self.ai_commit_buffer.clear();
                // 流式期间已逐段填入输入框，这里用最终结果做一次干净覆盖，
                // 确保最终的 trim 和换行规范化。
                self.commit_message.set_value(message);
                self.status = "AI 已生成提交信息".into();
                self.last_error = None;
            }
            UiEvent::AiCommitMessageDelta { delta } => {
                // 直接把增量追加到输入框，让用户实时看到生成内容，
                // 而不是等全部完成后才一次性填入。
                self.ai_commit_buffer.push_str(&delta);
                self.commit_message.value.push_str(&delta);
                self.commit_message.caret = self.commit_message.value.len();
            }
            UiEvent::AiReviewGenerated { review } => {
                self.ai_review_loading = false;
                self.ai_review_buffer.clear();
                self.ai_review_reasoning_buffer.clear();
                self.ai_review = Some(Arc::new(review));
                self.status = "AI 评审已生成".into();
                self.last_error = None;
            }
            UiEvent::AiReviewDelta {
                content_delta,
                reasoning_delta,
            } => {
                if let Some(delta) = content_delta {
                    self.ai_review_buffer.push_str(&delta);
                }
                if let Some(delta) = reasoning_delta {
                    self.ai_review_reasoning_buffer.push_str(&delta);
                }
            }
            UiEvent::AiRequestFailed { error } => {
                self.ai_commit_loading = false;
                self.ai_review_loading = false;
                self.ai_commit_buffer.clear();
                self.ai_review_buffer.clear();
                self.ai_review_reasoning_buffer.clear();
                // 测试连接失败时也要解锁 busy，否则弹窗按钮永久禁用。
                self.busy = false;
                self.last_error = Some(error);
            }
            UiEvent::AiConnectionTested { message } => {
                self.busy = false;
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
            UiEvent::UpdateCheckFailed { error } => {
                self.update_checking = false;
                if !error.is_empty() {
                    self.update_error = Some(error.clone());
                    self.status = error.clone();
                    if let Some(message) = update_check_toast_message(&error) {
                        self.notify_toast(AppToastKind::Info, message, cx);
                    }
                }
            }
            UiEvent::UpdateDownloadProgress { downloaded, total } => {
                let mb_down = downloaded as f64 / 1_048_576.0;
                let mb_total = total as f64 / 1_048_576.0;
                self.update_download_progress = Some(format!("{:.1} MB / {:.1} MB", mb_down, mb_total));
            }
            UiEvent::UpdateReadyToInstall { staging_dir, manifest } => {
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
        }
        cx.notify();
    }

    fn merge_metadata_snapshot(&mut self, snapshot: RepositorySnapshot) {
        let mut merged = self.snapshot.take().unwrap_or_default();
        merged.path = snapshot.path;
        merged.head = snapshot.head;
        merged.branches = snapshot.branches;
        merged.remotes = snapshot.remotes;
        merged.tags = snapshot.tags;
        merged.stashes = snapshot.stashes;
        merged.conflicts = snapshot.conflicts;
        self.repo_path = Some(merged.path.clone());
        self.sync_selected_remote(&merged);
        self.change_indexes = ChangeListIndexes::rebuild(&merged.changes);
        self.snapshot = Some(merged);
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
        let tab = self.active_tab_state_mut();
        sync_conflict_state_from_paths(
            &mut tab.main_mode,
            &mut tab.conflict_workbench,
            &conflict_paths,
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
        self.ensure_conflict_views_loaded();
        if self.conflict_workbench.files.contains_key(&path) {
            self.sync_conflict_editor_from_state();
            self.maybe_auto_open_external_merge_for_selected_conflict();
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
            if self.active_dialog == Some(DialogState::NetworkProxySettings) {
                self.save_network_proxy_settings();
            }
        } else if matches!(field, FieldId::ExternalMergeIntellijPath) {
            if self.active_dialog == Some(DialogState::ExternalMergeSettings) {
                self.save_external_merge_settings_from_form_and_resume();
            }
        } else if matches!(
            field,
            FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
        ) {
            if self.active_dialog == Some(DialogState::AiProviderSettings) {
                self.save_ai_provider_settings_from_form();
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
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(-1, false);
            cx.notify();
        }
    }

    fn text_down(&mut self, _: &TextDown, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(1, false);
            cx.notify();
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
            ) && self.active_dialog == Some(DialogState::NetworkProxySettings)
            {
                self.save_network_proxy_settings();
                cx.notify();
            } else if matches!(
                field,
                FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
            ) && self.active_dialog == Some(DialogState::AiProviderSettings)
            {
                self.save_ai_provider_settings_from_form();
                cx.notify();
            } else if field == FieldId::ExternalMergeIntellijPath
                && self.active_dialog == Some(DialogState::ExternalMergeSettings)
            {
                self.save_external_merge_settings_from_form();
                cx.notify();
            }
        }
    }

    fn on_browse_content_copy(
        &mut self,
        _: &BrowseContentCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browse_content_copy(cx);
    }

    fn on_browse_content_select_all(
        &mut self,
        _: &BrowseContentSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browse_content_select_all(cx);
    }

    fn focused_field(&self, window: &Window, _cx: &App) -> Option<FieldId> {
        [
            (FieldId::CloneUrl, &self.clone_url),
            (FieldId::ClonePath, &self.clone_path),
            (FieldId::BranchName, &self.branch_name),
            (FieldId::BranchRename, &self.branch_rename),
            (FieldId::RemoteName, &self.remote_name),
            (FieldId::RemoteUrl, &self.remote_url),
            (FieldId::CommitMessage, &self.commit_message),
            (FieldId::StashMessage, &self.stash_message),
            (FieldId::CredentialUsername, &self.credential_username),
            (FieldId::CredentialSecret, &self.credential_secret),
            (FieldId::CredentialKeyPath, &self.credential_key_path),
            (FieldId::CredentialPassphrase, &self.credential_passphrase),
            (FieldId::CredentialRemoteUrl, &self.credential_remote_url),
            (
                FieldId::CredentialDisplayName,
                &self.credential_display_name,
            ),
            (FieldId::ConflictEditor, &self.conflict_editor),
            (FieldId::RemoteBranchName, &self.remote_branch_name),
            (FieldId::RemoteBranchSearch, &self.remote_branch_search),
            (
                FieldId::SidebarLocalBranchSearch,
                &self.sidebar_local_branch_search,
            ),
            (
                FieldId::SidebarRemoteBranchSearch,
                &self.sidebar_remote_branch_search,
            ),
            (FieldId::ProxyHttpUrl, &self.proxy_http_url),
            (FieldId::ProxyHttpsUrl, &self.proxy_https_url),
            (FieldId::ProxySocks5Url, &self.proxy_socks5_url),
            (FieldId::AiBaseUrl, &self.ai_base_url),
            (FieldId::AiApiKey, &self.ai_api_key),
            (FieldId::AiModel, &self.ai_model),
            (
                FieldId::ExternalMergeIntellijPath,
                &self.external_merge_intellij_path,
            ),
        ]
        .into_iter()
        .find_map(|(id, field)| field.focus.is_focused(window).then_some(id))
        .or_else(|| self.focused_workflow_input(window))
    }

    fn field(&self, id: FieldId) -> &TextFieldState {
        match id {
            FieldId::CloneUrl => &self.clone_url,
            FieldId::ClonePath => &self.clone_path,
            FieldId::BranchName => &self.branch_name,
            FieldId::BranchRename => &self.branch_rename,
            FieldId::RemoteName => &self.remote_name,
            FieldId::RemoteUrl => &self.remote_url,
            FieldId::CommitMessage => &self.commit_message,
            FieldId::StashMessage => &self.stash_message,
            FieldId::CredentialUsername => &self.credential_username,
            FieldId::CredentialSecret => &self.credential_secret,
            FieldId::CredentialKeyPath => &self.credential_key_path,
            FieldId::CredentialPassphrase => &self.credential_passphrase,
            FieldId::CredentialRemoteUrl => &self.credential_remote_url,
            FieldId::CredentialDisplayName => &self.credential_display_name,
            FieldId::ConflictEditor => &self.conflict_editor,
            FieldId::RemoteBranchName => &self.remote_branch_name,
            FieldId::RemoteBranchSearch => &self.remote_branch_search,
            FieldId::SidebarLocalBranchSearch => &self.sidebar_local_branch_search,
            FieldId::SidebarRemoteBranchSearch => &self.sidebar_remote_branch_search,
            FieldId::ProxyHttpUrl => &self.proxy_http_url,
            FieldId::ProxyHttpsUrl => &self.proxy_https_url,
            FieldId::ProxySocks5Url => &self.proxy_socks5_url,
            FieldId::AiBaseUrl => &self.ai_base_url,
            FieldId::AiApiKey => &self.ai_api_key,
            FieldId::AiModel => &self.ai_model,
            FieldId::ExternalMergeIntellijPath => &self.external_merge_intellij_path,
            FieldId::WorkflowInput(index) => self.workflow_input_field(index),
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
            FieldId::CredentialUsername => &mut self.credential_username,
            FieldId::CredentialSecret => &mut self.credential_secret,
            FieldId::CredentialKeyPath => &mut self.credential_key_path,
            FieldId::CredentialPassphrase => &mut self.credential_passphrase,
            FieldId::CredentialRemoteUrl => &mut self.credential_remote_url,
            FieldId::CredentialDisplayName => &mut self.credential_display_name,
            FieldId::ConflictEditor => &mut self.conflict_editor,
            FieldId::RemoteBranchName => &mut self.remote_branch_name,
            FieldId::RemoteBranchSearch => &mut self.remote_branch_search,
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

    fn open_repo_in_file_manager(&mut self, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            self.notify_warning("请先打开一个仓库", cx);
            return;
        };
        if !repo_path.exists() {
            let message = format!("仓库目录不存在：{}", repo_path.display());
            self.last_error = Some(message.clone());
            self.notify_error(message, cx);
            return;
        }
        if !repo_path.is_dir() {
            let message = format!("仓库路径不是文件夹：{}", repo_path.display());
            self.last_error = Some(message.clone());
            self.notify_error(message, cx);
            return;
        }

        // 顶部路径胶囊只负责快速跳转，实际打开目录的跨平台命令集中放在 system 模块。
        match system::open_directory(&repo_path) {
            Ok(()) => {
                self.status = "仓库目录已在资源管理器中打开".into();
                self.last_error = None;
                self.notify_success("仓库目录已在资源管理器中打开", cx);
            }
            Err(err) => {
                let message = format!("仓库目录打开失败：{err}");
                self.last_error = Some(message.clone());
                self.notify_error(message, cx);
            }
        }
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
        self.remote_branch_operation.branch_dropdown_open = false;
        self.remote_branch_search.clear();
        self.branch_context_menu = None;
        self.remote_context_menu = None;
        self.change_context_menu = None;
        self.credential_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        self.encoding_menu_closed_by_capture = None;
        self.toolbar_more_menu = None;
    }

    pub(crate) fn toggle_sidebar_section(&mut self, section: SidebarSection) {
        self.close_popups();
        self.sidebar_sections.toggle(section);
    }

    pub(crate) fn close_dialog(&mut self) {
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

    fn close_credential_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.credential_context_menu.is_some() {
            self.credential_context_menu = None;
            cx.notify();
        }
    }

    fn open_credential_manager(&mut self) {
        self.close_popups();
        self.active_dialog = Some(DialogState::CredentialManager);
        self.reload_credential_records("凭据列表已加载");
    }

    pub(crate) fn open_network_proxy_settings(&mut self) {
        self.close_popups();
        self.reset_proxy_form_from_settings();
        self.active_dialog = Some(DialogState::NetworkProxySettings);
        self.status = "代理设置已打开".into();
        self.last_error = None;
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

    pub(crate) fn save_network_proxy_settings_and_close(&mut self) {
        self.save_network_proxy_settings();
        if self.last_error.is_none() {
            self.active_dialog = None;
        }
    }

    pub(crate) fn open_ai_provider_settings(&mut self) {
        self.close_popups();
        self.reset_ai_form_from_settings();
        self.active_dialog = Some(DialogState::AiProviderSettings);
        self.status = "AI 设置已打开".into();
        self.last_error = None;
    }

    pub(crate) fn open_external_merge_settings(&mut self) {
        self.close_popups();
        self.reset_external_merge_form_from_settings();
        self.active_dialog = Some(DialogState::ExternalMergeSettings);
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

    pub(crate) fn save_external_merge_settings_from_form_and_close(&mut self) {
        self.save_external_merge_settings_from_form();
        if self.last_error.is_none() {
            self.active_dialog = None;
        }
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

    pub(crate) fn open_update_settings(&mut self) {
        self.close_popups();
        self.active_dialog = Some(DialogState::UpdateSettings);
        self.last_error = None;
    }

    pub(crate) fn start_update_check(&mut self) {
        if self.update_checking { return; }
        self.update_checking = true;
        self.update_error = None;
        self.status = "检查更新中".into();

        let tx = self.tx.clone();
        let preferences = self.update_preferences.clone();
        let proxy_settings = self.proxy_settings.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let sources = update::default_manifest_sources();
            match update::check_for_update(&sources, &preferences, &proxy_settings) {
                Ok(UpdateCheckResult::UpdateAvailable { manifest, asset }) => {
                    send_ui_event(&tx, UiEvent::UpdateCheckFinished {
                        manifest: Arc::new(manifest),
                        asset,
                    });
                }
                Ok(UpdateCheckResult::UpToDate) => {
                    send_ui_event(&tx, UiEvent::UpdateCheckFailed {
                        error: "当前已是最新版本".into(),
                    });
                }
                Ok(UpdateCheckResult::SkippedVersion) => {
                    // 用户跳过了此版本，静默忽略
                    send_ui_event(&tx, UiEvent::UpdateCheckFailed {
                        error: String::new(),
                    });
                }
                Err(err) => {
                    send_ui_event(&tx, UiEvent::UpdateCheckFailed {
                        error: err.to_string(),
                    });
                }
            }
        });
    }

    pub(crate) fn start_update_download(&mut self) {
        let Some(manifest) = self.available_update.clone() else { return; };
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
            match update::download_update(&asset, &config_dir, &proxy_settings, Some(&on_progress)) {
                Ok((zip_path, computed_sha256)) => {
                    // SHA-256 校验
                    if computed_sha256 != asset.sha256 {
                        send_ui_event(&tx, UiEvent::UpdateInstallFailed {
                            error: "更新包 SHA-256 校验失败，文件可能被篡改".into(),
                        });
                        return;
                    }
                    // 解压 staging
                    let version = manifest.version.clone();
                    match update::prepare_staging(&zip_path, &version, &config_dir) {
                        Ok(staging_dir) => {
                            send_ui_event(&tx, UiEvent::UpdateReadyToInstall {
                                staging_dir,
                                manifest,
                            });
                        }
                        Err(err) => {
                            send_ui_event(&tx, UiEvent::UpdateInstallFailed {
                                error: format!("更新包解压失败：{err}"),
                            });
                        }
                    }
                }
                Err(err) => {
                    send_ui_event(&tx, UiEvent::UpdateInstallFailed {
                        error: format!("更新包下载失败：{err}"),
                    });
                }
            }
        });
    }

    pub(crate) fn install_update(&mut self, staging_dir: &Path, _version: &str) {
        // 检查写入权限
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("khaslana.exe"));
        let exe_dir = current_exe.parent().unwrap_or_else(|| Path::as_ref(Path::new(".")));

        // 尝试在 exe 目录创建临时文件来验证写入权限
        let test_file = exe_dir.join(".khaslana_update_test");
        let writable = fs::File::create(&test_file).is_ok();
        let _ = fs::remove_file(&test_file);

        if !writable {
            let version = self.available_update
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

        let _ = Command::new(&new_updater_str)
            .args([
                "--pid", &pid_str,
                "--target-exe", &current_exe_str,
                "--new-exe", &new_exe_str,
                "--new-updater", &new_updater_str,
                "--backup-dir", &backup_dir_str,
                "--restart",
            ])
            .spawn();

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
        if let Err(err) = self.storage.save_update_preferences(&self.update_preferences) {
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

    pub(crate) fn save_ai_provider_settings_from_form_and_close(&mut self) {
        self.save_ai_provider_settings_from_form();
        if self.last_error.is_none() {
            self.active_dialog = None;
        }
    }

    pub(crate) fn test_network_proxy_settings(&mut self) {
        if self.busy {
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
        self.busy = true;
        self.status = "正在测试代理连接".into();
        self.last_error = None;
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
                .or_else(|| snapshot.remotes.iter().find(|remote| remote.name == "origin"))
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
        self.active_dialog = Some(DialogState::CredentialManager);
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
            self.discover_ssh_credentials_if_needed();
        }
    }

    fn discover_ssh_credentials_if_needed(&mut self) {
        if self.ssh_credential_discovery.result.is_none()
            && !self.ssh_credential_discovery.loading
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
            self.last_error = Some("凭据类型与远端 URL 协议不匹配".into());
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
                    && let Err(error) = ssh_credentials::validate_ssh_private_key_path(Path::new(&key_path))
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
            Ok(_) => {
                self.reset_credential_form();
                self.active_dialog = Some(DialogState::CredentialManager);
                self.last_error = None;
                self.reload_credential_records("凭据已添加");
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
                self.active_dialog = Some(DialogState::CredentialManager);
                self.credential_context_menu = None;
                self.reload_credential_records("凭据已删除");
            }
            Err(err) => {
                self.active_dialog = Some(DialogState::CredentialManager);
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn test_credential_record(&mut self, record_id: String) {
        if self.busy {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        self.busy = true;
        self.status = "正在测试凭据连接".to_string();
        self.last_error = None;
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
            self.active_tab = Some(tab.id);
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

    fn refresh(&mut self) {
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

    fn fetch(&mut self) {
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

    fn open_remote_branch_operation(&mut self, kind: RemoteBranchOperationKind) {
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
        self.with_repo_blocking("合并完成", move |service, repo| {
            service.merge_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_remote_branch(&mut self, name: String) {
        self.close_browse_if_comparing();
        self.with_repo_blocking("远端分支已拉取到本地", move |service, repo| {
            service.checkout_remote_branch(repo, &BranchName::new(name))
        });
    }

    pub(crate) fn checkout_tag(&mut self, name: String) {
        self.close_browse_if_comparing();
        self.with_repo_blocking("检出标签完成", move |service, repo| {
            service.checkout_tag(repo, &TagName::new(name))
        });
    }

    pub(crate) fn apply_stash(&mut self, index: usize) {
        self.with_repo_blocking("应用贮藏完成", move |service, repo| {
            service.apply_stash(repo, index)
        });
    }

    pub(crate) fn pop_stash(&mut self, index: usize) {
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
        self.with_repo_blocking("分支已重置", move |service, repo| {
            service.reset_to_commit(repo, &oid, mode)
        });
    }

    fn revert_commit(&mut self, oid: String) {
        self.with_repo_blocking("回滚提交完成", move |service, repo| {
            service.revert_commit(repo, &oid)
        });
    }

    fn revert_merge_commit(&mut self, oid: String) {
        self.with_repo_blocking("撤销合并完成", move |service, repo| {
            service.revert_merge_commit(repo, &oid)
        });
    }

    fn uncommit_to_staged(&mut self, oid: String) {
        self.with_repo_blocking("提交已还原到暂存区", move |service, repo| {
            service.uncommit_to_staged(repo, &oid)
        });
    }

    fn discard_change(&mut self, paths: Vec<String>, scope: DiffScope, target: DiscardTarget) {
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
        self.encoding_menu_target = None;
        self.active_dialog = None;
        let (x, y) = clamped_menu_position(event, window, CHANGE_MENU_WIDTH, CHANGE_MENU_HEIGHT);
        self.change_context_menu = Some(ChangeContextMenu { path, scope, x, y });
    }

    fn mouse_down_inside_context_menu(&self, event: &MouseDownEvent) -> bool {
        let x: f32 = event.position.x.into();
        let y: f32 = event.position.y.into();
        self.branch_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, BRANCH_MENU_WIDTH, BRANCH_MENU_HEIGHT)
        }) || self.remote_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, REMOTE_MENU_WIDTH, REMOTE_MENU_HEIGHT)
        }) || self.change_context_menu.as_ref().is_some_and(|menu| {
            point_in_menu(x, y, menu.x, menu.y, CHANGE_MENU_WIDTH, CHANGE_MENU_HEIGHT)
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
            .commit_context_menu
            .as_ref()
            .is_some_and(|menu| point_in_menu(x, y, menu.x, menu.y, COMMIT_MENU_WIDTH, menu.height))
            || self
                .toolbar_more_menu
                .as_ref()
                .is_some_and(|menu| point_in_toolbar_more_menu(x, y, menu))
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
        self.close_popups();
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
            ResizeTarget::HistoryTop => self.resizing_history_top_height = Some(state),
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = Some(state),
        }
    }

    fn update_resize_column(&mut self, target: ResizeTarget, event: &MouseMoveEvent) {
        let Some(resize) = self.resize_state(target) else {
            return;
        };
        let current_x: f32 = event.position.x.into();
        let delta = current_x - resize.start_x;
        match target {
            ResizeTarget::HistoryTop => {
                let current_y: f32 = event.position.y.into();
                let delta = current_y - resize.start_y;
                let height = (resize.start_height + delta)
                    .clamp(MIN_HISTORY_TOP_HEIGHT, MAX_HISTORY_TOP_HEIGHT);
                self.set_row_height(target, height);
            }
            ResizeTarget::HistoryFiles | ResizeTarget::WorkflowTemplates => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_HISTORY_FILES_WIDTH, MAX_HISTORY_FILES_WIDTH);
                self.set_column_width(target, width);
            }
            ResizeTarget::BrowseFiles => {
                let width = (resize.start_width + delta)
                    .clamp(MIN_BROWSE_TREE_WIDTH, MAX_BROWSE_TREE_WIDTH);
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
            ResizeTarget::HistoryTop => self.resizing_history_top_height = None,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width = None,
        }
    }

    fn reset_resize_target(&mut self, target: ResizeTarget) {
        self.finish_resize_column(target);
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = DEFAULT_SIDEBAR_WIDTH,
            ResizeTarget::Changes => self.changes_width = DEFAULT_CHANGES_WIDTH,
            ResizeTarget::WorkflowTemplates => {
                self.workflow_templates_width = DEFAULT_CHANGES_WIDTH
            }
            ResizeTarget::HistoryFiles => self.history_files_width = DEFAULT_HISTORY_FILES_WIDTH,
            ResizeTarget::HistoryTop => self.history_top_height = DEFAULT_HISTORY_TOP_HEIGHT,
            ResizeTarget::BrowseFiles => self.browse_tree_width = DEFAULT_BROWSE_TREE_WIDTH,
        }
    }

    fn column_width(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width,
            ResizeTarget::Changes => self.changes_width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width,
            ResizeTarget::HistoryFiles => self.history_files_width,
            ResizeTarget::HistoryTop => 0.0,
            ResizeTarget::BrowseFiles => self.browse_tree_width,
        }
    }

    fn set_column_width(&mut self, target: ResizeTarget, width: f32) {
        match target {
            ResizeTarget::Sidebar => self.sidebar_width = width,
            ResizeTarget::Changes => self.changes_width = width,
            ResizeTarget::WorkflowTemplates => self.workflow_templates_width = width,
            ResizeTarget::HistoryFiles => self.history_files_width = width,
            ResizeTarget::HistoryTop => {}
            ResizeTarget::BrowseFiles => self.browse_tree_width = width,
        }
    }

    fn row_height(&self, target: ResizeTarget) -> f32 {
        match target {
            ResizeTarget::HistoryTop => self.history_top_height,
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::BrowseFiles => 0.0,
        }
    }

    fn set_row_height(&mut self, target: ResizeTarget, height: f32) {
        match target {
            ResizeTarget::HistoryTop => self.history_top_height = height,
            ResizeTarget::Sidebar
            | ResizeTarget::Changes
            | ResizeTarget::WorkflowTemplates
            | ResizeTarget::HistoryFiles
            | ResizeTarget::BrowseFiles => {}
        }
    }

    fn resize_state(&self, target: ResizeTarget) -> Option<ResizeState> {
        match target {
            ResizeTarget::Sidebar => self.resizing_sidebar_width,
            ResizeTarget::Changes => self.resizing_changes_width,
            ResizeTarget::WorkflowTemplates => self.resizing_workflow_templates_width,
            ResizeTarget::HistoryFiles => self.resizing_history_files_width,
            ResizeTarget::HistoryTop => self.resizing_history_top_height,
            ResizeTarget::BrowseFiles => self.resizing_browse_tree_width,
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
        self.close_popups();
        if self.main_mode == MainMode::Conflict {
            self.ensure_conflict_views_loaded();
            self.sync_conflict_editor_from_state();
        }
        if self.main_mode == MainMode::Workflow {
            self.refresh_workflow_templates();
        }
        if self.main_mode == MainMode::History {
            self.ensure_history_loaded();
        }
    }

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

    /// 刷新历史时保留旧列表可见，等新数据就绪后直接替换
    fn refresh_history(&mut self) {
        // 保留：commits、graph_rows、has_more、refs_cache、selected_commit
        // 清空详情（操作后可能已过时）
        self.history_files.clear();
        self.history_selected_file = None;
        self.history_diff = None;
        self.history_diff_headers_expanded = false;
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
        self.status = "已退出分支浏览".to_string();
    }

    /// 对比视图依赖具体分支的差异；切换分支/检出后旧差异会失效，
    /// 命中对比视图时关闭它，回到工作区，让新 HEAD 的状态正常展示。
    fn close_browse_if_comparing(&mut self) {
        if self.main_mode == MainMode::Browse && self.browse.list_mode == BrowseListMode::Compare {
            self.close_browse();
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

    /// Ctrl+C：把选中的行用 `\n` 拼接写入剪贴板。
    pub(crate) fn browse_content_copy(&mut self, cx: &mut Context<Self>) {
        let (Some(start), Some(end)) = (self.browse.sel_start, self.browse.sel_end) else {
            return;
        };
        let Some(content) = self.browse.content.as_ref() else {
            return;
        };
        if content.is_binary {
            return;
        }
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let lines: Vec<&str> = content
            .lines
            .get(lo..=hi)
            .map(|slice| slice.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if lines.is_empty() {
            return;
        }
        let text = lines.join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notify_success(
            "已复制 {count} 行".replace("{count}", &lines.len().to_string()),
            cx,
        );
    }

    /// Ctrl+A：全选。
    pub(crate) fn browse_content_select_all(&mut self, cx: &mut Context<Self>) {
        let line_count = match self.browse.content.as_ref() {
            Some(content) if !content.is_binary => content.lines.len(),
            _ => return,
        };
        if line_count == 0 {
            return;
        }
        self.browse.sel_start = Some(0);
        self.browse.sel_end = Some(line_count - 1);
        cx.notify();
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
        self.clear_history();
        self.status = format!("提交记录范围已切换为{}", scope.label());
        self.load_history_page(false);
    }

    fn reload_history_if_active(&mut self) {
        if self.main_mode == MainMode::History {
            self.load_history_page(false);
        }
    }

    fn ensure_history_loaded(&mut self) {
        if self.main_mode == MainMode::History
            && self.repo_path.is_some()
            && self.history_commits.is_empty()
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
        let refs_cache = self.history_refs_cache.clone();
        let offset = if append {
            self.history_commits.len()
        } else {
            0
        };
        let load_id = self.repository_load_id;
        self.history_loading.commits = true;
        self.status = if append {
            "正在加载更多提交记录".to_string()
        } else {
            "正在加载提交记录".to_string()
        };
        self.last_error = None;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let (mut commits, refs_cache) = service.commit_history_with_refs(
                    &repo,
                    scope,
                    offset,
                    HISTORY_PAGE_SIZE + 1,
                    refs_cache.as_ref(),
                )?;
                let has_more = commits.len() > HISTORY_PAGE_SIZE;
                commits.truncate(HISTORY_PAGE_SIZE);
                perf_log(
                    "history.commits",
                    started,
                    format!(
                        "tab={} scope={} append={} offset={} commits={} has_more={}",
                        tab_id.0,
                        scope.label(),
                        append,
                        offset,
                        commits.len(),
                        has_more
                    ),
                );
                Ok(UiEvent::HistoryCommitsLoaded {
                    tab_id,
                    commits,
                    refs_cache,
                    append,
                    has_more,
                    scope,
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
        self.history_loading.files = true;
        self.history_loading.diff = false;
        self.status = "正在加载提交文件".to_string();

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
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
            self.history_diff = Some(diff);
            self.history_diff_headers_expanded = false;
            self.status = "提交差异已加载".to_string();
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

    fn commit_and_push(&mut self) {
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
            self.status = "差异已加载".to_string();
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

    fn mode_button(
        &self,
        label: &'static str,
        mode: MainMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.mode_button_with_icon(label, mode, None, cx)
    }

    fn mode_button_with_icon(
        &self,
        label: &'static str,
        mode: MainMode,
        icon: Option<ToolbarIcon>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.main_mode == mode;
        let icon_color = if selected {
            ui_theme::PRIMARY
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        segmented_button(format!("mode-{label}"), selected, true)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.set_main_mode(mode);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when_some(icon, |this, icon| {
                        this.child(ui::icons::toolbar_icon(icon, icon_color))
                    })
                    .child(label),
            )
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
        matches!(id, FieldId::CommitMessage | FieldId::ConflictEditor)
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
        let multiline_overflows = multiline_input_should_scroll(id, &field.value);
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
                let (scroll_id, handle_id) = if id == FieldId::ConflictEditor {
                    ("conflict-editor-scroll", CONFLICT_RESULT_SCROLL_HANDLE_ID)
                } else {
                    ("commit-message-input-scroll", "commit-message-input-scroll")
                };
                let handle = self.scroll_handle(handle_id);
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
                scrollable_frame_when(
                    scroll_id,
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    multiline_overflows,
                    cx,
                )
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

    fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let repo_open = self.repo_path.is_some();
        let remote_open = !self.loading.remote() && self.current_remote().is_some();
        let viewport_width = f32::from(window.viewport_size().width);
        let more_placement = toolbar_more_action_placement(toolbar_layout_mode(viewport_width));
        let has_conflicts = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.conflicts.is_empty());
        let behind_count = self
            .branch_sync_status
            .as_ref()
            .map(|s| s.behind)
            .unwrap_or(0);
        let ahead_count = self
            .branch_sync_status
            .as_ref()
            .map(|s| s.ahead)
            .unwrap_or(0);

        // 自定义标题栏沿用 Pencil 主页面结构：品牌、Git 操作、拖动区、模式切换和窗口控制。
        hero_toolbar()
            .flex()
            .items_center()
            .h(px(52.0))
            .pl(px(16.0))
            .child(self.render_titlebar_brand())
            // Git 操作按钮保留现有能力，宽屏时继续展开“更多”中的动作。
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.0))
                    .ml(px(20.0))
                    .child(self.render_toolbar_action_button(
                        "打开",
                        ToolbarIcon::Open,
                        None,
                        !self.busy,
                        |this, _, _| this.browse_open(),
                        cx,
                    ))
                    .child(self.render_toolbar_action_button(
                        "刷新",
                        ToolbarIcon::Refresh,
                        None,
                        repo_open && !self.busy,
                        |this, _, _| this.refresh(),
                        cx,
                    ))
                    .child(self.render_toolbar_action_button(
                        "获取",
                        ToolbarIcon::Fetch,
                        None,
                        repo_open && remote_open && !self.busy,
                        |this, _, _| this.fetch(),
                        cx,
                    ))
                    .child(self.render_toolbar_action_button(
                        "拉取",
                        ToolbarIcon::Pull,
                        if behind_count > 0 {
                            Some(format!("↓{}", behind_count))
                        } else {
                            None
                        },
                        repo_open && remote_open && !self.busy,
                        |this, _, _| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Pull)
                        },
                        cx,
                    ))
                    .child(self.render_toolbar_action_button(
                        "推送",
                        ToolbarIcon::Push,
                        if ahead_count > 0 {
                            Some(format!("↑{}", ahead_count))
                        } else {
                            None
                        },
                        repo_open && remote_open && !self.busy,
                        |this, _, _| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Push)
                        },
                        cx,
                    ))
                    .when(more_placement.show_inline_actions, |this| {
                        this.child(self.render_toolbar_action_button(
                            "克隆",
                            ToolbarIcon::Clone,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::Clone,
                                repo_open,
                                self.busy,
                            ),
                            |this, window, _cx| this.open_clone_dialog(window),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "贮藏",
                            ToolbarIcon::Stash,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::Stash,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_stash_dialog(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "子模块",
                            ToolbarIcon::Submodule,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::Submodule,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_submodule_manager(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "凭据管理",
                            ToolbarIcon::Credentials,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::Credentials,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_credential_manager(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "代理设置",
                            ToolbarIcon::Proxy,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::Proxy,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_network_proxy_settings(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "AI 设置",
                            ToolbarIcon::Ai,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::AiSettings,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_ai_provider_settings(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "合并工具",
                            ToolbarIcon::Workflow,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::ExternalMergeSettings,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_external_merge_settings(),
                            cx,
                        ))
                        .child(self.render_toolbar_action_button(
                            "更新设置",
                            ToolbarIcon::Update,
                            None,
                            toolbar_more_action_enabled(
                                ToolbarMoreAction::UpdateSettings,
                                repo_open,
                                self.busy,
                            ),
                            |this, _, _| this.open_update_settings(),
                            cx,
                        ))
                    })
                    .when(more_placement.show_more_button, |this| {
                        this.child(self.render_toolbar_more_button(cx))
                    }),
            )
            .child(self.render_titlebar_drag_area())
            // 右侧模式切换与 Pencil 稿一致使用图标和药丸选中态。
            .child(
                div()
                    .flex()
                    .flex_none()
                    // GPUI 需要显式宽度才能在主标题栏中为右侧固定区域预留空间。
                    .w(px(if has_conflicts { 384.0 } else { 288.0 }))
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        mode_pill(
                            "mode-worktree".into(),
                            "工作区",
                            Some(ToolbarIcon::Worktree),
                            self.main_mode == MainMode::Worktree,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                this.set_main_mode(MainMode::Worktree);
                                cx.notify();
                            },
                        )),
                    )
                    .when(
                        has_conflicts,
                        |this| {
                            this.child(
                                mode_pill(
                                    "mode-conflict".into(),
                                    "冲突处理",
                                    None,
                                    self.main_mode == MainMode::Conflict,
                                )
                                .on_click(cx.listener(
                                    |this, _event, _window, cx| {
                                        this.set_main_mode(MainMode::Conflict);
                                        cx.notify();
                                    },
                                )),
                            )
                        },
                    )
                    .child(
                        mode_pill(
                            "mode-history".into(),
                            "提交记录",
                            Some(ToolbarIcon::History),
                            self.main_mode == MainMode::History,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                this.set_main_mode(MainMode::History);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        mode_pill(
                            "mode-workflow".into(),
                            "工作流",
                            Some(ToolbarIcon::Workflow),
                            self.main_mode == MainMode::Workflow,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                this.set_main_mode(MainMode::Workflow);
                                cx.notify();
                            },
                        )),
                    )
                    .mr(px(8.0)),
            )
            .child(self.render_window_controls(window))
    }

    /// 品牌区交给 Windows 原生命中测试处理拖动和双击最大化。
    fn render_titlebar_brand(&self) -> impl IntoElement {
        div()
            .id("titlebar-brand")
            .flex()
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(8.0))
            .cursor(CursorStyle::Arrow)
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .size(px(28.0))
                    .items_center()
                    .justify_center()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .bg(rgb(ui_theme::PRIMARY))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::PRIMARY_FOREGROUND))
                    .child("K"),
            )
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("Khaslana"),
            )
    }

    /// 中间弹性空白区使用原生标题栏命中区域，支持拖动、双击和系统菜单。
    fn render_titlebar_drag_area(&self) -> impl IntoElement {
        div()
            .id("titlebar-drag-area")
            .flex_1()
            .min_w(px(24.0))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
    }

    /// Windows 窗口控制按钮通过原生命中测试获得最小化、还原和关闭行为。
    fn render_window_controls(&self, window: &Window) -> impl IntoElement {
        let maximize_icon = if window.is_maximized() {
            ToolbarIcon::Restore
        } else {
            ToolbarIcon::Maximize
        };
        div()
            .flex()
            .flex_none()
            .w(px(132.0))
            .h_full()
            .child(self.render_window_control_button(
                "window-minimize",
                ToolbarIcon::Minus,
                false,
                WindowControlArea::Min,
            ))
            .child(self.render_window_control_button(
                "window-maximize",
                maximize_icon,
                false,
                WindowControlArea::Max,
            ))
            .child(self.render_window_control_button(
                "window-close",
                ToolbarIcon::Close,
                true,
                WindowControlArea::Close,
            ))
    }

    fn render_window_control_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        danger: bool,
        area: WindowControlArea,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .w(px(44.0))
            .h_full()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .window_control_area(area)
            .hover(move |this| {
                if danger {
                    this.bg(rgb(ui_theme::COLOR_ERROR))
                } else {
                    this.bg(rgb(ui_theme::ACCENT))
                }
            })
            .child(toolbar_icon_with_size(icon, ui_theme::FOREGROUND, 12.0, 16.0))
    }

    /// 工具栏操作按钮：图标 + 中文标签 + 可选差异数角标
    fn render_toolbar_action_button(
        &self,
        label: &'static str,
        icon_kind: ToolbarIcon,
        sync_label: Option<String>,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text_color = if enabled {
            ui_theme::FOREGROUND
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        div()
            .id(label)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.0))
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed())
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.5))
            .text_color(rgb(text_color))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(icon_kind, text_color))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(label),
            )
            .when_some(sync_label, |this, label| {
                this.child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(ui_theme::PRIMARY))
                        .child(label),
                )
            })
    }

    fn render_toolbar_path_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo_path = self
            .repo_path
            .as_ref()
            .map(|path| path.display().to_string());
        let repo_open = repo_path.is_some();
        let label = repo_path.unwrap_or_else(|| "未打开仓库".into());

        div().flex_1().min_w(px(0.0)).flex().justify_center().child(
            div()
                .id("toolbar-repo-path-pill")
                .min_w(px(0.0))
                .max_w(px(520.0))
                .px_3()
                .py_1()
                .rounded_full()
                .border_1()
                .border_color(rgb(ui_theme::BORDER))
                .bg(rgb(ui_theme::CARD))
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(ui_theme::FOREGROUND))
                .truncate()
                .when(repo_open, |this| {
                    this.cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                        .tooltip(|_window, cx| tooltip_text("点击打开仓库目录", cx))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.open_repo_in_file_manager(cx);
                            cx.stop_propagation();
                        }))
                })
                .child(label),
        )
    }

    fn render_toolbar_more_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = !self.busy;
        let text_color = if enabled {
            ui_theme::FOREGROUND
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        div()
            .id("更多")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.0))
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed())
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.5))
            .text_color(rgb(text_color))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !enabled {
                    return;
                }
                if this.toolbar_more_menu.is_some() {
                    this.toolbar_more_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                let position = event.position();
                let viewport_size = window.viewport_size();
                let (x, y) = toolbar_more_menu_position(
                    position.x.into(),
                    position.y.into(),
                    f32::from(viewport_size.width),
                    f32::from(viewport_size.height),
                );
                let button_x =
                    (f32::from(position.x) - TOOLBAR_MORE_BUTTON_ANCHOR_WIDTH / 2.0).max(0.0);
                let button_y = (f32::from(position.y) - 22.0).max(0.0);
                this.toolbar_more_menu = Some(ToolbarMoreMenu {
                    x,
                    y,
                    button_x,
                    button_y,
                });
                cx.stop_propagation();
                cx.notify();
            }))
            .child(toolbar_icon(ToolbarIcon::More, text_color))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("更多"),
            )
    }

    fn render_toolbar_more_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.toolbar_more_menu.as_ref() else {
            return div().into_any_element();
        };
        let repo_open = self.repo_path.is_some();
        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(TOOLBAR_MORE_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(self.toolbar_more_menu_item(
                "克隆",
                ToolbarIcon::Clone,
                toolbar_more_action_enabled(ToolbarMoreAction::Clone, repo_open, self.busy),
                |this, window, _cx| this.open_clone_dialog(window),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "贮藏",
                ToolbarIcon::Stash,
                toolbar_more_action_enabled(ToolbarMoreAction::Stash, repo_open, self.busy),
                |this, _, _| this.open_stash_dialog(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "子模块",
                ToolbarIcon::Submodule,
                toolbar_more_action_enabled(ToolbarMoreAction::Submodule, repo_open, self.busy),
                |this, _, _| this.open_submodule_manager(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "凭据管理",
                ToolbarIcon::Credentials,
                toolbar_more_action_enabled(ToolbarMoreAction::Credentials, repo_open, self.busy),
                |this, _, _| this.open_credential_manager(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "代理设置",
                ToolbarIcon::Proxy,
                toolbar_more_action_enabled(ToolbarMoreAction::Proxy, repo_open, self.busy),
                |this, _, _| this.open_network_proxy_settings(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "AI 设置",
                ToolbarIcon::Ai,
                toolbar_more_action_enabled(ToolbarMoreAction::AiSettings, repo_open, self.busy),
                |this, _, _| this.open_ai_provider_settings(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "合并工具",
                ToolbarIcon::Workflow,
                toolbar_more_action_enabled(
                    ToolbarMoreAction::ExternalMergeSettings,
                    repo_open,
                    self.busy,
                ),
                |this, _, _| this.open_external_merge_settings(),
                cx,
            ))
            .child(self.toolbar_more_menu_item(
                "更新设置",
                ToolbarIcon::Update,
                toolbar_more_action_enabled(ToolbarMoreAction::UpdateSettings, repo_open, self.busy),
                |this, _, _| this.open_update_settings(),
                cx,
            ))
            .into_any_element()
    }

    fn toolbar_more_menu_item(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(format!("toolbar-more-{label}"))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .text_size(px(12.0))
            .text_color(rgb(if enabled {
                ui_theme::FOREGROUND
            } else {
                ui_theme::MUTED_FOREGROUND
            }))
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.toolbar_more_menu = None;
                        on_click(this, window, cx);
                        cx.notify();
                    }))
            })
            .child(ui::icons::toolbar_icon(
                icon,
                if enabled {
                    ui_theme::MUTED_FOREGROUND
                } else {
                    ui_theme::MUTED_FOREGROUND
                },
            ))
            .child(label)
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.scroll_handle("repo-tab-bar-scroll");
        let content = div()
            .id("repo-tab-bar-scroll")
            .flex()
            .items_center()
            .gap_1()
            .min_w(px(0.0))
            .w_full()
            .h(px(36.0))
            .px_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .overflow_x_scroll()
            .track_scroll(&handle)
            .children(
                self.tabs
                    .iter()
                    .map(|tab| self.render_repo_tab(tab, cx).into_any_element())
                    .collect::<Vec<_>>(),
            )
            .child(
                div()
                    .id("repo-tab-add")
                    .flex()
                    .flex_none()
                    .size(px(28.0))
                    .items_center()
                    .justify_center()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.browse_open();
                        cx.stop_propagation();
                    }))
                    .child(toolbar_icon_with_size(
                        ToolbarIcon::Plus,
                        ui_theme::MUTED_FOREGROUND,
                        14.0,
                        14.0,
                    )),
            )
            .into_any_element();

        scrollable_frame_intrinsic(
            "repo-tab-bar-scroll",
            ScrollbarMode::Horizontal,
            content,
            handle,
            cx,
        )
        .into_any_element()
    }

    fn render_repo_tab(&self, tab: &RepoTabState, cx: &mut Context<Self>) -> impl IntoElement {
        let id = tab.id;
        let selected = self.active_tab == Some(id);
        let title = tab.display_name();
        let status_dot = if tab.busy || tab.loading != RepositoryLoading::default() {
            "..."
        } else {
            ""
        };

        div()
            .id(format!("repo-tab-{}", id.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .max_w(px(220.0))
            .px(px(14.0))
            .py(px(6.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else {
                rgb(ui_theme::CARD)
            })
            .cursor_pointer()
            .hover(|this| {
                if selected {
                    this.bg(rgb(ui_theme::CARD))
                } else {
                    this.bg(rgb(ui_theme::SECONDARY))
                }
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.activate_tab(id);
                cx.notify();
            }))
            .child(toolbar_icon_with_size(
                ToolbarIcon::Open,
                if selected {
                    ui_theme::PRIMARY
                } else {
                    ui_theme::MUTED_FOREGROUND
                },
                12.0,
                12.0,
            ))
            .child(
                div()
                    .min_w(px(0.0))
                    .max_w(px(150.0))
                    .text_size(px(12.0))
                    .text_color(if selected {
                        rgb(ui_theme::FOREGROUND)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .truncate()
                    .child(format!("{title}{status_dot}")),
            )
            .child(
                div()
                    .id(format!("repo-tab-close-{}", id.0))
                    .flex()
                    .flex_none()
                    .size(px(14.0))
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::COLOR_ERROR)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.close_tab(id);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(toolbar_icon_with_size(
                        ToolbarIcon::Close,
                        ui_theme::MUTED_FOREGROUND,
                        10.0,
                        12.0,
                    )),
            )
    }

    fn render_tag_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.tag_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(TAG_MENU_WIDTH))
            .child(context_menu_item(
                "检出标签",
                !self.busy,
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

    fn render_change_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.change_context_menu.clone() else {
            return div().into_any_element();
        };
        let selected_count = self.change_selection.selected(&menu.scope).len();
        let selected_paths = self.selected_change_paths(menu.scope.clone());
        let all_paths = self.change_paths(menu.scope.clone());
        let all_count = all_paths.len();

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
                    !self.busy,
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
                    selected_count > 0 && !self.busy,
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
                    all_count > 0 && !self.busy,
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
                    !self.busy,
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
                    selected_count > 0 && !self.busy,
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
                    all_count > 0 && !self.busy,
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
                let can_uncommit = !self.busy && menu.is_head;
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
                !self.busy,
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
                !self.busy,
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
                !self.busy,
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
                !self.busy,
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

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (staged_rows, unstaged_rows) = if let Some(snapshot) = self.snapshot.as_ref() {
            let staged_rows = self
                .change_indexes
                .staged
                .iter()
                .filter_map(|index| snapshot.changes.get(*index))
                .cloned()
                .map(|change| {
                    self.change_row(change, DiffScope::Staged, cx)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            let unstaged_rows = self
                .change_indexes
                .unstaged
                .iter()
                .filter_map(|index| snapshot.changes.get(*index))
                .cloned()
                .map(|change| {
                    self.change_row(change, DiffScope::Unstaged, cx)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            (staged_rows, unstaged_rows)
        } else {
            (Vec::new(), Vec::new())
        };
        let has_staged_selection = !self.change_selection.staged.is_empty();
        let has_unstaged_selection = !self.change_selection.unstaged.is_empty();
        let has_staged = !staged_rows.is_empty();
        let has_unstaged = !unstaged_rows.is_empty();
        let staged_count = staged_rows.len();
        let unstaged_count = unstaged_rows.len();

        app_panel()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.changes_width))
            .min_w(px(self.changes_width))
            .h_full()
            .border_r_1()
            .border_color(rgb(ui_theme::BORDER))
            .when_some(self.render_rebase_banner(cx), |this, banner| {
                this.child(banner)
            })
            .child(self.render_conflict_section(cx))
            .child(self.render_change_section(
                "已暂存变更",
                "staged-change-list",
                "暂存区加载中...",
                self.loading.staged(),
                staged_rows,
                has_staged || self.loading.staged(),
                staged_count,
                true,
                vec![
                        // 设计图：minus 图标按钮 22×22，取消暂存全部
                        self.change_icon_button(
                            "取消暂存全部",
                            ToolbarIcon::Minus,
                            has_staged && !self.busy,
                            |this, _, _| this.unstage_all(),
                            cx,
                        )
                        .into_any_element(),
                    ],
                cx,
            ))
            .child(self.render_change_section(
                "未暂存变更",
                "unstaged-change-list",
                "修改区加载中...",
                self.loading.unstaged(),
                unstaged_rows,
                has_unstaged || self.loading.unstaged(),
                unstaged_count,
                false,
                vec![
                        // 设计图：plus 图标按钮 22×22，暂存全部
                        self.change_icon_button(
                            "暂存全部",
                            ToolbarIcon::Plus,
                            has_unstaged && !self.busy,
                            |this, _, _| this.stage_all(),
                            cx,
                        )
                        .into_any_element(),
                        // 设计图：trash 图标按钮 22×22，丢弃全部（DESTRUCTIVE 色）
                        self.change_destructive_icon_button(
                            "丢弃全部",
                            has_unstaged && !self.busy,
                            |this, _, _| this.confirm_discard_all(),
                            cx,
                        )
                        .into_any_element(),
                    ],
                cx,
            ))
    }

    fn render_change_section(
        &self,
        title: &'static str,
        id: &'static str,
        loading_text: &'static str,
        loading: bool,
        rows: Vec<gpui::AnyElement>,
        content_present: bool,
        count: usize,
        is_staged: bool,
        actions: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = if rows.is_empty() && loading {
            vec![placeholder_row(loading_text).into_any_element()]
        } else {
            rows
        };

        // 设计图：
        // 未暂存变更标题 + 计数 badge（SECONDARY bg / SECONDARY_FOREGROUND fg）
        // 已暂存变更标题 + 计数 badge（PRIMARY bg / PRIMARY_FOREGROUND fg）
        let badge_bg = if is_staged {
            ui_theme::PRIMARY
        } else {
            ui_theme::SECONDARY
        };
        let badge_fg = if is_staged {
            ui_theme::PRIMARY_FOREGROUND
        } else {
            ui_theme::SECONDARY_FOREGROUND
        };
        let count_badge = if count > 0 {
            Some(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(ui_theme::RADIUS_PILL))
                    .bg(rgb(badge_bg))
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(badge_fg))
                    .child(count.to_string())
                    .into_any_element(),
            )
        } else {
            None
        };

        // 设计图：已暂存标题上下都有 border（分割线），未暂存标题只有下 border
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .when(is_staged, |this| {
                this.border_t_1().border_color(rgb(ui_theme::BORDER))
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(title),
                            )
                            .when_some(count_badge, |this, badge| this.child(badge)),
                    )
                    .child(div().flex().items_center().gap(px(4.0)).children(actions)),
            )
            .child({
                let handle = self.scroll_handle(id);
                let content = div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(2.0))
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    id,
                    ScrollbarMode::Both,
                    content,
                    handle,
                    content_present,
                    cx,
                )
            })
    }

    pub(crate) fn render_column_splitter(
        &self,
        target: ResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let active = self.resize_state(target).is_some();
        let horizontal = target == ResizeTarget::HistoryTop;

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
            .cursor(if horizontal {
                CursorStyle::ResizeRow
            } else {
                CursorStyle::ResizeColumn
            })
            .bg(if active {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::CARD)
            })
            .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
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
                                let (resizing, active_dialog) = {
                                    let view = entity.read(cx);
                                    (
                                        view.resize_state(target).is_some(),
                                        view.active_dialog.is_some(),
                                    )
                                };
                                if column_splitter_should_clear_resize(active_dialog, resizing) {
                                    entity.update(cx, |this, cx| {
                                        this.finish_resize_column(target);
                                        cx.notify();
                                    });
                                    return;
                                }
                                if !resizing
                                    || !event.dragging()
                                    || !column_splitter_accepts_mouse_events(active_dialog)
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
                            let (resizing, active_dialog) = {
                                let view = entity.read(cx);
                                (
                                    view.resize_state(target).is_some(),
                                    view.active_dialog.is_some(),
                                )
                            };
                            if !resizing {
                                return;
                            }
                            if !column_splitter_accepts_mouse_events(active_dialog)
                                && !column_splitter_should_clear_resize(active_dialog, resizing)
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

    fn change_row(
        &self,
        change: khaslana::WorktreeChange,
        scope: DiffScope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let path = change.path.clone();
        let selected = self.is_change_selected(&scope, &change.path);
        let state = match scope {
            DiffScope::Staged => change.staged.as_ref(),
            DiffScope::Unstaged => change.unstaged.as_ref(),
        };
        let state_label = state.map(|state| state.label()).unwrap_or(" ");
        // 设计图：状态 badge 背景色
        // M → PRIMARY, A → GIT_ADDED, ? → GIT_UNTRACKED, D → DESTRUCTIVE
        let badge_bg = state
            .map(change_state_badge_bg)
            .unwrap_or(ui_theme::MUTED_FOREGROUND);
        let is_staged = scope == DiffScope::Staged;

        // 行内图标按钮：plus（暂存）或 minus（取消暂存）
        let row_action_icon = if is_staged {
            ToolbarIcon::Minus
        } else {
            ToolbarIcon::Plus
        };
        let row_action_label = if is_staged {
            "取消暂存此文件"
        } else {
            "暂存此文件"
        };
        let row_action_enabled = !self.busy;
        let row_action_click: std::sync::Arc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
            if is_staged {
                std::sync::Arc::new({
                    let path = path.clone();
                    move |this: &mut Self, _window: &mut Window, _cx: &mut Context<Self>| {
                        this.unstage_paths(vec![path.clone()], "取消暂存");
                    }
                })
            } else {
                std::sync::Arc::new({
                    let path = path.clone();
                    move |this: &mut Self, _window: &mut Window, _cx: &mut Context<Self>| {
                        this.stage_paths(vec![path.clone()], "暂存");
                    }
                })
            };

        // 设计图：已暂存行 bg ACCENT，未暂存行无背景
        list_row_surface(
            format!("change-{}-{}", diff_scope_id(&scope), change.path),
            selected,
        )
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(8.0))
        .overflow_hidden()
        .cursor_pointer()
        .when(is_staged && !selected, |this| {
            this.bg(rgb(ui_theme::ACCENT))
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let path = path.clone();
                let scope = scope.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.select_change_from_mouse(path.clone(), scope.clone(), event);
                    this.change_context_menu = None;
                    cx.notify();
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.open_change_context_menu(path.clone(), scope.clone(), event, _window);
                cx.notify();
            }),
        )
        // 设计图：状态字母 badge — 圆角 pill，白字，状态色背景
        .child(
            div()
                .flex_none()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(ui_theme::RADIUS_XS))
                .bg(rgb(badge_bg))
                .text_size(px(9.0))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(ui_theme::PRIMARY_FOREGROUND))
                .justify_center()
                .child(state_label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::FOREGROUND))
                .truncate()
                .child(change.path),
        )
        // 设计图：行内图标按钮 20×20
        .child(self.change_row_icon_button(
            row_action_label,
            row_action_icon,
            ui_theme::MUTED_FOREGROUND,
            row_action_enabled,
            {
                let click = row_action_click;
                move |this, _window, cx| {
                    click(this, _window, cx);
                    cx.notify();
                }
            },
            cx,
        ))
    }

    fn render_diff_and_commit(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(self.render_diff(cx))
            .child(self.render_commit_box(window, cx))
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 全文视图模式下标题前缀"全文："，提示当前展示整份文件而非仅改动区域
        let prefix = if self.full_file_view { "全文：" } else { "" };
        let title = self
            .diff
            .as_ref()
            .map(|diff| {
                format!(
                    "{prefix}差异：{} ({})",
                    diff.path,
                    diff_scope_label(&diff.scope)
                )
            })
            .unwrap_or_else(|| "差异".to_string());

        div()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            .min_h(px(260.0))
            .child(self.diff_section_header(title, EncodingMenuTarget::Worktree, cx))
            .child(self.render_virtual_diff(
                "diff-scroll",
                self.diff.clone(),
                self.diff_headers_expanded,
                DiffHeaderTarget::Worktree,
                "请选择一个变更文件查看差异".to_string(),
                cx,
            ))
            .child(self.render_encoding_dropdown(EncodingMenuTarget::Worktree, cx))
    }

    pub(crate) fn render_virtual_diff(
        &self,
        scroll_id: &'static str,
        diff: Option<Arc<FileDiff>>,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = diff_render_model_for(diff.as_deref(), headers_expanded);
        let row_count = model.row_count;
        let content_present = diff.is_some() && row_count > 0;
        // 以内容最宽的文本行作为列表水平宽度的测量基准，保证长行也能左右滚动。
        let width_measure_index =
            widest_diff_row_index(diff.as_deref(), &model).or_else(|| row_count.checked_sub(1));
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
                    return diff_line(DiffLineKind::Context, None, None, String::new())
                        .into_any_element();
                };
                if line.kind == DiffLineKind::Header {
                    diff_line(line.kind.clone(), None, None, line.content.clone())
                        .into_any_element()
                } else {
                    diff_line(
                        line.kind.clone(),
                        line.old_lineno,
                        line.new_lineno,
                        line.content.clone(),
                    )
                    .into_any_element()
                }
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
                diff_line(DiffLineKind::Context, None, None, message.to_string()).into_any_element()
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
                    .child(self.full_file_toggle_button(target, cx))
                    .child(self.encoding_button(diff, target, cx)),
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

    fn render_commit_box(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_commit = self.repo_path.is_some() && !self.busy;
        let can_commit_and_push = can_commit && self.current_remote().is_some();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(self.input(FieldId::CommitMessage, false, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(self.render_ai_commit_button(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(self.primary_button(
                                "提交",
                                can_commit,
                                |this, _, _| this.commit(),
                                cx,
                            ))
                            .child(self.primary_button(
                                "提交并推送",
                                can_commit_and_push,
                                |this, _, _| this.commit_and_push(),
                                cx,
                            )),
                    ),
            )
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
            .items_center()
            .gap(px(8.0))
            .h(px(24.0))
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
                        .text_color(rgb(ui_theme::COLOR_ERROR_FOREGROUND))
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
                    .child(format!("v{}", env!("CARGO_PKG_VERSION")))
            )
    }

    fn has_active_loading(&self) -> bool {
        let tab = self.active_tab_state();
        tab.operation_kind.shows_progress()
            && (tab.busy || tab.loading != RepositoryLoading::default())
    }

    fn render_feedback_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut left_stack = feedback_stack(false);
        let mut right_stack = feedback_stack(true);

        for feedback in self
            .feedbacks
            .iter()
            .filter(|feedback| !feedback.kind.is_important())
        {
            left_stack = left_stack.child(feedback_bubble(feedback, cx));
        }

        for feedback in self
            .feedbacks
            .iter()
            .filter(|feedback| feedback.kind.is_important())
        {
            right_stack = right_stack.child(feedback_bubble(feedback, cx));
        }

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .child(left_stack)
            .child(right_stack)
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
            DialogState::ConfirmDiscardChange {
                scope,
                target,
                paths,
            } => self
                .render_confirm_discard_change_dialog(scope, target, paths, cx)
                .into_any_element(),
            DialogState::CredentialManager => {
                self.render_credential_manager_dialog(cx).into_any_element()
            }
            DialogState::CredentialDetails { record_id } => self
                .render_credential_details_dialog(record_id, cx)
                .into_any_element(),
            DialogState::CredentialForm { editing } => self
                .render_credential_form_dialog(editing, window, cx)
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
            DialogState::NetworkProxySettings => self
                .render_network_proxy_settings_dialog(window, cx)
                .into_any_element(),
            DialogState::AiProviderSettings => self
                .render_ai_provider_settings_dialog(window, cx)
                .into_any_element(),
            DialogState::ExternalMergeSettings => self
                .render_external_merge_settings_dialog(window, cx)
                .into_any_element(),
            DialogState::StashForm => self.render_stash_form_dialog(window, cx).into_any_element(),
            DialogState::ConfirmDropStash { index, message } => self
                .render_confirm_drop_stash_dialog(index, message, cx)
                .into_any_element(),
            DialogState::RemoteBranchOperation { kind } => self
                .render_remote_branch_operation_dialog(kind, window, cx)
                .into_any_element(),
            DialogState::ConfirmConflictResolve => self
                .render_confirm_conflict_resolve_dialog(cx)
                .into_any_element(),
            // ── 更新对话框 ──
            DialogState::UpdateSettings => self
                .render_update_settings_dialog(cx)
                .into_any_element(),
            DialogState::NewVersionAvailable { version, notes, published_at, size } => self
                .render_new_version_dialog(&version, &notes, &published_at, size, cx)
                .into_any_element(),
            DialogState::ConfirmInstallUpdate { version } => self
                .render_confirm_install_dialog(&version, cx)
                .into_any_element(),
            DialogState::UpdateNoWritePermission { version } => self
                .render_no_write_permission_dialog(&version, cx)
                .into_any_element(),
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
        let skipped = self.update_preferences.skipped_version.clone();

        self.dialog_panel("更新设置", cx)
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
                        div()
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(skipped.map(|v| format!("v{v}")).unwrap_or_else(|| "无".to_string())),
                    ),
            )
            .child(dialog_actions()
                .child(self.primary_button(
                    "立即检查",
                    !self.update_checking && !self.busy,
                    |this, _, _| this.start_update_check(),
                    cx,
                ))
                .child(self.button(
                    "清除跳过",
                    self.update_preferences.skipped_version.is_some(),
                    |this, _, _| this.clear_skipped_version(),
                    cx,
                ))
                .child(self.button("关闭", true, |this, _, _| this.close_dialog(), cx))
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
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .max_h(px(120.0))
                    .overflow_hidden()
                    .child(notes.to_string()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("包大小：{:.1} MB", size_mb)),
            )
            .child(dialog_actions()
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
                .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx))
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
                    .child(format!("版本 v{version} 已下载并校验通过，应用将重启以完成安装。")),
            )
            .child(danger_callout("安装过程中应用会自动退出并重启，请确保没有未保存的工作。"))
            .child(dialog_actions()
                .child(self.primary_button(
                    "立即重启",
                    true,
                    move |this, _, _| {
                        if let Some(dir) = staging_dir.clone() {
                            this.install_update(&dir, &version_owned);
                        } else {
                            this.update_error = Some("staging 目录丢失".into());
                        }
                    },
                    cx,
                ))
                .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx))
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
                    .child("当前目录没有写入权限，无法自动安装新版本。请手动下载新版本："),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button("打开 CNB 下载页", true, |_, _, _| {
                        open_url("https://cnb.cool/suhoan/khaslana-release");
                    }, cx))
                    .child(self.button("打开 GitHub Release", true, |_, _, _| {
                        open_url("https://github.com/FuturePrayer/khaslana/releases");
                    }, cx)),
            )
            .child(dialog_actions()
                .child(self.button("关闭", true, |this, _, _| this.close_dialog(), cx))
            )
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
            .id("dialog-凭据管理")
            .w(px(860.0))
            .max_h(px(620.0))
            .min_w(px(0.0))
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.close_credential_context_menu(cx);
                    cx.stop_propagation();
                }),
            )
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
                            .child("凭据管理"),
                    )
                    .child(
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
            .child(div().flex().justify_end().child(self.button(
                "关闭",
                !self.busy,
                |this, _, _| this.close_dialog(),
                cx,
            )))
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
                    |this, _, _| this.active_dialog = Some(DialogState::CredentialManager),
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
                |this, _, _| this.active_dialog = Some(DialogState::CredentialManager),
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
                            this.test_credential_record(record_id.clone());
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
                            this.active_dialog = Some(DialogState::CredentialManager);
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
        if toolbar_layout_mode(f32::from(window.viewport_size().width)) == ToolbarLayoutMode::Full
            && self.toolbar_more_menu.is_some()
        {
            self.toolbar_more_menu = None;
        }

        app_shell_surface()
            .relative()
            .flex()
            .flex_col()
            .text_color(rgb(ui_theme::FOREGROUND))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                this.encoding_menu_closed_by_capture = None;
                if this.mouse_down_inside_context_menu(event) {
                    return;
                }
                if this.branch_context_menu.is_some()
                    || this.remote_context_menu.is_some()
                    || this.change_context_menu.is_some()
                    || this.credential_context_menu.is_some()
                    || this.tag_context_menu.is_some()
                    || this.stash_context_menu.is_some()
                    || this.commit_context_menu.is_some()
                    || this.encoding_menu_target.is_some()
                    || this.toolbar_more_menu.is_some()
                {
                    let closed_encoding_menu = this.encoding_menu_target;
                    this.branch_context_menu = None;
                    this.remote_context_menu = None;
                    this.change_context_menu = None;
                    this.credential_context_menu = None;
                    this.tag_context_menu = None;
                    this.stash_context_menu = None;
                    this.commit_context_menu = None;
                    this.encoding_menu_target = None;
                    this.encoding_menu_closed_by_capture = closed_encoding_menu;
                    this.toolbar_more_menu = None;
                    cx.notify();
                }
            }))
            .child(self.render_toolbar(window, cx))
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(self.render_sidebar(window, cx))
                    .child(self.render_column_splitter(ResizeTarget::Sidebar, cx))
                    .child(match self.main_mode {
                        MainMode::Worktree => div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(self.render_changes(cx))
                            .child(self.render_column_splitter(ResizeTarget::Changes, cx))
                            .child(self.render_diff_and_commit(window, cx))
                            .into_any_element(),
                        MainMode::Conflict => self
                            .render_conflict_workbench(window, cx)
                            .into_any_element(),
                        MainMode::History => self.render_history_view(cx).into_any_element(),
                        MainMode::Workflow => {
                            self.render_workflow_view(window, cx).into_any_element()
                        }
                        MainMode::Stash => self.render_stash_preview_view(cx).into_any_element(),
                        MainMode::Browse => self.render_browse_view(cx).into_any_element(),
                    }),
            )
            .child(self.render_status())
            .child(self.render_branch_context_menu(cx))
            .child(self.render_remote_context_menu(cx))
            .child(self.render_change_context_menu(cx))
            .child(self.render_commit_context_menu(cx))
            .child(self.render_tag_context_menu(cx))
            .child(self.render_stash_context_menu(cx))
            .child(self.render_toolbar_more_menu(cx))
            .child(self.render_dialogs(window, cx))
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

fn normalize_repo_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
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

fn update_check_toast_message(_error: &str) -> Option<String> {
    None
}

fn dedupe_repo_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(normalize_repo_path(path)))
        .collect()
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod app_tests;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    Application::new()
        .with_assets(assets::AppAssets::new())
        .run(|cx: &mut App| {
            init_yororen_components(cx);
            cx.set_global(GlobalTheme::new(cx.window_appearance()));
            cx.set_global(I18n::with_embedded(
                Locale::new("zh-CN").expect("zh-CN locale is valid"),
            ));
            let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
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
                KeyBinding::new("cmd-c", BrowseContentCopy, Some("BrowseContent")),
                KeyBinding::new("cmd-a", BrowseContentSelectAll, Some("BrowseContent")),
                KeyBinding::new("ctrl-c", BrowseContentCopy, Some("BrowseContent")),
                KeyBinding::new("ctrl-a", BrowseContentSelectAll, Some("BrowseContent")),
            ]);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
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
                    window.focus(&view.read(cx).focus_handle(cx));
                    view
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
