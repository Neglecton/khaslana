//! 索引查询 API 层（参照 codebase-memory-mcp 的工具面语义）。
//!
//! 全部函数走只读连接 + `store.load_graph()` 内存图遍历（Phase 1 规模毫秒级；
//! 超大仓库的优化点是改走 SQL 邻接查询，此处先保持简单）。消费方：
//! 内嵌 MCP 服务器（`mcp.rs`）、全局符号搜索面板（bin crate）、以及后续
//! Phase 2 AI 代码搜索的 agent 工具层。
//!
//! 风险标签移植参考项目 `cbm_hop_to_risk`：hop 1→CRITICAL / 2→HIGH /
//! 3→MEDIUM / ≥4→LOW——离改动越近的调用方越需要优先审查。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use serde::Serialize;

use super::err;
use super::graph::{EdgeType, GraphBuffer, NodeId, NodeLabel};
use super::store::{CodeIndexStore, open_read_only_if_exists};
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
    pub direction: TraceDirection,
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
fn label_priority(label: NodeLabel) -> u8 {
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

fn candidate_of(node: &super::graph::GraphNode) -> SymbolCandidate {
    SymbolCandidate {
        name: node.name.clone(),
        label: node.label.as_str().to_string(),
        qualified_name: node.qualified_name.clone(),
        file_path: node.file_path.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
    }
}

/// 精确名（name 或 qualified_name）查找，返回全部候选。
pub fn find_symbol_candidates(db_path: &Path, name: &str) -> Result<Vec<SymbolCandidate>> {
    let graph = load_graph(db_path)?;
    Ok(graph
        .nodes
        .iter()
        .filter(|n| n.name == name || n.qualified_name == name)
        .map(candidate_of)
        .collect())
}

/// 内部便捷：只读打开已存在的索引库并载入全图。未索引（文件不存在 /
/// 非本引擎库 / schema 版本不符）时返回中文错误——绝不创建幽灵空库。
fn open_query_store(db_path: &Path) -> Result<CodeIndexStore> {
    open_read_only_if_exists(db_path)?.ok_or_else(|| {
        err("代码索引不存在或尚未建立，请先在 Khaslana 设置中心启用代码索引或调用 refresh_index")
    })
}

fn load_graph(db_path: &Path) -> Result<GraphBuffer> {
    open_query_store(db_path)?.load_graph()
}

// ---------------------------------------------------------------------------
// 调用链 BFS
// ---------------------------------------------------------------------------

fn risk_for_hop(hop: u32) -> &'static str {
    match hop {
        1 => "CRITICAL",
        2 => "HIGH",
        3 => "MEDIUM",
        _ => "LOW",
    }
}

/// CALLS 邻接表（outbound / inbound 两份）。
struct CallAdjacency {
    outbound: HashMap<NodeId, Vec<NodeId>>,
    inbound: HashMap<NodeId, Vec<NodeId>>,
}

impl CallAdjacency {
    fn build(graph: &GraphBuffer) -> Self {
        let mut adjacency = Self {
            outbound: HashMap::new(),
            inbound: HashMap::new(),
        };
        for edge in &graph.edges {
            if edge.etype != EdgeType::Calls {
                continue;
            }
            adjacency
                .outbound
                .entry(edge.source)
                .or_default()
                .push(edge.target);
            adjacency
                .inbound
                .entry(edge.target)
                .or_default()
                .push(edge.source);
        }
        adjacency
    }

    fn step(&self, node: NodeId, inbound: bool) -> Vec<NodeId> {
        let map = if inbound {
            &self.inbound
        } else {
            &self.outbound
        };
        map.get(&node).cloned().unwrap_or_default()
    }
}

