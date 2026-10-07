//! V2 文档式工作流视图。沿用模态生命周期与字段路由，在页面内承接焦点隔离。
use super::*;
use gpui_kit::base::FocusTrapElement;
use crate::ui::components::{floating_panel, settings_command_button, SettingsButtonTone};
use super::visual::{badge, icon, icon_button, menu_button, step_icon, measure_menu_field, fit_menu};
use gpui_kit::assets::IconName;
use gpui_kit::component::{button::{Button, ButtonVariants}, menu::{DropdownMenu, PopupMenuItem}};

pub(super) fn column() -> gpui::Div {
    div().flex().flex_col().min_w_0().gap_3()
}

pub(super) fn card() -> gpui::Div {
    column().p_3().border_1().border_color(rgb(ui_theme::WORKFLOW_OUTLINE))
        .rounded(px(ui_theme::RADIUS_SM)).bg(rgb(ui_theme::WB_PANEL))
}

pub(super) fn text(value: impl Into<gpui::SharedString>) -> gpui::Div {
    div().text_size(px(14.0)).text_color(rgb(ui_theme::CONTENT_PRIMARY)).child(value.into())
}

pub(super) fn hint(value: impl Into<gpui::SharedString>) -> gpui::Div {
    div().text_size(px(13.0)).text_color(rgb(ui_theme::WORKFLOW_META)).child(value.into())
}

pub(super) fn variable_rail_visible(width: f32) -> bool { width >= 1100.0 }

pub(super) fn next_step_id(steps: &[WorkflowEditorStepData]) -> String {
    (1..).map(|index| format!("step-{index}"))
        .find(|id| steps.iter().all(|step| step.invoke.value(WorkflowStepSlot::StepId) != id))
        .expect("步骤数有限")
}

pub(super) fn v2_copy_data(data: &WorkflowEditorData) -> WorkflowEditorData {
    let mut copy = data.clone();
    copy.version = 2;
    copy.editing_path = None;
    copy.file_name = format!("{}-v2", data.file_name.trim());
    copy.error = None;
    copy
}

pub(super) fn duplicate_step_data(steps: &[WorkflowEditorStepData], index: usize) -> Option<WorkflowEditorStepData> {
    let mut step = steps.get(index)?.clone();
    if step.kind == WorkflowStepKind::Invoke {
        step.invoke.set(WorkflowStepSlot::StepId, next_step_id(steps));
        step.invoke.set(WorkflowStepSlot::SaveAs, String::new());
    } else if step.kind == WorkflowStepKind::FilterBranches {
        step.output.clear();
    }
    Some(step)
}

