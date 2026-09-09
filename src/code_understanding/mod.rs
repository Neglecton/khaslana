// 代码理解与逻辑可视化服务层。
//
// DEV-01 交付类型契约（设计文档 §4/§8/§12）：EntityKey、关系、证据、
// GraphSlice、AnswerDocument 与统一错误码。全部类型 serde 可往返，
// 作为 AI/MCP/UI 的唯一契约源（design.md §7：拟新增 API 用 Rust 类型
// 作契约，serde DTO 为适配层）。
//
// 本阶段只有纯数据层与校验；检索/agent/布局随 DEV-09～12 落地，
// 不提前创建空模块。

mod types;

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
