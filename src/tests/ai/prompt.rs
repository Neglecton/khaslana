use super::*;

#[test]
fn commit_message_prompts_contains_diff_and_system_rules() {
    let (system, user) = commit_message_prompts("diff --git a/x b/x", None);
    assert_eq!(system.role, ChatRole::System);
    assert!(system.content.contains("提交信息"));
    assert_eq!(user.role, ChatRole::User);
    assert!(user.content.contains("diff --git a/x b/x"));
}

#[test]
fn commit_message_prompts_includes_hint_when_provided() {
    let (_system, user) = commit_message_prompts("diff", Some("只看接口"));
    assert!(user.content.contains("补充要求"));
    assert!(user.content.contains("只看接口"));
}

#[test]
fn commit_message_prompts_omits_hint_section_when_empty() {
    let (_system, user) = commit_message_prompts("diff", Some("   "));
    assert!(!user.content.contains("补充要求"));
}

#[test]
fn commit_message_prompts_truncates_long_diff() {
    let long = "a".repeat(COMMIT_DIFF_TEXT_LIMIT * 2);
    let (_system, user) = commit_message_prompts(&long, None);
    assert!(user.content.contains("已截断"));
}

#[test]
fn truncate_text_keeps_short_text_intact() {
    assert_eq!(truncate_text("hello", 10), "hello");
}

#[test]
fn truncate_text_appends_notice_when_too_long() {
    let result = truncate_text("abcdefgh", 3);
    assert!(result.starts_with("abc"));
    assert!(result.contains("已截断"));
}

#[test]
fn conflict_merge_prompts_whole_file_mode() {
    let (system, user) = conflict_merge_prompts("src/lib.rs", "<<<<<<< OURS\nx\n", None);
    assert_eq!(system.role, ChatRole::System);
    // 系统提示说明标记格式与输出约束（不得残留冲突标记、非冲突行逐字保留）。
    assert!(system.content.contains("diff3"));
    assert!(system.content.contains("冲突标记"));
    assert!(system.content.contains("逐字保留"));
    assert_eq!(user.role, ChatRole::User);
    assert!(user.content.contains("src/lib.rs"));
    assert!(user.content.contains("完整内容"));
    // 整文件模式不出现分段说明。
    assert!(!user.content.contains("第 "));
    assert!(user.content.contains("<<<<<<< OURS\nx"));
}

#[test]
fn conflict_merge_prompts_segment_mode_marks_position() {
    let (_system, user) = conflict_merge_prompts("a.txt", "block body", Some((2, 5)));
    assert!(user.content.contains("第 2/5 段"));
    assert!(user.content.contains("只输出这一段"));
    assert!(user.content.contains("block body"));
}

#[test]
fn workflow_template_prompts_embeds_doc_and_request() {
    let (system, user) = workflow_template_prompts("基于 master 创建 release 分支并推送", None);
    assert_eq!(system.role, ChatRole::System);
    // 文档关键章节嵌入（AI 参考工作流使用文档）
    assert!(system.content.contains("支持的步骤"));
    assert!(system.content.contains("filterBranches"));
    assert!(system.content.contains("version 字段恒为 1"));
    assert_eq!(user.role, ChatRole::User);
    // 新建模式：不含现有模板标记，含需求
    assert!(!user.content.contains("【现有模板】"));
    assert!(user.content.contains("基于 master 创建 release 分支并推送"));
}

#[test]
fn workflow_template_prompts_edit_mode_includes_current_definition() {
    let current = r#"{ version: 1, name: "旧模板", steps: [{ op: "ensureClean" }] }"#;
    let (_system, user) = workflow_template_prompts("加一个推送步骤", Some(current));
    assert!(user.content.contains("【现有模板】"));
    assert!(user.content.contains("旧模板"));
    assert!(user.content.contains("加一个推送步骤"));
}

#[test]
fn workflow_template_prompts_trims_and_rejects_empty_request() {
    // 空白需求 trim 后仍生成 prompt（守卫在 UI 层做），但内容不含空白原文
    let (_system, user) = workflow_template_prompts("   ", None);
    assert!(user.content.contains("【功能需求】"));
}
