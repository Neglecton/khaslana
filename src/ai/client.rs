// OpenAI Chat Completions 兼容 HTTP 客户端。
//
// 第一版只支持 `AiApiType::ChatCompletions`，用 ureq 同步阻塞调用，
// 适合在 rayon 后台线程中执行。响应解析时通用剥离思考链
// （`reasoning_content` 字段或 `<think>` 标签），作为可选展示，非任何模型专用兼容。

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
}
