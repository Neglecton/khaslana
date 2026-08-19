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

// 生成结果空正文校验：正常内容（含 trim）/ 纯空白 / 仅思考过程 / 完全为空。
fn chat_result_of(content: &str, reasoning: Option<&str>) -> ChatResult {
    ChatResult {
        content: content.to_string(),
        reasoning: reasoning.map(str::to_string),
    }
}

#[test]
fn validate_generated_content_accepts_and_trims_normal_content() {
    let result = chat_result_of("  feat: xxx\n", None);
    let validated = validate_generated_content(&result, "空文案", "仅思考文案").unwrap();
    assert_eq!(validated, "feat: xxx");
}

#[test]
fn validate_generated_content_rejects_whitespace_only() {
    let result = chat_result_of("   \n\t ", None);
    let error = validate_generated_content(&result, "空文案", "仅思考文案")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "空文案");
}

#[test]
fn validate_generated_content_rejects_reasoning_only_with_distinct_message() {
    let result = chat_result_of("", Some("模型思考过程…"));
    let error = validate_generated_content(&result, "空文案", "仅思考文案")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "仅思考文案");

    // 纯空白的思考链视同没有。
    let result = chat_result_of("", Some("   "));
    let error = validate_generated_content(&result, "空文案", "仅思考文案")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "空文案");
}

// ── Agentic 工具调用协议 ─────────────────────────────────────────────────

#[test]
fn agent_message_serialization_shapes() {
    // system / user：普通 {role, content}。
    let system_message = AgentChatMessage::System("规则".into());
    let system = agent_message_to_request(&system_message);
    let value = serde_json::to_value(&system).unwrap();
    assert_eq!(value["role"], "system");
    assert_eq!(value["content"], "规则");
    assert!(value.get("tool_calls").is_none());
    assert!(value.get("tool_call_id").is_none());

    let user_message = AgentChatMessage::User("输入".into());
    let user = agent_message_to_request(&user_message);
    assert_eq!(serde_json::to_value(&user).unwrap()["role"], "user");

    // assistant 带 tool_calls 且正文为空：content 序列化为 null，tool_calls 完整回填。
    let assistant_message = AgentChatMessage::Assistant {
        content: String::new(),
        tool_calls: vec![AgentToolCall {
            id: "call_1".into(),
            name: "read_lines".into(),
            arguments: r#"{"path":"src/lib.rs"}"#.into(),
        }],
    };
    let assistant = agent_message_to_request(&assistant_message);
    let value = serde_json::to_value(&assistant).unwrap();
    assert_eq!(value["role"], "assistant");
    assert!(value["content"].is_null());
    assert_eq!(value["tool_calls"][0]["id"], "call_1");
    assert_eq!(value["tool_calls"][0]["type"], "function");
    assert_eq!(value["tool_calls"][0]["function"]["name"], "read_lines");
    assert_eq!(
        value["tool_calls"][0]["function"]["arguments"],
        r#"{"path":"src/lib.rs"}"#
    );

    // assistant 纯文本回复：content 为字符串且不携带 tool_calls 字段。
    let plain_message = AgentChatMessage::Assistant {
        content: "结论".into(),
        tool_calls: Vec::new(),
    };
    let plain = agent_message_to_request(&plain_message);
    let value = serde_json::to_value(&plain).unwrap();
    assert_eq!(value["content"], "结论");
    assert!(value.get("tool_calls").is_none());

    // tool 结果消息：{role:"tool", content, tool_call_id}。
    let tool_message = AgentChatMessage::Tool {
        tool_call_id: "call_1".into(),
        content: "文件内容…".into(),
    };
    let tool = agent_message_to_request(&tool_message);
    let value = serde_json::to_value(&tool).unwrap();
    assert_eq!(value["role"], "tool");
    assert_eq!(value["tool_call_id"], "call_1");
    assert_eq!(value["content"], "文件内容…");
}

