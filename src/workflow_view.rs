use std::collections::BTreeMap;
use std::fs;
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::SystemTime;

use crate::ui::theme::rgb;
use chrono::{DateTime, Local};
use directories::BaseDirs;
use git2::Repository;
use gpui::{
    ClickEvent, Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px, uniform_list,
};
use gpui_kit::base::Button as BaseButton;
use khaslana::{
    BranchKind, WorkflowDefinition, WorkflowExecutor, WorkflowInputDefinition, WorkflowPreview,
    WorkflowProgressEvent, WorkflowRunControl, WorkflowRunOptions, WorkflowSourceBranch,
    GitError, parse_workflow_json5,
};
use khaslana::workflow::remote_templates::{
    MANAGED_TEMPLATE_DIR, RemoteTemplateEntry, download_cnb_template, list_cnb_templates,
};
use khaslana::workflow::extensions::{
    WorkflowExternalGrant, load_mcp_config, register_external_actions_with_ai,
};
use khaslana::workflow::browser_runtime;

use crate::{
    DialogState, FieldId, MainMode, OperationBlocker, RepositoryLoading, RepositorySnapshot,
    RepositoryView, ResizeTarget, ScrollbarMode, TextFieldState, UiEvent,
    WorkflowActiveRun, WorkflowRunIdentity, WorkflowPendingExternal,
    WORKFLOW_TEMPLATE_MENU_HEIGHT, WORKFLOW_TEMPLATE_MENU_WIDTH, WorkflowTemplateContextMenu,
    clamped_menu_position, dialog_actions, scrollable_frame_when, scrollable_uniform_frame,
    send_ui_event,
    system::open_directory,
    tasks::{TaskKind, panic_message},
    ui::{
        components::{
            command_group, dialog_panel_size, floating_panel, list_row_surface, page_header,
            panel_section_header,
        },
        theme as ui_theme,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct WorkflowInputFieldState {
    key: String,
    label: String,
    description: Option<String>,
    required: bool,
    /// 持业务真值的文本框（Kit 输入宿主经 `try_field` 读写这里）。
    pub(crate) field: TextFieldState,
}

/// 一条工作流日志：标题行 + 可选的明细行（如逐个删除/命中的分支）。
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkflowLogEntry {
    pub(crate) message: String,
    pub(crate) details: Vec<String>,
}

impl From<String> for WorkflowLogEntry {
    fn from(message: String) -> Self {
        Self {
            message,
            details: Vec::new(),
        }
    }
}

impl From<&str> for WorkflowLogEntry {
    fn from(message: &str) -> Self {
        Self {
            message: message.to_string(),
            details: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorkflowTemplateItem {
    pub(crate) path: PathBuf,
    pub(crate) display_name: String,
    file_name: String,
    modified_label: String,
    pub(crate) error: Option<String>,
}

/// 模板导航的轻量模型：只保存快照下标，不预先创建任何 GPUI 元素。
/// `uniform_list` 回调收到可视 range 后才按下标读取模板并构造行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowTemplateListModel {
    indices: Vec<usize>,
}

impl WorkflowTemplateListModel {
    fn from_templates(templates: &[WorkflowTemplateItem]) -> Self {
        Self {
            indices: (0..templates.len()).collect(),
        }
    }

    fn len(&self) -> usize {
        self.indices.len()
    }

    fn index_at(&self, row: usize) -> Option<usize> {
        self.indices.get(row).copied()
    }
}

const WORKFLOW_TEMPLATE_ROW_HEIGHT: f32 = 56.0;
const WORKFLOW_TEMPLATE_LIST_SCROLL_ID: &str = "workflow-template-list";
const WORKFLOW_INPUT_LIST_SCROLL_ID: &str = "workflow-input-list";
const WORKFLOW_PREVIEW_LIST_SCROLL_ID: &str = "workflow-preview-list";
const WORKFLOW_LOG_LIST_SCROLL_ID: &str = "workflow-log-list";

/// Runbook Studio 的内容分栏策略，保持布局判断与 GPUI 渲染解耦。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStudioLayout {
    NoDefinition,
    PreviewOnly,
    InputsAndPreview,
}

impl WorkflowStudioLayout {
    fn from_state(has_definition: bool, has_inputs: bool) -> Self {
        if !has_definition {
            Self::NoDefinition
        } else if has_inputs {
            Self::InputsAndPreview
        } else {
            Self::PreviewOnly
        }
    }

    fn has_inputs(self) -> bool {
        matches!(self, Self::InputsAndPreview)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowConsoleState {
    Collapsed,
    Expanded,
}

impl WorkflowConsoleState {
    fn is_expanded(self) -> bool {
        matches!(self, Self::Expanded)
    }
}

pub(crate) fn workflow_studio_layout(
    has_definition: bool,
    has_inputs: bool,
    _busy: bool,
    _log_count: usize,
) -> WorkflowStudioLayout {
    // busy/log 只影响底部 Console；主区列策略仅由 definition 与 inputs 决定。
    WorkflowStudioLayout::from_state(has_definition, has_inputs)
}

pub(crate) fn workflow_console_state(busy: bool, log_count: usize) -> WorkflowConsoleState {
    if busy || log_count > 0 {
        WorkflowConsoleState::Expanded
    } else {
        WorkflowConsoleState::Collapsed
    }
}

/// 按绑定文件名取模板显示名（设置页 / 绑定弹窗 / 触发提示共用）；
/// 列表查不到（外部删除或尚未加载）时回退文件主干。
pub(crate) fn workflow_display_name_for_file(
    templates: &[WorkflowTemplateItem],
    file: &str,
) -> String {
    templates
        .iter()
        .find(|template| template.file_name.eq_ignore_ascii_case(file))
        .map(|template| template.display_name.clone())
        .unwrap_or_else(|| {
            Path::new(file)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string())
        })
}

/// 快捷键触发的执行方式决策（纯函数，可单测）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowShortcutTrigger {
    /// 必填变量未填：跳到工作流页提示填写，不运行（前台后台一致）。
    JumpToFillInputs,
    /// 跳到工作流页并立即运行（默认）。
    JumpAndRun,
    /// 留在当前页后台运行（绑定弹窗勾选「后台执行」）。
    RunInBackground,
}

pub(crate) fn workflow_shortcut_trigger_decision(
    missing_required_inputs: bool,
    background: bool,
) -> WorkflowShortcutTrigger {
    if missing_required_inputs {
        WorkflowShortcutTrigger::JumpToFillInputs
    } else if background {
        WorkflowShortcutTrigger::RunInBackground
    } else {
        WorkflowShortcutTrigger::JumpAndRun
    }
}

/// 合并决策：旧输入值非空则保留（模板被修改后重载仍不丢已填变量），
/// 否则维持重载后的新值（默认值）。
pub(crate) fn merged_workflow_input_value(previous: Option<&str>, current_value: &str) -> String {
    match previous {
        Some(value) if !value.trim().is_empty() => value.to_string(),
        _ => current_value.to_string(),
    }
}

/// 重载模板后按 key 合并回填旧输入值：模板在磁盘上被修改时触发仍运行最新
/// 内容，但用户已填的非空变量值不丢（空值不覆盖新默认，被删变量自然丢弃）。
pub(crate) fn merge_workflow_input_values(
    previous: &BTreeMap<String, String>,
    inputs: &mut [WorkflowInputFieldState],
) {
    for input in inputs.iter_mut() {
        let merged = merged_workflow_input_value(
            previous.get(&input.key).map(String::as_str),
            &input.field.value,
        );
        input.field.set_value(merged);
    }
}

/// 模板行的标准点击直接加载；忙碌期间不重复启动加载。
pub(crate) fn workflow_template_click_loads(standard_click: bool, busy: bool) -> bool {
    standard_click && !busy
}

/// 模板仅在当前工作流确实来自该路径时显示选中，外部文件不会误高亮旧模板。
pub(crate) fn workflow_template_selection_matches(
    selected_template_path: Option<&Path>,
    loaded_file_path: Option<&Path>,
    template_path: Option<&Path>,
) -> bool {
    let Some(template_path) = template_path else {
        return false;
    };
    selected_template_path == Some(template_path) && loaded_file_path == Some(template_path)
}

fn workflow_selected_template_path(
    loaded_file_path: Option<&Path>,
    templates: &[WorkflowTemplateItem],
) -> Option<PathBuf> {
    loaded_file_path
        .filter(|path| templates.iter().any(|template| &template.path == *path))
        .map(Path::to_path_buf)
}

fn workflow_empty_line(text: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(ui_theme::SPACE_2))
        .py(px(ui_theme::SPACE_2))
        .text_size(px(12.0))
        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
        .child(text)
}

impl WorkflowInputFieldState {
    fn new(
        key: String,
        input: &WorkflowInputDefinition,
        value: String,
        cx: &mut Context<RepositoryView>,
    ) -> Self {
        let label = input
            .label
            .as_ref()
            .map(|label| label.trim())
            .filter(|label| !label.is_empty())
            .unwrap_or(&key)
            .to_string();
        let mut field = TextFieldState::new(cx, label.clone());
        field.set_value(value);
        Self {
            key,
            label,
            description: input
                .description
                .as_ref()
                .map(|description| description.trim())
                .filter(|description| !description.is_empty())
                .map(ToOwned::to_owned),
            required: input.required,
            field,
        }
    }
}

impl RepositoryView {
    /// 刷新工作流模板列表：目录 IO 与 JSON5 解析在短任务池后台执行，
    /// 结果经 `UiEvent::WorkflowTemplatesLoaded` 回到 UI 线程应用——
    /// 切换到工作流页与手动刷新都不在 UI 线程做文件系统操作。
    pub(crate) fn refresh_workflow_templates(&mut self) {
        self.workflow_templates_request_id = self.workflow_templates_request_id.wrapping_add(1);
        let request_id = self.workflow_templates_request_id;
        self.workflow_template_dir = workflow_templates_dir();
        self.status = "正在刷新工作流模板".to_string();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = load_workflow_templates();
            send_ui_event(&tx, UiEvent::WorkflowTemplatesLoaded { request_id, result });
        });
    }

    pub(crate) fn open_remote_workflow_templates(&mut self, cx: &mut Context<Self>) {
        self.active_dialog = Some(DialogState::RemoteWorkflowTemplates);
        self.load_remote_workflow_catalog(cx);
    }

    pub(crate) fn load_remote_workflow_catalog(&mut self, _cx: &mut Context<Self>) {
        if self.remote_workflow_loading {
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            self.remote_workflow_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(dir) = workflow_templates_dir() else {
            self.remote_workflow_error = Some("无法定位工作流模板目录".into());
            return;
        };
        self.remote_workflow_catalog_request_id = self.remote_workflow_catalog_request_id.wrapping_add(1);
        let request_id = self.remote_workflow_catalog_request_id;
        self.remote_workflow_loading = true;
        self.remote_workflow_error = None;
        self.remote_workflow_catalog.clear();
        self.status = "正在读取远端工作流模板目录".into();
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let result = list_cnb_templates(&service, &dir).map_err(|err| err.to_string());
            send_ui_event(&tx, UiEvent::WorkflowRemoteCatalogLoaded { request_id, result });
        });
    }

    pub(crate) fn apply_remote_workflow_catalog(
        &mut self,
        request_id: u64,
        result: Result<Vec<RemoteTemplateEntry>, String>,
        _cx: &mut Context<Self>,
    ) {
        if request_id != self.remote_workflow_catalog_request_id {
            return;
        }
        self.remote_workflow_loading = false;
        match result {
            Ok(catalog) => {
                self.status = format!("远端共有 {} 个工作流模板", catalog.len());
                self.remote_workflow_catalog = catalog;
                self.remote_workflow_error = None;
            }
            Err(err) => {
                self.status = format!("远端模板目录读取失败：{err}");
                self.remote_workflow_error = Some(err);
            }
        }
    }

    pub(crate) fn download_remote_workflow_template(&mut self, file_name: String) {
        if self.remote_workflow_downloading.is_some() {
            return;
        }
        let Some(template) = self.remote_workflow_catalog.iter()
            .find(|template| template.file_name == file_name).cloned() else {
            return;
        };
        let Some(dir) = workflow_templates_dir() else {
            self.remote_workflow_error = Some("无法定位工作流模板目录".into());
            return;
        };
        self.remote_workflow_downloading = Some(file_name.clone());
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = download_cnb_template(&dir, &template).map_err(|err| err.to_string());
            send_ui_event(&tx, UiEvent::WorkflowRemoteTemplateDownloaded { file_name, result });
        });
    }

    pub(crate) fn apply_remote_workflow_download(
        &mut self,
        file_name: String,
        result: Result<PathBuf, String>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_workflow_downloading.as_deref() != Some(&file_name) {
            return;
        }
        self.remote_workflow_downloading = None;
        match result {
            Ok(_) => {
                self.remote_workflow_error = None;
                self.refresh_workflow_templates();
                self.notify_success(format!("工作流模板已下载：{file_name}"), cx);
            }
            Err(err) => {
                self.remote_workflow_error = Some(err.clone());
                self.notify_error(format!("工作流模板下载失败：{err}"), cx);
            }
        }
    }

    /// 后台模板加载结果的应用端（由事件泵调用）。
    pub(crate) fn apply_workflow_templates(
        &mut self,
        request_id: u64,
        result: Result<Vec<WorkflowTemplateItem>, String>,
        cx: &mut Context<Self>,
    ) {
        if request_id != self.workflow_templates_request_id {
            return;
        }
        match result {
            Ok(templates) => {
                let count = templates.len();
                self.workflow_templates = templates;
                // 刷新后若当前文件已不在模板目录，清掉旧选中态，避免导航误导详情对象。
                self.workflow_state.selected_template_path = workflow_selected_template_path(
                    self.workflow_state.file_path.as_deref(),
                    &self.workflow_templates,
                );
                // 剪枝文件已不存在的工作流快捷键绑定（兜底应用外删除/改名）。
                // 仅在 Ok 分支执行：目录读取失败不代表文件已删除。
                self.prune_workflow_shortcut_bindings_against_templates(cx);
                self.last_error = None;
                self.status = format!("已刷新，共 {count} 个工作流模板");
            }
            Err(err) => {
                self.workflow_templates.clear();
                self.workflow_state.selected_template_path = None;
                // 目录读取失败不作为严重错误上报，仅记录状态；
                // 用户可通过"打开目录"验证目录是否可用
                self.status = err;
            }
        }
    }

    /// 列表刷新后剪枝模板文件已不存在的工作流快捷键绑定；有变化则保存并重注册。
    fn prune_workflow_shortcut_bindings_against_templates(&mut self, cx: &mut Context<Self>) {
        let template_files: Vec<String> = self
            .workflow_templates
            .iter()
            .map(|template| template.file_name.clone())
            .collect();
        let prefix = format!("{MANAGED_TEMPLATE_DIR}/");
        let old_files = self.workflow_shortcut_bindings.bindings.keys()
            .filter(|file| file.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        let mut migrated = false;
        for old_file in old_files {
            let file = old_file.strip_prefix(&prefix).unwrap_or_default();
            if template_files.iter().any(|name| name == file) {
                if let Some(binding) = self.workflow_shortcut_bindings.bindings.remove(&old_file) {
                    self.workflow_shortcut_bindings.bindings.entry(file.to_string()).or_insert(binding);
                    migrated = true;
                }
            }
        }
        let before = self.workflow_shortcut_bindings.bindings.len();
        self.workflow_shortcut_bindings
            .bindings
            .retain(|file, _| template_files.iter().any(|name| name == file));
        if migrated || self.workflow_shortcut_bindings.bindings.len() != before {
            tracing::warn!("workflow shortcut bindings pruned against template list");
            self.persist_workflow_shortcut_bindings(cx);
        }
    }

    /// 清空已加载的工作流（定义/预览/文件路径/变量输入/日志）。
    /// 合并自 dev_lcc：删除模板时若删的是当前加载的工作流，用它避免
    /// 详情区残留失效引用；新版 UI 重构时该方法曾被移除导致合并冲突遗漏。
    pub(crate) fn clear_workflow_file(&mut self) {
        self.workflow_state.definition_generation = self.workflow_state.definition_generation.wrapping_add(1);
        self.workflow_state.definition = None;
        self.workflow_state.preview = None;
        self.workflow_state.file_path = None;
        self.workflow_state.inputs.clear();
        self.workflow_state.source_branch = None;
        self.workflow_state.log.clear();
        self.workflow_state.pending_external = None;
        self.workflow_state.approved_external = None;
        if self.browser_runtime_origin == self.active_tab_id() {
            self.browser_runtime_notice = None;
            self.browser_runtime_origin = None;
            self.browser_runtime_resume = None;
        }
        self.last_error = None;
    }

    pub(crate) fn open_workflow_template_dir(&mut self) {
        let Some(dir) = workflow_templates_dir() else {
            self.last_error = Some("无法定位工作流模板目录".into());
            return;
        };
        if let Err(err) = ensure_workflow_templates_dir(&dir) {
            self.last_error = Some(format!("工作流模板目录创建失败：{err}"));
            return;
        }
        if let Err(err) = open_directory(&dir) {
            self.last_error = Some(format!("工作流模板目录打开失败：{err}"));
            return;
        }
        self.status = "工作流模板目录已打开".to_string();
        self.last_error = None;
    }

    pub(crate) fn load_workflow_file(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.load_workflow_file_with_source(path, None, cx);
    }

    fn load_workflow_file_with_source(
        &mut self,
        path: PathBuf,
        source_branch: Option<WorkflowSourceBranch>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                self.last_error = Some(format!("工作流文件读取失败：{err}"));
                return;
            }
        };
        let definition = match parse_workflow_json5(&content) {
            Ok(definition) => definition,
            Err(err) => {
                self.last_error = Some(err.to_string());
                return;
            }
        };
        let inputs = match self.build_workflow_inputs(&definition, &repo_path, source_branch.as_ref(), cx) {
            Ok(inputs) => inputs,
            Err(err) => {
                self.last_error = Some(err.to_string());
                return;
            }
        };
        let template_match = self
            .workflow_templates
            .iter()
            .find(|template| template.path == path)
            .map(|template| template.path.clone());
        self.workflow_state.definition_generation = self.workflow_state.definition_generation.wrapping_add(1);
        self.workflow_state.pending_external = None;
        self.workflow_state.approved_external = None;
        if self.browser_runtime_origin == self.active_tab_id() {
            self.browser_runtime_notice = None;
            self.browser_runtime_origin = None;
            self.browser_runtime_resume = None;
        }
        self.workflow_state.definition = Some(definition);
        self.workflow_state.file_path = Some(path);
        // 外部选择的文件不借用旧模板选中态，模板列表中的文件则与已加载路径保持一致。
        self.workflow_state.selected_template_path = template_match;
        self.workflow_state.inputs = inputs;
        self.workflow_state.source_branch = source_branch;
        self.workflow_state.log.clear();
        self.status = "工作流已加载".to_string();
        self.last_error = None;
        self.refresh_workflow_preview();
    }

    pub(crate) fn open_workflow_from_branch(
        &mut self,
        path: PathBuf,
        branch: String,
        kind: BranchKind,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.branch_context_menu = None;
        self.clear_workflow_file();
        self.load_workflow_file_with_source(
            path,
            Some(WorkflowSourceBranch { name: branch, kind }),
            cx,
        );
        self.set_main_mode(MainMode::Workflow);
    }

    pub(crate) fn run_workflow(&mut self) {
        if !self.ensure_no_merge_in_progress("运行工作流") {
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
        let Some(definition) = self.workflow_state.definition.clone() else {
            self.last_error = Some("请先选择工作流文件".into());
            return;
        };
        if self.busy {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let external_grant = match WorkflowExternalGrant::for_definition(&definition) {
            Ok(grant) => grant,
            Err(err) => { self.last_error = Some(err.to_string()); return; }
        };
        let external = if let Some(mut grant) = external_grant {
            match self.workflow_state.approved_external.take() {
                Some(approved) if approved.tab_id == tab_id
                    && approved.definition_generation == self.workflow_state.definition_generation => Some(approved),
                _ => {
                    let Some(data_dir) = khaslana::storage::active_data_dir() else {
                        self.last_error = Some("无法定位 MCP 配置目录".into());
                        return;
                    };
                    let mut config = match load_mcp_config(&data_dir) {
                        Ok(config) => config,
                        Err(err) => { self.last_error = Some(err.to_string()); return; }
                    };
                    let uses_browser = grant.uses_browser_runtime(&config);
                    if uses_browser && !browser_runtime::ready(&data_dir) {
                        self.browser_runtime_notice = Some("浏览器 MCP 运行组件尚未就绪，点击下载并启用".into());
                        self.browser_runtime_origin = Some(tab_id);
                        self.last_error = None;
                        return;
                    }
                    if uses_browser {
                        if let Err(err) = config.configure_browser_proxy(&self.proxy_settings) {
                            self.last_error = Some(err.to_string()); return;
                        }
                    }
                    if self.browser_runtime_origin == Some(tab_id) && !self.browser_runtime_downloading {
                        self.browser_runtime_notice = None;
                        self.browser_runtime_origin = None;
                    }
                    if let Err(err) = grant.prepare_skills(&data_dir) {
                        self.last_error = Some(err.to_string()); return;
                    }
                    let permissions = match grant.permission_lines(&config) {
                        Ok(lines) => lines,
                        Err(err) => { self.last_error = Some(err.to_string()); return; }
                    };
                    self.workflow_state.pending_external = Some(WorkflowPendingExternal {
                        tab_id,
                        definition_generation: self.workflow_state.definition_generation,
                        config,
                        grant,
                    });
                    self.active_dialog = Some(DialogState::ConfirmWorkflowExternal {
                        tab_id,
                        definition_generation: self.workflow_state.definition_generation,
                        permissions,
                    });
                    self.last_error = None;
                    return;
                }
            }
        } else { None };
        let control = WorkflowRunControl::new();
        let identity = WorkflowRunIdentity {
            tab_id,
            run_id: control.id(),
            repo_path: repo_path.clone(),
            definition_generation: self.workflow_state.definition_generation,
        };
        self.workflow_state.active_run = Some(WorkflowActiveRun {
            identity: identity.clone(),
            control: control.clone(),
        });
        self.workflow_state.log.clear();
        let service = self.service_for_tab(tab_id);
        let ai_settings = self.ai_settings.clone();
        let ai_proxy = self.proxy_settings.proxy_url_for_target(&ai_settings.normalized_base_url());
        let external_data_dir = external.as_ref().and_then(|_| khaslana::storage::active_data_dir());
        let tx = self.tx.clone();
        let options = self.workflow_run_options();
        self.apply_status_event(Some(tab_id), |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = OperationBlocker::Modal;
            this.status = "正在运行工作流".to_string();
            this.last_error = None;
        });
        self.tasks.spawn(if external.is_some() { TaskKind::External } else { TaskKind::Long }, move || {
            let result = catch_unwind(AssertUnwindSafe(
                || -> khaslana::Result<(RepositorySnapshot, Vec<WorkflowLogEntry>, String)> {
                    let mut repo = Repository::open(&repo_path)?;
                    let mut log: Vec<WorkflowLogEntry> = Vec::new();
                    let mut registry = khaslana::WorkflowActionRegistry::default();
                    if let Some(external) = external {
                        register_external_actions_with_ai(&mut registry, external.config,
                            Some(external.grant), Some(ai_settings), ai_proxy,
                            external_data_dir.as_deref())?;
                    }
                    let result = WorkflowExecutor::with_actions(&service, &registry).run_with_control(
                        &mut repo, &definition, options, &control,
                        |event| {
                            let entry = workflow_progress_entry(&event);
                            log.push(entry.clone());
                            send_ui_event(&tx, UiEvent::WorkflowProgress {
                                identity: identity.clone(), entry,
                            });
                        },
                    )?;
                    let message =
                        format!("工作流“{}”已完成（{} 步）", result.name, result.steps_run);
                    Ok((result.snapshot, log, message))
                },
            ));
            match result {
                Ok(Ok((snapshot, log, message))) => {
                    send_ui_event(
                        &tx,
                        UiEvent::WorkflowFinished {
                            identity,
                            message,
                            snapshot,
                            log,
                        },
                    );
                }
                other => {
                    let (error, cancelled) = match other {
                        Ok(Err(GitError::WorkflowCancelled)) => {
                            ("工作流已取消；当前步骤可能已生效，已完成的步骤不会撤销".to_string(), true)
                        }
                        Ok(Err(err)) => (err.to_string(), false),
                        Err(payload) => {
                            (format!("工作流异常退出：{}", panic_message(payload)), false)
                        }
                        Ok(Ok(_)) => unreachable!(),
                    };
                    let snapshot = catch_unwind(AssertUnwindSafe(|| {
                        Repository::open(&repo_path).ok()
                            .and_then(|mut repo| service.snapshot_after_operation(&mut repo).ok())
                    }))
                    .ok()
                    .flatten();
                    send_ui_event(&tx, UiEvent::WorkflowStopped {
                        identity,
                        error,
                        cancelled,
                        snapshot,
                    });
                }
            }
        });
    }

    pub(crate) fn cancel_workflow(&mut self) {
        let Some(run) = self.workflow_state.active_run.as_ref() else {
            return;
        };
        run.control.cancel();
        self.status = "正在取消工作流，等待当前步骤结束".into();
    }

    pub(crate) fn start_browser_runtime_download(&mut self) {
        self.start_browser_runtime_download_inner(true);
    }

    pub(crate) fn start_browser_runtime_download_for_settings(&mut self) {
        self.start_browser_runtime_download_inner(false);
    }

    fn start_browser_runtime_download_inner(&mut self, resume_workflow: bool) {
        if self.browser_runtime_downloading { return; }
        let tab_id = if resume_workflow { self.active_tab_id() } else { None };
        if resume_workflow && tab_id.is_none() { return; }
        self.browser_runtime_origin = tab_id;
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.browser_runtime_notice = Some("无法定位应用数据目录".into());
            return;
        };
        self.browser_runtime_request_id = self.browser_runtime_request_id.wrapping_add(1);
        let request_id = self.browser_runtime_request_id;
        self.browser_runtime_downloading = true;
        self.browser_runtime_resume = tab_id
            .map(|tab_id| (tab_id, self.workflow_state.definition_generation));
        self.browser_runtime_notice = Some("正在准备浏览器 MCP 运行组件".into());
        let proxy = self.proxy_settings.clone();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::External, move || {
            let result = match catch_unwind(AssertUnwindSafe(||
                browser_runtime::install(&data_dir, &proxy, |message| {
                    send_ui_event(&tx, UiEvent::WorkflowBrowserRuntimeProgress { request_id, message });
                }))) {
                Ok(result) => result.map_err(|err| err.to_string()),
                Err(payload) => Err(format!("浏览器 MCP 安装异常：{}", panic_message(payload))),
            };
            send_ui_event(&tx, UiEvent::WorkflowBrowserRuntimeFinished { request_id, result });
        });
    }

    pub(crate) fn apply_browser_runtime_finished(&mut self, request_id: u64,
        result: Result<(), String>, cx: &mut Context<Self>) {
        if request_id != self.browser_runtime_request_id { return; }
        self.browser_runtime_downloading = false;
        let resume = self.browser_runtime_resume.take();
        match result {
            Ok(()) => {
                self.browser_runtime_notice = None;
                self.browser_runtime_origin = None;
                self.notify_success("浏览器 MCP 运行组件已就绪", cx);
                if resume.is_some_and(|(tab_id, generation)|
                    self.active_tab_id() == Some(tab_id)
                        && self.workflow_state.definition_generation == generation) {
                    self.run_workflow();
                }
            }
            Err(error) => {
                self.browser_runtime_notice = Some(format!("浏览器 MCP 下载失败：{error}"));
            }
        }
        if self.settings_center == Some(crate::SettingsCategory::Ai) {
            self.refresh_ai_extensions();
        }
        cx.notify();
    }

    /// 必填变量缺失检测：必填且当前文本为空（有默认值的必填项在构建输入时已预填）。
    fn workflow_missing_required_input(inputs: &[WorkflowInputFieldState]) -> bool {
        inputs
            .iter()
            .any(|input| input.required && input.field.value.trim().is_empty())
    }

    /// 工作流快捷键触发入口：按文件名解析模板 → 重读磁盘最新内容（模板被
    /// 修改后运行的一定是最新版本）→ 按 key 合并回填已填变量 → 依「后台执行」
    /// 配置跳页运行或后台运行。守卫顺序与 `run_workflow` 一致，执行闭环保留在
    /// `run_workflow` 内（busy/合并/净树检查与事件闭环全部复用）。
    pub(crate) fn trigger_workflow_shortcut(
        &mut self,
        file: String,
        background: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_no_merge_in_progress("运行工作流") {
            return;
        }
        if self.active_tab_id().is_none() || self.repo_path.is_none() {
            self.notify_error("请先打开一个仓库", cx);
            return;
        }
        // 解析模板路径：优先模板列表（file_name 大小写不敏感匹配），回退模板
        // 目录直接探盘（本会话尚未进入工作流页、列表未加载时绑定仍可触发）。
        let path = self
            .workflow_templates
            .iter()
            .find(|template| template.file_name.eq_ignore_ascii_case(&file))
            .map(|template| template.path.clone())
            .or_else(|| {
                workflow_templates_dir()
                    .map(|dir| dir.join(&file))
                    .filter(|path| path.is_file())
            });
        let Some(path) = path else {
            // 模板已不存在：自愈移除失效绑定，不留死键位。
            self.remove_workflow_shortcut_binding(&file, cx);
            self.notify_warning(format!("工作流模板不存在或已被移动：{file}"), cx);
            return;
        };
        // 快照旧输入值，重载后按 key 合并回填（后台重复触发不丢已填变量）。
        let previous_inputs: BTreeMap<String, String> = self.workflow_input_values();
        self.load_workflow_file(path, cx);
        if self.workflow_state.definition.is_none() {
            // load_workflow_file 已把读取/解析错误写入 last_error（模板损坏时不拿
            // 内存旧定义运行），转成 toast。
            let message = self
                .last_error
                .take()
                .unwrap_or_else(|| "工作流文件加载失败".into());
            self.notify_error(message, cx);
            return;
        }
        merge_workflow_input_values(&previous_inputs, &mut self.workflow_state.inputs);
        let missing_required = Self::workflow_missing_required_input(&self.workflow_state.inputs);
        match workflow_shortcut_trigger_decision(missing_required, background) {
            WorkflowShortcutTrigger::JumpToFillInputs => {
                self.set_main_mode(MainMode::Workflow);
                self.notify_warning("请先填写运行变量后再运行", cx);
            }
            WorkflowShortcutTrigger::JumpAndRun => {
                self.set_main_mode(MainMode::Workflow);
                self.run_workflow();
            }
            WorkflowShortcutTrigger::RunInBackground => {
                self.run_workflow();
            }
        }
    }

    /// 打开「绑定工作流快捷键」弹窗（模板行右键菜单入口）。
    pub(crate) fn open_workflow_shortcut_binding_dialog(&mut self, path: PathBuf) {
        let Some(file) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            return;
        };
        self.close_popups();
        self.active_dialog = Some(DialogState::WorkflowShortcutBinding { file });
        self.last_error = None;
    }

    /// 绑定弹窗 body：当前键位、「后台执行」开关（默认不勾选 = 跳页运行）、
    /// 录制（复用根捕获 + skip_shortcuts 机制）与清除。
    pub(crate) fn render_workflow_shortcut_binding_dialog(
        &self,
        file: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let binding = self.workflow_shortcut_bindings.bindings.get(file).cloned();
        let is_recording = matches!(
            &self.recording_shortcut,
            Some(crate::ShortcutRecordingTarget::Workflow { file: target }) if target == file
        );
        let display_name = workflow_display_name_for_file(&self.workflow_templates, file);
        let current_binding_text = binding
            .as_ref()
            .map(|binding| crate::shortcuts_view::format_keystroke(&binding.keystroke))
            .unwrap_or_else(|| "未绑定".to_string());
        let background_checked = binding.as_ref().is_some_and(|b| b.background);

        self.dialog_panel("绑定工作流快捷键", cx)
            .track_focus(&self.workflow_shortcut_binding_focus)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(display_name),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("当前绑定：{current_binding_text}")),
            )
            // 「后台执行」勾选框：默认不勾选（触发时跳转到工作流页并运行）；
            // 勾选后留在当前页后台运行（进度走状态栏，完成/失败走 toast）。
            // 未绑定键位时不可开启（无绑定即无触发语义）。
            .child({
                let toggle: Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> = {
                    let file = file.to_string();
                    Rc::new(move |this, _window, cx| {
                        if let Some(binding) =
                            this.workflow_shortcut_bindings.bindings.get_mut(&file)
                        {
                            binding.background = !binding.background;
                            this.persist_workflow_shortcut_bindings(cx);
                        }
                    })
                };
                let toggle_for_label = toggle.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.toggle_switch(
                        "workflow-shortcut-background-toggle",
                        background_checked,
                        binding.is_none(),
                        move |this, _next, window, cx| toggle(this, window, cx),
                        cx,
                    ))
                    .child(
                        div()
                            .id("workflow-shortcut-background-label")
                            .when(binding.is_some(), |this| {
                                this.cursor_pointer().on_click(cx.listener(
                                    move |this, _event, window, cx| {
                                        toggle_for_label(this, window, cx);
                                        cx.notify();
                                    },
                                ))
                            })
                            .when(binding.is_none(), |this| {
                                this.opacity(0.62).cursor_not_allowed()
                            })
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("后台执行（触发时不切换到工作流页）"),
                    )
            })
            .child(
                dialog_actions()
                    .child(
                        div()
                            .id("workflow-shortcut-record")
                            .flex()
                            .items_center()
                            .justify_center()
                            .min_h(px(28.0))
                            .px(px(10.0))
                            .py_1()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER_MUTED))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::SURFACE_RAISED))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                            .on_click(cx.listener({
                                let file = file.to_string();
                                move |this, _event, window, cx| {
                                    let target = crate::ShortcutRecordingTarget::Workflow {
                                        file: file.clone(),
                                    };
                                    if this.recording_shortcut.as_ref() == Some(&target) {
                                        // 再次点击取消录制，恢复正常绑定。
                                        this.recording_shortcut = None;
                                        crate::register_all_key_bindings(
                                            &mut cx.deref_mut(),
                                            &this.shortcut_bindings,
                                            &this.workflow_shortcut_bindings,
                                            false,
                                        );
                                    } else {
                                        // 进入录制态：夺取焦点到弹窗面板（使 keydown
                                        // dispatch_path 经过 overlay），跳过全部快捷键
                                        // 绑定，按键直达根捕获层。
                                        this.recording_shortcut = Some(target.clone());
                                        window.focus(&this.workflow_shortcut_binding_focus, cx);
                                        crate::register_all_key_bindings(
                                            &mut cx.deref_mut(),
                                            &this.shortcut_bindings,
                                            &this.workflow_shortcut_bindings,
                                            true,
                                        );
                                    }
                                    cx.notify();
                                }
                            }))
                            .child(if is_recording {
                                "取消录制（Esc）"
                            } else {
                                "开始录制"
                            }),
                    )
                    .when(binding.is_some(), |this| {
                        this.child(
                            div()
                                .id("workflow-shortcut-clear")
                                .flex()
                                .items_center()
                                .justify_center()
                                .min_h(px(28.0))
                                .px(px(10.0))
                                .py_1()
                                .border_1()
                                .border_color(rgb(ui_theme::BORDER_MUTED))
                                .rounded(px(ui_theme::RADIUS_XS))
                                .bg(rgb(ui_theme::SURFACE_RAISED))
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                                .on_click(cx.listener({
                                    let file = file.to_string();
                                    move |this, _event, _window, cx| {
                                        this.remove_workflow_shortcut_binding(&file, cx);
                                        cx.notify();
                                    }
                                }))
                                .child("清除快捷键"),
                        )
                    })
                    .child(
                        div()
                            .id("workflow-shortcut-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .min_h(px(28.0))
                            .px(px(10.0))
                            .py_1()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::PRIMARY))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::PRIMARY_FOREGROUND))
                            .cursor_pointer()
                            .hover(|this| this.opacity(0.9))
                            .on_click(cx.listener({
                                let file = file.to_string();
                                move |this, _event, _window, cx| {
                                    // 关闭前若本模板仍在录制，取消录制并恢复正常绑定。
                                    if matches!(
                                        &this.recording_shortcut,
                                        Some(crate::ShortcutRecordingTarget::Workflow { file: target }) if target == &file
                                    ) {
                                        this.recording_shortcut = None;
                                        crate::register_all_key_bindings(
                                            &mut cx.deref_mut(),
                                            &this.shortcut_bindings,
                                            &this.workflow_shortcut_bindings,
                                            false,
                                        );
                                    }
                                    this.close_dialog();
                                    cx.notify();
                                }
                            }))
                            .child("关闭"),
                    ),
            )
    }

    pub(crate) fn render_workflow_external_confirm_dialog(
        &self,
        tab_id: crate::RepoTabId,
        definition_generation: u64,
        permissions: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        const SCROLL_ID: &str = "workflow-external-permissions";
        let valid = self.active_tab_id() == Some(tab_id)
            && self.workflow_state.definition_generation == definition_generation
            && self.workflow_state.pending_external.as_ref().is_some_and(|pending|
                pending.tab_id == tab_id && pending.definition_generation == definition_generation);
        let handle = self.scroll_handle(SCROLL_ID);
        let content = div()
            .id(SCROLL_ID)
            .flex()
            .flex_col()
            .gap_2()
            .overflow_y_scroll()
            .track_scroll(&handle)
            .children(permissions.iter().map(|permission| {
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(permission.clone())
            }))
            .into_any_element();
        self.dialog_panel("确认工作流外部操作", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("本次运行将执行以下本地脚本或外部工具。请检查模板及本地 MCP 配置。"),
            )
            .child(div().h(px(180.0)).min_h(px(0.0)).child(
                scrollable_frame_when(SCROLL_ID, ScrollbarMode::Vertical, content, handle, true, cx),
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| {
                        this.workflow_state.pending_external = None;
                        this.close_dialog();
                    }, cx))
                    .child(self.button("授权并运行一次", valid, move |this, _, _| {
                        let pending = this.workflow_state.pending_external.take();
                        this.close_dialog();
                        if let Some(pending) = pending.filter(|pending|
                            this.active_tab_id() == Some(tab_id)
                            && pending.definition_generation == definition_generation
                            && this.workflow_state.definition_generation == definition_generation) {
                            this.workflow_state.approved_external = Some(pending);
                            this.run_workflow();
                        }
                    }, cx)),
            )
    }

    pub(crate) fn render_remote_workflow_template_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        const SCROLL_ID: &str = "remote-workflow-template-list";
        let (panel_width, panel_height) = dialog_panel_size(window, 680.0, 560.0);
        let root = workflow_templates_dir();
        let rows = self.remote_workflow_catalog.iter().map(|template| {
            let file_name = template.file_name.clone();
            let exists = root.as_ref().is_some_and(|root| root.join(&file_name).exists());
            let downloading = self.remote_workflow_downloading.as_deref() == Some(&file_name);
            let enabled = !exists && self.remote_workflow_downloading.is_none();
            let label = if exists { "已存在" } else if downloading { "下载中" } else { "下载" };
            let button_name = file_name.clone();
            let commit = template.commit.chars().take(8).collect::<String>();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(ui_theme::BORDER_MUTED))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                .child(template.display_name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(format!("{file_name} · {commit}")),
                        ),
                )
                .child(
                    BaseButton::new(format!("remote-workflow-download-{file_name}"))
                        .disabled(!enabled)
                        .accessibility_label(format!("下载工作流模板 {file_name}"))
                        .focus_visible(|this| this.border_1().border_color(rgb(ui_theme::PRIMARY)))
                        .flex_none()
                        .min_h(px(28.0))
                        .px_3()
                        .rounded(px(ui_theme::RADIUS_XS))
                        .border_1()
                        .border_color(rgb(ui_theme::BORDER_MUTED))
                        .bg(rgb(ui_theme::SURFACE_RAISED))
                        .text_size(px(12.0))
                        .when(enabled, |this| this.cursor_pointer())
                        .when(!enabled, |this| this.opacity(0.6).cursor_not_allowed())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.download_remote_workflow_template(button_name.clone());
                                cx.notify();
                            }
                        }))
                        .child(label),
                )
                .into_any_element()
        }).collect::<Vec<_>>();
        let handle = self.scroll_handle(SCROLL_ID);
        let content = div()
            .id(SCROLL_ID)
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .children(rows)
            .into_any_element();

        div()
            .id("dialog-远端工作流模板")
            .w(panel_width)
            .h(panel_height)
            .p_4()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("下载工作流模板"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("从 CNB 目录选择模板下载；本地同名文件会保留。"),
            )
            .when_some(self.remote_workflow_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .rounded_sm()
                    .when(self.remote_workflow_loading, |this| {
                        this.child(workflow_empty_line("正在读取远端模板…"))
                    })
                    .when(!self.remote_workflow_loading && self.remote_workflow_catalog.is_empty(), |this| {
                        this.child(workflow_empty_line("暂无可显示的远端模板"))
                    })
                    .when(!self.remote_workflow_catalog.is_empty(), |this| {
                        this.child(scrollable_frame_when(
                            SCROLL_ID,
                            ScrollbarMode::Vertical,
                            content,
                            handle,
                            true,
                            cx,
                        ))
                    }),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        "重新加载",
                        !self.remote_workflow_loading && self.remote_workflow_downloading.is_none(),
                        |this, _, cx| this.load_remote_workflow_catalog(cx),
                        cx,
                    ))
                    .child(self.button("关闭", true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    pub(crate) fn render_workflow_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 模板列表与 Runbook Studio 各是一张悬浮面板，页面根不铺底色。
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(self.render_workflow_template_column(cx))
            .child(self.render_column_splitter(ResizeTarget::WorkflowTemplates, cx))
            .child(self.render_workflow_detail(window, cx))
    }

    fn render_workflow_detail(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file_label = self
            .workflow_state
            .file_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "未选择工作流文件".to_string());
        let source_label = self.workflow_state.source_branch.as_ref().map(|source| {
            let kind = match source.kind {
                BranchKind::Local => "本地",
                BranchKind::Remote => "远端",
            };
            format!("来源分支：{}（{kind}）", source.name)
        });
        let workflow_name = self
            .workflow_state
            .preview
            .as_ref()
            .map(|preview| preview.name.clone())
            .or_else(|| {
                self.workflow_state
                    .definition
                    .as_ref()
                    .map(|definition| definition.display_name())
            })
            .unwrap_or_else(|| "尚未选择工作流".to_string());
        let workflow_name = if self.workflow_state.definition.is_some()
            && self.workflow_state.file_path.is_some()
            && self.workflow_state.selected_template_path.is_none()
        {
            format!("外部工作流 · {workflow_name}")
        } else {
            workflow_name
        };
        let layout = workflow_studio_layout(
            self.workflow_state.definition.is_some(),
            !self.workflow_state.inputs.is_empty(),
            self.busy,
            self.workflow_state.log.len(),
        );
        let status_label = if self.busy {
            "运行中"
        } else if self.last_error.is_some() {
            "需要修正"
        } else if self.workflow_state.definition.is_some() {
            "已就绪"
        } else {
            "未选择"
        };
        let status_color = if self.busy {
            ui_theme::PRIMARY
        } else if self.last_error.is_some() {
            ui_theme::FEEDBACK_ERROR_TEXT
        } else if self.workflow_state.definition.is_some() {
            ui_theme::FEEDBACK_SUCCESS_TEXT
        } else {
            ui_theme::CONTENT_SECONDARY
        };

        // Runbook Studio 是右侧独立悬浮面板：输入、步骤预览与运行日志
        // 都在同一张卡内分层，卡与模板列表之间只隔拖拽区的空隙。
        floating_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(
                page_header("Runbook Studio", None).child(
                    command_group()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(px(120.0))
                                .max_w(px(260.0))
                                .text_size(px(11.0))
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(workflow_name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                        .child(file_label),
                                )
                                .when_some(source_label, |this, source| {
                                    this.child(
                                        div()
                                            .truncate()
                                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                            .child(source),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(if self.busy {
                                    ui_theme::PRIMARY_SUBTLE
                                } else if self.last_error.is_some() {
                                    ui_theme::FEEDBACK_ERROR_BG
                                } else if self.workflow_state.definition.is_some() {
                                    ui_theme::FEEDBACK_SUCCESS_BG
                                } else {
                                    ui_theme::SURFACE_SUNKEN
                                }))
                                .text_size(px(11.0))
                                .text_color(rgb(status_color))
                                .child(status_label),
                        )
                        .child(self.primary_button(
                            if self.busy { "运行中..." } else { "运行" },
                            self.workflow_state.definition.is_some() && !self.busy,
                            |this, _, _| this.run_workflow(),
                            cx,
                        ))
                        .when(self.workflow_state.active_run.as_ref().is_some_and(|run| !run.control.is_cancelled()), |this| {
                            this.child(self.secondary_button(
                                "取消运行".into(),
                                true,
                                |this, _, _| this.cancel_workflow(),
                                cx,
                            ))
                        }),
                ),
            )
            .when_some(self.browser_runtime_notice.clone().filter(|_|
                self.browser_runtime_origin == self.active_tab_id()), |this, notice| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(ui_theme::SPACE_2))
                        .px(px(ui_theme::SPACE_4))
                        .py(px(ui_theme::SPACE_2))
                        .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                .child(notice),
                        )
                        .child(self.button(
                            if self.browser_runtime_downloading { "下载中..." } else { "下载并启用" },
                            !self.browser_runtime_downloading,
                            |this, _, _| this.start_browser_runtime_download(),
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(ui_theme::SPACE_4))
                    .gap(px(ui_theme::SPACE_3))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .gap(px(ui_theme::SPACE_4))
                            .when(layout.has_inputs(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .flex_col()
                                        .w(px(280.0))
                                        .h_full()
                                        .min_h(px(0.0))
                                        .child(self.render_workflow_inputs(window, cx)),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .child(self.render_workflow_preview(cx)),
                            ),
                    )
                    .child(self.render_workflow_log(cx)),
            )
    }

    fn render_workflow_template_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dir_label = self
            .workflow_template_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "无法定位模板目录".to_string());
        let model = Arc::new(WorkflowTemplateListModel::from_templates(
            &self.workflow_templates,
        ));
        let content_present = !model.indices.is_empty();
        // 渲染宽度只做一次钳制（与拖拽层共用 main.rs 的同一对常量）；
        // 不得在渲染层另设窄钳制区间，否则拖拽状态的变化不会反映到布局。
        let width = self.workflow_templates_width.clamp(
            crate::MIN_WORKFLOW_TEMPLATES_WIDTH,
            crate::MAX_WORKFLOW_TEMPLATES_WIDTH,
        );
        let scroll_id = WORKFLOW_TEMPLATE_LIST_SCROLL_ID;
        let legacy_handle = self.scroll_handle(scroll_id);
        let scroll_handle = self.uniform_scroll_handle(scroll_id);
        // 复用旧句柄的底层滚动状态，保持切换仓库和既有滚动条交互语义。
        scroll_handle.0.borrow_mut().base_handle = legacy_handle;
        let list_handle = scroll_handle.clone();
        let model_for_rows = Arc::clone(&model);

        // 模板导航列是独立悬浮面板（白底 + 圆角 + 投影），与右侧内容面板
        // 之间只隔拖拽区的空隙；右侧分隔线由列分割条统一绘制。
        floating_panel()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_w(px(crate::MIN_WORKFLOW_TEMPLATES_WIDTH))
            .min_h(px(0.0))
            .child(
                panel_section_header("模板导航")
                    .top_rounded()
                    .action(
                        self.button(
                            "新建",
                            !self.busy,
                            |this, _, cx| this.open_workflow_editor(cx),
                            cx,
                        )
                        .into_any_element(),
                    )
                    .action(
                        self.button(
                            "刷新",
                            !self.busy,
                            |this, _, cx| {
                                this.refresh_workflow_templates();
                                cx.notify();
                            },
                            cx,
                        )
                        .into_any_element(),
                    )
                    .action(
                        self.button(
                            "下载模板",
                            !self.busy,
                            |this, _, cx| this.open_remote_workflow_templates(cx),
                            cx,
                        )
                        .into_any_element(),
                    )
                    .action(
                        self.button(
                            "目录",
                            !self.busy,
                            |this, _, _| this.open_workflow_template_dir(),
                            cx,
                        )
                        .into_any_element(),
                    )
                    .build(),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .text_size(px(ui_theme::TYPE_META))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(dir_label),
            )
            .child({
                let model_for_rows = Arc::clone(&model_for_rows);
                let content = div()
                    .id(scroll_id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(ui_theme::SPACE_2))
                    .child(
                        uniform_list(
                            scroll_id,
                            model.len().max(1),
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, cx| {
                                    range
                                        .map(|row| {
                                            let element = model_for_rows
                                                .index_at(row)
                                                .and_then(|index| {
                                                    this.workflow_templates.get(index)
                                                })
                                                .map(|template| {
                                                    this.workflow_template_row(template, cx)
                                                        .into_any_element()
                                                })
                                                .unwrap_or_else(|| {
                                                    workflow_empty_line(
                                                        if model_for_rows.len() == 0 {
                                                            "暂无模板，可从目录或外部文件加载"
                                                        } else {
                                                            ""
                                                        },
                                                    )
                                                    .into_any_element()
                                                });
                                            div()
                                                .flex()
                                                .flex_none()
                                                .w_full()
                                                // 行槽位保持 56px 均匀高度，上下各让 2px
                                                // 内边距，多个模板的白色底色之间留出缝隙。
                                                .py(px(2.0))
                                                .h(px(WORKFLOW_TEMPLATE_ROW_HEIGHT))
                                                .min_h(px(WORKFLOW_TEMPLATE_ROW_HEIGHT))
                                                .child(element)
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .with_sizing_behavior(ListSizingBehavior::Auto)
                        .track_scroll(&list_handle)
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full(),
                    )
                    .into_any_element();
                scrollable_uniform_frame(
                    scroll_id,
                    ScrollbarMode::Vertical,
                    content,
                    scroll_handle,
                    content_present,
                    cx,
                )
            })
    }

    fn workflow_template_row(
        &self,
        template: &WorkflowTemplateItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let click_path = template.path.clone();
        let enabled = !self.busy;
        let right_click_path = template.path.clone();
        let has_error = template.error.is_some();
        // 已绑定快捷键的模板在行右缘显示键位 chip（后台执行的加「后台」标记）；
        // 文本行同步留出右内边距，避免与 chip 重叠。
        let shortcut_binding = self
            .workflow_shortcut_bindings
            .bindings
            .get(&template.file_name)
            .map(|binding| (binding.keystroke.clone(), binding.background));
        let has_shortcut = shortcut_binding.is_some();
        let selected = workflow_template_selection_matches(
            self.workflow_state.selected_template_path.as_deref(),
            self.workflow_state.file_path.as_deref(),
            Some(&template.path),
        );
        // 背景、悬停与命中区域填满 uniform_list 槽位，不随两行文本的实际宽度收缩。
        // 行内文本禁止 truncate()/text_ellipsis：uniform_list 每帧以 MinContent 测量
        // 第 0 项，行内容会塌到 min-content 宽度（约 0）；带省略号的 nowrap 文本在
        // 该测量轮按坍缩宽度截断后，TextLayout 记忆化（wrap_width=None 恒命中）会把
        // 这份截断布局固化到绘制——模板名从此永远只显示「…」。因此这里用
        // overflow_hidden + nowrap 硬裁剪代替省略号截断；完整名称由右侧运行配置
        // 面板的标题展示。
        list_row_surface(
            format!("workflow-template-{}", template.path.display()),
            selected,
        )
        .flex()
        .w_full()
        .min_w(px(0.0))
        .h_full()
        .flex_col()
        .gap(px(ui_theme::SPACE_1))
        .justify_center()
        .px(px(ui_theme::SPACE_2))
        .py(px(ui_theme::SPACE_2))
        .when(enabled, |this| this.cursor_pointer())
        .when(!enabled, |this| this.cursor_not_allowed().opacity(0.62))
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            // 单击就是标准加载入口；双击保留幂等行为，但不再承担唯一加载职责。
            if workflow_template_click_loads(event.standard_click(), this.busy) {
                this.load_workflow_file(click_path.clone(), cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                // 右键菜单互斥：先清掉其它弹层再打开本菜单
                this.branch_context_menu = None;
                this.remote_context_menu = None;
                this.change_context_menu = None;
                this.file_path_context_menu = None;
                this.credential_context_menu = None;
                this.tag_context_menu = None;
                this.stash_context_menu = None;
                this.commit_context_menu = None;
                this.encoding_menu_target = None;
                this.active_dialog = None;
                let (x, y) = clamped_menu_position(
                    event,
                    window,
                    WORKFLOW_TEMPLATE_MENU_WIDTH,
                    WORKFLOW_TEMPLATE_MENU_HEIGHT,
                );
                this.reset_context_menu_selection();
                this.workflow_template_context_menu = Some(WorkflowTemplateContextMenu {
                    path: right_click_path.clone(),
                    x,
                    y,
                });
                cx.notify();
            }),
        )
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .font_weight(if selected {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if has_error {
                    rgb(ui_theme::DESTRUCTIVE)
                } else {
                    rgb(ui_theme::CONTENT_PRIMARY)
                })
                .when(has_shortcut, |this| this.pr(px(72.0)))
                .child(template.display_name.clone()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(10.0))
                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                .when(has_shortcut, |this| this.pr(px(72.0)))
                .child(format!(
                    "{} · {}{}",
                    template.file_name,
                    template.modified_label,
                    template
                        .error
                        .as_ref()
                        .map(|error| format!(" · {error}"))
                        .unwrap_or_default()
                )),
        )
        .when_some(shortcut_binding, |this, (keystroke, background)| {
            this.relative().child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .right(px(ui_theme::SPACE_2))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::STATE_HOVER))
                            .text_size(px(10.0))
                            .font_family("Consolas")
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(crate::shortcuts_view::format_keystroke(&keystroke)),
                    )
                    .when(background, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(ui_theme::RADIUS_PILL))
                                .bg(rgb(ui_theme::WB_ROW_HOVER))
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                .child("后台"),
                        )
                    }),
            )
        })
    }

    pub(crate) fn workflow_input_field(&self, index: usize) -> &TextFieldState {
        // 直接索引会在 inputs 被异步重建后越界 panic（FieldId 跨重建存活的
        // 场景：IME 组合中/暂存焦点）。回退 branch_name 满足借用契约，
        // 与 WorkflowEditor 分支的 Option 化处理同构。
        self.workflow_state
            .inputs
            .get(index)
            .map(|input| &input.field)
            .unwrap_or(&self.branch_name)
    }

    pub(crate) fn workflow_input_field_mut(&mut self, index: usize) -> &mut TextFieldState {
        // 借用隔离（同 workflow_editor_field_or_fallback 的探测-分路技巧）：
        // workflow_state 经 Deref 落在 RepoTabState 上，match 写法会让
        // scrutinee 的借用横跨两臂。越界（inputs 异步重建后 FieldId 仍被
        // 寻址的瞬态）回落 branch_name，与只读版兜底一致、不 panic。
        if self.workflow_state.inputs.get(index).is_some() {
            &mut self.workflow_state.inputs[index].field
        } else {
            &mut self.branch_name
        }
    }

    pub(crate) fn focused_workflow_input(&self, window: &Window) -> Option<FieldId> {
        self.workflow_state
            .inputs
            .iter()
            .enumerate()
            .find_map(|(index, input)| {
                input
                    .field
                    .focus
                    .is_focused(window)
                    .then_some(FieldId::WorkflowInput(index))
            })
    }

    pub(crate) fn workflow_input_changed(&mut self) {
        if self.workflow_state.definition.is_some() {
            self.refresh_workflow_preview();
        }
    }

    fn build_workflow_inputs(
        &self,
        definition: &WorkflowDefinition,
        repo_path: &std::path::Path,
        source_branch: Option<&WorkflowSourceBranch>,
        cx: &mut Context<Self>,
    ) -> khaslana::Result<Vec<WorkflowInputFieldState>> {
        let mut fields = Vec::new();
        let tab_id = self
            .active_tab_id()
            .ok_or_else(|| khaslana::GitError::Message("请先打开一个仓库".into()))?;
        let service = self.service_for_tab(tab_id);
        let repo = Repository::open(repo_path)?;
        let base_options = WorkflowRunOptions {
            default_remote: self.current_remote().unwrap_or_else(|| "origin".into()),
            input_vars: BTreeMap::new(),
            source_branch: source_branch.cloned(),
        };
        for (key, input) in &definition.inputs {
            let value = match input.default.as_ref() {
                Some(default) => WorkflowExecutor::new(&service).resolve_template(
                    &repo,
                    definition,
                    &base_options,
                    default,
                )?,
                None => String::new(),
            };
            fields.push(WorkflowInputFieldState::new(key.clone(), input, value, cx));
        }
        Ok(fields)
    }

    fn workflow_input_values(&self) -> BTreeMap<String, String> {
        self.workflow_state
            .inputs
            .iter()
            .map(|input| (input.key.clone(), input.field.value.clone()))
            .collect()
    }

    fn workflow_run_options(&self) -> WorkflowRunOptions {
        WorkflowRunOptions {
            default_remote: self.current_remote().unwrap_or_else(|| "origin".into()),
            input_vars: self.workflow_input_values(),
            source_branch: self.workflow_state.source_branch.clone(),
        }
    }

    pub(crate) fn refresh_workflow_preview(&mut self) {
        let Some(definition) = self.workflow_state.definition.as_ref() else {
            self.workflow_state.preview = None;
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.workflow_state.preview = None;
            return;
        };
        let Some(tab_id) = self.active_tab_id() else {
            self.workflow_state.preview = None;
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        match self.preview_workflow(&repo_path, definition, self.workflow_run_options(), tab_id) {
            Ok(preview) => {
                self.workflow_state.preview = Some(preview);
                self.last_error = None;
            }
            Err(err) => {
                self.workflow_state.preview = None;
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn preview_workflow(
        &self,
        repo_path: &std::path::Path,
        definition: &WorkflowDefinition,
        options: WorkflowRunOptions,
        tab_id: crate::RepoTabId,
    ) -> khaslana::Result<WorkflowPreview> {
        let service = self.service_for_tab(tab_id);
        let repo = Repository::open(repo_path)?;
        let data_dir = khaslana::storage::active_data_dir();
        let config = if let Some(data_dir) = &data_dir {
            load_mcp_config(data_dir)?
        } else {
            Default::default()
        };
        let mut registry = khaslana::WorkflowActionRegistry::default();
        register_external_actions_with_ai(&mut registry, config, None,
            Some(self.ai_settings.clone()), None, data_dir.as_deref())?;
        WorkflowExecutor::with_actions(&service, &registry).preview(&repo, definition, &options)
    }

    fn render_workflow_inputs(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.workflow_state.inputs.is_empty() {
            return div().into_any_element();
        }
        let handle = self.scroll_handle(WORKFLOW_INPUT_LIST_SCROLL_ID);
        // 外层是受限高度的 flex 列，scrollable_frame_when 为其直接子项；
        // 内容节点建立列宽基准并只负责滚动，不能再声明 flex_1/min_h 以免丢失边界。
        let content = div()
            .id(WORKFLOW_INPUT_LIST_SCROLL_ID)
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&handle)
            .children(
                self.workflow_state
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(index, input)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ui_theme::SPACE_1))
                            .pb(px(ui_theme::SPACE_2))
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER_MUTED))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(ui_theme::SPACE_1))
                                    .text_size(px(12.0))
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(input.label.clone())
                                    .when(input.required, |this| {
                                        this.child(
                                            div().text_color(rgb(ui_theme::DESTRUCTIVE)).child("*"),
                                        )
                                    }),
                            )
                            .child(self.input(FieldId::WorkflowInput(index), false, window, cx))
                            .when_some(input.description.clone(), |this, description| {
                                this.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                        .child(description),
                                )
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(ui_theme::SPACE_3))
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("运行配置"),
            )
            .child(scrollable_frame_when(
                WORKFLOW_INPUT_LIST_SCROLL_ID,
                ScrollbarMode::Vertical,
                content,
                handle,
                true,
                cx,
            ))
            .into_any_element()
    }

    fn render_workflow_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = self.workflow_state.preview.as_ref();
        let steps = preview
            .map(|preview| preview.steps.as_slice())
            .unwrap_or(&[]);
        let content_present = !steps.is_empty();
        let rows = if steps.is_empty() {
            vec![
                workflow_empty_line(if self.workflow_state.definition.is_some() {
                    "暂无可展示的步骤"
                } else {
                    "选择模板后生成步骤预览"
                })
                .into_any_element(),
            ]
        } else {
            steps
                .iter()
                .enumerate()
                .map(|(position, step)| {
                    let details = step
                        .details
                        .iter()
                        .map(|detail| {
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .truncate()
                                .child(detail.clone())
                        })
                        .collect::<Vec<_>>();
                    div()
                        .flex()
                        .flex_none()
                        .gap(px(ui_theme::SPACE_3))
                        .min_h(px(62.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .flex_none()
                                .w(px(24.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(22.0))
                                        .rounded_full()
                                        .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                                        .text_size(px(11.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child(format!("{}", step.index + 1)),
                                )
                                .when(position + 1 < steps.len(), |this| {
                                    this.child(
                                        div()
                                            .flex_1()
                                            .w(px(1.0))
                                            .mt(px(ui_theme::SPACE_1))
                                            .bg(rgb(ui_theme::BORDER_MUTED)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.0))
                                .gap(px(ui_theme::SPACE_1))
                                .pb(px(ui_theme::SPACE_3))
                                .border_b_1()
                                .border_color(rgb(ui_theme::BORDER_MUTED))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(ui_theme::SPACE_2))
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(rgb(ui_theme::PRIMARY))
                                                .child(step.op),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                                .truncate()
                                                .child(step.summary.clone()),
                                        ),
                                )
                                .children(details),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(ui_theme::SPACE_2))
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("步骤时间线"),
            )
            .child({
                let handle = self.scroll_handle(WORKFLOW_PREVIEW_LIST_SCROLL_ID);
                // 预览内容本身不占据父级剩余空间，由直接包裹它的滚动框提供有界视口。
                let content = div()
                    .id(WORKFLOW_PREVIEW_LIST_SCROLL_ID)
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    WORKFLOW_PREVIEW_LIST_SCROLL_ID,
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    content_present,
                    cx,
                )
            })
    }

    fn render_workflow_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let console = workflow_console_state(self.busy, self.workflow_state.log.len());
        let rows = self
            .workflow_state
            .log
            .iter()
            .map(|entry| {
                let details = entry
                    .details
                    .iter()
                    .map(|detail| {
                        div()
                            .pl(px(ui_theme::SPACE_3))
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(detail.clone())
                    })
                    .collect::<Vec<_>>();
                div()
                    .flex_none()
                    .pb(px(ui_theme::SPACE_1))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(entry.message.clone())
                    .children(details)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty_message = if self.busy {
            self.status.clone()
        } else {
            "运行后在此查看状态与日志".to_string()
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .when(console.is_expanded(), |this| {
                this.max_h(px(200.0)).min_h(px(160.0))
            })
            .when(!console.is_expanded(), |this| this.min_h(px(42.0)))
            .gap(px(ui_theme::SPACE_2))
            .pt(px(ui_theme::SPACE_2))
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_none()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("Console"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(if self.busy {
                                ui_theme::PRIMARY
                            } else if self.last_error.is_some() {
                                ui_theme::DESTRUCTIVE
                            } else {
                                ui_theme::CONTENT_SECONDARY
                            }))
                            .truncate()
                            .child(if self.busy {
                                self.status.clone()
                            } else if self.workflow_state.log.is_empty() {
                                "空闲".to_string()
                            } else {
                                format!("{} 条记录", self.workflow_state.log.len())
                            }),
                    ),
            )
            .when(console.is_expanded(), |this| {
                this.child({
                    let content = if rows.is_empty() {
                        vec![
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(empty_message)
                                .into_any_element(),
                        ]
                    } else {
                        rows
                    };
                    let handle = self.scroll_handle(WORKFLOW_LOG_LIST_SCROLL_ID);
                    // Console 的展开高度由父容器锁定；内容节点只挂滚动，不参与高度分配。
                    let content = div()
                        .id(WORKFLOW_LOG_LIST_SCROLL_ID)
                        .overflow_y_scroll()
                        .track_scroll(&handle)
                        .children(content)
                        .into_any_element();
                    scrollable_frame_when(
                        WORKFLOW_LOG_LIST_SCROLL_ID,
                        ScrollbarMode::Vertical,
                        content,
                        handle,
                        true,
                        cx,
                    )
                })
            })
    }
}

fn workflow_progress_entry(event: &WorkflowProgressEvent) -> WorkflowLogEntry {
    match event {
        WorkflowProgressEvent::Started { name, total } => WorkflowLogEntry {
            message: format!("开始运行工作流“{name}”（{total} 步）"),
            details: Vec::new(),
        },
        WorkflowProgressEvent::StepStarted {
            index,
            total,
            label,
            details,
        } => WorkflowLogEntry {
            message: format!("步骤 {}/{}：{label}", index + 1, total),
            details: details.clone(),
        },
        WorkflowProgressEvent::StepFinished {
            index,
            total,
            label,
            details,
        } => WorkflowLogEntry {
            message: format!("步骤 {}/{} 完成：{label}", index + 1, total),
            details: details.clone(),
        },
        WorkflowProgressEvent::StepDetail { index, total, label, detail } => WorkflowLogEntry {
            message: format!("步骤 {}/{}：{label}", index + 1, total),
            details: vec![detail.clone()],
        },
        WorkflowProgressEvent::Finished { name, total } => WorkflowLogEntry {
            message: format!("工作流“{name}”已完成（{total} 步）"),
            details: Vec::new(),
        },
    }
}

/// 工作流模板目录，跟随实际激活的数据目录（与 DB / ai-reviews 同源，
/// 经 `active_data_dir` 解析）下的 `workflows/` 子目录。不能直接用
/// `portable_database_dir`：exe 位于危险目录时数据不落在 exe 旁。
pub(crate) fn workflow_templates_dir() -> Option<PathBuf> {
    khaslana::storage::active_data_dir().map(|dir| dir.join("workflows"))
}

/// 旧版工作流模板目录（`~/.khaslana/workflows`），仅用于一次性便携迁移的来源。
fn legacy_workflow_templates_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| workflow_templates_dir_from_home(dirs.home_dir()))
}

