//! 代码理解的多轮会话与请求生命周期（CU2-T4）。
//!
//! 本模块只负责内存态会话，不接 UI、不持久化答案：保留最近 5 次完成结果，
//! 发送给模型时最多取最近 3 次摘要并限制为 8K 字符。请求路由键同时携带
//! 项目、标签会话与请求代际，迟到事件和已分离请求不会回填当前会话。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::code_index::SourceRef;

use super::agent::{UnderstandingAnswer, UnderstandingEvent};
use super::analysis::AnalysisCompletion;
use super::source::source_id_of;
use super::tools::UnderstandingTools;
use super::{ErrorCode, UnderstandingError, UnderstandingResult};

/// 会话内保留的完成答案数量。
pub const UNDERSTANDING_SESSION_HISTORY_LIMIT: usize = 5;
/// 回灌给模型的最近答案数量。
pub const UNDERSTANDING_PROMPT_HISTORY_LIMIT: usize = 3;
/// 回灌给模型的历史摘要字符预算。
pub const UNDERSTANDING_PROMPT_HISTORY_MAX_CHARS: usize = 8_000;

/// 请求的稳定路由键；`generation` 是会话内请求代际，不是索引代际。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UnderstandingRequestKey {
    pub project_key: String,
    pub session_key: String,
    pub generation: u64,
}

/// 可向模型回灌的一次历史摘要。历史只作待核对背景，不替代重新读取源码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnderstandingHistorySummary {
    pub question: String,
    pub summary: String,
    pub scope: String,
    pub completion: AnalysisCompletion,
}

/// 单次请求冻结的追问上下文。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnderstandingPromptContext {
    pub history: Vec<UnderstandingHistorySummary>,
    pub selected_source: Option<SourceRef>,
}

impl UnderstandingPromptContext {
    /// 构造有界历史文本；优先保留最新摘要，再按时间顺序呈现。
    pub(crate) fn history_text(&self) -> String {
        let mut remaining = UNDERSTANDING_PROMPT_HISTORY_MAX_CHARS;
        let mut blocks = Vec::new();
        for summary in self
            .history
            .iter()
            .rev()
            .take(UNDERSTANDING_PROMPT_HISTORY_LIMIT)
        {
            if remaining == 0 {
                break;
            }
            if !blocks.is_empty() {
                let separator_chars = "\n\n".chars().count();
                if remaining <= separator_chars {
                    break;
                }
                remaining -= separator_chars;
            }
            let completion = match summary.completion {
                AnalysisCompletion::Complete => "complete",
                AnalysisCompletion::Partial => "partial",
            };
            let block = format!(
                "上一问：{}\n上一答摘要：{}\n当时检索范围：{}\n完成状态：{}",
                summary.question, summary.summary, summary.scope, completion
            );
            let clipped: String = block.chars().take(remaining).collect();
            remaining = remaining.saturating_sub(clipped.chars().count());
            blocks.push(clipped);
        }
        blocks.reverse();
        blocks.join("\n\n")
    }
}

/// 会话保存的一次完成答案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnderstandingHistoryEntry {
    pub request: UnderstandingRequestKey,
    pub answer: UnderstandingAnswer,
}

/// 纯服务层会话状态；页面展示可在 T5 映射为对应视觉状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnderstandingSessionStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Stale,
}

/// 开始请求时返回的冻结票据。取消标志必须等任务实际退出后才释放。
#[derive(Clone, Debug)]
pub struct UnderstandingRequestTicket {
    pub key: UnderstandingRequestKey,
    pub cancel: Arc<AtomicBool>,
    pub prompt_context: UnderstandingPromptContext,
}

/// 携带路由身份的 agent 事件。
pub struct UnderstandingSessionEvent {
    pub request: UnderstandingRequestKey,
    pub event: UnderstandingEvent,
}

struct ActiveRequest {
    key: UnderstandingRequestKey,
    cancel: Arc<AtomicBool>,
    accepts_events: bool,
}

