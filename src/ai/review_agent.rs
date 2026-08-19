// Diff-first Agentic 代码评审：让大模型通过工具调用按需深入仓库代码。
//
// 初始输入只包含变更文件清单 + 预算内的 diff（省 token），模型在真正需要
// 时调用内置工具（读文件/读差异/看树/搜代码/查历史/看追溯）获取上下文，
// 多轮循环直到给出最终评审。全程非流式：每轮结束后通过 AgentEvent 把
// 思维链与工具轨迹回传 UI（Codex/ZCode 式时间线）。
//
// 守卫（成本控制）：轮次 / 工具总次数 / 单结果与累计结果字符数均有上限，
// 超限后请求省略 tools 字段强制模型收尾；不支持工具调用的端点不降级，
// 由 client 层按状态码直接报错引导更换供应商。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::GitService;
use crate::ai::client::{AGENT_STREAM_MAX_RETRIES, AgentChatMessage, ChatClient, ToolSchema};
use crate::ai::review::{AiReviewResult, AiReviewStep};
use crate::git::CodeSearchMatch;
use crate::types::{
    BlameHunkInfo, BrowseCompareFile, CommitInfo, DiffEncodingChoice, DiffLineKind, FileDiff,
    GitError, HistoryScope, Result,
};

/// 最多带工具的请求轮数。带 tool_calls 的轮必有 ≥1 次调用，轮数 ≤ 调用数 + 1，
/// 与 `MAX_TOOL_CALLS_TOTAL` 同值即永不先于调用线触顶（避免「每轮 1 次调用」
/// 的模型被轮次线提前截停）。
pub(crate) const MAX_TOOL_ROUNDS: usize = 120;
/// 工具调用总次数上限。
pub(crate) const MAX_TOOL_CALLS_TOTAL: usize = 120;
/// 单个工具结果的字符上限（截断带标注）。
pub(crate) const MAX_TOOL_RESULT_CHARS: usize = 8_000;
/// 全部工具结果的累计字符上限，超限强制收尾。按 ~200K token 上下文
///（3-4 字符/token）估算：30K 初始 + 400K 工具结果 + 历史与最终输出
/// ≈ 120-150K token，留足余量。
pub(crate) const MAX_TOTAL_TOOL_RESULT_CHARS: usize = 400_000;
/// 初始 diff 的总字符预算；全量放得下就全量给。
pub(crate) const INITIAL_DIFF_BUDGET_CHARS: usize = 30_000;
/// 预算超限时单个文件 diff 的字符上限。
pub(crate) const INITIAL_PER_FILE_CHARS: usize = 4_000;
/// 评审轮次的输出 token 上限（覆盖默认 800，评审正文较长）。reasoning
/// 模型的思考 token 在多数兼容端点与正文共用该预算，过小会让长思维链
/// 在中途触及上限（finish_reason=length、无正文无调用），故给到 16384。
pub(crate) const REVIEW_MAX_TOKENS: u32 = 16_384;
/// read_lines 单次最大行数窗口。
const READ_LINES_MAX_LINES: usize = 400;
/// read_lines 单文件字节上限。
const READ_LINES_MAX_BYTES: u64 = 1024 * 1024;
/// get_file_tree 条目数上限。
const FILE_TREE_MAX_ENTRIES: usize = 300;
/// get_file_history 条数上限（钳制模型传入的 limit）。
const FILE_HISTORY_MAX_LIMIT: usize = 20;
/// search_code 命中上限。
const SEARCH_MAX_HITS: usize = 50;

/// agent 评审的输入：目标引用 + 差异文件清单（由 UI 侧浏览状态提供）。
pub struct ReviewAgentInput {
    pub repo_path: PathBuf,
    pub target_display_name: String,
    pub target_commit_oid: String,
    pub compare_files: Vec<BrowseCompareFile>,
}

/// agent 运行期间回传 UI 的事件。
pub enum AgentEvent {
    /// 一个新步骤（思维链或工具调用完成）。
    Step(AiReviewStep),
    /// 进度文案（如「第 2 轮 · 已执行工具 5 次」）。
    Progress(String),
    /// 当前轮的流式增量（正文/思考链），供 UI 实时显示「思考中」文本。
    Delta {
        content: Option<String>,
        reasoning: Option<String>,
    },
    /// 评审完成（与返回值同源，便于闭包直接映射为 UiEvent）。
    Done(AiReviewResult),
}

