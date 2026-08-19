use super::*;

use crate::types::{BlameCommitInfo, ChangeState};

fn blame_commit() -> BlameCommitInfo {
    BlameCommitInfo {
        oid: "0123456789abcdef0123456789abcdef01234567".into(),
        short_oid: "0123456".into(),
        author: "张三".into(),
        time: 1_755_000_000,
        summary: "初始提交".into(),
    }
}

#[test]
fn clamp_line_range_defaults_and_window() {
    // 默认整个文件。
    assert_eq!(clamp_line_range(100, None, None), (1, 100));
    // 指定范围钳制在文件内。
    assert_eq!(clamp_line_range(100, Some(10), Some(30)), (10, 30));
    // end < start 抬到 start。
    assert_eq!(clamp_line_range(100, Some(50), Some(20)), (50, 50));
    // 超出文件的 start/end 钳到边界。
    assert_eq!(clamp_line_range(100, Some(0), Some(999)), (1, 100));
    // 窗口最宽 400 行。
    assert_eq!(clamp_line_range(1000, None, None), (1, 400));
    assert_eq!(clamp_line_range(1000, Some(900), Some(999)), (900, 999));
    assert_eq!(clamp_line_range(1000, Some(900), None), (900, 1000));
    // 空文件。
    assert_eq!(clamp_line_range(0, None, None), (1, 0));
}

#[test]
fn truncate_result_chars_annotates_only_when_over() {
    let short = "abc".repeat(10);
    assert_eq!(truncate_result_chars(&short, 100), short);

    let long = "x".repeat(300);
    let truncated = truncate_result_chars(&long, 100);
    assert!(truncated.starts_with(&"x".repeat(100)));
    assert!(truncated.contains("（结果过长，已截断）"));
}

#[test]
fn initial_prompt_full_diffs_within_budget() {
    let entries = vec![
        InitialDiffEntry {
            path: "src/lib.rs".into(),
            status_label: "M",
            patch: "+modified\n".into(),
        },
        InitialDiffEntry {
            path: "docs.md".into(),
            status_label: "A",
            patch: "+added line\n".into(),
        },
    ];
    let prompt = assemble_initial_user_prompt("feature/x", &entries);
    assert!(prompt.contains("目标分支：feature/x"));
    assert!(prompt.contains("变更文件（2 个）："));
    assert!(prompt.contains("- [M] src/lib.rs"));
    assert!(prompt.contains("- [A] docs.md"));
    // 预算内：diff 原样保留，无截断标注。
    assert!(prompt.contains("+modified\n"));
    assert!(!prompt.contains("已截断"));
}

#[test]
fn initial_prompt_truncates_per_file_over_budget() {
    let big_patch = "x".repeat(20_000);
    let entries = vec![
        InitialDiffEntry {
            path: "big1.rs".into(),
            status_label: "M",
            patch: big_patch.clone(),
        },
        InitialDiffEntry {
            path: "big2.rs".into(),
            status_label: "M",
            patch: big_patch,
        },
    ];
    let prompt = assemble_initial_user_prompt("feature/x", &entries);
    // 总量 40K > 30K 预算：进入逐文件截断模式。
    assert!(prompt.contains("已截断"));
    // 单文件保留 INITIAL_PER_FILE_CHARS 字符 + 标注。
    assert!(prompt.contains(&"x".repeat(INITIAL_PER_FILE_CHARS)));
    assert!(prompt.matches(&"x".repeat(INITIAL_PER_FILE_CHARS)).count() >= 2);
}

#[test]
fn initial_prompt_total_capped_in_truncation_mode() {
    // 200 个文件 × 20K：旧实现总量 = 200 × 4K = 800K 撑爆上下文；
    // 现在截断模式下总量仍受 INITIAL_DIFF_BUDGET_CHARS 二次封顶。
    let big_patch = "x".repeat(20_000);
    let entries: Vec<InitialDiffEntry> = (0..200)
        .map(|index| InitialDiffEntry {
            path: format!("file{index}.rs"),
            status_label: "M",
            patch: big_patch.clone(),
        })
        .collect();
    let prompt = assemble_initial_user_prompt("feature/x", &entries);
    // x 只来自 diff 本体；头部与截断标注现在一并计入预算扣减，
    // 总量严格不超过预算（不再为标注留每文件 20 字符余量）。
    let x_count = prompt.matches('x').count();
    assert!(
        x_count <= INITIAL_DIFF_BUDGET_CHARS,
        "x 字符总量 {x_count} 超出二次封顶"
    );
    // 预算耗尽后的文件降级为「仅清单 + read_diff 引导」。
    assert!(prompt.contains("差异未附：初始预算已用尽，请用 read_diff 获取"));
}

#[test]
fn tool_budget_force_finish_at_each_limit() {
    let mut budget = ToolBudget::default();
    assert!(!budget.force_finish());

    budget.rounds = MAX_TOOL_ROUNDS;
    assert!(budget.force_finish());

    budget.rounds = 0;
    budget.calls = MAX_TOOL_CALLS_TOTAL;
    assert!(budget.force_finish());

    budget.calls = 0;
    budget.note_result(MAX_TOTAL_TOOL_RESULT_CHARS);
    assert!(budget.force_finish());

    // 记账饱和不 panic。
    budget.note_result(usize::MAX);
    assert!(budget.result_chars >= MAX_TOTAL_TOOL_RESULT_CHARS);
}

