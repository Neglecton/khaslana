// 代码理解页面（CU2-T5）：问题框、回答主区、读写表、来源侧栏与步骤简图。
//
// 页面按 development.md §9.1 的固定流程实现六种可见状态（空态/校验态/生成态/
// 后台分离/失败态/完成态），全部数据来自真实 `AnalysisResult` 与任务注册表，
// 不硬编码演示流程。生命周期遵守用户确认的三条固定规则：
// 1. 只有结构有效的完成结果（Completed 或明确收尾的终态 Partial）进入历史；
// 2. 显式取消不保存流式半截回答（进入 CancelPending，任务退出前不释放名额）；
// 3. 切换页面、标签或仓库只分离显示，任务继续在后台运行——本页从不调用
//    `UnderstandingSession::detach_active()`（那是旧「离页即取消」语义）。
//
// 任务注册表独立于当前主模式与标签页：所有 agent 事件先按
// `(project_key, generation)` 路由到注册表，再决定刷新当前页面、只更新
// 状态栏任务条或仅完成持久化；旧代际/错误项目事件直接丢弃并记调试日志。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::tasks::TaskKind;
use crate::ui::components::{ButtonTone, command_group, dialog_overlay, dialog_panel};
use crate::ui::icons::ToolbarIcon;
use crate::ui::theme::{self as ui_theme, rgb};
use crate::{FieldId, MainMode, RepositoryView, ResizeTarget, UiEvent, send_ui_event};
use gpui::{AnyElement, Context, IntoElement, ScrollHandle, Window, div, prelude::*, px};
use khaslana::ai::ChatClient;
use khaslana::code_index::read_index_stats;
use khaslana::code_understanding::{
    AnalysisCompletion, AnalysisEvidenceState, AnalysisResult, CallRelation, CallRelationKind,
    ChatTurnProvider, DataObjectCategory, DataOperationKind, FlowLinkKind, SourceLine,
    UNDERSTANDING_HISTORY_FORMAT_VERSION, UNDERSTANDING_HISTORY_LIST_LIMIT,
    UnderstandingAgentInput, UnderstandingAnswer, UnderstandingError, UnderstandingEvent,
    UnderstandingHistoryEntry, UnderstandingHistoryRecord, UnderstandingRequestKey,
    UnderstandingRequestTicket, UnderstandingSession, UnderstandingSourceRecord, UnderstandingStep,
    UnderstandingTools, list_understanding_history_records, save_understanding_history_record,
    validate_history_source,
};
// ── 布局常量 ──────────────────────────────────────────────
/// 宽窗来源侧栏默认宽度。
pub(crate) const UNDERSTANDING_SOURCE_DEFAULT_WIDTH: f32 = 336.0;
/// 来源侧栏可拖拽范围。
pub(crate) const UNDERSTANDING_SOURCE_MIN_WIDTH: f32 = 280.0;
pub(crate) const UNDERSTANDING_SOURCE_MAX_WIDTH: f32 = 480.0;
/// 回答可用宽度低于该值时源码替换回答主区并显示「返回回答」。
pub(crate) const UNDERSTANDING_NARROW_ANSWER_WIDTH: f32 = 720.0;
/// 来源侧栏在引用范围外额外显示的上下文行数。
const SOURCE_CONTEXT_LINES: u32 = 10;

/// S1 空态的示例问题（点击填入输入框）。
const EXAMPLE_QUESTIONS: [&str; 3] = [
    "登录逻辑是怎么实现的？调用了什么，又被谁调用？",
    "密码校验失败时会发生什么，会写哪些数据？",
    "这段流程读写了哪些表？哪些是推断出来的？",
];

// ── 页面状态模型 ──────────────────────────────────────────
/// 页面可见状态（渲染时由注册表条目推导；S2 校验态为发送动作内的瞬时检查）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnderstandingPagePhase {
    /// 未打开仓库：问题框不可发送，正文区只有「打开仓库」主动作。
    NoRepo,
    /// S1 空态/恢复态。
    Empty,
    /// S3 流式生成（当前页面正在显示该任务）。
    Running,
    /// 显式取消已请求，等待任务在轮次边界退出。
    CancelPending,
    /// 失败：保留问题与已完成步骤，可重试。
    Failed,
    /// S4 完成（Completed 或终态 Partial），或正在查看历史记录。
    Finished,
}
/// 一次问答的页面显示数据（跨页面分离保留在注册表条目里）。
#[derive(Default)]
pub(crate) struct UnderstandingDisplay {
    pub(crate) question: String,
    pub(crate) steps: Vec<UnderstandingStep>,
    pub(crate) live_reasoning: String,
    pub(crate) live_content: String,
    pub(crate) progress: Option<String>,
    pub(crate) answer: Option<UnderstandingAnswer>,
    pub(crate) error: Option<String>,
    /// 完成时任务线程写本地历史的结果；None = 未结束。
    pub(crate) saved_to_history: Option<bool>,
}
/// 来源侧栏：按需打开的一片源码（引用复验后读取）。
pub(crate) struct UnderstandingSourcePanel {
    pub(crate) relative_path: String,
    pub(crate) lines: Vec<SourceLine>,
    /// 引用复验失败时保持打开但显示失效状态，不按旧行号渲染内容。
    pub(crate) valid: bool,
    pub(crate) invalid_reason: Option<String>,
    pub(crate) scroll: ScrollHandle,
}
/// 每仓库的本地完成历史状态（只读；恢复与失效复验见 development.md §9.4）。
#[derive(Default)]
pub(crate) struct UnderstandingHistoryState {
    pub(crate) records: Vec<UnderstandingHistoryRecord>,
    pub(crate) loading: bool,
    pub(crate) loaded: bool,
    pub(crate) open: bool,
    /// 正在查看的历史记录（页面切到只读完成态）。
    pub(crate) viewing: Option<UnderstandingHistoryRecord>,
}
/// 注册表条目：一个仓库的理解会话 + 显示状态。
pub(crate) struct UnderstandingRepoState {
    pub(crate) session: UnderstandingSession,
    /// 当前（或最近一次）请求的路由键；事件代际守卫用。
    pub(crate) request: UnderstandingRequestKey,
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    pub(crate) cancel_pending: bool,
    pub(crate) display: UnderstandingDisplay,
    /// 完成态简图折叠开关（per-repo 保留）。
    pub(crate) flow_collapsed: bool,
    pub(crate) source_panel: Option<UnderstandingSourcePanel>,
    /// 「解释此处」追问聚焦的来源。
    pub(crate) focused_source: Option<khaslana::code_index::SourceRef>,
    pub(crate) history: UnderstandingHistoryState,
}
/// 独立于主模式与标签页的理解任务注册表，按仓库键（项目键）索引。
/// 首版约束：同仓库最多一个在途任务；全局 AI 名额与评审共享
/// （`ai_review_running_tasks`，上限 MAX_CONCURRENT_AI_REVIEWS）。
#[derive(Default)]
pub(crate) struct UnderstandingTaskRegistry {
    repos: HashMap<String, UnderstandingRepoState>,
}
impl UnderstandingTaskRegistry {
    pub(crate) fn get(&self, project_key: &str) -> Option<&UnderstandingRepoState> {
        self.repos.get(project_key)
    }

    pub(crate) fn get_mut(&mut self, project_key: &str) -> Option<&mut UnderstandingRepoState> {
        self.repos.get_mut(project_key)
    }

    /// 取得（或初始化）仓库条目；项目键来自仓库路径，恒非空。
    pub(crate) fn get_or_init(&mut self, project_key: &str) -> &mut UnderstandingRepoState {
        self.repos
            .entry(project_key.to_string())
            .or_insert_with(|| {
                let session =
                    UnderstandingSession::new(project_key, "primary").expect("仓库路径必然非空");
                UnderstandingRepoState {
                    session,
                    request: UnderstandingRequestKey {
                        project_key: project_key.to_string(),
                        session_key: "primary".to_string(),
                        generation: 0,
                    },
                    cancel: None,
                    cancel_pending: false,
                    display: UnderstandingDisplay::default(),
                    flow_collapsed: false,
                    source_panel: None,
                    focused_source: None,
                    history: UnderstandingHistoryState::default(),
                }
            })
    }

