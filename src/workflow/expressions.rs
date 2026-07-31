use unicode_segmentation::UnicodeSegmentation;

use crate::{GitError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WorkflowExpressionValue {
    String(String),
    Array(Vec<String>),
}

impl WorkflowExpressionValue {
    pub(super) fn into_string(self, expression: &str) -> Result<String> {
        match self {
            Self::String(value) => Ok(value),
            Self::Array(_) => Err(GitError::Message(format!(
                "工作流表达式最终结果是数组，请使用 first、last、nth 或 join 转为字符串：{expression}"
            ))),
        }
    }
}

struct MethodCall {
    name: String,
    args: Vec<String>,
}

pub(super) fn evaluate_workflow_expression(
    expression: &str,
    mut resolve_primary: impl FnMut(&str) -> Result<WorkflowExpressionValue>,
) -> Result<WorkflowExpressionValue> {
    let stages = split_unquoted(expression, '|')?;
    let Some((primary, methods)) = stages.split_first() else {
        return Err(GitError::Message("工作流表达式不能为空".into()));
    };
    let primary = primary.trim();
    if primary.is_empty() {
        return Err(GitError::Message("工作流表达式不能为空".into()));
    }

    let mut value = resolve_primary(primary)?;
    for method in methods {
        value = apply_method(value, parse_method_call(method)?)?;
    }
    Ok(value)
}

fn parse_method_call(segment: &str) -> Result<MethodCall> {
    let parts = split_unquoted(segment, ':')?;
    let Some((name, args)) = parts.split_first() else {
        return Err(GitError::Message("工作流内置方法名不能为空".into()));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(GitError::Message("工作流内置方法名不能为空".into()));
    }
    Ok(MethodCall {
        name: name.to_string(),
        args: args
            .iter()
            .map(|arg| parse_method_arg(arg))
            .collect::<Result<Vec<_>>>()?,
    })
}

fn parse_method_arg(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    let Some(first) = trimmed.chars().next() else {
        return Ok(String::new());
    };
    if first != '"' && first != '\'' {
        return Ok(trimmed.to_string());
    }
    if !trimmed.ends_with(first) || trimmed.len() < 2 {
        return Err(GitError::Message(format!(
            "工作流方法参数引号未闭合：{trimmed}"
        )));
    }
    let inner = &trimmed[first.len_utf8()..trimmed.len() - first.len_utf8()];
    unescape_quoted_arg(inner)
}

fn unescape_quoted_arg(raw: &str) -> Result<String> {
    let mut output = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(GitError::Message("工作流方法参数转义符缺少目标字符".into()));
        };
        match escaped {
            'n' => output.push('\n'),
            't' => output.push('\t'),
            'r' => output.push('\r'),
            '\\' => output.push('\\'),
            '"' => output.push('"'),
            '\'' => output.push('\''),
            other => output.push(other),
        }
    }
    Ok(output)
}

fn split_unquoted(input: &str, delimiter: char) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if quote.is_some() && ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }

        if let Some(open_quote) = quote {
            current.push(ch);
            if ch == open_quote {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }

        if ch == delimiter {
            parts.push(current);
            current = String::new();
            continue;
        }

        current.push(ch);
    }

    if quote.is_some() {
        return Err(GitError::Message(format!(
            "工作流表达式引号未闭合：{input}"
        )));
    }

    parts.push(current);
    Ok(parts)
}

fn apply_method(
    value: WorkflowExpressionValue,
    method: MethodCall,
) -> Result<WorkflowExpressionValue> {
    match value {
        WorkflowExpressionValue::String(value) => apply_string_method(value, method),
        WorkflowExpressionValue::Array(value) => apply_array_method(value, method),
    }
}

fn apply_string_method(value: String, method: MethodCall) -> Result<WorkflowExpressionValue> {
    match method.name.as_str() {
        "trim" => {
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::String(value.trim().to_string()))
        }
        "lower" => {
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::String(value.to_lowercase()))
        }
        "upper" => {
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::String(value.to_uppercase()))
        }
        "replace" => {
            expect_arg_count(&method, 2)?;
            let from = &method.args[0];
            if from.is_empty() {
                return Err(GitError::Message(
                    "工作流方法 replace 的查找内容不能为空".into(),
                ));
            }
            Ok(WorkflowExpressionValue::String(
                value.replace(from, &method.args[1]),
            ))
        }
        "truncate" => {
            expect_arg_count(&method, 1)?;
            Ok(WorkflowExpressionValue::String(take_graphemes(
                &value,
                parse_usize_arg(&method, 0)?,
            )))
        }
        "suffix" => {
            expect_arg_count(&method, 1)?;
            Ok(WorkflowExpressionValue::String(take_suffix_graphemes(
                &value,
                parse_usize_arg(&method, 0)?,
            )))
        }
        "default" => {
            expect_arg_count(&method, 1)?;
            if value.trim().is_empty() {
                Ok(WorkflowExpressionValue::String(method.args[0].clone()))
            } else {
                Ok(WorkflowExpressionValue::String(value))
            }
        }
        "slug" => {
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::String(slugify(&value)))
        }
        "alt" => {
            // 把逗号分隔列表转成正则交替分组，便于把"前缀枚举输入"嵌进 pattern。
            // 例："dev,uat,release" | alt → "(dev|uat|release)"；每项 trim，忽略空项。
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::String(regex_alternation(&value)))
        }
        "split" => {
            expect_arg_count(&method, 1)?;
            let delimiter = &method.args[0];
            if delimiter.is_empty() {
                return Err(GitError::Message(
                    "工作流方法 split 的分隔符不能为空".into(),
                ));
            }
            Ok(WorkflowExpressionValue::Array(
                value.split(delimiter).map(str::to_string).collect(),
            ))
        }
        _ => Err(unknown_or_type_error(&method.name, "字符串")),
    }
}

