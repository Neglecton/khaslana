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
fn review_prompts_contains_file_and_branch() {
    let (system, user) = review_prompts("src/main.rs", "some diff", "feature/x");
    assert_eq!(system.role, ChatRole::System);
    assert!(system.content.contains("代码评审"));
    assert_eq!(user.role, ChatRole::User);
    assert!(user.content.contains("src/main.rs"));
    assert!(user.content.contains("feature/x"));
    assert!(user.content.contains("some diff"));
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
