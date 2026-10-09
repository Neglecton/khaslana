//! 索引覆盖证据：区分刻意排除、读取失败、部分语法树和当前文件的新鲜度。

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::err;
use super::graph::{GraphBuffer, NodeLabel};
use super::store::open_read_only_if_exists;
use crate::types::Result;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorRange {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileCoverage {
    pub path: String,
    pub status: String,
    pub reason: String,
    #[serde(default)]
    pub error_ranges: Vec<ErrorRange>,
    #[serde(default)]
    pub error_ranges_truncated: bool,
    #[serde(default)]
    pub unresolved_calls: usize,
    #[serde(default)]
    pub unresolved_no_candidate: usize,
    #[serde(default)]
    pub unresolved_with_candidates: usize,
    #[serde(default)]
    pub call_sites: usize,
}

impl FileCoverage {
    pub(super) fn new(path: &str, status: &str, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: status.into(),
            reason: reason.into(),
            ..Default::default()
        }
    }
}

pub(super) fn summarize(graph: &GraphBuffer) -> Value {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut unresolved = 0;
    let mut no_candidate = 0;
    let mut with_candidates = 0;
    for node in &graph.nodes {
        if node.label != NodeLabel::File {
            continue;
        }
        let properties: Value = serde_json::from_str(&node.properties).unwrap_or_default();
        let status = properties["coverage"]["status"]
            .as_str()
            .unwrap_or("unknown");
        *counts.entry(status.into()).or_default() += 1;
        unresolved += properties["coverage"]["unresolved_calls"]
            .as_u64()
            .unwrap_or(0);
        no_candidate += properties["coverage"]["unresolved_no_candidate"].as_u64().unwrap_or(0);
        with_candidates += properties["coverage"]["unresolved_with_candidates"].as_u64().unwrap_or(0);
    }
    if let Some(project) = graph
        .nodes
        .iter()
        .find(|node| node.label == NodeLabel::Project)
    {
        if let Ok(properties) = serde_json::from_str::<Value>(&project.properties) {
            if let Some(issues) = properties["discovery_issues"].as_array() {
                for issue in issues {
                    *counts
                        .entry(issue["status"].as_str().unwrap_or("unknown").into())
                        .or_default() += 1;
                }
            }
        }
    }
    json!({ "counts": counts, "unresolved_calls": unresolved,
        "unresolved_no_candidate": no_candidate, "unresolved_with_candidates": with_candidates,
        "coverage_note": "未解析调用分为无同语言仓库定义（可能为外部库）和有候选但无法安全确定目标；旧库可能没有分类，均不代表语法解析失败率。统计不是完整性证明。" })
}

pub(super) fn source_freshness(
    stored: Option<&(u64, u64)>,
    root: &Path,
    path: &str,
) -> &'static str {
    let Ok(meta) = std::fs::metadata(root.join(path)) else {
        return "unavailable";
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_nanos() as u64);
    match stored {
        Some((time, size)) if mtime == Some(*time) && *size == meta.len() => "metadata_matches",
        Some(_) => "metadata_changed",
        None => "unknown",
    }
}

pub(super) fn attach_discovery_issues(graph: &mut GraphBuffer, issues: &[FileCoverage]) {
    if let Some(project) = graph
        .nodes
        .iter_mut()
        .find(|node| node.label == NodeLabel::Project)
    {
        let mut properties: Value =
            serde_json::from_str(&project.properties).unwrap_or_else(|_| json!({}));
        properties["discovery_issues"] = json!(issues);
        project.properties = properties.to_string().into();
    }
}