    /// 全部条目（状态栏任务条遍历用）。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &UnderstandingRepoState)> {
        self.repos.iter()
    }
}
/// 由页面数据推导可见状态。
pub(crate) fn understanding_page_phase(
    state: Option<&UnderstandingRepoState>,
    repo_open: bool,
) -> UnderstandingPagePhase {
    if !repo_open {
        return UnderstandingPagePhase::NoRepo;
    }
    let Some(state) = state else {
        return UnderstandingPagePhase::Empty;
    };
    if state.history.viewing.is_some() {
        return UnderstandingPagePhase::Finished;
    }
    if state.session.has_active_request() {
        return if state.cancel_pending {
            UnderstandingPagePhase::CancelPending
        } else {
            UnderstandingPagePhase::Running
        };
    }
    if state.display.answer.is_some() {
        UnderstandingPagePhase::Finished
    } else if state.display.error.is_some() {
        UnderstandingPagePhase::Failed
    } else {
        UnderstandingPagePhase::Empty
    }
}
// ── 展示纯函数（可单测） ──────────────────────────────────
/// 数据对象类别的中文标签。
pub(crate) fn data_object_category_label(category: DataObjectCategory) -> &'static str {
    match category {
        DataObjectCategory::DbObject => "数据库对象",
        DataObjectCategory::Cache => "缓存",
        DataObjectCategory::External => "外部调用",
        DataObjectCategory::Message => "消息",
        DataObjectCategory::Unknown => "未知",
    }
}
/// 单个数据操作的中文标签。
pub(crate) fn data_operation_label(operation: DataOperationKind) -> &'static str {
    match operation {
        DataOperationKind::Read => "读",
        DataOperationKind::Insert => "增",
        DataOperationKind::Update => "改",
        DataOperationKind::Delete => "删",
        DataOperationKind::Unknown => "未知",
    }
}
/// 操作列表标签（保持模型给出的顺序）。
pub(crate) fn data_operations_label(operations: &[DataOperationKind]) -> String {
    if operations.is_empty() {
        "未知".to_string()
    } else {
        operations
            .iter()
            .map(|operation| data_operation_label(*operation))
            .collect::<Vec<_>>()
            .join("/")
    }
}
/// 证据状态徽标标签。
pub(crate) fn evidence_state_label(state: AnalysisEvidenceState) -> &'static str {
    match state {
        AnalysisEvidenceState::Observed => "已读源码",
        AnalysisEvidenceState::Inferred => "推断",
        AnalysisEvidenceState::Unknown => "未知",
    }
}
/// 调用关系性质徽标。
pub(crate) fn call_relation_kind_label(kind: CallRelationKind) -> &'static str {
    match kind {
        CallRelationKind::Direct => "直接",
        CallRelationKind::Indirect => "间接",
        CallRelationKind::Candidate => "候选",
    }
}
/// 流程连接语义标签。
pub(crate) fn flow_link_kind_label(kind: FlowLinkKind) -> &'static str {
    match kind {
        FlowLinkKind::Call => "调用",
        FlowLinkKind::Branch => "分支",
        FlowLinkKind::Read => "读取",
        FlowLinkKind::Write => "写入",
        FlowLinkKind::SequenceHint => "顺序",
    }
}
/// 流程简图的线性渲染行：按回答顺序自上而下排布；相邻步骤之间的连接渲染
/// 为箭头行，非相邻（分支/循环）连接汇总在末尾，不复制循环步骤、不做图布局。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UnderstandingFlowRow {
    Step {
        title: String,
        detail: Option<String>,
        source_ids: Vec<String>,
    },
    /// 相邻步骤之间的连接（from → 下一个步骤）。
    Link {
        kind_label: &'static str,
        label: Option<String>,
    },
    /// 非相邻连接（分支/循环），显示为「来源步骤 → 目标步骤」。
    Detour {
        from_title: String,
        to_title: String,
        kind_label: &'static str,
        label: Option<String>,
    },
}
pub(crate) fn understanding_flow_rows(result: &AnalysisResult) -> Vec<UnderstandingFlowRow> {
    let mut rows = Vec::new();
    if result.steps.is_empty() {
        return rows;
    }
    let title_of = |id: &str| {
        result
            .steps
            .iter()
            .find(|step| step.id == id)
            .map(|step| step.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    for (index, step) in result.steps.iter().enumerate() {
        if index > 0 {
            let previous_id = result.steps[index - 1].id.as_str();
            // 相邻对已有显式连接就用其语义/标签；否则按回答顺序显示普通顺序行。
            match result
                .links
                .iter()
                .find(|link| link.from == previous_id && link.to == step.id)
            {
                Some(link) => rows.push(UnderstandingFlowRow::Link {
                    kind_label: flow_link_kind_label(link.kind),
                    label: link.label.clone(),
                }),
                None => rows.push(UnderstandingFlowRow::Link {
                    kind_label: flow_link_kind_label(FlowLinkKind::SequenceHint),
                    label: None,
                }),
            }
        }
        rows.push(UnderstandingFlowRow::Step {
            title: step.title.clone(),
            detail: step.detail.clone(),
            source_ids: step.source_ids.clone(),
        });
    }
    // 非相邻连接：to 不是 from 的直接后继才列出（分支 / 循环回边）。
    let step_order: HashMap<&str, usize> = result
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect();
    for link in &result.links {
        let Some(&from_index) = step_order.get(link.from.as_str()) else {
            continue;
        };
        let Some(&to_index) = step_order.get(link.to.as_str()) else {
            continue;
        };
        if to_index == from_index + 1 {
            continue;
        }
        rows.push(UnderstandingFlowRow::Detour {
            from_title: title_of(&link.from),
            to_title: title_of(&link.to),
            kind_label: flow_link_kind_label(link.kind),
            label: link.label.clone(),
        });
    }
    rows
}
/// 「复制结论」的纯文本形态：摘要 + 业务发现 + 数据读写 + 未知项 + 范围说明。
pub(crate) fn understanding_plain_text(result: &AnalysisResult) -> String {
    let mut blocks = vec![result.summary.clone()];
    if !result.findings.is_empty() {
        let findings = result
            .findings
            .iter()
            .map(|finding| {
                let condition = finding
                    .condition
                    .as_ref()
                    .map(|condition| format!("（{condition}）"))
                    .unwrap_or_default();
                format!(
                    "- {}{} [{}]",
                    finding.text,
                    condition,
                    evidence_state_label(finding.state)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("业务发现：\n{findings}"));
    }
    if !result.data_accesses.is_empty() {
        let accesses = result
            .data_accesses
            .iter()
            .map(|access| {
                let conditions = if access.conditions.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", access.conditions.join("；"))
                };
                format!(
                    "- {} · {} · {}{} [{}]",
                    access.object,
                    data_object_category_label(access.category),
                    data_operations_label(&access.operations),
                    conditions,
                    evidence_state_label(access.state)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("数据读写：\n{accesses}"));
    }
    if !result.unknowns.is_empty() {
        let unknowns = result
            .unknowns
            .iter()
            .map(|unknown| format!("- {unknown}"))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("未确认：\n{unknowns}"));
    }
    if !result.coverage_note.is_empty() {
        blocks.push(format!("检索范围：{}", result.coverage_note));
    }
    blocks.join("\n\n")
}
// ── 内部辅助 ──────────────────────────────────────────────
/// 索引库路径（与设置页同一解析：数据目录 + 仓库哈希 8 位）。
fn understanding_index_db_path(project_key: &str) -> Option<PathBuf> {
    let data_dir = khaslana::storage::active_data_dir()?;
    khaslana::code_index::open_index_db_path(
        &data_dir,
        &khaslana::ai::review_store::repo_key(project_key),
    )
    .ok()
}
/// 来源侧栏读取：复验过的引用范围 ± 上下文行。
fn read_source_lines(
    repo_root: &Path,
    index_db_path: &Path,
    source_ref: &khaslana::code_index::SourceRef,
) -> Result<(Vec<SourceLine>, bool), UnderstandingError> {
    let tools = UnderstandingTools::open(repo_root, index_db_path)?;
    let start = source_ref
        .start_line
        .saturating_sub(SOURCE_CONTEXT_LINES)
        .max(1);
    let end = source_ref.end_line.saturating_add(SOURCE_CONTEXT_LINES);
    let read = tools
        .source_service()
        .read_file(&source_ref.relative_path, start, end)?;
    Ok((read.lines, read.truncated))
}
/// 历史记录 → 会话答案条目（复用 `validate_history_source` 的复验契约）。
fn history_entry_from_record(
    record: &UnderstandingHistoryRecord,
) -> Result<UnderstandingHistoryEntry, UnderstandingError> {
    let mut sources = Vec::with_capacity(record.sources.len());
    for source in &record.sources {
        sources.push(source.to_source_ref()?);
    }
    Ok(UnderstandingHistoryEntry {
        request: UnderstandingRequestKey {
            project_key: record.project_key.clone(),
            session_key: record.session_key.clone(),
            generation: record.generation,
        },
        answer: UnderstandingAnswer {
            analysis: record.analysis.clone(),
            steps: record.steps.clone(),
            reasoning: record.reasoning.clone(),
            sources,
        },
    })
}
/// S2 / 来源动作的可行动提示种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeAction {
    None,
    OpenAiSettings,
    OpenIndexSettings,
}
// ── RepositoryView 动作 ───────────────────────────────────
impl RepositoryView {
    /// 当前仓库键（未打开仓库返回 None）。
    fn understanding_project_key(&self) -> Option<String> {
        self.repo_path
            .as_ref()
            .map(|path| path.display().to_string())
    }

    fn understanding_state(&self) -> Option<&UnderstandingRepoState> {
        let key = self.understanding_project_key()?;
        self.understanding_tasks.get(&key)
    }

    fn understanding_state_mut(&mut self) -> Option<&mut UnderstandingRepoState> {
        let key = self.understanding_project_key()?;
        self.understanding_tasks.get_mut(&key)
    }

    /// 任务是否已从当前页面分离（不在理解页，或页面显示的不是该仓库）。
    pub(crate) fn understanding_task_detached_from(&self, project_key: &str) -> bool {
        self.main_mode != MainMode::CodeUnderstanding
            || self.repo_path.as_ref().map(Path::new) != Some(Path::new(project_key))
    }

    /// S2 提交：同步校验后创建请求并派发后台任务；失败留在当前态并给出可行动提示。
    pub(crate) fn submit_understanding_question(&mut self, cx: &mut Context<Self>) {
        let Some(project_key) = self.understanding_project_key() else {
            return;
        };
        let question = self.understanding_question.value.trim().to_string();
        if question.is_empty() {
            return;
        }
        // 1) AI 配置：不配置不做无意义请求，给设置入口。
        if !self.ai_settings.is_usable() {
            self.understanding_notice = Some((
                String::from("请先在设置中配置并启用 AI 供应商"),
                NoticeAction::OpenAiSettings,
            ));
            cx.notify();
            return;
        }
        // 2) 全局 AI 名额（与评审/一次性生成共享）。
        if self.ai_review_running_tasks >= crate::MAX_CONCURRENT_AI_REVIEWS {
            self.understanding_notice = Some((
                format!(
                    "已有 {} 个 AI 任务在进行中，请等待完成或取消后再试",
                    crate::MAX_CONCURRENT_AI_REVIEWS
                ),
                NoticeAction::None,
            ));
            cx.notify();
            return;
        }
        // 3) 基础索引：工具契约依赖索引库；没有则引导建立（不要求 Java 深度分析）。
        let Some(index_db_path) = understanding_index_db_path(&project_key) else {
            self.understanding_notice = Some((
                String::from("无法定位数据目录，请重启应用后重试"),
                NoticeAction::None,
            ));
            cx.notify();
            return;
        };
        if !index_db_path.exists() {
            self.understanding_notice = Some((
                String::from("当前仓库还没有代码索引，请先建立基础索引"),
                NoticeAction::OpenIndexSettings,
            ));
            cx.notify();
            return;
        }
        // 4) 同仓库单任务：在途任务（含取消中）未退出前不允许新请求。
        if self
            .understanding_tasks
            .get(&project_key)
            .is_some_and(|state| state.session.has_active_request())
        {
            self.understanding_notice = Some((
                String::from("当前仓库已有理解任务在运行，请等待完成或取消"),
                NoticeAction::None,
            ));
            cx.notify();
            return;
        }
        let repo_root = PathBuf::from(&project_key);
        let ticket = {
            let state = self.understanding_tasks.get_or_init(&project_key);
            match state.session.begin_request(&question) {
                Ok(ticket) => ticket,
                Err(_) => {
                    self.understanding_notice = Some((
                        String::from("无法开始新的理解请求，请稍后重试"),
                        NoticeAction::None,
                    ));
                    cx.notify();
                    return;
                }
            }
        }; // 冻结本次路由身份与显示状态。
        let state = self.understanding_tasks.get_or_init(&project_key);
        state.request = ticket.key.clone();
        state.cancel = Some(ticket.cancel.clone());
        state.cancel_pending = false;
        state.display = UnderstandingDisplay {
            question: question.clone(),
            ..UnderstandingDisplay::default()
        };
        state.source_panel = None;
        state.history.viewing = None;
        self.understanding_notice = None;
        self.understanding_question.clear();
        self.status = "正在分析代码".into();
        cx.notify();
        self.spawn_understanding_task(repo_root, index_db_path, ticket, question);
    }

    /// 派发理解 agent 后台任务（Ai 池）。完成历史在任务线程落盘——即使 UI
    /// 已分离（切页面/仓库）也能保存；panic 补发该代际 Failed 精确归位名额。
    fn spawn_understanding_task(
        &mut self,
        repo_root: PathBuf,
        index_db_path: PathBuf,
        ticket: UnderstandingRequestTicket,
        question: String,
    ) {
        self.ai_review_running_tasks += 1;
        let settings = self.ai_settings.clone();
        let model = settings.model.clone();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target(&settings.normalized_base_url());
        let data_dir = khaslana::storage::active_data_dir();
        let tx = self.tx.clone();
        let started_at = Instant::now();
        let key = ticket.key.clone();
        let cancel = ticket.cancel.clone();
        let prompt_context = ticket.prompt_context.clone();
        let input = UnderstandingAgentInput {
            repo_root,
            index_db_path,
            request_id: format!("cu-{}", key.generation),
            question,
        };
        self.tasks.spawn(TaskKind::Ai, move || {
            let panic_tx = tx.clone();
            let panic_key = key.clone();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let client = ChatClient::new(settings, proxy_url);
                let provider = ChatTurnProvider::new(&client);
                let mut on_event = |event: UnderstandingEvent| match event {
                    UnderstandingEvent::Progress(message) => {
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingProgress {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                                message,
                            },
                        );
                    }
                    UnderstandingEvent::Delta { content, reasoning } => {
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingDelta {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                                content_delta: content,
                                reasoning_delta: reasoning,
                            },
                        );
                    }
                    UnderstandingEvent::Step(step) => {
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingStepAdded {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                                step,
                            },
                        );
                    }
                    // 完成事件由返回值统一发送，避免双发。
                    UnderstandingEvent::Done(_) => {}
                };
                match khaslana::code_understanding::run_understanding_agent_with_context(
                    &input,
                    &provider,
                    &cancel,
                    &prompt_context,
                    &mut on_event,
                ) {
                    Ok(Some(answer)) => {
                        // 能到达这里的都是结构有效结果（Completed 或明确收尾的
                        // Partial）；Failed/Cancelled/半截正文不会进入该分支，
                        // 也就不会写历史。
                        let finished_at_millis = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_millis() as u64)
                            .unwrap_or(0);
                        let record = UnderstandingHistoryRecord {
                            format_version: UNDERSTANDING_HISTORY_FORMAT_VERSION,
                            project_key: key.project_key.clone(),
                            session_key: key.session_key.clone(),
                            generation: key.generation,
                            question: answer.analysis.context.question.clone(),
                            completion: answer.analysis.completion,
                            analysis: answer.analysis.clone(),
                            steps: answer.steps.clone(),
                            reasoning: answer.reasoning.clone(),
                            sources: answer
                                .sources
                                .iter()
                                .map(UnderstandingSourceRecord::from_source_ref)
                                .collect(),
                            model: model.clone(),
                            started_at_millis: 0,
                            finished_at_millis,
                            duration_secs: started_at.elapsed().as_secs(),
                            index_generation: answer.analysis.context.index_generation,
                        };
                        let saved = data_dir
                            .as_ref()
                            .map(|dir| save_understanding_history_record(dir, record).is_ok())
                            .unwrap_or_else(|| {
                                tracing::warn!(
                                    target: "khaslana::code_understanding",
                                    "无法定位数据目录，代码理解历史未保存"
                                );
                                false
                            });
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingFinished {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                                answer,
                                saved,
                            },
                        );
                    }
                    Ok(None) => {
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingCancelled {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                            },
                        );
                    }
                    Err(err) => {
                        send_ui_event(
                            &tx,
                            UiEvent::UnderstandingFailed {
                                project_key: key.project_key.clone(),
                                generation: key.generation,
                                error: err.to_string(),
                            },
                        );
                    }
                }
            }));
            if let Err(payload) = outcome {
                let message = crate::tasks::panic_message(payload);
                tracing::error!(
                    target: "khaslana::code_understanding",
                    "代码理解任务 panic：{message}"
                );
                send_ui_event(
                    &panic_tx,
                    UiEvent::UnderstandingFailed {
                        project_key: panic_key.project_key.clone(),
                        generation: panic_key.generation,
                        error: format!("代码理解任务异常终止：{message}"),
                    },
                );
            }
        });
    }

    /// 显式取消：立即给「正在安全取消」反馈；名额在任务实际退出
    /// （UnderstandingCancelled 事件）后才释放。
    pub(crate) fn cancel_understanding_task_for(
        &mut self,
        project_key: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.understanding_tasks.get_mut(project_key) else {
            return;
        };
        if state.session.cancel_active().is_some() {
            state.cancel_pending = true;
            state.display.progress = Some("正在安全取消…".into());
            cx.notify();
        }
    }

    /// 状态栏任务条「查看」：切到任务仓库并进入代码理解页（恢复实时进度）。
    pub(crate) fn show_understanding_task(&mut self, project_key: String, cx: &mut Context<Self>) {
        let tab_id = self
            .tabs
            .iter()
            .find(|tab| {
                tab.repo_path
                    .as_ref()
                    .is_some_and(|path| path.display().to_string() == project_key)
            })
            .map(|tab| tab.id);
        if let Some(tab_id) = tab_id {
            self.activate_tab(tab_id);
        }
        self.set_main_mode(MainMode::CodeUnderstanding);
        cx.notify();
    }

    /// 「后台运行」：切回工作区，任务继续在后台执行（只分离显示，不取消）。
    pub(crate) fn run_understanding_in_background(&mut self, cx: &mut Context<Self>) {
        self.set_main_mode(MainMode::Worktree);
        cx.notify();
    }

    /// 切换流程简图折叠开关。
    pub(crate) fn toggle_understanding_flow(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.understanding_state_mut() {
            state.flow_collapsed = !state.flow_collapsed;
            cx.notify();
        }
    }

    // ── 事件处理（main.rs::handle_ui_event 分发；全部按 (project_key, generation) 守卫）──
    fn understanding_event_state_mut(
        &mut self,
        project_key: &str,
        generation: u64,
    ) -> Option<&mut UnderstandingRepoState> {
        let Some(state) = self.understanding_tasks.get_mut(project_key) else {
            tracing::debug!(
                target: "khaslana::code_understanding",
                "丢弃未知项目的理解事件：{project_key}"
            );
            return None;
        };
        if state.request.generation != generation {
            tracing::debug!(
                target: "khaslana::code_understanding",
                "丢弃旧代际理解事件：{project_key}#{generation}"
            );
            return None;
        }
        Some(state)
    }

    pub(crate) fn handle_understanding_progress(
        &mut self,
        project_key: String,
        generation: u64,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
            return;
        };
        state.display.progress = Some(message);
        if !detached {
            cx.notify();
        }
    }

    pub(crate) fn handle_understanding_step(
        &mut self,
        project_key: String,
        generation: u64,
        step: UnderstandingStep,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
            return;
        };
        state.display.steps.push(step);
        state.display.live_reasoning.clear();
        state.display.live_content.clear();
        if !detached {
            cx.notify();
        }
    }

    pub(crate) fn handle_understanding_delta(
        &mut self,
        project_key: String,
        generation: u64,
        content_delta: Option<String>,
        reasoning_delta: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
            return;
        };
        if let Some(content) = content_delta {
            state.display.live_content.push_str(&content);
        }
        if let Some(reasoning) = reasoning_delta {
            state.display.live_reasoning.push_str(&reasoning);
        }
        if !detached {
            cx.notify();
        }
    }

    pub(crate) fn handle_understanding_finished(
        &mut self,
        project_key: String,
        generation: u64,
        answer: UnderstandingAnswer,
        saved: bool,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        // 会话接收判定在独立作用域内完成，随后立即归还 &mut self 做名额归位。
        let accepted = {
            let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
                // 迟到完成（已取消/已换代）同样归位名额。
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                return;
            };
            let request = state.request.clone();
            state
                .session
                .finish_answer(&request, answer.clone())
                .unwrap_or(false)
        };
        self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
        if !accepted {
            return;
        }
        let Some(state) = self.understanding_tasks.get_mut(&project_key) else {
            return;
        };
        state.cancel = None;
        state.display.answer = Some(answer);
        state.display.progress = None;
        state.display.live_reasoning.clear();
        state.display.live_content.clear();
        state.display.saved_to_history = Some(saved);
        if detached {
            // 后台完成只提示一次；历史已在任务线程落盘。
            self.notify_success("代码理解已完成，可点击状态栏查看", cx);
        } else if !saved {
            self.notify_warning("回答已完成，但写入本地历史失败", cx);
        }
        cx.notify();
    }

    pub(crate) fn handle_understanding_failed(
        &mut self,
        project_key: String,
        generation: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        let accepted = {
            let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                return;
            };
            let request = state.request.clone();
            state.session.finish_error(&request)
        };
        self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
        if !accepted {
            return;
        }
        let Some(state) = self.understanding_tasks.get_mut(&project_key) else {
            return;
        };
        state.cancel = None;
        state.display.error = Some(error.clone());
        state.display.progress = None;
        state.display.live_reasoning.clear();
        state.display.live_content.clear();
        if detached {
            self.notify_error(format!("代码理解失败：{error}"), cx);
        }
        cx.notify();
    }

    pub(crate) fn handle_understanding_cancelled(
        &mut self,
        project_key: String,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let detached = self.understanding_task_detached_from(&project_key);
        let question = {
            let Some(state) = self.understanding_event_state_mut(&project_key, generation) else {
                self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
                return;
            };
            let request = state.request.clone();
            state.session.finish_cancelled(&request);
            state.cancel = None;
            state.cancel_pending = false;
            // 取消完成后保留原问题供修改或重试；不保存半截回答。
            let question = state.display.question.clone();
            state.display = UnderstandingDisplay {
                question: question.clone(),
                ..UnderstandingDisplay::default()
            };
            question
        };
        self.ai_review_running_tasks = self.ai_review_running_tasks.saturating_sub(1);
        if !detached {
            self.understanding_question.set_value(question);
        }
        cx.notify();
    }

    pub(crate) fn handle_understanding_history_loaded(
        &mut self,
        project_key: String,
        records: Vec<UnderstandingHistoryRecord>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.understanding_tasks.get_mut(&project_key) else {
            return;
        };
        state.history.records = records;
        state.history.loading = false;
        state.history.loaded = true;
        cx.notify();
    }

    // ── 历史与来源 ──
    /// 进入页面时补齐本地完成历史（懒加载一次）。
    pub(crate) fn ensure_understanding_history_loaded(&mut self) {
        let Some(project_key) = self.understanding_project_key() else {
            return;
        };
        if self
            .understanding_tasks
            .get(&project_key)
            .is_some_and(|state| state.history.loaded || state.history.loading)
        {
            return;
        }
        self.understanding_tasks
            .get_or_init(&project_key)
            .history
            .loading = true;
        let Some(data_dir) = khaslana::storage::active_data_dir() else {
            if let Some(state) = self.understanding_tasks.get_mut(&project_key) {
                state.history.loading = false;
                state.history.loaded = true;
            }
            return;
        };
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let records = list_understanding_history_records(
                &data_dir,
                &project_key,
                UNDERSTANDING_HISTORY_LIST_LIMIT,
            )
            .unwrap_or_default();
            send_ui_event(
                &tx,
                UiEvent::UnderstandingHistoryLoaded {
                    project_key,
                    records,
                },
            );
        });
    }

    /// 补齐当前仓库的索引统计投影（缓存缺失且库存在时后台读库）。
    pub(crate) fn ensure_understanding_index_stats(&mut self) {
        let Some(project_key) = self.understanding_project_key() else {
            return;
        };
        if self.code_index_stats.contains_key(&project_key) {
            return;
        }
        let Some(db_path) = understanding_index_db_path(&project_key) else {
            return;
        };
        if !db_path.exists() {
            return;
        }
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let stats = read_index_stats(&db_path).ok().flatten();
            send_ui_event(
                &tx,
                UiEvent::CodeIndexStatsLoaded {
                    repo_path: project_key,
                    stats,
                },
            );
        });
    }

    pub(crate) fn open_understanding_history(&mut self, cx: &mut Context<Self>) {
        self.ensure_understanding_history_loaded();
        if let Some(state) = self.understanding_state_mut() {
            state.history.open = true;
        }
        cx.notify();
    }

    pub(crate) fn close_understanding_history(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.understanding_state_mut() {
            state.history.open = false;
        }
        cx.notify();
    }

    /// 打开一条历史记录（进入只读完成态；来源点击仍走复验）。
    pub(crate) fn view_understanding_history_record(
        &mut self,
        finished_at_millis: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.understanding_state_mut() else {
            return;
        };
        if let Some(record) = state
            .history
            .records
            .iter()
            .find(|record| record.finished_at_millis == finished_at_millis)
            .cloned()
        {
            state.history.viewing = Some(record);
            state.history.open = false;
            state.source_panel = None;
            state.focused_source = None;
        }
        cx.notify();
    }

    pub(crate) fn close_understanding_history_view(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.understanding_state_mut() {
            state.history.viewing = None;
            state.source_panel = None;
            state.focused_source = None;
        }
        cx.notify();
    }

    /// 点击来源：先复验（项目/索引代际/文件 hash），失败显示失效状态而不是
    /// 按旧行号打开新内容。读取数据源与当前显示一致（内存答案或历史记录）。
    pub(crate) fn open_understanding_source(&mut self, source_id: String, cx: &mut Context<Self>) {
        let Some(project_key) = self.understanding_project_key() else {
            return;
        };
        let repo_root = PathBuf::from(&project_key);
        let Some(index_db_path) = understanding_index_db_path(&project_key) else {
            return;
        };
        let viewing = self
            .understanding_tasks
            .get(&project_key)
            .and_then(|state| state.history.viewing.clone());
        let has_answer = self
            .understanding_tasks
            .get(&project_key)
            .is_some_and(|state| state.display.answer.is_some() || state.history.viewing.is_some());
        if !has_answer {
            self.understanding_notice =
                Some((String::from("没有可打开的回答来源"), NoticeAction::None));
            cx.notify();
            return;
        }
        let outcome = if let Some(record) = viewing.as_ref() {
            // 历史来源：重建答案条目后复验（文件 hash / 索引代际 / 项目归属）。
            history_entry_from_record(record).and_then(|entry| {
                validate_history_source(&repo_root, &index_db_path, &entry, &source_id)
            })
        } else {
            UnderstandingTools::open(&repo_root, &index_db_path)
                .and_then(|tools| tools.validate_source(&source_id))
        };
        let (panel, focused) = match outcome {
            Ok(source_ref) => match read_source_lines(&repo_root, &index_db_path, &source_ref) {
                Ok((lines, truncated)) => {
                    let relative_path = source_ref.relative_path.clone();
                    let panel = UnderstandingSourcePanel {
                        relative_path,
                        lines,
                        valid: true,
                        invalid_reason: truncated
                            .then(|| "该来源超过单次读取上限，已截断显示".to_string()),
                        scroll: ScrollHandle::default(),
                    };
                    (Some(panel), Some(source_ref))
                }
                Err(error) => (Some(invalid_source_panel(error.to_string())), None),
            },
            Err(error) => (Some(invalid_source_panel(error.to_string())), None),
        };
        let state = self.understanding_tasks.get_or_init(&project_key);
        state.source_panel = panel;
        state.focused_source = focused;
        cx.notify();
    }

    pub(crate) fn close_understanding_source(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.understanding_state_mut() {
            state.source_panel = None;
            state.focused_source = None;
        }
        cx.notify();
    }

    /// 「解释此处」：把当前来源设为追问聚焦范围（携带选中范围的新问题）。
    pub(crate) fn focus_understanding_source(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.understanding_state_mut() else {
            return;
        };
        let Some(source_ref) = state.focused_source.clone() else {
            return;
        };
        if state.session.select_source(Some(source_ref)).is_err() {
            return;
        }
        cx.notify();
    }

    /// 「复制结论」：把当前显示的答案复制为纯文本。
    pub(crate) fn copy_understanding_conclusion(&mut self, cx: &mut Context<Self>) {
        let result = self.understanding_state().and_then(|state| {
            state
                .history
                .viewing
                .as_ref()
                .map(|record| record.analysis.clone())
                .or_else(|| {
                    state
                        .display
                        .answer
                        .as_ref()
                        .map(|answer| answer.analysis.clone())
                })
        });
        let Some(result) = result else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(understanding_plain_text(
            &result,
        )));
        self.notify_success("已复制结论", cx);
    }

    /// 打开 AI 设置（S2 校验失败的可行动提示）。
    pub(crate) fn open_understanding_ai_settings(&mut self, cx: &mut Context<Self>) {
        self.select_settings_category(crate::SettingsCategory::Ai);
        cx.notify();
    }

    /// 打开索引设置（无索引引导）。
    pub(crate) fn open_understanding_index_settings(&mut self, cx: &mut Context<Self>) {
        self.select_settings_category(crate::SettingsCategory::CodeIndex);
        cx.notify();
    }
}
/// 来源失效面板（保留来源按钮位置，显示失效原因与重新分析入口）。
fn invalid_source_panel(reason: String) -> UnderstandingSourcePanel {
    UnderstandingSourcePanel {
        relative_path: String::new(),
        lines: Vec::new(),
        valid: false,
        invalid_reason: Some(reason),
        scroll: ScrollHandle::default(),
    }
}
// ── 页面渲染 ──────────────────────────────────────────────
/// 长文本的单行摘要（时间线/状态栏任务条摘要展示用）。
pub(crate) fn excerpt_line(text: &str, max_chars: usize) -> String {
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        single_line.chars().take(max_chars).collect::<String>() + "…"
    }
}
/// 历史记录完成时间（本地时区）。
fn understanding_time_label(millis: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis as i64)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
/// 证据状态徽标（浅底深字，随主题切换）。
fn evidence_state_badge(state: AnalysisEvidenceState) -> impl IntoElement {
    let (label, bg, fg) = match state {
        AnalysisEvidenceState::Observed => (
            "已读源码",
            ui_theme::FEEDBACK_SUCCESS_BG,
            ui_theme::FEEDBACK_SUCCESS_TEXT,
        ),
        AnalysisEvidenceState::Inferred => (
            "推断",
            ui_theme::FEEDBACK_INFO_BG,
            ui_theme::FEEDBACK_INFO_TEXT,
        ),
        AnalysisEvidenceState::Unknown => (
            "未知",
            ui_theme::FEEDBACK_WARNING_BG,
            ui_theme::FEEDBACK_WARNING_TEXT,
        ),
    };
    crate::ui::components::status_pill_badge(label, bg, fg)
}
fn completion_badge(completion: AnalysisCompletion) -> impl IntoElement {
    let (label, bg, fg) = match completion {
        AnalysisCompletion::Complete => (
            "已完成",
            ui_theme::FEEDBACK_SUCCESS_BG,
            ui_theme::FEEDBACK_SUCCESS_TEXT,
        ),
        AnalysisCompletion::Partial => (
            "部分完成",
            ui_theme::FEEDBACK_WARNING_BG,
            ui_theme::FEEDBACK_WARNING_TEXT,
        ),
    };
    crate::ui::components::status_pill_badge(label, bg, fg)
}
impl RepositoryView {
    /// 当前仓库来源侧栏宽度（布局字段，默认 336）。
    pub(crate) fn understanding_source_width(&self) -> f32 {
        self.column_width(ResizeTarget::UnderstandingSource)
    }