/// 从一组起点做 BFS，收集 hop 1..=depth 的节点（排除起点自身）。
/// visited 全局去重（多起点联合遍历，参照参考项目 bfs_union_same_name）。
fn bfs_hops(
    graph: &GraphBuffer,
    adjacency: &CallAdjacency,
    starts: &[NodeId],
    inbound: bool,
    depth: u32,
    max_nodes: usize,
) -> Vec<TraceHop> {
    let mut visited: HashSet<NodeId> = starts.iter().copied().collect();
    let mut queue: VecDeque<(NodeId, u32)> = starts.iter().map(|id| (*id, 0)).collect();
    let mut hops: Vec<TraceHop> = Vec::new();
    while let Some((node, hop)) = queue.pop_front() {
        if hop >= depth {
            continue;
        }
        for next in adjacency.step(node, inbound) {
            if !visited.insert(next) {
                continue;
            }
            if hops.len() >= max_nodes {
                return hops;
            }
            let info = graph.get(next);
            hops.push(TraceHop {
                name: info.name.clone(),
                qualified_name: info.qualified_name.clone(),
                file_path: info.file_path.clone(),
                hop: hop + 1,
                risk: risk_for_hop(hop + 1),
            });
            queue.push_back((next, hop + 1));
        }
    }
    hops.sort_by(|a, b| {
        a.hop
            .cmp(&b.hop)
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    hops
}

/// 在已载入的图内做精确名解析（避免调用方二次开库载图）。
/// `callable_only` 限定 Function/Method（trace 语义）；歧义取最高
/// label 优先级层级，同层多个 → Ambiguous。
enum GraphResolution<'a> {
    One(&'a super::graph::GraphNode),
    Ambiguous(Vec<SymbolCandidate>),
    NotFound,
}

fn resolve_in_graph<'a>(
    graph: &'a GraphBuffer,
    name: &str,
    callable_only: bool,
) -> GraphResolution<'a> {
    let candidates: Vec<&super::graph::GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| {
            (n.name == name || n.qualified_name == name)
                && (!callable_only || matches!(n.label, NodeLabel::Function | NodeLabel::Method))
        })
        .collect();
    if candidates.is_empty() {
        return GraphResolution::NotFound;
    }
    let top_priority = candidates
        .iter()
        .map(|n| label_priority(n.label))
        .max()
        .expect("候选非空");
    let group: Vec<&super::graph::GraphNode> = candidates
        .into_iter()
        .filter(|n| label_priority(n.label) == top_priority)
        .collect();
    if group.len() == 1 {
        GraphResolution::One(group[0])
    } else {
        let mut candidates: Vec<SymbolCandidate> = group.iter().map(|n| candidate_of(n)).collect();
        candidates.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        GraphResolution::Ambiguous(candidates)
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
    let graph = load_graph(db_path)?;
    let resolved = match resolve_in_graph(&graph, function_name, true) {
        GraphResolution::One(node) => node,
        GraphResolution::Ambiguous(c) => return Ok(TraceOutcome::Ambiguous(c)),
        GraphResolution::NotFound => return Ok(TraceOutcome::NotFound),
    };

    let adjacency = CallAdjacency::build(&graph);
    let start = resolved.id;
    let starts = vec![start];
    let result = TraceResult {
        function: resolved.name.clone(),
        direction,
        callees: if matches!(direction, TraceDirection::Outbound | TraceDirection::Both) {
            bfs_hops(&graph, &adjacency, &starts, false, depth, max_nodes)
        } else {
            Vec::new()
        },
        callers: if matches!(direction, TraceDirection::Inbound | TraceDirection::Both) {
            bfs_hops(&graph, &adjacency, &starts, true, depth, max_nodes)
        } else {
            Vec::new()
        },
    };
    Ok(TraceOutcome::Found(result))
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
    let graph = load_graph(db_path)?;
    let resolved = match resolve_in_graph(&graph, name, false) {
        GraphResolution::One(node) => node,
        GraphResolution::Ambiguous(c) => return Ok(DetailOutcome::Ambiguous(c)),
        GraphResolution::NotFound => return Ok(DetailOutcome::NotFound),
    };

    let adjacency = CallAdjacency::build(&graph);
    let starts = vec![resolved.id];
    let candidate = candidate_of(resolved);
    let source = repo_root.and_then(|root| read_source_snippet(root, &candidate));
    Ok(DetailOutcome::Found(Box::new(SymbolDetail {
        name: resolved.name.clone(),
        label: resolved.label.as_str().to_string(),
        qualified_name: resolved.qualified_name.clone(),
        file_path: resolved.file_path.clone(),
        start_line: resolved.start_line,
        end_line: resolved.end_line,
        callers: bfs_hops(&graph, &adjacency, &starts, true, 1, 100),
        callees: bfs_hops(&graph, &adjacency, &starts, false, 1, 100),
        source,
    })))
}

