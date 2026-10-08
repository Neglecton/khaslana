//! 查询在只读事务中按需展开邻接关系，不受整图缓存容量限制。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use rusqlite::types::Value;
use super::queries::*;
use super::store::{CodeIndexStore, open_read_only_if_exists, parse_label};
use super::err;
use crate::types::Result;

struct Node {
    id: i64,
    candidate: SymbolCandidate,
    properties: String,
}

fn read<T>(path: &Path, query: impl FnOnce(&CodeIndexStore) -> Result<T>) -> Result<T> {
    let store = open_read_only_if_exists(path)?.ok_or_else(|| err("代码索引不存在或尚未建立，请先调用 refresh_index"))?;
    let tx = store.conn.unchecked_transaction().map_err(|error| err(format!("开启索引查询失败：{error}")))?;
    let value = query(&store)?;
    tx.commit().map_err(|error| err(format!("结束索引查询失败：{error}")))?;
    Ok(value)
}

fn nodes(conn: &Connection, sql: &str, args: &[Value]) -> Result<Vec<Node>> {
    let mut stmt = conn.prepare_cached(sql).map_err(|error| err(format!("准备节点查询失败：{error}")))?;
    let rows = stmt.query_map(params_from_iter(args), |row| Ok(Node { id: row.get(0)?,
        candidate: SymbolCandidate { label: row.get(1)?, name: row.get(2)?, qualified_name: row.get(3)?,
            file_path: row.get(4)?, start_line: row.get(5)?, end_line: row.get(6)? }, properties: row.get(7)? }))
        .map_err(|error| err(format!("查询节点失败：{error}")))?;
    rows.collect::<rusqlite::Result<_>>().map_err(|error| err(format!("读取节点失败：{error}")))
}

const COLUMNS: &str = "id,label,name,qualified_name,file_path,start_line,end_line,properties";

fn named(conn: &Connection, name: &str) -> Result<Vec<Node>> {
    nodes(conn, &format!("SELECT {COLUMNS} FROM nodes WHERE qualified_name=?1 OR name=?1 ORDER BY id"), &[name.to_string().into()])
}

pub(super) fn find_symbol_candidates(path: &Path, name: &str) -> Result<Vec<SymbolCandidate>> {
    read(path, |store| Ok(named(&store.conn, name)?.into_iter().map(|node| node.candidate).collect()))
}

fn resolve(conn: &Connection, name: &str, callable: bool) -> Result<Vec<Node>> {
    let mut candidates = named(conn, name)?;
    if candidates.iter().any(|node| node.candidate.qualified_name == name) {
        candidates.retain(|node| node.candidate.qualified_name == name);
    }
    candidates.retain(|node| !callable || matches!(node.candidate.label.as_str(), "Function" | "Method"));
    let priority = candidates.iter().map(|node| label_priority(parse_label(&node.candidate.label))).max();
    candidates.retain(|node| Some(label_priority(parse_label(&node.candidate.label))) == priority);
    candidates.sort_by(|a, b| a.candidate.qualified_name.cmp(&b.candidate.qualified_name));
    Ok(candidates)
}

fn placeholders(len: usize) -> String { vec!["?"; len].join(",") }

