# 索引 schema v3 DDL 最终草案（DEV-01 交付）

状态：M1 实现前最终草案。DEV-02 落库时按本文执行；如实现中发现冲突，
先改本文（含版本号）再动代码。基线 schema v2 见 `src/code_index/store.rs`
（nodes / edges / file_hashes / meta / nodes_fts）。

## 0. 设计前提（来自 design.md §4.5）

- 在**同一个 SQLite 文件**内升级到 v3：复用 nodes/edges 作为旧查询兼容投影，
  新增事实/证据/关系表。不新建第二个数据库文件（Windows 下替换被 GUI/MCP
  持有的文件不可靠）。
- 聚合 `edges` 与新 `relations` 由**同一事实集**在发布事务内派生，
  禁止独立双写产生漂移。
- 所有跨表删除、FTS 更新、代际递增同事务完成；发布协议 =
  「提取前记基准代际 → BEGIN IMMEDIATE 后核对基准代际 → 匹配才写入并 +1」。
- schema 不符时返回 NeedsRebuild，不给旧数据贴新版本号。

## 1. 迁移与版本策略

- `schema_version`：2 → 3。打开时发现 v2 库：**不可增量升级**（v2 无内容
  哈希与事实表，增量会带进不可信旧语义），整库逻辑重建，UI 呈现
  NeedsRebuild / 「需要重建索引」。
- v3 库内 `meta` 新增键：`generation`（u64，发布事务内递增）、
  `manifest_digest`、`adapter_versions`（JSON）、`canonical_root`、
  `coverage`（JSON）、`repo_path`（v2 已有约定，保留）。
- 迁移事务失败/取消/进程退出：旧库保持原样并继续返回 NeedsRebuild，不出现
  半升级库。用户启动重建后，在**同一 SQLite 文件的单个写事务**中删除旧 schema、
  建立 v3 schema 并写入完整结果；不沿用当前 `CodeIndexStore::open` 的自动删文件路径。
  事务提交前失败即回滚到 v2。旧进程仍持连接时先返回 IndexBusy/NeedsRebuild，
  不强删主文件、WAL 或 SHM。

### 1.1 稳定 ID 规范

所有身份材料按固定字段顺序编码，每个字段使用 `<UTF-8 字节长度>:<原文>`，再取
SHA-256 小写 hex；不使用可出现在字段值中的单字符分隔符。摘要固定 64 位。

| 前缀 | 规范材料 |
| --- | --- |
| `sk1` | language、project_key、module、source_set、package、relative_path、type_chain、kind、name、signature；Java/Kotlin 的 relative_path 留空，普通语言必须填写项目内相对路径 |
| `cs1` | owner SymbolKey、relative_path、start_byte、end_byte、同位置 ordinal |
| `ep1` | entry_kind、module_key、declaring EntityKey、入口判别串（规范 HTTP method/path、namespace#statement 等） |
| `r1` | source EntityKey、target EntityKey 或空串、kind、rule_id、candidate_group、按源码顺序的 conditions |
| `e1` | evidence kind、rule_id、excerpt_digest、按规范键排序的完整 SourceRef 列表 |
| `bn1` | project_key、generation、来源 relation_id、reason、同来源 ordinal；仅在该代 GraphSlice 内稳定，不持久化为源码实体 |

任何材料字段或规范化规则变化都必须递增相应前缀版本并触发 NeedsRebuild。
`manifest_digest` 使用按 relative_path 排序后的 `(relative_path, content_sha256)` 长度前缀
材料计算 SHA-256；`adapter_versions` 按 adapter id 排序后参与发布元数据，不依赖 HashMap
迭代顺序。

## 2. 既有表变更

### entities（新增统一实体注册表）

`relations` 的端点跨 symbol/callsite/entry 三种持久实体，不能分别声明单一外键。
先用注册表统一承接引用完整性；`boundary` 是查询期投影，不写入数据库。

```sql
CREATE TABLE entities (
  entity_key TEXT PRIMARY KEY,
  entity_kind TEXT NOT NULL CHECK(entity_kind IN ('symbol', 'callsite', 'entry')),
  rel_path TEXT NOT NULL DEFAULT '',
  CHECK (
    (entity_kind = 'symbol'   AND length(entity_key) = 68 AND substr(entity_key, 1, 4) = 'sk1:') OR
    (entity_kind = 'callsite' AND length(entity_key) = 68 AND substr(entity_key, 1, 4) = 'cs1:') OR
    (entity_kind = 'entry'    AND length(entity_key) = 68 AND substr(entity_key, 1, 4) = 'ep1:')
  ),
  CHECK(substr(entity_key, 5) NOT GLOB '*[^0-9a-f]*')
);
CREATE INDEX idx_entities_path_kind ON entities(rel_path, entity_kind);
```

