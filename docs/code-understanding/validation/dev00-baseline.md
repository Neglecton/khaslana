# DEV-00 交接：基线核查、样例与标注、验收协议修订、性能环境记录

日期：2026-09-06。执行依据 [development.md](../development.md) §2 开工核查与 DEV-00 任务卡。本文是 M0 交接记录，不含任何业务行为修改。

## 1. 基线与最终状态

- 文档基线 HEAD：`b4ec8914139ba768f7a6596e3f870173869eabfb`（与 design.md §1 声明一致），分支 `dev_lcc`。
- 开工时 `git status --short`：已有 `src/tests/code_index.rs` 修改及未跟踪 `docs/code-understanding/`、`src/tests/fixtures/`；本轮仅在这些任务范围内继续修改，未触碰业务代码。
- 本批新增/修订（均不触碰业务行为）：
  - `src/tests/code_index.rs` 末尾新增 `mod java_grammar_probe`（3 个语法探针）和同级 fixture manifest 守卫（1 个协议闭合测试）；
  - `src/tests/fixtures/code_understanding/`（README 标注格式 + java 30 个源文件 + spring 38 个源文件 + 两份 expected.json）；
  - `docs/code-understanding/validation/`（本文 + question-bank.md）。

## 2. 源码接入点核对清单（design.md §1 逐项）

全部经 khaslana-code-index MCP 图谱（repo 键 `31b69cef`，5551 节点/12352 边）核对行号，未依赖文件名猜测：

| 设计文档声称 | 核对结果 |
| --- | --- |
| `discover.rs::discover_files` | ✅ `src/code_index/discover.rs:152` |
| `extract.rs::SymbolDef` | ✅ `src/code_index/extract.rs:28`（label/name/scope/start_line/end_line——设计缺口属实：无参数签名/字节范围/注解） |
| `extract.rs::CallSite` | ✅ `extract.rs:46`（callee_display/name/line/owner——无调用点身份、无 receiver/参数事实） |
| `lang_spec.rs` Java 规则 | ✅ `lang_spec.rs:253` 使用 `tree_sitter_java::LANGUAGE`，规格表 13 处 `LangSpec` |
| `resolve.rs::Registry::resolve_call` | ✅ `resolve.rs:70`（四级策略链与设计描述一致） |
| `graph.rs::GraphNode/GraphEdge` | ✅ `graph.rs:102` / `graph.rs:116` |
| `pipeline.rs::run_incremental_inner` | ✅ `pipeline.rs:266`；`files_hash_rows` 在 `pipeline.rs:224` |
| `store.rs` SQLite/WAL/FTS5、schema_version=2 | ✅ `store.rs:20` `CODE_INDEX_SCHEMA_VERSION: u32 = 2`；`store.rs:205` 事务发布 |
| `FileHashRow` 无 sha256 | ✅ **缺口属实**：`store.rs:32` 仅 rel_path/mtime_ns/size 三字段 |
| `queries.rs::trace_calls` 逐次 `load_graph` | ✅ `queries.rs:345` 函数体首行 `load_graph(db_path)`（整图载入）；helper `queries.rs:201`、store 实现 `store.rs:324` |
| `TraceHop` 含 risk | ✅ `queries.rs:34`（name/qualified_name/file_path/hop/risk） |
| `read_source_snippet` | ✅ `queries.rs:415` |
| `mcp.rs::McpServer::call_tool` | ✅ `mcp.rs:297`；工具名 8 个：list_projects/search_symbols/get_symbol_detail/trace_path/get_architecture/detect_changes/index_status/refresh_index（`mcp.rs:298-316`），与设计文档“问答白名单排除 detect_changes”的前提一致 |
| `code_palette_view.rs` | ✅ 存在；确认后 `open_blame_file` 进追溯（设计缺口描述属实） |
| `ai/client.rs::ChatClient::request_agent_stream` | ✅ `ai/client.rs:310` |
| `tasks.rs::TaskKind` | ✅ `tasks.rs:79`：Short/Long/Ai/Index 四变体；AI 计数 `MAX_CONCURRENT_AI_REVIEWS = 3`（`main.rs:452`，ai 线程数直接引用该常量 `tasks.rs:29`） |
| `docs/ui-design-system.md` | ✅ 存在 |

结论：设计文档 §1 的接入点与缺口描述在基线提交上全部属实，无需修订即可进入 DEV-01。

## 3. tree-sitter-java 语法探针（DEV-00 清单第 4 条）

- 版本：`tree-sitter-java 0.23.5`（Cargo.toml 与 Cargo.lock 一致；runtime `tree-sitter 0.25`）。
- 方法：直接读取 crate 源码 `src/node-types.json`，并在 `src/tests/code_index.rs::java_grammar_probe` 用真实解析器解析覆盖 12 类结构的 Java 样例做运行时断言。
- 锁定的关键事实（DEV-04/05 实现必须按此，不得照抄其他版本资料）：
  - `method_declaration` fields：`type/name/parameters/body/type_parameters/dimensions`；children 含 `modifiers`、`annotation`、`marker_annotation`、`throws`。
  - `method_invocation` fields：`object/name/arguments/type_arguments`——**receiver 字段名是 `object`**。
  - `object_creation_expression` fields：`type/arguments/type_arguments`；`constructor_declaration` fields：`name/parameters/body`。
  - 注解两种节点：`annotation`（带 `arguments`）与 `marker_annotation`。
  - `import_declaration` 无 fields；通配导入经 `asterisk` 子节点；**没有** `static_import_declaration` 节点类型（static 是导入语句内的匿名 token）。
  - record/enum/sealed/switch 表达式/lambda/方法引用节点类型存在（`record_declaration`、`enum_declaration`、`lambda_expression`、`method_reference`、`switch_expression`）。
