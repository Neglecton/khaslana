// AI prompt 构造纯函数：commit message 生成、冲突合并。
//
// 所有函数都是纯函数，不依赖网络/Git/GPUI，方便单元测试。

/// commit message 生成时用户 prompt 中允许的 diff 文本最大字符数，
/// 避免超出常见模型上下文窗口。
pub(crate) const COMMIT_DIFF_TEXT_LIMIT: usize = 6000;

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

/// 构造 AI 冲突合并建议的 system + user prompt。
///
/// - `path`：冲突文件路径（仓库相对路径）。
/// - `diff3_text`：带 `<<<<<<< OURS / ||||||| BASE / ======= / >>>>>>> THEIRS`
///   标记的完整 diff3 文本（整文件模式）或其中一段（分段模式）。
/// - `segment`：分段模式下为 `Some((第几段, 总段数))`，要求只输出本段；
///   整文件模式传 `None`。
pub fn conflict_merge_prompts(
    path: &str,
    diff3_text: &str,
    segment: Option<(usize, usize)>,
) -> (ChatMessage, ChatMessage) {
    let system = ChatMessage {
        role: ChatRole::System,
        content: "你是一名资深的 Git 合并专家，负责解决 diff3 格式的合并冲突。\n\
                  输入文本中的冲突块格式为：\n\
                  <<<<<<< OURS（当前分支的改动）\n\
                  ||||||| BASE（共同祖先内容，供理解原始意图）\n\
                  =======\n\
                  >>>>>>> THEIRS（传入分支的改动）\n\
                  要求：\n\
                  1. 只输出解决冲突后的完整文本，不要任何解释、代码块围栏或多余前后缀。\n\
                  2. 每个冲突块综合 OURS 与 THEIRS 两侧的改动意图智能合并，两侧的语义都应保留；BASE 仅用于理解演化过程，不是必须保留的内容。\n\
                  3. 非冲突的上下文行必须逐字保留：不增删空行、不改格式、不“顺手优化”。\n\
                  4. 输出中不得残留任何冲突标记（<<<<<<<、|||||||、=======、>>>>>>>）。\n\
                  5. 无法确定取舍时，优先同时保留两侧内容（OURS 在前），并保证语法正确。"
            .to_string(),
    };

    let mut user = String::new();
    match segment {
        Some((index, total)) => user.push_str(&format!(
            "以下是文件 {path} 的第 {index}/{total} 段（文件过长已分段，各段首尾相接）。请只输出这一段解决冲突后的完整内容：\n\n"
        )),
        None => user.push_str(&format!(
            "以下是文件 {path} 的完整内容，请输出解决全部冲突后的完整文件：\n\n"
        )),
    }
    user.push_str("```diff3\n");
    user.push_str(diff3_text);
    user.push_str("\n```");

    let user = ChatMessage {
        role: ChatRole::User,
        content: user,
    };
    (system, user)
}

/// 工作流模板 AI 生成：system prompt 中嵌入的工作流格式参考文档
///（编译期嵌入，随 docs/workflows.md 更新自动同步）。
const WORKFLOW_DOC_TEXT: &str = include_str!("../../docs/workflows.md");

/// 用户需求描述的最大字符数（防超长输入撑爆上下文）。
pub(crate) const WORKFLOW_REQUEST_TEXT_LIMIT: usize = 4000;

/// 构造工作流模板生成/编辑的 system + user prompt。
///
/// - `request`：用户对模板功能的自然语言描述。
/// - `current_definition_json5`：编辑模式下当前模板的 JSON5 序列化文本；
///   新建模式传 `None`，AI 从零生成。
///
/// system 要求模型只输出一个完整的工作流 JSON5 对象；user 携带需求描述，
/// 编辑模式附当前模板内容让 AI 在其基础上修改。
pub fn workflow_template_prompts(
    request: &str,
    current_definition_json5: Option<&str>,
) -> (ChatMessage, ChatMessage) {
    let mut system = String::from(
        "你是 Khaslana Git 客户端的工作流模板生成助手。请根据用户的功能需求，生成一份可直接保存使用的 Khaslana 工作流模板（JSON5 格式）。

         下面是工作流的完整使用文档，请严格遵循其中的文件格式、步骤类型、变量语法和限制：

",
    );
    system.push_str(WORKFLOW_DOC_TEXT);
    system.push_str(
        "

输出要求（必须严格遵守）：
         1. 只输出一个完整的工作流 JSON5 对象，不要输出任何解释、前后缀文字或 markdown 代码块标记。
         2. version 字段恒为 1。
         3. steps 至少一个，op 只能使用文档「支持的步骤」章节列出的类型。
         4. 变量引用一律使用 ${...} 表达式语法；inputs 的键不得以 git. / run. / date: 开头。
         5. 为字段填写合理的中文 label 与说明，便于其他用户理解。",
    );

    let request = truncate_text(request.trim(), WORKFLOW_REQUEST_TEXT_LIMIT);
    let mut user = match current_definition_json5 {
        Some(current) => format!(
            "请修改以下现有工作流模板，使其满足新的功能需求。保持未涉及的部分不变，只改动需求要求的部分。

             【现有模板】
```json5
{current}
```

"
        ),
        None => String::from("请从零生成一个新的工作流模板。

"),
    };
    user.push_str(
        "【功能需求】
",
    );
    user.push_str(&request);

    (
        ChatMessage {
            role: ChatRole::System,
            content: system,
        },
        ChatMessage {
            role: ChatRole::User,
            content: user,
        },
    )
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
#[path = "../tests/ai/prompt.rs"]
mod tests;
