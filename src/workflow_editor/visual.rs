//! V2 文档控件。图标与弹出菜单复用 Kit；字段继续使用同一输入宿主。
use super::*;
use super::document::{card, column, hint, text, edit_tool_selection};
use gpui_kit::assets::IconName;
use gpui_kit::base::ElementExt;
use gpui_kit::component::{Icon, Sizable, Disableable, button::{Button, ButtonVariants}, collapsible::Collapsible, menu::{DropdownMenu, PopupMenu, PopupMenuItem}};

type MenuWidth = std::rc::Rc<std::cell::Cell<Option<gpui::Pixels>>>;

pub(super) fn measure_menu_field(field: gpui::Div, width: MenuWidth) -> gpui::Div {
    field.on_prepaint(move |bounds, _, _| width.set(Some(bounds.size.width)))
}

pub(super) fn fit_menu(menu: PopupMenu, width: &MenuWidth, window: &Window) -> PopupMenu {
    // Kit 的 min_w / max_w 都作用于菜单内容盒；同时设置才能覆盖默认最大宽度。
    let available = (window.viewport_size().width - px(16.0)).max(px(1.0));
    let width = width.get().unwrap_or(px(200.0)).min(available).max(px(1.0));
    menu.min_w(width).max_w(width)
}

pub(super) fn icon(name: IconName, color: u32) -> Icon {
    Icon::new(name).with_size(px(16.0)).text_color(rgb(color))
}

pub(super) fn badge(label: impl Into<gpui::SharedString>) -> gpui::Div {
    div().flex().items_center().flex_none().px_2().py_1().rounded(px(ui_theme::RADIUS_XS))
        .bg(rgb(ui_theme::PRIMARY_SUBTLE)).text_color(rgb(ui_theme::PRIMARY))
        .text_size(px(12.0)).child(label.into())
}

pub(super) fn icon_button(id: impl Into<gpui::ElementId>, label: &str, name: IconName, enabled: bool) -> Button {
    Button::new(id).ghost().compact().icon(icon(name, ui_theme::WORKFLOW_META))
        .tooltip(label.to_string()).accessibility_label(label.to_string()).disabled(!enabled)
        .w(px(28.0)).h(px(28.0)).min_w(px(0.0)).rounded(px(ui_theme::RADIUS_XS))
}

pub(super) fn menu_button(id: impl Into<gpui::ElementId>, label: impl Into<gpui::SharedString>, enabled: bool) -> Button {
    // 下拉需要原生 label 测量文字，宽度由外层字段容器分配，不能在弹层包装内 flex_1。
    Button::new(id).small().label(label).dropdown_caret(true).disabled(!enabled)
        .w_full().h(px(36.0)).min_w(px(0.0)).px_3()
        .bg(rgb(ui_theme::WB_PANEL)).text_color(rgb(ui_theme::CONTENT_PRIMARY))
        .border_1().border_color(rgb(ui_theme::WORKFLOW_OUTLINE)).rounded(px(ui_theme::RADIUS_XS))
}

pub(super) fn step_icon(step: &WorkflowEditorStepData) -> (IconName, &'static str) {
    if step.kind == WorkflowStepKind::Invoke {
        match step.invoke.value(WorkflowStepSlot::Uses) {
            "skill.run" => (IconName::Sparkles, "Skill"),
            "js.run" => (IconName::Braces, "JavaScript"),
            "mcp.call" => (IconName::Plug, "MCP"),
            _ => (IconName::Workflow, "动作"),
        }
    } else {
        (match step.kind {
            WorkflowStepKind::Merge => IconName::GitMerge,
            WorkflowStepKind::Fetch | WorkflowStepKind::Pull => IconName::Download,
            WorkflowStepKind::Push => IconName::Upload,
            WorkflowStepKind::DeleteBranches => IconName::Trash,
            WorkflowStepKind::EnsureClean | WorkflowStepKind::GuardRemoteBranch | WorkflowStepKind::AssertBranch => IconName::ShieldCheck,
            _ => IconName::GitBranch,
        }, "Git")
    }
}

