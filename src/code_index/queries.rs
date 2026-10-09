//! 索引查询 API 层（参照 codebase-memory-mcp 的工具面语义）。
//!
//! 只读查询在同一 SQLite 快照内按需读取节点和调用邻接关系。消费方：
//! 内嵌 MCP 服务器（`mcp.rs`）、全局符号搜索面板（bin crate）、以及后续
//! Phase 2 AI 代码搜索的 agent 工具层。
//!
//! 风险标签移植参考项目 `cbm_hop_to_risk`：hop 1→CRITICAL / 2→HIGH /
//! 3→MEDIUM / ≥4→LOW——离改动越近的调用方越需要优先审查。

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::err;
use super::graph::NodeLabel;
use crate::types::Result;

/// 单条符号候选（精确名查找结果）。
#[derive(Clone, Debug, Serialize)]
pub struct SymbolCandidate {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// 一跳调用关系。
#[derive(Clone, Debug, Serialize)]
pub struct TraceHop {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub hop: u32,
    pub risk: &'static str,
    /// 单目标查询的直接边提供调用点证据，total 仍统计去重的调用者节点。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_sites: Option<CallSiteSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallSiteSummary {
    pub file_path: String,
    pub count: usize,
    pub lines: Vec<u32>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceDirection {
    Inbound,
    Outbound,
    Both,
}

impl TraceDirection {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "inbound" => Self::Inbound,
            "outbound" => Self::Outbound,
            _ => Self::Both,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TraceResult {
    pub function: String,
    pub qualified_name: String,
    pub direction: TraceDirection,
    pub callers_total: usize,
    pub callees_total: usize,
    pub offset: usize,
    pub has_more: bool,
    /// direction 为 Outbound/Both 时存在。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callees: Vec<TraceHop>,
    /// direction 为 Inbound/Both 时存在。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callers: Vec<TraceHop>,
}

#[derive(Debug)]
pub enum TraceOutcome {
    Found(TraceResult),
    Ambiguous(Vec<SymbolCandidate>),
    NotFound,
}

/// 符号详情（get_symbol_detail / 搜索面板右栏的数据载体）。
#[derive(Clone, Debug, Serialize)]
pub struct SymbolDetail {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub docstring: String,
    pub source_freshness: String,
    pub callers: Vec<TraceHop>,
    pub callees: Vec<TraceHop>,
    /// 定义处源码片段（repo_root 可用时读取；超长截断）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSnippet>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceSnippet {
    pub start_line: u32,
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum DetailOutcome {
    Found(Box<SymbolDetail>),
    Ambiguous(Vec<SymbolCandidate>),
    NotFound,
}

/// 索引总览（get_architecture / 设置页状态卡共用口径）。
#[derive(Debug, Serialize)]
pub struct IndexOverview {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub files: usize,
    pub symbols: usize,
    pub calls: usize,
    pub label_counts: Vec<(String, usize)>,
    pub edge_counts: Vec<(String, usize)>,
    /// 按文件扩展名聚合（无法识别的扩展名计为 "other"）。
    pub languages: Vec<(String, usize)>,
    /// 顶层目录 → 符号数（根目录文件计为 "(根目录)"），取前 10。
    pub top_dirs: Vec<(String, usize)>,
    /// CALLS 入边最多的可调用符号（fan-in 热点），取前 10。
    pub hotspots: Vec<Hotspot>,
}

#[derive(Debug, Serialize)]
pub struct Hotspot {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub fan_in: usize,
}

/// 工作区/分支变更的影响报告（detect_changes / Phase 4 影响分析雏形）。
#[derive(Debug, Serialize)]
pub struct ImpactReport {
    pub changed_files: Vec<String>,
    pub changed_count: usize,
    /// 变更文件中定义的符号（排除 File/Folder 等结构节点）。
    pub impacted_symbols: Vec<SymbolCandidate>,
    /// 受影响符号的入边调用方展开（去重，含 hop/风险分级）。
    pub callers: Vec<TraceHop>,
}

/// 源码片段读取上限（get_symbol_detail 的防御性钳制）。
const SOURCE_MAX_LINES: usize = 200;
/// 源码文件读取字节上限（与引擎 PARSE_MAX_BYTES 同口径）。
const SOURCE_MAX_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// 名称解析
// ---------------------------------------------------------------------------

/// 标签解析优先级（参照参考项目 node_resolution_score 的层级思想）：
/// Function/Method > 其他定义符号 > 文件级节点。
pub(super) fn label_priority(label: NodeLabel) -> u8 {
    match label {
        NodeLabel::Function | NodeLabel::Method => 2,
        NodeLabel::Class
        | NodeLabel::Struct
        | NodeLabel::Interface
        | NodeLabel::Enum
        | NodeLabel::Trait
        | NodeLabel::Type
        | NodeLabel::Field => 1,
        _ => 0,
    }
}

/// 精确名查询通过数据库索引定位候选，不载入整图。
pub fn find_symbol_candidates(db_path: &Path, name: &str) -> Result<Vec<SymbolCandidate>> {
    super::sql_queries::find_symbol_candidates(db_path, name)
}

// ---------------------------------------------------------------------------
// 调用链 BFS
// ---------------------------------------------------------------------------

pub(super) fn risk_for_hop(hop: u32) -> &'static str {
    match hop {
        1 => "CRITICAL",
        2 => "HIGH",
        3 => "MEDIUM",
        _ => "LOW",
    }
}

/// 调用链追踪：名称 → 唯一解析 → CALLS 边 BFS。
pub fn trace_calls(
    db_path: &Path,
    function_name: &str,
    direction: TraceDirection,
    depth: u32,
    max_nodes: usize,
) -> Result<TraceOutcome> {
    trace_calls_page(db_path, function_name, direction, depth, max_nodes, 0)
}

/// 每个方向独立分页；先统计深度范围内的可达节点，避免静默截断调用关系。
pub fn trace_calls_page(
    db_path: &Path,
    function_name: &str,
    direction: TraceDirection,
    depth: u32,
    limit: usize,
    offset: usize,
) -> Result<TraceOutcome> {
    super::sql_queries::trace_calls_page(db_path, function_name, direction, depth, limit, offset)
}

// ---------------------------------------------------------------------------
// 符号详情
// ---------------------------------------------------------------------------

/// 符号详情：定义 + 一跳调用关系 + 源码片段。
pub fn symbol_detail(
    db_path: &Path,
    repo_root: Option<&Path>,
    name: &str,
) -> Result<DetailOutcome> {
    super::sql_queries::symbol_detail(db_path, repo_root, name)
}

/// 读取定义处源码片段（行区间 1-based，钳 200 行）。行号对文件实际行数
/// 双向钳制——索引过期（文件被改短而增量未跑）时不 panic，直接返回 None。
pub(super) fn read_source_snippet(repo_root: &Path, candidate: &SymbolCandidate) -> Option<SourceSnippet> {
    if candidate.file_path.is_empty() || candidate.start_line == 0 {
        return None;
    }
    let path = repo_root.join(&candidate.file_path);
    let Ok(meta) = std::fs::metadata(&path) else {
        return None;
    };
    if meta.len() > SOURCE_MAX_BYTES {
        return None;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return None;
    };
    let text = String::from_utf8_lossy(&bytes);
    let all_lines: Vec<&str> = text.lines().collect();
    if all_lines.is_empty() {
        return None;
    }
    let start = (candidate.start_line as usize).clamp(1, all_lines.len());
    let end = (candidate.end_line as usize)
        .min(all_lines.len())
        .max(start);
    let take_end = (end - start + 1).min(SOURCE_MAX_LINES);
    let truncated = end - start + 1 > SOURCE_MAX_LINES;
    Some(SourceSnippet {
        start_line: start as u32,
        lines: all_lines[start - 1..start - 1 + take_end]
            .iter()
            .map(|l| l.to_string())
            .collect(),
        truncated,
    })
}

// ---------------------------------------------------------------------------
// 索引总览
// ---------------------------------------------------------------------------

/// 索引总览统计（label/边类型分布、语言、目录密度、调用热点）。
pub fn index_overview(db_path: &Path) -> Result<IndexOverview> {
    super::sql_queries::index_overview(db_path)
}

// ---------------------------------------------------------------------------
// 变更检测与影响分析（git2 实现，替代参考项目的 shell git 三命令）
// ---------------------------------------------------------------------------

/// 收集变更文件：默认 = 工作区三态并集（未暂存 + 已暂存 + 未跟踪，
/// git2 statuses 一次给出）；提供 base_branch 时额外并入
/// merge-base(base)...HEAD 的已提交差异（三点语义）。
pub fn changed_files_via_git(repo_root: &Path, base_branch: Option<&str>) -> Result<Vec<String>> {
    let repo = git2::Repository::open(repo_root).map_err(|e| err(format!("打开仓库失败：{e}")))?;
    let mut files: HashSet<String> = HashSet::new();

    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .include_ignored(false)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|e| err(format!("读取工作区状态失败：{e}")))?;
    for entry in statuses.iter() {
        // git2 0.21 的 StatusEntry::path() 返回 Result<&str>。
        if let Ok(path) = entry.path() {
            // rename 条目的 path() 已是新路径；删除条目也保留（符号域仍在图里）。
            files.insert(path.replace('\\', "/"));
        }
    }

