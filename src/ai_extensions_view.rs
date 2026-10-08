//! 设置中心的 Skill、MCP 与浏览器运行环境配置。

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use gpui::{Context, Div, FontWeight, SharedString, Window, div, prelude::*, px};
use gpui_kit::component::button::Button;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::{Disableable, IconName, Sizable, h_flex, v_flex};
use khaslana::workflow::browser_runtime::{self, BrowserRuntimeInfo};
use khaslana::workflow::extensions::{
    self, WorkflowMcpConfig, WorkflowMcpServer, WorkflowMcpTool, WorkflowSkillPreview,
    WorkflowToolAccess,
};

use crate::{DialogState, FieldId, RepositoryView, ScrollbarMode, SettingsCategory,
    TextFieldState, UiEvent, dialog_actions, dialog_panel_size, scrollable_frame_when,
    send_ui_event, system::open_directory, tasks::{TaskKind, panic_message},
    ui::theme::{self as ui_theme, rgb}};
use crate::ui::components::{SettingsButtonTone, settings_command_button};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AiSettingsTab {
    #[default]
    Connection,
    Skill,
    Mcp,
    Runtime,
}

impl AiSettingsTab {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Connection => 0,
            Self::Skill => 1,
            Self::Mcp => 2,
            Self::Runtime => 3,
        }
    }

    pub(crate) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Skill,
            2 => Self::Mcp,
            3 => Self::Runtime,
            _ => Self::Connection,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct McpToolDraft {
    pub(crate) name: String,
    pub(crate) allowed: bool,
    pub(crate) access: WorkflowToolAccess,
}

fn discovered_mcp_tools(names: Vec<String>, previous: &[McpToolDraft],
    new_server: bool) -> Vec<McpToolDraft> {
    let previous: BTreeMap<_, _> = previous.iter()
        .map(|tool| (tool.name.as_str(), (tool.allowed, tool.access))).collect();
    names.into_iter().map(|name| {
        // 新服务开箱即用；旧配置的白名单与本次手动关闭的工具不能被重测覆盖。
        let (allowed, access) = previous.get(name.as_str()).copied()
            .unwrap_or((new_server, WorkflowToolAccess::Write));
        McpToolDraft { name, allowed, access }
    }).collect()
}

pub(crate) struct AiExtensionsUiState {
    pub(crate) connection_test: crate::ai_settings_page::state::AiConnectionTestState,
    pub(crate) save_error: Option<String>,
    pub(crate) tab: AiSettingsTab,
    pub(crate) skills: Vec<(String, String)>,
    pub(crate) mcp: WorkflowMcpConfig,
    pub(crate) runtime: Option<BrowserRuntimeInfo>,
    pub(crate) loading: bool,
    pub(crate) load_request_id: u64,
    pub(crate) error: Option<String>,
    pub(crate) skill_preview: Option<WorkflowSkillPreview>,
    pub(crate) skill_preview_request_id: u64,
    pub(crate) skill_picker_open: bool,
    pub(crate) edit_mcp_name: Option<String>,
    pub(crate) mcp_id: TextFieldState,
    pub(crate) mcp_command: TextFieldState,
    pub(crate) mcp_args: TextFieldState,
    pub(crate) mcp_tools: Vec<McpToolDraft>,
    pub(crate) mcp_tools_expanded: bool,
    pub(crate) mcp_auto_discover: bool,
    pub(crate) mcp_enabled: bool,
    pub(crate) mcp_verified: Option<(String, Vec<String>)>,
    pub(crate) mcp_testing: bool,
    pub(crate) mcp_test_request_id: u64,
    pub(crate) action_busy: bool,
    pub(crate) action_request_id: u64,
}