fn status(label: &str, available: bool) -> gpui::Div {
    badge(label.to_string()).gap_1()
        .bg(rgb(if available { ui_theme::WORKFLOW_SUCCESS_BG } else { ui_theme::FEEDBACK_WARNING_BG }))
        .text_color(rgb(if available { ui_theme::WORKFLOW_SUCCESS_TEXT } else { ui_theme::FEEDBACK_WARNING_TEXT }))
        .child(icon(if available { IconName::Check } else { IconName::Info },
            if available { ui_theme::WORKFLOW_SUCCESS_TEXT } else { ui_theme::FEEDBACK_WARNING_TEXT }))
}

impl RepositoryView {
    pub(super) fn document_menu_width(&self, key: String) -> MenuWidth {
        // 打开菜单会重新渲染，测量值需跨帧保留，不能在 builder 内重建为零宽。
        self.workflow_editor.as_ref().expect("编辑器状态缺失").document_menu_widths
            .borrow_mut().entry(key).or_default().clone()
    }

    pub(super) fn document_input(&self, id: WorkflowEditorFieldId, _window: &Window, _cx: &mut Context<Self>) -> gpui::AnyElement {
        let field_id = FieldId::WorkflowEditor(id);
        let blocked = self.active_operation_blocker_message().is_some() && !self.operation_blocker_allows_text_field(field_id);
        let height = if matches!(id, WorkflowEditorFieldId::StepParam { slot: WorkflowStepSlot::Task, .. }) { 72.0 } else { 128.0 };
        self.kit_field(field_id).map(|field| field.render_workflow_document(height, blocked))
            .unwrap_or_else(|| div().into_any_element())
    }

