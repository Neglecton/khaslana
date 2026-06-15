// AI review 结果类型：正文 + 可选思考链。
//
// review 面板渲染时把 content 作为主内容展示；若模型返回了思考链
// （reasoning_content 字段或 `<think>` 标签），折叠展示在正文上方/下方。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReviewResult {
    /// 评审正文（已剥离思考链）。
    pub content: String,
    /// 可选思考链；普通模型为 None，reasoning 模型可能返回。
    pub reasoning: Option<String>,
}
