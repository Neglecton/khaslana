use super::*;

fn plain(text: &str) -> MdInlineSpan {
    MdInlineSpan {
        text: text.to_string(),
        ..Default::default()
    }
}

// 所有块的行内 span 拼接应还原全部文字内容（渲染不能吞字）。
fn all_block_text(blocks: &[MdBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            MdBlock::Heading { spans, .. } => {
                for span in spans {
                    out.push_str(&span.text);
                }
            }
            MdBlock::Paragraph(lines) => {
                for line in lines {
                    for span in &line.spans {
                        out.push_str(&span.text);
                    }
                    out.push('\n');
                }
            }
            MdBlock::Code { text } => out.push_str(text),
            MdBlock::Quote(inner) => out.push_str(&all_block_text(inner)),
            MdBlock::Rule => {}
        }
    }
    out
}

#[test]
fn empty_input_yields_no_blocks() {
    assert!(parse_markdown_blocks("").is_empty());
}

#[test]
fn heading_levels_parsed() {
    let blocks = parse_markdown_blocks("# 标题一\n\n### 标题三");
    assert_eq!(
        blocks,
        vec![
            MdBlock::Heading {
                level: 1,
                spans: vec![plain("标题一")],
            },
            MdBlock::Heading {
                level: 3,
                spans: vec![plain("标题三")],
            },
        ]
    );
}

#[test]
fn softbreak_splits_lines_within_paragraph() {
    let blocks = parse_markdown_blocks("第一行\n第二行");
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        MdBlock::Paragraph(lines) => {
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0].spans, vec![plain("第一行")]);
            assert_eq!(lines[1].spans, vec![plain("第二行")]);
        }
        _ => panic!("应为段落"),
    }
}

#[test]
fn bold_italic_strike_and_inline_code_styles() {
    let blocks = parse_markdown_blocks("普通 **加粗** *斜体* ~~删除~~ `代码`");
    let MdBlock::Paragraph(lines) = &blocks[0] else {
        panic!("应为段落")
    };
    let spans = &lines[0].spans;
    // 相邻同样式已合并；逐个校验样式标记
    let bold = spans.iter().find(|s| s.text == "加粗").unwrap();
    assert!(bold.bold && !bold.italic && !bold.code);
    let italic = spans.iter().find(|s| s.text == "斜体").unwrap();
    assert!(italic.italic && !italic.bold);
    let strike = spans.iter().find(|s| s.text == "删除").unwrap();
    assert!(strike.strike);
    let code = spans.iter().find(|s| s.text == "代码").unwrap();
    assert!(code.code && !code.bold && !code.italic);
}

#[test]
fn fenced_code_block_keeps_lines() {
    let blocks = parse_markdown_blocks("前文\n\n```rust\nfn a() {\n    1\n}\n```\n\n后文");
    assert_eq!(blocks.len(), 3);
    match &blocks[1] {
        MdBlock::Code { text } => {
            assert_eq!(text, "fn a() {\n    1\n}\n");
        }
        _ => panic!("第二块应为代码块"),
    }
}

#[test]
fn unordered_list_markers_and_indent() {
    let blocks = parse_markdown_blocks("- 甲\n- 乙\n  - 乙.一\n- 丙");
    assert_eq!(blocks.len(), 1);
    let MdBlock::Paragraph(lines) = &blocks[0] else {
        panic!("列表应扁平化为单段多行")
    };
    assert_eq!(lines.len(), 4);
    // 前缀是行首 muted span
    assert_eq!(lines[0].spans[0].text, "• ");
    assert!(lines[0].spans[0].muted);
    assert_eq!(lines[0].spans[1].text, "甲");
    assert_eq!(lines[0].indent, 1);
    // 嵌套项缩进加深
    assert_eq!(lines[2].indent, 2);
    assert_eq!(lines[2].spans[1].text, "乙.一");
}

#[test]
fn ordered_list_uses_numbered_markers() {
    let blocks = parse_markdown_blocks("3. 第三\n4. 第四");
    let MdBlock::Paragraph(lines) = &blocks[0] else {
        panic!("应为段落")
    };
    assert_eq!(lines[0].spans[0].text, "3. ");
    assert_eq!(lines[1].spans[0].text, "4. ");
}

#[test]
fn block_quote_wraps_inner_blocks() {
    let blocks = parse_markdown_blocks("> 引用文本\n\n正文");
    assert_eq!(blocks.len(), 2);
    match &blocks[0] {
        MdBlock::Quote(inner) => {
            // all_block_text 对段内每行追加换行（行间分隔），引用内单行
            // 文本同样带行尾换行。
            assert_eq!(all_block_text(inner), "引用文本\n");
        }
        _ => panic!("第一块应为引用"),
    }
}

#[test]
fn horizontal_rule_parsed() {
    let blocks = parse_markdown_blocks("上\n\n---\n\n下");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[1], MdBlock::Rule);
}

#[test]
fn link_text_kept_without_interactive_parts() {
    let blocks = parse_markdown_blocks("见 [文档](https://example.com) 说明");
    let text = all_block_text(&blocks);
    assert!(text.contains("文档"));
    assert!(!text.contains("https://example.com"));
}

// 流式半截输入：未闭合围栏与未闭合加粗不 panic，且产出合理块
//（CommonMark 解析器在 EOF 自动收尾）。
#[test]
fn streaming_partial_document_is_safe() {
    let blocks = parse_markdown_blocks("## 评审中\n\n- 问题一：**重点");
    assert!(!blocks.is_empty());

    let blocks = parse_markdown_blocks("开始\n\n```\nfn unfinished(");
    match blocks.last() {
        Some(MdBlock::Code { text }) => assert!(text.contains("fn unfinished(")),
        _ => panic!("未闭合围栏应收尾为代码块"),
    }
}

// 行内 span 拼接还原整行文字（合并逻辑不能吞字或重复）。
#[test]
fn spans_concatenate_to_original_text() {
    let source = "混合 **粗** 与 `code` 及 *斜*";
    let blocks = parse_markdown_blocks(source);
    let rebuilt = all_block_text(&blocks);
    // 还原文字（去掉 markdown 标记字符）
    assert!(rebuilt.contains("混合"));
    assert!(rebuilt.contains("粗"));
    assert!(rebuilt.contains("与"));
    assert!(rebuilt.contains("code"));
    assert!(rebuilt.contains("及"));
    assert!(rebuilt.contains("斜"));
    assert!(!rebuilt.contains("**"));
    assert!(!rebuilt.contains('`'));
}

#[test]
fn task_list_marker_keeps_checkbox_text() {
    let blocks = parse_markdown_blocks("- [x] 已完成项\n- [ ] 待办项\n");
    let text = all_block_text(&blocks);
    // 勾选框字符保留在列表前缀之后，不被吞掉。
    assert_eq!(text, "• [x] 已完成项\n• [ ] 待办项\n");
}