/// 读取定义处源码片段（行区间 1-based，钳 200 行）。行号对文件实际行数
/// 双向钳制——索引过期（文件被改短而增量未跑）时不 panic，直接返回 None。
fn read_source_snippet(repo_root: &Path, candidate: &SymbolCandidate) -> Option<SourceSnippet> {
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
    let store = CodeIndexStore::open(db_path)?;
    let graph = store.load_graph()?;
    let stats = store.read_stats()?.unwrap_or_default();

    let mut label_counts: HashMap<String, usize> = HashMap::new();
    for node in &graph.nodes {
        *label_counts
            .entry(node.label.as_str().to_string())
            .or_default() += 1;
    }
    let mut label_counts: Vec<(String, usize)> = label_counts.into_iter().collect();
    label_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut edge_counts: HashMap<String, usize> = HashMap::new();
    for edge in &graph.edges {
        *edge_counts
            .entry(edge.etype.as_str().to_string())
            .or_default() += 1;
    }
    let mut edge_counts: Vec<(String, usize)> = edge_counts.into_iter().collect();
    edge_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // 语言分布：File 节点按扩展名聚合。
    let mut languages: HashMap<String, usize> = HashMap::new();
    let mut top_dirs: HashMap<String, usize> = HashMap::new();
    for node in &graph.nodes {
        if node.label != NodeLabel::File {
            continue;
        }
        let ext = std::path::Path::new(&node.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| "other".to_string());
        *languages.entry(ext).or_default() += 1;
        let dir = match node.file_path.split_once('/') {
            Some((top, _)) => top.to_string(),
            None => "(根目录)".to_string(),
        };
        // 目录密度按符号数而非文件数（口径：该目录下定义的符号总量）。
        top_dirs.insert(dir, 0);
    }
    for node in &graph.nodes {
        if !node.label.is_symbol() || node.file_path.is_empty() {
            continue;
        }
        let dir = match node.file_path.split_once('/') {
            Some((top, _)) => top.to_string(),
            None => "(根目录)".to_string(),
        };
        if let Some(count) = top_dirs.get_mut(&dir) {
            *count += 1;
        }
    }
    let mut languages: Vec<(String, usize)> = languages.into_iter().collect();
    languages.sort_by(|a, b| b.1.cmp(&a.1));
    let mut top_dirs: Vec<(String, usize)> = top_dirs.into_iter().collect();
    top_dirs.sort_by(|a, b| b.1.cmp(&a.1));
    top_dirs.truncate(10);

    // 热点：CALLS 入边 Top 10。
    let mut fan_in: HashMap<NodeId, usize> = HashMap::new();
    for edge in &graph.edges {
        if edge.etype == EdgeType::Calls {
            *fan_in.entry(edge.target).or_default() += 1;
        }
    }
    let mut hotspots: Vec<Hotspot> = fan_in
        .into_iter()
        .map(|(id, fan_in)| {
            let node = graph.get(id);
            Hotspot {
                name: node.name.clone(),
                qualified_name: node.qualified_name.clone(),
                file_path: node.file_path.clone(),
                fan_in,
            }
        })
        .collect();
    hotspots.sort_by(|a, b| {
        b.fan_in
            .cmp(&a.fan_in)
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    hotspots.truncate(10);

    Ok(IndexOverview {
        total_nodes: graph.node_count(),
        total_edges: graph.edges.len(),
        files: stats.files,
        symbols: stats.symbols,
        calls: stats.calls,
        label_counts,
        edge_counts,
        languages,
        top_dirs,
        hotspots,
    })
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
    let graph = load_graph(db_path)?;
    let changed: HashSet<&str> = changed_files.iter().map(|s| s.as_str()).collect();

    let impacted: Vec<SymbolCandidate> = graph
        .nodes
        .iter()
        .filter(|n| {
            n.label.is_symbol() && !n.file_path.is_empty() && changed.contains(n.file_path.as_str())
        })
        .map(candidate_of)
        .collect();

    let impacted_ids: Vec<NodeId> = impacted
        .iter()
        .filter_map(|c| graph.find_by_qn(&c.qualified_name))
        .collect();
    let adjacency = CallAdjacency::build(&graph);
    let callers = if impacted_ids.is_empty() || expand_depth == 0 {
        Vec::new()
    } else {
        bfs_hops(&graph, &adjacency, &impacted_ids, true, expand_depth, 200)
    };

    Ok(ImpactReport {
        changed_count: changed_files.len(),
        changed_files: changed_files.to_vec(),
        impacted_symbols: impacted,
        callers,
    })
}
