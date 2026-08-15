// AI 领域层汇总入口。
//
// 模块组织：
// - config.rs：供应商配置类型、校验、默认值。
// - prompt.rs：构造 commit / review prompt 的纯函数。
// - client.rs：OpenAI Chat Completions 兼容 HTTP 客户端、思考链剥离。
// - review.rs：review 结果结构化类型。
//
// 本模块不含 GPUI 依赖；UI 接入在 src/ai_view.rs。

pub mod client;
pub mod config;
pub mod prompt;
pub mod review;

pub use client::{
    ChatClient, ChatResult, StreamDelta, split_reasoning, validate_generated_content,
};
pub use config::{AiApiType, AiProviderSettings};
pub use prompt::{ChatMessage, ChatRole, commit_message_prompts, review_prompts};
pub use review::AiReviewResult;