    /// 页头副标题：分支 + 索引状态投影。
    fn understanding_index_status_text(&self) -> String {
        let Some(repo_path) = self.repo_path.as_ref() else {
            return "未打开仓库".to_string();
        };
        let key = repo_path.display().to_string();
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.clone())
            .unwrap_or_else(|| "未知分支".to_string());
        let indexing = self
            .code_index_task
            .as_ref()
            .is_some_and(|task| task.repo_path == key);
        let index_part = if indexing {
            "索引建立中".to_string()
        } else if let Some(stats) = self.code_index_stats.get(&key) {
            if self.code_index_enabled_cache.contains(&key) {
                format!("已索引 {} 文件 · {} 符号", stats.files, stats.symbols)
            } else {
                format!("索引已停用（已有 {} 文件数据）", stats.files)
            }
        } else {
            "未建立代码索引".to_string()
        };
        format!("{branch} · {index_part}")
    }

    pub(crate) fn render_code_understanding_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let project_key = self.understanding_project_key();
        let state = project_key
            .as_deref()
            .and_then(|key| self.understanding_tasks.get(key));
        let phase = understanding_page_phase(state, self.repo_path.is_some()); // 宽窄布局：中央区宽度 ≈ 视口 - 导航列；回答可用宽度不足时源码替换主区。
        let viewport_width: f32 = window.viewport_size().width.into();
        let presentation = self.context_navigator_presentation(window);
        let nav_width = if presentation == crate::chrome_view::ContextNavigatorPresentation::Docked
        {
            self.column_width(ResizeTarget::Sidebar)
        } else {
            ui_theme::NAVIGATOR_COLLAPSED_WIDTH
        };
        let central_width = (viewport_width - nav_width).max(0.0);
        let source_width = self.understanding_source_width();
        let panel_open = state.is_some_and(|state| state.source_panel.is_some());
        let narrow = panel_open && central_width - source_width < UNDERSTANDING_NARROW_ANSWER_WIDTH;
        let index_line = self.understanding_index_status_text();
        let history_open = state.is_some_and(|state| state.history.open);
        let mut page = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(self.render_understanding_header(&index_line, cx))
            .child(self.render_understanding_notice(cx));
        if narrow && panel_open {
            // 窄窗：源码替换回答主区（「返回回答」在侧栏头部）。
            page = page.child(
                self.render_understanding_source_panel(true, cx)
                    .into_any_element(),
            );
        } else {
            page = page.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(
                        self.render_understanding_body(window, phase, cx)
                            .into_any_element(),
                    )
                    .when(panel_open, |this| {
                        this.child(
                            self.render_column_splitter(ResizeTarget::UnderstandingSource, cx),
                        )
                        .child(
                            self.render_understanding_source_panel(false, cx)
                                .into_any_element(),
                        )
                    }),
            );
        }
        page.when(history_open, |this| {
            this.child(self.render_understanding_history_overlay(cx))
        })
    }

    fn render_understanding_header(
        &self,
        index_line: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 副标题是动态字符串（分支/索引状态），不走 page_header 的
        // `&'static str` description 槽位，结构与 page_header 保持一致。
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(ui_theme::SPACE_3))
            .min_h(px(40.0))
            .px(px(ui_theme::SPACE_4))
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                div()
                    .min_w(px(0.0))
                    .flex()
                    .items_baseline()
                    .gap(px(ui_theme::SPACE_2))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_PAGE_TITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("代码理解"),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(ui_theme::TYPE_BODY))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(index_line.to_string()),
                    ),
            )
            .child(
                command_group()
                    .child(self.app_button(
                        "历史",
                        Some(ToolbarIcon::History),
                        None,
                        ButtonTone::Neutral,
                        true,
                        |this, _window, cx| this.open_understanding_history(cx),
                        cx,
                    ))
                    .child(self.app_button(
                        "索引设置",
                        Some(ToolbarIcon::Search),
                        None,
                        ButtonTone::Neutral,
                        true,
                        |this, _window, cx| this.open_understanding_index_settings(cx),
                        cx,
                    )),
            )
    }

    /// S2 提示条：可行动错误（AI 未配置 / 无索引 / 名额满）。
    fn render_understanding_notice(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((message, action)) = self.understanding_notice.as_ref() else {
            return div().into_any_element();
        };
        let action_button = match action {
            NoticeAction::OpenAiSettings => Some(
                self.app_button(
                    "打开 AI 设置",
                    None,
                    None,
                    ButtonTone::Neutral,
                    true,
                    |this, _window, cx| this.open_understanding_ai_settings(cx),
                    cx,
                )
                .into_any_element(),
            ),
            NoticeAction::OpenIndexSettings => Some(
                self.app_button(
                    "打开索引设置",
                    None,
                    None,
                    ButtonTone::Neutral,
                    true,
                    |this, _window, cx| this.open_understanding_index_settings(cx),
                    cx,
                )
                .into_any_element(),
            ),
            NoticeAction::None => None,
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(ui_theme::SPACE_2))
            .px(px(ui_theme::SPACE_4))
            .py(px(ui_theme::SPACE_2))
            .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
            .border_b_1()
            .border_color(rgb(ui_theme::FEEDBACK_WARNING_BORDER))
            .text_size(px(ui_theme::TYPE_BODY))
            .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
            .child(div().flex_1().min_w(px(0.0)).child(message.clone()))
            .children(action_button)
            .into_any_element()
    }

    fn render_understanding_body(
        &self,
        window: &Window,
        phase: UnderstandingPagePhase,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match phase {
            UnderstandingPagePhase::NoRepo => {
                self.render_understanding_no_repo(cx).into_any_element()
            }
            UnderstandingPagePhase::Empty => self
                .render_understanding_empty(window, cx)
                .into_any_element(),
            UnderstandingPagePhase::Running | UnderstandingPagePhase::CancelPending => {
                self.render_understanding_running(cx).into_any_element()
            }
            UnderstandingPagePhase::Failed => self
                .render_understanding_failed(window, cx)
                .into_any_element(),
            UnderstandingPagePhase::Finished => self
                .render_understanding_finished(window, cx)
                .into_any_element(),
        }
    }
    /// S3 生成态 / CancelPending：问题复显 + 进度 + 步骤时间线 + 动作条。
    fn render_understanding_running(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.understanding_state() else {
            return div().into_any_element();
        };
        let display = &state.display;
        let question = display.question.clone();
        let cancel_pending = state.cancel_pending;
        let handle = self.scroll_handle("understanding-answer-scroll");
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(
                div()
                    .id("understanding-running-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ui_theme::SPACE_3))
                            .p(px(ui_theme::SPACE_4))
                            .child(self.render_question_card(&question))
                            .children(self.render_understanding_timeline()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(ui_theme::SPACE_2))
                    .px(px(ui_theme::SPACE_4))
                    .py(px(ui_theme::SPACE_2))
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::SURFACE_BASE))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(if cancel_pending {
                                "正在安全取消：任务将在工具或模型轮次边界退出"
                            } else {
                                "切换页面或仓库不会中断任务，可点「后台运行」先去处理其他工作"
                            }),
                    )
                    .child(
                        command_group()
                            .child(self.app_button(
                                "后台运行",
                                None,
                                None,
                                ButtonTone::Neutral,
                                !cancel_pending,
                                |this, _window, cx| this.run_understanding_in_background(cx),
                                cx,
                            ))
                            .child(self.app_button(
                                if cancel_pending {
                                    "取消中…"
                                } else {
                                    "取消"
                                },
                                None,
                                None,
                                ButtonTone::Neutral,
                                !cancel_pending,
                                |this, _window, cx| {
                                    let project_key = this
                                        .repo_path
                                        .as_ref()
                                        .map(|path| path.display().to_string());
                                    if let Some(project_key) = project_key {
                                        this.cancel_understanding_task_for(&project_key, cx);
                                    }
                                },
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_question_card(&self, question: &str) -> AnyElement {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .px(px(ui_theme::SPACE_4))
            .py(px(ui_theme::SPACE_3))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_META))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("已提交的问题"),
            )
            .child(
                div()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .rounded(px(ui_theme::RADIUS_SM))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::SURFACE_SUNKEN))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(question.to_string()),
            )
            .into_any_element()
    }

    /// 生成态时间线：已落定步骤 + 当前轮 live 文本。只显示面向用户的动作。
    fn render_understanding_timeline(&self) -> Vec<AnyElement> {
        let Some(state) = self.understanding_state() else {
            return Vec::new();
        };
        let display = &state.display;
        let mut rows = Vec::new();
        for (index, step) in display.steps.iter().enumerate() {
            rows.push(self.render_understanding_step_row(step, index));
        }
        if !display.live_reasoning.is_empty() {
            let reasoning = display.live_reasoning.clone();
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ui_theme::SPACE_1))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child("思考中…"),
                    )
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child(excerpt_line(&reasoning, 220)),
                    )
                    .into_any_element(),
            );
        }
        if !display.live_content.is_empty() {
            let content = display.live_content.clone();
            rows.push(
                div()
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(excerpt_line(&content, 400))
                    .into_any_element(),
            );
        }
        if let Some(progress) = display.progress.as_ref() {
            let progress = progress.clone();
            rows.push(
                div()
                    .text_size(px(ui_theme::TYPE_META))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .child(progress)
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_understanding_step_row(&self, step: &UnderstandingStep, _index: usize) -> AnyElement {
        let row = match step {
            UnderstandingStep::Reasoning { text } => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(format!("思考：{}", excerpt_line(text, 60))),
                )
                .child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                        .child(excerpt_line(text, 160)),
                ),
            UnderstandingStep::Message { text } => div()
                .text_size(px(ui_theme::TYPE_BODY))
                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                .child(text.clone()),
            UnderstandingStep::ToolCall {
                args_summary,
                result_excerpt,
                error,
                ..
            } => {
                let (dot_color, result_color) = if *error {
                    (ui_theme::GIT_REMOVED, ui_theme::FEEDBACK_ERROR_TEXT)
                } else {
                    (ui_theme::GIT_ADDED, ui_theme::CONTENT_TERTIARY)
                };
                let summary = excerpt_line(args_summary, 120);
                let excerpt = excerpt_line(result_excerpt, 180);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ui_theme::SPACE_1))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ui_theme::SPACE_2))
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(dot_color)))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(ui_theme::TYPE_BODY))
                                    .font_family("Consolas, monospace")
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(summary),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(ui_theme::SPACE_4))
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(result_color))
                            .child(excerpt),
                    )
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_1))
            .px(px(ui_theme::SPACE_3))
            .py(px(ui_theme::SPACE_2))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(row)
            .into_any_element()
    }

    /// Failed：错误卡 + 保留的问题与已完成步骤 + 重试。
    fn render_understanding_failed(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.understanding_state() else {
            return div().into_any_element();
        };
        let question = state.display.question.clone();
        let error = state.display.error.clone().unwrap_or_default();
        let steps = state.display.steps.clone();
        let handle = self.scroll_handle("understanding-answer-scroll");
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(
                div()
                    .id("understanding-failed-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ui_theme::SPACE_3))
                            .p(px(ui_theme::SPACE_4))
                            .child(self.render_question_card(&question))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(ui_theme::SPACE_2))
                                    .p(px(ui_theme::SPACE_3))
                                    .rounded(px(ui_theme::RADIUS_SM))
                                    .bg(rgb(ui_theme::FEEDBACK_ERROR_BG))
                                    .border_1()
                                    .border_color(rgb(ui_theme::FEEDBACK_ERROR_BORDER))
                                    .child(
                                        div()
                                            .text_size(px(ui_theme::TYPE_BODY))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                                            .child("分析失败，已保留问题与已完成步骤"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(ui_theme::TYPE_META))
                                            .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                                            .child(excerpt_line(&error, 400)),
                                    )
                                    .child(self.app_button(
                                        "重试",
                                        None,
                                        None,
                                        ButtonTone::Primary,
                                        true,
                                        |this, _window, cx| this.retry_understanding_question(cx),
                                        cx,
                                    )),
                            )
                            .children(
                                steps
                                    .iter()
                                    .enumerate()
                                    .map(|(index, step)| {
                                        self.render_understanding_step_row(step, index)
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    ),
            )
            .child(self.render_understanding_input_area(window, cx))
            .into_any_element()
    }

    /// 失败重试：以失败问题原文创建新代际（失败结果不写历史）。
    pub(crate) fn retry_understanding_question(&mut self, cx: &mut Context<Self>) {
        let Some(question) = self
            .understanding_state()
            .map(|state| state.display.question.clone())
        else {
            return;
        };
        if question.trim().is_empty() {
            return;
        }
        self.understanding_question.set_value(question);
        self.submit_understanding_question(cx);
    }

    /// S4 完成态（内存答案或历史记录）：固定顺序呈现
    /// 摘要/流程 → 调用上下游 → 数据读写 → 其他副作用与未知项 → 来源。
    fn render_understanding_finished(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.understanding_state() else {
            return div().into_any_element();
        };
        // 数据源三选一：历史查看记录 → 内存答案；二者都无则回退空态内容。
        let viewing = state.history.viewing.clone();
        let saved_label = state.display.saved_to_history;
        let flow_collapsed = state.flow_collapsed;
        let (question, answer, sources) = if let Some(record) = viewing.as_ref() {
            (
                record.analysis.context.question.clone(),
                record.analysis.clone(),
                record.sources.clone(),
            )
        } else if let Some(memory) = state.display.answer.as_ref() {
            let sources = memory
                .sources
                .iter()
                .map(UnderstandingSourceRecord::from_source_ref)
                .collect();
            (
                memory.analysis.context.question.clone(),
                memory.analysis.clone(),
                sources,
            )
        } else {
            return div().into_any_element();
        };
        let is_history = viewing.is_some();
        let answer_scroll_id = if is_history {
            "understanding-history-answer-scroll"
        } else {
            "understanding-answer-scroll"
        };
        let completion = answer.completion;
        let analysis_step_count = viewing
            .as_ref()
            .map(|record| record.steps.len())
            .unwrap_or(state.display.steps.len());
        let answer_meta = format!(
            "{} 个来源 · {} 个分析步骤",
            sources.len(),
            analysis_step_count
        );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            // Pencil 稿中的已提交问题固定在回答区顶部，不随长回答滚走。
            .child(self.render_question_card(&question))
            .child(div().flex_none().h(px(1.0)).bg(rgb(ui_theme::BORDER_MUTED)))
            .child(
                div()
                    .id(answer_scroll_id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle("understanding-answer-scroll"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ui_theme::SPACE_4))
                            .p(px(ui_theme::SPACE_4))
                            // ── 完成状态与次级动作 ──
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(ui_theme::SPACE_2))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(ui_theme::SPACE_2))
                                            .child(
                                                div()
                                                    .text_size(px(ui_theme::TYPE_META))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(rgb(
                                                        ui_theme::FEEDBACK_SUCCESS_TEXT,
                                                    ))
                                                    .child("分析完成"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(ui_theme::TYPE_META))
                                                    .font_family("Consolas, monospace")
                                                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                                    .child(answer_meta),
                                            )
                                            .when_some(saved_label, |this, saved| {
                                                this.child(
                                                    div()
                                                        .text_size(px(ui_theme::TYPE_META))
                                                        .text_color(rgb(if saved {
                                                            ui_theme::CONTENT_TERTIARY
                                                        } else {
                                                            ui_theme::FEEDBACK_WARNING_TEXT
                                                        }))
                                                        .child(if saved {
                                                            "已保存"
                                                        } else {
                                                            "历史保存失败"
                                                        }),
                                                )
                                            })
                                            .when(is_history, |this| {
                                                this.child(
                                                    div()
                                                        .text_size(px(ui_theme::TYPE_META))
                                                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                                        .child("历史记录（只读）"),
                                                )
                                            }),
                                    )
                                    .child(
                                        command_group()
                                            .child(self.app_button(
                                                "复制结论",
                                                None,
                                                None,
                                                ButtonTone::Neutral,
                                                true,
                                                |this, _window, cx| {
                                                    this.copy_understanding_conclusion(cx);
                                                },
                                                cx,
                                            ))
                                            .when(is_history, |this| {
                                                this.child(self.app_button(
                                                    "关闭历史",
                                                    None,
                                                    None,
                                                    ButtonTone::Neutral,
                                                    true,
                                                    |this, _window, cx| {
                                                        this.close_understanding_history_view(cx);
                                                    },
                                                    cx,
                                                ))
                                            }),
                                    ),
                            )
                            // ── 摘要 ──
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(ui_theme::SPACE_2))
                                    .p(px(ui_theme::SPACE_3))
                                    .rounded(px(ui_theme::RADIUS_SM))
                                    .bg(rgb(ui_theme::SURFACE_SUNKEN))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(ui_theme::TYPE_BODY))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                                    .child("结论概述"),
                                            )
                                            .child(completion_badge(completion)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(ui_theme::TYPE_BODY))
                                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                            .child(answer.summary.clone()),
                                    ),
                            )
                            // ── 业务流程 ──
                            .when(!answer.steps.is_empty(), |this| {
                                this.child(self.render_flow_section(&answer, flow_collapsed, cx))
                            })
                            // ── 调用上下游 ──
                            .when(
                                !answer.callers.is_empty() || !answer.callees.is_empty(),
                                |this| {
                                    this.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(ui_theme::SPACE_2))
                                            .child(self.render_section_title("调用上下游"))
                                            .when(!answer.callers.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .text_size(px(ui_theme::TYPE_META))
                                                        .text_color(rgb(
                                                            ui_theme::CONTENT_SECONDARY,
                                                        ))
                                                        .child("被这些代码调用"),
                                                )
                                                .children(answer.callers.iter().enumerate().map(
                                                    |(index, relation)| {
                                                        self.render_call_relation_row(
                                                            relation, index, cx,
                                                        )
                                                    },
                                                ))
                                            })
                                            .when(!answer.callees.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .text_size(px(ui_theme::TYPE_META))
                                                        .text_color(rgb(
                                                            ui_theme::CONTENT_SECONDARY,
                                                        ))
                                                        .child("调用了这些代码"),
                                                )
                                                .children(answer.callees.iter().enumerate().map(
                                                    |(index, relation)| {
                                                        self.render_call_relation_row(
                                                            relation, index, cx,
                                                        )
                                                    },
                                                ))
                                            }),
                                    )
                                },
                            )
                            // ── 数据读写 ──
                            .when(!answer.data_accesses.is_empty(), |this| {
                                this.child(self.render_data_section(&answer, cx))
                            })
                            // ── 其他副作用与未知项 ──
                            .when(
                                !answer.unknowns.is_empty() || !answer.coverage_note.is_empty(),
                                |this| this.child(self.render_unknowns_section(&answer)),
                            )
                            // ── 来源 ──
                            .when(!sources.is_empty(), |this| {
                                this.child(self.render_sources_section(&sources, cx))
                            }),
                    ),
            )
            // 追问输入（历史只读查看时同样允许基于该内容继续提问）。
            .child(self.render_understanding_input_area(window, cx))
            .into_any_element()
    }

    fn render_section_title(&self, title: &'static str) -> AnyElement {
        div()
            .flex_none()
            .text_size(px(ui_theme::TYPE_BODY))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
            .pb(px(ui_theme::SPACE_1))
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(title)
            .into_any_element()
    }

    /// 流程简图：按 Pencil 稿横向排列步骤卡；空间不足时横向滚动，
    /// 非相邻分支与回边保留在下方摘要中。
    fn render_flow_section(
        &self,
        answer: &AnalysisResult,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let branch_count = answer
            .links
            .iter()
            .filter(|link| matches!(link.kind, FlowLinkKind::Branch))
            .count();
        let flow_meta = if branch_count == 0 {
            format!("{} 步", answer.steps.len())
        } else {
            format!("{} 步 · {} 个分支", answer.steps.len(), branch_count)
        };
        let header = div()
            .id("understanding-flow-toggle")
            .flex()
            .items_center()
            .justify_between()
            .pb(px(ui_theme::SPACE_1))
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.toggle_understanding_flow(cx);
            }))
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("业务步骤"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(ui_theme::SPACE_2))
                    .text_size(px(ui_theme::TYPE_META))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(flow_meta)
                    .child(if collapsed { "展开" } else { "收起" }),
            );
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .child(header);
        if collapsed {
            return section.into_any_element();
        }
        let mut step_elements = Vec::new();
        for (index, step) in answer.steps.iter().enumerate() {
            if index > 0 {
                let previous = &answer.steps[index - 1];
                let link = answer
                    .links
                    .iter()
                    .find(|link| link.from == previous.id && link.to == step.id);
                let kind_label = link
                    .map(|link| flow_link_kind_label(link.kind))
                    .unwrap_or_else(|| flow_link_kind_label(FlowLinkKind::SequenceHint));
                let link_label = link.and_then(|link| link.label.clone());
                step_elements.push(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(ui_theme::SPACE_1))
                        .w(px(54.0))
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                        .child("→")
                        .child(kind_label)
                        .when_some(link_label, |this, label| {
                            this.child(
                                div()
                                    .max_w(px(54.0))
                                    .truncate()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(label),
                            )
                        })
                        .into_any_element(),
                );
            }
            let first_source = step.source_ids.first().cloned();
            step_elements.push(
                div()
                    .id(format!("understanding-flow-step-{index}"))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(ui_theme::SPACE_2))
                    .w(px(180.0))
                    .min_h(px(88.0))
                    .p(px(ui_theme::SPACE_3))
                    .rounded(px(ui_theme::RADIUS_SM))
                    .bg(rgb(if index == 0 {
                        ui_theme::PRIMARY_SUBTLE
                    } else {
                        ui_theme::SURFACE_BASE
                    }))
                    .border_1()
                    .border_color(rgb(if index == 0 {
                        ui_theme::PRIMARY
                    } else {
                        ui_theme::BORDER_MUTED
                    }))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_BODY))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(format!("{}. {}", index + 1, step.title)),
                    )
                    .when_some(step.detail.clone(), |this, detail| {
                        this.child(
                            div()
                                .text_size(px(ui_theme::TYPE_META))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(detail),
                        )
                    })
                    .when_some(first_source, |this, source_id| {
                        this.child(self.render_source_chip(
                            format!("understanding-flow-src-{index}"),
                            "查看来源",
                            source_id,
                            cx,
                        ))
                    })
                    .into_any_element(),
            );
        }
        let detours = understanding_flow_rows(answer)
            .into_iter()
            .filter_map(|row| match row {
                UnderstandingFlowRow::Detour {
                    from_title,
                    to_title,
                    kind_label,
                    label,
                } => Some(match label {
                    Some(label) => format!("{from_title} —{kind_label}（{label}）→ {to_title}"),
                    None => format!("{from_title} —{kind_label}→ {to_title}"),
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        section = section
            .child(
                div()
                    .id("understanding-flow-horizontal")
                    .flex()
                    .items_center()
                    .gap(px(ui_theme::SPACE_2))
                    .overflow_x_scroll()
                    .children(step_elements),
            )
            .when(!detours.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(ui_theme::SPACE_1))
                        .p(px(ui_theme::SPACE_2))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                        .children(detours),
                )
            });
        section.into_any_element()
    }

    fn render_call_relation_row(
        &self,
        relation: &CallRelation,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = call_relation_kind_label(relation.kind);
        let first_source = relation.source_ids.first().cloned();
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_1))
            .px(px(ui_theme::SPACE_3))
            .py(px(ui_theme::SPACE_2))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(ui_theme::SPACE_2))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(ui_theme::TYPE_BODY))
                            .font_family("Consolas, monospace")
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(relation.name.clone()),
                    )
                    .child(crate::ui::components::status_pill_badge(
                        kind,
                        ui_theme::SURFACE_SUNKEN,
                        ui_theme::CONTENT_SECONDARY,
                    )),
            )
            .when_some(relation.detail.as_ref(), |this, detail| {
                this.child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(detail.clone()),
                )
            })
            .when_some(relation.relative_path.as_ref(), |this, path| {
                this.child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .font_family("Consolas, monospace")
                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                        .child(path.clone()),
                )
            })
            .when_some(first_source, |this, source_id| {
                this.child(self.render_source_chip(
                    format!("understanding-call-src-{index}"),
                    "查看来源",
                    source_id,
                    cx,
                ))
            })
            .into_any_element()
    }

    /// 数据读写区：按 Pencil 稿使用紧凑表格，类别通过行底色与标签区分，
    /// 避免每条记录都变成一张独立卡片。
    fn render_data_section(&self, answer: &AnalysisResult, cx: &mut Context<Self>) -> AnyElement {
        let rows = answer
            .data_accesses
            .iter()
            .enumerate()
            .map(|(index, access)| {
                let conditions = if access.conditions.is_empty() {
                    access
                        .access_method
                        .clone()
                        .unwrap_or_else(|| "—".to_string())
                } else {
                    access.conditions.join("；")
                };
                let first_source = access.source_ids.first().cloned();
                let row_bg = if access.category == DataObjectCategory::DbObject {
                    ui_theme::SURFACE_BASE
                } else {
                    ui_theme::FEEDBACK_SUCCESS_BG
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(ui_theme::SPACE_2))
                    .min_h(px(40.0))
                    .px(px(ui_theme::SPACE_3))
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(row_bg))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .w(px(160.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(ui_theme::TYPE_META))
                                    .font_family("Consolas, monospace")
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(access.object.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                    .child(data_object_category_label(access.category)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(96.0))
                            .truncate()
                            .text_size(px(ui_theme::TYPE_META))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(data_operations_label(&access.operations)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(conditions),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .w(px(104.0))
                            .when_some(first_source.clone(), |this, source_id| {
                                this.child(self.render_source_chip(
                                    format!("understanding-data-src-{index}"),
                                    "查看来源",
                                    source_id,
                                    cx,
                                ))
                            })
                            .when(first_source.is_none(), |this| {
                                this.child(evidence_state_badge(access.state))
                            }),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(ui_theme::RADIUS_SM))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .min_h(px(36.0))
                            .px(px(ui_theme::SPACE_3))
                            .bg(rgb(ui_theme::SURFACE_SUNKEN))
                            .child(
                                div()
                                    .text_size(px(ui_theme::TYPE_BODY))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child("数据读写"),
                            )
                            .child(
                                div()
                                    .text_size(px(ui_theme::TYPE_META))
                                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                    .child("数据库、缓存与外部调用分开标记"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ui_theme::SPACE_2))
                            .min_h(px(28.0))
                            .px(px(ui_theme::SPACE_3))
                            .bg(rgb(ui_theme::SURFACE_BASE))
                            .text_size(px(ui_theme::TYPE_META))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child(div().flex_none().w(px(160.0)).child("对象"))
                            .child(div().flex_none().w(px(96.0)).child("操作"))
                            .child(div().flex_1().min_w(px(0.0)).child("发生条件"))
                            .child(div().flex_none().w(px(104.0)).text_right().child("来源")),
                    )
                    .children(rows),
            )
            .when(
                answer
                    .data_accesses
                    .iter()
                    .any(|access| access.state != AnalysisEvidenceState::Observed),
                |this| {
                    this.child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child("没有直接源码证据的记录会在“来源”列显示推断或未知状态。"),
                    )
                },
            )
            .into_any_element()
    }

    fn render_unknowns_section(&self, answer: &AnalysisResult) -> AnyElement {
        let unknowns = answer
            .unknowns
            .iter()
            .map(|unknown| {
                div()
                    .flex()
                    .gap(px(ui_theme::SPACE_2))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("·".to_string())
                    .child(unknown.clone())
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .child(self.render_section_title("未确认与范围"))
            .when(!unknowns.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child("以下内容无法由源码确认，AI 未编造结论："),
                )
                .children(unknowns)
            })
            .when(!answer.coverage_note.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                        .child(answer.coverage_note.clone()),
                )
            })
            .into_any_element()
    }

    /// 来源区：全部来源 chip 按钮；点击打开来源侧栏。
    fn render_sources_section(
        &self,
        sources: &[UnderstandingSourceRecord],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chips = sources
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                let source_id = record.source_id();
                if source_id.is_empty() {
                    return None;
                }
                let label = format!(
                    "{}:{}-{}",
                    record.relative_path, record.start_line, record.end_line
                );
                Some(self.render_dynamic_source_chip(
                    format!("understanding-source-{index}"),
                    label,
                    source_id,
                    cx,
                ))
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .child(self.render_section_title("来源"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(ui_theme::SPACE_1))
                    .children(chips),
            )
            .into_any_element()
    }

    /// 来源 chip 文字按钮（动态标签版本）。
    fn render_dynamic_source_chip(
        &self,
        id: String,
        label: String,
        source_id: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = format!("cu-chip-{id}");
        div()
            .id(element_id)
            .flex_none()
            .max_w(px(360.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .px(px(ui_theme::SPACE_2))
            .py(px(ui_theme::SPACE_1))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
            .text_size(px(ui_theme::TYPE_META))
            .font_family("Consolas, monospace")
            .text_color(rgb(ui_theme::PRIMARY))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::STATE_SELECTION)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_understanding_source(source_id.clone(), cx);
            }))
            .child(label)
            .into_any_element()
    }

    /// 来源 chip 文字按钮（每个来源唯一元素 id）。
    fn render_source_chip(
        &self,
        id: String,
        label: &'static str,
        source_id: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = format!("cu-chip-{id}");
        div()
            .id(element_id)
            .flex_none()
            .px(px(ui_theme::SPACE_2))
            .py(px(ui_theme::SPACE_1))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
            .text_size(px(ui_theme::TYPE_META))
            .font_family("Consolas, monospace")
            .text_color(rgb(ui_theme::PRIMARY))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::STATE_SELECTION)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_understanding_source(source_id.clone(), cx);
            }))
            .child(label)
            .into_any_element()
    }

    /// 来源侧栏：宽窗为固定宽列，窄窗（`full`）替换回答主区。
    fn render_understanding_source_panel(&self, full: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.understanding_state() else {
            return div().into_any_element();
        };
        let Some(panel) = state.source_panel.as_ref() else {
            return div().into_any_element();
        };
        let has_focused = state.focused_source.is_some();
        let relative_path = panel.relative_path.clone();
        let valid = panel.valid;
        let invalid_reason = panel.invalid_reason.clone();
        let lines = panel.lines.clone();
        let scroll = panel.scroll.clone();
        let file_name = Path::new(&relative_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(if relative_path.is_empty() {
                "来源"
            } else {
                relative_path.as_str()
            })
            .to_string();
        let parent_path = Path::new(&relative_path)
            .parent()
            .and_then(|path| path.to_str())
            .filter(|path| !path.is_empty())
            .unwrap_or("项目根目录")
            .replace('\\', "/");
        let range_label = state
            .focused_source
            .as_ref()
            .map(|source| format!("L{}–{}", source.start_line, source.end_line));
        let width = if full {
            None
        } else {
            Some(self.understanding_source_width())
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .when_some(width, |this, width| this.w(px(width)))
            .when(full, |this| this.flex_1())
            .border_l_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::SURFACE_BASE))
            // 头部：文件名 + 返回/关闭
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(ui_theme::SPACE_2))
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(ui_theme::TYPE_META))
                            .font_family("Consolas, monospace")
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(file_name),
                    )
                    .child(command_group().child(self.app_button(
                        if full { "返回回答" } else { "关闭" },
                        None,
                        None,
                        ButtonTone::Neutral,
                        true,
                        |this, _window, cx| this.close_understanding_source(cx),
                        cx,
                    ))),
            )
            // 路径与引用范围独立成轻量信息条，避免长路径挤压标题和操作。
            .when(!relative_path.is_empty(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(ui_theme::SPACE_2))
                        .px(px(ui_theme::SPACE_3))
                        .py(px(ui_theme::SPACE_2))
                        .bg(rgb(ui_theme::SURFACE_SUNKEN))
                        .border_b_1()
                        .border_color(rgb(ui_theme::BORDER_MUTED))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .truncate()
                                .text_size(px(ui_theme::TYPE_META))
                                .font_family("Consolas, monospace")
                                .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                .child(parent_path),
                        )
                        .when_some(range_label, |this, label| {
                            this.child(
                                div()
                                    .flex_none()
                                    .px(px(ui_theme::SPACE_2))
                                    .py(px(2.0))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                                    .text_size(px(ui_theme::TYPE_META))
                                    .font_family("Consolas, monospace")
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .child(label),
                            )
                        }),
                )
            })
            // 失效状态条
            .when(!valid, |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(ui_theme::SPACE_2))
                        .m(px(ui_theme::SPACE_3))
                        .p(px(ui_theme::SPACE_3))
                        .rounded(px(ui_theme::RADIUS_SM))
                        .bg(rgb(ui_theme::FEEDBACK_WARNING_BG))
                        .border_1()
                        .border_color(rgb(ui_theme::FEEDBACK_WARNING_BORDER))
                        .child(
                            div()
                                .text_size(px(ui_theme::TYPE_BODY))
                                .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                                .child("来源已失效，不能按旧行号打开新内容"),
                        )
                        .when_some(invalid_reason, |this, reason| {
                            this.child(
                                div()
                                    .text_size(px(ui_theme::TYPE_META))
                                    .text_color(rgb(ui_theme::FEEDBACK_WARNING_TEXT))
                                    .child(reason),
                            )
                        })
                        .child(self.app_button(
                            "重新分析",
                            None,
                            None,
                            ButtonTone::Primary,
                            true,
                            |this, _window, cx| this.retry_understanding_question(cx),
                            cx,
                        )),
                )
            })
            // 内容：行号 + 文本（等宽）
            .when(valid, |this| {
                this.child(
                    div()
                        .id("understanding-source-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .py(px(ui_theme::SPACE_2))
                                .children(lines.iter().map(|line| self.render_source_line(line))),
                        ),
                )
            })
            // 底部：解释此处（追问聚焦）
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ui_theme::SPACE_1))
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(if valid {
                                ui_theme::FEEDBACK_SUCCESS_TEXT
                            } else {
                                ui_theme::CONTENT_TERTIARY
                            }))
                            .child(if valid { "✓" } else { "·" })
                            .child(if valid {
                                "文件与行号已复验"
                            } else {
                                "来源等待重新分析"
                            }),
                    )
                    .child(self.app_button(
                        "解释此处",
                        None,
                        None,
                        ButtonTone::Neutral,
                        has_focused,
                        |this, _window, cx| this.focus_understanding_source(cx),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_source_line(&self, line: &SourceLine) -> AnyElement {
        let number = line.line_number.to_string();
        let text = line.text.clone();
        div()
            .flex()
            .flex_row()
            .min_h(px(18.0))
            .flex_none()
            .child(
                div()
                    .flex_none()
                    .w(px(48.0))
                    .px(px(ui_theme::SPACE_2))
                    .text_size(px(ui_theme::TYPE_META))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                    .text_right()
                    .child(number),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .pr(px(ui_theme::SPACE_3))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(if text.is_empty() {
                        " ".to_string()
                    } else {
                        text
                    }),
            )
            .into_any_element()
    }

    /// 历史弹窗：本地完成记录列表（最近 20 条，坏文件已在读取时跳过）。
    fn render_understanding_history_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.understanding_state() else {
            return div().into_any_element();
        };
        let records = state.history.records.clone();
        let loading = state.history.loading;
        dialog_overlay()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                this.close_understanding_history(cx);
            }))
            .child(
                dialog_panel("本地完成历史")
                    .w(px(560.0))
                    .child(
                        div()
                            .id("understanding-history-list-scroll")
                            .flex()
                            .flex_col()
                            .gap(px(ui_theme::SPACE_2))
                            .max_h(px(420.0))
                            .overflow_y_scroll()
                            .when(loading, |this| {
                                this.child(
                                    div()
                                        .text_size(px(ui_theme::TYPE_BODY))
                                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                        .child("正在加载…"),
                                )
                            })
                            .when(!loading && records.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(ui_theme::TYPE_BODY))
                                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                        .child("暂无完成记录"),
                                )
                            })
                            .children(records.iter().map(|record| {
                                let millis = record.finished_at_millis;
                                let question = record.analysis.context.question.clone();
                                let completion = record.completion;
                                let model = record.model.clone();
                                div()
                                    .id(format!("understanding-history-{millis}"))
                                    .flex()
                                    .flex_col()
                                    .gap(px(ui_theme::SPACE_1))
                                    .px(px(ui_theme::SPACE_3))
                                    .py(px(ui_theme::SPACE_2))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .border_1()
                                    .border_color(rgb(ui_theme::BORDER_MUTED))
                                    .cursor_pointer()
                                    .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.view_understanding_history_record(millis, cx);
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(ui_theme::SPACE_2))
                                            .child(
                                                div()
                                                    .min_w(px(0.0))
                                                    .flex_1()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(ui_theme::TYPE_BODY))
                                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                                    .child(question),
                                            )
                                            .child(completion_badge(completion)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(ui_theme::TYPE_META))
                                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                            .child(format!(
                                                "{} · {model}",
                                                understanding_time_label(millis)
                                            )),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child(
                                "仅保存结构有效的完成结果（含明确标出未完成范围的 partial）；按仓库隔离，保留最近 30 条，不做云同步。",
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_understanding_no_repo(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(ui_theme::SPACE_3))
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_TITLE))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("打开仓库后开始提问"),
            )
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("代码理解基于当前仓库的索引与源码回答业务问题"),
            )
            .child(self.app_button(
                "打开仓库…",
                Some(ToolbarIcon::Open),
                None,
                ButtonTone::Primary,
                true,
                |this, _window, cx| {
                    this.browse_open();
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }

    /// S1 空态：欢迎说明、示例问题、最近完成记录与问题输入。
    fn render_understanding_empty(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let handle = self.scroll_handle("understanding-answer-scroll");
        let recent = self
            .understanding_state()
            .map(|state| {
                state
                    .history
                    .records
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .id("understanding-empty-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ui_theme::SPACE_4))
                    .p(px(ui_theme::SPACE_6))
                    .max_w(px(760.0))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_TITLE))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("用一句话问业务逻辑"),
                    )
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_BODY))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(
                                "例如“登录逻辑怎么实现的？”——AI 会检索代码索引、按需阅读源码，给出入口、调用上下游、数据读写与来源，可继续追问。",
                            ),
                    )
                    .child(self.render_example_questions(cx))
                    .when(!recent.is_empty(), |this| {
                        this.child(self.render_recent_history(recent, cx))
                    })
                    .child(self.render_understanding_input_area(window, cx)),
            )
            .into_any_element()
    }

    fn render_example_questions(&self, cx: &mut Context<Self>) -> AnyElement {
        let chips = EXAMPLE_QUESTIONS
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let question = question.to_string();
                let question_for_click = question.clone();
                div()
                    .id(format!("understanding-example-{index}"))
                    .flex_none()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .rounded(px(ui_theme::RADIUS_PILL))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::SURFACE_BASE))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .cursor_pointer()
                    .hover(|this| {
                        this.bg(rgb(ui_theme::STATE_HOVER))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.understanding_question
                            .set_value(question_for_click.clone());
                        cx.notify();
                    }))
                    .child(question)
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_wrap()
            .gap(px(ui_theme::SPACE_2))
            .children(chips)
            .into_any_element()
    }

    /// 空态的最近完成记录（点击进入只读完成态）。
    fn render_recent_history(
        &self,
        records: Vec<UnderstandingHistoryRecord>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = records
            .into_iter()
            .map(|record| {
                let millis = record.finished_at_millis;
                let question = record.analysis.context.question.clone();
                let completion = record.completion;
                let model = record.model.clone();
                div()
                    .id(format!("understanding-recent-{millis}"))
                    .flex()
                    .flex_col()
                    .gap(px(ui_theme::SPACE_1))
                    .w_full()
                    .px(px(ui_theme::SPACE_3))
                    .py(px(ui_theme::SPACE_2))
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::SURFACE_BASE))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.view_understanding_history_record(millis, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ui_theme::SPACE_2))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(ui_theme::TYPE_BODY))
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(question),
                            )
                            .child(completion_badge(completion)),
                    )
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child(format!("{} · {model}", understanding_time_label(millis))),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_META))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("最近完成记录"),
            )
            .children(rows)
            .into_any_element()
    }

    /// 问题输入区（S1/Failed/Finished 共用；运行中不渲染）。
    fn render_understanding_input_area(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_send = !self.understanding_question.value.trim().is_empty();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(ui_theme::SPACE_2))
            .p(px(ui_theme::SPACE_3))
            .rounded(px(ui_theme::RADIUS_SM))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                div()
                    .text_size(px(ui_theme::TYPE_META))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("向当前仓库提问"),
            )
            .child(self.multi_line_input(FieldId::CodeUnderstandingQuestion, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(ui_theme::SPACE_2))
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child("Enter 发送 · Shift+Enter 换行；仅发送回答所需的代码片段"),
                    )
                    .child(self.app_button(
                        "发送",
                        None,
                        None,
                        ButtonTone::Primary,
                        can_send,
                        |this, _window, cx| this.submit_understanding_question(cx),
                        cx,
                    )),
            )
            .into_any_element()
    }
}
