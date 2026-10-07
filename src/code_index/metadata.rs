//! 定义处的签名与文档，只取声明和相邻注释，避免把函数体噪声加入搜索。

use tree_sitter::Node;

pub(super) fn symbol_metadata(node: Node, source: &[u8]) -> (String, String) {
    let declaration = node
        .child_by_field_name("value")
        .filter(|value| {
            matches!(
                value.kind(),
                "arrow_function" | "function_expression" | "function"
            )
        })
        .unwrap_or(node);
    let end = declaration
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or(declaration.end_byte());
    let signature = String::from_utf8_lossy(&source[node.start_byte()..end])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // 注释可能附着在 export、装饰器或变量声明的外层包装上。
    let mut anchor = node;
    while let Some(parent) = anchor.parent() {
        if !matches!(
            parent.kind(),
            "export_statement"
                | "decorated_definition"
                | "lexical_declaration"
                | "variable_declaration"
                | "template_declaration"
        ) {
            break;
        }
        anchor = parent;
    }
    let mut comments = Vec::new();
    let mut previous = anchor.prev_named_sibling();
    let mut start_row = anchor.start_position().row;
    while let Some(sibling) = previous {
        if sibling.kind() == "attribute_item" {
            start_row = sibling.start_position().row;
            previous = sibling.prev_named_sibling();
            continue;
        }
        if !sibling.kind().contains("comment") || sibling.end_position().row + 1 < start_row {
            break;
        }
        let text = sibling.utf8_text(source).unwrap_or_default();
        let line_start = source[..sibling.start_byte()]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |offset| offset + 1);
        if !source[line_start..sibling.start_byte()]
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            break;
        }
        // 文件级说明不归属下一个定义。
        if text.trim_start().starts_with("//!") || text.trim_start().starts_with("/*!") {
            break;
        }
        comments.push(text.to_string());
        start_row = sibling.start_position().row;
        previous = sibling.prev_named_sibling();
    }
    comments.reverse();
    // Python 的首条字符串语句才是文档字符串，普通字符串和函数体不参与检索。
    if node.kind() == "function_definition" || node.kind() == "class_definition" {
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            if let Some(first) = body
                .named_children(&mut cursor)
                .find(|child| !child.kind().contains("comment"))
            {
                if first.kind() == "expression_statement" {
                    if let Some(string) = first
                        .named_child(0)
                        .filter(|child| child.kind() == "string")
                    {
                        comments.push(string.utf8_text(source).unwrap_or_default().to_string());
                    }
                }
            }
        }
    }
    let docstring = comments
        .join("\n")
        .lines()
        .map(|line| {
            line.trim()
                .trim_matches(|c| matches!(c, '/' | '*' | '#' | '\'' | '"'))
                .trim()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (
        signature.chars().take(2000).collect(),
        docstring.chars().take(4000).collect(),
    )
}
