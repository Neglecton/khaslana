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
