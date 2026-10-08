//! 符号检索：名称优先、文档补充，结构过滤与分页共用同一份匹配集合。

use std::path::Path;

use regex::{Regex, RegexBuilder};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::err;
use super::graph::GraphBuffer;
use super::store::{CodeIndexStore, SearchHit, camel_split, open_read_only_if_exists};
use crate::types::Result;

#[derive(Clone, Debug)]
pub struct SearchOptions<'a> {
    pub query: Option<&'a str>,
    pub label: Option<&'a str>,
    /// 仓库相对路径 glob，支持 *、?、[...]；反斜杠会归一为正斜杠。
    pub file_pattern: Option<&'a str>,
    pub name_pattern: Option<&'a str>,
    pub qn_pattern: Option<&'a str>,
    pub offset: usize,
    pub limit: usize,
}

impl Default for SearchOptions<'_> {
    fn default() -> Self {
        Self {
            query: None,
            label: None,
            file_pattern: None,
            name_pattern: None,
            qn_pattern: None,
            offset: 0,
            limit: 20,
        }
    }
}

pub fn search_symbols_with_options(
    db_path: &Path,
    options: &SearchOptions<'_>,
) -> Result<(Vec<SearchHit>, usize)> {
    let store = open_read_only_if_exists(db_path)?
        .ok_or_else(|| err("代码索引不存在或尚未建立，请先调用 refresh_index"))?;
    store.search_with_options(options)
}

fn pattern_regex(pattern: Option<&str>) -> Result<Option<Regex>> {
    pattern
        .map(|pattern| {
            RegexBuilder::new(pattern)
                .size_limit(1024 * 1024)
                .build()
                .map_err(|e| err(format!("名称正则表达式无效：{e}")))
        })
        .transpose()
}

