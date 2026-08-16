use super::*;
use crate::ai::prompt::{ChatMessage, ChatRole};

/// 构造一个含两个冲突块和上下文的 diff3 样例文本。
fn sample_diff3() -> String {
    [
        "ctx-a\n",
        "ctx-b\n",
        "<<<<<<< OURS\n",
        "ours-one\n",
        "||||||| BASE\n",
        "base-one\n",
        "=======\n",
        "theirs-one\n",
        ">>>>>>> THEIRS\n",
        "ctx-c\n",
        "<<<<<<< OURS\n",
        "ours-two\n",
        "||||||| BASE\n",
        "base-two\n",
        "=======\n",
        "theirs-two\n",
        ">>>>>>> THEIRS\n",
        "ctx-d\n",
    ]
    .concat()
}

fn message(role: ChatRole, content: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: content.to_string(),
    }
}

#[test]
fn split_diff3_keeps_concatenation_identity_and_limit() {
    let text = sample_diff3();
    // 段上限 40 迫使在块边界切开；块本身（83 字符）超过段上限但在单块
    // 硬上限内，作为独立超限段完整放行。
    let segments = split_diff3_text(&text, 40, 200).unwrap();

    // 拼接恒等：所有段按顺序逐字节等于原文。
    let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(joined, text);

    for segment in &segments {
        // 纯上下文段不超过段上限；冲突段允许超到单块硬上限。
        let cap = if segment.has_conflicts { 200 } else { 40 };
        assert!(
            segment.text.chars().count() <= cap,
            "段超限：{:?}",
            segment.text
        );
        if segment.has_conflicts {
            // 标记平衡：有开始必有结束，块没有被切开。
            let starts = segment.text.matches("<<<<<<<").count();
            let ends = segment.text.matches(">>>>>>>").count();
            assert_eq!(starts, ends, "冲突块被切开：{}", segment.text);
        }
    }
    // 至少有一个含冲突的段和一个纯上下文段。
    assert!(segments.iter().any(|s| s.has_conflicts));
    assert!(segments.iter().any(|s| !s.has_conflicts));
}

#[test]
fn split_diff3_packs_multiple_context_lines_into_one_segment() {
    let text = sample_diff3();
    // 上限 26：ctx-a+ctx-b（12）与 ctx-c（6）各自成段不超限，
    // 单独验证上下文行的贪心装段（多行打包进同一段）。
    let segments = split_diff3_text(&text, 26, 200).unwrap();
    let ctx_segments: Vec<&MergeSegment> = segments.iter().filter(|s| !s.has_conflicts).collect();
    assert!(ctx_segments.len() >= 2);
    for segment in &ctx_segments {
        assert!(segment.text.chars().count() <= 26);
    }
    // 首个上下文段应打包了相邻的两行上下文。
    assert_eq!(ctx_segments[0].text, "ctx-a\nctx-b\n");
}

#[test]
fn split_diff3_whole_text_within_limit_stays_single_segment() {
    let text = sample_diff3();
    let segments = split_diff3_text(&text, 10_000, 20_000).unwrap();
    assert_eq!(segments.len(), 1);
    assert!(segments[0].has_conflicts);
    assert_eq!(segments[0].text, text);
}

#[test]
fn split_diff3_rejects_oversized_single_block() {
    let long_ours = format!("{}\n", "x".repeat(120));
    let text = format!(
        "<<<<<<< OURS\n{long_ours}||||||| BASE\n{long_ours}=======\n{long_ours}>>>>>>> THEIRS\n"
    );
    let error = split_diff3_text(&text, 200, 300).unwrap_err().to_string();
    assert!(error.contains("单个冲突块"), "unexpected: {error}");
}

#[test]
fn split_diff3_allows_oversized_block_as_standalone_segment() {
    // 块超过段上限但未超过单块硬上限：作为独立超限段完整放行。
    let long_ours = format!("{}\n", "x".repeat(60));
    let text = format!(
        "before\n<<<<<<< OURS\n{long_ours}||||||| BASE\n{long_ours}=======\n{long_ours}>>>>>>> THEIRS\nafter\n"
    );
    let segments = split_diff3_text(&text, 100, 300).unwrap();
    let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(joined, text);
    let oversized = segments
        .iter()
        .find(|s| s.text.chars().count() > 100)
        .expect("超限块应作为独立段");
    assert!(oversized.has_conflicts);
    assert!(oversized.text.contains("<<<<<<<"));
    assert!(oversized.text.contains(">>>>>>>"));
}