#[test]
fn agent_request_omits_tools_when_none() {
    let user_message = AgentChatMessage::User("hi".into());
    let messages = vec![agent_message_to_request(&user_message)];
    let body = ChatCompletionsRequest {
        model: "test-model",
        messages,
        temperature: 0.2,
        max_tokens: 4000,
        stream: false,
        tools: None,
        tool_choice: None,
    };
    let value = serde_json::to_value(&body).unwrap();
    assert!(value.get("tools").is_none());
    assert!(value.get("tool_choice").is_none());
    assert_eq!(value["stream"], false);
    assert_eq!(value["max_tokens"], 4000);

    // 携带工具时 tools/tool_choice 均出现且形状正确。
    let schema = ToolSchema {
        name: "search_code",
        description: "搜索代码",
        parameters: serde_json::json!({"type":"object","properties":{}}),
    };
    let body = ChatCompletionsRequest {
        model: "test-model",
        messages: vec![agent_message_to_request(&user_message)],
        temperature: 0.2,
        max_tokens: 4000,
        stream: false,
        tools: Some(vec![ToolDefinition {
            kind: "function",
            function: ToolFunction {
                name: schema.name,
                description: schema.description,
                parameters: &schema.parameters,
            },
        }]),
        tool_choice: Some("auto"),
    };
    let value = serde_json::to_value(&body).unwrap();
    assert_eq!(value["tools"][0]["type"], "function");
    assert_eq!(value["tools"][0]["function"]["name"], "search_code");
    assert_eq!(value["tool_choice"], "auto");
}