/// 单项目、单标签页的代码理解会话。
pub struct UnderstandingSession {
    project_key: String,
    session_key: String,
    next_generation: u64,
    status: UnderstandingSessionStatus,
    active: Option<ActiveRequest>,
    history: VecDeque<UnderstandingHistoryEntry>,
    selected_source: Option<SourceRef>,
}

impl UnderstandingSession {
    pub fn new(
        project_key: impl Into<String>,
        session_key: impl Into<String>,
    ) -> UnderstandingResult<Self> {
        let project_key = project_key.into();
        let session_key = session_key.into();
        if project_key.trim().is_empty() || session_key.trim().is_empty() {
            return Err(UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                "代码理解会话需要非空的项目键和标签会话键",
            ));
        }
        Ok(Self {
            project_key,
            session_key,
            next_generation: 0,
            status: UnderstandingSessionStatus::Idle,
            active: None,
            history: VecDeque::new(),
            selected_source: None,
        })
    }

    pub fn project_key(&self) -> &str {
        &self.project_key
    }

    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    pub fn status(&self) -> UnderstandingSessionStatus {
        self.status
    }

    pub fn history(&self) -> &VecDeque<UnderstandingHistoryEntry> {
        &self.history
    }

    pub fn has_active_request(&self) -> bool {
        self.active.is_some()
    }

    pub fn selected_source(&self) -> Option<&SourceRef> {
        self.selected_source.as_ref()
    }

    /// 设置后续问题聚焦的源码范围；只接收当前项目的来源。
    pub fn select_source(&mut self, source: Option<SourceRef>) -> UnderstandingResult<()> {
        if let Some(source) = source.as_ref()
            && source.project_key != self.project_key
        {
            return Err(UnderstandingError::new(
                ErrorCode::OutsideProject,
                "选中的源码来源不属于当前项目",
            ));
        }
        self.selected_source = source;
        Ok(())
    }

    /// 开始新问题。已有任务即使已请求取消，也必须等实际退出后才能开始下一问。
    pub fn begin_request(
        &mut self,
        question: &str,
    ) -> UnderstandingResult<UnderstandingRequestTicket> {
        if question.trim().is_empty() {
            return Err(UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                "代码理解问题不能为空",
            ));
        }
        if self.active.is_some() {
            return Err(UnderstandingError::new(
                ErrorCode::IndexBusy,
                "当前代码理解请求尚未退出，请等待取消完成后再提问",
            ));
        }
        self.next_generation = self.next_generation.checked_add(1).ok_or_else(|| {
            UnderstandingError::new(ErrorCode::AnswerInvalid, "代码理解请求代际已耗尽")
        })?;
        let key = UnderstandingRequestKey {
            project_key: self.project_key.clone(),
            session_key: self.session_key.clone(),
            generation: self.next_generation,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let prompt_context = self.prompt_context();
        self.active = Some(ActiveRequest {
            key: key.clone(),
            cancel: Arc::clone(&cancel),
            accepts_events: true,
        });
        self.status = UnderstandingSessionStatus::Running;
        Ok(UnderstandingRequestTicket {
            key,
            cancel,
            prompt_context,
        })
    }

    /// 用户取消：立即停止接收后续事件，但仍保留 active，直到工作线程确认退出。
    pub fn cancel_active(&mut self) -> Option<UnderstandingRequestKey> {
        let active = self.active.as_mut()?;
        active.cancel.store(true, Ordering::Relaxed);
        active.accepts_events = false;
        self.status = UnderstandingSessionStatus::Cancelled;
        Some(active.key.clone())
    }

    /// 切项目或离页：取消后台请求并将其标为 stale，迟到消息不得回填。
    pub fn detach_active(&mut self) -> Option<UnderstandingRequestKey> {
        let active = self.active.as_mut()?;
        active.cancel.store(true, Ordering::Relaxed);
        active.accepts_events = false;
        self.status = UnderstandingSessionStatus::Stale;
        Some(active.key.clone())
    }

    /// 工作线程实际退出后清理 active。路由键不匹配时不影响当前请求。
    pub fn finish_cancelled(&mut self, request: &UnderstandingRequestKey) -> bool {
        if !self.active_matches(request) {
            return false;
        }
        self.active = None;
        true
    }

    /// 接收完成答案。迟到/已分离答案被丢弃，已完成历史最多保留 5 条。
    pub fn finish_answer(
        &mut self,
        request: &UnderstandingRequestKey,
        answer: UnderstandingAnswer,
    ) -> UnderstandingResult<bool> {
        if !self.active_matches(request) {
            return Ok(false);
        }
        let accepts_events = self
            .active
            .as_ref()
            .is_some_and(|active| active.accepts_events);
        self.active = None;
        if !accepts_events {
            return Ok(false);
        }
        if answer.analysis.context.project_key != self.project_key {
            self.status = UnderstandingSessionStatus::Failed;
            return Err(UnderstandingError::new(
                ErrorCode::OutsideProject,
                "代码理解答案不属于当前项目",
            ));
        }
        self.status = match answer.analysis.completion {
            AnalysisCompletion::Complete => UnderstandingSessionStatus::Completed,
            AnalysisCompletion::Partial => UnderstandingSessionStatus::Partial,
        };
        self.history.push_back(UnderstandingHistoryEntry {
            request: request.clone(),
            answer,
        });
        while self.history.len() > UNDERSTANDING_SESSION_HISTORY_LIMIT {
            self.history.pop_front();
        }
        Ok(true)
    }

    /// 接收失败；迟到失败不会覆盖当前状态。
    pub fn finish_error(&mut self, request: &UnderstandingRequestKey) -> bool {
        if !self.active_matches(request) {
            return false;
        }
        let accepts_events = self
            .active
            .as_ref()
            .is_some_and(|active| active.accepts_events);
        self.active = None;
        if accepts_events {
            self.status = UnderstandingSessionStatus::Failed;
        }
        accepts_events
    }

    /// 事件是否仍属于当前可见请求。
    pub fn accepts_event(&self, event: &UnderstandingSessionEvent) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.accepts_events && active.key == event.request)
    }

    pub fn tag_event(
        request: &UnderstandingRequestKey,
        event: UnderstandingEvent,
    ) -> UnderstandingSessionEvent {
        UnderstandingSessionEvent {
            request: request.clone(),
            event,
        }
    }

    fn active_matches(&self, request: &UnderstandingRequestKey) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.key == *request)
    }

    fn prompt_context(&self) -> UnderstandingPromptContext {
        let history = self
            .history
            .iter()
            .rev()
            .take(UNDERSTANDING_PROMPT_HISTORY_LIMIT)
            .map(|entry| UnderstandingHistorySummary {
                question: entry.answer.analysis.context.question.clone(),
                summary: entry.answer.analysis.summary.clone(),
                scope: entry.answer.analysis.context.scope.clone(),
                completion: entry.answer.analysis.completion,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        UnderstandingPromptContext {
            history,
            selected_source: self.selected_source.clone(),
        }
    }
}

/// 点击历史来源前重新核对项目、索引代际与文件 hash；失败时调用方应提示重新分析。
pub fn validate_history_source(
    repo_root: &Path,
    index_db_path: &Path,
    entry: &UnderstandingHistoryEntry,
    source_id: &str,
) -> UnderstandingResult<SourceRef> {
    let source = entry
        .answer
        .sources
        .iter()
        .find(|source| source_id_of(source) == source_id)
        .ok_or_else(|| {
            UnderstandingError::new(ErrorCode::AnswerInvalid, "来源 ID 不属于这次历史答案")
        })?;
    if source.project_key != entry.request.project_key {
        return Err(UnderstandingError::new(
            ErrorCode::OutsideProject,
            "历史来源与请求项目不一致",
        ));
    }
    let tools = UnderstandingTools::open(repo_root, index_db_path)?;
    if tools.context().project_key != entry.request.project_key {
        return Err(UnderstandingError::new(
            ErrorCode::OutsideProject,
            "历史答案不属于当前项目",
        ));
    }
    tools.source_service().validate_source_ref(source)
}