fn valid_path(value: &str) -> bool {
    !value.trim().is_empty()
        && !value.contains(':')
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

pub fn check_coverage(
    db_path: &Path,
    repo_root: Option<&Path>,
    paths: &[String],
    scopes: &[String],
    offset: usize,
    limit: usize,
) -> Result<Value> {
    if paths.is_empty() && scopes.is_empty() {
        return Err(err("至少提供 paths 或 scopes"));
    }
    let paths: Vec<String> = paths.iter().map(|path| path.replace('\\', "/")).collect();
    let scopes: Vec<String> = scopes
        .iter()
        .map(|scope| scope.replace('\\', "/").trim_end_matches('/').to_string())
        .collect();
    if paths.iter().chain(&scopes).any(|path| !valid_path(path)) {
        return Err(err("覆盖检查路径必须是仓库内的相对路径"));
    }
    let store = open_read_only_if_exists(db_path)?.ok_or_else(|| err("代码索引尚未建立"))?;
    let tx = store
        .conn
        .unchecked_transaction()
        .map_err(|e| err(format!("开启覆盖查询失败：{e}")))?;
    let generation: String = tx
        .query_row("SELECT value FROM meta WHERE key='generation'", [], |row| {
            row.get(0)
        })
        .unwrap_or_default();
    let mut records = std::collections::BTreeMap::<String, Value>::new();
    let mut select = tx.prepare("SELECT file_path, json_extract(properties, '$.coverage') FROM nodes WHERE label='File'")
        .map_err(|e| err(format!("读取覆盖信息失败：{e}")))?;
    let mut rows = select
        .query([])
        .map_err(|e| err(format!("读取覆盖信息失败：{e}")))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| err(format!("读取覆盖信息失败：{e}")))?
    {
        let path: String = row.get(0).map_err(|e| err(e.to_string()))?;
        let coverage: Option<String> = row.get(1).map_err(|e| err(e.to_string()))?;
        let coverage = coverage
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| {
                json!({
                    "path": path, "status": "unknown", "reason": "旧索引没有覆盖记录，请刷新索引"
                })
            });
        records.insert(path, coverage);
    }
    drop(rows);
    drop(select);
    let project_properties: String = tx
        .query_row(
            "SELECT properties FROM nodes WHERE label='Project' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();
    if let Ok(properties) = serde_json::from_str::<Value>(&project_properties) {
        if let Some(issues) = properties["discovery_issues"].as_array() {
            for issue in issues {
                if let Some(path) = issue["path"].as_str() {
                    records.insert(path.to_string(), issue.clone());
                }
            }
        }
    }
    for path in &paths {
        if !records.contains_key(path) {
            let excluded = records
                .iter()
                .find(|(prefix, record)| {
                    record["status"] == "excluded" && path.starts_with(&format!("{prefix}/"))
                })
                .map(|(_, record)| record.clone());
            records.insert(path.clone(), excluded.map(|mut record| { record["path"] = json!(path); record }).unwrap_or_else(|| json!({
                "path": path, "status": "not_indexed", "reason": "未记录该文件；可能被忽略规则排除或尚未索引，需直接核对源码"
            })));
        }
    }
    let matched: Vec<_> = records
        .into_iter()
        .filter(|(path, _)| {
            paths.contains(path)
                || scopes.iter().any(|scope| {
                    scope == "." || scope == path || path.starts_with(&format!("{scope}/"))
                })
        })
        .collect();
    let total = matched.len();
    let mut returned = Vec::new();
    // 只核对当前页的磁盘元数据，避免目录翻页时反复 stat 全仓库。
    for (path, mut record) in matched.into_iter().skip(offset).take(limit) {
        let freshness = if let Some(root) = repo_root {
            match std::fs::metadata(root.join(&path)) {
                Ok(meta) if meta.is_file() => {
                    let stored: Option<(i64, i64)> = tx
                        .query_row(
                            "SELECT mtime_ns, size FROM file_hashes WHERE rel_path=?1",
                            [&path],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .ok();
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_nanos() as u64);
                    match stored {
                        Some((time, size))
                            if mtime == Some(time as u64) && size as u64 == meta.len() =>
                        {
                            "metadata_matches"
                        }
                        Some(_) => "metadata_changed",
                        None => "unknown",
                    }
                }
                Ok(_) => "directory",
                Err(_) => "unavailable",
            }
        } else {
            "snapshot_or_unknown"
        };
        record["freshness"] = json!(freshness);
        returned.push(record);
    }
    tx.commit()
        .map_err(|e| err(format!("结束覆盖查询失败：{e}")))?;
    let next = offset.saturating_add(returned.len());
    Ok(
        json!({ "generation": generation, "total": total, "offset": offset, "results": returned,
        "has_more": next < total, "next_offset": (next < total).then_some(next),
        "coverage_note": "metadata_matches 只证明时间和大小一致。unresolved_no_candidate 为无同语言仓库定义，unresolved_with_candidates 为有候选但目标不确定；旧库可能缺分类，未解析调用不等于语法解析失败。缺口与过期需回读源码。" }),
    )
}