impl AiExtensionsUiState {
    pub(crate) fn new(cx: &mut Context<RepositoryView>) -> Self {
        Self {
            connection_test: Default::default(),
            save_error: None,
            tab: AiSettingsTab::Connection,
            skills: Vec::new(),
            mcp: WorkflowMcpConfig::default(),
            runtime: None,
            loading: false,
            load_request_id: 0,
            error: None,
            skill_preview: None,
            skill_preview_request_id: 0,
            skill_picker_open: false,
            edit_mcp_name: None,
            mcp_id: TextFieldState::new(cx, "例如 my-local-service"),
            mcp_command: TextFieldState::new(cx, "例如 npx、uvx 或服务程序完整路径"),
            mcp_args: TextFieldState::new(cx, "每行一个启动参数"),
            mcp_tools: Vec::new(),
            mcp_tools_expanded: false,
            mcp_auto_discover: true,
            mcp_enabled: true,
            mcp_verified: None,
            mcp_testing: false,
            mcp_test_request_id: 0,
            action_busy: false,
            action_request_id: 0,
        }
    }
}

fn guarded<T>(stage: &'static str, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(action))
        .unwrap_or_else(|payload| Err(format!("{stage}异常：{}", panic_message(payload))))
}

fn primary_text(text: impl Into<SharedString>) -> Div {
    div().text_size(px(ui_theme::TYPE_BODY))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(ui_theme::CONTENT_PRIMARY)).child(text.into())
}

fn meta_text(text: impl Into<SharedString>) -> Div {
    div().text_size(px(ui_theme::TYPE_META))
        .text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(text.into())
}

fn error_text(text: impl Into<SharedString>) -> Div {
    div().text_size(px(ui_theme::TYPE_META))
        .text_color(rgb(ui_theme::DESTRUCTIVE)).child(text.into())
}

fn settings_row(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    h_flex().w_full().min_w(px(0.0)).items_center().justify_between().gap_3()
        .px_3().py_3().rounded(px(ui_theme::RADIUS_XS))
        .border_1().border_color(rgb(ui_theme::BORDER_MUTED))
        .bg(rgb(ui_theme::SURFACE_BASE))
        .child(v_flex().flex_1().min_w(px(0.0)).gap_1()
            .child(primary_text(title).truncate())
            .child(meta_text(description).truncate()))
}

impl RepositoryView {
    pub(crate) fn refresh_ai_extensions(&mut self) {
        self.load_ai_extensions(false);
    }

    pub(crate) fn inspect_ai_runtime(&mut self) {
        self.load_ai_extensions(true);
    }

