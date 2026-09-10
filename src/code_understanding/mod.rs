// 代码理解与逻辑可视化服务层。
//
// DEV-01 交付类型契约（设计文档 §4/§8/§12）：EntityKey、关系、证据、
// GraphSlice、AnswerDocument 与统一错误码。全部类型 serde 可往返，
// 作为 AI/MCP/UI 的唯一契约源（design.md §7：拟新增 API 用 Rust 类型
// 作契约，serde DTO 为适配层）。
//
// V2 在严格静态图协议之外增加只读源码、工具会话与 AI 业务问答：
// - source.rs：固定允许文件清单、有限读取、来源 ID 发放与 hash 复验；
// - tools.rs：六个只读工具（符号/源码检索、文件读取、目录树、调用线索）；
// - analysis.rs：会话级业务答案 `AnalysisResult` 与来源/结构校验；
// - agent.rs：单问题 agent 闭环（轮次/工具/HTTP 预算、格式修复、取消）。
// - session.rs：追问历史、请求代际、取消/分离与旧来源复验。
// AI 业务发现不会写回 GraphSlice，也不会把会话推断伪装成索引权威关系。

mod agent;
mod analysis;
mod session;
mod source;
mod tools;
mod types;

pub use agent::{
    ChatTurnProvider, UNDERSTANDING_MAX_HTTP_ATTEMPTS, UNDERSTANDING_MAX_TOOL_CALLS,
    UNDERSTANDING_MAX_TOOL_RESULT_CHARS, UNDERSTANDING_MAX_TOOL_ROUNDS,
    UNDERSTANDING_MAX_TOTAL_RESULT_CHARS, UnderstandingAgentInput, UnderstandingAnswer,
    UnderstandingEvent, UnderstandingStep, UnderstandingTurnProvider, run_understanding_agent,
    run_understanding_agent_with_context,
};
pub use analysis::{
    ANALYSIS_MAX_LINKS, ANALYSIS_MAX_STEPS, ANALYSIS_PROTOCOL_VERSION, AnalysisCompletion,
    AnalysisContext, AnalysisEvidenceState, AnalysisFinding, AnalysisResult, CallRelation,
    CallRelationKind, DataAccess, DataObjectCategory, DataOperationKind, FlowLink, FlowLinkKind,
    FlowStep, analysis_json_schema, analysis_output_protocol, analysis_system_prompt,
    parse_analysis_result, validate_analysis_result,
};
pub use session::{
    UNDERSTANDING_PROMPT_HISTORY_LIMIT, UNDERSTANDING_PROMPT_HISTORY_MAX_CHARS,
    UNDERSTANDING_SESSION_HISTORY_LIMIT, UnderstandingHistoryEntry, UnderstandingHistorySummary,
    UnderstandingPromptContext, UnderstandingRequestKey, UnderstandingRequestTicket,
    UnderstandingSession, UnderstandingSessionEvent, UnderstandingSessionStatus,
    validate_history_source,
};
pub(crate) use source::source_id_of;
pub use source::{
    FileTreeEntry, FileTreeEntryKind, FileTreeResult, SOURCE_FILE_MAX_BYTES, SOURCE_READ_MAX_CHARS,
    SOURCE_READ_MAX_LINES, SOURCE_SEARCH_MAX_FILES, SOURCE_SEARCH_MAX_RESULTS,
    SOURCE_TREE_MAX_DEPTH, SOURCE_TREE_MAX_ENTRIES, SourceLine, SourceRead, SourceSearchMatch,
    SourceSearchResult, SourceService, UnderstandingResult,
};
pub use tools::{
    CallTraceDirection, GetFileTreeArgs, GetSymbolArgs, ReadFileArgs, SearchCodeArgs,
    SearchSymbolsArgs, SearchSymbolsResult, SymbolCandidateView, SymbolDetailView,
    SymbolRelationView, ToolEnvelope, TraceCallsArgs, TraceCallsResult, UnderstandingTools,
    tool_schemas,
};

pub use types::{
    ANSWER_PROTOCOL_VERSION, AnswerClaim, AnswerDocument, AnswerStep, BoundaryNode, Certainty,
    CompletionStatus, EntityKey, ErrorCode, EvidenceBundle, EvidenceKind, EvidenceRecord,
    GraphSlice, GraphSliceNode, Nature, RelationDirection, RelationKind, RelationRecord,
    ResolutionState, SearchFilter, Uncertainty, UncertaintyKind, UnderstandingError,
    evidence_id_from_parts, protocol_version, relation_id_from_parts, validate_answer_document,
};

#[cfg(test)]
#[path = "../tests/code_understanding_types.rs"]
mod types_tests;

#[cfg(test)]
#[path = "../tests/code_understanding_tools.rs"]
mod tools_tests;

#[cfg(test)]
#[path = "../tests/code_understanding_live.rs"]
mod live_tests;

#[cfg(test)]
#[path = "../tests/code_understanding_srcprobe.rs"]
mod srcprobe_tests;

#[cfg(test)]
#[path = "../tests/code_understanding_session.rs"]
mod session_tests;
