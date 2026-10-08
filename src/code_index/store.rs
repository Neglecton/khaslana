//! 索引数据库存储（参照 codebase-memory-mcp 的 `store/store.c`）。
//!
//! 每仓库一个独立 SQLite 文件（`<数据目录>/code-index/<repo哈希8>/index.db`），
//! 与主库零关联——索引任务在自己的线程打开连接，不与 AppStorage 的单连接互斥锁
//! 竞争。schema 与参考项目同构（单仓库单库，去掉 project 列）；节点主键不用
//! AUTOINCREMENT：全量写入使用紧凑编号，增量保持未修改数据库行的主键。
//! 全量采用「删辅助索引 → 批量插入 → 重建索引」的 bulk 模式（对齐参考项目
//! `cbm_store_begin_bulk`），FTS5 为 contentless 表、rowid 显式取节点 id。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use super::err;
use super::graph::{EdgeType, GraphBuffer, NodeId};
use crate::types::Result;

/// v4 增加独立调用记录表；旧库保持可查询，下一次刷新补建调用暂存。
pub const CODE_INDEX_SCHEMA_VERSION: u32 = 4;

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
        conn.pragma_update(None, "cache_size", -32768).ok();
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
        if version.as_deref() == Some("3") {
            store.conn.execute_batch("CREATE TABLE IF NOT EXISTS call_records (file_path TEXT PRIMARY KEY, data BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS call_names (name TEXT NOT NULL, file_path TEXT NOT NULL REFERENCES call_records(file_path) ON DELETE CASCADE, PRIMARY KEY(name,file_path)) WITHOUT ROWID;
                CREATE INDEX IF NOT EXISTS idx_call_names_file ON call_names(file_path);")
                .map_err(|error| err(format!("迁移调用记录表失败：{error}")))?;
            store.set_meta("schema_version", "4")?;
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
                CREATE TABLE IF NOT EXISTS call_records (file_path TEXT PRIMARY KEY, data BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS call_names (name TEXT NOT NULL, file_path TEXT NOT NULL REFERENCES call_records(file_path) ON DELETE CASCADE, PRIMARY KEY(name,file_path)) WITHOUT ROWID;
                CREATE INDEX IF NOT EXISTS idx_call_names_file ON call_names(file_path);
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
                content='', tokenize='unicode61 remove_diacritics 2');
            CREATE TABLE IF NOT EXISTS call_records (file_path TEXT PRIMARY KEY, data BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS call_names (name TEXT NOT NULL, file_path TEXT NOT NULL REFERENCES call_records(file_path) ON DELETE CASCADE, PRIMARY KEY(name,file_path)) WITHOUT ROWID;
                CREATE INDEX IF NOT EXISTS idx_call_names_file ON call_names(file_path);")
            .map_err(|e| err(format!("迁移全文索引失败：{e}")))?;
        super::search::fill_stored_search_index(&tx)?;
        tx.execute("UPDATE meta SET value = ?1 WHERE key = 'schema_version'", params![CODE_INDEX_SCHEMA_VERSION.to_string()])
            .map_err(|e| err(format!("更新索引版本失败：{e}")))?;
        tx.commit().map_err(|e| err(format!("提交索引迁移失败：{e}")))
    }

    /// 旧图缺少文档或独立调用暂存时，下一次刷新补做一次全量提取。
    pub(super) fn has_search_metadata(&self) -> bool {
        self.conn.query_row("SELECT value FROM meta WHERE key = 'search_content_version'", [],
            |row| row.get::<_, String>(0)).is_ok_and(|version| version == "4")
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

    /// 全量替换图内容、调用记录和元信息；增量刷新使用按文件事务更新。
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

        // 每批只构造有界参数数组，减少 SQLite 的 prepare/step 次数。
        for batch in graph.nodes.chunks(256) {
            if cancelled() { return Ok(false); }
            let sql = format!("INSERT INTO nodes(id,label,name,qualified_name,file_path,start_line,end_line,properties) VALUES {}",
                vec!["(?,?,?,?,?,?,?,?)"; batch.len()].join(","));
            let args: Vec<rusqlite::types::Value> = batch.iter().flat_map(|node| [
                (node.id as i64 + 1).into(), node.label.as_str().to_string().into(), node.name.clone().into(),
                node.qualified_name.to_string().into(), node.file_path.to_string().into(),
                (node.start_line as i64).into(), (node.end_line as i64).into(), node.properties.to_string().into(),
            ]).collect();
            tx.prepare_cached(&sql).and_then(|mut stmt| stmt.execute(rusqlite::params_from_iter(args)))
                .map_err(|error| err(format!("批量写入节点失败：{error}")))?;
        }
        for batch in graph.edges.chunks(512) {
            if cancelled() { return Ok(false); }
            let sql = format!("INSERT INTO edges(source_id,target_id,type,properties) VALUES {}", vec!["(?,?,?,?)"; batch.len()].join(","));
            let args: Vec<rusqlite::types::Value> = batch.iter().flat_map(|edge| [
                (edge.source as i64 + 1).into(), (edge.target as i64 + 1).into(), edge.etype.as_str().to_string().into(), edge.properties.to_string().into(),
            ]).collect();
            tx.prepare_cached(&sql).and_then(|mut stmt| stmt.execute(rusqlite::params_from_iter(args)))
                .map_err(|error| err(format!("批量写入关系失败：{error}")))?;
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(label);
             CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
             CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
             CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);
             CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);",
        )
        .map_err(|e| err(format!("重建辅助索引失败：{e}")))?;

        super::search::fill_search_index(&tx, graph, cancel)?;
        tx.execute("DELETE FROM call_records", []).map_err(|error| err(format!("清理调用记录失败：{error}")))?;
        write_calls(&tx, graph, None, cancel)?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO file_hashes (rel_path, mtime_ns, size) VALUES (?1, ?2, ?3)")
                .map_err(|e| err(format!("准备文件哈希写入失败：{e}")))?;
            for h in hashes {
                stmt.execute(params![h.rel_path, h.mtime_ns as i64, h.size as i64,])
                    .map_err(|e| err(format!("写入文件哈希失败：{e}")))?;
            }
        }

        write_meta(&tx, graph, meta)?;

        if cancelled() { return Ok(false); }
        tx.commit()
            .map_err(|e| err(format!("提交索引事务失败：{e}")))?;
        Ok(true)
    }

    pub(super) fn callers_for_names(&self, names: &std::collections::HashSet<String>) -> Result<std::collections::HashSet<String>> {
        let names: Vec<_> = names.iter().collect();
        let mut paths = std::collections::HashSet::new();
        for batch in names.chunks(256) {
            let sql = format!("SELECT DISTINCT file_path FROM call_names WHERE name IN ({})", vec!["?"; batch.len()].join(","));
            let mut stmt = self.conn.prepare(&sql).map_err(|error| err(format!("定位受影响调用失败：{error}")))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(batch.iter().copied()), |row| row.get::<_, String>(0))
                .map_err(|error| err(format!("定位受影响调用失败：{error}")))?;
            for row in rows { paths.insert(row.map_err(|error| err(format!("读取调用文件失败：{error}")))?); }
        }
        Ok(paths)
    }

    pub(super) fn load_call_records(&self, graph: &mut GraphBuffer, paths: &std::collections::HashSet<String>) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT data FROM call_records WHERE file_path=?1").map_err(|error| err(format!("读取调用记录失败：{error}")))?;
        for path in paths {
            if graph.call_records.contains_key(path.as_str()) { continue; }
            use rusqlite::OptionalExtension;
            let bytes: Option<Vec<u8>> = stmt.query_row(params![path], |row| row.get(0)).optional()
                .map_err(|error| err(format!("读取调用数据失败：{error}")))?;
            if let Some(bytes) = bytes { graph.call_records.insert(path.as_str().into(), super::calls::FileCalls::decode(&bytes)?); }
        }
        Ok(())
    }

    /// 保留未修改行与辅助索引；节点编号通过 QN 映射，不能使用内存紧凑下标作为旧库主键。
    pub(super) fn update_files_cancellable(
        &mut self, graph: &GraphBuffer, hashes: &[FileHashRow], meta: &CodeIndexMeta,
        changed: &std::collections::HashSet<String>, affected: &std::collections::HashSet<String>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<bool> {
        let cancelled = || cancel.load(std::sync::atomic::Ordering::Relaxed);
        if cancelled() { return Ok(false); }
        let tx = self.conn.transaction().map_err(|error| err(format!("开启增量事务失败：{error}")))?;
        for path in changed {
            if cancelled() { return Ok(false); }
            super::search::delete_file_search_index(&tx, path)?;
            tx.execute("DELETE FROM nodes WHERE file_path=?1", params![path]).map_err(|error| err(format!("删除旧文件节点失败：{error}")))?;
            tx.execute("DELETE FROM file_hashes WHERE rel_path=?1", params![path]).map_err(|error| err(format!("删除旧文件元数据失败：{error}")))?;
            tx.execute("DELETE FROM call_records WHERE file_path=?1", params![path]).map_err(|error| err(format!("删除旧调用记录失败：{error}")))?;
        }
        let mut ids = HashMap::<String, i64>::new();
        {
            let mut stmt = tx.prepare("SELECT qualified_name,id FROM nodes").map_err(|error| err(format!("读取节点编号失败：{error}")))?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
                .map_err(|error| err(format!("读取节点编号失败：{error}")))?;
            for row in rows { let (qn, id) = row.map_err(|error| err(format!("读取节点编号失败：{error}")))?; ids.insert(qn, id); }
        }
        let mut node_ids = Vec::with_capacity(graph.nodes.len());
        let mut fresh = std::collections::HashSet::new();
        for node in &graph.nodes {
            if node.id % 512 == 0 && cancelled() { return Ok(false); }
            let id = if let Some(&id) = ids.get(node.qualified_name.as_ref()) {
                if node.label == super::graph::NodeLabel::Project || node.label == super::graph::NodeLabel::Branch
                    || (node.label == super::graph::NodeLabel::File && affected.contains(node.file_path.as_ref())) {
                    tx.execute("UPDATE nodes SET properties=?1 WHERE id=?2 AND properties<>?1", params![node.properties.as_ref(), id])
                        .map_err(|error| err(format!("更新节点覆盖失败：{error}")))?;
                }
                id
            } else {
                tx.execute("INSERT INTO nodes(label,name,qualified_name,file_path,start_line,end_line,properties) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![node.label.as_str(), node.name, node.qualified_name.as_ref(), node.file_path.as_ref(), node.start_line, node.end_line, node.properties.as_ref()])
                    .map_err(|error| err(format!("增量写入节点失败：{error}")))?;
                let id = tx.last_insert_rowid();
                fresh.insert(node.id);
                super::search::insert_search_node(&tx, node, id)?;
                id
            };
            node_ids.push(id);
        }
        // 重解析的调用文件可能仍指向未修改节点，必须先精确删除其旧 CALLS。
        for path in affected {
            tx.execute("DELETE FROM edges WHERE type='CALLS' AND source_id IN (SELECT id FROM nodes WHERE file_path=?1)", params![path])
                .map_err(|error| err(format!("清理受影响调用失败：{error}")))?;
        }
        {
            let mut stmt = tx.prepare("INSERT OR IGNORE INTO edges(source_id,target_id,type,properties) VALUES(?1,?2,?3,?4)")
                .map_err(|error| err(format!("准备增量关系失败：{error}")))?;
            for (index, edge) in graph.edges.iter().enumerate() {
                if index % 512 == 0 && cancelled() { return Ok(false); }
                let relevant = fresh.contains(&edge.source) || fresh.contains(&edge.target)
                    || (edge.etype == EdgeType::Calls && affected.contains(graph.get(edge.source).file_path.as_ref()));
                if relevant { stmt.execute(params![node_ids[edge.source as usize], node_ids[edge.target as usize], edge.etype.as_str(), edge.properties.as_ref()])
                    .map_err(|error| err(format!("增量写入关系失败：{error}")))?; }
            }
        }
        // 孤儿模块没有文件路径，需要同步删除；FTS 使用旧内容发出专用删除命令。
        let modules: Vec<(i64, String)> = {
            let mut stmt = tx.prepare("SELECT id,qualified_name FROM nodes WHERE label='Module'").map_err(|error| err(format!("读取旧模块失败：{error}")))?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|error| err(format!("读取旧模块失败：{error}")))?
                .collect::<rusqlite::Result<_>>().map_err(|error| err(format!("读取旧模块失败：{error}")))?
        };
        let live: std::collections::HashSet<_> = graph.nodes.iter().filter(|node| node.label == super::graph::NodeLabel::Module).map(|node| node.qualified_name.as_ref()).collect();
        for (id, qn) in modules { if !live.contains(qn.as_str()) {
            super::search::delete_search_node(&tx, id)?;
            tx.execute("DELETE FROM nodes WHERE id=?1", params![id]).map_err(|error| err(format!("删除孤儿模块失败：{error}")))?;
        } }
        write_calls(&tx, graph, Some(changed), Some(cancel))?;
        {
            let mut stmt = tx.prepare("INSERT INTO file_hashes(rel_path,mtime_ns,size) VALUES(?1,?2,?3)")
                .map_err(|error| err(format!("准备增量文件元数据失败：{error}")))?;
            for hash in hashes.iter().filter(|hash| changed.contains(&hash.rel_path)) {
                stmt.execute(params![hash.rel_path, hash.mtime_ns as i64, hash.size as i64]).map_err(|error| err(format!("写入增量文件元数据失败：{error}")))?;
            }
        }
        write_meta(&tx, graph, meta)?;
        if cancelled() { return Ok(false); }
        tx.commit().map_err(|error| err(format!("提交增量事务失败：{error}")))?;
        Ok(true)
    }

    /// 从库载入完整图（增量路径）。数据库行 id 映射回紧凑下标。
    pub fn load_graph(&self) -> Result<GraphBuffer> { self.load_graph_inner(true) }

    /// 增量路径保留数据库中的无关 CALLS，仅加载结构和类型关系。
    pub(super) fn load_graph_without_calls(&self) -> Result<GraphBuffer> { self.load_graph_inner(false) }

    fn load_graph_inner(&self, calls: bool) -> Result<GraphBuffer> {
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
        for row in rows {
            let (old_id, label, name, qn, file_path, sl, el, props) = row.map_err(|error| err(format!("读取节点失败：{error}")))?;
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
            .prepare(if calls { "SELECT source_id, target_id, type, properties FROM edges" } else { "SELECT source_id, target_id, type, properties FROM edges WHERE type<>'CALLS'" })
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
    /// total/has_more 语义使用。rowid 与数据库 nodes.id 相等，增量更新沿用真实主键。
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
        .map(|v| (v == "2" || v == "3") || v == CODE_INDEX_SCHEMA_VERSION.to_string())
        .unwrap_or(false);
    Ok(version_ok.then_some(store))
}

fn open_read_only(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| err(format!("打开索引库失败：{e}")))?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).ok();
    Ok(conn)
}

