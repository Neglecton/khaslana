# CU2-T4 追问与生命周期交接

日期：2026-09-10。状态：服务层完成（无 UI）；真实模型业务验收与 T5 页面未开始。

## 实际修改

- 新增 `src/code_understanding/session.rs`：单项目、单标签会话；保留最近 5 次完成答案，
  向模型回灌最近 3 次摘要，历史文本总量严格限制为 8000 字符。
- 请求路由键包含项目键、标签会话键和请求代际。取消或离页后立即拒收事件，后台任务
  实际退出前仍占用 active，避免释放并发许可后旧任务继续运行。
- 会话状态覆盖 Idle、Running、Completed、Partial、Failed、Cancelled、Stale；
  完成答案只在路由身份匹配且会话仍接收事件时进入历史。
- `run_understanding_agent_with_context` 接收冻结的追问上下文；历史摘要只作待核对背景，
  明确不是指令。选中源码只作聚焦线索，要求模型重新调用只读工具核对。
- `UnderstandingAnswer` 保存本次实际发放并完成校验的 `SourceRef`。历史来源点击通过
  `validate_history_source` 重新核对项目、索引代际、允许路径、文件 hash 和范围；
  文件或索引变化后拒绝打开旧引用。
- `UnderstandingSessionEvent` 为每个 agent 事件附加完整请求路由身份，供 T5 接线时
  丢弃其他项目、其他标签或旧代际的迟到消息。

本批只修改代码理解服务、测试与交接文档，没有修改 `main.rs`、视图模块或页面状态。

## 自动验证

- `cargo test --target-dir target-t4 --lib code_understanding --no-fail-fast`：
  62 passed，0 failed，7 ignored，540 filtered out。
- `cargo test --target-dir target-t4 --lib --no-fail-fast`：
  600 passed，0 failed，9 ignored。
- `cargo test --target-dir target-t4 --bin khaslana --no-fail-fast`：
  303 passed，0 failed。
- `cargo check --all-targets`：成功，零警告。
- 本批文件 `git diff --check`：通过。

专项测试覆盖：历史保留上限、最近三问顺序、8000 字符预算、取消后等待任务退出、
离页后的迟到事件与完成答案丢弃、项目/标签/代际错配拒绝、历史来源成功复验，以及
源码变化后的 `SourceChanged`。

## 已知限制与下一步

- 会话仅驻内存，不持久化答案；这是首版既定范围。
- T4 提供生命周期状态和路由契约，尚未接入 AI 线程池或现有应用事件；该接线属于 T5。
- B01～B03 的真实模型双跑仍未完成，当前通过的是脚本 provider + 真实索引/源码工具。
- B02～B05 的完整样例矩阵仍是剩余验收工作，不以继续扩展静态分析器为前置条件。

下一步建议：先执行 B01～B03 真实模型验收并记录语义偏差；用户确认开始页面后，再用本批
会话状态、路由键和事件包装接入 T5 原生页面。