#[test]
fn split_diff3_rejects_unbalanced_markers() {
    let error = split_diff3_text("<<<<<<< OURS\nnever closed\n", 100, 200)
        .unwrap_err()
        .to_string();
    assert!(error.contains("不配对"), "unexpected: {error}");
}

#[test]
fn build_segment_messages_keeps_all_turns_within_budget() {
    let system = message(ChatRole::System, "sys");
    let history = vec![
        (
            message(ChatRole::User, "seg-1"),
            message(ChatRole::Assistant, "res-1"),
        ),
        (
            message(ChatRole::User, "seg-2"),
            message(ChatRole::Assistant, "res-2"),
        ),
    ];
    let current = message(ChatRole::User, "seg-3");
    let messages = build_segment_messages(system, &history, current, 1_000);
    // system + 2 完整回合 + 当前段。
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0].content, "sys");
    assert_eq!(messages[5].content, "seg-3");
    assert_eq!(messages[3].content, "seg-2");
}

#[test]
fn build_segment_messages_drops_oldest_turns_over_budget() {
    let system = message(ChatRole::System, "sys");
    let history = vec![
        (
            message(ChatRole::User, &"old-1 ".repeat(10)),
            message(ChatRole::Assistant, "res-1"),
        ),
        (
            message(ChatRole::User, &"new-1 ".repeat(2)),
            message(ChatRole::Assistant, "res-2"),
        ),
    ];
    let current = message(ChatRole::User, "cur");
    // 预算只够 system + 最新一回合 + 当前段。
    let budget = "sys".len() + "new-1 new-1 ".len() + "res-2".len() + "cur".len();
    let messages = build_segment_messages(system, &history, current, budget);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].content, "new-1 new-1 ");
    assert_eq!(messages[2].content, "res-2");
    assert_eq!(messages[3].content, "cur");
    assert!(
        !messages.iter().any(|m| m.content.contains("old-1")),
        "最旧回合应被滑窗丢弃"
    );
}

#[test]
fn strip_code_fence_removes_fenced_wrapper() {
    assert_eq!(
        strip_code_fence("```diff3\nresolved content\n```"),
        "resolved content\n"
    );
    assert_eq!(strip_code_fence("```\nplain\n```"), "plain\n");
    // 前后空白与围栏后空行一并清理，尾部换行保留。
    assert_eq!(strip_code_fence("\n\n```text\nbody\n\n```\n"), "body\n\n");
}

#[test]
fn strip_code_fence_keeps_unfenced_text_untouched() {
    // 无围栏：原样返回（含尾部空白差异也保持 trim 前原文）。
    assert_eq!(strip_code_fence("plain text"), "plain text");
    assert_eq!(strip_code_fence("plain text\n"), "plain text\n");
    // 只有开头像围栏、结尾不是：不剥。
    assert_eq!(
        strip_code_fence("```rust\nfn main() {}\n"),
        "```rust\nfn main() {}\n"
    );
    // 单行围栏：无正文可剥，原样返回。
    assert_eq!(strip_code_fence("```"), "```");
}

#[test]
fn response_conflict_marker_detection() {
    assert!(response_contains_conflict_markers(
        "<<<<<<< OURS\ncontent\n"
    ));
    assert!(response_contains_conflict_markers("kept\n>>>>>>> THEIRS\n"));
    assert!(response_contains_conflict_markers("||||||| BASE\n"));
    // 七个等号单独出现是正文合法内容（RST/Markdown 分隔线），不判定。
    assert!(!response_contains_conflict_markers("content\n=======\n"));
    // 缩进的标记不算行首标记。
    assert!(!response_contains_conflict_markers(
        "    <<<<<<< indented\n"
    ));
    // 普通代码中的小于号注释不误报。
    assert!(!response_contains_conflict_markers("let x = a << b;\n"));
}
