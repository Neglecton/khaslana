use super::*;

fn eval(expression: &str) -> Result<WorkflowExpressionValue> {
    evaluate_workflow_expression(expression, |primary| {
        Ok(match primary {
            "branch" => "feature/demo".to_string(),
            "delimited" => "a|b:c".to_string(),
            "emoji" => "你好吗🙂x".to_string(),
            "spaced" => " Hello/World ".to_string(),
            "empty" => " ".to_string(),
            other => other.to_string(),
        })
    })
}

fn eval_string(expression: &str) -> Result<String> {
    eval(expression)?.into_string(expression)
}

#[test]
fn string_methods_transform_values() {
    assert_eq!(eval_string("spaced | trim | lower").unwrap(), "hello/world");
    assert_eq!(
        eval_string("feature/demo_branch | replace:\"_\":\"-\" | upper").unwrap(),
        "FEATURE/DEMO-BRANCH"
    );
    assert_eq!(eval_string("abcdef | truncate:3").unwrap(), "abc");
    assert_eq!(eval_string("abcdef | suffix:2").unwrap(), "ef");
    assert_eq!(eval_string("empty | default:\"master\"").unwrap(), "master");
    assert_eq!(
        eval_string("Feature/ABC 123 | slug").unwrap(),
        "feature-abc-123"
    );
}

#[test]
fn grapheme_methods_do_not_split_emoji_or_cjk() {
    assert_eq!(eval_string("emoji | truncate:4").unwrap(), "你好吗🙂");
    assert_eq!(eval_string("emoji | suffix:2").unwrap(), "🙂x");
}

#[test]
fn array_methods_consume_split_values() {
    assert_eq!(eval_string("branch | split:\"/\" | last").unwrap(), "demo");
    assert_eq!(
        eval_string("a,,b | split:\",\" | compact | join:\"/\"").unwrap(),
        "a/b"
    );
    assert_eq!(eval_string("a/b/c | split:\"/\" | nth:1").unwrap(), "b");
    assert_eq!(
        eval_string("empty | split:\",\" | compact | first:\"fallback\"").unwrap(),
        "fallback"
    );
}

#[test]
fn final_array_is_rejected() {
    let err = eval_string("a/b | split:\"/\"").unwrap_err();
    assert!(err.to_string().contains("最终结果是数组"));
}

#[test]
fn invalid_method_arguments_return_chinese_errors() {
    assert!(
        eval_string("abc | truncate:x")
            .unwrap_err()
            .to_string()
            .contains("非负整数")
    );
    assert!(
        eval_string("abc | split:\"\"")
            .unwrap_err()
            .to_string()
            .contains("分隔符不能为空")
    );
    assert!(
        eval_string("abc | missing")
            .unwrap_err()
            .to_string()
            .contains("内置方法")
    );
}

#[test]
fn parser_allows_quoted_delimiters_inside_method_args() {
    assert_eq!(
        eval_string("delimited | replace:\"|\":\":\"").unwrap(),
        "a:b:c"
    );
}
