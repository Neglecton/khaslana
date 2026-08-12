use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, thread};

use crate::ui::theme::rgb;
use chrono::{DateTime, Local};
use directories::BaseDirs;
use git2::Repository;
use gpui::{ClickEvent, Context, IntoElement, Window, div, prelude::*, px};
use khaslana::{
    WorkflowDefinition, WorkflowExecutor, WorkflowInputDefinition, WorkflowPreview,
    WorkflowProgressEvent, WorkflowRunOptions, parse_workflow_json5,
};

use crate::{
    FieldId, OperationBlocker, RepositoryLoading, RepositorySnapshot, RepositoryView, ResizeTarget,
    ScrollbarMode, TextFieldState, UiEvent, placeholder_row, scrollable_frame_when,
    section_header_action, send_ui_event,
    system::open_directory,
    tasks::TaskKind,
    ui::{
        components::{dialog_actions, section_title},
        theme as ui_theme,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct WorkflowInputFieldState {
    key: String,
    label: String,
    description: Option<String>,
    required: bool,
    field: TextFieldState,
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
    display_name: String,
    file_name: String,
    modified_label: String,
    error: Option<String>,
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
    pub(crate) fn browse_workflow_file(&mut self) {
        self.status = "正在选择工作流文件".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .add_filter("JSON5 工作流", &["json5", "jsonc"])
                .pick_file();
            send_ui_event(&tx, UiEvent::WorkflowFileSelected { path });
        });
    }

    pub(crate) fn refresh_workflow_templates(&mut self) {
        self.workflow_template_dir = workflow_templates_dir();
        match load_workflow_templates() {
            Ok(templates) => {
                let count = templates.len();
                self.workflow_templates = templates;
                self.last_error = None;
                self.status = format!("已刷新，共 {count} 个工作流模板");
            }
            Err(err) => {
                self.workflow_templates.clear();
                // 目录读取失败不作为严重错误上报，仅记录状态；
                // 用户可通过"打开目录"或"选择文件"验证目录是否可用
                self.status = err;
            }
        }
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

    pub(crate) fn clear_workflow_file(&mut self) {
        self.workflow_state.definition = None;
        self.workflow_state.preview = None;
        self.workflow_state.file_path = None;
        self.workflow_state.inputs.clear();
        self.workflow_state.log.clear();
        self.last_error = None;
    }

    pub(crate) fn load_workflow_file(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
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
        let inputs = match self.build_workflow_inputs(&definition, &repo_path, cx) {
            Ok(inputs) => inputs,
            Err(err) => {
                self.last_error = Some(err.to_string());
                return;
            }
        };
        self.workflow_state.definition = Some(definition);
        self.workflow_state.file_path = Some(path);
        self.workflow_state.inputs = inputs;
        self.workflow_state.log.clear();
        self.status = "工作流已加载".to_string();
        self.last_error = None;
        self.refresh_workflow_preview();
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
        self.workflow_state.log.clear();
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let options = WorkflowRunOptions {
            default_remote: self.current_remote().unwrap_or_else(|| "origin".into()),
            input_vars: self.workflow_input_values(),
        };
        self.apply_status_event(Some(tab_id), |this| {
            this.repository_load_id = this.repository_load_id.wrapping_add(1);
            this.loading = RepositoryLoading::default();
            this.busy = true;
            this.operation_blocker = OperationBlocker::Modal;
            this.status = "正在运行工作流".to_string();
            this.last_error = None;
        });
        self.tasks.spawn(TaskKind::Long, move || {
            let result =
                (|| -> khaslana::Result<(RepositorySnapshot, Vec<WorkflowLogEntry>, String)> {
                    let mut repo = Repository::open(repo_path)?;
                    let mut log: Vec<WorkflowLogEntry> = Vec::new();
                    let result = WorkflowExecutor::new(&service).run(
                        &mut repo,
                        &definition,
                        options,
                        |event| {
                            let entry = workflow_progress_entry(&event);
                            log.push(entry.clone());
                            send_ui_event(&tx, UiEvent::WorkflowProgress { tab_id, entry });
                        },
                    )?;
                    let message =
                        format!("工作流“{}”已完成（{} 步）", result.name, result.steps_run);
                    Ok((result.snapshot, log, message))
                })();
            match result {
                Ok((snapshot, log, message)) => {
                    send_ui_event(
                        &tx,
                        UiEvent::WorkflowFinished {
                            tab_id,
                            message,
                            snapshot,
                            log,
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::WorkflowProgress {
                            tab_id,
                            entry: WorkflowLogEntry {
                                message: format!("工作流失败：{err}"),
                                details: Vec::new(),
                            },
                        },
                    );
                    send_ui_event(
                        &tx,
                        UiEvent::OperationFailed {
                            tab_id: Some(tab_id),
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn render_workflow_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
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
        let workflow_name = self
            .workflow_state
            .preview
            .as_ref()
            .map(|preview| preview.name.clone())
            .unwrap_or_else(|| "选择或双击模板后显示预览".to_string());

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(section_header_action(
                "工作流详情",
                Some(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(self.button(
                            "选择文件",
                            !self.busy,
                            |this, _, _| this.browse_workflow_file(),
                            cx,
                        ))
                        .child(self.button(
                            "清空",
                            self.workflow_state.definition.is_some() && !self.busy,
                            |this, _, _| this.clear_workflow_file(),
                            cx,
                        ))
                        .into_any_element(),
                ),
            ))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::PRIMARY))
                            .font_weight(gpui::FontWeight::BOLD)
                            .truncate()
                            .child(workflow_name),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .truncate()
                            .child(file_label),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_3()
                    .gap_3()
                    .child(self.render_workflow_inputs(window, cx))
                    .child(self.render_workflow_preview(cx))
                    .child(self.render_workflow_log(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .pb_3()
                    .child(dialog_actions().child(self.primary_button(
                        if self.busy { "运行中..." } else { "运行" },
                        self.workflow_state.definition.is_some() && !self.busy,
                        |this, _, _| this.run_workflow(),
                        cx,
                    ))),
            )
    }

    fn render_workflow_template_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dir_label = self
            .workflow_template_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "无法定位模板目录".to_string());
        let rows = if self.workflow_templates.is_empty() {
            vec![
                placeholder_row("暂无工作流模板。可以把 .json5/.jsonc 放入模板目录。")
                    .into_any_element(),
            ]
        } else {
            self.workflow_templates
                .iter()
                .map(|template| self.workflow_template_row(template, cx).into_any_element())
                .collect::<Vec<_>>()
        };
        let content_present = !self.workflow_templates.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.workflow_templates_width))
            .min_w(px(0.0))
            .min_h(px(0.0))
            .border_r_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(section_header_action(
                "工作流模板",
                Some(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child({
                            let enabled = !self.busy;
                            div()
                                .id("workflow-refresh-templates")
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_none()
                                .min_h(px(28.0))
                                .px_3()
                                .py_1()
                                .border_1()
                                .border_color(rgb(ui_theme::BORDER))
                                .rounded(px(ui_theme::RADIUS_XS))
                                .bg(rgb(ui_theme::ACCENT))
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .text_size(px(12.0))
                                .when(enabled, |el| el.cursor_pointer())
                                .when(!enabled, |el| el.cursor_not_allowed().opacity(0.78))
                                .when(enabled, |el| {
                                    el.hover(|el| el.bg(rgb(ui_theme::SECONDARY)))
                                        .active(|el| el.opacity(0.82))
                                })
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    if enabled {
                                        this.refresh_workflow_templates();
                                        cx.notify();
                                    }
                                }))
                                .child("刷新")
                        })
                        .child(self.button(
                            "目录",
                            !self.busy,
                            |this, _, _| this.open_workflow_template_dir(),
                            cx,
                        ))
                        .into_any_element(),
                ),
            ))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(dir_label),
            )
            .child({
                let handle = self.scroll_handle("workflow-template-list");
                let content = div()
                    .id("workflow-template-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_2()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    "workflow-template-list",
                    ScrollbarMode::Vertical,
                    content,
                    handle,
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
        let has_error = template.error.is_some();
        let selected = self
            .workflow_state
            .selected_template_path
            .as_ref()
            .is_some_and(|selected| selected == &template.path);
        div()
            .id(format!("workflow-template-{}", template.path.display()))
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::BORDER)
            })
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else {
                rgb(ui_theme::CARD)
            })
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.workflow_state.selected_template_path = Some(click_path.clone());
                if event.standard_click() && event.click_count() >= 2 && !this.busy {
                    this.load_workflow_file(click_path.clone(), cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .font_weight(if selected {
                        gpui::FontWeight::BOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if has_error {
                        rgb(ui_theme::DESTRUCTIVE)
                    } else {
                        rgb(ui_theme::FOREGROUND)
                    })
                    .child(template.display_name.clone()),
            )
            .child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
    }

    pub(crate) fn workflow_input_field(&self, index: usize) -> &TextFieldState {
        &self.workflow_state.inputs[index].field
    }

    pub(crate) fn workflow_input_field_mut(&mut self, index: usize) -> &mut TextFieldState {
        &mut self.workflow_state.inputs[index].field
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
        WorkflowExecutor::new(&service).preview(&repo, definition, &options)
    }

    fn render_workflow_inputs(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.workflow_state.inputs.is_empty() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded_sm()
            .p_3()
            .child(section_title("变量输入"))
            .children(
                self.workflow_state
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(index, input)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(12.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
                                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                        .child(description),
                                )
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }

    fn render_workflow_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .workflow_state
            .preview
            .as_ref()
            .map(|preview| {
                preview
                    .steps
                    .iter()
                    .map(|step| {
                        // summary 列改为纵向容器：标题行 + 明细子行（如筛选命中的分支清单）。
                        let summary_col = div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .truncate()
                                    .child(step.summary.clone()),
                            )
                            .children(
                                step.details
                                    .iter()
                                    .map(|detail| {
                                        div()
                                            .pl_4()
                                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                            .truncate()
                                            .child(detail.clone())
                                    })
                                    .collect::<Vec<_>>(),
                            );
                        div()
                            .flex()
                            .flex_none()
                            .items_start()
                            .gap_2()
                            .px_2()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .bg(rgb(ui_theme::CARD))
                            .text_size(px(12.0))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(46.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child(format!("#{}", step.index + 1)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(110.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .child(step.op),
                            )
                            .child(summary_col)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![placeholder_row("暂无工作流预览").into_any_element()]);
        let content_present = self
            .workflow_state
            .preview
            .as_ref()
            .is_some_and(|preview| !preview.steps.is_empty());

        div()
            .flex()
            .flex_col()
            .min_h(px(170.0))
            .flex_1()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded_sm()
            .child(section_title("步骤预览"))
            .child({
                let handle = self.scroll_handle("workflow-preview-list");
                let content = div()
                    .id("workflow-preview-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    "workflow-preview-list",
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    content_present,
                    cx,
                )
            })
    }

    fn render_workflow_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = if self.workflow_state.log.is_empty() {
            vec![placeholder_row("运行后显示步骤日志").into_any_element()]
        } else {
            self.workflow_state
                .log
                .iter()
                .map(|entry| {
                    // 明细行（如逐个删除/命中的分支）以缩进子行渲染。
                    let detail_rows = entry
                        .details
                        .iter()
                        .map(|detail| {
                            div()
                                .pl_4()
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(detail.clone())
                        })
                        .collect::<Vec<_>>();
                    div()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .bg(rgb(ui_theme::CARD))
                        .child(entry.message.clone())
                        .children(detail_rows)
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };
        let content_present = !self.workflow_state.log.is_empty();

        div()
            .flex()
            .flex_col()
            .min_h(px(130.0))
            .flex_1()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded_sm()
            .child(section_title("运行日志"))
            .child({
                let handle = self.scroll_handle("workflow-log-list");
                let content = div()
                    .id("workflow-log-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(rows)
                    .into_any_element();
                scrollable_frame_when(
                    "workflow-log-list",
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    content_present,
                    cx,
                )
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
        WorkflowProgressEvent::Finished { name, total } => WorkflowLogEntry {
            message: format!("工作流“{name}”已完成（{total} 步）"),
            details: Vec::new(),
        },
    }
}

/// 工作流模板目录，统一指向便携数据目录下的 `workflows/` 子目录。
pub(crate) fn workflow_templates_dir() -> Option<PathBuf> {
    khaslana::portable_database_dir().map(|dir| dir.join("workflows"))
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
    ensure_workflow_templates_dir(&dir).map_err(|err| format!("工作流模板目录创建失败：{err}"))?;
    // 一次性便携迁移：便携目录下没有模板文件、且旧目录存在模板时，把旧模板拷贝过来。
    migrate_legacy_workflow_templates(&dir);
    load_workflow_templates_from_dir(&dir)
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
    let entries = fs::read_dir(dir).map_err(|err| format!("工作流模板目录读取失败：{err}"))?;
    let mut templates = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| format!("工作流模板目录读取失败：{err}"))?;
        let path = entry.path();
        if !is_workflow_template_path(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
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
