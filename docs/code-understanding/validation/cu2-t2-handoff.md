# CU2-T2/T3 单问 agent 闭环与业务答案交接

日期：2026-09-10。状态：服务层完成（无 UI）；真实模型业务验收与 T4/T5 未开始。

## 任务号与实际修改

CU2-T2（单问题 agent 闭环）与 CU2-T3（业务答案与数据读写）在同一闭环内实现，
因为答案 DTO 是 agent 收尾的产物，分开交付无法自证。

新增：

- `src/code_understanding/agent.rs`：单问闭环。首轮组装 system 提示 +
  问题；每轮流式请求（复用 `ChatClient::request_agent_stream` 与评审 agent
  的瞬态重试/退避模式）；tool_calls 逐个执行并按 `tool_call_id` 回填；
  触顶强制收尾；最终正文解析 + 校验，失败最多修复一次；取消在轮次边界生效。
- `src/code_understanding/analysis.rs`：会话级 `AnalysisResult`（独立
  `version=1`，不复用 `ANSWER_PROTOCOL_VERSION`）、JSON 解析（剥代码围栏、
  容忍前后说明文字、半截 JSON 直接失败）、来源/结构校验、system 提示与
  JSON schema（提示词与校验常量同源）。
- `src/tests/code_understanding_analysis.rs`、`src/tests/code_understanding_agent.rs`。

修改：

- `src/code_understanding/mod.rs`：导出两个新模块。
- `src/code_understanding/tools.rs`：新增 `generation()` / `issued_sources()`
  访问器；六个参数结构体改为逐字段 `#[serde(default)]`（字段级默认值不会给
  结构体强加 `Default` 约束，此前用结构体级 `#[serde(default)]` 会让
  `Deserialize` 要求 `Default`，而工具参数只需缺省可选字段）。
- `src/code_understanding/source.rs`：新增 `issued_sources()`（按路径与起始行
  排序，供检索范围摘要）。
- `src/ai/client.rs`：新增 `ChatClient::max_tokens()`；把
  `AgentStreamError::{transient,transient_once,fatal}` 提为 `pub`，使自定义
  `UnderstandingTurnProvider`（含测试）能构造错误。评审 agent 未受影响。

## 复用接口

| 能力 | 复用 |
| --- | --- |
| AI 传输 | `ai::ChatClient::request_agent_stream`（SSE、工具协议、读空闲超时、截断检测） |
| 错误文案 | `classify_agent_http_error`、`agent_request_error`、`AgentStreamError::retryable/retry_ceiling` |
| agent 循环形态 | `ai::review_agent` 的轮次/预算/收尾/重试组织 |
| 工具与来源 | `code_understanding::tools` 六工具 + `source::SourceService`（T1） |
| 索引查询 | `code_index::{search_symbols_filtered,symbol_detail,trace_calls,open_read_only_if_exists}` |
| 结果截断 | `ai::review_agent::truncate_result_chars` |
| 文本清洗 | `ai::merge::strip_code_fence` |

## 会话答案结构要点

- 身份字段（`context.project_key/request_id/index_generation/question`）由服务
  覆盖写入，模型只能填 `scope`（实际检索范围）。
- `source_ids` 必须是本会话工具发放的 `sr1:…`；每个 finding / caller / callee /
  data_access / step 至少一个来源。校验按 ID 去重，避免同一来源重复读文件。
- `data_accesses` 区分 `db_object/cache/external/message/unknown`，操作只接受
  `read/insert/update/delete/unknown`；对象类别未知时 `state` 必须为 `unknown`
  （不得把未知对象当已确认表）。
- `links` 端点必须在 `steps` 内；`steps` ≤ 15、`links` ≤ 20。
- 校验只判结构与来源归属，不判语义；语义正确性由人工按引用内容核对
  （design.md §7「校验两层分开」）。

## 预算（集中常量，无用户旋钮）

| 项 | 值 |
| --- | --- |
| 工具次数 | 40 |
| 模型轮次 | 41（工具轮最多等于调用次数，另留收尾/格式修复） |
| 工具结果累计 | 120K 字符 |
| 单条工具结果 | 8K 字符 |
| HTTP 尝试（含瞬态重试） | 80 |
| 格式修复 | 1 次 |
| 输出 token | 沿用客户端 `AiProviderSettings.max_tokens`，不新增旋钮 |

触顶时先注入「预算已用尽（指明具体限额），立即输出 JSON、无法确认写 unknowns、
completion 标 partial」的 user 指令并省略 tools；收尾轮仍吐 tool_calls 时，
正文能解析就宽容接受，否则报 `BudgetExceeded` 且文案指名触顶限额并说明「已读取
的来源仍可查看」。同一轮批量调用逐项检查额度；重复 `tool_call_id` 不重复执行，
但仍回填配对消息。

## 测试命令与结果

- `cargo test --lib code_understanding --no-fail-fast`：52 passed，0 failed，
  540 filtered out。
- `cargo test --lib --no-fail-fast`：590 passed，0 failed，2 ignored。
- `cargo check --all-targets`：成功，零警告。
- `cargo build --release`：成功，零警告。
- `cargo clippy --lib --tests`：本批新增文件无提示（仓库既有 clippy 提示位于
  非本批代码）。
- `git diff --check`：通过。

测试覆盖：B01 普通 Java 登录闭环（真实索引 + 真实源码工具，七个工具调用，
答案引用真实 `sr1:` 来源）；缺索引边时 `search_code + read_file` 补查并标
partial；预算触顶；重复 `tool_call_id`；格式修复成功/失败；伪造来源拒绝；
未知对象类别标 observed 被拒；悬空 link 被拒；轮次边界取消与退避期取消；
瞬态重试后成功；重试耗尽报尝试次数;不支持工具调用端点分流；缺索引直接报错。

## 已知限制与下一步

- **脚本化模型不等于真实理解**。测试用脚本 provider 驱动真实索引与工具，能证明
  工具中介、来源绑定、预算与生命周期正确，不能证明模型能理解项目。开发文档 §7
  的真实模型业务验收（B01～B03 各两遍）尚未执行；本批未连接任何真实供应商。
- **B02～B05 样例未建**（Spring + MyBatis、Spring + JPA、缺边、动态表边界），
  属 T3 剩余验收对象。
- **无 UI**：`run_understanding_agent` 可直接调用，但还没有问题输入、回答主区、
  来源侧栏与步骤简图；接入 AI 池（`TaskKind::Ai`）与并发许可、事件落 UI 是 T5。
  本轮未在 `main.rs`/`ui` 侧接线，避免与 T5 页面重构冲突。
- **追问与生命周期（T4）未实现**：会话历史摘要、最近 3 次答案回灌、切换项目/
  离页的旧消息隔离、来源点击时的 hash 复验提示，都留给 T4；当前每问独立，
  校验时已做来源 hash 复验。
- 符号搜索的 path/language 过滤仍是「前 1000 条 FTS 候选内过滤」（T1 限制未变）。
- `UnderstandingTools::open` 要求索引已存在，否则返回 `IndexMissing`；「无索引
  时引导建索引」的交互属于 T5 页面职责。

下一步：T4（追问与生命周期）或先补 B02/B03 样例跑真实模型验收；两者都不需要
先扩静态分析器。