/// 初始上下文里的一个文件条目（差异已加载为 patch 文本）。
pub(crate) struct InitialDiffEntry {
    pub path: String,
    pub status_label: &'static str,
    pub patch: String,
}

// ── 工具参数（serde 解析模型传入的 arguments JSON 字符串） ────────────────

#[derive(Deserialize)]
struct ReadLinesArgs {
    path: String,
    start: Option<usize>,
    end: Option<usize>,
}

#[derive(Deserialize)]
struct PathOnlyArgs {
    path: String,
}

#[derive(Deserialize)]
struct FileTreeArgs {
    path: Option<String>,
}

#[derive(Deserialize)]
struct FileHistoryArgs {
    path: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SearchCodeArgs {
    query: String,
    is_regex: Option<bool>,
    /// 可选的目录前缀限定（如 "src/"），大仓库降噪。
    path_prefix: Option<String>,
}

/// 工具调用预算记账：轮次 / 次数 / 累计结果体积，任一超限即强制收尾。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolBudget {
    pub rounds: usize,
    pub calls: usize,
    pub result_chars: usize,
}

impl ToolBudget {
    /// 是否应停止提供工具（本轮请求省略 tools，逼模型直接给结论）。
    pub fn force_finish(&self) -> bool {
        self.rounds >= MAX_TOOL_ROUNDS
            || self.calls >= MAX_TOOL_CALLS_TOTAL
            || self.result_chars >= MAX_TOTAL_TOOL_RESULT_CHARS
    }

    /// 首个触顶限额的命名，供强制收尾指令与报错文案引用——「超出工具调用
    /// 限额」的笼统文案会让用户误以为只有调用次数一条线（体积线约 50 次
    /// 满额结果就触顶，轮次线另有独立计数）。
    pub fn limit_reason(&self) -> Option<&'static str> {
        if self.rounds >= MAX_TOOL_ROUNDS {
            Some("工具调用轮次上限")
        } else if self.calls >= MAX_TOOL_CALLS_TOTAL {
            Some("工具调用次数上限")
        } else if self.result_chars >= MAX_TOTAL_TOOL_RESULT_CHARS {
            Some("工具结果累计体积上限")
        } else {
            None
        }
    }

    pub fn note_result(&mut self, chars: usize) {
        self.result_chars = self.result_chars.saturating_add(chars);
    }
}

/// 按字符数截断工具结果，超出时追加中文标注。
pub(crate) fn truncate_result_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let truncated: String = text.chars().take(limit).collect();
    format!("{truncated}\n…（结果过长，已截断）")
}

/// read_lines 行窗口钳制：默认从 1 开始；窗口最宽 READ_LINES_MAX_LINES；
/// 越界钳到文件范围内；返回 1 基闭区间。空文件返回 (1, 0) 表示无行。
pub(crate) fn clamp_line_range(
    total_lines: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> (usize, usize) {
    if total_lines == 0 {
        return (1, 0);
    }
    let start = start.unwrap_or(1).clamp(1, total_lines);
    let end = end.unwrap_or(total_lines).clamp(start, total_lines);
    let end = end.min(start + READ_LINES_MAX_LINES - 1).min(total_lines);
    (start, end)
}

/// 装配初始 user prompt：目标分支 + 变更文件清单 + 预算内 diff。
///
/// diff 总量在预算内时全量给出；超预算时逐文件截到 `INITIAL_PER_FILE_CHARS`
/// **且总量仍受 `INITIAL_DIFF_BUDGET_CHARS` 二次封顶**——否则文件数 × 4K
/// 会随变更文件数线性膨胀（200 个文件 ≈ 800K 字符），远超初始预算并撑爆
/// 上下文。预算耗尽后的文件只进清单不附 diff，标注可用 read_diff 获取。
/// 纯函数，便于单测。
pub(crate) fn assemble_initial_user_prompt(
    target_name: &str,
    entries: &[InitialDiffEntry],
) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "目标分支：{target_name}（相对当前分支的三点比较，只包含目标分支领先引入的改动）\n\n"
    ));
    prompt.push_str(&format!("变更文件（{} 个）：\n", entries.len()));
    for entry in entries {
        prompt.push_str(&format!("- [{}] {}\n", entry.status_label, entry.path));
    }

    let total: usize = entries.iter().map(|e| e.patch.chars().count()).sum();
    let per_file_truncate = total > INITIAL_DIFF_BUDGET_CHARS;
    // 截断模式下的剩余总预算：计入每个文件实际发出的全部字符——diff 本体、
    // 截断标注与 `===== [M] path =====` 头部，否则文件数多时头部开销会让
    // 实际 prompt 略超预算线性膨胀。预算耗尽的文件降级为「仅清单 +
    // read_diff 引导」。
    let mut remaining_budget = INITIAL_DIFF_BUDGET_CHARS;
    prompt.push_str("\n差异：\n");
    for entry in entries {
        let header = format!("\n===== [{}] {} =====\n", entry.status_label, entry.path);
        prompt.push_str(&header);
        if entry.patch.is_empty() {
            prompt.push_str("（无文本差异）\n");
            continue;
        }
        if !per_file_truncate {
            prompt.push_str(&entry.patch);
            if !entry.patch.ends_with('\n') {
                prompt.push('\n');
            }
            continue;
        }
        remaining_budget = remaining_budget.saturating_sub(header.chars().count());
        if remaining_budget == 0 {
            prompt.push_str("（差异未附：初始预算已用尽，请用 read_diff 获取）\n");
            continue;
        }
        let allowance = remaining_budget.min(INITIAL_PER_FILE_CHARS);
        let patch_chars = entry.patch.chars().count().min(allowance);
        let emitted = truncate_result_chars(&entry.patch, patch_chars.max(1));
        remaining_budget = remaining_budget.saturating_sub(emitted.chars().count());
        prompt.push_str(&emitted);
        prompt.push('\n');
    }
    prompt
}

