// AI 领域层汇总入口。
//
// 模块组织：
// - config.rs：供应商配置类型、校验、默认值。
// - prompt.rs：构造 commit / 冲突合并 prompt 的纯函数（评审 prompt 在 review_agent.rs）。
// - client.rs：OpenAI Chat Completions 兼容 HTTP 客户端、思考链剥离、工具调用协议。
// - review.rs：review 结果结构化类型。
// - review_agent.rs：Diff-first Agentic 评审（system prompt、工具注册表、预算守卫、多轮循环）。
// - review_store.rs：评审记录本地落盘（JSON 文件 + 每仓库保留上限）。
// - merge.rs：冲突合并建议的 diff3 分段、滑动窗口与响应清洗纯函数。
//
// 本模块不含 GPUI 依赖；UI 接入在 src/ai_view.rs。

pub mod client;
pub mod config;
pub mod merge;
pub mod prompt;
pub mod review;
pub mod review_agent;
pub mod review_store;

pub use client::{
    AgentChatMessage, AgentToolCall, AgentTurn, ChatClient, ChatResult, StreamDelta, ToolSchema,
    agent_request_error, split_reasoning, validate_generated_content,
};
pub use config::{AiApiType, AiProviderSettings};
pub use merge::{
    MERGE_CONTEXT_BUDGET_CHARS, MERGE_SEGMENT_LIMIT, MERGE_SINGLE_BLOCK_LIMIT,
    MERGE_WHOLE_FILE_LIMIT, MergeSegment, build_segment_messages,
    response_contains_conflict_markers, split_diff3_text, strip_code_fence,
};
pub use prompt::{ChatMessage, ChatRole, commit_message_prompts, conflict_merge_prompts};
pub use review::{AiReviewResult, AiReviewStep};
pub use review_agent::{AgentEvent, ReviewAgentInput, file_diff_to_patch_text, run_review_agent};
pub use review_store::{AiReviewRecord, list_review_records, save_review_record};
