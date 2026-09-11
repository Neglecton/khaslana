// `code_understanding::analysis` 的纯函数测试：正文解析与 schema/提示同源。
//
// 需要真实来源会话的校验测试在 `code_understanding_agent.rs`（那里有索引
// 样例可以发放真实 source_id）。

use super::*;

fn minimal_json(summary: &str) -> String {
    serde_json::json!({
        "summary": summary,
        "findings": [{
            "text": "登录入口是 LoginController",
            "state": "observed",
            "source_ids": ["sr1:abc"]
        }],
        "completion": "complete"
    })
    .to_string()
}

#[test]
fn parse_accepts_plain_json() {
    let raw = minimal_json("登录逻辑…");
    let result = parse_analysis_result(&raw).unwrap();
    assert_eq!(result.summary, "登录逻辑…");
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].state, AnalysisEvidenceState::Observed);
    assert_eq!(result.completion, AnalysisCompletion::Complete);
}

#[test]
fn parse_strips_code_fence() {
    let raw = format!("```json\n{}\n```", minimal_json("围栏"));
    let result = parse_analysis_result(&raw).unwrap();
    assert_eq!(result.summary, "围栏");
}

#[test]
fn parse_tolerates_surrounding_prose() {
    let raw = format!(
        "好的，我完成了分析。\n{}\n以上是我的结论。",
        minimal_json("夹带说明")
    );
    let result = parse_analysis_result(&raw).unwrap();
    assert_eq!(result.summary, "夹带说明");
}

#[test]
fn parse_escapes_raw_control_characters_inside_strings() {
    let raw = minimal_json("第一行\n第二行\t缩进")
        .replace("\\n", "\n")
        .replace("\\t", "\t");
    let result = parse_analysis_result(&raw).unwrap();
    assert_eq!(result.summary, "第一行\n第二行\t缩进");
}

#[test]
fn parse_reports_the_remaining_error_after_control_character_cleanup() {
    let error = parse_analysis_result("{\"summary\":\"第一行\n第二行\"}").unwrap_err();
    assert!(error.message.contains("missing field"), "{}", error.message);
    assert!(!error.message.contains("control character"), "{}", error.message);
}

#[test]
fn parse_rejects_empty_and_truncated_json() {
    assert!(parse_analysis_result("   ").is_err());
    // 半截 JSON 不能被当成成功结果。
    let truncated = r#"{"summary":"半句", "findings":[{"text":"x""#;
    let error = parse_analysis_result(truncated).unwrap_err();
    assert_eq!(error.code, ErrorCode::AnswerInvalid);
    assert!(error.message.contains("合法 JSON"), "{}", error.message);
}

#[test]
fn parse_rejects_json_without_required_fields() {
    // 缺 findings/steps 且缺 completion：结构反序列化直接失败。
    let raw = r#"{"summary":"只有摘要"}"#;
    let error = parse_analysis_result(raw).unwrap_err();
    assert_eq!(error.code, ErrorCode::AnswerInvalid);
}

#[test]
fn schema_and_protocol_are_serializable_and_consistent() {
    let schema = analysis_json_schema();
    assert_eq!(schema["version"], ANALYSIS_PROTOCOL_VERSION);
    assert!(schema["data_accesses"][0]["category"].is_string());
    let protocol = analysis_output_protocol();
    // 约束文案与常量同源，避免提示词预算与校验脱钩。
    assert!(protocol.contains(&ANALYSIS_MAX_STEPS.to_string()));
    assert!(protocol.contains(&ANALYSIS_MAX_LINKS.to_string()));
    assert!(protocol.contains("source_ids"));
    let prompt = analysis_system_prompt();
    assert!(prompt.contains("读写哪些表"));
    assert!(prompt.contains("未找到"));
    assert!(prompt.contains(&ANALYSIS_MAX_STEPS.to_string()));
}

#[test]
fn dto_roundtrips_through_serde() {
    let raw = serde_json::json!({
        "version": 1,
        "summary": "s",
        "coverage_note": "c",
        "findings": [{"text":"f","state":"inferred","condition":"密码错误","source_ids":["sr1:1"]}],
        "callers": [{"name":"LoginController#login","relative_path":"a.java","detail":"入口","kind":"direct","source_ids":["sr1:1"]}],
        "callees": [{"name":"UserRepository#find","kind":"candidate","source_ids":["sr1:1"]}],
        "data_accesses": [{
            "object":"app_user","category":"db_object","operations":["read","update"],
            "conditions":["按用户名读取"],"access_method":"UserRepository","state":"observed",
            "source_ids":["sr1:1"]
        }],
        "steps": [{"id":"s1","title":"校验账号","source_ids":["sr1:1"]}],
        "links": [],
        "unknowns": ["动态表名未确认"],
        "completion": "partial"
    })
    .to_string();
    let parsed = parse_analysis_result(&raw).unwrap();
    let back = serde_json::to_string(&parsed).unwrap();
    let reparsed = parse_analysis_result(&back).unwrap();
    assert_eq!(parsed, reparsed);
    assert_eq!(parsed.data_accesses[0].operations.len(), 2);
    assert_eq!(
        parsed.data_accesses[0].category,
        DataObjectCategory::DbObject
    );
}