pub(super) fn parse_label(text: &str) -> super::graph::NodeLabel {
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

fn write_meta(tx: &Connection, graph: &GraphBuffer, meta: &CodeIndexMeta) -> Result<()> {
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
        ("search_content_version", "4"),
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

    Ok(())
}

fn write_calls(conn: &Connection, graph: &GraphBuffer, paths: Option<&std::collections::HashSet<String>>,
    cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<()> {
    let mut stmt = conn.prepare("INSERT INTO call_records(file_path,data) VALUES(?1,?2) ON CONFLICT(file_path) DO UPDATE SET data=excluded.data")
        .map_err(|error| err(format!("准备调用记录失败：{error}")))?;
    for (path, records) in &graph.call_records {
        if cancel.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed)) { break; }
        if paths.is_some_and(|paths| !paths.contains(path.as_ref())) { continue; }
        stmt.execute(params![path.as_ref(), records.encode()?]).map_err(|error| err(format!("写入调用记录失败：{error}")))?;
        conn.execute("DELETE FROM call_names WHERE file_path=?1", params![path.as_ref()]).map_err(|error| err(format!("清理调用名字失败：{error}")))?;
        let names: std::collections::HashSet<_> = records.calls.iter().map(|call| call.name.as_ref()).collect();
        let names: Vec<_> = names.into_iter().collect();
        for batch in names.chunks(256) {
            let sql = format!("INSERT INTO call_names(name,file_path) VALUES {}", vec!["(?,?)"; batch.len()].join(","));
            let args: Vec<_> = batch.iter().flat_map(|name| [*name, path.as_ref()]).collect();
            conn.prepare_cached(&sql).and_then(|mut stmt| stmt.execute(rusqlite::params_from_iter(args)))
                .map_err(|error| err(format!("写入调用名字失败：{error}")))?;
        }
    }
    Ok(())
}