/// 把 FileDiff 转成 patch 风格文本，供 AI prompt 与 read_diff 工具共用。
pub fn file_diff_to_patch_text(diff: &FileDiff) -> String {
    if diff.is_binary {
        return format!("（{} 是二进制文件）\n", diff.path);
    }
    let mut text = String::new();
    for line in &diff.lines {
        match line.kind {
            DiffLineKind::Header => {
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Context => {
                text.push(' ');
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Added => {
                text.push('+');
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Removed => {
                text.push('-');
                text.push_str(&line.content);
                text.push('\n');
            }
        }
    }
    text
}

/// 追溯块 → 「行段 → 归属」摘要文本。纯函数，便于单测。
pub(crate) fn format_blame_hunks(hunks: &[BlameHunkInfo]) -> String {
    let mut text = String::new();
    for hunk in hunks {
        let end = hunk.start_line + hunk.line_count.saturating_sub(1);
        match &hunk.commit {
            Some(commit) => {
                let date = chrono::DateTime::<chrono::Utc>::from_timestamp(commit.time, 0)
                    .map(|time| time.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "时间未知".to_string());
                text.push_str(&format!(
                    "L{}-L{} {} {} {} {}\n",
                    hunk.start_line, end, commit.short_oid, commit.author, date, commit.summary
                ));
            }
            None => {
                text.push_str(&format!("L{}-L{} （工作区未提交）\n", hunk.start_line, end));
            }
        }
    }
    text
}

/// 文件历史条目 → 摘要文本。纯函数，便于单测。
pub(crate) fn format_history_lines(commits: &[CommitInfo]) -> String {
    let mut text = String::new();
    for commit in commits {
        let date = chrono::DateTime::<chrono::Utc>::from_timestamp(commit.time, 0)
            .map(|time| time.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "时间未知".to_string());
        text.push_str(&format!(
            "{} {} — {}, {}\n",
            commit.short_oid, commit.summary, commit.author, date
        ));
    }
    text
}

/// 代码搜索命中 → 摘要文本。纯函数，便于单测。
pub(crate) fn format_search_hits(matches: &[CodeSearchMatch]) -> String {
    let mut text = String::new();
    for hit in matches {
        text.push_str(&format!("{}:{}: {}\n", hit.path, hit.lineno, hit.line));
    }
    text
}

// ── agent 主循环 ─────────────────────────────────────────────────────────

/// 运行完整评审 agent。在后台线程调用；`on_event` 在本线程同步回调。
///
/// `is_cancelled` 在每轮边界检查：用户取消后不再发起后续请求，返回
/// `Ok(None)`（不算失败、不触发 Done）；正常完成返回 `Ok(Some(结果))`。
pub fn run_review_agent(
    input: &ReviewAgentInput,
    service: &GitService,
    client: &ChatClient,
    is_cancelled: &std::sync::atomic::AtomicBool,
    on_event: &mut impl FnMut(AgentEvent),
) -> Result<Option<AiReviewResult>> {
    let repo = git2::Repository::open(&input.repo_path)?;

    on_event(AgentEvent::Progress("正在准备评审上下文…".to_string()));
    let entries = collect_initial_entries(service, &repo, input)?;
    let user_prompt = assemble_initial_user_prompt(&input.target_display_name, &entries);

    let mut messages = vec![
        AgentChatMessage::System(agent_system_prompt().to_string()),
        AgentChatMessage::User(user_prompt),
    ];
    let mut steps: Vec<AiReviewStep> = Vec::new();
    let mut budget = ToolBudget::default();
    let tools = tool_schemas();
    // 强制收尾指令只注入一次（触顶后可能连续多轮收尾重试）。
    let mut finish_instruction_injected = false;

    let final_turn = loop {
        // 取消检查在轮次边界：当前轮的流式请求自然读完后退出。
        if is_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        on_event(AgentEvent::Progress(format!(
            "第 {} 轮 · 已执行工具 {} 次",
            budget.rounds + 1,
            budget.calls
        )));
        // 触顶后的强制收尾轮：省略 tools 之外，先注入一条明确的 user 指令——
        // 只靠省略 tools，部分模型会延续对话惯性继续吐 tool_calls，导致
        // 「没到工具调用次数上限却报限额错误」的误伤。
        let force_finish = budget.force_finish();
        if force_finish && !finish_instruction_injected {
            messages.push(AgentChatMessage::User(format!(
                "工具调用预算已用尽（已到{}），请立即基于已获得的信息输出最终评审，不要再调用工具。",
                budget.limit_reason().unwrap_or("工具调用上限")
            )));
            finish_instruction_injected = true;
        }
        let effective_tools: &[ToolSchema] = if force_finish { &[] } else { &tools };
        // 每轮都走流式：增量实时回传 UI（思考中/正文），且超时是读空闲
        // 语义，长思维链不会被整体限时掐断。瞬态失败（供应商网关掐流、
        // 5xx、限流）在此层自动重试：重试提示走 Progress 事件，UI 侧
        // Progress 会清空上一轮的 live 流式文本，半截思考随之复位。
        let mut turn: Option<crate::ai::client::AgentTurn> = None;
        let mut last_retry_reason = String::new();
        for attempt in 0..=AGENT_STREAM_MAX_RETRIES {
            if attempt > 0 {
                // 退避期间复查取消：用户取消后不再消耗重试请求与等待。
                if is_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(None);
                }
                on_event(AgentEvent::Progress(format!(
                    "响应中断（{last_retry_reason}），正在重试第 {attempt}/{} 次…",
                    AGENT_STREAM_MAX_RETRIES
                )));
                std::thread::sleep(std::time::Duration::from_secs(1u64 << (attempt - 1)));
            }
            match client.request_agent_stream(
                &messages,
                effective_tools,
                REVIEW_MAX_TOKENS,
                &mut |delta| {
                    let (content, reasoning) = match delta {
                        crate::ai::client::StreamDelta::Content(text) => (Some(text), None),
                        crate::ai::client::StreamDelta::Reasoning(text) => (None, Some(text)),
                    };
                    on_event(AgentEvent::Delta { content, reasoning });
                },
            ) {
                Ok(result) => {
                    turn = Some(result);
                    break;
                }
                Err(err) => {
                    tracing::warn!(
                        target: "khaslana::ai",
                        "agent 第 {} 轮请求失败（第 {} 次尝试，可重试：{}）：{}",
                        budget.rounds + 1,
                        attempt + 1,
                        err.retryable(),
                        err
                    );
                    // 按错误自带的重试上限判停：瞬态故障给满 3 次，
                    // finish_reason=length 型截断基本确定性，只再试 1 次。
                    if !err.retryable() || attempt >= err.retry_ceiling() {
                        let message = if attempt > 0 {
                            format!("{err}（已自动重试 {attempt} 次仍失败）")
                        } else {
                            err.to_string()
                        };
                        return Err(GitError::Message(message));
                    }
                    last_retry_reason = err.to_string();
                }
            }
        }
        let turn = turn.expect("重试循环必然经 break 或 return 退出");
        budget.rounds += 1;

        if let Some(reasoning) = turn
            .reasoning
            .clone()
            .filter(|reasoning| !reasoning.trim().is_empty())
        {
            steps.push(AiReviewStep::Reasoning { text: reasoning });
            on_event(AgentEvent::Step(steps.last().cloned().unwrap()));
        }

        if turn.tool_calls.is_empty() {
            break turn;
        }
        // 强制收尾轮里模型仍尝试调用工具：有正文就宽容接受为最终结论，
        // 没有正文才报错，且文案指明具体触顶的限额。
        if force_finish {
            if !turn.content.trim().is_empty() {
                break turn;
            }
            return Err(GitError::Message(format!(
                "AI 评审已达{}后仍未输出结论，请重试或更换模型",
                budget.limit_reason().unwrap_or("工具调用上限")
            )));
        }

        // 中间轮的非思考链正文（模型明确说的话）进入时间线，不折叠展示。
        if !turn.content.trim().is_empty() {
            steps.push(AiReviewStep::Message {
                text: turn.content.trim().to_string(),
            });
            on_event(AgentEvent::Step(steps.last().cloned().unwrap()));
        }

        let calls = turn.tool_calls.clone();
        messages.push(AgentChatMessage::Assistant {
            content: turn.content,
            tool_calls: turn.tool_calls,
        });
        for call in calls {
            // 单轮内额度检查：模型一轮批量发起多个调用时不能整体放行
            //（否则 119 + 一轮 10 个 = 129 次超额）。超限的调用不执行，
            // 但仍回填 tool 消息——OpenAI 协议要求每个 tool_call_id 配对。
            if budget.force_finish() {
                let note = format!(
                    "工具预算已用尽（已到{}），请直接输出最终评审。",
                    budget.limit_reason().unwrap_or("工具调用上限")
                );
                budget.note_result(note.chars().count());
                steps.push(AiReviewStep::ToolCall {
                    name: call.name.clone(),
                    args_summary: tool_args_summary(&call.name, &call.arguments),
                    result_excerpt: note.clone(),
                    error: true,
                });
                on_event(AgentEvent::Step(steps.last().cloned().unwrap()));
                messages.push(AgentChatMessage::Tool {
                    tool_call_id: call.id,
                    content: note,
                });
                continue;
            }
            budget.calls += 1;
            let (step, tool_message) = execute_tool(service, &repo, input, &call);
            budget.note_result(tool_message.chars().count());
            steps.push(step);
            on_event(AgentEvent::Step(steps.last().cloned().unwrap()));
            messages.push(AgentChatMessage::Tool {
                tool_call_id: call.id,
                content: tool_message,
            });
        }
    };

    // 取消边界兜底：末轮流式恰好在用户按下取消后才完成时，按取消语义
    // 收尾（不落盘、不算完成），避免「已取消却仍保存记录并提示后台完成」。
    if is_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(None);
    }

    // 空正文按失败处理，与单文件评审一致。
    let content = final_turn.content.trim().to_string();
    if content.is_empty() {
        return Err(GitError::Message(
            if final_turn
                .reasoning
                .as_deref()
                .is_some_and(|reasoning| !reasoning.trim().is_empty())
            {
                "AI 未返回评审正文（仅返回了思考过程），请重试或更换模型"
            } else {
                "AI 返回的评审内容为空，请重试或检查模型配置"
            }
            .into(),
        ));
    }

    let result = AiReviewResult {
        content,
        reasoning: final_turn.reasoning,
        steps,
    };
    on_event(AgentEvent::Done(result.clone()));
    Ok(Some(result))
}

/// agent 评审的 system prompt：评审规则 + 工具使用方式 + 输出格式。
fn agent_system_prompt() -> &'static str {
    "你是一名资深代码评审专家，对目标分支相对当前分支的全部改动做 diff-first 评审。\n\
     工作方式：\n\
     - 初始输入包含变更文件清单与预算内的差异；被截断的文件可用 read_diff 获取完整差异。\n\
     - 需要理解上下文时用 read_lines 读取目标分支上的文件片段，用 get_file_tree 浏览目录，\
     用 search_code 定位标识符的定义与引用（可限定目录前缀），get_file_history / get_blame \
     了解改动历史与行归属（get_blame 基于当前 HEAD 与工作区，不是目标分支版本）。\n\
     - 相互独立的调查请在同一轮批量发起多个工具调用，减少往返轮次。\n\
     - 小改动若初始差异已足够评审，可以不调用任何工具直接输出结论。\n\
     - 工具调用有次数与体积上限，请克制使用：优先评审差异本身，只对真正需要核实的点深入。\n\
     - 全部调查完成后输出最终评审（之后的回复不再调用工具）。\n\
     输出要求（最终回复，中文，Markdown）：\n\
     1. 按文件分组：每个文件一个 `### <路径>` 小节；跨文件的整体问题放最前的 `### 总体` 小节。\n\
     2. 小节内按“严重问题 / 建议 / 风险点”用列表组织，没有的组可省略。\n\
     3. 只评论实际改动与经工具核实过的代码，不要凭空假设未展示的代码。\n\
     4. 指出潜在 bug、安全、性能、可读性问题并给出简短改进建议；示例只给关键片段。\n\
     5. 严重问题尽量引用差异或 read_lines 中的行号作为证据，便于核对。\n\
     6. 保持简洁，聚焦最重要的问题（全部文件合计不超过 15 个要点）。"
}

