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

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use super::err;
use super::facts::{FileFacts, SnapshotFacts};
use super::graph::{EdgeType, GraphBuffer, NodeId};
use super::identity::{
    CoverageSummary, SymbolKeyMaterial, sha256_hex, symbol_key_from_material, symbol_key_material,
};
use crate::ai::review_store::repo_key;
use crate::code_understanding::{
    EntityKey, EvidenceKind, RelationKind, evidence_id_from_parts, relation_id_from_parts,
};
use crate::types::Result;

/// 索引库 schema 版本：建库时写入 meta；不匹配时由调用方显式整库重建。
pub const CODE_INDEX_SCHEMA_VERSION: u32 = 3;

/// 建库路径：`<数据目录>/code-index/<repo哈希8>/index.db`。
/// 目录不存在时创建。
pub fn open_index_db_path(data_dir: &Path, repo_hash8: &str) -> Result<PathBuf> {
    let dir = data_dir.join("code-index").join(repo_hash8);
    std::fs::create_dir_all(&dir).map_err(|e| err(format!("创建索引目录失败：{e}")))?;
    Ok(dir.join("index.db"))
}

/// file_hashes 行（mtime+size 用于增量预筛，sha256 是发布时的真实内容指纹）。
#[derive(Clone, Debug)]
pub struct FileHashRow {
    pub rel_path: String,
    pub sha256: String,
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
    /// 在已经完成内存提取后，将不兼容库原子替换为 v3 快照。任一步失败时
    /// SQLite 回滚到原来的 v2 文件；这里绝不能在提取之前调用。
    pub fn rebuild_incompatible(
        path: &Path,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        meta: &CodeIndexMeta,
    ) -> Result<()> {
        let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| err(format!("打开待重建索引库失败：{e}")))?;
        enable_foreign_keys(&conn)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| err(format!("锁定待重建索引库失败：{e}")))?;
        tx.execute_batch("DROP TRIGGER IF EXISTS search_fts_update; DROP TRIGGER IF EXISTS search_fts_delete; DROP TRIGGER IF EXISTS search_fts_insert; DROP TABLE IF EXISTS search_fts; DROP TABLE IF EXISTS search_documents; DROP TABLE IF EXISTS diagnostics; DROP TABLE IF EXISTS entry_points; DROP TABLE IF EXISTS relation_evidence; DROP TABLE IF EXISTS evidence; DROP TABLE IF EXISTS relations; DROP TABLE IF EXISTS call_sites; DROP TABLE IF EXISTS symbol_semantics; DROP TABLE IF EXISTS file_facts; DROP TABLE IF EXISTS nodes_fts; DROP TABLE IF EXISTS edges; DROP TABLE IF EXISTS nodes; DROP TABLE IF EXISTS entities; DROP TABLE IF EXISTS file_hashes; DROP TABLE IF EXISTS meta;")
            .map_err(|e| err(format!("清理旧 schema 失败：{e}")))?;
        tx.execute_batch(Self::V3_SCHEMA_SQL)
            .map_err(|e| err(format!("创建 v3 schema 失败：{e}")))?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('schema_version','3'),('generation','0')",
            [],
        )
        .map_err(|e| err(format!("初始化 v3 元信息失败：{e}")))?;
        Self::write_snapshot(&tx, graph, hashes, &SnapshotFacts::default(), meta, 0)?;
        tx.commit()
            .map_err(|e| err(format!("提交索引重建失败：{e}")))
    }

    pub fn rebuild_incompatible_with_facts(
        path: &Path,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        facts: &SnapshotFacts,
        meta: &CodeIndexMeta,
    ) -> Result<()> {
        let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| err(format!("打开待重建索引库失败：{e}")))?;
        enable_foreign_keys(&conn)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| err(format!("锁定待重建索引库失败：{e}")))?;
        tx.execute_batch("DROP TRIGGER IF EXISTS search_fts_update; DROP TRIGGER IF EXISTS search_fts_delete; DROP TRIGGER IF EXISTS search_fts_insert; DROP TABLE IF EXISTS search_fts; DROP TABLE IF EXISTS search_documents; DROP TABLE IF EXISTS diagnostics; DROP TABLE IF EXISTS entry_points; DROP TABLE IF EXISTS relation_evidence; DROP TABLE IF EXISTS evidence; DROP TABLE IF EXISTS relations; DROP TABLE IF EXISTS call_sites; DROP TABLE IF EXISTS symbol_semantics; DROP TABLE IF EXISTS file_facts; DROP TABLE IF EXISTS nodes_fts; DROP TABLE IF EXISTS edges; DROP TABLE IF EXISTS nodes; DROP TABLE IF EXISTS entities; DROP TABLE IF EXISTS file_hashes; DROP TABLE IF EXISTS meta;")
            .map_err(|e| err(format!("清理旧 schema 失败：{e}")))?;
        tx.execute_batch(Self::V3_SCHEMA_SQL)
            .map_err(|e| err(format!("创建 v3 schema 失败：{e}")))?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('schema_version','3'),('generation','0')",
            [],
        )
        .map_err(|e| err(format!("初始化 v3 元信息失败：{e}")))?;
        Self::write_snapshot(&tx, graph, hashes, facts, meta, 0)?;
        tx.commit()
            .map_err(|e| err(format!("提交索引重建失败：{e}")))
    }

    /// 打开（必要时创建）索引库。历史/损坏库绝不自动删除，调用方必须显式重建。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(store) = Self::try_open(path)? {
            return Ok(store);
        }
        Err(err(
            "[NeedsRebuild] 索引 schema 不兼容，需要重建；旧数据未被修改",
        ))
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
        enable_foreign_keys(&conn)?;

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
            .execute_batch(Self::V3_SCHEMA_SQL)
            .map_err(|e| err(format!("初始化索引 schema 失败：{e}")))?;
        self.set_meta(
            "schema_version",
            CODE_INDEX_SCHEMA_VERSION.to_string().as_str(),
        )?;
        self.set_meta("generation", "0")
    }
    const V3_SCHEMA_SQL: &str = r#"
                CREATE TABLE IF NOT EXISTS entities (entity_key TEXT PRIMARY KEY, entity_kind TEXT NOT NULL CHECK(entity_kind IN ('symbol','callsite','entry')), rel_path TEXT NOT NULL DEFAULT '', CHECK((entity_kind='symbol' AND length(entity_key)=68 AND substr(entity_key,1,4)='sk1:') OR (entity_kind='callsite' AND length(entity_key)=68 AND substr(entity_key,1,4)='cs1:') OR (entity_kind='entry' AND length(entity_key)=68 AND substr(entity_key,1,4)='ep1:')), CHECK(substr(entity_key,5) NOT GLOB '*[^0-9a-f]*'));
                CREATE TABLE IF NOT EXISTS nodes (
                  id INTEGER PRIMARY KEY,
                  label TEXT NOT NULL,
                  name TEXT NOT NULL,
                  qualified_name TEXT NOT NULL UNIQUE,
                  file_path TEXT DEFAULT '',
                  start_line INTEGER DEFAULT 0,
                  end_line INTEGER DEFAULT 0,
                  properties TEXT DEFAULT '{}',
                  symbol_key TEXT UNIQUE REFERENCES entities(entity_key) ON DELETE CASCADE,
                  CHECK((label IN ('Project','Branch','Folder','File','Module') AND symbol_key IS NULL) OR (label IN ('Function','Method','Class','Struct','Interface','Enum','Trait','Type','Field') AND symbol_key IS NOT NULL))
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
                  sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
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
                CREATE TABLE IF NOT EXISTS file_facts (rel_path TEXT PRIMARY KEY REFERENCES file_hashes(rel_path) ON DELETE CASCADE, sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'), adapter_versions_json TEXT NOT NULL, parse_status TEXT NOT NULL CHECK(parse_status IN ('ok','partial','error','skipped')), facts_json TEXT NOT NULL, extracted_at INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS symbol_semantics (symbol_key TEXT PRIMARY KEY REFERENCES nodes(symbol_key) ON DELETE CASCADE, module_key TEXT DEFAULT '', source_set TEXT DEFAULT '', package TEXT DEFAULT '', signature TEXT DEFAULT '', kind TEXT NOT NULL, type_metadata_json TEXT DEFAULT '{}', annotations_json TEXT DEFAULT '[]', stability TEXT NOT NULL DEFAULT 'stable');
                CREATE TABLE IF NOT EXISTS call_sites (callsite_key TEXT PRIMARY KEY REFERENCES entities(entity_key) ON DELETE CASCADE, owner_key TEXT NOT NULL REFERENCES nodes(symbol_key) ON DELETE CASCADE, rel_path TEXT NOT NULL REFERENCES file_hashes(rel_path) ON DELETE CASCADE, start_byte INTEGER NOT NULL CHECK(start_byte >= 0), end_byte INTEGER NOT NULL CHECK(end_byte >= start_byte), start_line INTEGER NOT NULL CHECK(start_line >= 1), end_line INTEGER NOT NULL CHECK(end_line >= start_line), content_sha256 TEXT NOT NULL CHECK(length(content_sha256)=64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'), receiver_expr TEXT DEFAULT '', callee_expr TEXT NOT NULL, argument_exprs_json TEXT DEFAULT '[]', resolution TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS relations (relation_id TEXT PRIMARY KEY CHECK(length(relation_id)=67 AND substr(relation_id,1,3)='r1:' AND substr(relation_id,4) NOT GLOB '*[^0-9a-f]*'), source_key TEXT NOT NULL REFERENCES entities(entity_key) ON DELETE CASCADE, target_key TEXT REFERENCES entities(entity_key) ON DELETE CASCADE, kind TEXT NOT NULL CHECK(kind IN ('contains','calls','inherits','implements','overrides','dispatch_candidate','injects_candidate','handles','maps_to')), certainty TEXT NOT NULL CHECK(certainty IN ('syntactic','inferred','unknown')), resolution_state TEXT NOT NULL CHECK(resolution_state IN ('resolved','ambiguous','unresolved','external')), rule_id TEXT, candidate_group TEXT, conditions_json TEXT DEFAULT '[]', heuristic_score REAL, CHECK(target_key IS NOT NULL OR resolution_state IN ('unresolved','external')));
                CREATE TABLE IF NOT EXISTS evidence (evidence_id TEXT PRIMARY KEY CHECK(length(evidence_id)=67 AND substr(evidence_id,1,3)='e1:' AND substr(evidence_id,4) NOT GLOB '*[^0-9a-f]*'), kind TEXT NOT NULL CHECK(kind IN ('declaration','call_site','annotation','sql_mapping','control_flow','module_aggregation')), rule_id TEXT, excerpt_digest TEXT NOT NULL, refs_json TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS relation_evidence (relation_id TEXT NOT NULL REFERENCES relations(relation_id) ON DELETE CASCADE, evidence_id TEXT NOT NULL REFERENCES evidence(evidence_id) ON DELETE CASCADE, PRIMARY KEY(relation_id,evidence_id));
                CREATE TABLE IF NOT EXISTS entry_points (entry_key TEXT PRIMARY KEY REFERENCES entities(entity_key) ON DELETE CASCADE, kind TEXT NOT NULL, symbol_key TEXT REFERENCES nodes(symbol_key) ON DELETE SET NULL, module_key TEXT DEFAULT '', meta_json TEXT DEFAULT '{}', evidence_id TEXT REFERENCES evidence(evidence_id) ON DELETE SET NULL);
                CREATE TABLE IF NOT EXISTS diagnostics (id INTEGER PRIMARY KEY, rel_path TEXT DEFAULT '', adapter TEXT NOT NULL, code TEXT NOT NULL, detail TEXT DEFAULT '', range_json TEXT DEFAULT '{}');
                CREATE TABLE IF NOT EXISTS search_documents (doc_id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL, module_key TEXT DEFAULT '', generation INTEGER NOT NULL);
                CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(title, body, content='search_documents', content_rowid='rowid', tokenize='unicode61 remove_diacritics 2');
                CREATE TRIGGER IF NOT EXISTS search_fts_insert AFTER INSERT ON search_documents BEGIN INSERT INTO search_fts(rowid,title,body) VALUES(new.rowid,new.title,new.body); END;
                CREATE TRIGGER IF NOT EXISTS search_fts_delete AFTER DELETE ON search_documents BEGIN INSERT INTO search_fts(search_fts,rowid,title,body) VALUES('delete',old.rowid,old.title,old.body); END;
                CREATE TRIGGER IF NOT EXISTS search_fts_update AFTER UPDATE ON search_documents BEGIN INSERT INTO search_fts(search_fts,rowid,title,body) VALUES('delete',old.rowid,old.title,old.body); INSERT INTO search_fts(rowid,title,body) VALUES(new.rowid,new.title,new.body); END;
                CREATE INDEX IF NOT EXISTS idx_entities_path_kind ON entities(rel_path,entity_kind); CREATE INDEX IF NOT EXISTS idx_semantics_module ON symbol_semantics(module_key,source_set); CREATE INDEX IF NOT EXISTS idx_callsites_owner ON call_sites(owner_key); CREATE INDEX IF NOT EXISTS idx_callsites_path ON call_sites(rel_path); CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_key,kind,target_key); CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_key,kind,source_key); CREATE INDEX IF NOT EXISTS idx_relations_kind ON relations(kind); CREATE INDEX IF NOT EXISTS idx_entry_kind ON entry_points(kind); CREATE INDEX IF NOT EXISTS idx_diagnostics_path ON diagnostics(rel_path); CREATE INDEX IF NOT EXISTS idx_search_docs_kind ON search_documents(kind);
                "#;

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
    /// 对齐参考项目「增量也整体重写 DB」的做法；schema 损坏库必须显式重建，
    /// 不能借此路径静默修复。
    pub fn replace_all(
        &mut self,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        meta: &CodeIndexMeta,
    ) -> Result<()> {
        let baseline_generation = self.generation()?;
        self.replace_all_at_generation(graph, hashes, meta, baseline_generation)
    }

    /// 发布一个已在内存中完成的批次，并核对提取前捕获的基准代际。
    pub fn replace_all_at_generation(
        &mut self,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        meta: &CodeIndexMeta,
        baseline_generation: u64,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| err(format!("开启索引事务失败：{e}")))?;
        let current_generation: u64 = tx
            .query_row("SELECT value FROM meta WHERE key='generation'", [], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| err(format!("核对索引代际失败：{e}")))?
            .parse()
            .map_err(|_| err("索引代际格式无效"))?;
        if current_generation != baseline_generation {
            return Err(err("[GenerationMismatch] 索引已更新，本次发布已回滚"));
        }
        Self::write_snapshot(
            &tx,
            graph,
            hashes,
            &SnapshotFacts::default(),
            meta,
            current_generation,
        )?;
        tx.commit()
            .map_err(|e| err(format!("提交索引事务失败：{e}")))?;
        Ok(())
    }

    /// DEV-03 的真实事实发布入口。旧 `replace_all*` 仍保留给既有测试和
    /// 兼容调用；新管线必须走本函数，不能回退为 generic 占位事实。
    pub fn replace_all_with_facts_at_generation(
        &mut self,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        facts: &SnapshotFacts,
        meta: &CodeIndexMeta,
        baseline_generation: u64,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| err(format!("开启索引事务失败：{e}")))?;
        let current_generation: u64 = tx
            .query_row("SELECT value FROM meta WHERE key='generation'", [], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| err(format!("核对索引代际失败：{e}")))?
            .parse()
            .map_err(|_| err("索引代际格式无效"))?;
        if current_generation != baseline_generation {
            return Err(err("[GenerationMismatch] 索引已更新，本次发布已回滚"));
        }
        Self::write_snapshot(&tx, graph, hashes, facts, meta, current_generation)?;
        tx.commit()
            .map_err(|e| err(format!("提交索引事务失败：{e}")))
    }

    /// 写入一个完整的发布快照。调用者拥有事务，从而保证重建和普通发布共享
    /// 完全相同的事实→关系→兼容投影顺序。
    fn write_snapshot(
        tx: &rusqlite::Transaction<'_>,
        graph: &GraphBuffer,
        hashes: &[FileHashRow],
        facts: &SnapshotFacts,
        meta: &CodeIndexMeta,
        current_generation: u64,
    ) -> Result<()> {
        // bulk 模式：先删辅助索引，插完重建。
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_nodes_label;
             DROP INDEX IF EXISTS idx_nodes_name;
             DROP INDEX IF EXISTS idx_nodes_file;
             DROP INDEX IF EXISTS idx_edges_source;
             DROP INDEX IF EXISTS idx_edges_target;",
        )
        .map_err(|e| err(format!("清理索引辅助索引失败：{e}")))?;

        tx.execute("DELETE FROM relation_evidence", [])
            .and_then(|_| tx.execute("DELETE FROM relations", []))
            .and_then(|_| tx.execute("DELETE FROM call_sites", []))
            .and_then(|_| tx.execute("DELETE FROM entry_points", []))
            .and_then(|_| tx.execute("DELETE FROM symbol_semantics", []))
            .and_then(|_| tx.execute("DELETE FROM file_facts", []))
            .and_then(|_| tx.execute("DELETE FROM search_documents", []))
            .and_then(|_| tx.execute("DELETE FROM evidence", []))
            .and_then(|_| tx.execute("DELETE FROM edges", []))
            .and_then(|_| tx.execute("DELETE FROM nodes", []))
            .and_then(|_| tx.execute("DELETE FROM entities", []))
            .and_then(|_| tx.execute("DELETE FROM file_hashes", []))
            .map_err(|e| err(format!("清空旧图失败：{e}")))?;
        // contentless FTS 的整表清空特殊命令。
        tx.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all')", [])
            .map_err(|e| err(format!("清空全文索引失败：{e}")))?;

        let mut keys = HashMap::new();
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO nodes (id, label, name, qualified_name, file_path, start_line, end_line, properties, symbol_key)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| err(format!("准备节点写入失败：{e}")))?;
            for node in &graph.nodes {
                let key = node
                    .label
                    .is_symbol()
                    .then(|| stable_symbol_key(node, meta))
                    .transpose()?;
                if let Some(key) = &key {
                    tx.execute("INSERT INTO entities(entity_key, entity_kind, rel_path) VALUES(?1, 'symbol', ?2)", params![key, node.file_path])
                        .map_err(|e| err(format!("写入实体注册表失败：{e}")))?;
                    keys.insert(node.id, key.clone());
                }
                stmt.execute(params![
                    node.id as i64 + 1,
                    node.label.as_str(),
                    node.name,
                    node.qualified_name,
                    node.file_path,
                    node.start_line,
                    node.end_line,
                    node.properties,
                    key,
                ])
                .map_err(|e| err(format!("写入节点失败：{e}")))?;
            }
            drop(stmt);
            for node in &graph.nodes {
                if let Some(key) = keys.get(&node.id) {
                    let properties: serde_json::Value =
                        serde_json::from_str(&node.properties).unwrap_or_default();
                    let java = properties.get("java");
                    let symbol = java.and_then(|value| value.get("symbol"));
                    let module_key = java
                        .and_then(|value| value.get("module_key"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let source_set = java
                        .and_then(|value| value.get("source_set"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let package = java
                        .and_then(|value| value.get("package"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let signature = symbol
                        .and_then(|value| value.get("signature"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let kind = symbol
                        .and_then(|value| value.get("kind"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| node.label.as_str().to_ascii_lowercase());
                    let stability = symbol
                        .and_then(|value| value.get("stability"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("stable");
                    let type_metadata_json = symbol
                        .map(serde_json::Value::to_string)
                        .unwrap_or_else(|| "{}".into());
                    let annotations_json = symbol
                        .and_then(|value| value.get("annotations"))
                        .map(serde_json::Value::to_string)
                        .unwrap_or_else(|| "[]".into());
                    tx.execute(
                        "INSERT INTO symbol_semantics(symbol_key,module_key,source_set,package,signature,kind,type_metadata_json,annotations_json,stability) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        params![key,module_key,source_set,package,signature,kind,type_metadata_json,annotations_json,stability],
                    )
                    .map_err(|e| err(format!("写入符号语义失败：{e}")))?;
                    tx.execute("INSERT INTO search_documents(doc_id,kind,title,body,generation) VALUES(?1,'symbol',?2,?3,?4)", params![key, node.name, node.qualified_name, (current_generation + 1) as i64])
                        .map_err(|e| err(format!("写入检索文档失败：{e}")))?;
                }
            }
            for edge in &graph.edges {
                if edge.etype == EdgeType::Calls {
                    if let (Some(source), Some(target)) =
                        (keys.get(&edge.source), keys.get(&edge.target))
                    {
                        let source = EntityKey::Symbol(super::identity::parse_symbol_key(source)?);
                        let target = EntityKey::Symbol(super::identity::parse_symbol_key(target)?);
                        let relation_id = relation_id_from_parts(
                            &source,
                            Some(&target),
                            RelationKind::Calls,
                            None,
                            None,
                            &[],
                        );
                        let properties: serde_json::Value =
                            serde_json::from_str(&edge.properties).unwrap_or_default();
                        let strategy = properties
                            .get("strategy")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown");
                        let confidence = properties
                            .get("confidence")
                            .and_then(|value| value.as_f64());
                        tx.execute("INSERT INTO relations(relation_id,source_key,target_key,kind,certainty,resolution_state,rule_id,heuristic_score) VALUES(?1,?2,?3,'calls','inferred','resolved',?4,?5)", params![relation_id, source.id(), target.id(), format!("legacy:{strategy}"), confidence])
                            .map_err(|e| err(format!("写入关系失败：{e}")))?;
                    }
                }
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
                .prepare("INSERT INTO file_hashes (rel_path, sha256, mtime_ns, size) VALUES (?1, ?2, ?3, ?4)")
                .map_err(|e| err(format!("准备文件哈希写入失败：{e}")))?;
            for h in hashes {
                stmt.execute(params![
                    h.rel_path,
                    h.sha256,
                    h.mtime_ns as i64,
                    h.size as i64,
                ])
                .map_err(|e| err(format!("写入文件哈希失败：{e}")))?;
                let facts = facts
                    .files
                    .iter()
                    .find(|fact| fact.rel_path == h.rel_path)
                    .cloned()
                    .unwrap_or_else(|| {
                        FileFacts::from_json(
                            h.rel_path.clone(),
                            h.sha256.clone(),
                            "skipped",
                            serde_json::json!({"definitions":[],"imports":[],"calls":[]}),
                            meta.indexed_at,
                        )
                    });
                tx.execute("INSERT INTO file_facts(rel_path,sha256,adapter_versions_json,parse_status,facts_json,extracted_at) VALUES(?1,?2,?3,?4,?5,?6)", params![facts.rel_path,facts.sha256,facts.adapter_versions_json,facts.parse_status,facts.facts_json,facts.extracted_at as i64])
                    .map_err(|e| err(format!("写入原始事实失败：{e}")))?;
            }
        }

        // 调用点以位置为身份，不按 callee 聚合。无法对应函数的顶层调用没有可用
        // owner_key，故只记录诊断；普通语言的函数范围内调用都可稳定落表。
        let mut call_ordinals: HashMap<(String, String, u32, u32), u32> = HashMap::new();
        for call in &facts.calls {
            let owner = call
                .owner_qn
                .as_ref()
                .and_then(|qualified_name| graph.find_by_qn(qualified_name))
                .and_then(|node_id| keys.get(&node_id));
            let Some(owner) = owner else {
                tx.execute("INSERT INTO diagnostics(rel_path,adapter,code,detail,range_json) VALUES(?1,'generic','unowned_call',?2,?3)", params![call.rel_path, call.detail, format!("{{\"start_byte\":{},\"end_byte\":{}}}", call.start_byte, call.end_byte)])
                    .map_err(|e| err(format!("写入调用点诊断失败：{e}")))?;
                continue;
            };
            let owner_key = super::identity::parse_symbol_key(owner)?;
            let ordinal_key = (
                owner.clone(),
                call.rel_path.clone(),
                call.start_byte,
                call.end_byte,
            );
            let ordinal = *call_ordinals.entry(ordinal_key).or_insert(0);
            *call_ordinals
                .get_mut(&(
                    owner.clone(),
                    call.rel_path.clone(),
                    call.start_byte,
                    call.end_byte,
                ))
                .expect("调用点序号已插入") += 1;
            let callsite = EntityKey::callsite_from_parts(
                &owner_key,
                &call.rel_path,
                call.start_byte as u64,
                call.end_byte as u64,
                ordinal,
            );
            tx.execute(
                "INSERT INTO entities(entity_key,entity_kind,rel_path) VALUES(?1,'callsite',?2)",
                params![callsite.id(), call.rel_path],
            )
            .map_err(|e| err(format!("写入调用点实体失败：{e}")))?;
            let owner_id = graph
                .nodes
                .iter()
                .find(|node| keys.get(&node.id) == Some(owner))
                .map(|node| node.id);
            let resolved_target = owner_id.and_then(|source| {
                graph
                    .edges
                    .iter()
                    .find(|edge| {
                        if edge.etype != EdgeType::Calls || edge.source != source {
                            return false;
                        }
                        serde_json::from_str::<serde_json::Value>(&edge.properties)
                            .ok()
                            .and_then(|properties| {
                                properties
                                    .get("callee")
                                    .and_then(|value| value.as_str())
                                    .map(|callee| callee == call.callee_expr)
                            })
                            .unwrap_or(false)
                    })
                    .and_then(|edge| {
                        let target_key = keys.get(&edge.target)?.clone();
                        let properties: serde_json::Value =
                            serde_json::from_str(&edge.properties).unwrap_or_default();
                        let strategy = properties
                            .get("strategy")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let confidence = properties
                            .get("confidence")
                            .and_then(|value| value.as_f64());
                        Some((target_key, strategy, confidence))
                    })
            });
            let resolution_state = if resolved_target.is_some() {
                "resolved"
            } else {
                call.resolution.as_str()
            };
            let resolution = serde_json::json!({"state":resolution_state,"target":resolved_target.as_ref().map(|target| target.0.as_str()),"reason":call.detail}).to_string();
            tx.execute("INSERT INTO call_sites(callsite_key,owner_key,rel_path,start_byte,end_byte,start_line,end_line,content_sha256,receiver_expr,callee_expr,argument_exprs_json,resolution) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![callsite.id(), owner, call.rel_path, call.start_byte as i64, call.end_byte as i64, call.start_line as i64, call.end_line as i64, call.sha256, call.receiver_expr, call.callee_expr, serde_json::to_string(&call.argument_exprs).unwrap_or_else(|_| "[]".into()), resolution])
                .map_err(|e| err(format!("写入调用点失败：{e}")))?;
            let reference = super::identity::SourceRef::new(super::identity::SourceRefParts {
                project_key: repo_key(&meta.repo_path),
                generation: current_generation + 1,
                relative_path: call.rel_path.clone(),
                content_sha256: call.sha256.clone(),
                start_byte: call.start_byte as u64,
                end_byte: call.end_byte as u64,
                start_line: call.start_line,
                end_line: call.end_line,
            })?;
            let evidence_id = evidence_id_from_parts(
                EvidenceKind::CallSite,
                &[reference.clone()],
                None,
                &sha256_hex(call.callee_expr.as_bytes()),
            );
            tx.execute("INSERT INTO evidence(evidence_id,kind,excerpt_digest,refs_json) VALUES(?1,'call_site',?2,?3)", params![evidence_id, sha256_hex(call.callee_expr.as_bytes()), serde_json::to_string(&[reference]).unwrap()])
                .map_err(|e| err(format!("写入调用点证据失败：{e}")))?;
            let relation_id = if let Some((target_key, strategy, confidence)) = resolved_target {
                let target = EntityKey::Symbol(super::identity::parse_symbol_key(&target_key)?);
                let relation_id = relation_id_from_parts(
                    &callsite,
                    Some(&target),
                    RelationKind::Calls,
                    None,
                    None,
                    &[],
                );
                tx.execute("INSERT INTO relations(relation_id,source_key,target_key,kind,certainty,resolution_state,rule_id,heuristic_score) VALUES(?1,?2,?3,'calls','inferred','resolved',?4,?5)", params![relation_id, callsite.id(), target.id(), format!("legacy:{strategy}"), confidence])
                    .map_err(|e| err(format!("写入调用点关系失败：{e}")))?;
                relation_id
            } else {
                let relation_id =
                    relation_id_from_parts(&callsite, None, RelationKind::Calls, None, None, &[]);
                tx.execute("INSERT INTO relations(relation_id,source_key,target_key,kind,certainty,resolution_state) VALUES(?1,?2,NULL,'calls','unknown',?3)", params![relation_id, callsite.id(), resolution_state])
                    .map_err(|e| err(format!("写入未解析调用关系失败：{e}")))?;
                relation_id
            };
            tx.execute(
                "INSERT INTO relation_evidence(relation_id,evidence_id) VALUES(?1,?2)",
                params![relation_id, evidence_id],
            )
            .map_err(|e| err(format!("关联调用点证据失败：{e}")))?;
        }
        for diagnostic in &facts.diagnostics {
            tx.execute("INSERT INTO diagnostics(rel_path,adapter,code,detail,range_json) VALUES(?1,?2,?3,?4,?5)", params![diagnostic.rel_path, diagnostic.adapter, diagnostic.code, diagnostic.detail, diagnostic.range_json])
                .map_err(|e| err(format!("写入覆盖诊断失败：{e}")))?;
        }

        let generation = current_generation;
        let manifest_digest = manifest_digest(hashes);
        let calls_total = if facts.calls.is_empty() {
            graph
                .edges
                .iter()
                .filter(|edge| edge.etype == EdgeType::Calls)
                .count() as u64
        } else {
            facts.calls.len() as u64
        };
        let calls_resolved = if facts.calls.is_empty() {
            calls_total
        } else {
            tx.query_row("SELECT COUNT(*) FROM call_sites WHERE json_extract(resolution, '$.state') = 'resolved'", [], |row| row.get::<_, i64>(0)).unwrap_or(0) as u64
        };
        let calls_ambiguous = facts
            .calls
            .iter()
            .filter(|call| call.resolution == "ambiguous")
            .count() as u64;
        let calls_external = facts
            .calls
            .iter()
            .filter(|call| call.resolution == "external")
            .count() as u64;
        let files_parsed = if facts.files.is_empty() {
            hashes.len() as u64
        } else {
            facts
                .files
                .iter()
                .filter(|file| file.parse_status == "ok")
                .count() as u64
        };
        let files_partial = facts
            .files
            .iter()
            .filter(|file| file.parse_status == "partial")
            .count() as u64;
        let mut files_skipped = facts
            .files
            .iter()
            .filter(|file| file.parse_status == "skipped")
            .count() as u64;
        files_skipped = files_skipped.saturating_add(facts.excluded_count);
        let files_unreadable = facts
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "unreadable")
            .count() as u64;
        let files_discovered = if facts.files_discovered == 0 {
            hashes.len() as u64
        } else {
            facts.files_discovered
        };
        let classified = files_parsed
            .checked_add(files_partial)
            .and_then(|value| value.checked_add(files_skipped))
            .and_then(|value| value.checked_add(files_unreadable))
            .ok_or_else(|| err("覆盖文件计数溢出"))?;
        if classified != files_discovered {
            return Err(err(format!(
                "覆盖文件计数不闭合：发现 {files_discovered}，分类 {classified}"
            )));
        }
        let classified_known_calls = calls_resolved
            .checked_add(calls_ambiguous)
            .and_then(|value| value.checked_add(calls_external))
            .ok_or_else(|| err("覆盖调用计数溢出"))?;
        let calls_unresolved =
            calls_total
                .checked_sub(classified_known_calls)
                .ok_or_else(|| {
                    err(format!(
                        "覆盖调用计数不闭合：总数 {calls_total}，已分类 {classified_known_calls}"
                    ))
                })?;
        let coverage = serde_json::to_string(&CoverageSummary {
            files_discovered,
            files_parsed,
            files_partial,
            files_skipped,
            files_unreadable,
            calls_total,
            calls_resolved,
            calls_ambiguous,
            calls_unresolved,
            calls_external,
        })
        .map_err(|e| err(format!("序列化覆盖摘要失败：{e}")))?;
        let adapter_versions =
            serde_json::to_string(&[("generic", if facts.files.is_empty() { "1" } else { "2" })])
                .map_err(|e| err(format!("序列化适配器版本失败：{e}")))?;
        tx.execute(
            "DELETE FROM meta WHERE key NOT IN ('schema_version', 'generation')",
            [],
        )
        .map_err(|e| err(format!("清理旧元信息失败：{e}")))?;
        for (key, value) in [
            ("repo_name", meta.repo_name.as_str()),
            ("repo_path", meta.repo_path.as_str()),
            ("branch", meta.branch.as_str()),
            ("indexed_at", &meta.indexed_at.to_string()),
            ("duration_ms", &meta.duration_ms.to_string()),
            ("mode", meta.mode.as_str()),
            ("generation", &(generation + 1).to_string()),
            ("manifest_digest", manifest_digest.as_str()),
            ("adapter_versions", adapter_versions.as_str()),
            ("canonical_root", meta.repo_path.as_str()),
            ("coverage", coverage.as_str()),
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

    /// 发布前读取代际。提取器以此作为基线；后续 DEV-07 用它驱动跨进程冲突重试。
    pub fn generation(&self) -> Result<u64> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key='generation'", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| err(format!("读取索引代际失败：{e}")))?
            .parse()
            .map_err(|_| err("索引代际格式无效"))
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
            .prepare("SELECT rel_path, sha256, mtime_ns, size FROM file_hashes")
            .map_err(|e| err(format!("读取文件哈希失败：{e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileHashRow {
                    rel_path: row.get(0)?,
                    sha256: row.get(1)?,
                    mtime_ns: row.get::<_, i64>(2)?.max(0) as u64,
                    size: row.get::<_, i64>(3)?.max(0) as u64,
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

fn stable_symbol_key(node: &super::graph::GraphNode, meta: &CodeIndexMeta) -> Result<String> {
    let language = match node.file_path.rsplit('.').next() {
        Some("java") => "java",
        Some("kt") | Some("kts") => "kotlin",
        Some("py") => "python",
        Some("js") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("go") => "go",
        Some("rs") => "rust",
        _ => "text",
    };
    let project_key = if meta.repo_path.is_empty() {
        "unknown".to_string()
    } else {
        repo_key(&meta.repo_path)
    };
    let properties: serde_json::Value = serde_json::from_str(&node.properties).unwrap_or_default();
    let java = properties.get("java");
    let symbol = java.and_then(|value| value.get("symbol"));
    let module = java
        .and_then(|value| value.get("module_key"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let source_set = java
        .and_then(|value| value.get("source_set"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let package = java
        .and_then(|value| value.get("package"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let owner_chain = symbol
        .and_then(|value| value.get("owner_chain"))
        .and_then(|value| value.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.as_str())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    let stability = symbol
        .and_then(|value| value.get("stability"))
        .and_then(|value| value.as_str())
        .unwrap_or("stable");
    let java_type_chain = if language == "java" && stability == "local" {
        let start_byte = symbol
            .and_then(|value| value.get("start_byte"))
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        format!("{owner_chain}@local:{start_byte}")
    } else {
        owner_chain.clone()
    };
    let semantic_kind = symbol
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| node.label.as_str());
    let signature = symbol
        .and_then(|value| value.get("signature"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let material = symbol_key_material(&SymbolKeyMaterial {
        language,
        project_key: &project_key,
        module,
        source_set,
        package,
        relative_path: if matches!(language, "java" | "kotlin") {
            ""
        } else {
            &node.file_path
        },
        type_chain: if language == "java" {
            &java_type_chain
        } else {
            &node.qualified_name
        },
        kind: semantic_kind,
        name: &node.name,
        signature,
    })?;
    Ok(symbol_key_from_material(&material).to_string())
}

fn manifest_digest(hashes: &[FileHashRow]) -> String {
    let mut rows: Vec<_> = hashes.iter().collect();
    rows.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    let mut material = String::new();
    for row in rows {
        material.push_str(&format!(
            "{}:{}{}:{}",
            row.rel_path.len(),
            row.rel_path,
            row.sha256.len(),
            row.sha256
        ));
    }
    sha256_hex(material.as_bytes())
}

fn enable_foreign_keys(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| err(format!("启用 SQLite 外键失败：{e}")))?;
    let enabled: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|e| err(format!("确认 SQLite 外键失败：{e}")))?;
    if enabled != 1 {
        return Err(err("SQLite 外键未启用"));
    }
    Ok(())
}

/// 只读打开索引库读统计（设置页展示用）。库不存在返回 None。
/// WAL 模式下与正在写入的索引任务并发安全（读者不阻塞）。
pub fn read_index_stats(db_path: &Path) -> Result<Option<IndexStats>> {
    let Some(store) = open_read_only_if_exists(db_path)? else {
        return Ok(None);
    };
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
    let Some(store) = open_read_only_if_exists(db_path)? else {
        return Ok((Vec::new(), 0));
    };
    store.search_symbols_filtered(query, label, limit)
}

/// 只读打开一个已存在的索引库（查询层专用）。文件不存在时返回 `Ok(None)`；
/// 存在但 schema 版本不符或损坏时返回 `[NeedsRebuild]`——绝不创建/重建。
pub fn open_read_only_if_exists(db_path: &Path) -> Result<Option<CodeIndexStore>> {
    if !db_path.exists() {
        return Ok(None);
    }
    let conn = open_read_only(db_path)
        .map_err(|e| err(format!("[NeedsRebuild] 索引库无法读取，需要重建：{e}")))?;
    let store = CodeIndexStore { conn };
    let version = store
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| {
            err(format!(
                "[NeedsRebuild] 索引库损坏或 schema 不完整，需要重建：{e}"
            ))
        })?;
    if version != CODE_INDEX_SCHEMA_VERSION.to_string() {
        return Err(err(format!(
            "[NeedsRebuild] 索引 schema 版本为 {version}，需要重建"
        )));
    }
    Ok(Some(store))
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