    pub(super) fn document_options(&self, index: usize, slot: WorkflowStepSlot, options: Vec<String>, width: MenuWidth, cx: &Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity();
        let current = self.workflow_editor.as_ref().and_then(|editor| editor.data.steps.get(index))
            .map(|step| step.invoke.value(slot).to_string()).unwrap_or_default();
        div().w(px(28.0)).h(px(36.0)).flex_none().child(
            icon_button(format!("workflow-v2-options-{index}-{slot:?}"), if options.is_empty() { "没有可选项目，请先在设置中配置" } else { "选择已配置的项目" }, IconName::ChevronDown, !options.is_empty())
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |mut menu, window, _| {
                menu = fit_menu(menu, &width, window).scrollable(true).max_h(px(240.0));
                for value in &options {
                    let value = value.clone();
                    menu = menu.item(PopupMenuItem::new(value.clone()).checked(value == current).on_click(window.listener_for(&entity, move |this, _, _, cx| {
                        this.workflow_document_set_slot(index, slot, value.clone()); cx.notify();
                    })));
                }
                menu
            })).into_any_element()
    }

    pub(super) fn document_parameter(&self, index: usize, slot: WorkflowStepSlot, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let step = &editor.data.steps[index];
        let value = step.invoke.value(slot);
        let options = match slot {
            WorkflowStepSlot::Skill => self.ai_extensions.skills.iter().map(|(name, _)| name.clone()).collect(),
            WorkflowStepSlot::Server => std::iter::once("browser.edge".to_string()).chain(self.ai_extensions.mcp.servers.keys().cloned()).collect(),
            WorkflowStepSlot::Tool => self.document_server_tools(step.invoke.value(WorkflowStepSlot::Server)),
            _ => Vec::new(),
        };
        let selector = matches!(slot, WorkflowStepSlot::Skill | WorkflowStepSlot::Server | WorkflowStepSlot::Tool);
        let roomy = f32::from(window.viewport_size().width) >= 1380.0;
        let label = match slot { WorkflowStepSlot::Skill => "选择 Skill", WorkflowStepSlot::Task => "任务描述", WorkflowStepSlot::SaveAs => "输出变量", _ => slot.label() };
        let width = self.document_menu_width(format!("options-{index}-{slot:?}"));
        let mut field = div().flex().min_w_0().flex_1().gap_1().items_center()
            .child(div().flex_1().min_w_0().when(slot == WorkflowStepSlot::Task, |input| input.w_full())
                .child(self.document_input(WorkflowEditorFieldId::StepParam { step: index, slot }, window, cx)))
            .when(selector, |row| row.child(self.document_options(index, slot, options, width.clone(), cx)));
        if roomy && (selector || slot == WorkflowStepSlot::SaveAs) {
            field = field.flex_none().w(px(if slot == WorkflowStepSlot::SaveAs { 400.0 } else { 308.0 }));
        }
        if selector { field = measure_menu_field(field, width); }
        if slot == WorkflowStepSlot::Task { field = field.flex_col().items_start().w_full().child(hint("描述要完成的任务，支持使用输入变量")); }
        let available = match slot {
            WorkflowStepSlot::Skill => self.ai_extensions.skills.iter().any(|(name, _)| name == value),
            WorkflowStepSlot::Server => value == "browser.edge" || self.ai_extensions.mcp.servers.contains_key(value),
            _ => false,
        };
        let mut row = div().flex().items_start().min_w_0().gap_3()
            .child(text(label.to_string()).w(px(108.0)).flex_none().pt(px(8.0))).child(field);
        if matches!(slot, WorkflowStepSlot::Skill | WorkflowStepSlot::Server) {
            row = row.child(status(if available { if slot == WorkflowStepSlot::Skill { "已安装" } else { "已配置" } } else { "未找到" }, available));
        }
        let help = match slot {
            WorkflowStepSlot::Skill => "使用本地安装的 Skill 执行任务",
            WorkflowStepSlot::Server => "使用已配置的服务调用工具",
            WorkflowStepSlot::SaveAs => "保存结果，供后续步骤引用 ${out.变量名}",
            _ => "",
        };
        if !help.is_empty() && roomy { row = row.child(hint(help).flex_1().min_w_0().pt_2()); }
        else if slot == WorkflowStepSlot::SaveAs { return column().gap_1().child(row).child(hint(help).pl(px(120.0))); }
        row
    }

    fn document_server_tools(&self, name: &str) -> Vec<String> {
        if name == khaslana::workflow::browser_runtime::SERVER_ID {
            khaslana::workflow::extensions::builtin_browser_server().tools.keys().cloned().collect()
        } else { self.ai_extensions.mcp.servers.get(name).map(v2::configured_server_tools).unwrap_or_default() }
    }

    fn document_select_tool(&mut self, index: usize, server: &str, tool: &str, selected: bool) {
        let source = self.workflow_editor.as_ref().and_then(|editor| editor.data.steps.get(index))
            .map(|step| step.invoke.value(WorkflowStepSlot::Tools).to_string()).unwrap_or_default();
        match edit_tool_selection(&source, server, tool, selected) {
            Ok(value) => self.workflow_document_set_slot(index, WorkflowStepSlot::Tools, value),
            Err(error) => { if let Some(editor) = self.workflow_editor.as_mut() { editor.data.error = Some(error); } }
        }
    }

    pub(super) fn document_tools(&self, index: usize, cx: &mut Context<Self>) -> gpui::Div {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let step = &editor.data.steps[index];
        let selected: serde_json::Value = json5::from_str(step.invoke.value(WorkflowStepSlot::Tools)).unwrap_or_default();
        let selected = selected.as_array().cloned().unwrap_or_default();
        let key = step.invoke.value(WorkflowStepSlot::StepId).to_string();
        let expanded = editor.document_tools_open.get(&key).copied().unwrap_or(!selected.is_empty());
        let enabled = !editor.ai_loading;
        let label = if selected.is_empty() { "调用工具（可选）".to_string() }
            else { format!("调用工具（已选 {} 个）", selected.len()) };
        let toggle = Button::new(format!("workflow-v2-tools-toggle-{key}")).ghost().small().label(label)
            .icon(icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }, ui_theme::WORKFLOW_META))
            .disabled(!enabled).accessibility_label(if expanded { "收起调用工具" } else { "展开调用工具（可选）" })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(editor) = this.workflow_editor.as_mut() { editor.document_tools_open.insert(key.clone(), !expanded); }
                cx.notify();
            }));
        let servers = std::iter::once("browser.edge".to_string())
            .chain(self.ai_extensions.mcp.servers.keys().filter(|name| name.as_str() != "browser.edge").cloned())
            .map(|name| { let tools = self.document_server_tools(&name); (name, tools) })
            .filter(|(_, tools)| !tools.is_empty()).collect::<Vec<_>>();
        let mut chips = div().flex().flex_wrap().items_center().min_w_0().flex_1().gap_1();
        for (position, item) in selected.iter().enumerate() {
            let Some(tool) = item["tool"].as_str() else { continue; };
            let Some(server) = item["server"].as_str() else { continue; };
            let tool = tool.to_string(); let server = server.to_string();
            chips = chips.child(badge(format!("{server} / {tool}")).py(px(2.0)).gap_1()
                .child(icon_button(format!("workflow-v2-tool-remove-{index}-{position}"), &format!("移除 {server} / {tool}"), IconName::X, enabled)
                    .w(px(18.0)).h(px(18.0)).on_click(cx.listener(move |this, _, _, cx| { this.document_select_tool(index, &server, &tool, false); cx.notify(); }))));
        }
        if selected.is_empty() { chips = chips.child(hint("未添加工具，可直接执行此步骤")); }
        let entity = cx.entity();
        let tool_width = self.document_menu_width(format!("tools-{index}"));
        let tool_measure = tool_width.clone();
        let menu_button = icon_button(format!("workflow-v2-tool-picker-{index}"), if servers.is_empty() { "没有可选工具，请先在 AI 设置中配置" } else { "添加可调用的工具" }, IconName::Plus, enabled && !servers.is_empty())
            .w(px(36.0)).h(px(36.0))
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |mut menu, window, _| {
                menu = fit_menu(menu, &tool_width, window).scrollable(true).max_h(px(260.0));
                for (server, tools) in &servers {
                    menu = menu.label(server.clone());
                    for tool in tools {
                        let checked = selected.iter().any(|item| item["server"] == *server && item["tool"] == *tool);
                        let tool = tool.clone(); let server = server.clone();
                        menu = menu.item(PopupMenuItem::new(tool.clone()).checked(checked).on_click(window.listener_for(&entity, move |this, _, _, cx| {
                            this.document_select_tool(index, &server, &tool, !checked); cx.notify();
                        })));
                    }
                } menu
            });
        let content = column().gap_2()
            .child(hint("仅在步骤需要外部能力时添加。Skill 由 AI 按任务调用；JavaScript 在脚本中通过 mcp.call(...) 调用。"))
            .child(div().flex().items_start().gap_3().child(text("允许调用").w(px(108.0)).pt_2().flex_none())
                .child(measure_menu_field(div().relative().flex().flex_1().min_w_0().items_center().gap_1().rounded(px(ui_theme::RADIUS_XS))
                    // 边框独立绘制，箭头触发器的右边界与整个字段相同。
                    .child(div().absolute().inset_0().border_1().border_color(rgb(ui_theme::WORKFLOW_OUTLINE)).rounded(px(ui_theme::RADIUS_XS)))
                    .child(chips.pl_1().py_1()).child(div().w(px(36.0)).h(px(36.0)).flex_none().child(menu_button)), tool_measure)));
        column().child(Collapsible::new().open(expanded).gap_2().child(toggle).content(content))
    }

    pub(super) fn document_git_parameters(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let step = &self.workflow_editor.as_ref().expect("编辑器状态缺失").data.steps[index];
        let mut body = column();
        for slot in step.kind.slots() { body = body.child(self.document_field(WorkflowEditorFieldId::StepParam { step: index, slot: *slot }, slot.label(), window, cx)); }
        let flags = match step.kind {
            WorkflowStepKind::CreateBranch => vec![(WorkflowStepFlag::CreateCheckout, "创建后切换到新分支", step.checkout)],
            WorkflowStepKind::Push => vec![(WorkflowStepFlag::PushSetUpstream, "推送时建立上游跟踪", step.set_upstream)],
            WorkflowStepKind::GuardRemoteBranch => vec![(WorkflowStepFlag::GuardFetch, "检查前先获取远端引用", step.guard_fetch)],
            WorkflowStepKind::FilterBranches => vec![(WorkflowStepFlag::FilterSkipCurrent, "排除当前分支", step.filter_skip_current)],
            WorkflowStepKind::DeleteBranches => vec![(WorkflowStepFlag::DeleteDryRun, "试运行（只列出待删除分支）", step.delete_dry_run), (WorkflowStepFlag::DeleteSkipCurrent, "跳过当前分支", step.delete_skip_current)],
            _ => Vec::new(),
        };
        for (flag, label, current) in flags {
            body = body.child(div().flex().items_center().gap_3().child(div().w(px(108.0)).flex_none())
                .child(self.toggle_switch(format!("workflow-v2-flag-{index}-{flag:?}"), current, false, move |this, next, _, _| {
                    if let Some(step) = this.workflow_editor.as_mut().and_then(|editor| editor.data.steps.get_mut(index)) {
                        match flag { WorkflowStepFlag::CreateCheckout => step.checkout = next, WorkflowStepFlag::PushSetUpstream => step.set_upstream = next,
                            WorkflowStepFlag::GuardFetch => step.guard_fetch = next, WorkflowStepFlag::FilterSkipCurrent => step.filter_skip_current = next,
                            WorkflowStepFlag::DeleteDryRun => step.delete_dry_run = next, WorkflowStepFlag::DeleteSkipCurrent => step.delete_skip_current = next }
                    }
                }, cx)).child(hint(label)));
        }
        if step.kind == WorkflowStepKind::GuardRemoteBranch {
            for (exists, current) in [(true, step.on_exists), (false, step.on_missing)] {
                let entity = cx.entity();
                let width = self.document_menu_width(format!("guard-{index}-{exists}"));
                let measured = width.clone();
                body = body.child(div().flex().items_center().gap_3().child(text(if exists { "远端分支存在" } else { "远端分支不存在" }).w(px(108.0)))
                    .child(measure_menu_field(div().flex_1().min_w_0().h(px(36.0)).child(
                        menu_button(format!("workflow-v2-guard-{index}-{exists}"), if current == RemoteBranchGuardAction::Fail { "停止工作流" } else { "继续执行" }, true)
                        .dropdown_menu(move |mut menu, window, _| {
                            menu = fit_menu(menu, &width, window);
                            for (value, label) in [(RemoteBranchGuardAction::Fail, "停止工作流"), (RemoteBranchGuardAction::Continue, "继续执行")] {
                                menu = menu.item(PopupMenuItem::new(label).checked(value == current).on_click(window.listener_for(&entity, move |this, _, _, cx| {
                                    this.workflow_editor_set_guard_action(index, exists.then_some(value), (!exists).then_some(value)); cx.notify();
                                })));
                            } menu
                        })), measured)));
            }
        }
        if step.kind == WorkflowStepKind::EnsureClean { body = body.child(hint(step.kind.description())); }
        body
    }

    pub(super) fn document_variable_card(&self, id: String, name: String, description: String, default: Option<String>, cx: &Context<Self>) -> gpui::Div {
        let copied = name.clone();
        card().border_0().gap_1().bg(rgb(ui_theme::WORKFLOW_SURFACE))
            .child(div().flex().items_center().gap_1().child(text(name.clone()).font_weight(gpui::FontWeight::SEMIBOLD).flex_1().min_w_0())
                .child(icon_button(format!("workflow-v2-variable-copy-{id}"), &format!("复制 {name}"), IconName::Copy, true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                        this.status = format!("已复制变量引用 {copied}");
                        this.notify_success(this.status.clone(), cx);
                    }))))
            .child(badge(if default.is_some() { "字符串" } else { "变量" }))
            .when(!description.is_empty(), |card| card.child(hint(description)))
            .when_some(default, |card, value| card.child(hint(format!("默认值：{value}"))))
    }
}
