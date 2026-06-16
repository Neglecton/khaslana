// OpenAI Chat Completions 兼容 HTTP 客户端。
//
// 支持一次性请求（`request`，连接测试用）和流式 SSE 请求（`request_stream`，
// commit/review 用，避免长输出超时）。流式时逐 chunk 通过回调增量推送，
// 同时在本地累积完整文本，结束时统一剥离 `<think>` 思考链。
//
// 响应解析时通用剥离思考链（`reasoning_content` 字段或 `<think>` 标签），
// 作为可选展示，非任何模型专用兼容。

use std::io::{BufRead, BufReader};

use serde::{Deserialize, Serialize};

use crate::ai::config::AiProviderSettings;
use crate::ai::prompt::ChatMessage;
use crate::ai::review::AiReviewResult;
use crate::types::{GitError, Result as KhaslanaResult};

/// 单次聊天请求的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatResult {
    /// 回复正文（已剥离思考链）。
    pub content: String,
    /// 可选思考链；普通模型为 None。
    pub reasoning: Option<String>,
}

impl ChatResult {
    /// 转 review 结果类型，供对比模式 AI 评审面板使用。
    pub fn into_review(self) -> AiReviewResult {
        AiReviewResult {
            content: self.content,
            reasoning: self.reasoning,
        }
    }
}

/// 从 OpenAI 兼容响应的 message content 中剥离 `<think>...</think>` 思考链。
///
/// 返回 (剥离后的正文, 可选思考链文本)。支持多段、跨行；若存在未闭合的
/// `<think>` 开标签，把其后全部内容当作思考链。无 `<think>` 时返回原文 + None。
///
/// 这是纯函数，便于单元测试。
pub fn split_reasoning(content: &str) -> (String, Option<String>) {
    if !content.contains("<think>") {
        return (content.to_string(), None);
    }

    let mut body = String::with_capacity(content.len());
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut rest = content;

    loop {
        match rest.find("<think>") {
            Some(open) => {
                // 保留 `<think>` 之前的正文。
                body.push_str(&rest[..open]);
                let after_open = &rest[open + "<think>".len()..];
                match after_open.find("</think>") {
                    Some(close) => {
                        reasoning_parts.push(after_open[..close].to_string());
                        rest = &after_open[close + "</think>".len()..];
                    }
                    None => {
                        // 未闭合：把剩余全部当作思考链。
                        reasoning_parts.push(after_open.to_string());
                        break;
                    }
                }
            }
            None => {
                body.push_str(rest);
                break;
            }
        }
    }

    let body = body.trim().to_string();
    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        let joined = reasoning_parts
            .iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if joined.is_empty() {
            None
        } else {
            Some(joined)
        }
    };
    (body, reasoning)
}

/// OpenAI Chat Completions 请求体。
#[derive(Serialize)]
struct ChatCompletionsRequest<'a> {
    model: &'a str,
    messages: Vec<RequestMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// OpenAI Chat Completions 响应（只解析需要的字段）。
#[derive(Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    /// 部分兼容 API（如 DeepSeek reasoner）单独返回思考链字段。
    #[serde(default)]
    reasoning_content: Option<String>,
}

pub struct ChatClient {
    settings: AiProviderSettings,
    proxy_url: Option<String>,
}

impl ChatClient {
    pub fn new(settings: AiProviderSettings, proxy_url: Option<String>) -> Self {
        Self {
            settings,
            proxy_url,
        }
    }

    /// 发送一次聊天请求，返回正文和可选思考链。
    pub fn request(&self, messages: &[ChatMessage]) -> KhaslanaResult<ChatResult> {
        match self.settings.api_type {
            crate::ai::config::AiApiType::ChatCompletions => {
                self.request_chat_completions(messages)
            }
        }
    }

    /// 流式发送聊天请求；每个增量 chunk 通过 `on_delta` 回调推送，
    /// 全部读完后返回剥离思考链后的最终结果。
    pub fn request_stream(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut impl FnMut(StreamDelta),
    ) -> KhaslanaResult<ChatResult> {
        match self.settings.api_type {
            crate::ai::config::AiApiType::ChatCompletions => {
                self.request_chat_completions_stream(messages, on_delta)
            }
        }
    }