#[test]
fn tool_budget_limit_reason_names_tripped_limit() {
    // 未触顶没有原因。
    let mut budget = ToolBudget::default();
    assert_eq!(budget.limit_reason(), None);

    // 三条线各自命名，首个触顶的优先（轮次 > 次数 > 体积）。
    budget.rounds = MAX_TOOL_ROUNDS;
    assert_eq!(budget.limit_reason(), Some("工具调用轮次上限"));

    budget.calls = MAX_TOOL_CALLS_TOTAL;
    assert_eq!(budget.limit_reason(), Some("工具调用轮次上限"));

    budget.rounds = 0;
    assert_eq!(budget.limit_reason(), Some("工具调用次数上限"));

    budget.calls = 0;
    budget.note_result(MAX_TOTAL_TOOL_RESULT_CHARS);
    assert_eq!(budget.limit_reason(), Some("工具结果累计体积上限"));
}

#[test]
fn blame_hunks_format_committed_and_uncommitted() {
    let hunks = vec![
        BlameHunkInfo {
            commit: Some(blame_commit()),
            start_line: 1,
            line_count: 3,
        },
        BlameHunkInfo {
            commit: None,
            start_line: 4,
            line_count: 2,
        },
    ];
    let text = format_blame_hunks(&hunks);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("L1-L3 0123456 张三 "));
    assert!(lines[0].ends_with(" 初始提交"));
    assert_eq!(lines[1], "L4-L5 （工作区未提交）");
}

#[test]
fn search_hits_and_history_format() {
    let hits = vec![CodeSearchMatch {
        path: "src/lib.rs".into(),
        lineno: 12,
        line: "fn target() {}".into(),
    }];
    assert_eq!(format_search_hits(&hits), "src/lib.rs:12: fn target() {}\n");

    let commit = CommitInfo {
        oid: "0123456789abcdef0123456789abcdef01234567".into(),
        short_oid: "0123456".into(),
        summary: "修复".into(),
        message: "修复".into(),
        author: "李四".into(),
        author_email: None,
        committer: "李四".into(),
        committer_email: None,
        time: 1_755_000_000,
        parents: vec![],
        refs: vec![],
    };
    let text = format_history_lines(std::slice::from_ref(&commit));
    assert!(text.starts_with("0123456 修复 — 李四, "));
}

#[test]
fn tool_args_summary_variants() {
    assert_eq!(
        tool_args_summary(
            "read_lines",
            r#"{"path":"src/lib.rs","start":100,"end":180}"#
        ),
        "read_lines src/lib.rs:100-180"
    );
    assert_eq!(
        tool_args_summary("read_lines", r#"{"path":"src/lib.rs"}"#),
        "read_lines src/lib.rs"
    );
    assert_eq!(tool_args_summary("get_file_tree", "{}"), "get_file_tree /");
    assert_eq!(
        tool_args_summary("search_code", r#"{"query":"TODO","is_regex":true}"#),
        "search_code TODO（正则）"
    );
    // 超长参数截到 60 字符。
    let long_query = "q".repeat(100);
    let summary = tool_args_summary("search_code", &format!(r#"{{"query":"{long_query}"}}"#));
    assert!(summary.contains('…') && summary.chars().count() < 80);
    // 坏 JSON 容错：路径显示为空而不是 panic。
    assert_eq!(tool_args_summary("read_diff", "not json"), "read_diff ");
    assert_eq!(tool_args_summary("mystery", "{}"), "mystery");
}

#[test]
fn patch_text_renders_line_kinds() {
    let diff = FileDiff {
        path: "a.txt".into(),
        scope: crate::types::DiffScope::Staged,
        is_binary: false,
        untracked: false,
        old_size: None,
        new_size: None,
        encoding: crate::types::DiffEncodingInfo {
            requested: DiffEncodingChoice::Auto,
            resolved: DiffEncodingChoice::Utf8,
            lossy: false,
        },
        lines: vec![
            crate::types::DiffLine {
                kind: crate::types::DiffLineKind::Header,
                content: "--- a.txt".into(),
                old_lineno: None,
                new_lineno: None,
                hunk_index: 0,
            },
            crate::types::DiffLine {
                kind: crate::types::DiffLineKind::Context,
                content: "keep".into(),
                old_lineno: Some(1),
                new_lineno: Some(1),
                hunk_index: 0,
            },
            crate::types::DiffLine {
                kind: crate::types::DiffLineKind::Removed,
                content: "old".into(),
                old_lineno: Some(2),
                new_lineno: None,
                hunk_index: 0,
            },
            crate::types::DiffLine {
                kind: crate::types::DiffLineKind::Added,
                content: "new".into(),
                old_lineno: None,
                new_lineno: Some(2),
                hunk_index: 0,
            },
        ],
    };
    let text = file_diff_to_patch_text(&diff);
    assert_eq!(text, "--- a.txt\n keep\n-old\n+new\n");

    // 二进制文件给占位说明。
    let binary = FileDiff {
        path: "img.png".into(),
        is_binary: true,
        lines: Vec::new(),
        ..diff
    };
    assert_eq!(
        file_diff_to_patch_text(&binary),
        "（img.png 是二进制文件）\n"
    );
}

/// 变更状态标签与初始上下文的对接：ChangeState::label 输出直接进清单。
#[test]
fn change_state_labels_feed_initial_entries() {
    assert_eq!(ChangeState::Added.label(), "A");
    assert_eq!(ChangeState::Renamed.label(), "R");
    let entries = vec![InitialDiffEntry {
        path: "renamed.rs".into(),
        status_label: ChangeState::Renamed.label(),
        patch: String::new(),
    }];
    let prompt = assemble_initial_user_prompt("t", &entries);
    assert!(prompt.contains("- [R] renamed.rs"));
    assert!(prompt.contains("（无文本差异）"));
}