fn apply_array_method(value: Vec<String>, method: MethodCall) -> Result<WorkflowExpressionValue> {
    match method.name.as_str() {
        "compact" => {
            expect_arg_count(&method, 0)?;
            Ok(WorkflowExpressionValue::Array(
                value
                    .into_iter()
                    .filter(|item| !item.trim().is_empty())
                    .collect(),
            ))
        }
        "first" => {
            expect_arg_range(&method, 0, 1)?;
            array_item_or_default(value.first(), method.args.first(), "first")
        }
        "last" => {
            expect_arg_range(&method, 0, 1)?;
            array_item_or_default(value.last(), method.args.first(), "last")
        }
        "nth" => {
            expect_arg_range(&method, 1, 2)?;
            let index = parse_usize_arg(&method, 0)?;
            array_item_or_default(value.get(index), method.args.get(1), "nth")
        }
        "join" => {
            expect_arg_count(&method, 1)?;
            Ok(WorkflowExpressionValue::String(value.join(&method.args[0])))
        }
        _ => Err(unknown_or_type_error(&method.name, "数组")),
    }
}

fn array_item_or_default(
    item: Option<&String>,
    default: Option<&String>,
    method_name: &str,
) -> Result<WorkflowExpressionValue> {
    item.cloned()
        .or_else(|| default.cloned())
        .map(WorkflowExpressionValue::String)
        .ok_or_else(|| {
            GitError::Message(format!(
                "工作流方法 {method_name} 无法从空数组取得元素，请提供默认值"
            ))
        })
}

fn expect_arg_count(method: &MethodCall, expected: usize) -> Result<()> {
    if method.args.len() == expected {
        return Ok(());
    }
    Err(GitError::Message(format!(
        "工作流方法 {} 需要 {} 个参数，实际收到 {} 个",
        method.name,
        expected,
        method.args.len()
    )))
}

fn expect_arg_range(method: &MethodCall, min: usize, max: usize) -> Result<()> {
    if (min..=max).contains(&method.args.len()) {
        return Ok(());
    }
    Err(GitError::Message(format!(
        "工作流方法 {} 需要 {} 到 {} 个参数，实际收到 {} 个",
        method.name,
        min,
        max,
        method.args.len()
    )))
}

fn parse_usize_arg(method: &MethodCall, index: usize) -> Result<usize> {
    let value = &method.args[index];
    value.parse::<usize>().map_err(|_| {
        GitError::Message(format!(
            "工作流方法 {} 的第 {} 个参数必须是非负整数：{}",
            method.name,
            index + 1,
            value
        ))
    })
}

fn unknown_or_type_error(method_name: &str, value_type: &str) -> GitError {
    GitError::Message(format!(
        "工作流内置方法 {method_name} 不存在，或不能用于{value_type}值"
    ))
}

fn take_graphemes(value: &str, count: usize) -> String {
    UnicodeSegmentation::graphemes(value, true)
        .take(count)
        .collect()
}

fn take_suffix_graphemes(value: &str, count: usize) -> String {
    let graphemes = UnicodeSegmentation::graphemes(value, true).collect::<Vec<_>>();
    let start = graphemes.len().saturating_sub(count);
    graphemes[start..].concat()
}

fn slugify(value: &str) -> String {
    let mut output = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !output.is_empty() && !last_was_separator {
            output.push('-');
            last_was_separator = true;
        }
    }
    if last_was_separator {
        output.pop();
    }
    output
}

/// 把逗号分隔的列表转换成正则交替分组字符串。
///
/// 例如 `"dev, uat , release,"` 会变成 `"(dev|uat|release)"`：
/// 每项会去除首尾空白，空项被忽略；若结果为空则返回空字符串（不包裹括号）。
/// 用于把"前缀枚举输入"嵌入 `filterBranches` 的 `pattern`。
fn regex_alternation(value: &str) -> String {
    let parts: Vec<&str> = value
        .split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!("({})", parts.join("|"))
}

#[cfg(test)]
#[path = "../tests/workflow/expressions.rs"]
mod tests;
