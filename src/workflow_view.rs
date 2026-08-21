use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::ui::theme::rgb;
use chrono::{DateTime, Local};
use directories::BaseDirs;
use git2::Repository;
use gpui::{
    ClickEvent, Context, IntoElement, ListSizingBehavior, Window, div, prelude::*, px, uniform_list,
};
use khaslana::{
    WorkflowDefinition, WorkflowExecutor, WorkflowInputDefinition, WorkflowPreview,
    WorkflowProgressEvent, WorkflowRunOptions, parse_workflow_json5,
};

use crate::{
    FieldId, OperationBlocker, RepositoryLoading, RepositorySnapshot, RepositoryView, ResizeTarget,
    ScrollbarMode, TextFieldState, UiEvent, scrollable_frame_when, scrollable_uniform_frame,
    send_ui_event,
    system::open_directory,
    tasks::TaskKind,
    ui::{
        components::{command_group, list_row_surface, page_header, tooltip_text},
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
const WORKFLOW_REFRESH_DISABLED_REASON: &str = "工作流运行期间无法刷新模板";

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
        self.workflow_template_dir = workflow_templates_dir();
        self.status = "正在刷新工作流模板".to_string();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = load_workflow_templates();
            send_ui_event(&tx, UiEvent::WorkflowTemplatesLoaded { result });
        });
    }

    /// 后台模板加载结果的应用端（由事件泵调用）。
    pub(crate) fn apply_workflow_templates(
        &mut self,
        result: Result<Vec<WorkflowTemplateItem>, String>,
    ) {
        match result {
            Ok(templates) => {
                let count = templates.len();
                self.workflow_templates = templates;
                // 刷新后若当前文件已不在模板目录，清掉旧选中态，避免导航误导详情对象。
                self.workflow_state.selected_template_path = workflow_selected_template_path(
                    self.workflow_state.file_path.as_deref(),
                    &self.workflow_templates,
                );
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
        let template_match = self
            .workflow_templates
            .iter()
            .find(|template| template.path == path)
            .map(|template| template.path.clone());
        self.workflow_state.definition = Some(definition);
        self.workflow_state.file_path = Some(path);
        // 外部选择的文件不借用旧模板选中态，模板列表中的文件则与已加载路径保持一致。
        self.workflow_state.selected_template_path = template_match;
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
            .bg(rgb(ui_theme::SURFACE_CANVAS))
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

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::SURFACE_BASE))
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
                                ),
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
                        )),
                ),
            )
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

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_w(px(crate::MIN_WORKFLOW_TEMPLATES_WIDTH))
            .min_h(px(0.0))
            .border_r_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::SURFACE_SUNKEN))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(px(ui_theme::SPACE_2))
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("模板导航"),
                    )
                    .child(
                        command_group()
                            .child({
                                let enabled = !self.busy;
                                div()
                                    .id("workflow-refresh-templates")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_none()
                                    .min_h(px(24.0))
                                    .px_2()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .text_size(px(11.0))
                                    .text_color(if enabled {
                                        rgb(ui_theme::CONTENT_SECONDARY)
                                    } else {
                                        rgb(ui_theme::MUTED_FOREGROUND)
                                    })
                                    .when(enabled, |el| {
                                        el.cursor_pointer()
                                            .hover(|el| el.bg(rgb(ui_theme::STATE_HOVER)))
                                    })
                                    .when(!enabled, |el| {
                                        el.cursor_not_allowed().tooltip(move |_window, cx| {
                                            tooltip_text(WORKFLOW_REFRESH_DISABLED_REASON, cx)
                                        })
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
                            )),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .truncate()
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
        let has_error = template.error.is_some();
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
                .child(template.display_name.clone()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(10.0))
                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