    fn request_chat_completions(&self, messages: &[ChatMessage]) -> KhaslanaResult<ChatResult> {
        let url = format!(
            "{}{}",
            self.settings.normalized_base_url(),
            self.settings.api_type.endpoint_path()
        );

        let req_messages: Vec<RequestMessage<'_>> = messages
            .iter()
            .map(|msg| RequestMessage {
                role: msg.role.as_str(),
                content: &msg.content,
            })
            .collect();

        let body = ChatCompletionsRequest {
            model: &self.settings.model,
            messages: req_messages,
            temperature: self.settings.temperature,
            max_tokens: self.settings.max_tokens,
            stream: false,
        };

        let agent = self.build_agent();
        let body_value = serde_json::to_value(&body)
            .map_err(|err| GitError::Message(format!("AI 请求体序列化失败：{err}")))?;
        let response = agent
            .post(&url)
            .set(
                "Authorization",
                &format!("Bearer {}", self.settings.api_key),
            )
            .set("Content-Type", "application/json")
            .send_json(body_value)
            .map_err(|err| GitError::Message(format!("AI 请求失败：{err}")))?;

        let parsed: ChatCompletionsResponse = response
            .into_json()
            .map_err(|err| GitError::Message(format!("AI 响应解析失败：{err}")))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| GitError::Message("AI 响应中没有 choices".into()))?;

        let content = choice.message.content.unwrap_or_default();
        // 优先使用单独的 reasoning_content 字段；否则从 content 中剥离 <think>。
        let (content, reasoning) = if let Some(reasoning) = choice.message.reasoning_content {
            let reasoning = reasoning.trim();
            let reasoning = if reasoning.is_empty() {
                None
            } else {
                Some(reasoning.to_string())
            };
            // 若 content 里还残留 <think>，再剥离一次，避免重复展示。
            let (cleaned, extra) = split_reasoning(&content);
            let reasoning = match (reasoning, extra) {
                (Some(r), Some(e)) => Some(format!("{r}\n\n{e}")),
                (Some(r), None) => Some(r),
                (None, Some(e)) => Some(e),
                (None, None) => None,
            };
            (cleaned, reasoning)
        } else {
            split_reasoning(&content)
        };

        let content = content.trim().to_string();
        if content.is_empty() && reasoning.is_none() {
            return Err(GitError::Message("AI 返回了空内容".into()));
        }

        Ok(ChatResult { content, reasoning })
    }

    fn request_chat_completions_stream(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut impl FnMut(StreamDelta),
    ) -> KhaslanaResult<ChatResult> {
        let url = format!(
            "{}{}",
            self.settings.normalized_base_url(),
            self.settings.api_type.endpoint_path()
        );

        let req_messages: Vec<RequestMessage<'_>> = messages
            .iter()
            .map(|msg| RequestMessage {
                role: msg.role.as_str(),
                content: &msg.content,
            })
            .collect();

        let body = ChatCompletionsRequest {
            model: &self.settings.model,
            messages: req_messages,
            temperature: self.settings.temperature,
            max_tokens: self.settings.max_tokens,
            stream: true,
        };

        let agent = self.build_streaming_agent();
        let body_value = serde_json::to_value(&body)
            .map_err(|err| GitError::Message(format!("AI 请求体序列化失败：{err}")))?;
        let response = agent
            .post(&url)
            .set(
                "Authorization",
                &format!("Bearer {}", self.settings.api_key),
            )
            .set("Content-Type", "application/json")
            .send_json(body_value)
            .map_err(|err| GitError::Message(format!("AI 请求失败：{err}")))?;

        // 逐行读取 SSE 流，累积完整 content 与 reasoning_content。
        let reader = BufReader::new(response.into_reader());
        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        for line in reader.lines() {
            let line = line.map_err(|err| GitError::Message(format!("AI 流读取失败：{err}")))?;
            match parse_sse_line(&line) {
                Some(SseLineResult::Chunk(chunk)) => {
                    for choice in chunk.choices {
                        if let Some(text) = choice.delta.content {
                            if !text.is_empty() {
                                full_content.push_str(&text);
                                on_delta(StreamDelta::Content(text));
                            }
                        }
                        if let Some(text) = choice.delta.reasoning_content {
                            if !text.is_empty() {
                                full_reasoning.push_str(&text);
                                on_delta(StreamDelta::Reasoning(text));
                            }
                        }
                    }
                }
                Some(SseLineResult::Done) => break,
                None => {}
            }
        }

        // 结束后统一剥离 `<think>`，与非流式逻辑一致。
        let (content, extra_reasoning) = split_reasoning(&full_content);
        let reasoning = merge_reasoning(
            (!full_reasoning.trim().is_empty()).then(|| full_reasoning.trim().to_string()),
            extra_reasoning,
        );

        let content = content.trim().to_string();
        if content.is_empty() && reasoning.is_none() {
            return Err(GitError::Message("AI 返回了空内容".into()));
        }

        Ok(ChatResult { content, reasoning })
    }

    fn build_streaming_agent(&self) -> ureq::Agent {
        // 流式场景不设整体超时：把 request_timeout_secs 当作读空闲超时，
        // 长输出只要持续有数据就不算超时。
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(
                self.settings.request_timeout_secs.max(1),
            ));
        if let Some(proxy_url) = self.proxy_url.as_deref() {
            if !proxy_url.trim().is_empty() {
                if let Ok(proxy) = ureq::Proxy::new(proxy_url) {
                    builder = builder.proxy(proxy);
                }
            }
        }
        builder.build()
    }

    fn build_agent(&self) -> ureq::Agent {
        let mut builder = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(
            self.settings.request_timeout_secs.max(1),
        ));
        if let Some(proxy_url) = self.proxy_url.as_deref() {
            if !proxy_url.trim().is_empty() {
                if let Ok(proxy) = ureq::Proxy::new(proxy_url) {
                    builder = builder.proxy(proxy);
                }
            }
        }
        builder.build()
    }
}

