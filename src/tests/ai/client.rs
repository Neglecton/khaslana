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