- 运行命令：`cargo test --lib java_grammar_probe`（结果见 §5）。升级语法 crate 时本组测试必须重跑并按结果修订 DEV-04 的节点表。

## 4. 样例与题库交付

- `src/tests/fixtures/code_understanding/README.md`：标注格式规范（expected.json schema、顶层 canonical question_bank、规范符号引用协议、SourceRef/source selector、certainty 取值）。
- 普通 Java J01～J12：30 个最小源文件 + `java/expected.json`（无构建依赖；J09 含 Maven 双模块与同 FQN 隔离样例；J11 无 `.git`、无构建文件、含默认包）。
- Spring S01～S12：38 个最小源文件（Java + Mapper XML）+ `spring/expected.json`（S12 为“订单取消释放库存”完整业务样例，含 Profile 条件候选与动态 SQL）。
- 固定题库：两份 expected.json 顶层 canonical question_bank 与 `validation/question-bank.md`——普通 Java 20 题 + Spring 20 题（各 8 业务定位/4 调用解释/4 类型或注入边界/4 未知边界），每题标注 fixture 来源与允许答案要点，已冻结 v1。

## 5. 回归基线（DEV-00 清单第 5 条，改动前后对照）

| 命令 | 改动前（HEAD b4ec891） | 本批改动后 |
| --- | --- | --- |
| `cargo test --lib code_index` | ✅ 39 passed / 0 failed / 2 ignored（exit 0），含 `pipeline_full_then_incremental`、`incremental_prunes_orphan_module_nodes`（增量等价）、`mcp_tests` 12 项（MCP 协议与多仓库） | ✅ 43 passed / 0 failed / 2 ignored（exit 0；+3 语法探针 +1 fixture manifest 守卫） |
| `cargo test --bin khaslana dedicated_fields` | ✅ 2 passed / 0 failed（Ctrl+P 当前唯一回归保护） | ✅ 2 passed / 0 failed |
| `cargo test --lib java_grammar_probe` | — | ✅ 3 passed / 0 failed |

Ctrl+P 状态面板（`src/code_palette_view.rs`）没有专属单元测试；其回归仅由 `dedicated_fields_cover_all_field_ids`（FieldId::CodePaletteSearch 注册守卫）覆盖。已存在问题，与本功能新增无关；DEV-13 接入“理解此符号”动作时需一并补齐。

## 6. 性能环境记录（validation 首条）

- OS：Windows 10 x64（10.0.19045）；工具链：rustc/cargo 1.96.0（2026-05-25）。
- CPU：AMD Ryzen 5 5600 6 核 12 线程；内存：31.9 GiB；磁盘：Maxsun 240GB X5（SSD，系统盘）+ HGST 1TB HDD；源码与 target 位于 SSD 分区。
- 构建与测试均按仓库默认 profile（dev/test，未开 LTO/PGO）；基线测试耗时 `cargo test --lib code_index` ≈ 0.9s（测试本体，不含编译）。
- 标准集（10K 文件/100K 符号/500K 关系）与压力集（50K 文件）由实现阶段生成，生成脚本、种子与结果在 DEV-07/09/10 各批补录至本目录，禁止以 khaslana 自仓库 183 文件规模的成绩冒充标准集指标。
- 环境事故记录：开工当日 `C:\Users\chen\.cargo\config` 曾损坏（TOML 混入 HTML 标记），导致全部 cargo 命令失败；已由用户修复，非本功能引入。

## 7. §11 交接格式

- **基线与最终提交**：开工 `b4ec891`；本批未提交（探针测试 + fixtures + validation 文档）。
- **完成的需求/任务编号**：DEV-00/DEV-00b 全部执行清单；CU-14 的样例与题库映射部分就绪。
- **实际模块、公开接口与关键类型**：无业务 API 变化；新增测试模块 `code_index::tests::java_grammar_probe` 与 fixtures 目录。
- **schema/协议/适配器版本变化**：无。`CODE_INDEX_SCHEMA_VERSION` 仍为 2。
- **验证命令、实际测试数、退出结果**：见 §5 表格。
- **release 构建结果**：本批不适用（仅测试与文档；development.md §10）。
- **性能数据**：本批仅记录环境（§6）；无指标声明。
- **UI 截图与未验证项**：不适用（无 UI）。
- **已知限制**：fixtures 尚未被任何索引测试消费（DEV-04/05/09 接入）；expected.json 的业务断言映射代码未实现，当前仅有 manifest 守卫校验 JSON、题号、文件和断言字段；真实模型评估（CU-04/06）未开始，需 AI 供应商可用时按题库执行。
- **下一项可直接执行的任务与依赖**：DEV-01（核心 DTO：EntityKey/SourceRef/GraphSlice/AnswerDocument + DDL 草案定稿），依赖 DEV-00b 验收协议修订已完成。
