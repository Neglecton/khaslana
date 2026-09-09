# DEV-02 交接

状态：完成（2026-09-08）。

## 实现

- `CODE_INDEX_SCHEMA_VERSION` 升至 3；新库启用 SQLite 外键并建立 v3 事实、关系、证据、入口、诊断和检索表。
- 文件哈希改为真实 SHA-256；符号写入稳定键和 entities 注册表，项目维度使用规范化 `repo_key`；generic facts 占位写入 `file_facts`。
- `relations` 与旧 `edges` 从同一个内存 GraphBuffer 发布输入派生，避免独立双写；发布元数据包含稳定排序的 manifest digest、generic adapter 版本与诚实的通用覆盖摘要。
- 发布使用 `BEGIN IMMEDIATE` 和 generation 基线校验；冲突返回 `GenerationMismatch` 并回滚。
- v2/不兼容库正常打开时返回 `NeedsRebuild`，不会删除文件；强制全量索引先完成内存构图，再在同一事务内 DROP/CREATE v3、写入快照并提交一次。取消或写入失败时 v2 旧库保留。

## 验证

- `cargo test --lib`：569 passed，2 ignored。
- `cargo test --bin khaslana dedicated_fields`：2 passed。
- `cargo build --release`：通过。
- v2 定向测试覆盖：识别不删除、原子重建、取消保留、失败回滚。
- 覆盖 v2 重建只发布一次（generation=1、mode=rebuild）与 stale generation 回滚。
- `git diff --check`：通过。

## 后续

DEV-03 才补真实原始提取事实、调用点、证据和覆盖诊断；当前 `facts_json`、
`call_sites`、`evidence` 与 coverage 只提供 schema/generic 基线，不得将其误作
Java/Spring 语义。现有通用索引没有完整签名，稳定键中的容器材料暂沿用兼容图 QN；
DEV-03/04 落实事实模型时须以真实签名和语言语义替换，不能依赖 QN 的 `#N` 消歧。
