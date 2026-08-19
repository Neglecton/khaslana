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
use crate::types::{GitError, Result as KhaslanaResult};

/// 单次聊天请求的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatResult {
    /// 回复正文（已剥离思考链）。
    pub content: String,
    /// 可选思考链；普通模型为 None。
    pub reasoning: Option<String>,
}

// ── Agentic 工具调用协议 ─────────────────────────────────────────────────
//
// 仅用于评审 agent 的非流式多轮循环：请求携带 tools 定义，响应解析
// message.tool_calls；assistant（带 tool_calls）与 tool（带 tool_call_id）
// 两种消息形态回填进对话。不引入 SDK：主流 Rust OpenAI SDK 均为
// async/tokio + reqwest 栈，与本项目「同步 ureq + 应用内代理」架构冲突
// 且体积增长明显；非流式工具报文只是少量 serde 结构。

/// 工具定义（OpenAI function calling）。
#[derive(Clone, Debug)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    /// 参数 JSON Schema（`serde_json::json!` 构造）。
    pub parameters: serde_json::Value,
}

/// 模型发起的一次工具调用；`arguments` 是 JSON 字符串，由调用方按工具定义解析。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// agent 循环的消息形态（比普通 ChatMessage 多出带 tool_calls 的
/// assistant 回复与 tool 结果两态，仅 agent 请求内部使用）。
#[derive(Clone, Debug, PartialEq)]
pub enum AgentChatMessage {
    System(String),
    User(String),
    Assistant {
        content: String,
        tool_calls: Vec<AgentToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// 一轮 agent 请求的结果：正文 + 可选思考链 + 模型发起的工具调用。
#[derive(Clone, Debug, PartialEq)]
pub struct AgentTurn {
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<AgentToolCall>,
}

/// agent 流式单次请求的错误：`retryable` 标记瞬态故障（网络中断、5xx、
/// 流中途被截断、无有效数据块等），由上层决定是否自动重试；配置类错误
///（400/404/422、URL/代理无效）重试不会有结果，直接失败。
///
/// `retry_ceiling` 是该错误允许的额外重试次数上限（不含首次请求）：
/// 瞬态网络/服务故障给满默认上限；`finish_reason=length` 型截断对同一
/// 请求基本是确定性的（模型输出上限低于请求的 max_tokens），只重试 1 次
/// 避免白白消耗完整流式请求与退避时间。
#[derive(Debug)]
pub struct AgentStreamError {
    message: String,
    retryable: bool,
    retry_ceiling: usize,
}

/// 单轮流式请求的默认重试次数上限（不含首次请求）。
pub const AGENT_STREAM_MAX_RETRIES: usize = 3;

impl AgentStreamError {
    /// 瞬态故障：重试至默认上限。
    fn transient(message: String) -> Self {
        Self {
            message,
            retryable: true,
            retry_ceiling: AGENT_STREAM_MAX_RETRIES,
        }
    }

    /// 基本确定性的失败：只再试 1 次（采样波动可能产生更短输出）。
    fn transient_once(message: String) -> Self {
        Self {
            message,
            retryable: true,
            retry_ceiling: 1,
        }
    }

    /// 配置/能力类错误：重试无意义。
    fn fatal(message: String) -> Self {
        Self {
            message,
            retryable: false,
            retry_ceiling: 0,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// 允许的额外重试次数（不含首次请求）。
    pub fn retry_ceiling(&self) -> usize {
        self.retry_ceiling
    }
}

impl std::fmt::Display for AgentStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
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

/// OpenAI Chat Completions 请求体。消息类型泛型化以同时承载普通请求
/// （RequestMessage）与 agent 请求（AgentRequestMessage）。
#[derive(Serialize)]
struct ChatCompletionsRequest<'a, M: Serialize> {
    model: &'a str,
    messages: Vec<M>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// agent 循环消息的请求序列化形态。
#[derive(Serialize)]
struct AgentRequestMessage<'a> {
    role: &'a str,
    /// assistant 带 tool_calls 时 content 序列化为 null（OpenAI 规范允许）
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<AgentRequestToolCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct AgentRequestToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    function: AgentRequestFunction<'a>,
}

#[derive(Serialize)]
struct AgentRequestFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct ToolDefinition<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    function: ToolFunction<'a>,
}

#[derive(Serialize)]
struct ToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
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

    /// agent 循环的一轮流式请求：请求可携带工具定义，SSE 增量同时推送
    /// 正文/思考链（供 UI 实时展示），结束后解析模型发起的 tool_calls
    /// （空表示模型已完成作答）。本方法只做**一次**请求，瞬态失败以
    /// `AgentStreamError::retryable` 标记，由调用方（评审 agent 循环）
    /// 决定重试策略与 UI 提示。
    ///
    /// 流式语义下超时是「读空闲」而非整体限时（`build_streaming_agent`），
    /// reasoning 模型的长思考只要持续出数据就不会被掐断；非流式的
    /// `timeout_global` 整体限时会让长思维链必然超时，这是 agent 循环
    /// 必须走流式的原因。
    ///
    /// 「流结束但既无正文也无工具调用」按截断处理（可重试）：供应商网关
    /// 掐断连接时 EOF 不带 `[DONE]`，半截思考链此前被当成合法回合返回，
    /// 表现为「模型思考一半就停了」。
    ///
    /// `tools` 为空时请求省略 tools 字段（用于预算耗尽后强制收尾）。
    pub fn request_agent_stream(
        &self,
        messages: &[AgentChatMessage],
        tools: &[ToolSchema],
        max_tokens: u32,
        on_delta: &mut impl FnMut(StreamDelta),
    ) -> Result<AgentTurn, AgentStreamError> {
        let url = format!(
            "{}{}",
            self.settings.normalized_base_url(),
            self.settings.api_type.endpoint_path()
        );

        let req_messages: Vec<AgentRequestMessage<'_>> =
            messages.iter().map(agent_message_to_request).collect();
        let tool_definitions: Vec<ToolDefinition<'_>> = tools
            .iter()
            .map(|tool| ToolDefinition {
                kind: "function",
                function: ToolFunction {
                    name: tool.name,
                    description: tool.description,
                    parameters: &tool.parameters,
                },
            })
            .collect();
        let has_tools = !tool_definitions.is_empty();

        let body = ChatCompletionsRequest {
            model: &self.settings.model,
            messages: req_messages,
            temperature: self.settings.temperature,
            max_tokens,
            stream: true,
            tools: has_tools.then_some(tool_definitions),
            tool_choice: has_tools.then_some("auto"),
        };

        let agent = self
            .build_streaming_agent()
            .map_err(|err| AgentStreamError::fatal(err.to_string()))?;
        let response = agent
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.settings.api_key))
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(classify_agent_http_error)?;