/// 加载初始上下文：逐文件取紧凑 diff 并转 patch 文本。
fn collect_initial_entries(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
) -> Result<Vec<InitialDiffEntry>> {
    let mut entries = Vec::with_capacity(input.compare_files.len());
    for file in &input.compare_files {
        let patch = match service.browse_file_diff_for_compare(
            repo,
            &input.target_commit_oid,
            Path::new(&file.path),
            file.old_path.as_deref().map(Path::new),
            false,
            DiffEncodingChoice::Auto,
        ) {
            Ok(diff) => file_diff_to_patch_text(&diff),
            Err(err) => format!("（差异加载失败：{err}）\n"),
        };
        entries.push(InitialDiffEntry {
            path: file.path.clone(),
            status_label: file.status.label(),
            patch,
        });
    }
    Ok(entries)
}

/// 内置工具定义（名称 / 中文描述 / 手写 JSON Schema）。
fn tool_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "read_lines",
            description: "读取目标分支上某文件的指定行范围（1 基闭区间），返回带行号的文本。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "仓库相对路径" },
                    "start": { "type": "integer", "description": "起始行（默认 1）" },
                    "end": { "type": "integer", "description": "结束行（默认起始行 + 400）" }
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "read_diff",
            description: "读取某个变更文件的完整差异（目标分支相对当前分支，三点比较）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "变更文件列表中的路径" }
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "get_file_tree",
            description: "列出目标分支某目录的直接子条目（不传 path 为仓库根）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "目录路径（默认根目录）" }
                }
            }),
        },
        ToolSchema {
            name: "get_file_history",
            description: "查看某文件最近的提交历史（全部引用范围，默认 20 条）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "仓库相对路径" },
                    "limit": { "type": "integer", "description": "条数上限（默认 20，最大 20）" }
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "get_blame",
            description: "查看某文件每段行的归属提交（基于当前 HEAD 与工作区未提交改动）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "仓库相对路径" }
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "search_code",
            description: "在目标分支的代码里按子串或正则逐行搜索，返回文件、行号与命中行；可用目录前缀缩小范围。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索内容（子串或正则）" },
                    "is_regex": { "type": "boolean", "description": "是否按正则解释（默认 false）" },
                    "path_prefix": { "type": "string", "description": "可选目录前缀（如 src/），限定搜索范围" }
                },
                "required": ["query"]
            }),
        },
    ]
}

