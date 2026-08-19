// AI review 结果类型：正文 + 可选思考链 + agent 执行轨迹。
//
// review 面板渲染时把 steps（思维链/工具调用）按时间线折叠展示在正文
// 上方（Codex/ZCode 式 harness 观感），content 作为主内容（Markdown）展示。

use serde::{Deserialize, Serialize};

/// agent 评审过程的一个步骤：某轮思维链、assistant 中间说明或一次工具调用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiReviewStep {
    /// 某一轮的思考链（reasoning 模型的推理文本）。
    Reasoning { text: String },
    /// 中间轮 assistant 的非思考链正文（如「我先看看这个文件的实现」）。
    /// 与思维链不同，UI 上**不折叠**、整段直出——它是模型明确说出来的话，
    /// 不该像思考链那样被下一个工具消息覆盖掉。
    Message { text: String },
    /// 一次工具调用：摘要一行（工具名 + 关键参数），详情展开查看。
    ToolCall {
        name: String,
        /// 一行摘要，如 `read_lines src/lib.rs:100-180`。
        args_summary: String,
        /// 执行结果摘录（已按工具守卫截断）或错误信息。
        result_excerpt: String,
        error: bool,
    },
}

/// Reasoning 步骤摘要里保留的思维链文本长度上限。
const REASONING_SUMMARY_CHARS: usize = 40;

impl AiReviewStep {
    /// 时间线上的一行摘要。
    ///
    /// Reasoning 步骤取思维链首行非空文本截断（`思考：xxx`）——「思考中…」
    /// 只属于 UI 流式期间的瞬时状态（live 区），轮次落定后该行显示实际
    /// 内容摘要；空文本兜底「思考」。纯函数，便于单测。
    pub fn summary(&self) -> String {
        match self {
            Self::Reasoning { text } => {
                let first_line = text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("");
                if first_line.is_empty() {
                    "思考".to_string()
                } else {
                    let snippet: String =
                        first_line.chars().take(REASONING_SUMMARY_CHARS).collect();
                    let ellipsis = if first_line.chars().count() > REASONING_SUMMARY_CHARS {
                        "…"
                    } else {
                        ""
                    };
                    format!("思考：{snippet}{ellipsis}")
                }
            }
            Self::ToolCall { args_summary, .. } => args_summary.clone(),
            // Message 步骤 UI 上不折叠（整段直出），summary 仅供极端场合
            // 的单行降级，取首行截断。
            Self::Message { text } => {
                let first_line = text.lines().next().unwrap_or("").trim();
                if first_line.is_empty() {
                    "回复".to_string()
                } else {
                    let snippet: String =
                        first_line.chars().take(REASONING_SUMMARY_CHARS).collect();
                    let ellipsis = if first_line.chars().count() > REASONING_SUMMARY_CHARS {
                        "…"
                    } else {
                        ""
                    };
                    format!("回复：{snippet}{ellipsis}")
                }
            }
        }
    }

    /// 展开查看的详情文本。
    pub fn detail(&self) -> &str {
        match self {
            Self::Reasoning { text } => text,
            Self::Message { text } => text,
            Self::ToolCall { result_excerpt, .. } => result_excerpt,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::ToolCall { error: true, .. })
    }

    /// UI 上是否按「折叠行 + 点击展开」渲染：思维链与工具调用折叠，
    /// Message 整段直出不折叠。
    pub fn is_collapsible(&self) -> bool {
        !matches!(self, Self::Message { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReviewResult {
    /// 评审正文（已剥离思考链，Markdown）。
    pub content: String,
    /// 可选思考链（末轮）；普通模型为 None，reasoning 模型可能返回。
    pub reasoning: Option<String>,
    /// agent 执行轨迹（按时间顺序：各轮思维链与工具调用）。
    pub steps: Vec<AiReviewStep>,
}

#[cfg(test)]
#[path = "../tests/ai/review.rs"]
mod tests;