/// 标识符保留驼峰拆词；标点按 tokenizer 的边界处理，不把用户输入当 FTS 语法。
fn query_tokens(query: &str) -> Vec<String> {
    let split = camel_split(query);
    let mut tokens: Vec<String> = split
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

/// unicode61 不会切分连续汉字；按字建立位置索引，查询仍作为完整短语匹配。
fn index_text(input: &str) -> String {
    let mut text = String::new();
    for ch in camel_split(input).chars() {
        if matches!(ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x20000..=0x323af) {
            if !text.is_empty() && !text.ends_with(' ') {
                text.push(' ');
            }
            text.push(ch);
            text.push(' ');
        } else {
            text.push(ch);
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl CodeIndexStore {
    pub fn search_with_options(
        &self,
        options: &SearchOptions<'_>,
    ) -> Result<(Vec<SearchHit>, usize)> {
        let tx = self.conn.unchecked_transaction().map_err(|e| err(format!("开启搜索快照失败：{e}")))?;
        let result = self.search_page(options, false)?;
        // 先保持多词查询的精度；完全无命中时再放宽，避免自然描述中的附加词吞掉结果。
        let result = if result.1 == 0 && query_tokens(options.query.unwrap_or("")).len() > 1 {
            self.search_page(options, true)
        } else {
            Ok(result)
        }?;
        tx.commit().map_err(|e| err(format!("结束搜索快照失败：{e}")))?;
        Ok(result)
    }

    fn search_page(
        &self,
        options: &SearchOptions<'_>,
        partial: bool,
    ) -> Result<(Vec<SearchHit>, usize)> {
        let name_regex = pattern_regex(options.name_pattern)?;
        let qn_regex = pattern_regex(options.qn_pattern)?;
        let query = options.query.unwrap_or("").trim();
        let tokens = query_tokens(query);
        if options.query.is_some() && tokens.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let terms: Vec<String> = tokens
            .iter()
            .map(|token| format!("\"{}\"*", index_text(token)))
            .collect();
        let match_expr =
            (!terms.is_empty()).then(|| terms.join(if partial { " OR " } else { " AND " }));
        let all_expr = (!terms.is_empty()).then(|| terms.join(" AND "));
        let file_pattern = options
            .file_pattern
            .map(|pattern| pattern.replace('\\', "/"));
        let from = if match_expr.is_some() {
            "FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.rowid WHERE nodes_fts MATCH ?1"
        } else {
            "FROM nodes n WHERE ?1 IS NULL"
        };
        // 默认只查定义符号，避免项目名、目录、文件节点挤占结果；显式标签可查结构节点。
        let filters = "AND ((?2 IS NULL AND n.label IN ('Function','Method','Class','Struct','Interface','Enum','Trait','Type','Field')) OR n.label = ?2)
                       AND (?3 IS NULL OR n.file_path GLOB ?3)";
        let relevance = if match_expr.is_some() {
            "CASE WHEN n.id IN (SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH ?5) THEN 0 ELSE 1 END,
             bm25(nodes_fts, 12.0, 3.0, 0.0, 0.5, 1.0)"
        } else {
            "CASE WHEN ?5 IS NULL THEN 0 ELSE 1 END"
        };
        let mut sql = format!(
            "SELECT n.name, n.label, n.qualified_name, n.file_path, n.start_line, n.end_line, n.properties
             {from} {filters}
             ORDER BY CASE WHEN n.qualified_name = ?4 OR n.name = ?4 THEN 0
                           WHEN lower(n.name) = lower(?4) THEN 1 ELSE 2 END,
                      {relevance},
                      CASE WHEN n.label IN ('Function','Method') THEN 0 WHEN n.label = 'Field' THEN 2 ELSE 1 END,
                      n.qualified_name, n.id"
        );
        let has_regex = name_regex.is_some() || qn_regex.is_some();
        let mut total = 0usize;
        if !has_regex {
            total = self
                .conn
                .query_row(
                    &format!("SELECT count(*) {from} {filters}"),
                    params![match_expr, options.label, file_pattern],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| err(format!("全文检索失败：{e}")))?
                .max(0) as usize;
            sql.push_str(" LIMIT ?6 OFFSET ?7");
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| err(format!("全文检索失败：{e}")))?;
        let mut rows = if has_regex {
            stmt.query(params![
                match_expr,
                options.label,
                file_pattern,
                query,
                all_expr
            ])
        } else {
            stmt.query(params![
                match_expr,
                options.label,
                file_pattern,
                query,
                all_expr,
                options.limit.min(i64::MAX as usize) as i64,
                options.offset.min(i64::MAX as usize) as i64
            ])
        }
        .map_err(|e| err(format!("全文检索失败：{e}")))?;
        let mut hits = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| err(format!("读取搜索结果失败：{e}")))?
        {
            let hit = (|| -> rusqlite::Result<SearchHit> {
                let properties: String = row.get(6)?;
                let properties: Value = serde_json::from_str(&properties).unwrap_or_default();
                Ok(SearchHit {
                    name: row.get(0)?,
                    label: row.get(1)?,
                    qualified_name: row.get(2)?,
                    file_path: row.get(3)?,
                    start_line: row.get::<_, i64>(4)?.max(0) as u32,
                    end_line: row.get::<_, i64>(5)?.max(0) as u32,
                    signature: properties["signature"].as_str().unwrap_or("").to_string(),
                    docstring: properties["docstring"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(400)
                        .collect(),
                })
            })()
            .map_err(|e| err(format!("读取搜索结果失败：{e}")))?;
            if name_regex
                .as_ref()
                .is_some_and(|pattern| !pattern.is_match(&hit.name))
                || qn_regex
                    .as_ref()
                    .is_some_and(|pattern| !pattern.is_match(&hit.qualified_name))
            {
                continue;
            }
            if has_regex {
                if total >= options.offset && hits.len() < options.limit {
                    hits.push(hit);
                }
                total += 1;
            } else {
                hits.push(hit);
            }
        }
        Ok((hits, total))
    }
}

/// 沿用 contentless FTS，额外索引声明和文档；原始属性仍存 nodes 表。
pub(super) fn fill_search_index(conn: &Connection, graph: &GraphBuffer, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<()> {
    let mut stmt = conn.prepare("INSERT INTO nodes_fts (rowid, name, qualified_name, label, file_path, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
        .map_err(|e| err(format!("准备全文索引失败：{e}")))?;
    for node in &graph.nodes {
        if node.id % 512 == 0 && cancel.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed)) { break; }
        let properties: Value = serde_json::from_str(&node.properties).unwrap_or_default();
        let body = format!(
            "{} {}",
            properties["signature"].as_str().unwrap_or(""),
            properties["docstring"].as_str().unwrap_or("")
        );
        stmt.execute(params![
            node.id as i64 + 1,
            index_text(&node.name),
            index_text(&node.qualified_name),
            node.label.as_str(),
            index_text(&node.file_path),
            index_text(&body)
        ])
        .map_err(|e| err(format!("写入全文索引失败：{e}")))?;
    }
    Ok(())
}

/// 迁移在同一事务快照内读取节点，保留真实 rowid（不能使用内存图的紧凑编号）。
pub(super) fn fill_stored_search_index(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare("SELECT id, name, qualified_name, label, file_path, properties FROM nodes")
        .map_err(|e| err(format!("读取迁移节点失败：{e}")))?;
    let mut rows = select
        .query([])
        .map_err(|e| err(format!("读取迁移节点失败：{e}")))?;
    let mut insert = conn.prepare("INSERT INTO nodes_fts (rowid, name, qualified_name, label, file_path, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
        .map_err(|e| err(format!("准备全文索引迁移失败：{e}")))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| err(format!("读取迁移节点失败：{e}")))?
    {
        let values = (|| -> rusqlite::Result<_> {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })()
        .map_err(|e| err(format!("读取迁移节点失败：{e}")))?;
        let properties: Value = serde_json::from_str(&values.5).unwrap_or_default();
        let body = format!(
            "{} {}",
            properties["signature"].as_str().unwrap_or(""),
            properties["docstring"].as_str().unwrap_or("")
        );
        insert
            .execute(params![
                values.0,
                index_text(&values.1),
                index_text(&values.2),
                values.3,
                index_text(&values.4),
                index_text(&body)
            ])
            .map_err(|e| err(format!("迁移全文索引失败：{e}")))?;
    }
    Ok(())
}

/// 单节点增量更新也使用与全量导入一致的分词和文档字段。
pub(super) fn insert_search_node(conn: &Connection, node: &super::graph::GraphNode, id: i64) -> Result<()> {
    let properties: Value = serde_json::from_str(&node.properties).unwrap_or_default();
    let body = format!("{} {}", properties["signature"].as_str().unwrap_or(""), properties["docstring"].as_str().unwrap_or(""));
    conn.prepare_cached("INSERT INTO nodes_fts(rowid,name,qualified_name,label,file_path,body) VALUES(?1,?2,?3,?4,?5,?6)")
        .and_then(|mut stmt| stmt.execute(params![id, index_text(&node.name), index_text(&node.qualified_name), node.label.as_str(), index_text(&node.file_path), index_text(&body)]))
        .map_err(|error| err(format!("更新全文索引失败：{error}")))?;
    Ok(())
}

pub(super) fn delete_file_search_index(conn: &Connection, path: &str) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id FROM nodes WHERE file_path=?1").map_err(|error| err(format!("读取旧搜索节点失败：{error}")))?;
    let ids: Vec<i64> = stmt.query_map(params![path], |row| row.get(0)).map_err(|error| err(format!("读取旧搜索节点失败：{error}")))?
        .collect::<rusqlite::Result<_>>().map_err(|error| err(format!("读取旧搜索节点失败：{error}")))?;
    for id in ids { delete_search_node(conn, id)?; }
    Ok(())
}

pub(super) fn delete_search_node(conn: &Connection, id: i64) -> Result<()> {
    let (name, qn, label, path, properties): (String, String, String, String, String) = conn.query_row(
        "SELECT name,qualified_name,label,file_path,properties FROM nodes WHERE id=?1", params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .map_err(|error| err(format!("读取旧搜索内容失败：{error}")))?;
    let properties: Value = serde_json::from_str(&properties).unwrap_or_default();
    let body = format!("{} {}", properties["signature"].as_str().unwrap_or(""), properties["docstring"].as_str().unwrap_or(""));
    conn.prepare_cached("INSERT INTO nodes_fts(nodes_fts,rowid,name,qualified_name,label,file_path,body) VALUES('delete',?1,?2,?3,?4,?5,?6)")
        .and_then(|mut stmt| stmt.execute(params![id, index_text(&name), index_text(&qn), label, index_text(&path), index_text(&body)]))
        .map_err(|error| err(format!("删除旧全文索引失败：{error}")))?;
    Ok(())
}