/// 执行一次工具调用，返回 (UI 步骤, 回填给模型的 tool 消息文本)。
///
/// 工具执行失败不终止整个评审：错误文本作为工具结果回填，模型可自行调整。
fn execute_tool(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    call: &crate::ai::client::AgentToolCall,
) -> (AiReviewStep, String) {
    let outcome = dispatch_tool(service, repo, input, &call.name, &call.arguments);
    let (result_excerpt, error) = match outcome {
        Ok(text) => (text, false),
        Err(err) => (format!("工具执行失败：{err}"), true),
    };
    let step = AiReviewStep::ToolCall {
        name: call.name.clone(),
        args_summary: tool_args_summary(&call.name, &call.arguments),
        result_excerpt: result_excerpt.clone(),
        error,
    };
    (step, result_excerpt)
}

/// 工具参数的一行摘要（时间线上展示）。
fn tool_args_summary(name: &str, arguments: &str) -> String {
    let value: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let quote = |text: &str| {
        let mut visible = text.replace('\n', " ");
        if visible.chars().count() > 60 {
            visible = visible.chars().take(60).collect::<String>() + "…";
        }
        visible
    };
    match name {
        "read_lines" => {
            let path = value["path"].as_str().unwrap_or("");
            let start = value["start"].as_u64();
            let end = value["end"].as_u64();
            match (start, end) {
                (Some(start), Some(end)) => format!("read_lines {path}:{start}-{end}"),
                (Some(start), None) => format!("read_lines {path}:{start}-…"),
                _ => format!("read_lines {path}"),
            }
        }
        "read_diff" => format!("read_diff {}", quote(value["path"].as_str().unwrap_or(""))),
        "get_file_tree" => {
            let path = value["path"].as_str().unwrap_or("");
            if path.is_empty() {
                "get_file_tree /".to_string()
            } else {
                format!("get_file_tree {path}")
            }
        }
        "get_file_history" => format!(
            "get_file_history {}",
            quote(value["path"].as_str().unwrap_or(""))
        ),
        "get_blame" => format!("get_blame {}", quote(value["path"].as_str().unwrap_or(""))),
        "search_code" => format!(
            "search_code {}{}",
            quote(value["query"].as_str().unwrap_or("")),
            if value["is_regex"].as_bool().unwrap_or(false) {
                "（正则）"
            } else {
                ""
            }
        ),
        other => other.to_string(),
    }
}