增量发布按 `entities.rel_path` 删除受影响实体，由外键级联清理符号、调用点、入口和
关系；随后先写 entities，再写各实体明细。全量关系重算仍在同一发布事务内执行。

### nodes（保留 + 增列）

```sql
CREATE TABLE nodes (
  id INTEGER PRIMARY KEY,
  label TEXT NOT NULL,
  name TEXT NOT NULL,
  qualified_name TEXT NOT NULL UNIQUE,
  file_path TEXT DEFAULT '',
  start_line INTEGER DEFAULT 0,
  end_line INTEGER DEFAULT 0,
  properties TEXT DEFAULT '{}',
  -- Project/Branch/Folder/File/Module 是兼容结构节点，保持 NULL；
  -- 其余源码符号必须登记稳定键。QN 保留供旧查询兼容。
  symbol_key TEXT UNIQUE REFERENCES entities(entity_key) ON DELETE CASCADE,
  CHECK (
    (label IN ('Project', 'Branch', 'Folder', 'File', 'Module') AND symbol_key IS NULL) OR
    (label IN ('Function', 'Method', 'Class', 'Struct', 'Interface', 'Enum', 'Trait', 'Type', 'Field')
      AND symbol_key IS NOT NULL)
  )
);
```

`UNIQUE(symbol_key)` 已自动建立索引，不重复创建等价索引。

### edges（保留，作为兼容投影）

结构与 v2 完全一致（source_id/target_id/type/properties + 双向索引）。
发布时由 `relations` 派生：certainty=syntactic 的 Calls → CALLS 边；
DispatchCandidate/InjectsCandidate 等候选不进 edges（旧 UI 不得把候选
渲染成实线业务链）。

### file_hashes（启用 sha256 列）

```sql
CREATE TABLE file_hashes (
  rel_path TEXT PRIMARY KEY,
  sha256 TEXT NOT NULL CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
                                    -- v3 起必填：解析字节指纹
  mtime_ns INTEGER NOT NULL DEFAULT 0,
  size INTEGER NOT NULL DEFAULT 0
);
```

v2 的 sha256 恒为空串，因此不满足 v3 CHECK，必须重建；v3 写入真实内容指纹
（identity::content_fingerprint）。

### nodes_fts（保留）

contentless FTS5 结构不变；v3 起搜索文本与 `search_documents` 同事务维护。

## 3. 新增表

### file_facts — 原始提取事实（按文件持久化）

```sql
CREATE TABLE file_facts (
  rel_path TEXT PRIMARY KEY REFERENCES file_hashes(rel_path) ON DELETE CASCADE,
  sha256 TEXT NOT NULL CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
                                    -- 提取时内容指纹；读取时校验
  adapter_versions_json TEXT NOT NULL, -- 本文件实际消费的语言/框架适配器版本表
  parse_status TEXT NOT NULL CHECK(parse_status IN ('ok', 'partial', 'error', 'skipped')),
  facts_json TEXT NOT NULL,         -- 原始事实：定义/导入/调用点/注解/控制流范围
  extracted_at INTEGER NOT NULL
);
```

未解析调用、多候选解析事实保存在 facts_json 中，关系层不丢原始信息。

### symbol_semantics — 符号语义元数据

```sql
CREATE TABLE symbol_semantics (
  symbol_key TEXT PRIMARY KEY REFERENCES nodes(symbol_key) ON DELETE CASCADE,
  module_key TEXT DEFAULT '',
  source_set TEXT DEFAULT '',       -- main | test | ''
  package TEXT DEFAULT '',
  signature TEXT DEFAULT '',
  kind TEXT NOT NULL,               -- method/constructor/class/...
  type_metadata_json TEXT DEFAULT '{}',   -- 参数/返回/泛型原文
  annotations_json TEXT DEFAULT '[]',
  stability TEXT NOT NULL DEFAULT 'stable' -- stable | local（匿名/局部声明）
);
CREATE INDEX idx_semantics_module ON symbol_semantics(module_key, source_set);
```

### call_sites — 调用点（不聚合去重）

