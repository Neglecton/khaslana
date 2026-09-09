# DEV-01 交接：核心 DTO、错误码与 DDL 最终草案（审查修正版）

日期：2026-09-07。依据 [development.md](../development.md) DEV-01 任务卡。基线：DEV-00 之后的 `dev_lcc` 工作区（含 DEV-00b fixture oracle 修订）。

## 1. 基线与最终状态

- 开工基线：DEV-00 完成后的工作区（HEAD 仍为 `b4ec891`，工作区含 DEV-00 未提交产物与外部追加的 DEV-00b oracle 修订）。
- 本批新增/修改：
  - 新增 `src/code_index/identity.rs` + `src/tests/code_index_identity.rs`（#[path] 挂载）
  - 新增 `src/code_understanding/{mod,types}.rs` + `src/tests/code_understanding_types.rs`
  - `src/code_index/mod.rs`：`identity` 挂为 `pub mod` 并导出稳定身份契约
  - `src/lib.rs`：挂载 `pub mod code_understanding`
  - 新增 `docs/code-understanding/validation/schema-v3-ddl.md`（DDL 最终草案）

## 2. 实际模块、公开接口与关键类型

### `khaslana::code_index::identity`（稳定键与证据身份）

| 项 | 说明 |
| --- | --- |
| `SymbolKey` | `sk1:<64 位小写 sha256-hex>` 新类型；自定义 Deserialize 强制校验，非法值不能进入 `digest()` |
| `SymbolKeyMaterial` / `symbol_key_material` | 强类型 10 字段材料，增加普通语言 relative_path；每段用 UTF-8 字节长度前缀编码，无分隔符碰撞 |
| `symbol_key_from_material` | 幂等构造（同材料同键） |
| `parse_symbol_key` | 外部输入校验：前缀 + 64 位 hex |
| `sha256_hex` / `content_fingerprint` | 内容指纹（sha2 0.11，已有依赖） |
| `SourceRefParts` / `SourceRef` | 命名字段构造与自定义 Deserialize 共用校验；半开字节区间、行号 1 起、规范相对路径、64 位小写内容指纹 |
| `IndexSnapshot` | schema/generation/manifest/适配器唯一性与 CoverageSummary 均可校验，反序列化不接受失效快照 |
| `CoverageSummary` | 文件五态与调用五分类必须分别等于总计，反序列化时拒绝矛盾计数 |
| `ProjectContext` | project_key/canonical_root/index_db_path |

### `khaslana::code_understanding`（领域 DTO 契约）

- `ANSWER_PROTOCOL_VERSION = 1` + `protocol_version()`
- `EntityKey`：tagged serde（`kind`/`id`）四变体；所有入口严格校验 64 位小写摘要；`callsite_from_parts`、`entry_from_parts`、`boundary_from_parts` 固化生成材料
- `relation_id_from_parts` / `evidence_id_from_parts`：分别固化 r1/e1 的确定性身份材料
- `RelationKind`（9 种，snake_case serde）、`Certainty`（syntactic/inferred/unknown）、`ResolutionState`（resolved/ambiguous/unresolved/external）——两维度正交存储，类型注释强制
- `RelationRecord`（含 candidate_group/conditions/heuristic_score，Eq 因 f64 不派生）
- `EvidenceKind`/`EvidenceRecord`/`EvidenceBundle`（封闭证据集：contains/get）
- `GraphSlice` + `GraphSliceNode` + `BoundaryNode` + `slice_invariants()`（端点完整、边界类型、ID 唯一、覆盖一致、截断状态一致）
- `SearchFilter`/`RelationDirection`
- `AnswerDocument` + `AnswerClaim`（nature: observed/inferred）+ `AnswerStep`（相邻步骤不保证调用边）+ `Uncertainty`/`UncertaintyKind` + `CompletionStatus`
- `validate_answer_document(&doc, &bundle, &graph)`：关系/实体只信任权威 GraphSlice；证据只信任本次 bundle；校验 claim 非空证据、SourceRef 项目/代际、覆盖与完成状态
- `ErrorCode`（design.md §12 全部 16 码）+ 中文 Display + 默认可重试性；`UnderstandingError`（code/message/retryable_override，Display 含码与说明，可转 GitError）

## 3. schema/协议/适配器版本变化与迁移方式

- 本批**未改运行时 schema**（`CODE_INDEX_SCHEMA_VERSION` 仍为 2）；v3 DDL 为纸面草案（schema-v3-ddl.md），DEV-02 实装。
- 协议新增：`ANSWER_PROTOCOL_VERSION = 1`（AnswerDocument 契约版本）。
- ID 命名空间定型：`sk1:`/`cs1:`/`ep1:`/`bn1:`/`r1:`/`e1:` + 64 位小写 hex；材料使用长度前缀编码，规则变化必须递增前缀版本。
- 草案定 v2→v3 不可增量升级、整库重建（v2 无内容哈希与事实表）。

## 4. 验证命令、实际测试数、退出结果

| 命令 | 结果 |
| --- | --- |
| `cargo test --lib types_tests` | ✅ 17 passed / 0 failed（含非法 JSON、伪造关系、跨代证据、缺失边界端点、DDL 执行与 FK 检查） |
| `cargo test --lib identity::tests` | ✅ 12 passed / 0 failed（含跨文件身份、普通语言路径必填、分隔符碰撞、非法 SHA/路径、覆盖计数） |
| `cargo test --lib`（全量） | ✅ 559 passed / 0 failed / 2 ignored；不得再用基线加法推算测试数 |
| DEV-01 文件定向 `rustfmt --check` | ✅ 通过 |

## 5. release 构建结果

`cargo build --release`：✅ 通过，零错误零警告。

## 6. 已知限制 / 设计取舍

1. v3 DDL 已作为 SQL 代码块在内存 SQLite 执行并开启 FK 检查，但运行时 schema 仍是 v2；DEV-02 才接入发布事务和旧库 NeedsRebuild 流程。
2. `boundary` 是查询期投影，不进入 entities；数据库 NULL target 必须在进入 GraphSlice 前转换为 bn1 端点。
3. `heuristic_score` 仅作兼容投影的排序参考，UI/AI 契约层不消费。
4. 全项目 `cargo fmt --check` 仍会报告 DEV-01 范围外的 `src/code_index_view.rs` 既有格式差异；本批未越界格式化该业务文件，DEV-01 新增/关联文件已定向通过。

## 7. 下一项可直接执行的任务与依赖

**DEV-02**（schema 升级、原始 facts/调用点/证据、稳定键与内容 hash；`graph.rs`、`store.rs`、`facts.rs`）：依赖 DEV-01 完成 ✅。按 schema-v3-ddl.md §4 发布协议实装；注意 draft §1 的「v2 不可增量升级」结论与 §2 file_hashes 启用 sha256 列。