/// 按工具名分发执行；参数 JSON 无效时返回中文错误。
fn dispatch_tool(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    name: &str,
    arguments: &str,
) -> Result<String> {
    let parse_error = |err: serde_json::Error| GitError::Message(format!("参数解析失败：{err}"));
    match name {
        "read_lines" => {
            let args: ReadLinesArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_read_lines(service, repo, input, &args)
        }
        "read_diff" => {
            let args: PathOnlyArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_read_diff(service, repo, input, &args.path)
        }
        "get_file_tree" => {
            let args: FileTreeArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_get_file_tree(service, repo, input, args.path.as_deref())
        }
        "get_file_history" => {
            let args: FileHistoryArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_get_file_history(service, repo, &args.path, args.limit)
        }
        "get_blame" => {
            let args: PathOnlyArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_get_blame(service, repo, &args.path)
        }
        "search_code" => {
            let args: SearchCodeArgs = serde_json::from_str(arguments).map_err(parse_error)?;
            tool_search_code(
                service,
                repo,
                input,
                &args.query,
                args.is_regex.unwrap_or(false),
                args.path_prefix
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty()),
            )
        }
        other => Err(GitError::Message(format!("未知工具：{other}"))),
    }
}

/// read_lines：读目标分支文件指定行范围（带行号输出）。
fn tool_read_lines(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    args: &ReadLinesArgs,
) -> Result<String> {
    let path = PathBuf::from(&args.path);
    if args.path.trim().is_empty() {
        return Err(GitError::Message("缺少 path 参数".into()));
    }
    // 体积守卫：先读对象头，超过 1MB 不整体加载。
    if !blob_size_within(
        repo,
        service,
        &input.target_commit_oid,
        &path,
        READ_LINES_MAX_BYTES,
    )? {
        return Ok(format!(
            "（{} 超过 1MB，超出读取上限，请用 read_diff 或缩小范围）",
            args.path
        ));
    }
    let content = service.browse_file_content(
        repo,
        &input.target_commit_oid,
        &path,
        DiffEncodingChoice::Auto,
    )?;
    if content.is_binary {
        return Ok(format!("（{} 是二进制文件，无法按行读取）", args.path));
    }
    let (start, end) = clamp_line_range(content.lines.len(), args.start, args.end);
    let mut text = String::new();
    for (index, line) in content
        .lines
        .iter()
        .enumerate()
        .take(end)
        .skip(start.saturating_sub(1))
    {
        text.push_str(&format!("{:>5} | {}\n", index + 1, line));
    }
    if text.is_empty() {
        text.push_str("（文件为空）\n");
    }
    Ok(truncate_result_chars(&text, MAX_TOOL_RESULT_CHARS))
}

