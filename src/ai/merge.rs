// AI 冲突合并建议的纯函数层：diff3 文本分段、多段对话滑动窗口、
// 响应清洗与冲突标记检测。
//
// 所有函数都是纯函数，不依赖网络/Git/GPUI，方便单元测试。

use super::prompt::ChatMessage;
use crate::types::{GitError, Result};

/// 整文件单请求模式的 diff3 文本字符上限。按主流 ~200K token 上下文窗口、
/// 代码约 3-4 字符/token 保守估算：60K 字符约 15-20K 输入 token，为输出
/// 与 system prompt 留足余量，几乎所有在线模型都能一次处理。
pub const MERGE_WHOLE_FILE_LIMIT: usize = 60_000;

/// 分段模式每段字符上限：24K 字符约 6-8K token，单段请求与响应在
/// 小上下文模型上也可用。
pub const MERGE_SEGMENT_LIMIT: usize = 24_000;

/// 分段模式携带对话历史的滑动窗口预算（字符）：从新到旧保留已完成段的
/// (请求, 响应) 回合，超出预算丢弃最旧回合；system 与当前段始终保留。
/// 150K 字符约 40-50K token，在 200K 窗口内为当前段和输出留出空间。
pub const MERGE_CONTEXT_BUDGET_CHARS: usize = 150_000;

/// 单个冲突块允许的最大字符数：超出说明块本身就超过可处理规模，
/// 明确报错而非静默截断（不做块内再切分，避免切开一个冲突的两侧改动）。
pub const MERGE_SINGLE_BLOCK_LIMIT: usize = MERGE_WHOLE_FILE_LIMIT;

/// diff3 文本的一个分段。
/// `has_conflicts == false` 的段是纯上下文（无冲突标记），不送 LLM，
/// 原样透传参与拼接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeSegment {
    pub text: String,
    pub has_conflicts: bool,
}

/// 把 diff3 文本按段上限切分为若干段。
///
/// - 以「单行上下文 / 完整冲突块」为原子单元贪心装段，**绝不切开
///   冲突块**（块从 `<<<<<<<` 行到配对的 `>>>>>>>` 行整体归属一段）；
/// - 所有段按顺序逐字节拼接恒等于原文；
/// - 单个冲突块超过 `segment_limit` 时作为独立超限段放行（仍完整），
///   超过 `max_unit_chars` 时报错；
/// - 纯上下文段（长文件中两块之间的大段未改动文本）标记
///   `has_conflicts = false`，调用方可直接透传省一次请求。
pub fn split_diff3_text(
    text: &str,
    segment_limit: usize,
    max_unit_chars: usize,
) -> Result<Vec<MergeSegment>> {
    // 先切成原子单元：上下文行（含换行）或完整冲突块。
    let mut units: Vec<MergeSegment> = Vec::new();
    let mut in_block = false;
    let mut block = String::new();
    for line in split_lines_preserve_endings(text) {
        let line_start = trim_line_ending(line);
        if !in_block && line_start.starts_with("<<<<<<<") {
            in_block = true;
            block.clear();
        }
        if in_block {
            block.push_str(line);
            if line_start.starts_with(">>>>>>>") {
                units.push(MergeSegment {
                    text: std::mem::take(&mut block),
                    has_conflicts: true,
                });
                in_block = false;
            }
            continue;
        }
        units.push(MergeSegment {
            text: line.to_string(),
            has_conflicts: false,
        });
    }
    // 输入保证标记配对（来自 merge_file_from_index）；兜底防御。
    if in_block {
        return Err(GitError::Message(
            "冲突文本标记不配对，无法分段生成合并建议".into(),
        ));
    }

    // 贪心装段。
    let mut segments: Vec<MergeSegment> = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    let mut current_has_conflicts = false;
    for unit in units {
        let unit_chars = unit.text.chars().count();
        if unit_chars > max_unit_chars {
            return Err(GitError::Message(format!(
                "单个冲突块超过 {max_unit_chars} 字符，暂不支持 AI 合并建议，请手动解决或使用外部合并工具"
            )));
        }
        // 当前段装不下下一个单元：先冲刷；单元本身超上限（只可能是
        // 冲突块）时也作为独立段放行。
        if !current.is_empty()
            && (current_chars + unit_chars > segment_limit || unit_chars > segment_limit)
        {
            segments.push(MergeSegment {
                text: std::mem::take(&mut current),
                has_conflicts: current_has_conflicts,
            });
            current_chars = 0;
            current_has_conflicts = false;
        }
        if unit_chars > segment_limit {
            segments.push(unit);
            continue;
        }
        current.push_str(&unit.text);
        current_chars += unit_chars;
        current_has_conflicts |= unit.has_conflicts;
    }
    if !current.is_empty() {
        segments.push(MergeSegment {
            text: current,
            has_conflicts: current_has_conflicts,
        });
    }
    Ok(segments)
}

