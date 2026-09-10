# CU2-T0 复用盘点与最小样例

日期：2026-09-10。状态：完成；已进入 CU2-T1。

## 开工基线

- HEAD：`59de3e4a0f05b5af5ac7a31142f9e43f5cf93065`（`dev_lcc`，相对
  `origin/dev_lcc` ahead 1）。
- 开工时工作区已有未提交的 V2 文档调整，以及 `src/code_index/` Java 提取、
  解析、事实、图、管线、存储和旧 Java fixture 修改；另有
  `java/resolve.rs`、`java_resolve.rs` 等未跟踪文件。本批将其视为保留资产，
  未删除、回退或整体格式化。
- codebase-memory-mcp 已以 fast 模式刷新：5846 nodes / 25493 edges。
- 开工基线 `cargo test --lib code_understanding --no-fail-fast`：18 passed，
  0 failed，578 filtered out。

## 可复用接口映射

| V2 需要 | 当前接口 | 结论 |
| --- | --- | --- |
| 符号候选 | `code_index::search_symbols_filtered` | 可直接作导航；已有 FTS、路径和行号，V2 在会话层补范围过滤及候选 ID |
| 符号详情 | `code_index::symbol_detail` | 可复用定义与一跳关系；旧源码读取不具备 V2 来源注册语义，因此只取元数据，正文改走安全 reader |
| 调用上下游 | `code_index::trace_calls` | 可复用；当前每次载入全图，首版先加深度/数量上限，性能实测不达标再局部改 SQL |
| 文件身份 | `code_index::SourceRef`、`content_fingerprint`、索引 `generation` | 可复用 hash/代际；V2 另加会话发放的 `source_id`，拒绝模型自行拼路径与行号 |
| 项目身份 | `code_index::ProjectContext`、`ai::review_store::repo_key` | 可复用；打开工具会话时核对项目根、索引记录路径和项目键 |
| 严格答案 | `code_understanding::AnswerDocument/GraphSlice/EvidenceBundle` | 保持原语义，只接受索引权威关系；不承载 AI 会话发现 |
| AI 传输 | `ai::ChatClient::request_agent_stream`、`ToolSchema` | T2 复用 SSE、重试、截断与工具协议；T1 先交付独立工具 schema 与 Rust 服务 |
| agent 循环 | `ai::review_agent::run_review_agent` | 参考轮次、批量调用和收尾守卫；代码理解使用独立白名单，不复用 Git/diff 工具 |
| 后台执行 | `tasks::TaskExecutor` 的 `TaskKind::Ai` | T2 复用 AI 池；并发许可需与现有 AI 任务总量统一，不能另开无上限入口 |

## 已确认边界

- `AnswerDocument` 不能通过“允许 AI 自报关系”来适配 V2；后续新增独立
  `AnalysisResult`，结构与来源校验和语义判断分层。
- 现有 `symbol_detail` 的源码片段读取只做体积/行数钳制，不校验项目 allowlist、
  根外链接或来源 hash；V2 工具不能直接暴露这条读取路径。
- `discover_files` 已提供 gitignore、固定排除目录/后缀和子模块剪枝，可作为每问
  允许文件清单；XML/SQL 等无符号文本仍会进入清单。
- 当前 Java 增量含 Java 时转全量、`field_access` 体积较大。它们不阻塞 T1；
  若真实工具延迟或资源基准超限，再做最小修复。

## 最小业务样例

新增 `src/tests/fixtures/code_understanding_business/B01`：普通 Java 账号密码登录。
样例包含 Controller 与另一个直接调用方、核心 AuthService、明确 JDBC SQL、
用户不存在/密码错误/成功三类分支、`app_user` 读写、`login_attempt` 插入和
内存会话副作用。`expected.json` 保存人工事实范围与未知边界，不保存模型答案。

固定主问题：

1. 登录逻辑怎么实现的？调用了什么，被什么调用？读写哪些表？
2. 密码错误时会发生什么，会写入什么数据？
3. 谁还调用了这个认证服务？这里的会话怎么创建？

后续 B02/B03/B04/B05 仍属于 T3 业务答案验收，不把补齐静态 Java/Spring 规则
设为 T1/T2 前置。

## 下一步

CU2-T1 已开始：`source.rs` 提供安全文件清单、有限读取、全文检索、目录树与
来源失效检查；`tools.rs` 适配六个只读工具。完成其测试和审查后进入 T2 agent
循环，让真实问题在无 UI 条件下跑通 B01。