fn workflow_templates_dir_from_home(home: &Path) -> PathBuf {
    home.join(".khaslana").join("workflows")
}

fn ensure_workflow_templates_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

fn load_workflow_templates() -> Result<Vec<WorkflowTemplateItem>, String> {
    let dir = workflow_templates_dir().ok_or_else(|| "无法定位工作流模板目录".to_string())?;
    let first_use = !dir.exists();
    ensure_workflow_templates_dir(&dir).map_err(|err| format!("工作流模板目录创建失败：{err}"))?;
    // 只在目录初次创建时迁移旧模板，避免用户删除全部模板后刷新又被拷回来。
    if first_use {
        migrate_legacy_workflow_templates(&dir);
    }
    import_managed_workflow_templates_once(&dir)
        .map_err(|err| format!("旧版远端模板导入失败：{err}"))?;
    load_workflow_templates_from_dir(&dir)
}

/// 把旧版批量同步的模板导入普通目录一次。保留旧文件作为备份，之后删掉
/// 普通目录里的模板也不会从备份重新出现。
fn import_managed_workflow_templates_once(dir: &Path) -> std::io::Result<()> {
    let marker = dir.join(".remote-cnb-imported");
    if marker.exists() {
        return Ok(());
    }
    let managed = dir.join(MANAGED_TEMPLATE_DIR);
    if managed.is_dir() {
        for entry in fs::read_dir(&managed)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() || !is_workflow_template_path(&path) {
                continue;
            }
            let target = dir.join(entry.file_name());
            if !target.exists() {
                fs::copy(path, target)?;
            }
        }
    }
    fs::write(marker, [])
}