/// read_diff：某变更文件的完整差异（超长截断）。
fn tool_read_diff(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    path: &str,
) -> Result<String> {
    if path.trim().is_empty() {
        return Err(GitError::Message("缺少 path 参数".into()));
    }
    let old_path = input
        .compare_files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| file.old_path.clone());
    let diff = service.browse_file_diff_for_compare(
        repo,
        &input.target_commit_oid,
        Path::new(path),
        old_path.as_deref().map(Path::new),
        false,
        DiffEncodingChoice::Auto,
    )?;
    Ok(truncate_result_chars(
        &file_diff_to_patch_text(&diff),
        MAX_TOOL_RESULT_CHARS,
    ))
}

/// get_file_tree：列目录直接子条目（上限 300 条）。
fn tool_get_file_tree(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    path: Option<&str>,
) -> Result<String> {
    let prefix = path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let entries = service.browse_tree_entries(repo, &input.target_commit_oid, prefix.as_deref())?;
    let mut text = String::new();
    for entry in entries.iter().take(FILE_TREE_MAX_ENTRIES) {
        match entry.kind {
            crate::types::BrowseEntryKind::Directory => {
                text.push_str(&format!("dir  {}/\n", entry.name));
            }
            crate::types::BrowseEntryKind::Submodule => {
                text.push_str(&format!("sub  {} (子模块)\n", entry.name));
            }
            crate::types::BrowseEntryKind::File => {
                text.push_str(&format!("file {} ({} B)\n", entry.name, entry.size));
            }
        }
    }
    if entries.len() > FILE_TREE_MAX_ENTRIES {
        text.push_str(&format!(
            "…（共 {} 条，仅显示前 {} 条）\n",
            entries.len(),
            FILE_TREE_MAX_ENTRIES
        ));
    }
    Ok(text)
}

