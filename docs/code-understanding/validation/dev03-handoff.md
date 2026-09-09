# DEV-03 交接

状态：已完成（2026-09-08，含审查修正）。可作为 DEV-04 开工依据。

## 实现

- tree-sitter 提取结果记录 `has_error`，调用点保存精确字节/行范围、接收者原文和参数原文。
- 每次发布从真实文件重建 `file_facts`：definitions、imports、calls 均写入 JSON；`ok`、`partial`、`error`、`skipped` 不再使用全量 `ok` 占位。
- 调用点按 DEV-01 的 `cs1` 构造器逐点持久化；可证明的通用 CALLS 解析写入关系、`e1` 调用点证据和 `relation_evidence`。无法证明目标的调用仍保留为原始事实和 `unresolved` 状态。
- 二进制、超限、不可读、语法 ERROR、无适配器和无归属调用均写入 diagnostics；coverage 由真实文件状态与调用解析状态计算。
- 全量、增量和 v2 原子重建均走带事实的单事务发布，保留既有 GraphBuffer/nodes/edges 兼容投影。
- 每个发现文件在一轮索引内只读取、只解析一次；内容 hash、文件事实、调用事实和兼容图均从同一批 `ParseOutput` 派生。解析线程异常、取消、代际冲突或事务写入失败均不发布部分事实。
- generic resolver 的结果只标记为 `inferred`，并保留 `legacy:<strategy>` 与启发式分数；未解析调用以 `target_key=NULL`、`unknown/unresolved` 关系落盘并关联调用点证据，不伪装为确定关系。

## 验证

- `cargo test --lib`：573 passed，0 failed，2 ignored。
- `cargo test --bin khaslana dedicated_fields`：2 passed，0 failed。
- `cargo build --release`：通过。
- `rustfmt --edition 2024 --check src/code_index/extract.rs src/code_index/pipeline.rs src/code_index/store.rs src/code_index/facts.rs src/tests/code_index.rs`：通过。
- `git diff --check`：通过。
- DEV-03 专项覆盖：重复调用点与证据闭环、未解析目标、Java receiver/参数、排除/二进制/超限/无适配器/partial/不可读诊断、全量与增量等价、取消及失败发布回滚。

## 边界

DEV-03 不实现 Java 类型/重载/源码根、Spring/MyBatis/JPA 或查询/UI，这些从 DEV-04 起继续。发现阶段的剪枝只持久化聚合 `excluded` 诊断，不枚举被整目录剪枝的内部文件；coverage 将该聚合数计入 skipped，不能据此还原每个排除路径。增量为保证事实一致性会重读本轮全部已发现文件，但兼容图只合并变更文件；后续 DEV-07 可在证明与全量结果等价后优化事实复用。
