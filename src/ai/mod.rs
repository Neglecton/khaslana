// AI 领域层汇总入口。
//
// 模块组织：
// - config.rs：供应商配置类型、校验、默认值。
// - prompt.rs：构造 commit / review / 冲突合并 prompt 的纯函数。
// - client.rs：OpenAI Chat Completions 兼容 HTTP 客户端、思考链剥离。
// - review.rs：review 结果结构化类型。
// - merge.rs：冲突合并建议的 diff3 分段、滑动窗口与响应清洗纯函数。
//
// 本模块不含 GPUI 依赖；UI 接入在 src/ai_view.rs。

pub mod client;
pub mod config;
pub mod merge;
pub mod prompt;
pub mod review;

pub use client::{
    ChatClient, ChatResult, StreamDelta, split_reasoning, validate_generated_content,
};
pub use config::{AiApiType, AiProviderSettings};
pub use merge::{
    MERGE_CONTEXT_BUDGET_CHARS, MERGE_SEGMENT_LIMIT, MERGE_SINGLE_BLOCK_LIMIT,
    MERGE_WHOLE_FILE_LIMIT, MergeSegment, build_segment_messages,
    response_contains_conflict_markers, split_diff3_text, strip_code_fence,
};
pub use prompt::{
    ChatMessage, ChatRole, commit_message_prompts, conflict_merge_prompts, review_prompts,
};
pub use review::AiReviewResult;
