//! 评审专用索引工具：所有位置、调用关系和源码都来自固定的目标提交。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::code_index::{self, DetailOutcome, SearchOptions, TraceDirection, TraceOutcome};
use crate::types::{GitError, Result};

pub(super) struct ReviewIndex {
    root: PathBuf,
    commit: String,
    db_path: Option<PathBuf>,
    data_dir: Option<PathBuf>,
}

impl ReviewIndex {
    pub fn new(root: PathBuf, commit: String) -> Self {
        Self {
            root,
            commit,
            db_path: None,
            data_dir: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(root: PathBuf, commit: String, data_dir: PathBuf) -> Self {
        Self {
            root,
            commit,
            db_path: None,
            data_dir: Some(data_dir),
        }
    }

    fn ensure(&mut self, cancelled: &AtomicBool) -> Result<PathBuf> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(GitError::Message("评审已取消".into()));
        }
        let path = match &self.db_path {
            Some(path) => path.clone(),
            None => {
                let data = self
                    .data_dir
                    .clone()
                    .or_else(crate::storage::active_data_dir)
                    .ok_or_else(|| GitError::Message("数据目录不可用，无法建立评审索引".into()))?;
                let path = code_index::commit_index_path(&data, &self.root, &self.commit)?;
                self.db_path = Some(path.clone());
                path
            }
        };
        if !code_index::ensure_commit_index(&self.root, &self.commit, &path, cancelled)? {
            return Err(GitError::Message("评审已取消".into()));
        }
        Ok(path)
    }

    pub fn dispatch(
        &mut self,
        repo: &git2::Repository,
        name: &str,
        arguments: &str,
        cancelled: &AtomicBool,
    ) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)
            .map_err(|error| GitError::Message(format!("参数解析失败：{error}")))?;
        let db = self.ensure(cancelled)?;
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = (args["limit"].as_u64().unwrap_or(15) as usize).clamp(1, 50);
        let required = |key: &str| {
            args[key]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| GitError::Message(format!("缺少参数 {key}")))
        };
        let mut value = match name {
            "search_symbols" => {
                let (hits, total) = code_index::search_symbols_with_options(
                    &db,
                    &SearchOptions {
                        query: Some(required("query")?),
                        label: args["label"].as_str(),
                        file_pattern: args["file_pattern"].as_str(),
                        name_pattern: args["name_pattern"].as_str(),
                        qn_pattern: None,
                        offset,
                        limit,
                    },
                )?;
                json!({ "results": hits, "total": total, "offset": offset })
            }
            "get_symbol_detail" => match code_index::symbol_detail(&db, None, required("name")?)? {
                DetailOutcome::Found(detail) => {
                    let mut value = serde_json::to_value(&*detail)
                        .map_err(|error| GitError::Message(error.to_string()))?;
                    value.as_object_mut().unwrap().remove("callers");
                    value.as_object_mut().unwrap().remove("callees");
                    let oid = git2::Oid::from_str(&self.commit)?;
                    let tree = repo.find_commit(oid)?.tree()?;
                    let entry = tree.get_path(std::path::Path::new(&detail.file_path))?;
                    let blob = repo.find_blob(entry.id())?;
                    let text = String::from_utf8_lossy(blob.content());
                    let source: Vec<_> = text
                        .lines()
                        .skip(detail.start_line.saturating_sub(1) as usize)
                        .take(
                            (detail.end_line.saturating_sub(detail.start_line) as usize + 1)
                                .min(200),
                        )
                        .map(str::to_string)
                        .collect();
                    value["source"] = json!({ "start_line": detail.start_line, "lines": source,
                        "truncated": detail.end_line.saturating_sub(detail.start_line) >= 200 });
                    value["source_freshness"] = json!("commit_snapshot");
                    value
                }
                DetailOutcome::Ambiguous(candidates) => {
                    json!({ "status": "ambiguous", "suggestions": candidates })
                }
                DetailOutcome::NotFound => {
                    json!({ "status": "not_found", "hint": "先用 search_symbols 取得目标提交中的 qualified_name" })
                }
            },
            "trace_path" => match code_index::trace_calls_page(
                &db,
                required("function_name")?,
                TraceDirection::parse(args["direction"].as_str().unwrap_or("both")),
                (args["depth"].as_u64().unwrap_or(2) as u32).clamp(1, 8),
                limit,
                offset,
            )? {
                TraceOutcome::Found(result) => serde_json::to_value(result)
                    .map_err(|error| GitError::Message(error.to_string()))?,
                TraceOutcome::Ambiguous(candidates) => {
                    json!({ "status": "ambiguous", "suggestions": candidates })
                }
                TraceOutcome::NotFound => {
                    json!({ "status": "not_found", "hint": "先用 search_symbols 确认限定名" })
                }
            },
            "check_index_coverage" => {
                let strings = |key: &str| -> Result<Vec<String>> {
                    match args.get(key) {
                        None => Ok(Vec::new()),
                        Some(value) => value
                            .as_array()
                            .ok_or_else(|| GitError::Message(format!("{key} 必须是字符串数组")))?
                            .iter()
                            .map(|item| {
                                item.as_str().map(str::to_string).ok_or_else(|| {
                                    GitError::Message(format!("{key} 必须是字符串数组"))
                                })
                            })
                            .collect(),
                    }
                };
                code_index::check_coverage(
                    &db,
                    None,
                    &strings("paths")?,
                    &strings("scopes")?,
                    offset,
                    limit,
                )?
            }
            _ => return Err(GitError::Message(format!("未知索引工具 {name}"))),
        };
        value["target_commit"] = json!(self.commit);
        value["coverage_note"] = json!(
            "索引与源码均对应 target_commit；解析和调用关系是尽力推断。部分覆盖时回读目标提交源码，不能把空结果当作不存在。"
        );
        bounded_result(value)
    }
}