/// 构造分段模式第 k 段的请求消息：system + 滑动窗口内的历史回合 + 当前段。
///
/// 历史按（请求, 响应）回合从新到旧累计字符数，连同 system 与当前段
/// 超出 `budget_chars` 时丢弃最旧回合（「文件尽量放同一个对话里」与
/// 「总上下文受限」的折中：近期段提供命名/风格连续性，窗口保证不超限）。
pub fn build_segment_messages(
    system: ChatMessage,
    history: &[(ChatMessage, ChatMessage)],
    current: ChatMessage,
    budget_chars: usize,
) -> Vec<ChatMessage> {
    let message_chars = |message: &ChatMessage| message.content.chars().count();
    let mut total = message_chars(&system) + message_chars(&current);
    let mut keep_from = history.len();
    for (index, (request, response)) in history.iter().enumerate().rev() {
        let turn_chars = message_chars(request) + message_chars(response);
        if total + turn_chars > budget_chars {
            break;
        }
        total += turn_chars;
        keep_from = index;
    }
    let mut messages = vec![system];
    for (request, response) in &history[keep_from..] {
        messages.push(request.clone());
        messages.push(response.clone());
    }
    messages.push(current);
    messages
}

/// 剥离 LLM 常见的代码块围栏：trim 后首行是围栏（可带语言标注，如
/// ```diff3）且末行是纯围栏时，去掉这两个围栏行。
///
/// 仅防御性清洗：markdown 等内容本身以围栏行开头结尾时会误剥，但
/// prompt 已明确要求不围栏，误剥概率远低于模型违规围栏的概率。
/// 围栏后紧贴的空行去除；内容尾部的换行保留（分段拼接依赖行尾换行）。
pub fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some((first_line, rest)) = trimmed.split_once('\n') else {
        return text.to_string();
    };
    let is_fence_open = first_line.starts_with("```");
    let rest_trimmed = rest.trim_end();
    if !is_fence_open || !rest_trimmed.ends_with("```") || rest_trimmed == "```" {
        return text.to_string();
    }
    let body = &rest_trimmed[..rest_trimmed.len() - 3];
    body.trim_start_matches('\n').to_string()
}

/// 响应中是否残留 diff3 冲突标记（行首 `<<<<<<<` / `>>>>>>>` / `|||||||`）。
/// `=======` 不单独判定：正文合法内容行可能恰好是七个等号
/// （RST/Markdown 分隔线），会误伤；前三种标记在正常代码中极罕见。
pub fn response_contains_conflict_markers(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>") || line.starts_with("|||||||")
    })
}

/// 按行切分并保留行尾换行（拼接恒等的前提）。
fn split_lines_preserve_endings(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let split = rest
            .find('\n')
            .map(|position| position + 1)
            .unwrap_or(rest.len());
        let (line, remainder) = rest.split_at(split);
        rest = remainder;
        Some(line)
    })
}

/// 去掉行尾 `\n` / `\r\n`，得到标记行比较用的纯文本。
fn trim_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

#[cfg(test)]
#[path = "../tests/ai/merge.rs"]
mod tests;
