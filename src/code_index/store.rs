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

/// 索引库 schema 版本：v2 原位升级全文索引，保留已有图与仓库元数据。
pub const CODE_INDEX_SCHEMA_VERSION: u32 = 3;

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
    pub generation: String,
    pub coverage: serde_json::Value,
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

#[derive(Clone, Debug, serde::Serialize)]
pub struct SearchHit {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub docstring: String,
}

pub struct CodeIndexStore {
    pub(super) conn: Connection,
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
    /// 打开（必要时创建）索引库。已知旧版本原位迁移，其余不兼容版本重建。
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

        let mut store = Self { conn };
        if !exists {
            store.initialize_schema()?;
            return Ok(Some(store));
        }
        // 已存在的库校验 schema 版本；不匹配返回 None 由调用方重建。
        let version = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();
        if version.as_deref() == Some("2") {
            store.migrate_search_schema()?;
            return Ok(Some(store));
        }
        let version_ok = version.as_deref() == Some(CODE_INDEX_SCHEMA_VERSION.to_string().as_str());
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
                  name, qualified_name, label, file_path, body,
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

    pub(super) fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| err(format!("写入索引元信息失败：{e}")))?;
        Ok(())
    }

    fn migrate_search_schema(&mut self) -> Result<()> {
        let tx = self.conn.transaction().map_err(|e| err(format!("开启索引迁移失败：{e}")))?;
        tx.execute_batch("DROP TABLE nodes_fts;
            CREATE VIRTUAL TABLE nodes_fts USING fts5(name, qualified_name, label, file_path, body,
                content='', tokenize='unicode61 remove_diacritics 2');")
            .map_err(|e| err(format!("迁移全文索引失败：{e}")))?;
        super::search::fill_stored_search_index(&tx)?;
        tx.execute("UPDATE meta SET value = ?1 WHERE key = 'schema_version'", params![CODE_INDEX_SCHEMA_VERSION.to_string()])
            .map_err(|e| err(format!("更新索引版本失败：{e}")))?;
        tx.commit().map_err(|e| err(format!("提交索引迁移失败：{e}")))
    }

    /// 旧图尚未采集签名和文档时，下一次刷新补做一次全量提取。
    pub(super) fn has_search_metadata(&self) -> bool {
        self.conn.query_row("SELECT value FROM meta WHERE key = 'search_content_version'", [],
            |row| row.get::<_, String>(0)).is_ok_and(|version| version == "3")
    }

    pub(super) fn retry_files(&self) -> Result<std::collections::HashSet<String>> {
        let mut select = self.conn.prepare("SELECT file_path FROM nodes WHERE label='File' AND json_extract(properties, '$.coverage.status') IN ('read_failed', 'parse_failed')")
            .map_err(|e| err(format!("读取索引重试信息失败：{e}")))?;
        let rows = select.query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| err(format!("读取索引重试信息失败：{e}")))?;
        let mut failed = std::collections::HashSet::new();
        for row in rows {
            failed.insert(row.map_err(|e| err(format!("读取索引重试信息失败：{e}")))?);
        }
        for issue in self.discovery_issues()? {
            if matches!(issue.status.as_str(), "read_failed" | "parse_failed") { failed.insert(issue.path); }
        }
        Ok(failed)
    }

    pub(super) fn discovery_issues(&self) -> Result<Vec<super::coverage::FileCoverage>> {
        use rusqlite::OptionalExtension;
        let properties: Option<String> = self.conn.query_row("SELECT properties FROM nodes WHERE label='Project' LIMIT 1", [], |row| row.get(0))
            .optional().map_err(|error| err(format!("读取发现覆盖信息失败：{error}")))?;
        let Some(properties) = properties else { return Ok(Vec::new()); };
        let properties: serde_json::Value = serde_json::from_str(&properties).map_err(|error| err(format!("发现覆盖信息损坏：{error}")))?;
        properties.get("discovery_issues").cloned().map(serde_json::from_value).transpose()
            .map_err(|error| err(format!("发现覆盖信息损坏：{error}"))).map(|issues| issues.unwrap_or_default())
    }

    /// 全量替换图内容 + 文件哈希表 + 元信息（整库重写语义，全量与增量共用；
    /// 对齐参考项目「增量也整体重写 DB」的做法，保证坏库可自愈）。
    pub fn replace_all(
        &mut self,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        meta: &CodeIndexMeta,
    ) -> Result<()> {
        self.replace_all_cancellable(graph, hashes, meta, None).map(|_| ())
    }

    pub(super) fn replace_all_cancellable(
        &mut self, graph: &GraphBuffer, hashes: &[FileHashRow], meta: &CodeIndexMeta,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<bool> {
        let cancelled = || cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed));
        if cancelled() { return Ok(false); }
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
                if node.id % 512 == 0 && cancelled() { return Ok(false); }
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

        super::search::fill_search_index(&tx, graph)?;
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
        let mut generation_bytes = [0u8; 16];
        getrandom::fill(&mut generation_bytes).map_err(|e| err(format!("生成索引代际失败：{e}")))?;
        let generation: String = generation_bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let coverage = super::coverage::summarize(graph).to_string();
        for (key, value) in [
            ("repo_name", meta.repo_name.as_str()),
            ("repo_path", meta.repo_path.as_str()),
            ("branch", meta.branch.as_str()),
            ("indexed_at", &meta.indexed_at.to_string()),
            ("duration_ms", &meta.duration_ms.to_string()),
            ("mode", meta.mode.as_str()),
            ("search_content_version", "3"),
            ("generation", generation.as_str()),
            ("coverage_summary", coverage.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| err(format!("写入索引元信息失败：{e}")))?;
        }

        if cancelled() { return Ok(false); }
        tx.commit()
            .map_err(|e| err(format!("提交索引事务失败：{e}")))?;
        Ok(true)
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
        let raw_nodes = rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|error| err(format!("读取节点失败：{error}")))?;
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
        for row in rows {
            let (src, tgt, etype, props) = row.map_err(|error| err(format!("读取边失败：{error}")))?;
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
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|error| err(format!("读取文件哈希失败：{error}")))
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
            generation: meta_of("generation"),
            coverage: serde_json::from_str(&meta_of("coverage_summary")).unwrap_or_default(),
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
        self.search_with_options(&super::search::SearchOptions {
            query: Some(query), label, limit, ..Default::default()
        })
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

pub fn index_generation(db_path: &Path) -> Result<Option<String>> {
    let Some(store) = open_read_only_if_exists(db_path)? else { return Ok(None); };
    use rusqlite::OptionalExtension;
    store.conn.query_row("SELECT value FROM meta WHERE key='generation'", [], |row| row.get(0))
        .optional().map_err(|error| err(format!("读取索引代际失败：{error}")))
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
        .map(|v| v == "2" || v == CODE_INDEX_SCHEMA_VERSION.to_string())
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