pub(super) fn tool_schemas() -> Vec<super::client::ToolSchema> {
    use super::client::ToolSchema;
    vec![
        ToolSchema {
            name: "search_symbols",
            description: "在评审目标提交中检索定义、签名和文档。名称精确匹配优先；支持多词、路径 glob、名称正则及标签过滤。返回 qualified_name 和位置，后续用 get_symbol_detail / trace_path。has_more 时传 offset=next_offset 继续翻页，保持条件不变。首次查询按需建立该提交的独立索引。",
            parameters: json!({ "type": "object", "properties": {
                "query": { "type": "string" }, "label": { "type": "string" }, "file_pattern": { "type": "string" },
                "name_pattern": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 50 }, "offset": { "type": "integer", "minimum": 0 }
            }, "required": ["query"] }),
        },
        ToolSchema {
            name: "get_symbol_detail",
            description: "读取评审目标提交的定义位置、签名、文档和源码（最多 200 行，超出预算时 source.truncated=true）。name 优先传 search_symbols 所得 qualified_name；同名歧义按 suggestions 重查。源码来自目标提交的 Git blob。调用关系使用 trace_path。",
            parameters: json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
        },
        ToolSchema {
            name: "trace_path",
            description: "在评审目标提交的索引中沿调用边追踪。function_name 优先使用 qualified_name；direction=inbound/outbound/both，默认 both；depth 默认 2，最大 8。两个方向独立分页，has_more 时传 offset=next_offset。启发式边可能遗漏或不确定，不能把无调用结果视为没有调用。",
            parameters: json!({ "type": "object", "properties": {
                "function_name": { "type": "string" }, "direction": { "type": "string", "enum": ["inbound", "outbound", "both"] },
                "depth": { "type": "integer", "minimum": 1, "maximum": 8 }, "limit": { "type": "integer", "minimum": 1, "maximum": 50 }, "offset": { "type": "integer", "minimum": 0 }
            }, "required": ["function_name"] }),
        },
        ToolSchema {
            name: "check_index_coverage",
            description: "核对评审目标提交的索引覆盖。至少传 paths（相对文件路径数组）或 scopes（目录数组，. 为全部）。报告语法错误范围、跳过原因和未解析调用数；部分或缺失覆盖需使用 read_lines/search_code 回读目标提交源码。",
            parameters: json!({ "type": "object", "properties": {
                "paths": { "type": "array", "items": { "type": "string" } }, "scopes": { "type": "array", "items": { "type": "string" } },
                "offset": { "type": "integer", "minimum": 0 }, "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
            }, "anyOf": [{ "required": ["paths"] }, { "required": ["scopes"] }] }),
        },
    ]
}

fn bounded_result(mut value: Value) -> Result<String> {
    // 保留可执行的翻页信息，不能让通用字符截断把 JSON 和限定名截成半条。
    for key in ["signature", "docstring"] {
        if let Some(text) = value[key].as_str() {
            value[key] = json!(text.chars().take(500).collect::<String>());
        }
    }
    loop {
        if let Some(results) = value["results"].as_array() {
            let next = value["offset"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(results.len() as u64);
            let more = next < value["total"].as_u64().unwrap_or(0);
            value["has_more"] = json!(more);
            value["next_offset"] = json!(more.then_some(next));
        } else if value["callers_total"].is_number() {
            let count = value["callers"]
                .as_array()
                .map_or(0, Vec::len)
                .max(value["callees"].as_array().map_or(0, Vec::len));
            let next = value["offset"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(count as u64);
            let total = value["callers_total"]
                .as_u64()
                .unwrap_or(0)
                .max(value["callees_total"].as_u64().unwrap_or(0));
            value["has_more"] = json!(next < total);
            value["next_offset"] = json!((next < total).then_some(next));
        }
        let text =
            serde_json::to_string(&value).map_err(|error| GitError::Message(error.to_string()))?;
        if text.chars().count() <= super::review_agent::MAX_TOOL_RESULT_CHARS {
            return Ok(text);
        }
        if let Some(results) = value["results"]
            .as_array_mut()
            .filter(|results| results.len() > 1)
        {
            results.pop();
            continue;
        }
        let count = value["callers"]
            .as_array()
            .map_or(0, Vec::len)
            .max(value["callees"].as_array().map_or(0, Vec::len));
        if count > 1 {
            for key in ["callers", "callees"] {
                if let Some(items) = value[key].as_array_mut() {
                    items.truncate(count - 1);
                }
            }
            continue;
        }
        if let Some(lines) = value["source"]["lines"]
            .as_array_mut()
            .filter(|lines| !lines.is_empty())
        {
            if lines.len() > 1 {
                lines.pop();
            } else {
                lines[0] = json!(
                    lines[0]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(1000)
                        .collect::<String>()
                );
            }
            value["source"]["truncated"] = json!(true);
            if value["source"]["lines"][0]
                .as_str()
                .is_some_and(|line| line.chars().count() <= 1000)
                && count <= 1
            {
                // 再次判断总量；超长身份信息不截断，直接明确报错。
                let text = serde_json::to_string(&value)
                    .map_err(|error| GitError::Message(error.to_string()))?;
                if text.chars().count() <= super::review_agent::MAX_TOOL_RESULT_CHARS {
                    return Ok(text);
                }
            }
            if value["source"]["lines"]
                .as_array()
                .is_some_and(|lines| lines.len() > 1)
            {
                continue;
            }
        }
        return Err(GitError::Message(
            "索引结果超过单次预算，请缩小范围或使用 read_lines 分段读取".into(),
        ));
    }
}

#[cfg(test)]
#[path = "../tests/ai/review_index.rs"]
mod tests;
