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

        let agent = self.build_agent()?;
        let mut response = agent
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.settings.api_key))
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|err| GitError::Message(format!("AI 请求失败：{err}")))?;

        let parsed: ChatCompletionsResponse = response
            .body_mut()
            .read_json()
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
        // 纯空白的思考链视同没有。
        let reasoning = reasoning.filter(|reasoning| !reasoning.trim().is_empty());
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

        let agent = self.build_streaming_agent()?;
        let response = agent
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.settings.api_key))
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|err| GitError::Message(format!("AI 请求失败：{err}")))?;

        // 逐行读取 SSE 流，累积完整 content 与 reasoning_content。
        let mut body = response.into_body();
        let reader = BufReader::new(body.as_reader());
        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        // 诊断计数：有效数据块数 / 以 data: 开头却解析失败的行数（用于区分
        // “供应商响应格式异常”与“有响应但没有内容”）。
        let mut valid_chunks = 0usize;
        let mut malformed_data_lines = 0usize;
        for line in reader.lines() {
            let line = line.map_err(|err| GitError::Message(format!("AI 流读取失败：{err}")))?;
            match parse_sse_line(&line) {
                Some(SseLineResult::Chunk(chunk)) => {
                    valid_chunks += 1;
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
                None => {
                    if line.trim_start().starts_with("data:") {
                        malformed_data_lines += 1;
                    }
                }
            }
        }
        if malformed_data_lines > 0 {
            tracing::warn!(
                target: "khaslana::ai",
                "AI 响应流中有 {malformed_data_lines} 行 data 数据解析失败"
            );
        }

        // 结束后统一剥离 `<think>`，与非流式逻辑一致。
        let (content, extra_reasoning) = split_reasoning(&full_content);
        let reasoning = merge_reasoning(
            (!full_reasoning.trim().is_empty()).then(|| full_reasoning.trim().to_string()),
            extra_reasoning,
        );
        // 纯空白的思考链视同没有。
        let reasoning = reasoning.filter(|reasoning| !reasoning.trim().is_empty());

        let content = content.trim().to_string();
        if content.is_empty() && reasoning.is_none() {
            return Err(GitError::Message(
                if valid_chunks == 0 {
                    "AI 响应流中没有有效数据块（供应商响应格式异常或响应被截断）"
                } else {
                    "AI 返回了空内容"
                }
                .into(),
            ));
        }

        Ok(ChatResult { content, reasoning })
    }

    fn build_streaming_agent(&self) -> KhaslanaResult<ureq::Agent> {
        // 流式场景不设整体超时：把 request_timeout_secs 当作读空闲超时
        // （ureq 3 的 timeout_recv_body 按每次读分别计算），长输出只要
        // 持续有数据就不算超时。
        let timeout = std::time::Duration::from_secs(self.settings.request_timeout_secs.max(1));
        let builder = ureq::Agent::config_builder()
            .timeout_connect(Some(std::time::Duration::from_secs(30)))
            .timeout_recv_body(Some(timeout))
            .proxy(resolve_proxy(self.proxy_url.as_deref())?);
        Ok(builder.build().new_agent())
    }

    fn build_agent(&self) -> KhaslanaResult<ureq::Agent> {
        let timeout = std::time::Duration::from_secs(self.settings.request_timeout_secs.max(1));
        let builder = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .proxy(resolve_proxy(self.proxy_url.as_deref())?);
        Ok(builder.build().new_agent())
    }
}

/// 把用户配置的代理 URL 解析为 ureq 代理。
///
/// - 未配置时返回 `None`，显式关闭 ureq 3 默认的环境变量代理自动检测，
///   保证应用内“不使用代理”设置生效（系统代理模式由调用方读取环境变量后传入）。
/// - URL 无效时返回错误而不是静默直连：用户显式配置的代理被绕过会暴露
///   真实网络路径，必须让请求失败并提示（例如 SOCKS5 代理写错协议前缀）。
fn resolve_proxy(proxy_url: Option<&str>) -> KhaslanaResult<Option<ureq::Proxy>> {
    match proxy_url.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => ureq::Proxy::new(url)
            .map(Some)
            .map_err(|err| GitError::Message(format!("代理配置无效，已取消 AI 请求：{err}"))),
        None => Ok(None),
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

/// 生成类结果（提交信息/评审）的空正文校验：正文 trim 为空即视为失败，
/// 区分「仅返回思考过程」与「完全为空」两种提示；正常时返回 trim 后的正文。
///
/// 传输层对「空」较宽松（有思考链即 Ok，评审面板需要展示思考过程），
/// 而生成场景正文才是用户要的结果，由调用方按用途做严格校验。
pub fn validate_generated_content(
    result: &ChatResult,
    empty_message: &str,
    reasoning_only_message: &str,
) -> KhaslanaResult<String> {
    let content = result.content.trim();
    if !content.is_empty() {
        return Ok(content.to_string());
    }
    Err(GitError::Message(
        if result
            .reasoning
            .as_deref()
            .is_some_and(|reasoning| !reasoning.trim().is_empty())
        {
            reasoning_only_message
        } else {
            empty_message
        }
        .into(),
    ))
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

/// 解析单行 SSE：`data: {...}` → Chunk，`data: [DONE]` → Done，
/// 其余（空行、非 data 前缀、JSON 解析失败）返回 None 容错跳过。
fn parse_sse_line(line: &str) -> Option<SseLineResult> {
    let trimmed = line.trim();
    let payload = trimmed.strip_prefix("data: ")?;
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
#[path = "../tests/ai/client.rs"]
mod tests;