```sql
CREATE TABLE call_sites (
  callsite_key TEXT PRIMARY KEY REFERENCES entities(entity_key) ON DELETE CASCADE,
                                    -- cs1:<64 位小写 hex>，身份含 owner+位置+序
  owner_key TEXT NOT NULL REFERENCES nodes(symbol_key) ON DELETE CASCADE,
  rel_path TEXT NOT NULL REFERENCES file_hashes(rel_path) ON DELETE CASCADE,
  start_byte INTEGER NOT NULL CHECK(start_byte >= 0),
  end_byte INTEGER NOT NULL CHECK(end_byte >= start_byte),
  start_line INTEGER NOT NULL CHECK(start_line >= 1),
  end_line INTEGER NOT NULL CHECK(end_line >= start_line),
  content_sha256 TEXT NOT NULL CHECK(
    length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  receiver_expr TEXT DEFAULT '',    -- receiver 原文（this/super/变量/类型名）
  callee_expr TEXT NOT NULL,        -- 调用名原文
  argument_exprs_json TEXT DEFAULT '[]',
  resolution TEXT NOT NULL          -- resolved|ambiguous|unresolved|external + 目标/候选
);
CREATE INDEX idx_callsites_owner ON call_sites(owner_key);
CREATE INDEX idx_callsites_path ON call_sites(rel_path);
```

同一方法重复调用保存为多行（J12 断言：调用点证据不丢）。
`resolution` 为 JSON 对象（状态、目标 symbol_key、候选列表、原因）。

### relations — 逻辑关系（新图真相源）

```sql
CREATE TABLE relations (
  relation_id TEXT PRIMARY KEY CHECK(
    length(relation_id) = 67 AND substr(relation_id, 1, 3) = 'r1:'
      AND substr(relation_id, 4) NOT GLOB '*[^0-9a-f]*'
  ),
  source_key TEXT NOT NULL REFERENCES entities(entity_key) ON DELETE CASCADE,
  target_key TEXT REFERENCES entities(entity_key) ON DELETE CASCADE,
                                    -- NULL 只存储未解析/外部目标；查询时投影 bn1 边界
  kind TEXT NOT NULL CHECK(kind IN (
    'contains', 'calls', 'inherits', 'implements', 'overrides',
    'dispatch_candidate', 'injects_candidate', 'handles', 'maps_to'
  )),
  certainty TEXT NOT NULL CHECK(certainty IN ('syntactic', 'inferred', 'unknown')),
  resolution_state TEXT NOT NULL CHECK(
    resolution_state IN ('resolved', 'ambiguous', 'unresolved', 'external')
  ),
  rule_id TEXT,                     -- 框架规则 id；syntactic 为 NULL
  candidate_group TEXT,             -- 同组候选共享分组
  conditions_json TEXT DEFAULT '[]',
  heuristic_score REAL,
  CHECK(target_key IS NOT NULL OR resolution_state IN ('unresolved', 'external'))
);
CREATE INDEX idx_relations_source ON relations(source_key, kind, target_key);
CREATE INDEX idx_relations_target ON relations(target_key, kind, source_key);
CREATE INDEX idx_relations_kind ON relations(kind);
```

`certainty` 与 `resolution_state` 两列正交存储（types.rs 注释约束），
查询层不得用一列推导另一列。数据库中的 NULL target 不得原样进入 GraphSlice；
查询服务必须按稳定 ID 规范生成 `bn1` BoundaryNode，并把返回的 RelationRecord.target_key
替换为该边界键，使 GraphSlice 的每条关系都有完整端点。

### evidence / relation_evidence — 证据

```sql
CREATE TABLE evidence (
  evidence_id TEXT PRIMARY KEY CHECK(
    length(evidence_id) = 67 AND substr(evidence_id, 1, 3) = 'e1:'
      AND substr(evidence_id, 4) NOT GLOB '*[^0-9a-f]*'
  ),
  kind TEXT NOT NULL CHECK(kind IN (
    'declaration', 'call_site', 'annotation', 'sql_mapping',
    'control_flow', 'module_aggregation'
  )),
  rule_id TEXT,
  excerpt_digest TEXT NOT NULL,
  refs_json TEXT NOT NULL           -- SourceRef[]（project_key/generation/path/hash/byte/line）
);

CREATE TABLE relation_evidence (
  relation_id TEXT NOT NULL REFERENCES relations(relation_id) ON DELETE CASCADE,
  evidence_id TEXT NOT NULL REFERENCES evidence(evidence_id) ON DELETE CASCADE,
  PRIMARY KEY (relation_id, evidence_id)
);
```

### entry_points — 入口登记