        // 逐行读取 SSE 流：正文/思考链增量累积并回调；tool_calls 分片
        // 经累加器按 index 聚合（id/name 首片、arguments 逐片追加）。
        let mut response_body = response.into_body();
        let reader = BufReader::new(response_body.as_reader());
        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = StreamingToolCallAccumulator::default();
        let mut valid_chunks = 0usize;
        let mut malformed_data_lines = 0usize;
        // 截断诊断信号：是否收到 [DONE]、最后的 finish_reason、供应商在
        // 流中发的 error 事件（部分网关在上游故障时以 data: {"error":…} 结束）。
        let mut saw_done = false;
        let mut finish_reason: Option<String> = None;
        let mut stream_error: Option<String> = None;
        for line in reader.lines() {
            let line =
                line.map_err(|err| AgentStreamError::transient(format!("AI 流读取失败：{err}")))?;
            match parse_sse_line(&line) {
                Some(SseLineResult::Chunk(chunk)) => {
                    valid_chunks += 1;
                    if let Some(error) = chunk.error
                        && stream_error.is_none()
                    {
                        stream_error = Some(error.message.unwrap_or_else(|| "未知错误".into()));
                    }
                    for choice in chunk.choices {
                        if let Some(reason) = choice.finish_reason
                            && !reason.is_empty()
                        {
                            finish_reason = Some(reason);
                        }
                        if let Some(text) = choice.delta.content
                            && !text.is_empty()
                        {
                            full_content.push_str(&text);
                            on_delta(StreamDelta::Content(text));
                        }
                        if let Some(text) = choice.delta.reasoning_content
                            && !text.is_empty()
                        {
                            full_reasoning.push_str(&text);
                            on_delta(StreamDelta::Reasoning(text));
                        }
                        for delta in choice.delta.tool_calls {
                            tool_calls.push(delta);
                        }
                    }
                }
                Some(SseLineResult::Done) => {
                    saw_done = true;
                    break;
                }
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

        // 结束后统一剥离 `<think>`，与普通流式请求一致。
        let (content, extra_reasoning) = split_reasoning(&full_content);
        let reasoning = merge_reasoning(
            (!full_reasoning.trim().is_empty()).then(|| full_reasoning.trim().to_string()),
            extra_reasoning,
        );
        let reasoning = reasoning.filter(|reasoning| !reasoning.trim().is_empty());
        let content = content.trim().to_string();
        let tool_calls = tool_calls.finish();
        if let Some(message) = stream_error {
            return Err(AgentStreamError::transient(format!(
                "AI 流式响应返回错误：{message}"
            )));
        }
        // 截断检测：`finish_reason=length`（token 预算耗尽）与未收到
        // `[DONE]`（连接中途断开）都意味着内容不完整——即使已产出部分
        // 正文或工具调用也不能放行，否则最终结论会「看起来完成了」实际
        // 是半句话且无任何报错。两者都按可重试瞬态失败处理。
        if let Some(err) =
            agent_stream_truncation_message(saw_done, finish_reason.as_deref(), max_tokens)
        {
            return Err(err);
        }
        // 对 agent 回合而言「无正文且无工具调用」是无效回合（最终回答必须
        // 有正文，中间轮必须有调用），连同半截思考链一起按截断失败处理。
        if content.is_empty() && tool_calls.is_empty() {
            return Err(AgentStreamError::transient(
                agent_turn_empty_failure_message(valid_chunks, reasoning.is_some()).to_string(),
            ));
        }

        Ok(AgentTurn {
            content,
            reasoning,
            tool_calls,
        })
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
            tools: None,
            tool_choice: None,
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

        let (content, reasoning) =
            split_response_message_parts(choice.message.content, choice.message.reasoning_content);

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
            tools: None,
            tool_choice: None,
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
    /// 部分网关在上游故障时以 `data: {"error": {...}}` 事件结束流。
    #[serde(default)]
    error: Option<StreamErrorEvent>,
}

#[derive(Deserialize, Debug)]
struct StreamErrorEvent {
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize, Debug)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDeltaJson,
    /// 本片的结束原因（"stop"/"length"/…），最后非空值生效；截断诊断用。
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
struct StreamDeltaJson {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    /// 模型发起的工具调用分片（仅请求携带 tools 时出现）。
    #[serde(default)]
    tool_calls: Vec<StreamToolCallDelta>,
}

/// 流式 tool_calls 分片（`delta.tool_calls[]`）。
///
/// OpenAI 流式协议里一次工具调用按 `index` 分多片到达：id 与函数名通常
/// 在首片，`arguments` 逐片追加，由 `StreamingToolCallAccumulator` 聚合；
/// 个别兼容端点不回 index（serde 默认 0，单调用场景不受影响）。
#[derive(Deserialize, Default, Debug)]
struct StreamToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: StreamToolCallFunctionDelta,
}

#[derive(Deserialize, Default, Debug)]
struct StreamToolCallFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// 流式 tool_calls 分片累加器（纯逻辑，可单测）。
///
/// 按 index 聚合一次 agent 轮次的全部工具调用分片；`finish` 时按 index
/// 排序产出，无 name 的残片丢弃、id 缺失按 index 合成、空 arguments
/// 兜底 `{}`（与协议约定一致）。
#[derive(Default)]
struct StreamingToolCallAccumulator {
    calls: std::collections::BTreeMap<usize, PartialStreamToolCall>,
}

#[derive(Default)]
struct PartialStreamToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl StreamingToolCallAccumulator {
    fn push(&mut self, delta: StreamToolCallDelta) {
        let entry = self.calls.entry(delta.index).or_default();
        // id 只在首片出现；后片若重复携带取首个非空。
        if entry.id.is_none()
            && let Some(id) = delta.id
            && !id.is_empty()
        {
            entry.id = Some(id);
        }
        if let Some(name) = delta.function.name
            && !name.is_empty()
        {
            entry.name.push_str(&name);
        }
        if let Some(arguments) = delta.function.arguments {
            entry.arguments.push_str(&arguments);
        }
    }

