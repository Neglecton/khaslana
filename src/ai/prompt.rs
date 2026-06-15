// AI prompt 构造纯函数：commit message 生成、code review。
//
// 所有函数都是纯函数，不依赖网络/Git/GPUI，方便单元测试。

/// commit message 生成时用户 prompt 中允许的 diff 文本最大字符数，
/// 避免超出常见模型上下文窗口。
pub(crate) const COMMIT_DIFF_TEXT_LIMIT: usize = 6000;
/// code review 时单文件 diff 文本最大字符数。
pub(crate) const REVIEW_DIFF_TEXT_LIMIT: usize = 8000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// 构造 commit message 生成的 system + user prompt。
///
/// - `diff_text`：所有 staged 文件拼成的 unified-like diff 文本。
/// - `hint`：用户可选的额外提示（如“只关注接口改动”）。
pub fn commit_message_prompts(diff_text: &str, hint: Option<&str>) -> (ChatMessage, ChatMessage) {
    let system = ChatMessage {
        role: ChatRole::System,
        content: "你是一个资深的 Git 提交信息助手。请根据给定的暂存区差异（staged diff）生成简洁、清晰的提交信息。\n\
                  要求：\n\
                  1. 只输出提交信息正文，不要输出解释、代码块标记或多余空行。\n\
                  2. 第一行为简短摘要（不超过 50 字），中文；如有必要可补一空行后写正文要点。\n\
                  3. 可使用 Conventional Commits 风格（如 feat/fix/refactor），但不要强求。\n\
                  4. 关注“为什么改”而非“改了什么文件”。"
            .to_string(),
    };

    let mut user = String::new();
    user.push_str("以下是本次提交的暂存区差异，请生成提交信息：\n\n");
    user.push_str("```diff\n");
    user.push_str(&truncate_text(diff_text, COMMIT_DIFF_TEXT_LIMIT));
    user.push_str("\n```");
    if let Some(hint) = hint {
        let hint = hint.trim();
        if !hint.is_empty() {
            user.push_str("\n\n补充要求：");
            user.push_str(hint);
        }
    }

    let user = ChatMessage {
        role: ChatRole::User,
        content: user,
    };
    (system, user)
}

/// 构造 AI code review 的 system + user prompt。
///
/// - `file_path`：被 review 的文件路径（仓库相对路径）。
/// - `diff_text`：该文件的 diff 文本。
/// - `branch_name`：目标分支名（用于上下文）。
pub fn review_prompts(
    file_path: &str,
    diff_text: &str,
    branch_name: &str,
) -> (ChatMessage, ChatMessage) {
    let system = ChatMessage {
        role: ChatRole::System,
        content: "你是一名资深代码评审专家（code reviewer）。请对给定的代码差异进行评审。\n\
                  要求：\n\
                  1. 用中文输出，结构清晰：按“严重问题 / 建议 / 风险点 / 优点”分组，没有的组可省略。\n\
                  2. 只针对 diff 中的实际改动评论，不要凭空假设未展示的代码。\n\
                  3. 指出潜在 bug、安全问题、性能问题、可读性问题，并给出简短改进建议。\n\
                  4. 不要直接重写整段代码；如需示例，只给关键片段。\n\
                  5. 保持简洁，聚焦最重要的 3-5 个点。"
            .to_string(),
    };

    let mut user = String::new();
    user.push_str("请评审以下文件的差异：\n\n");
    user.push_str(&format!("文件：{file_path}\n"));
    user.push_str(&format!("目标分支：{branch_name}\n\n"));
    user.push_str("```diff\n");
    user.push_str(&truncate_text(diff_text, REVIEW_DIFF_TEXT_LIMIT));
    user.push_str("\n```");

    let user = ChatMessage {
        role: ChatRole::User,
        content: user,
    };
    (system, user)
}

/// 截断文本到指定字符数；超出时追加截断提示。
pub(crate) fn truncate_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let truncated: String = text.chars().take(limit).collect();
    format!("{truncated}\n\n…（差异过长，已截断）")
}

#[cfg(test)]
mod tests {
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
}
