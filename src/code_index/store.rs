//! 索引数据库存储（参照 codebase-memory-mcp 的 `store/store.c`）。
//!
//! 每仓库一个独立 SQLite 文件（`<数据目录>/code-index/<repo哈希8>/index.db`），
//! 与主库零关联——索引任务在自己的线程打开连接，不与 AppStorage 的单连接互斥锁
//! 竞争。schema 与参考项目同构（单仓库单库，去掉 project 列）；节点主键不用
//! AUTOINCREMENT：整库重写后行号自然从 1 复用，增量载入的 id 映射保持紧凑。
//! 落盘采用「删辅助索引 → 批量插入 → 重建索引」的 bulk 模式（对齐参考项目
//! `cbm_store_begin_bulk`），FTS5 为 contentless 表、rowid 显式取节点 id。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use super::err;
use super::graph::{EdgeType, GraphBuffer, NodeId};
use crate::types::Result;

/// 索引库 schema 版本：建库时写入 meta，打开时不匹配则整库删除重建。
pub const CODE_INDEX_SCHEMA_VERSION: u32 = 2;

/// 建库路径：`<数据目录>/code-index/<repo哈希8>/index.db`。
/// 目录不存在时创建。
pub fn open_index_db_path(data_dir: &Path, repo_hash8: &str) -> Result<PathBuf> {
    let dir = data_dir.join("code-index").join(repo_hash8);
    std::fs::create_dir_all(&dir).map_err(|e| err(format!("创建索引目录失败：{e}")))?;
    Ok(dir.join("index.db"))
}

/// file_hashes 行（mtime+size 双键，sha256 预留未启用——对齐参考项目实际行为）。
#[derive(Clone, Debug)]
pub struct FileHashRow {
    pub rel_path: String,
    pub mtime_ns: u64,
    pub size: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CodeIndexMeta {
    pub repo_name: String,
    /// 仓库根目录绝对路径（MCP 多仓库模式的 list_projects / 按哈希解析反查用；
    /// 旧库无此键则留空，下次索引落盘时补写）。
    pub repo_path: String,
    pub branch: String,
    /// Unix 毫秒。
    pub indexed_at: u64,
    pub duration_ms: u64,
    pub mode: String,
}

#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    pub files: usize,
    pub symbols: usize,
    pub nodes: usize,
    pub edges: usize,
    pub calls: usize,
    pub db_bytes: u64,
    pub indexed_at: u64,
    pub duration_ms: u64,
    pub branch: String,
    pub mode: String,
    /// 仓库根目录绝对路径（meta 缺失时为空串，见 CodeIndexMeta::repo_path）。
    pub repo_path: String,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
}

pub struct CodeIndexStore {
    conn: Connection,
}

const SYMBOL_LABELS: &[&str] = &[
    "Function",
    "Method",
    "Class",
    "Struct",
    "Interface",
    "Enum",
    "Trait",
    "Type",
    "Field",
];