/// 分层、分批走有索引的邻接查询，仅记录可达节点，不创建全库节点/边对象。
fn bfs(conn: &Connection, starts: &[i64], inbound: bool, depth: u32, max_nodes: usize) -> Result<Vec<TraceHop>> {
    let mut visited: HashSet<i64> = starts.iter().copied().collect();
    let mut frontier = starts.to_vec();
    let mut hops = Vec::new();
    let (source, target) = if inbound { ("target_id", "source_id") } else { ("source_id", "target_id") };
    for hop in 1..=depth {
        if frontier.is_empty() || hops.len() >= max_nodes { break; }
        let mut next = Vec::new();
        for batch in frontier.chunks(256) {
            let sql = format!("SELECT n.id,n.label,n.name,n.qualified_name,n.file_path,n.start_line,n.end_line,''
                FROM edges e JOIN nodes n ON n.id=e.{target} WHERE e.type='CALLS' AND e.{source} IN ({}) ORDER BY n.id", placeholders(batch.len()));
            let args: Vec<_> = batch.iter().copied().map(Value::Integer).collect();
            for node in nodes(conn, &sql, &args)? {
                if !visited.insert(node.id) { continue; }
                let candidate = node.candidate;
                next.push(node.id);
                hops.push(TraceHop { name: candidate.name, qualified_name: candidate.qualified_name,
                    file_path: candidate.file_path, hop, risk: risk_for_hop(hop) });
            }
        }
        frontier = next;
    }
    hops.sort_by(|a, b| a.hop.cmp(&b.hop).then(a.qualified_name.cmp(&b.qualified_name)));
    hops.truncate(max_nodes);
    Ok(hops)
}

pub(super) fn trace_calls_page(path: &Path, name: &str, direction: TraceDirection, depth: u32, limit: usize, offset: usize) -> Result<TraceOutcome> {
    read(path, |store| {
        let mut candidates = resolve(&store.conn, name, true)?;
        if candidates.is_empty() { return Ok(TraceOutcome::NotFound); }
        if candidates.len() > 1 { return Ok(TraceOutcome::Ambiguous(candidates.into_iter().map(|node| node.candidate).collect())); }
        let node = candidates.pop().unwrap();
        let callers = if matches!(direction, TraceDirection::Inbound | TraceDirection::Both) { bfs(&store.conn, &[node.id], true, depth, usize::MAX)? } else { Vec::new() };
        let callees = if matches!(direction, TraceDirection::Outbound | TraceDirection::Both) { bfs(&store.conn, &[node.id], false, depth, usize::MAX)? } else { Vec::new() };
        let callers_total = callers.len();
        let callees_total = callees.len();
        Ok(TraceOutcome::Found(TraceResult { function: node.candidate.name, qualified_name: node.candidate.qualified_name,
            direction, callers_total, callees_total, offset,
            has_more: offset.saturating_add(limit) < callers_total.max(callees_total),
            callers: callers.into_iter().skip(offset).take(limit).collect(), callees: callees.into_iter().skip(offset).take(limit).collect() }))
    })
}

pub(super) fn symbol_detail(path: &Path, root: Option<&Path>, name: &str) -> Result<DetailOutcome> {
    read(path, |store| {
        let mut candidates = resolve(&store.conn, name, false)?;
        if candidates.is_empty() { return Ok(DetailOutcome::NotFound); }
        if candidates.len() > 1 { return Ok(DetailOutcome::Ambiguous(candidates.into_iter().map(|node| node.candidate).collect())); }
        let node = candidates.pop().unwrap();
        let candidate = node.candidate;
        let hash: Option<(u64, u64)> = store.conn.query_row("SELECT mtime_ns,size FROM file_hashes WHERE rel_path=?1", params![candidate.file_path],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64))).optional()
            .map_err(|error| err(format!("读取源文件元数据失败：{error}")))?;
        let freshness = root.map(|root| super::coverage::source_freshness(hash.as_ref(), root, &candidate.file_path)).unwrap_or("snapshot_or_unknown");
        let source = if freshness == "metadata_matches" { root.and_then(|root| {
            let snippet = read_source_snippet(root, &candidate);
            (super::coverage::source_freshness(hash.as_ref(), root, &candidate.file_path) == "metadata_matches").then_some(snippet).flatten()
        }) } else { None };
        let properties: serde_json::Value = serde_json::from_str(&node.properties).unwrap_or_default();
        Ok(DetailOutcome::Found(Box::new(SymbolDetail { name: candidate.name, label: candidate.label,
            qualified_name: candidate.qualified_name, file_path: candidate.file_path, start_line: candidate.start_line, end_line: candidate.end_line,
            signature: properties["signature"].as_str().unwrap_or("").to_string(), docstring: properties["docstring"].as_str().unwrap_or("").to_string(),
            source_freshness: freshness.into(), source,
            callers: bfs(&store.conn, &[node.id], true, 1, 100)?, callees: bfs(&store.conn, &[node.id], false, 1, 100)? })))
    })
}