/// 若便携工作流目录为空且旧目录存在模板文件，递归拷贝一次。
/// 幂等：便携目录已有模板则跳过，且同名文件不覆盖。
fn migrate_legacy_workflow_templates(portable_dir: &Path) {
    if dir_has_workflow_template(portable_dir) {
        return;
    }
    let Some(legacy_dir) = legacy_workflow_templates_dir() else {
        return;
    };
    if !legacy_dir.exists() || !dir_has_workflow_template(&legacy_dir) {
        return;
    }
    let _ = copy_workflow_templates(&legacy_dir, portable_dir);
}

fn dir_has_workflow_template(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if is_workflow_template_path(&entry.path())
            && entry.metadata().map(|m| m.is_file()).unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// 递归拷贝工作流模板文件（仅 `.json5`/`.jsonc`），同名文件跳过不覆盖。
fn copy_workflow_templates(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let dest = dst.join(entry.file_name());
        if metadata.is_dir() {
            copy_workflow_templates(&path, &dest)?;
        } else if is_workflow_template_path(&path) && !dest.exists() {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

fn load_workflow_templates_from_dir(dir: &Path) -> Result<Vec<WorkflowTemplateItem>, String> {
    let mut templates = Vec::new();
    let entries = fs::read_dir(dir)
        .map_err(|err| format!("工作流模板目录读取失败：{err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("工作流模板目录读取失败：{err}"))?;
        let path = entry.path();
        if !is_workflow_template_path(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        templates.push(workflow_template_item(path, metadata.modified().ok()));
    }

    templates.sort_by(|left, right| {
        left.file_name
            .to_lowercase()
            .cmp(&right.file_name.to_lowercase())
    });
    Ok(templates)
}

fn workflow_template_item(path: PathBuf, modified: Option<SystemTime>) -> WorkflowTemplateItem {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string());
    let fallback_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| file_name.clone());

    match fs::read_to_string(&path) {
        Ok(content) => match parse_workflow_json5(&content) {
            Ok(definition) => WorkflowTemplateItem {
                path,
                display_name: definition.display_name(),
                file_name,
                modified_label: workflow_modified_label(modified),
                error: None,
            },
            Err(err) => WorkflowTemplateItem {
                path,
                display_name: fallback_name,
                file_name,
                modified_label: workflow_modified_label(modified),
                error: Some(err.to_string()),
            },
        },
        Err(err) => WorkflowTemplateItem {
            path,
            display_name: fallback_name,
            file_name,
            modified_label: workflow_modified_label(modified),
            error: Some(format!("读取失败：{err}")),
        },
    }
}

pub(crate) fn is_managed_workflow_template_path(path: &Path) -> bool {
    workflow_templates_dir()
        .map(|root| root.join(MANAGED_TEMPLATE_DIR))
        .as_deref()
        == path.parent()
}

fn is_workflow_template_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("json5") || extension.eq_ignore_ascii_case("jsonc")
        })
}

fn workflow_modified_label(modified: Option<SystemTime>) -> String {
    modified
        .map(|time| {
            let local: DateTime<Local> = time.into();
            format!("修改于 {}", local.format("%Y-%m-%d %H:%M"))
        })
        .unwrap_or_else(|| "修改时间未知".to_string())
}

#[cfg(test)]
#[path = "tests/workflow_view.rs"]
mod tests;