/// get_file_history：文件最近提交历史（钳 20 条）。
fn tool_get_file_history(
    service: &GitService,
    repo: &git2::Repository,
    path: &str,
    limit: Option<usize>,
) -> Result<String> {
    if path.trim().is_empty() {
        return Err(GitError::Message("缺少 path 参数".into()));
    }
    let limit = limit
        .unwrap_or(FILE_HISTORY_MAX_LIMIT)
        .clamp(1, FILE_HISTORY_MAX_LIMIT);
    let (commits, _) = service.file_history(repo, HistoryScope::AllRefs, path, 0, limit, None)?;
    let mut text = format_history_lines(&commits);
    if text.is_empty() {
        text.push_str("（没有找到该文件的历史）\n");
    }
    Ok(truncate_result_chars(&text, MAX_TOOL_RESULT_CHARS))
}

/// get_blame：行段归属摘要（基于 HEAD + 工作区；target 版本追溯留作后续）。
fn tool_get_blame(service: &GitService, repo: &git2::Repository, path: &str) -> Result<String> {
    if path.trim().is_empty() {
        return Err(GitError::Message("缺少 path 参数".into()));
    }
    let view = service.blame_file(repo, Path::new(path), DiffEncodingChoice::Auto)?;
    let mut text = format_blame_hunks(&view.hunks);
    if text.is_empty() {
        text.push_str("（没有追溯信息）\n");
    }
    // 基准说明：其它工具都读目标提交，blame 只有 HEAD + 工作区版本，
    // 行号/归属可能与评审对象对不上，明示模型避免据此下错结论。
    let text = format!(
        "（注意：以下追溯基于当前 HEAD 与工作区，不是目标分支版本，行号可能与目标分支不一致）\n{text}"
    );
    Ok(truncate_result_chars(&text, MAX_TOOL_RESULT_CHARS))
}

/// search_code：目标分支代码搜索（上限 50 命中，可限定目录前缀）。
fn tool_search_code(
    service: &GitService,
    repo: &git2::Repository,
    input: &ReviewAgentInput,
    query: &str,
    is_regex: bool,
    path_prefix: Option<&str>,
) -> Result<String> {
    let matches = service.search_code(
        repo,
        &input.target_commit_oid,
        query,
        is_regex,
        path_prefix,
        SEARCH_MAX_HITS,
    )?;
    let mut text = format_search_hits(&matches);
    if text.is_empty() {
        text.push_str("（没有命中）\n");
    }
    Ok(truncate_result_chars(&text, MAX_TOOL_RESULT_CHARS))
}

/// 读 blob 对象头判断文件体积是否在限制内；路径不存在直接报错。
fn blob_size_within(
    repo: &git2::Repository,
    service: &GitService,
    commit_oid: &str,
    path: &Path,
    limit: u64,
) -> Result<bool> {
    let commit = service.find_commit_by_oid(repo, commit_oid)?;
    let tree = commit.tree()?;
    let entry = tree
        .get_path(path)
        .map_err(|_| GitError::Message(format!("文件不存在: {}", path.display())))?;
    if let Ok(odb) = repo.odb()
        && let Ok((size, _)) = odb.read_header(entry.id())
        && size as u64 > limit
    {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
#[path = "../tests/ai/review_agent.rs"]
mod tests;