pub(super) fn impacted_symbols_for_files(path: &Path, files: &[String], depth: u32) -> Result<ImpactReport> {
    read(path, |store| {
        let mut impacted = Vec::new();
        let mut seen = HashSet::new();
        for batch in files.chunks(256) {
            let args: Vec<_> = batch.iter().cloned().map(Value::Text).collect();
            let sql = format!("SELECT {COLUMNS} FROM nodes WHERE file_path IN ({}) ORDER BY id", placeholders(batch.len()));
            impacted.extend(nodes(&store.conn, &sql, &args)?.into_iter().filter(|node| parse_label(&node.candidate.label).is_symbol() && seen.insert(node.id)));
        }
        impacted.sort_by_key(|node| node.id);
        let ids: Vec<_> = impacted.iter().map(|node| node.id).collect();
        Ok(ImpactReport { changed_count: files.len(), changed_files: files.to_vec(),
            callers: bfs(&store.conn, &ids, true, depth, 200)?, impacted_symbols: impacted.into_iter().map(|node| node.candidate).collect() })
    })
}

pub(super) fn index_overview(path: &Path) -> Result<IndexOverview> {
    read(path, |store| {
        let conn = &store.conn;
        let stats = store.read_stats()?.unwrap_or_default();
        let counts = |sql: &str| -> Result<Vec<(String, usize)>> {
            let mut stmt = conn.prepare(sql).map_err(|error| err(format!("准备统计查询失败：{error}")))?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as usize))).map_err(|error| err(format!("读取统计失败：{error}")))?
                .collect::<rusqlite::Result<_>>().map_err(|error| err(format!("读取统计失败：{error}")))
        };
        let label_counts = counts("SELECT label,count(*) FROM nodes GROUP BY label ORDER BY count(*) DESC,label")?;
        let edge_counts = counts("SELECT type,count(*) FROM edges GROUP BY type ORDER BY count(*) DESC,type")?;
        // 只读取统计所需的短字段，逐行聚合，不反序列化属性或全部调用边。
        let mut languages = HashMap::<String, usize>::new();
        let mut dirs = HashMap::<String, usize>::new();
        let mut symbols = HashMap::<String, usize>::new();
        {
            let mut stmt = conn.prepare("SELECT label,name,file_path FROM nodes WHERE file_path<>''").map_err(|error| err(format!("读取目录统计失败：{error}")))?;
            let mut rows = stmt.query([]).map_err(|error| err(format!("读取目录统计失败：{error}")))?;
            while let Some(row) = rows.next().map_err(|error| err(format!("读取目录统计失败：{error}")))? {
                let label: String = row.get(0).map_err(|error| err(error.to_string()))?;
                let name: String = row.get(1).map_err(|error| err(error.to_string()))?;
                let file: String = row.get(2).map_err(|error| err(error.to_string()))?;
                let dir = file.split_once('/').map_or("(根目录)", |(dir, _)| dir).to_string();
                if label == "File" {
                    let ext = Path::new(&name).extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).unwrap_or_else(|| "other".into());
                    *languages.entry(ext).or_default() += 1;
                    dirs.entry(dir).or_default();
                } else if parse_label(&label).is_symbol() { *symbols.entry(dir).or_default() += 1; }
            }
        }
        for (dir, count) in &mut dirs { *count = symbols.get(dir).copied().unwrap_or_default(); }
        let sorted = |values: HashMap<String, usize>| { let mut values: Vec<_> = values.into_iter().collect(); values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); values };
        let languages = sorted(languages);
        let mut top_dirs = sorted(dirs); top_dirs.truncate(10);
        let hotspots = {
            let mut stmt = conn.prepare("SELECT n.name,n.qualified_name,n.file_path,count(*) FROM edges e JOIN nodes n ON n.id=e.target_id WHERE e.type='CALLS' GROUP BY e.target_id ORDER BY count(*) DESC,n.qualified_name LIMIT 10")
                .map_err(|error| err(format!("读取调用热点失败：{error}")))?;
            stmt.query_map([], |row| Ok(Hotspot { name: row.get(0)?, qualified_name: row.get(1)?, file_path: row.get(2)?, fan_in: row.get::<_, i64>(3)? as usize }))
                .map_err(|error| err(format!("读取调用热点失败：{error}")))?.collect::<rusqlite::Result<_>>().map_err(|error| err(format!("读取调用热点失败：{error}")))?
        };
        Ok(IndexOverview { total_nodes: stats.nodes, total_edges: stats.edges, files: stats.files, symbols: stats.symbols, calls: stats.calls,
            label_counts, edge_counts, languages, top_dirs, hotspots })
    })
}