    fn finish(self) -> Vec<AgentToolCall> {
        self.calls
            .into_iter()
            .filter(|(_, call)| !call.name.is_empty())
            .map(|(index, call)| AgentToolCall {
                id: call.id.unwrap_or_else(|| format!("call_{index}")),
                name: call.name,
                arguments: if call.arguments.is_empty() {
                    "{}".to_string()
                } else {
                    call.arguments
                },
            })
            .collect()
    }
}

/// 解析单行 SSE：`data: {...}` / `data:{...}`（SSE 规范允许省略冒号后的
/// 空格，个别兼容端点不带空格）→ Chunk，`data: [DONE]` → Done，
/// 其余（空行、非 data 前缀、JSON 解析失败）返回 None 容错跳过。
/// 无空格形式必须支持：整行跳过会丢 tool_calls 的 arguments 分片，
/// 聚合出的参数 JSON 随之损坏。
fn parse_sse_line(line: &str) -> Option<SseLineResult> {
    let trimmed = line.trim();
    let payload = trimmed.strip_prefix("data:")?.trim_start();
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

/// 响应 message 的正文/思考链提取（普通请求与 agent 轮共用）：
/// 优先使用单独的 reasoning_content 字段，否则从 content 中剥离 `<think>`。
fn split_response_message_parts(
    content: Option<String>,
    reasoning_content: Option<String>,
) -> (String, Option<String>) {
    let content = content.unwrap_or_default();
    if let Some(reasoning) = reasoning_content {
        let reasoning = reasoning.trim();
        let reasoning = if reasoning.is_empty() {
            None
        } else {
            Some(reasoning.to_string())
        };
        // 若 content 里还残留 <think>，再剥离一次，避免重复展示。
        let (cleaned, extra) = split_reasoning(&content);
        let reasoning = merge_reasoning(reasoning, extra);
        (cleaned, reasoning)
    } else {
        split_reasoning(&content)
    }
}

/// agent 循环消息 → 请求序列化形态。
fn agent_message_to_request(message: &AgentChatMessage) -> AgentRequestMessage<'_> {
    match message {
        AgentChatMessage::System(content) => AgentRequestMessage {
            role: "system",
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        },
        AgentChatMessage::User(content) => AgentRequestMessage {
            role: "user",
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        },
        AgentChatMessage::Assistant {
            content,
            tool_calls,
        } => AgentRequestMessage {
            role: "assistant",
            // 带 tool_calls 的 assistant 回复 content 序列化为 null（OpenAI 规范）
            content: (!content.is_empty()).then_some(content.as_str()),
            tool_calls: (!tool_calls.is_empty()).then(|| {
                tool_calls
                    .iter()
                    .map(|call| AgentRequestToolCall {
                        id: &call.id,
                        kind: "function",
                        function: AgentRequestFunction {
                            name: &call.name,
                            arguments: &call.arguments,
                        },
                    })
                    .collect()
            }),
            tool_call_id: None,
        },
        AgentChatMessage::Tool {
            tool_call_id,
            content,
        } => AgentRequestMessage {
            role: "tool",
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        },
    }
}

/// agent 请求 HTTP/网络错误的可重试性分流：
/// - 408（请求超时）/ 429（限流）/ 5xx（服务端故障）是瞬态故障，可重试
///   至默认上限；
/// - 其余 StatusCode（400/404/422 等）是配置或端点能力问题，重试不会有
///   结果，文案沿用 `agent_request_error` 分流直接失败；
/// - URL / 代理 / 协议要求类变体（BadUri、InvalidProxyUrl、RequireHttpsOnly、
///   TlsRequired）是用户配置问题，同样直接失败；
/// - **其余全部变体（Io、Timeout、HostNotFound、ConnectionFailed、TLS
///   握手失败、连接被重置及 `#[non_exhaustive]` 未来新增变体）是典型
///   瞬态网络故障，兜底按可重试处理**——连接阶段的网络错误正是自动重试
///   的核心目标，显式列举白名单会漏（旧实现只认 StatusCode，网络错误
///   全部落入不可重试，与注释语义相悖）。
pub fn classify_agent_http_error(err: ureq::Error) -> AgentStreamError {
    match &err {
        ureq::Error::StatusCode(code) if *code == 408 || *code == 429 || *code >= 500 => {
            AgentStreamError::transient(format!("AI 端点返回 HTTP {code}（服务暂不可用或过载）"))
        }
        ureq::Error::StatusCode(_) => AgentStreamError::fatal(agent_request_error(err).to_string()),
        ureq::Error::BadUri(_)
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::RequireHttpsOnly(_)
        | ureq::Error::TlsRequired => AgentStreamError::fatal(format!("AI 请求配置无效：{err}")),
        _ => AgentStreamError::transient(format!("AI 请求失败（网络错误）：{err}")),
    }
}

/// agent 流结束后的截断判定（纯函数，可单测）。
///
/// 两种信号都证明内容不完整，无论已产出多少正文/工具调用都必须按失败
/// 处理（放行会让最终评审「看似完成」实为半句话且无报错）：
/// 1. `finish_reason=length` —— 输出触及 max_tokens 上限被切断（reasoning
///    模型的思考与正文共用该预算，思考过长会挤占正文）。对同一请求基本
///    确定性，只允许重试 1 次；
/// 2. 未收到 `[DONE]` 结束标记 —— 连接在流完成前被供应商/网关中断，
///    属瞬态故障，重试至默认上限。
pub fn agent_stream_truncation_message(
    saw_done: bool,
    finish_reason: Option<&str>,
    max_tokens: u32,
) -> Option<AgentStreamError> {
    if finish_reason == Some("length") {
        return Some(AgentStreamError::transient_once(format!(
            "AI 本轮输出触及 max_tokens 上限（{max_tokens}）被截断，已产出的内容不完整\
             （reasoning 模型的思考与正文共用输出预算）；请重试、缩小比较范围或更换\
             输出上限更高的模型"
        )));
    }
    if !saw_done {
        return Some(AgentStreamError::transient(
            "AI 响应流在完成前被中断（未收到结束标记），已收到的内容不完整\
             （疑似供应商网关掐断了连接）"
                .to_string(),
        ));
    }
    None
}

/// agent 回合「无正文且无工具调用」的失败文案选择（纯函数，可单测）。
/// 截断信号（length / 无 DONE）已由 `agent_stream_truncation_message`
/// 先行拦截，这里只区分剩余成因。
pub fn agent_turn_empty_failure_message(valid_chunks: usize, has_reasoning: bool) -> &'static str {
    if valid_chunks == 0 {
        return "AI 响应流中没有有效数据块（供应商响应格式异常或响应被截断）";
    }
    if has_reasoning {
        return "AI 本轮只返回了思考过程，没有正文也没有工具调用（疑似被截断），\
                请重试或更换模型";
    }
    "AI 返回了空内容"
}

/// agent 请求 HTTP 错误的文案分流（不做降级，直接给出行动指引）：
/// - 400：成因不唯一（模型名/参数错误，或端点不支持工具调用），双因并提；
/// - 404/422：基本可以归因端点不支持工具调用（tools），引导更换供应商/模型；
/// - 其余错误原样透传。
pub fn agent_request_error(err: ureq::Error) -> GitError {
    if let ureq::Error::StatusCode(code @ (400 | 404 | 422)) = &err {
        return GitError::Message(match code {
            400 => format!(
                "AI 端点返回 HTTP 400，可能是模型名或请求参数错误，\
                 也可能该端点不支持工具调用（tools）；请检查 AI 设置中的模型名称，\
                 或更换支持工具调用的供应商/模型。原始错误：{err}"
            ),
            _ => format!(
                "AI 端点返回 HTTP {code}，疑似不支持工具调用（tools）。\
                 请在 AI 设置中更换支持工具调用的供应商或模型。原始错误：{err}"
            ),
        });
    }
    GitError::Message(format!("AI 请求失败：{err}"))
}

#[cfg(test)]
#[path = "../tests/ai/client.rs"]
mod tests;