#[test]
fn streaming_tool_call_accumulator_assembles_fragments() {
    // OpenAI 流式协议：id/name 首片、arguments 逐片追加，按 index 聚合。
    let mut accumulator = StreamingToolCallAccumulator::default();
    let chunks = [
        r#"{"index":0,"id":"call_a","function":{"name":"read_lines","arguments":""}}"#,
        r#"{"index":0,"function":{"arguments":"{\"path\""}}"#,
        r#"{"index":0,"function":{"arguments":":\"src/lib.rs\"}"}}"#,
        // 同轮第二个工具调用，index 乱序到达也能按序产出。
        r#"{"index":1,"id":"call_b","function":{"name":"read_diff","arguments":"{}"}}"#,
    ];
    for chunk in chunks {
        accumulator.push(serde_json::from_str(chunk).unwrap());
    }
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_a");
    assert_eq!(calls[0].name, "read_lines");
    assert_eq!(calls[0].arguments, r#"{"path":"src/lib.rs"}"#);
    assert_eq!(calls[1].id, "call_b");
    assert_eq!(calls[1].name, "read_diff");

    // id 缺失按 index 合成、arguments 为空回退 "{}"、无 name 的残片被过滤。
    let mut accumulator = StreamingToolCallAccumulator::default();
    let chunks = [
        r#"{"index":0,"function":{"name":"search_code"}}"#,
        r#"{"index":1,"id":"keep","function":{"arguments":"{}"}}"#,
    ];
    for chunk in chunks {
        accumulator.push(serde_json::from_str(chunk).unwrap());
    }
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_0");
    assert_eq!(calls[0].name, "search_code");
    assert_eq!(calls[0].arguments, "{}");

    // id 重复携带时只认首个非空。
    let mut accumulator = StreamingToolCallAccumulator::default();
    accumulator
        .push(serde_json::from_str(r#"{"index":0,"id":"first","function":{"name":"x"}}"#).unwrap());
    accumulator.push(
        serde_json::from_str(r#"{"index":0,"id":"second","function":{"arguments":"{}"}}"#).unwrap(),
    );
    let calls = accumulator.finish();
    assert_eq!(calls[0].id, "first");
}

#[test]
fn parse_sse_line_tool_call_delta_chunk() {
    // 含 tool_calls 分片的 SSE 行经既有 parse_sse_line 通道解析。
    let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_diff","arguments":"{"}}]}}]}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert_eq!(chunk.choices.len(), 1);
            let deltas = &chunk.choices[0].delta.tool_calls;
            assert_eq!(deltas.len(), 1);
            assert_eq!(deltas[0].index, 0);
            assert_eq!(deltas[0].id.as_deref(), Some("c1"));
            assert_eq!(deltas[0].function.name.as_deref(), Some("read_diff"));
            assert_eq!(deltas[0].function.arguments.as_deref(), Some("{"));
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
    // 无 tool_calls 的旧格式 chunk 仍正常（serde default 空 vec）。
    let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert!(chunk.choices[0].delta.tool_calls.is_empty());
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
}

#[test]
fn agent_request_error_branches_by_status_code() {
    // 404/422：归因端点不支持工具调用，引导更换供应商。
    for code in [404u16, 422] {
        let message = agent_request_error(ureq::Error::StatusCode(code)).to_string();
        assert!(
            message.contains("疑似不支持工具调用"),
            "HTTP {code} 应引导更换供应商：{message}"
        );
        assert!(message.contains(&code.to_string()));
    }
    // 400：成因不唯一，双因并提（模型名/参数错误 或 不支持工具调用）。
    let message = agent_request_error(ureq::Error::StatusCode(400)).to_string();
    assert!(message.contains("模型名或请求参数错误"), "got: {message}");
    assert!(message.contains("工具调用"), "got: {message}");
    assert!(!message.contains("疑似不支持工具调用"));
    // 其余状态码原样透传，不带更换供应商提示。
    let message = agent_request_error(ureq::Error::StatusCode(401)).to_string();
    assert!(!message.contains("疑似不支持工具调用"));
    assert!(message.contains("401"));
}

#[test]
fn split_response_message_parts_prefers_reasoning_field() {
    // 单独 reasoning_content 字段优先，同时剥掉 content 里残留的 <think>。
    let (content, reasoning) = split_response_message_parts(
        Some("正文<think>额外</think>".into()),
        Some("原生思考".into()),
    );
    assert_eq!(content, "正文");
    assert_eq!(reasoning.as_deref(), Some("原生思考\n\n额外"));

    // 无 reasoning_content 时走 <think> 剥离路径。
    let (content, reasoning) =
        split_response_message_parts(Some("<think>思考</think>正文".into()), None);
    assert_eq!(content, "正文");
    assert_eq!(reasoning.as_deref(), Some("思考"));

    // 空白 reasoning_content 视同没有。
    let (content, reasoning) = split_response_message_parts(Some("正文".into()), Some("  ".into()));
    assert_eq!(content, "正文");
    assert_eq!(reasoning, None);
}

#[test]
fn classify_agent_http_error_marks_transient_failures_retryable() {
    // 408 / 429 / 5xx 是瞬态故障，可自动重试。
    for code in [408u16, 429, 500, 502, 503, 504] {
        let err = classify_agent_http_error(ureq::Error::StatusCode(code));
        assert!(err.retryable(), "HTTP {code} 应可重试");
        assert!(err.message().contains(&code.to_string()), "got: {}", err);
    }
    // 配置类 / 端点能力类错误重试无意义，直接失败。
    for code in [400u16, 401, 403, 404, 422] {
        let err = classify_agent_http_error(ureq::Error::StatusCode(code));
        assert!(!err.retryable(), "HTTP {code} 不应重试");
    }
    // 400 的文案沿用双因归因（模型名/参数 或 不支持工具调用）。
    let err = classify_agent_http_error(ureq::Error::StatusCode(400));
    assert!(err.message().contains("模型名或请求参数错误"), "got: {err}");
}

#[test]
fn classify_agent_http_error_retries_network_variants() {
    // 连接阶段网络错误（连接被重置 / DNS 失败）是瞬态故障，必须可重试
    // ——旧实现只认 StatusCode，网络错误全部落入不可重试，与注释相悖。
    let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
    let err = classify_agent_http_error(ureq::Error::Io(io_err));
    assert!(err.retryable(), "Io 错误应可重试：{err}");
    assert!(err.message().contains("网络错误"), "got: {err}");
    assert_eq!(err.retry_ceiling(), AGENT_STREAM_MAX_RETRIES);

    let err = classify_agent_http_error(ureq::Error::HostNotFound);
    assert!(err.retryable(), "DNS 失败应可重试：{err}");

    // URL / 代理 / 协议要求类是配置问题，重试无意义。
    let err = classify_agent_http_error(ureq::Error::InvalidProxyUrl);
    assert!(!err.retryable(), "代理配置错误不应重试：{err}");
}

#[test]
fn agent_turn_empty_failure_message_selects_cause() {
    // 没有任何有效数据块：供应商格式异常或连接被立刻掐断。
    assert!(agent_turn_empty_failure_message(0, false).contains("没有有效数据块"));
    assert!(agent_turn_empty_failure_message(0, true).contains("没有有效数据块"));

    // 完整回复但仅思考过程：疑似模型行为或隐性截断。
    assert!(agent_turn_empty_failure_message(5, true).contains("只返回了思考过程"));

    // 完整回复且什么都没有：空内容。
    assert_eq!(
        agent_turn_empty_failure_message(5, false),
        "AI 返回了空内容"
    );
}

#[test]
fn agent_stream_truncation_message_flags_incomplete_turns() {
    // finish_reason=length：无论已产出多少内容都是截断（含 max_tokens 数值）；
    // 基本确定性失败只允许重试 1 次。
    let err = agent_stream_truncation_message(true, Some("length"), 8192).expect("length 应判截断");
    assert!(err.message().contains("max_tokens"), "got: {}", err);
    assert!(err.message().contains("8192"), "got: {}", err);
    assert_eq!(err.retry_ceiling(), 1);
    // 即使 [DONE] 已收到（协议完整走完）也一样是 token 预算截断。
    assert!(agent_stream_truncation_message(false, Some("length"), 8192).is_some());

    // 未收到 [DONE]：连接中途断开，瞬态故障重试至默认上限。
    let err =
        agent_stream_truncation_message(false, Some("stop"), 8192).expect("无结束标记应判截断");
    assert!(err.message().contains("未收到结束标记"), "got: {err}");
    assert_eq!(err.retry_ceiling(), AGENT_STREAM_MAX_RETRIES);
    assert!(agent_stream_truncation_message(false, None, 8192).is_some());

    // 正常结束（有 [DONE]、非 length）：不判截断。
    assert!(agent_stream_truncation_message(true, Some("stop"), 8192).is_none());
    assert!(agent_stream_truncation_message(true, None, 8192).is_none());
}

#[test]
fn parse_sse_line_reads_error_event_and_finish_reason() {
    // 网关在流中发 error 事件：choices 为空但 error 有消息，解析不失败。
    let line = r#"data: {"error":{"message":"upstream overloaded"}}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert!(chunk.choices.is_empty());
            assert_eq!(
                chunk.error.as_ref().and_then(|e| e.message.as_deref()),
                Some("upstream overloaded")
            );
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
    // finish_reason 分片解析（最后一片通常带 stop/length）。
    let line = r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("length"));
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
    // 旧格式（无 error / finish_reason 字段）不受影响。
    let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert!(chunk.error.is_none());
            assert_eq!(chunk.choices[0].finish_reason, None);
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
}

#[test]
fn parse_sse_line_accepts_data_prefix_without_space() {
    // SSE 规范允许 "data:" 后省略空格；不识别会整行跳过，agent 流丢
    // tool_calls 的 arguments 分片导致聚合出的参数 JSON 损坏。
    let line = r#"data:{"choices":[{"delta":{"content":"hi"}}]}"#;
    match parse_sse_line(line) {
        Some(SseLineResult::Chunk(chunk)) => {
            assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
        }
        other => panic!("expected Chunk, got {other:?}"),
    }
    // 结束标记同样支持无空格形式。
    assert!(matches!(
        parse_sse_line("data:[DONE]"),
        Some(SseLineResult::Done)
    ));
    // 非 data 前缀不受影响。
    assert!(parse_sse_line("event: message").is_none());
}