```sql
CREATE TABLE entry_points (
  entry_key TEXT PRIMARY KEY REFERENCES entities(entity_key) ON DELETE CASCADE,
                                    -- ep1:<64 位小写 hex>
  kind TEXT NOT NULL,               -- http_route|mapper_statement|jpa_repo|scheduled|listener
  symbol_key TEXT REFERENCES nodes(symbol_key) ON DELETE SET NULL,
  module_key TEXT DEFAULT '',
  meta_json TEXT DEFAULT '{}',      -- 路由 path/method、statement id 等
  evidence_id TEXT REFERENCES evidence(evidence_id) ON DELETE SET NULL
);
CREATE INDEX idx_entry_kind ON entry_points(kind);
```

### diagnostics — 覆盖缺口

```sql
CREATE TABLE diagnostics (
  id INTEGER PRIMARY KEY,
  rel_path TEXT DEFAULT '',
  adapter TEXT NOT NULL,            -- generic|java|spring|mybatis|jpa|source_reader
  code TEXT NOT NULL,               -- 原因码（unresolved_call/parse_error/excluded/...）
  detail TEXT DEFAULT '',
  range_json TEXT DEFAULT '{}'      -- 可选行/字节范围
);
CREATE INDEX idx_diagnostics_path ON diagnostics(rel_path);
```

### search_documents + search_fts — 检索文本（内容表 + FTS）

```sql
CREATE TABLE search_documents (
  doc_id TEXT PRIMARY KEY,          -- 实体键（symbol_key / entry_key）
  kind TEXT NOT NULL,               -- symbol|entry|comment|path
  title TEXT NOT NULL,
  body TEXT NOT NULL,               -- FQN/路径/注释/Javadoc/SQL 摘要
  module_key TEXT DEFAULT '',
  generation INTEGER NOT NULL
);
CREATE INDEX idx_search_docs_kind ON search_documents(kind);

CREATE VIRTUAL TABLE search_fts USING fts5(
  title, body,
  content='search_documents', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);
-- 外部内容表维护触发器（同事务）：
CREATE TRIGGER search_fts_insert AFTER INSERT ON search_documents BEGIN
  INSERT INTO search_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;
CREATE TRIGGER search_fts_delete AFTER DELETE ON search_documents BEGIN
  INSERT INTO search_fts(search_fts, rowid, title, body)
  VALUES ('delete', old.rowid, old.title, old.body);
END;
CREATE TRIGGER search_fts_update AFTER UPDATE ON search_documents BEGIN
  INSERT INTO search_fts(search_fts, rowid, title, body)
  VALUES ('delete', old.rowid, old.title, old.body);
  INSERT INTO search_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;
```

## 4. 发布协议（DEV-02/07 实现契约）

```
基准代际 g0 = meta.generation（读事务）
… 提取（内存/临时区，不锁库）…
BEGIN IMMEDIATE
  now = meta.generation
  if now != g0: ROLLBACK；丢弃本批产物；提示重新执行
  写 entities/file_facts/symbol_semantics/call_sites/relations/evidence/
     entry_points/diagnostics/search_documents（增量 = 变更文件级重写）
  删除无 relation_evidence 引用的孤立 evidence
  由同一事实集派生兼容投影 edges / nodes_fts / search_fts
  meta.generation = g0 + 1；写 manifest_digest / coverage
COMMIT
```

GUI 与 MCP 共用同一实现函数；短查询只读连接不跨网络等待。
旧 MCP 进程不遵守该协议，升级引导要求重启旧进程后再迁移。

## 5. 与 DEV-01 类型的对应

| DDL | Rust 契约（已实现） |
| --- | --- |
| entities / nodes.symbol_key | `identity::SymbolKey` 与 `types::EntityKey`（严格前缀和 64 位小写 hex） |
| file_hashes.sha256 / facts.sha256 | `identity::content_fingerprint` |
| evidence.refs_json | `identity::SourceRef`（区间不变量已校验） |
| relations.* | `types::RelationRecord` / RelationKind / Certainty / ResolutionState |
| call_sites.callsite_key | `EntityKey::CallSite`（cs1 命名空间） |
| entry_points.entry_key | `EntityKey::Entry`（ep1） |
| boundary（无表，运行时投影） | `EntityKey::Boundary`（bn1）+ `BoundaryNode` |
| meta.generation / coverage | `identity::IndexSnapshot` / `CoverageSummary` |

EntityKey 四命名空间（sk1/cs1/ep1/bn1）由 Rust 自定义反序列化与构造器校验。
持久实体只包含 sk1/cs1/ep1，并通过 entities 外键保证关系端点存在；bn1 仅由查询服务
投影。DDL CHECK 与 Rust 校验双层防御，insert helper 仍必须拒绝 kind/前缀不匹配。
