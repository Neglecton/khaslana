use super::*;
use crate::types::{DiffEncodingChoice, DiffEncodingInfo, DiffScope};

fn rs_lines() -> Vec<String> {
    vec![
        "// 注释".to_string(),
        "pub fn main() {".to_string(),
        "    let x = 42;".to_string(),
        "}".to_string(),
    ]
}

fn diff_line(kind: DiffLineKind, content: &str) -> crate::types::DiffLine {
    crate::types::DiffLine {
        kind,
        old_lineno: None,
        new_lineno: None,
        content: content.to_string(),
        hunk_index: 0,
    }
}

// span 拼接与原行字节恒等：渲染侧按区间上色不能吞字或重叠。
#[test]
fn spans_concatenate_to_original_line() {
    let spans = highlight("a.rs", &rs_lines(), false).unwrap();
    assert_eq!(spans.lines.len(), rs_lines().len());
    for (spans, line) in spans.lines.iter().zip(rs_lines()) {
        let mut rebuilt = String::new();
        for span in spans {
            assert!(span.end > span.start, "不允许零长度 span");
            assert!(
                span.start <= line.len() && span.end <= line.len(),
                "span 越界"
            );
            rebuilt.push_str(&line[span.start..span.end]);
        }
        assert_eq!(rebuilt, line, "span 拼接必须还原整行（含中文）");
    }
}

// 相邻同色 span 必须合并。
#[test]
fn adjacent_same_color_spans_merged() {
    let spans = highlight("a.rs", &rs_lines(), false).unwrap();
    for line in &spans.lines {
        for pair in line.windows(2) {
            assert!(
                pair[0].color != pair[1].color || pair[0].end < pair[1].start,
                "相邻同色未合并"
            );
        }
    }
}

// 扩展名检测：命中返回 Some，未知扩展/无扩展返回 None。
#[test]
fn detects_language_by_extension() {
    let lines = vec!["hello".to_string()];
    assert!(highlight("src/main.rs", &lines, false).is_some());
    assert!(highlight("script.py", &lines, false).is_some());
    assert!(highlight("README.md", &lines, false).is_some());
    assert!(highlight("data.xyz", &lines, false).is_none());
    assert!(highlight("noext", &lines, false).is_none());
}

// 无扩展名的常见文件名按语法名兜底命中（默认语法集含 Makefile，不含 Dockerfile）。
#[test]
fn detects_makefile_by_filename() {
    let lines = vec!["all:".to_string()];
    assert!(highlight("Makefile", &lines, false).is_some());
    assert!(highlight("build/GNUmakefile", &lines, false).is_some());
}

// 深浅主题产出不同颜色。
#[test]
fn dark_and_light_themes_differ() {
    let light = highlight("a.rs", &rs_lines(), false).unwrap();
    let dark = highlight("a.rs", &rs_lines(), true).unwrap();
    assert!(!light.dark);
    assert!(dark.dark);
    let light_colors: Vec<_> = light.lines.iter().flatten().map(|s| s.color).collect();
    let dark_colors: Vec<_> = dark.lines.iter().flatten().map(|s| s.color).collect();
    assert_ne!(light_colors, dark_colors);
}

// 体积/行数守卫：超限回退 None。
#[test]
fn oversized_input_skips_highlight() {
    let too_many_lines: Vec<String> = (0..=SYNTAX_MAX_LINES)
        .map(|index| format!("let v{index} = {index};"))
        .collect();
    assert!(highlight("a.rs", &too_many_lines, false).is_none());

    let too_many_bytes = vec!["x".repeat(SYNTAX_MAX_BYTES + 1)];
    assert!(highlight("a.rs", &too_many_bytes, false).is_none());
}

// 空文件与空行安全。
#[test]
fn empty_input_is_safe() {
    let empty = highlight("a.rs", &[], false);
    assert!(empty.is_some());
    assert!(empty.unwrap().lines.is_empty());

    let with_blank = highlight("a.rs", &["".to_string(), "fn f() {}".to_string()], false).unwrap();
    assert!(with_blank.lines[0].is_empty());
}

// diff 全文：索引与 FileDiff.lines 对齐，Header/hunk/EOFNL 行产空 vec。
#[test]
fn diff_highlight_index_aligned_and_headers_skipped() {
    let diff = FileDiff {
        path: "a.rs".to_string(),
        scope: DiffScope::Unstaged,
        is_binary: false,
        old_size: None,
        new_size: None,
        encoding: DiffEncodingInfo {
            requested: DiffEncodingChoice::Auto,
            resolved: DiffEncodingChoice::Utf8,
            lossy: false,
        },
        lines: vec![
            diff_line(DiffLineKind::Header, "diff --git a/a.rs b/a.rs"),
            diff_line(DiffLineKind::Header, "@@ -1,3 +1,3 @@"),
            diff_line(DiffLineKind::Context, "fn a() {"),
            diff_line(DiffLineKind::Removed, "    let x = 1;"),
            diff_line(DiffLineKind::Added, "    let x = 2;"),
            diff_line(DiffLineKind::Context, "\\ No newline at end of file"),
        ],
    };
    let spans = highlight_diff_lines(&diff, false).unwrap();
    assert_eq!(spans.lines.len(), diff.lines.len());
    assert!(spans.lines[0].is_empty(), "文件头不高亮");
    assert!(spans.lines[1].is_empty(), "hunk 头不高亮");
    assert!(!spans.lines[2].is_empty(), "上下文行应高亮");
    assert!(!spans.lines[3].is_empty(), "删除行应高亮");
    assert!(!spans.lines[4].is_empty(), "新增行应高亮");
    assert!(spans.lines[5].is_empty(), "EOFNL 标记行不高亮");

    // 代码行拼接还原（区间必须落在行内容内）
    for (index, line) in diff.lines.iter().enumerate() {
        let mut rebuilt = String::new();
        for span in &spans.lines[index] {
            rebuilt.push_str(&line.content[span.start..span.end]);
        }
        if !spans.lines[index].is_empty() {
            assert_eq!(rebuilt, line.content);
        }
    }
}
