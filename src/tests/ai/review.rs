use super::*;

#[test]
fn reasoning_summary_derives_from_first_line() {
    // 首行非空文本作为摘要，短文本不截断。
    let step = AiReviewStep::Reasoning {
        text: "先看 src/lib.rs 的改动\n再确认调用方".into(),
    };
    assert_eq!(step.summary(), "思考：先看 src/lib.rs 的改动");

    // 跳过前导空行取首个非空行。
    let step = AiReviewStep::Reasoning {
        text: "\n   \n第二行才是内容".into(),
    };
    assert_eq!(step.summary(), "思考：第二行才是内容");
}

#[test]
fn reasoning_summary_truncates_long_first_line() {
    let long_line = "长".repeat(60);
    let step = AiReviewStep::Reasoning {
        text: long_line.clone(),
    };
    let summary = step.summary();
    assert!(summary.starts_with(&format!("思考：{}", "长".repeat(40))));
    assert!(summary.ends_with('…'));
    assert_eq!(summary.chars().count(), "思考：".chars().count() + 40 + 1);
}

#[test]
fn reasoning_summary_falls_back_on_blank_text() {
    let step = AiReviewStep::Reasoning {
        text: "  \n\t\n".into(),
    };
    assert_eq!(step.summary(), "思考");
}

#[test]
fn tool_call_summary_passthrough_and_error_flag() {
    let step = AiReviewStep::ToolCall {
        name: "read_lines".into(),
        args_summary: "read_lines src/lib.rs:1-100".into(),
        result_excerpt: "     1 | fn main()".into(),
        error: false,
    };
    assert_eq!(step.summary(), "read_lines src/lib.rs:1-100");
    assert_eq!(step.detail(), "     1 | fn main()");
    assert!(!step.is_error());

    let error_step = AiReviewStep::ToolCall {
        name: "get_blame".into(),
        args_summary: "get_blame gone.rs".into(),
        result_excerpt: "工具执行失败：文件不存在: gone.rs".into(),
        error: true,
    };
    assert!(error_step.is_error());
    assert_eq!(error_step.detail(), "工具执行失败：文件不存在: gone.rs");
}

#[test]
fn message_step_summary_and_not_collapsible() {
    let step = AiReviewStep::Message {
        text: "我先看看这个文件的实现，再确认调用方。".into(),
    };
    // 摘要取首行截断，但 UI 走整段直出（不折叠）。
    assert_eq!(
        step.summary(),
        "回复：我先看看这个文件的实现，再确认调用方。"
    );
    assert!(!step.is_collapsible());
    assert_eq!(step.detail(), "我先看看这个文件的实现，再确认调用方。");
    assert!(!step.is_error());

    // 多行正文摘要只取首行；超长截断。
    let step = AiReviewStep::Message {
        text: format!("{}\n第二行", "字".repeat(60)),
    };
    let summary = step.summary();
    assert!(summary.starts_with(&format!("回复：{}", "字".repeat(40))));
    assert!(summary.ends_with('…'));

    // 空白正文兜底「回复」。
    let step = AiReviewStep::Message { text: "  ".into() };
    assert_eq!(step.summary(), "回复");

    // 思维链与工具调用仍为折叠行。
    let reasoning = AiReviewStep::Reasoning {
        text: "思考".into(),
    };
    let tool = AiReviewStep::ToolCall {
        name: "read_lines".into(),
        args_summary: "read_lines a".into(),
        result_excerpt: String::new(),
        error: false,
    };
    assert!(reasoning.is_collapsible());
    assert!(tool.is_collapsible());
}