    fn load_ai_extensions(&mut self, inspect_runtime: bool) {
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.ai_extensions.error = Some("无法定位应用数据目录".into());
            return;
        };
        self.ai_extensions.load_request_id = self.ai_extensions.load_request_id.wrapping_add(1);
        let request_id = self.ai_extensions.load_request_id;
        self.ai_extensions.loading = true;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::External, move || {
            let result = guarded("读取 AI 扩展设置", || {
                let skills = extensions::discover_skill_packages(&data_dir)
                    .map_err(|err| err.to_string())?;
                let mcp = extensions::load_user_mcp_config(&data_dir)
                    .map_err(|err| err.to_string())?;
                let runtime = if inspect_runtime { browser_runtime::inspect(&data_dir) }
                    else { browser_runtime::inspect_cached(&data_dir) };
                Ok((skills, mcp, runtime))
            });
            send_ui_event(&tx, UiEvent::AiExtensionsLoaded { request_id, result });
        });
    }

    pub(crate) fn apply_ai_extensions_loaded(&mut self, request_id: u64,
        result: Result<(Vec<(String, String)>, WorkflowMcpConfig, BrowserRuntimeInfo), String>) {
        if request_id != self.ai_extensions.load_request_id { return; }
        self.ai_extensions.loading = false;
        match result {
            Ok((skills, mcp, runtime)) => {
                self.ai_extensions.skills = skills;
                self.ai_extensions.mcp = mcp;
                self.ai_extensions.runtime = Some(runtime);
                self.ai_extensions.error = None;
            }
            Err(error) => self.ai_extensions.error = Some(error),
        }
    }

    pub(crate) fn select_ai_settings_tab(&mut self, tab: AiSettingsTab) {
        if tab != AiSettingsTab::Connection && !self.ai_settings.enabled { return; }
        self.ai_extensions.tab = tab;
        if tab == AiSettingsTab::Runtime { self.refresh_ai_extensions(); }
    }

    fn can_edit_ai_extensions(&self) -> bool {
        self.settings_center == Some(SettingsCategory::Ai) && self.ai_settings.enabled
    }

    pub(crate) fn open_ai_skill_folder(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_ai_extensions() { return; }
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.notify_error("无法定位应用数据目录", cx);
            return;
        };
        let folder = data_dir.join("workflow-skills");
        if let Err(error) = std::fs::create_dir_all(&folder).and_then(|()| open_directory(&folder)) {
            self.notify_error(format!("打开 Skill 目录失败：{error}"), cx);
        }
    }

    pub(crate) fn open_ai_skill_import(&mut self) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy
            || self.ai_extensions.skill_picker_open { return; }
        self.ai_extensions.skill_preview = None;
        self.ai_extensions.skill_preview_request_id =
            self.ai_extensions.skill_preview_request_id.wrapping_add(1);
        self.ai_extensions.error = None;
        self.ai_extensions.action_request_id = self.ai_extensions.action_request_id.wrapping_add(1);
        self.active_dialog = Some(DialogState::AiSkillImport);
    }

    pub(crate) fn pick_ai_skill_folder(&mut self) {
        if self.active_dialog != Some(DialogState::AiSkillImport)
            || self.ai_extensions.action_busy || self.ai_extensions.skill_picker_open { return; }
        self.ai_extensions.skill_preview_request_id =
            self.ai_extensions.skill_preview_request_id.wrapping_add(1);
        let request_id = self.ai_extensions.skill_preview_request_id;
        self.ai_extensions.skill_picker_open = true;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::External, move || {
            let result = guarded("预览 Skill", || {
                let folder = rfd::FileDialog::new().set_title("选择包含 SKILL.md 的文件夹")
                    .pick_folder();
                folder.map(|folder| extensions::preview_skill_folder(&folder)
                    .map_err(|err| err.to_string())).transpose()
            });
            send_ui_event(&tx, UiEvent::AiSkillPreviewed { request_id, result });
        });
    }

    pub(crate) fn apply_ai_skill_preview(&mut self, request_id: u64,
        result: Result<Option<WorkflowSkillPreview>, String>) {
        if request_id != self.ai_extensions.skill_preview_request_id { return; }
        self.ai_extensions.skill_picker_open = false;
        if self.active_dialog != Some(DialogState::AiSkillImport) { return; }
        match result {
            Ok(Some(preview)) => {
                self.ai_extensions.skill_preview = Some(preview);
                self.ai_extensions.error = None;
            }
            Ok(None) => {}
            Err(error) => self.ai_extensions.error = Some(error),
        }
    }

    pub(crate) fn install_ai_skill(&mut self) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy
            || self.ai_extensions.skill_picker_open
            || self.active_dialog != Some(DialogState::AiSkillImport) { return; }
        let Some(preview) = self.ai_extensions.skill_preview.as_ref() else { return; };
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.ai_extensions.error = Some("无法定位应用数据目录".into());
            return;
        };
        let source = preview.source.clone();
        let expected_sha256 = preview.content_sha256.clone();
        self.start_ai_extension_change("安装 Skill", move || {
            extensions::install_skill_folder(&data_dir, &source, &expected_sha256)
                .map(|preview| format!("Skill {} 已安装", preview.name))
                .map_err(|err| err.to_string())
        });
    }

    pub(crate) fn open_ai_skill_details(&mut self, name: String) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy { return; }
        self.active_dialog = Some(DialogState::AiSkillDetails { name });
    }

    pub(crate) fn remove_ai_skill(&mut self, name: String) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy { return; }
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.ai_extensions.error = Some("无法定位应用数据目录".into());
            return;
        };
        self.start_ai_extension_change("移除 Skill", move || {
            extensions::remove_skill_package(&data_dir, &name)
                .map(|()| format!("Skill {name} 已移除"))
                .map_err(|err| err.to_string())
        });
    }

    pub(crate) fn open_ai_mcp_form(&mut self, name: Option<String>) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy { return; }
        let existing = name.as_ref().and_then(|name| self.ai_extensions.mcp.servers.get(name));
        self.ai_extensions.mcp_id.set_value(name.clone().unwrap_or_default());
        self.ai_extensions.mcp_command.set_value(existing.map(|server| server.command.clone()).unwrap_or_default());
        self.ai_extensions.mcp_args.set_value(existing.map(|server| server.args.join("\n")).unwrap_or_default());
        self.ai_extensions.mcp_tools = existing.map(|server| server.tools.iter()
            .map(|(name, tool)| McpToolDraft { name: name.clone(), allowed: true,
                access: tool.access }).collect()).unwrap_or_default();
        self.ai_extensions.mcp_auto_discover = existing.is_none_or(|server| server.auto_discover);
        self.ai_extensions.mcp_enabled = existing.is_none_or(|server| server.enabled);
        if let Some(server) = existing {
            let names = server.cached_tools.iter().cloned()
                .chain(server.tools.keys().cloned()).collect::<std::collections::BTreeSet<_>>();
            self.ai_extensions.mcp_tools = discovered_mcp_tools(names.into_iter().collect(),
                &self.ai_extensions.mcp_tools, server.auto_discover);
        }
        self.ai_extensions.edit_mcp_name = name;
        self.ai_extensions.mcp_tools_expanded = false;
        self.ai_extensions.mcp_verified = None;
        self.ai_extensions.error = None;
        self.ai_extensions.mcp_testing = false;
        self.ai_extensions.mcp_test_request_id = self.ai_extensions.mcp_test_request_id.wrapping_add(1);
        self.ai_extensions.action_request_id = self.ai_extensions.action_request_id.wrapping_add(1);
        self.active_dialog = Some(DialogState::AiMcpForm);
    }

    fn ai_mcp_args(&self) -> Vec<String> {
        self.ai_extensions.mcp_args.value.lines().map(str::trim)
            .filter(|value| !value.is_empty()).map(str::to_owned).collect()
    }

    pub(crate) fn test_ai_mcp_form(&mut self) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.mcp_testing
            || self.active_dialog != Some(DialogState::AiMcpForm) { return; }
        let command = self.ai_extensions.mcp_command.value.trim().to_owned();
        let args = self.ai_mcp_args();
        self.ai_extensions.mcp_test_request_id = self.ai_extensions.mcp_test_request_id.wrapping_add(1);
        let request_id = self.ai_extensions.mcp_test_request_id;
        self.ai_extensions.mcp_testing = true;
        self.ai_extensions.mcp_verified = None;
        self.ai_extensions.error = None;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::External, move || {
            let result = guarded("测试 MCP 服务", ||
                extensions::inspect_mcp_server_tools(&command, &args).map_err(|err| err.to_string()));
            send_ui_event(&tx, UiEvent::AiMcpToolsInspected { request_id, command, args, result });
        });
    }

    pub(crate) fn apply_ai_mcp_tools(&mut self, request_id: u64, command: String,
        args: Vec<String>, result: Result<Vec<String>, String>) {
        if request_id != self.ai_extensions.mcp_test_request_id
            || self.active_dialog != Some(DialogState::AiMcpForm) { return; }
        self.ai_extensions.mcp_testing = false;
        if command != self.ai_extensions.mcp_command.value.trim() || args != self.ai_mcp_args() {
            self.ai_extensions.error = Some("启动命令或参数已更改，请重新测试连接".into());
            return;
        }
        match result {
            Ok(names) => {
                self.ai_extensions.mcp_tools = discovered_mcp_tools(names,
                    &self.ai_extensions.mcp_tools, self.ai_extensions.mcp_auto_discover);
                self.ai_extensions.mcp_verified = Some((command, args));
                self.ai_extensions.error = None;
            }
            Err(error) => self.ai_extensions.error = Some(error),
        }
    }

    pub(crate) fn save_ai_mcp_form(&mut self) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy
            || self.ai_extensions.mcp_testing || self.active_dialog != Some(DialogState::AiMcpForm) { return; }
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.ai_extensions.error = Some("无法定位应用数据目录".into());
            return;
        };
        let name = self.ai_extensions.mcp_id.value.trim().to_owned();
        let command = self.ai_extensions.mcp_command.value.trim().to_owned();
        let args = self.ai_mcp_args();
        let verified = self.ai_extensions.mcp_verified.as_ref() == Some(&(command.clone(), args.clone()));
        let cached_tools = if verified {
            self.ai_extensions.mcp_tools.iter().map(|tool| tool.name.clone()).collect()
        } else {
            self.ai_extensions.edit_mcp_name.as_ref()
                .and_then(|name| self.ai_extensions.mcp.servers.get(name))
                .filter(|server| server.command == command && server.args == args)
                .map(|server| server.cached_tools.clone()).unwrap_or_default()
        };
        let server = WorkflowMcpServer {
            command,
            args,
            tools: self.ai_extensions.mcp_tools.iter().filter(|tool| tool.allowed)
                .map(|tool| (tool.name.clone(), WorkflowMcpTool { access: tool.access }))
                .collect(),
            enabled: self.ai_extensions.mcp_enabled,
            auto_discover: self.ai_extensions.mcp_auto_discover,
            cached_tools,
        };
        let previous_name = self.ai_extensions.edit_mcp_name.clone();
        self.start_ai_extension_change("保存 MCP 服务", move || {
            extensions::upsert_user_mcp_server(&data_dir, previous_name.as_deref(), &name, server)
                .map(|()| format!("MCP 服务 {name} 已保存"))
                .map_err(|err| err.to_string())
        });
    }

    pub(crate) fn remove_ai_mcp_server(&mut self, name: String) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy { return; }
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            self.ai_extensions.error = Some("无法定位应用数据目录".into());
            return;
        };
        self.start_ai_extension_change("移除 MCP 服务", move || {
            extensions::remove_user_mcp_server(&data_dir, &name)
                .map(|()| format!("MCP 服务 {name} 已移除"))
                .map_err(|err| err.to_string())
        });
    }

    pub(crate) fn set_ai_mcp_server_enabled(&mut self, name: String, enabled: bool) {
        if !self.can_edit_ai_extensions() || self.ai_extensions.action_busy { return; }
        let Some(data_dir) = khaslana::storage::active_data_dir() else { return; };
        self.start_ai_extension_change("更新 MCP 服务状态", move || {
            extensions::set_user_mcp_server_enabled(&data_dir, &name, enabled)
                .map(|()| format!("MCP 服务 {name} 已{}", if enabled { "启用" } else { "禁用" }))
                .map_err(|err| err.to_string())
        });
    }

    fn start_ai_extension_change(&mut self, stage: &'static str,
        action: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.ai_extensions.action_request_id = self.ai_extensions.action_request_id.wrapping_add(1);
        let request_id = self.ai_extensions.action_request_id;
        self.ai_extensions.action_busy = true;
        self.ai_extensions.error = None;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::External, move || {
            let result = guarded(stage, action);
            send_ui_event(&tx, UiEvent::AiExtensionChanged { request_id, result });
        });
    }

    pub(crate) fn apply_ai_extension_change(&mut self, request_id: u64,
        result: Result<String, String>, cx: &mut Context<Self>) {
        if request_id != self.ai_extensions.action_request_id { return; }
        self.ai_extensions.action_busy = false;
        match result {
            Ok(message) => {
                if matches!(self.active_dialog, Some(DialogState::AiSkillImport
                    | DialogState::AiSkillDetails { .. } | DialogState::AiMcpForm
                    | DialogState::AiMcpRemove { .. } | DialogState::AiSkillRemove { .. })) {
                    self.close_dialog();
                }
                self.notify_success(message, cx);
                self.refresh_ai_extensions();
            }
            Err(error) => {
                self.ai_extensions.error = Some(error.clone());
                self.notify_error(error, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn start_ai_runtime_download(&mut self) {
        if !self.can_edit_ai_extensions() { return; }
        self.start_browser_runtime_download_for_settings();
    }
}

#[cfg(test)]
#[path = "tests/ai_extensions_view.rs"]
mod tests;

impl RepositoryView {
    pub(crate) fn render_ai_skill_import_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = self.ai_extensions.skill_preview.as_ref();
        let busy = self.ai_extensions.action_busy;
        let picking = self.ai_extensions.skill_picker_open;
        self.dialog_panel("安装本地 Skill", cx)
            .w(px(570.0))
            .child(meta_text("从文件夹读取 SKILL.md，确认内容后导入应用数据目录。"))
            .child(h_flex().w_full().justify_between().gap_2().items_center()
                .child(meta_text(preview.map(|item| item.source.display().to_string())
                    .unwrap_or_else(|| "尚未选择文件夹".into())).flex_1().min_w(px(0.0)).truncate())
                .child(self.button(if picking { "正在选择…" } else { "选择文件夹" }, !busy && !picking,
                    |this, _, _| this.pick_ai_skill_folder(), cx)))
            .when_some(preview, |this, preview| {
                this.child(v_flex().w_full().gap_2().p_3()
                    .rounded(px(ui_theme::RADIUS_XS)).border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(primary_text(&preview.name))
                    .child(meta_text(&preview.description))
                    .child(meta_text(format!("文件：{}", preview.files.join(" · ")))))
            })
            .when_some(self.ai_extensions.error.as_ref(), |this, error| {
                this.child(error_text(error.clone()))
            })
            .child(meta_text("导入后可查看和移除；运行 Skill 时仍需确认工具权限。"))
            .child(dialog_actions()
                .child(self.button("取消", !busy, |this, _, _| this.close_dialog(), cx))
                .child(self.primary_button("确认安装", preview.is_some() && !busy && !picking,
                    |this, _, _| this.install_ai_skill(), cx)))
    }

    pub(crate) fn render_ai_skill_details_dialog(&self, name: &str,
        cx: &mut Context<Self>) -> impl IntoElement {
        let description = self.ai_extensions.skills.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, description)| description.clone()).unwrap_or_default();
        let folder = khaslana::storage::active_data_dir()
            .map(|path| path.join("workflow-skills").join(name).display().to_string())
            .unwrap_or_else(|| "无法定位应用数据目录".into());
        let name_for_remove = name.to_owned();
        self.dialog_panel("Skill 详情", cx).w(px(560.0))
            .child(primary_text(name.to_owned()))
            .child(meta_text(description))
            .child(meta_text(folder))
            .child(meta_text("工作流调用前会展示所需 MCP 工具并请求确认。"))
            .child(dialog_actions()
                .child(self.button("关闭", true, |this, _, _| this.close_dialog(), cx))
                .child(self.danger_button("移除 Skill", !self.ai_extensions.action_busy,
                    move |this, _, cx| {
                        this.active_dialog = Some(DialogState::AiSkillRemove {
                            name: name_for_remove.clone() });
                        cx.notify();
                    }, cx)))
    }

    pub(crate) fn render_ai_skill_remove_dialog(&self, name: &str,
        cx: &mut Context<Self>) -> impl IntoElement {
        let name_for_remove = name.to_owned();
        self.dialog_panel("移除 Skill", cx)
            .child(primary_text(format!("确定移除 {name}？")))
            .child(meta_text("将删除应用数据目录中的该 Skill 文件夹。"))
            .when_some(self.ai_extensions.error.as_ref(), |this, error| {
                this.child(error_text(error.clone()))
            })
            .child(dialog_actions()
                .child(self.button("取消", !self.ai_extensions.action_busy,
                    |this, _, _| this.close_dialog(), cx))
                .child(self.danger_button("确认移除", !self.ai_extensions.action_busy,
                    move |this, _, _| this.remove_ai_skill(name_for_remove.clone()), cx)))
    }

    pub(crate) fn render_ai_mcp_form_dialog(&self, window: &Window,
        cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.ai_extensions.edit_mcp_name.is_some();
        let busy = self.ai_extensions.action_busy;
        let testing = self.ai_extensions.mcp_testing;
        let verified = self.ai_extensions.mcp_verified.as_ref().is_some_and(|(command, args)|
            command == self.ai_extensions.mcp_command.value.trim()
                && *args == self.ai_mcp_args());
        let selected_tools = self.ai_extensions.mcp_tools.iter().any(|tool| tool.allowed);
        let enabled_count = self.ai_extensions.mcp_tools.iter().filter(|tool| tool.allowed).count();
        let (panel_width, panel_height) = dialog_panel_size(window, 590.0, 620.0);
        let handle = self.scroll_handle("ai-mcp-form-scroll");
        let mut body = v_flex().w_full().gap_2()
            .child(meta_text("填写命令和参数即可保存；首次使用时连接。测试连接只读取工具列表。"))
            .child(primary_text("服务 ID"))
            .child(self.input(FieldId::AiMcpServerId, false, window, cx))
            .child(primary_text("启动命令"))
            .child(self.input(FieldId::AiMcpCommand, false, window, cx))
            .child(meta_text("可直接填写 npx，无需添加 .cmd；每个参数单独一行，无需加引号。"))
            .child(primary_text("启动参数（每行一个）"))
            .child(self.input(FieldId::AiMcpArgs, false, window, cx))
            .child(h_flex().w_full().justify_between().items_center()
                .child(meta_text(if verified { "本次连接测试成功" } else { "尚未验证当前配置，可直接保存" }))
                .child(self.button("测试连接", !busy && !testing,
                    |this, _, _| this.test_ai_mcp_form(), cx)));
        if testing { body = body.child(meta_text("正在连接 MCP 服务并读取工具列表…")); }
        let form_request_id = self.ai_extensions.mcp_test_request_id;
        body = body.child(h_flex().w_full().items_center().justify_between().gap_2()
            .child(meta_text("自动发现工具（运行前仍需授权）"))
            .child(self.toggle_switch("ai-mcp-auto-discover", self.ai_extensions.mcp_auto_discover,
                busy || testing, move |this, checked, _, _| {
                    if this.active_dialog == Some(DialogState::AiMcpForm)
                        && this.ai_extensions.mcp_test_request_id == form_request_id {
                        this.ai_extensions.mcp_auto_discover = checked;
                    }
                }, cx)));
        let mut tools = v_flex().w_full().gap_2()
            .child(meta_text("可关闭不需要的工具；读写类型用于运行前的提示，未知工具默认按写入处理。"));
        for (index, tool) in self.ai_extensions.mcp_tools.iter().enumerate() {
            let access_view = cx.entity();
            let form_request_id = self.ai_extensions.mcp_test_request_id;
            tools = tools.child(h_flex().w_full().items_center().justify_between().gap_2()
                .py_2().border_b_1().border_color(rgb(ui_theme::BORDER_MUTED))
                .child(h_flex().flex_1().min_w(px(0.0)).items_center().gap_2()
                    .child(self.toggle_switch(format!("ai-mcp-tool-{index}"), tool.allowed,
                        busy || testing, move |this, checked, _, _| {
                            if this.active_dialog == Some(DialogState::AiMcpForm)
                                && this.ai_extensions.mcp_test_request_id == form_request_id
                                && let Some(tool) = this.ai_extensions.mcp_tools.get_mut(index) {
                                tool.allowed = checked;
                                this.ai_extensions.mcp_auto_discover = false;
                            }
                        }, cx))
                    .child(primary_text(tool.name.clone()).truncate()))
                .child(Button::new(format!("ai-mcp-access-{index}")).small()
                    .disabled(busy || testing)
                    .label(if tool.access == WorkflowToolAccess::Read { "读取" } else { "写入" })
                    .on_click(move |_, _, cx| {
                        access_view.update(cx, |this, cx| {
                            if this.active_dialog == Some(DialogState::AiMcpForm)
                                && this.ai_extensions.mcp_test_request_id == form_request_id
                                && let Some(tool) = this.ai_extensions.mcp_tools.get_mut(index) {
                                tool.access = if tool.access == WorkflowToolAccess::Read {
                                    WorkflowToolAccess::Write
                                } else { WorkflowToolAccess::Read };
                            }
                            cx.notify();
                        });
                    })));
        }
        if verified || !self.ai_extensions.mcp_tools.is_empty() {
            let expanded = self.ai_extensions.mcp_tools_expanded;
            let view = cx.entity();
            let form_request_id = self.ai_extensions.mcp_test_request_id;
            body = body.child(Collapsible::new().w_full().gap_2().open(expanded)
                .child(h_flex().w_full().items_center().justify_between().gap_2()
                    .child(meta_text(format!("工具记录 {} 个 · 白名单 {enabled_count} 个",
                        self.ai_extensions.mcp_tools.len())))
                    .child(settings_command_button("ai-mcp-tools-advanced",
                        if expanded { "收起高级配置" } else { "高级配置（可选）" },
                        SettingsButtonTone::Secondary, true, cx)
                        .icon(if expanded { IconName::ChevronUp } else { IconName::ChevronDown })
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| {
                                if this.active_dialog == Some(DialogState::AiMcpForm)
                                    && this.ai_extensions.mcp_test_request_id == form_request_id {
                                    this.ai_extensions.mcp_tools_expanded = !expanded;
                                    cx.notify();
                                }
                            });
                        })))
                .content(tools));
            if !self.ai_extensions.mcp_auto_discover && !selected_tools {
                body = body.child(error_text("请至少启用一个工具后保存服务"));
            }
        }
        if let Some(error) = &self.ai_extensions.error {
            body = body.child(error_text(error.clone()));
        }
        body = body.child(meta_text("自动发现的未知工具按写入请求授权；关闭自动发现后只允许白名单工具。"));
        let viewport = div().id("ai-mcp-form-viewport").size_full().min_h(px(0.0))
            .overflow_y_scroll().track_scroll(&handle).child(body.flex_none());
        self.dialog_panel(if editing { "编辑 MCP 服务" } else { "添加 MCP 服务" }, cx)
            .w(panel_width).h(panel_height).overflow_hidden()
            .child(scrollable_frame_when("ai-mcp-form-scroll", ScrollbarMode::Vertical,
                viewport.into_any_element(), handle, true, cx))
            .child(dialog_actions().flex_none().bg(rgb(ui_theme::WB_PANEL))
                .child(self.button("取消", !busy && !testing,
                    |this, _, _| this.close_dialog(), cx))
                .child(self.primary_button("保存服务", !busy && !testing
                    && !self.ai_extensions.mcp_id.value.trim().is_empty()
                    && !self.ai_extensions.mcp_command.value.trim().is_empty()
                    && (self.ai_extensions.mcp_auto_discover || selected_tools),
                    |this, _, _| this.save_ai_mcp_form(), cx)))
    }

    pub(crate) fn render_ai_mcp_remove_dialog(&self, name: &str,
        cx: &mut Context<Self>) -> impl IntoElement {
        let name_for_remove = name.to_owned();
        self.dialog_panel("移除 MCP 服务", cx)
            .child(primary_text(format!("确定移除 {name}？")))
            .child(meta_text("工作流中引用此服务的步骤将无法再运行。"))
            .when_some(self.ai_extensions.error.as_ref(), |this, error| {
                this.child(error_text(error.clone()))
            })
            .child(dialog_actions()
                .child(self.button("取消", !self.ai_extensions.action_busy,
                    |this, _, _| this.close_dialog(), cx))
                .child(self.danger_button("确认移除", !self.ai_extensions.action_busy,
                    move |this, _, _| this.remove_ai_mcp_server(name_for_remove.clone()), cx)))
    }

    pub(crate) fn render_ai_mcp_builtin_details_dialog(&self,
        cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.ai_extensions.runtime.as_ref()
            .is_some_and(|runtime| runtime.mcp_ready);
        let mut panel = self.dialog_panel("内置 browser.edge 服务", cx).w(px(520.0))
            .child(meta_text(if ready { "运行组件已就绪" } else {
                "运行组件尚未安装，可在运行环境页下载并启用"
            }))
            .child(meta_text("客户端管理的 Microsoft Edge MCP；工作流调用前会逐次确认。"));
        for (name, tool) in extensions::builtin_browser_server().tools {
            let access = match tool.access {
                WorkflowToolAccess::Read => "读取",
                WorkflowToolAccess::Write => "写入",
            };
            panel = panel.child(settings_row(name, access));
        }
        panel.child(dialog_actions()
            .child(self.button("关闭", true, |this, _, _| this.close_dialog(), cx)))
    }
}