    if let Some(base) = base_branch {
        let base_commit = repo
            .revparse_single(base)
            .and_then(|obj| obj.peel_to_commit())
            .map_err(|e| err(format!("无法解析基线分支 {base}：{e}")))?;
        let head_commit = repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|e| err(format!("无法解析 HEAD：{e}")))?;
        let merge_base = repo
            .merge_base(base_commit.id(), head_commit.id())
            .map_err(|e| err(format!("计算 merge-base 失败：{e}")))?;
        let base_tree = repo
            .find_commit(merge_base)
            .and_then(|commit| commit.tree())
            .map_err(|e| err(format!("读取基线树失败：{e}")))?;
        let head_tree = head_commit
            .tree()
            .map_err(|e| err(format!("读取 HEAD 树失败：{e}")))?;
        let diff = repo
            .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)
            .map_err(|e| err(format!("生成基线差异失败：{e}")))?;
        for delta in diff.deltas() {
            if let Some(path) = delta.new_file().path() {
                files.insert(path.to_string_lossy().replace('\\', "/"));
            } else if let Some(path) = delta.old_file().path() {
                files.insert(path.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    let mut files: Vec<String> = files.into_iter().collect();
    files.sort();
    Ok(files)
}

/// 变更影响分析：变更文件 → 文件内定义的符号 → 沿 CALLS 入边展开调用方。
/// 变更文件由 [`changed_files_via_git`] 自动收集（工作区三态）。
pub fn impacted_symbols(
    db_path: &Path,
    repo_root: &Path,
    expand_depth: u32,
) -> Result<ImpactReport> {
    let changed_files = changed_files_via_git(repo_root, None)?;
    impacted_symbols_for_files(db_path, &changed_files, expand_depth)
}

/// 同上，但使用调用方给定的变更文件集（如 MCP 的 base_branch 三点 diff）。
pub fn impacted_symbols_for_files(
    db_path: &Path,
    changed_files: &[String],
    expand_depth: u32,
) -> Result<ImpactReport> {
    super::sql_queries::impacted_symbols_for_files(db_path, changed_files, expand_depth)
}