impl CodeIndexStore {
    /// 打开（必要时创建）索引库。schema 版本不符时删除重建。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(store) = Self::try_open(path)? {
            return Ok(store);
        }
        // 版本不符 / 库损坏：删掉主文件与 WAL 伴生文件后重建。
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{suffix}", path.display()));
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| err(format!("重置索引库失败：{e}")))?;
            }
        }
        Self::try_open(path)?.ok_or_else(|| err("索引库创建失败"))
    }

    fn try_open(path: &Path) -> Result<Option<Self>> {
        let exists = path.exists();
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| err(format!("打开索引库失败：{e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.busy_timeout(std::time::Duration::from_secs(10)).ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();

        let store = Self { conn };
        if !exists {
            store.initialize_schema()?;
            return Ok(Some(store));
        }
        // 已存在的库校验 schema 版本；不匹配返回 None 由调用方重建。
        let version_ok = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(|v| v == CODE_INDEX_SCHEMA_VERSION.to_string())
            .unwrap_or(false);
        Ok(if version_ok { Some(store) } else { None })
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS nodes (
                  id INTEGER PRIMARY KEY,
                  label TEXT NOT NULL,
                  name TEXT NOT NULL,
                  qualified_name TEXT NOT NULL UNIQUE,
                  file_path TEXT DEFAULT '',
                  start_line INTEGER DEFAULT 0,
                  end_line INTEGER DEFAULT 0,
                  properties TEXT DEFAULT '{}'
                );
                CREATE TABLE IF NOT EXISTS edges (
                  id INTEGER PRIMARY KEY,
                  source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                  target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                  type TEXT NOT NULL,
                  properties TEXT DEFAULT '{}',
                  UNIQUE(source_id, target_id, type)
                );
                CREATE TABLE IF NOT EXISTS file_hashes (
                  rel_path TEXT PRIMARY KEY,
                  sha256 TEXT NOT NULL DEFAULT '',
                  mtime_ns INTEGER NOT NULL DEFAULT 0,
                  size INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS meta (
                  key TEXT PRIMARY KEY,
                  value TEXT NOT NULL
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                  name, qualified_name, label, file_path,
                  content='', tokenize='unicode61 remove_diacritics 2'
                );
                "#,
            )
            .map_err(|e| err(format!("初始化索引 schema 失败：{e}")))?;
        self.set_meta(
            "schema_version",
            CODE_INDEX_SCHEMA_VERSION.to_string().as_str(),
        )
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| err(format!("写入索引元信息失败：{e}")))?;
        Ok(())
    }

    /// 全量替换图内容 + 文件哈希表 + 元信息（整库重写语义，全量与增量共用；
    /// 对齐参考项目「增量也整体重写 DB」的做法，保证坏库可自愈）。
    pub fn replace_all(
        &mut self,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        meta: &CodeIndexMeta,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| err(format!("开启索引事务失败：{e}")))?;

        // bulk 模式：先删辅助索引，插完重建。
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_nodes_label;
             DROP INDEX IF EXISTS idx_nodes_name;
             DROP INDEX IF EXISTS idx_nodes_file;
             DROP INDEX IF EXISTS idx_edges_source;
             DROP INDEX IF EXISTS idx_edges_target;",
        )
        .map_err(|e| err(format!("清理索引辅助索引失败：{e}")))?;

        tx.execute("DELETE FROM edges", [])
            .and_then(|_| tx.execute("DELETE FROM nodes", []))
            .and_then(|_| tx.execute("DELETE FROM file_hashes", []))
            .map_err(|e| err(format!("清空旧图失败：{e}")))?;
        // contentless FTS 的整表清空特殊命令。
        tx.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all')", [])
            .map_err(|e| err(format!("清空全文索引失败：{e}")))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO nodes (id, label, name, qualified_name, file_path, start_line, end_line, properties)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(|e| err(format!("准备节点写入失败：{e}")))?;
            for node in &graph.nodes {
                stmt.execute(params![
                    node.id as i64 + 1,
                    node.label.as_str(),
                    node.name,
                    node.qualified_name,
                    node.file_path,
                    node.start_line,
                    node.end_line,
                    node.properties,
                ])
                .map_err(|e| err(format!("写入节点失败：{e}")))?;
            }
        }
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO edges (source_id, target_id, type, properties)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| err(format!("准备边写入失败：{e}")))?;
            for edge in &graph.edges {
                stmt.execute(params![
                    edge.source as i64 + 1,
                    edge.target as i64 + 1,
                    edge.etype.as_str(),
                    edge.properties,
                ])
                .map_err(|e| err(format!("写入边失败：{e}")))?;
            }
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(label);
             CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
             CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
             CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);
             CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);",
        )
        .map_err(|e| err(format!("重建辅助索引失败：{e}")))?;

        {
            let mut stmt = tx
                .prepare("INSERT INTO nodes_fts (rowid, name, qualified_name, label, file_path) VALUES (?1, ?2, ?3, ?4, ?5)")
                .map_err(|e| err(format!("准备全文索引失败：{e}")))?;
            for node in &graph.nodes {
                // 参考项目同款技巧：入库前做 camelCase/snake_case 拆分，
                // unicode61 tokenizer 即可获得驼峰感知检索。
                stmt.execute(params![
                    node.id as i64 + 1,
                    camel_split(&node.name),
                    camel_split(&node.qualified_name),
                    node.label.as_str(),
                    camel_split(&node.file_path),
                ])
                .map_err(|e| err(format!("写入全文索引失败：{e}")))?;
            }
        }
        {
            let mut stmt = tx
                .prepare("INSERT INTO file_hashes (rel_path, mtime_ns, size) VALUES (?1, ?2, ?3)")
                .map_err(|e| err(format!("准备文件哈希写入失败：{e}")))?;
            for h in hashes {
                stmt.execute(params![h.rel_path, h.mtime_ns as i64, h.size as i64,])
                    .map_err(|e| err(format!("写入文件哈希失败：{e}")))?;
            }
        }

        tx.execute("DELETE FROM meta WHERE key != 'schema_version'", [])
            .map_err(|e| err(format!("清理旧元信息失败：{e}")))?;
        for (key, value) in [
            ("repo_name", meta.repo_name.as_str()),
            ("repo_path", meta.repo_path.as_str()),
            ("branch", meta.branch.as_str()),
            ("indexed_at", &meta.indexed_at.to_string()),
            ("duration_ms", &meta.duration_ms.to_string()),
            ("mode", meta.mode.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| err(format!("写入索引元信息失败：{e}")))?;
        }

        tx.commit()
            .map_err(|e| err(format!("提交索引事务失败：{e}")))?;
        Ok(())
    }

    /// 从库载入完整图（增量路径）。数据库行 id 映射回紧凑下标。
    pub fn load_graph(&self) -> Result<GraphBuffer> {
        let mut graph = GraphBuffer::new();
        let mut id_map: HashMap<i64, NodeId> = HashMap::new();

        let mut stmt = self
            .conn
            .prepare("SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties FROM nodes ORDER BY id")
            .map_err(|e| err(format!("读取节点失败：{e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(|e| err(format!("查询节点失败：{e}")))?;
        let raw_nodes: Vec<_> = rows.filter_map(|r| r.ok()).collect();
        for (old_id, label, name, qn, file_path, sl, el, props) in raw_nodes.into_iter() {
            let new_id = graph.upsert_node(
                parse_label(&label),
                name,
                qn,
                file_path,
                sl.max(0) as u32,
                el.max(0) as u32,
                props,
            );
            id_map.insert(old_id, new_id);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT source_id, target_id, type, properties FROM edges")
            .map_err(|e| err(format!("读取边失败：{e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| err(format!("查询边失败：{e}")))?;
        for (src, tgt, etype, props) in rows.filter_map(|r| r.ok()) {
            let (Some(src), Some(tgt)) = (id_map.get(&src), id_map.get(&tgt)) else {
                continue;
            };
            let Some(etype) = parse_edge_type(&etype) else {
                continue;
            };
            graph.add_edge(*src, *tgt, etype, props);
        }
        Ok(graph)
    }

    pub fn load_file_hashes(&self) -> Result<Vec<FileHashRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path, mtime_ns, size FROM file_hashes")
            .map_err(|e| err(format!("读取文件哈希失败：{e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileHashRow {
                    rel_path: row.get(0)?,
                    mtime_ns: row.get::<_, i64>(1)?.max(0) as u64,
                    size: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })
            .map_err(|e| err(format!("查询文件哈希失败：{e}")))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 读统计信息。空库返回 None（从未索引过）。
    pub fn read_stats(&self) -> Result<Option<IndexStats>> {
        let has_nodes: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .map_err(|e| err(format!("统计节点失败：{e}")))?;
        if has_nodes == 0 {
            return Ok(None);
        }
        let symbols: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM nodes WHERE label IN ({})",
                    SYMBOL_LABELS
                        .iter()
                        .map(|l| format!("'{l}'"))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let edges: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap_or(0);
        let calls: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges WHERE type = 'CALLS'", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let files: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM file_hashes", [], |r| r.get(0))
            .unwrap_or(0);
        let meta_of = |key: &str| -> String {
            self.conn
                .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap_or_default()
        };
        Ok(Some(IndexStats {
            files: files as usize,
            symbols: symbols as usize,
            nodes: has_nodes as usize,
            edges: edges as usize,
            calls: calls as usize,
            db_bytes: 0,
            indexed_at: meta_of("indexed_at").parse().unwrap_or(0),
            duration_ms: meta_of("duration_ms").parse().unwrap_or(0),
            branch: meta_of("branch"),
            mode: meta_of("mode"),
            repo_path: meta_of("repo_path"),
        }))
    }

    /// 符号搜索（FTS5 BM25，camelCase 感知）。设置页验证卡与 Phase 2 共用入口。
    pub fn search_symbols(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.search_symbols_filtered(query, None, limit)
            .map(|(hits, _)| hits)
    }

    /// 带标签过滤的符号搜索：过滤在 SQL 内完成（JOIN nodes），返回
    /// (命中列表, 过滤后真实总数)——总数不受 limit 截断影响，供 MCP 的
    /// total/has_more 语义使用。rowid 与 nodes.id 相等（落盘时都按缓冲序号 +1）。
    pub fn search_symbols_filtered(
        &self,
        query: &str,
        label: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<SearchHit>, usize)> {
        let tokens: Vec<String> = camel_split(query)
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        if tokens.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let match_expr = tokens.join(" ");
        let join = "FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.rowid WHERE nodes_fts MATCH ?1";
        let (count_sql, rows_sql) = if label.is_some() {
            (
                format!("SELECT count(*) {join} AND n.label = ?2"),
                format!(
                    "SELECT n.name, n.label, n.qualified_name, n.file_path, n.start_line {join} AND n.label = ?2 ORDER BY bm25(nodes_fts) LIMIT ?3"
                ),
            )
        } else {
            (
                format!("SELECT count(*) {join}"),
                format!(
                    "SELECT n.name, n.label, n.qualified_name, n.file_path, n.start_line {join} ORDER BY bm25(nodes_fts) LIMIT ?2"
                ),
            )
        };

        let total: usize = if let Some(label) = label {
            self.conn
                .query_row(&count_sql, params![match_expr, label], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|e| err(format!("全文检索失败：{e}")))? as usize
        } else {
            self.conn
                .query_row(&count_sql, params![match_expr], |r| r.get::<_, i64>(0))
                .map_err(|e| err(format!("全文检索失败：{e}")))? as usize
        };
        if total == 0 {
            return Ok((Vec::new(), 0));
        }

        let mut stmt = self
            .conn
            .prepare(&rows_sql)
            .map_err(|e| err(format!("全文检索失败：{e}")))?;
        let map_row = |row: &rusqlite::Row| {
            Ok(SearchHit {
                name: row.get(0)?,
                label: row.get(1)?,
                qualified_name: row.get(2)?,
                file_path: row.get(3)?,
                start_line: row.get::<_, i64>(4)?.max(0) as u32,
            })
        };
        let rows = if let Some(label) = label {
            stmt.query_map(params![match_expr, label, limit as i64], map_row)
        } else {
            stmt.query_map(params![match_expr, limit as i64], map_row)
        }
        .map_err(|e| err(format!("全文检索失败：{e}")))?;
        let hits: Vec<SearchHit> = rows.filter_map(|r| r.ok()).collect();
        Ok((hits, total))
    }
}

/// 只读打开索引库读统计（设置页展示用）。库不存在返回 None。
/// WAL 模式下与正在写入的索引任务并发安全（读者不阻塞）。
pub fn read_index_stats(db_path: &Path) -> Result<Option<IndexStats>> {
    if !db_path.exists() {
        return Ok(None);
    }
    let conn = open_read_only(db_path)?;
    let store = CodeIndexStore { conn };
    let mut stats = store.read_stats()?;
    if let Some(s) = stats.as_mut() {
        s.db_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    }
    Ok(stats)
}

/// 只读符号搜索入口（设置页验证卡与全局面板共用；无标签过滤）。
pub fn search_symbols(db_path: &Path, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    search_symbols_filtered(db_path, query, None, limit).map(|(hits, _)| hits)
}

/// 只读符号搜索（标签过滤 + 真实总数；MCP search_symbols 工具专用）。
pub fn search_symbols_filtered(
    db_path: &Path,
    query: &str,
    label: Option<&str>,
    limit: usize,
) -> Result<(Vec<SearchHit>, usize)> {
    if !db_path.exists() {
        return Ok((Vec::new(), 0));
    }
    let conn = open_read_only(db_path)?;
    CodeIndexStore { conn }.search_symbols_filtered(query, label, limit)
}

/// 只读打开一个已存在的索引库（查询层专用）。文件不存在、不是本引擎的
/// 索引库或 schema 版本不符时返回 `Ok(None)`——**绝不创建/重建**（幽灵库
/// 防护，对齐参考项目只读打开语义）；损坏库由 GUI 侧的全量重建路径处理。
pub fn open_read_only_if_exists(db_path: &Path) -> Result<Option<CodeIndexStore>> {
    if !db_path.exists() {
        return Ok(None);
    }
    let conn = open_read_only(db_path)?;
    let store = CodeIndexStore { conn };
    let version_ok = store
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(|v| v == CODE_INDEX_SCHEMA_VERSION.to_string())
        .unwrap_or(false);
    Ok(version_ok.then_some(store))
}

fn open_read_only(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| err(format!("打开索引库失败：{e}")))?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).ok();
    Ok(conn)
}

fn parse_label(text: &str) -> super::graph::NodeLabel {
    match text {
        "Project" => super::graph::NodeLabel::Project,
        "Branch" => super::graph::NodeLabel::Branch,
        "Folder" => super::graph::NodeLabel::Folder,
        "File" => super::graph::NodeLabel::File,
        "Module" => super::graph::NodeLabel::Module,
        "Function" => super::graph::NodeLabel::Function,
        "Method" => super::graph::NodeLabel::Method,
        "Class" => super::graph::NodeLabel::Class,
        "Struct" => super::graph::NodeLabel::Struct,
        "Interface" => super::graph::NodeLabel::Interface,
        "Enum" => super::graph::NodeLabel::Enum,
        "Trait" => super::graph::NodeLabel::Trait,
        "Type" => super::graph::NodeLabel::Type,
        "Field" => super::graph::NodeLabel::Field,
        _ => super::graph::NodeLabel::Function,
    }
}

fn parse_edge_type(text: &str) -> Option<EdgeType> {
    Some(match text {
        "CONTAINS_FOLDER" => EdgeType::ContainsFolder,
        "CONTAINS_FILE" => EdgeType::ContainsFile,
        "HAS_BRANCH" => EdgeType::HasBranch,
        "DEFINES" => EdgeType::Defines,
        "DEFINES_METHOD" => EdgeType::DefinesMethod,
        "CALLS" => EdgeType::Calls,
        "IMPORTS" => EdgeType::Imports,
        "INHERITS" => EdgeType::Inherits,
        "IMPLEMENTS" => EdgeType::Implements,
        _ => return None,
    })
}

/// camelCase / snake_case / 路径分隔符拆分为小写空格分隔的 token
/// （参照参考项目 `cbm_camel_split`：FTS 入库前预拆分，免自定义 tokenizer）。
pub fn camel_split(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 8);
    let mut prev_upper_run = false;
    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, '_' | '-' | '.' | '/' | '\\' | ':' | '#') {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            prev_upper_run = false;
            continue;
        }
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let start_new_word = if c.is_uppercase() {
            let after_lower_or_digit = prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit());
            // 缩写词边界：HTTPServer 的 S（prev 大写、next 小写）
            let acronym_end =
                prev.is_some_and(|p| p.is_uppercase()) && next.is_some_and(|n| n.is_lowercase());
            let boundary = after_lower_or_digit || (prev_upper_run && acronym_end);
            prev_upper_run = true;
            boundary
        } else {
            prev_upper_run = false;
            false
        };
        if start_new_word && !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