/// 流式增量 chunk 的种类。
#[derive(Clone, Debug)]
pub enum StreamDelta {
    /// 正文增量。
    Content(String),
    /// 思考链增量（DeepSeek 等 reasoning 模型原生流式字段）。
    Reasoning(String),
}

/// 单行 SSE 解析结果。
#[derive(Debug)]
enum SseLineResult {
    /// 一个包含 choices 的数据事件。
    Chunk(StreamChunk),
    /// `data: [DONE]` 结束标记。
    Done,
}

#[derive(Deserialize, Debug)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize, Debug)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDeltaJson,
}

#[derive(Deserialize, Default, Debug)]
struct StreamDeltaJson {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

/// 解析单行 SSE：`data:{...}` → Chunk，`data:[DONE]` → Done，
/// 其余（空行、非 data 前缀、JSON 解析失败）返回 None 容错跳过。
///
/// 按 HTML5 Server-Sent Events 规范（§9.2），字段名后的冒号是必须的，
/// 但冒号后的空格是**可选的**：`data:{...}`（无空格）与 `data: {...}`
///（一个空格）以及 `data:  {...}`（多个空格）都是合法格式。部分兼容
/// 服务端（含某些 OpenAI 兼容网关）恰好发送无空格的 `data:`，因此这里
/// 用 `strip_prefix("data:")` 后 `trim_start()` 兼容所有变体，避免静默
/// 丢弃合法事件导致流式输出残缺。
fn parse_sse_line(line: &str) -> Option<SseLineResult> {
    let trimmed = line.trim();
    let payload = trimmed.strip_prefix("data:")?.trim_start();
    // 外层 `trimmed` 已去掉行尾空白，理论上 payload 无尾部空白；这里仍用
    // `trim()` 比较是防御性的（零成本），避免个别服务端在 [DONE] 后带不可见字符。
    if payload.trim() == "[DONE]" {
        return Some(SseLineResult::Done);
    }
    serde_json::from_str::<StreamChunk>(payload)
        .ok()
        .map(SseLineResult::Chunk)
}

/// 合并原生 reasoning_content 与从 `<think>` 剥离出的额外思考链，
/// 与非流式 `request` 的处理保持一致。
fn merge_reasoning(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_reasoning_no_think_tag_returns_original() {
        let (body, reasoning) = split_reasoning("普通回复内容");
        assert_eq!(body, "普通回复内容");
        assert_eq!(reasoning, None);
    }

    #[test]
    fn split_reasoning_single_think_block() {
        let content = "前面正文<think>思考过程</think>后面正文";
        let (body, reasoning) = split_reasoning(content);
        assert_eq!(body, "前面正文后面正文");
        assert_eq!(reasoning.as_deref(), Some("思考过程"));
    }

    #[test]
    fn split_reasoning_multiple_think_blocks() {
        let content = "<think>第一段</think>中间<think>第二段</think>结尾";
        let (body, reasoning) = split_reasoning(content);
        assert_eq!(body, "中间结尾");
        assert_eq!(reasoning.as_deref(), Some("第一段\n\n第二段"));
    }

    #[test]
    fn split_reasoning_multiline_think_block() {
        let content = "<think>第一行\n第二行</think>\n正文";
        let (body, reasoning) = split_reasoning(content);
        assert_eq!(body, "正文");
        assert_eq!(reasoning.as_deref(), Some("第一行\n第二行"));
    }

    #[test]
    fn split_reasoning_unclosed_think_treats_rest_as_reasoning() {
        let content = "正文<think>未闭合的思考";
        let (body, reasoning) = split_reasoning(content);
        assert_eq!(body, "正文");
        assert_eq!(reasoning.as_deref(), Some("未闭合的思考"));
    }

    #[test]
    fn split_reasoning_empty_think_yields_no_reasoning() {
        let content = "正文<think></think>结尾";
        let (body, reasoning) = split_reasoning(content);
        assert_eq!(body, "正文结尾");
        assert_eq!(reasoning, None);
    }

    #[test]
    fn parse_sse_line_done_marker() {
        assert!(matches!(
            parse_sse_line("data: [DONE]"),
            Some(SseLineResult::Done)
        ));
        assert!(matches!(
            parse_sse_line("  data: [DONE]  "),
            Some(SseLineResult::Done)
        ));
    }

    #[test]
    fn parse_sse_line_content_chunk() {
        let line = r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#;
        match parse_sse_line(line) {
            Some(SseLineResult::Chunk(chunk)) => {
                assert_eq!(chunk.choices.len(), 1);
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hello"));
                assert!(chunk.choices[0].delta.reasoning_content.is_none());
            }
            other => panic!("expected Chunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_line_reasoning_chunk() {
        let line = r#"data: {"choices":[{"delta":{"reasoning_content":"思考"}}]}"#;
        match parse_sse_line(line) {
            Some(SseLineResult::Chunk(chunk)) => {
                assert_eq!(
                    chunk.choices[0].delta.reasoning_content.as_deref(),
                    Some("思考")
                );
                assert!(chunk.choices[0].delta.content.is_none());
            }
            other => panic!("expected Chunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_line_skips_non_data_lines() {
        assert!(parse_sse_line("").is_none());
        assert!(parse_sse_line(": comment").is_none());
        assert!(parse_sse_line("event: ping").is_none());
        // 坏 JSON 容错跳过，不 panic。
        assert!(parse_sse_line("data: {broken").is_none());
    }

    #[test]
    fn parse_sse_line_accepts_no_space_after_colon() {
        // 规范允许 `data:` 后无空格；部分兼容服务端正是此格式。
        // 改动前这里会返回 None（静默丢事件），改动后应解析为 Chunk。
        let line = r#"data:{"choices":[{"delta":{"content":"hello"}}]}"#;
        match parse_sse_line(line) {
            Some(SseLineResult::Chunk(chunk)) => {
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hello"));
            }
            other => panic!("expected Chunk for no-space data line, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_line_accepts_multiple_spaces_after_colon() {
        // 规范允许冒号后任意个空格（多余空格属于 value 的一部分被丢弃）。
        let line = r#"data:   {"choices":[{"delta":{"content":"hi"}}]}"#;
        match parse_sse_line(line) {
            Some(SseLineResult::Chunk(chunk)) => {
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
            }
            other => panic!("expected Chunk for multi-space data line, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_line_done_marker_without_space() {
        // `data:[DONE]`（无空格）也应识别为结束标记。
        assert!(matches!(
            parse_sse_line("data:[DONE]"),
            Some(SseLineResult::Done)
        ));
    }

    #[test]
    fn parse_sse_line_value_with_leading_space_is_preserved_when_single() {
        // 单个空格按规范应被剥离；但 value 本身的前导空格（JSON 外）无意义，
        // 这里只断言 JSON 仍能正确解析（payload 被 trim_start 后为合法 JSON）。
        let line = r#"data: {"choices":[]}"#;
        match parse_sse_line(line) {
            Some(SseLineResult::Chunk(chunk)) => assert!(chunk.choices.is_empty()),
            other => panic!("expected Chunk, got {other:?}"),
        }
    }

    #[test]
    fn merge_reasoning_combines_and_filters() {
        assert_eq!(
            merge_reasoning(Some("a".into()), Some("b".into())).as_deref(),
            Some("a\n\nb")
        );
        assert_eq!(
            merge_reasoning(Some("a".into()), None).as_deref(),
            Some("a")
        );
        assert_eq!(
            merge_reasoning(None, Some("b".into())).as_deref(),
            Some("b")
        );
        assert!(merge_reasoning(None, None).is_none());
    }
}