pub(super) fn edit_tool_selection(source: &str, server: &str, tool: &str, selected: bool) -> Result<String, String> {
    let mut value: serde_json::Value = if source.trim().is_empty() { serde_json::json!([]) }
        else { json5::from_str(source).map_err(|error| format!("工具列表格式错误：{error}"))? };
    let array = value.as_array_mut().ok_or("允许的工具必须是数组")?;
    array.retain(|item| !(item.get("server").and_then(serde_json::Value::as_str) == Some(server)
        && item.get("tool").and_then(serde_json::Value::as_str) == Some(tool)));
    if selected { array.push(serde_json::json!({"server":server,"tool":tool})); }
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

impl RepositoryView {
    pub(crate) fn submit_workflow_document_field(&mut self, field: FieldId, cx: &mut Context<Self>) -> bool {
        let FieldId::WorkflowEditor(id) = field else { return false; };
        if !self.workflow_document_editor_visible() { return false; }
        if self.workflow_editor.as_ref().is_some_and(|editor| editor.ai_loading) { return true; }
        match id {
            WorkflowEditorFieldId::AiDescription => self.generate_workflow_template_with_ai(cx),
            WorkflowEditorFieldId::PickerSearch => {
                let query = self.workflow_editor.as_ref().map(|editor| editor.picker_search_field.value.to_lowercase()).unwrap_or_default();
                if let Some((uses, _)) = [("mcp.call", "MCP 工具"), ("js.run", "JavaScript 转换输入组合变量"), ("skill.run", "AI Skill 本地安装任务")]
                    .into_iter().find(|(uses, label)| query.trim().is_empty() || format!("{uses} {label}").to_lowercase().contains(query.trim())) {
                    self.workflow_document_add_action(uses);
                } else if let Some(kind) = filter_workflow_step_kinds(&query).first() {
                    self.workflow_editor_add_step_of_kind(*kind);
                }
            }
            _ => self.save_workflow_editor(cx),
        }
        cx.notify();
        true
    }

    pub(crate) fn workflow_editor_copy_as_v2(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.workflow_editor.as_mut() else { return; };
        if editor.ai_loading { return; }
        editor.sync_from_fields();
        let data = v2_copy_data(&editor.data);
        let editor = WorkflowEditorState::from_data(data, cx);
        self.show_workflow_editor(editor, cx);
    }

    pub(crate) fn workflow_document_editor_visible(&self) -> bool {
        matches!(self.active_dialog, Some(crate::DialogState::WorkflowEditor))
            && self.workflow_editor.as_ref().is_some_and(|editor| editor.data.version == 2)
            && self.main_mode == crate::MainMode::Workflow
    }

    pub(crate) fn workflow_document_picker_focus(&self) -> Option<gpui::FocusHandle> {
        self.workflow_editor.as_ref().filter(|editor| self.workflow_document_editor_visible() && editor.step_picker_open)
            .map(|editor| editor.document_picker_focus.clone())
    }

    pub(crate) fn workflow_document_picker_search_focus(&self) -> Option<gpui::FocusHandle> {
        self.workflow_editor.as_ref().filter(|editor| self.workflow_document_editor_visible() && editor.step_picker_open)
            .map(|editor| editor.picker_search_field.focus.clone())
    }

    pub(crate) fn workflow_document_editor_blocks_mouse(&self, position: gpui::Point<gpui::Pixels>) -> bool {
        self.workflow_document_editor_visible() && self.ai_thinking_overlay.is_none()
            && self.workflow_editor.as_ref().is_some_and(|editor| editor.document_bounds.get().is_none_or(|bounds| !bounds.contains(&position)))
    }

    fn workflow_document_add_action(&mut self, uses: &'static str) {
        if let Some(editor) = self.workflow_editor.as_mut() {
            editor.sync_from_fields();
            let mut step = WorkflowEditorStepData::new(WorkflowStepKind::Invoke);
            step.invoke = v2::InvokeEditorData::new_action(uses, &next_step_id(&editor.data.steps));
            editor.data.steps.push(step);
            editor.data.selected_step = editor.data.steps.len() - 1;
            editor.step_picker_open = false;
            editor.ensure_field_capacity();
        }
    }

    fn workflow_document_duplicate(&mut self, index: usize) {
        if let Some(editor) = self.workflow_editor.as_mut() {
            editor.sync_from_fields();
            // 副本不能复用输出名，否则后续步骤可能读到不同动作的结果。
            let Some(step) = duplicate_step_data(&editor.data.steps, index) else { return; };
            editor.data.steps.insert(index + 1, step);
            editor.step_fields.insert(index + 1, WorkflowEditorStepState { fields: Default::default() });
            editor.data.selected_step = index + 1;
            editor.document_advanced_step = None;
        }
    }

    pub(super) fn workflow_document_set_slot(&mut self, index: usize, slot: WorkflowStepSlot, value: String) {
        if let Some(editor) = self.workflow_editor.as_mut() {
            if let Some(step) = editor.data.steps.get_mut(index) { step.set_slot_value(slot, value.clone()); }
            if let Some(field) = editor.step_fields.get_mut(index).and_then(|step| step.fields.get_mut(&slot)) {
                field.set_value(value);
            }
        }
    }

    fn workflow_document_check(&mut self) {
        let Some(editor) = self.workflow_editor.as_mut() else { return; };
        editor.sync_from_fields();
        let result = build_workflow_definition(&editor.data).and_then(|definition| {
            let serialized = json5::to_string(&definition).map_err(|error| error.to_string())?;
            parse_workflow_json5(&serialized).map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&definition).map_err(|error| error.to_string())
        });
        editor.data.error = result.as_ref().err().cloned();
        editor.document_preview_open = result.is_ok();
    }

    pub(super) fn document_command(&self, id: impl Into<gpui::ElementId>, label: impl Into<gpui::SharedString>,
        primary: bool, enabled: bool, cx: &Context<Self>) -> gpui_kit::component::button::Button {
        settings_command_button(id, label, if primary { SettingsButtonTone::Primary } else { SettingsButtonTone::Secondary }, enabled, cx)
            .min_w(px(0.0))
    }

    pub(super) fn document_field(&self, id: WorkflowEditorFieldId, label: &str, window: &Window,
        cx: &mut Context<Self>) -> gpui::Div {
        let multiline = RepositoryView::is_multiline_field(FieldId::WorkflowEditor(id));
        div().flex().items_start().min_w_0().gap_3()
            .child(text(label.to_string()).w(px(108.0)).flex_none().pt(px(8.0)))
            .child(div().flex_1().min_w_0().child(self.document_input(id, window, cx)))
            .when(multiline, |row| row.items_start())
    }

    pub(crate) fn render_workflow_document_editor(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(editor) = self.workflow_editor.as_ref() else { return div().into_any_element(); };
        let wide = variable_rail_visible(f32::from(window.viewport_size().width));
        let branch = self.snapshot.as_ref().and_then(|snapshot| snapshot.branches.iter().find(|branch| branch.is_head))
            .map(|branch| branch.name.clone()).unwrap_or_else(|| "未打开仓库".into());
        let scroll_handle = self.scroll_handle("workflow-v2-document");
        let mut document = column().flex_1().min_w_0();
        if editor.document_metadata_open {
            document = document.child(card().child(text("模板设置"))
                .child(column()
                .child(self.document_field(WorkflowEditorFieldId::Name, "模板名称", window, cx).flex_1())
                .child(self.document_field(WorkflowEditorFieldId::FileName, "保存文件名", window, cx)))
                .child(self.toggle_row("workflow-v2-clean", "运行前要求工作区干净", editor.data.require_clean_worktree,
                    |this, _, _| { if let Some(editor) = this.workflow_editor.as_mut() { editor.data.require_clean_worktree = !editor.data.require_clean_worktree; } }, cx)));
        }
        if editor.document_ai_open {
            document = document.child(card().child(text("描述工作流需求"))
                .child(self.input(FieldId::WorkflowEditor(WorkflowEditorFieldId::AiDescription), false, window, cx))
                .child(hint("AI 生成后请逐项检查，再保存模板。"))
                .child(self.document_command("workflow-v2-ai-generate", if editor.ai_loading { "生成中…" } else { "生成模板" }, true, !editor.ai_loading && self.ai_settings.is_usable(), cx)
                    .tooltip(if self.ai_settings.is_usable() { "基于当前草稿生成或调整步骤" } else { "请先在设置中心配置并启用 AI" })
                    .on_click(cx.listener(|this, _, _, cx| this.generate_workflow_template_with_ai(cx)))));
        }
        let mut inputs = card().child(div().flex().flex_wrap().items_center().justify_between().gap_2()
            .child(div().flex().flex_wrap().items_center().gap_4().child(text("工作流输入").font_weight(gpui::FontWeight::SEMIBOLD))
                .child(hint("在运行时提供这些输入值，用于后续步骤引用")))
            .child(self.document_command("workflow-v2-input-add", "添加输入", false, true, cx)
                .icon(icon(IconName::Plus, ui_theme::WORKFLOW_META)).ghost()
                .on_click(cx.listener(|this, _, _, cx| { this.workflow_editor_add_input_row(); cx.notify(); }))));
        if editor.data.inputs.is_empty() { inputs = inputs.child(hint("添加运行前填写的参数，在步骤中使用 ${变量名} 引用。")); }
        for (index, row) in editor.data.inputs.iter().enumerate() {
            inputs = inputs.child(column().gap_2()
                .child(div().flex().items_center().min_w_0().gap_3()
                    .child(div().w(px(96.0)).flex_none().child(self.document_input(WorkflowEditorFieldId::InputPart { index, part: WorkflowInputPart::Key }, window, cx)))
                    .child(badge("字符串"))
                    .child(div().flex_1().min_w_0().child(self.document_input(WorkflowEditorFieldId::InputPart { index, part: WorkflowInputPart::Default }, window, cx)))
                    .when(!row.description.is_empty() && wide, |line| line.child(hint(row.description.clone()).w(px(190.0)).flex_none()))
                    .child(icon_button(format!("workflow-v2-input-remove-{index}"), "删除输入", IconName::Trash, true)
                        .on_click(cx.listener(move |this, _, _, cx| { this.workflow_editor_remove_input_row(index); cx.notify(); }))))
                .when(editor.document_details_open, |group| group
                    .child(self.document_field(WorkflowEditorFieldId::InputPart { index, part: WorkflowInputPart::Label }, "运行时显示名称", window, cx))
                    .child(self.document_field(WorkflowEditorFieldId::InputPart { index, part: WorkflowInputPart::Description }, "说明", window, cx))
                    .child(div().flex().items_center().gap_2().child(self.toggle_switch(format!("workflow-v2-input-required-{index}"), row.required, false,
                        move |this, next, _, _| { if let Some(row) = this.workflow_editor.as_mut().and_then(|editor| editor.data.inputs.get_mut(index)) { row.required = next; } }, cx)).child(hint("必填")))));
        }
        document = document.child(inputs)
            .child(self.document_command("workflow-v2-input-details", if editor.document_details_open { "收起变量设置" } else { "输入说明与自定义变量" }, false, true, cx)
                .on_click(cx.listener(|this, _, _, cx| { if let Some(editor) = this.workflow_editor.as_mut() { editor.document_details_open = !editor.document_details_open; } cx.notify(); })));
        if editor.document_details_open {
            let mut vars = card().child(text("自定义变量"));
            for index in 0..editor.data.vars.len() { vars = vars.child(self.render_workflow_var_card(index, window, cx)); }
            document = document.child(vars.child(self.document_command("workflow-v2-var-add", "添加变量", false, true, cx)
                .on_click(cx.listener(|this, _, _, cx| { this.workflow_editor_add_var_row(); cx.notify(); }))));
        }
        {
            for index in 0..editor.data.steps.len() { document = document.child(self.render_document_step(index, window, cx)); }
            if editor.data.steps.is_empty() {
                document = document.child(card().child(text("从一个步骤开始"))
                    .child(hint("按顺序组合 Git 操作、MCP 工具、JavaScript 与 Skill。"))
                    .children(WORKFLOW_EDITOR_PRESETS.iter().map(|preset| {
                        let preset = *preset;
                        self.document_command(format!("workflow-v2-preset-{}", preset.title()), preset.title(), false, true, cx)
                            .on_click(cx.listener(move |this, _, _, cx| { this.apply_workflow_editor_preset(preset, cx); cx.notify(); }))
                    }).collect::<Vec<_>>()));
            }
            document = document.child(self.document_command("workflow-v2-step-add", "添加步骤", false, true, cx)
                .icon(icon(IconName::Plus, ui_theme::PRIMARY)).w_full().h(px(40.0))
                .text_color(rgb(ui_theme::PRIMARY)).border_color(rgb(ui_theme::WORKFLOW_OUTLINE)).border_dashed()
                .on_click(cx.listener(|this, _, _, cx| { this.workflow_editor_open_step_picker(); cx.notify(); })));
        }
        document = document.child(div().flex().items_center().gap_2().child(icon(IconName::Info, ui_theme::WORKFLOW_META))
            .child(hint("运行前预览并确认本次工具权限")));
        if editor.document_preview_open {
            if let Ok(definition) = build_workflow_definition(&editor.data) {
                document = document.child(card().child(text("模板检查通过"))
                    .child(hint("以下是保存内容；实际执行预览在保存后的运行页生成。"))
                    .child(text(serde_json::to_string_pretty(&definition).unwrap_or_default())));
            }
        }
        let mut columns = div().flex().min_w_0().gap_4().child(document);
        if wide { columns = columns.child(self.render_document_variables(cx).w(px(250.0)).flex_none()); }
        let document_bounds = editor.document_bounds.clone();
        floating_panel().id("workflow-v2-editor").relative().flex().flex_col().flex_1().min_w_0().min_h(px(0.0))
            .focus_trap("dialog-overlay-trap", &self.dialog_focus)
            .child(gpui::canvas(|_, _, _| (), move |bounds, _, _, _| document_bounds.set(Some(bounds)))
                .absolute().top_0().left_0().right_0().bottom_0())
            .child(column().flex_none().p_4().gap_2()
                .child(hint("工作流 / 编辑工作流"))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .child(div().flex().items_center().gap_3().flex_1().min_w_0()
                        .child(text(if editor.name_field.value.trim().is_empty() { "新建工作流".to_string() } else { editor.name_field.value.clone() })
                            .text_size(px(if wide { 26.0 } else { ui_theme::TYPE_PAGE_TITLE })).font_weight(gpui::FontWeight::BOLD).truncate()).child(badge("V2")))
                    .child(div().flex().items_center().gap_1().p_1().rounded(px(ui_theme::RADIUS_SM)).bg(rgb(ui_theme::WORKFLOW_SURFACE))
                        .child(self.document_command("workflow-v2-edit-mode", "编辑", false, true, cx).icon(icon(IconName::Pencil, ui_theme::PRIMARY))
                            .text_color(rgb(ui_theme::PRIMARY)).tooltip("当前正在编辑工作流"))
                        .child(self.document_command("workflow-v2-run-mode", "运行", false, !editor.ai_loading, cx).ghost().icon(icon(IconName::Play, ui_theme::WORKFLOW_META))
                            .tooltip("保存草稿并前往运行页，执行前仍需预览和确认权限")
                            .on_click(cx.listener(|this, _, _, cx| this.save_workflow_editor(cx)))))
                    .child(self.document_command("workflow-v2-save", "保存工作流", true, !editor.ai_loading, cx).icon(icon(IconName::Save, ui_theme::PRIMARY_FOREGROUND))
                        .on_click(cx.listener(|this, _, _, cx| this.save_workflow_editor(cx))))
                    .child(self.document_command("workflow-v2-check", "检查模板", false, !editor.ai_loading, cx).icon(icon(IconName::FileCheck, ui_theme::CONTENT_PRIMARY))
                        .on_click(cx.listener(|this, _, _, cx| { this.workflow_document_check(); cx.notify(); })))
                    .child(self.document_command("workflow-v2-ai", "AI 生成", false, true, cx).icon(icon(IconName::Sparkles, ui_theme::PRIMARY)).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(editor) = this.workflow_editor.as_mut() { editor.document_ai_open = !editor.document_ai_open; } cx.notify(); })))
                    .child(icon_button("workflow-v2-settings", "模板名称与运行设置", IconName::Settings2, !editor.ai_loading).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(editor) = this.workflow_editor.as_mut() { editor.document_metadata_open = !editor.document_metadata_open; } cx.notify(); })))
                    .child(icon_button("workflow-v2-close", "关闭编辑器", IconName::X, true).on_click(cx.listener(|this, _, _, cx| { this.close_workflow_editor(); cx.notify(); }))))
                .child(div().flex().items_center().gap_2().child(icon(IconName::GitBranch, ui_theme::PRIMARY))
                    .child(hint("当前分支：")).child(text(branch))))
            .when_some(editor.data.error.clone(), |panel, error| panel.child(text(error).text_color(rgb(ui_theme::DESTRUCTIVE)).px_4().pb_2()))
            .child(scrollable_frame_when("workflow-v2-document", ScrollbarMode::Vertical,
                div().id("workflow-v2-document").flex().flex_col().flex_1().min_h(px(0.0)).p_4()
                    .overflow_y_scroll().track_scroll(&scroll_handle).child(columns)
                    .when(!wide, |content| content.child(self.render_document_variables(cx).mt_3())).into_any_element(),
                scroll_handle, true, cx))
            .when(editor.step_picker_open, |panel| {
                let picker_scroll = self.scroll_handle("workflow-v2-picker-scroll");
                let width = (f32::from(window.viewport_size().width) - 320.0).clamp(280.0, 560.0);
                let height = (f32::from(window.viewport_size().height) - 220.0).clamp(180.0, 600.0);
                panel.child(div().absolute().inset_0().flex().items_center().justify_center()
                    .bg(crate::ui::theme::rgba(ui_theme::DIALOG_OVERLAY))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(floating_panel().w(px(width)).max_h(px(height)).flex().flex_col().min_h(px(0.0))
                        .focus_trap("workflow-v2-picker-trap", &editor.document_picker_focus)
                        .child(scrollable_frame_when("workflow-v2-picker-scroll", ScrollbarMode::Vertical,
                            div().id("workflow-v2-picker-scroll").flex().flex_col().min_h(px(0.0)).max_h(px(height))
                                .overflow_y_scroll().track_scroll(&picker_scroll).child(self.render_document_step_picker(window, cx)).into_any_element(),
                            picker_scroll, true, cx))))
            })
            .into_any_element()
    }

    fn render_document_step(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let step = &editor.data.steps[index];
        let expanded = editor.data.selected_step == index;
        let title = if step.kind == WorkflowStepKind::Invoke { step.invoke.title() } else { step.kind.display_name() };
        let (glyph, kind) = step_icon(step);
        let identity = if step.kind == WorkflowStepKind::Invoke { format!("id: {}", step.invoke.value(WorkflowStepSlot::StepId)) } else { step.kind.op_name().to_string() };
        let enabled = !editor.ai_loading;
        let mut header = div().flex().items_center().min_w_0().gap_2().p_3()
            .when(expanded, |header| header.bg(rgb(ui_theme::WORKFLOW_SURFACE)))
            .child(Button::new(format!("workflow-v2-expand-{index}")).ghost().compact().flex_1().min_w(px(0.0)).h(px(32.0)).px_0().py_0()
                .accessibility_label(format!("{}步骤 {}", if expanded { "收起" } else { "展开" }, index + 1))
                .child(div().flex().items_center().justify_start().w_full().min_w_0().gap_3()
                    .child(div().flex().items_center().justify_center().w(px(32.0)).h(px(32.0)).flex_none().rounded_full()
                        .bg(rgb(ui_theme::PRIMARY_SUBTLE)).text_color(rgb(ui_theme::PRIMARY)).child(format!("{:02}", index + 1)))
                    .child(icon(glyph, if kind == "Skill" { ui_theme::PRIMARY } else { ui_theme::CONTENT_SECONDARY }))
                    .child(div().flex().flex_wrap().items_center().min_w_0().gap_2()
                        .child(text(title.to_string()).font_weight(gpui::FontWeight::SEMIBOLD)).child(hint(identity)).child(badge(kind))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(editor) = this.workflow_editor.as_mut() { editor.data.selected_step = if expanded { usize::MAX } else { index }; } cx.notify();
                })));
        if expanded {
            for (action, label, glyph, available) in [("up", "上移", IconName::ArrowUp, index > 0), ("down", "下移", IconName::ArrowDown, index + 1 < editor.data.steps.len()), ("copy", "复制", IconName::Copy, true), ("delete", "删除", IconName::Trash, true)] {
                header = header.child(icon_button(format!("workflow-v2-{action}-{index}"), if available { label } else if action == "up" { "已是第一个步骤" } else { "已是最后一个步骤" }, glyph, enabled && available)
                    .when(action == "delete", |button| button.icon(icon(glyph, ui_theme::DESTRUCTIVE)))
                    .on_click(cx.listener(move |this, _, _, cx| { this.document_step_action(index, action); cx.notify(); })));
            }
            if step.kind == WorkflowStepKind::Invoke {
                header = header.child(icon_button(format!("workflow-v2-advanced-{index}"), "高级配置", IconName::Settings2, enabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = this.workflow_editor.as_mut() { editor.document_advanced_step = if editor.document_advanced_step == Some(index) { None } else { Some(index) }; } cx.notify();
                    })));
            }
        } else {
            let entity = cx.entity();
            let count = editor.data.steps.len();
            header = header.child(icon_button(format!("workflow-v2-more-{index}"), "步骤操作", IconName::Ellipsis, enabled)
                .dropdown_menu(move |mut menu, window, _| {
                    for (action, label, available) in [("up", "上移", index > 0), ("down", "下移", index + 1 < count), ("copy", "复制", true), ("delete", "删除", true)] {
                        if available { menu = menu.item(PopupMenuItem::new(label).on_click(window.listener_for(&entity, move |this, _, _, cx| { this.document_step_action(index, action); cx.notify(); }))); }
                    } menu
                }));
        }
        header = header.child(icon_button(format!("workflow-v2-chevron-{index}"), if expanded { "收起步骤" } else { "展开步骤" }, if expanded { IconName::ChevronUp } else { IconName::ChevronDown }, true)
            .on_click(cx.listener(move |this, _, _, cx| { if let Some(editor) = this.workflow_editor.as_mut() { editor.data.selected_step = if expanded { usize::MAX } else { index }; } cx.notify(); })));
        let mut panel = card().p_0().gap_0().border_color(rgb(if expanded { ui_theme::WORKFLOW_OUTLINE_ACTIVE } else { ui_theme::WORKFLOW_OUTLINE })).child(header);
        if !expanded {
            let summary = if step.kind == WorkflowStepKind::Invoke {
                let value = match step.invoke.value(WorkflowStepSlot::Uses) { "skill.run" => step.invoke.value(WorkflowStepSlot::Task), "mcp.call" => step.invoke.value(WorkflowStepSlot::Tool), _ => step.invoke.value(WorkflowStepSlot::Uses) };
                value.lines().next().unwrap_or_default().to_string()
            } else {
                step.kind.slots().iter().filter_map(|slot| { let value = step.slot_value(*slot); (!value.is_empty()).then(|| format!("{}：{value}", slot.label())) }).collect::<Vec<_>>().join(" · ")
            };
            return panel.when(!summary.trim().is_empty(), |panel| panel.child(hint(summary).pl(px(76.0)).pr_3().pb_3())).into_any_element();
        }
        let mut body = column().p_3();
        if step.kind != WorkflowStepKind::Invoke {
            body = body.child(self.document_git_parameters(index, window, cx));
            let entity = cx.entity();
            let current_kind = step.kind;
            let width = self.document_menu_width(format!("kind-{index}"));
            let measured = width.clone();
            body = body.child(div().flex().items_center().gap_3()
                .child(text("操作类型").w(px(108.0)).flex_none())
                .child(measure_menu_field(div().flex_1().min_w_0().h(px(36.0)).child(
                    menu_button(format!("workflow-v2-kind-{index}"), step.kind.display_name(), enabled).accessibility_label("更改 Git 操作类型")
                .dropdown_menu(move |mut menu, window, _| {
                    menu = fit_menu(menu, &width, window).scrollable(true).max_h(px(240.0));
                    for kind in WorkflowStepKind::all() { menu = menu.item(PopupMenuItem::new(kind.display_name()).checked(kind == current_kind).on_click(window.listener_for(&entity, move |this, _, _, cx| { this.workflow_editor_set_step_kind(index, kind); cx.notify(); }))); } menu
                })), measured)));
            return panel.child(body).into_any_element();
        }
        for (slot, _, _) in step.invoke.parameters() {
            body = if slot == WorkflowStepSlot::Tools { body.child(self.document_tools(index, cx)) }
                else { body.child(self.document_parameter(index, slot, window, cx)) };
        }
        body = body.child(self.document_parameter(index, WorkflowStepSlot::SaveAs, window, cx))
            .when_some(self.ai_extensions.error.clone(), |body, error| body.child(hint(error)));
        if editor.document_advanced_step == Some(index) || step.invoke.parameters().is_empty() {
            for slot in [WorkflowStepSlot::StepId, WorkflowStepSlot::Uses, WorkflowStepSlot::Arguments] { body = body.child(self.document_field(WorkflowEditorFieldId::StepParam { step: index, slot }, slot.label(), window, cx)); }
            if step.invoke.parameters().iter().any(|(slot, _, _)| *slot == WorkflowStepSlot::Tools) {
                body = body.child(self.document_field(WorkflowEditorFieldId::StepParam { step: index, slot: WorkflowStepSlot::Tools }, "工具完整参数", window, cx));
            }
            body = body.child(hint("高级参数保留 browserGuard 等额外配置；表单字段优先写入。未知动作保留完整 with 对象。"));
        }
        panel = panel.child(body);
        panel.into_any_element()
    }

    fn document_step_action(&mut self, index: usize, action: &str) {
        match action { "up" => self.workflow_editor_move_step(index, true), "down" => self.workflow_editor_move_step(index, false), "copy" => self.workflow_document_duplicate(index), _ => self.workflow_editor_remove_step(index) }
    }

    fn render_document_step_picker(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let query = editor.picker_search_field.value.trim().to_lowercase();
        let mut picker = card().child(div().flex().justify_between().child(text("添加步骤").font_weight(gpui::FontWeight::SEMIBOLD))
            .child(icon_button("workflow-v2-picker-close", "返回步骤", IconName::X, true).on_click(cx.listener(|this, _, _, cx| { this.workflow_editor_close_step_picker(); cx.notify(); }))))
            .child(hint("选择下一步要做的操作，按顺序组合工作流"))
            .child(self.document_input(WorkflowEditorFieldId::PickerSearch, window, cx));
        for (uses, label, description) in [("mcp.call", "MCP 工具", "调用已配置的服务与工具"), ("js.run", "JavaScript", "转换输入、组合变量或调用工具"), ("skill.run", "AI Skill", "使用本地安装的 Skill 执行任务")] {
            if query.is_empty() || format!("{uses} {label} {description}").to_lowercase().contains(&query) {
                picker = picker.child(self.document_command(format!("workflow-v2-picker-{uses}"), label, false, true, cx)
                    .h_auto().py_3().justify_start().icon(icon(match uses { "mcp.call" => IconName::Plug, "js.run" => IconName::Braces, _ => IconName::Sparkles }, ui_theme::PRIMARY))
                    .child(hint(description))
                    .on_click(cx.listener(move |this, _, _, cx| { this.workflow_document_add_action(uses); cx.notify(); })));
            }
        }
        picker = picker.child(text("Git 操作"));
        for kind in filter_workflow_step_kinds(&query) {
            picker = picker.child(self.document_command(format!("workflow-v2-picker-{}", kind.op_name()), kind.display_name(), false, true, cx)
                .h_auto().py_3().justify_start().icon(icon(IconName::GitBranch, ui_theme::WORKFLOW_META)).child(hint(kind.description()))
                .on_click(cx.listener(move |this, _, _, cx| { this.workflow_editor_add_step_of_kind(kind); cx.notify(); })));
        }
        picker.into_any_element()
    }

    fn render_document_variables(&self, cx: &Context<Self>) -> gpui::Div {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let mut rail = card().child(text("可用变量").font_weight(gpui::FontWeight::SEMIBOLD))
            .child(hint("在输入框或任务描述中使用 ${变量名} 插入变量；输出按步骤顺序解析。"));
        for (index, row) in editor.data.inputs.iter().enumerate() {
            if !row.key.trim().is_empty() { rail = rail.child(self.document_variable_card(format!("input-{index}"), format!("${{{}}}", row.key), row.description.clone(), Some(row.default_value.clone()), cx)); }
        }
        for (index, row) in editor.data.vars.iter().enumerate() { rail = rail.child(self.document_variable_card(format!("custom-{index}"), format!("${{{}}}", row.key), "自定义变量".into(), None, cx)); }
        for (index, step) in editor.data.steps.iter().enumerate() {
            if index >= editor.data.selected_step { break; }
            let output = if step.kind == WorkflowStepKind::Invoke { step.invoke.value(WorkflowStepSlot::SaveAs) } else if step.kind == WorkflowStepKind::FilterBranches { &step.output } else { "" };
            if !output.is_empty() { rail = rail.child(self.document_variable_card(format!("output-{index}-{output}"), format!("${{out.{output}}}"), format!("步骤 {} 的输出", index + 1), None, cx)); }
        }
        rail.child(self.document_variable_card("run-source-branch".into(), "${run.sourceBranch}".into(), "运行时的来源分支名".into(), None, cx))
            .child(self.document_variable_card("git-current-branch".into(), "${git.currentBranch}".into(), "执行到当前步骤时的分支".into(), None, cx))
    }
}
