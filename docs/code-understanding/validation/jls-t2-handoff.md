# JLS-T2 AI 接线与业务对照交接

日期：2026-09-13。状态：核心完成，可进入 JLS-T3；若 CU2-T5 基础源码视图接口尚未就绪，可按开发文档先做 JLS-T4。基线提交为 `6f058b8d63a833a3a7ecef6a1c374494ebd96317`，本批继续使用工作区中的 JLS-T0/T1 未提交成果，没有清理或回退其他任务修改。

## 1. 本批交付

- `src/code_understanding/tools.rs` 在六个基础只读工具之外增加可选 `query_java_semantics`。参数只接受五种 `operation` 与 tagged anchor：本问 `search_symbols` 发放的 `sc1:` candidate，或本问 `read_file/get_symbol` 发放的 `sr1:` source + 一基 Unicode 标量行列；limit 必须为 1～40。
- 候选注册表保存名称、Java 路径、索引行及已经由 `documentSymbol.selectionRange` 核准的 `sa1:` 锚点。候选首次查询不以行首或同名字符串猜列；零候选、多候选、重载歧义和源码变化均明确返回 partial/unavailable 指引。
- 语义结果继续保留 `provider=jdtls`、`plugin_id=java-jdtls`、引擎版本、epoch/revision、coverage 和截断原因；项目内位置通过 `SourceService` 发放 `source_links`，未索引符号也不创建 `sc1:` 或写回 SQLite。根外依赖仅保留底层外部 URI 提示。
- `trace_calls` 的原参数和基础 callers/callees 不变；Java 增强可用时新增可选 `semantic`，固定 `depth=1`，分别保存 inbound/outbound、caller/callee/call_site 与来源。基础索引和 JDT 结果不互相覆盖，响应明确要求冲突时读取调用位置。
- `src/code_understanding/agent.rs` 复用原 agent 循环，增加 `run_understanding_agent_with_semantics` 和带会话版本。工具列表在一问开始前冻结；调用方只能传入已配置的 `LspSemanticService`，agent 不安装、不启动、不等待、不授权也不重配 JDT。
- 系统提示补充语义核对规则：JDT 静态位置不证明 Spring 注入、AOP/反射运行路径、SQL 或物理表读写，仍需沿 Mapper/XML/SQL/实体映射读取来源。
- `LspSemanticService::query_with_rpc_budget` 接受上层 RPC 额度，结果记录实际 `rpc_count` 和 `cache_hit`。单语义工具最多 6 RPC，单问最多 40 RPC；candidate 精确定位的 `documentSymbol` 也计入。缓存命中仍计 agent 工具次数和结果字符，但服务 RPC 为 0。
- 语义服务连续不可用时仅第一条返回具体原因，后续返回一次性降级提示，避免模型在同一问重复耗尽预算；基础工具、40 工具/8K 单结果/120K 累计字符预算保持原样。
- Windows Job Object 所有者增加 `Send` 保证：Job HANDLE 仍由单一 `OwnedJob` 独占并只关闭一次，使统一语义服务可以由后台 AI 任务通过 `Arc` 持有；没有扩大进程清理范围。

## 2. 接口示例

```rust
let answer = run_understanding_agent_with_semantics(
    &input,
    &provider,
    &cancel,
    semantic_service,
    &mut on_event,
)?;
```

模型可见的新增工具仍只有一个：

```json
{
  "operation": "definition",
  "anchor": {
    "kind": "source",
    "source_id": "sr1:...",
    "line": 14,
    "column": 24
  },
  "limit": 20
}
```

项目没有在本问开始时启用增强时，仍只暴露原六工具；服务在问答中途禁用、变为 importing/partial/unavailable 或能力变化时，不修改已冻结 schema，只在响应中说明并继续基础路径。

## 3. 自动与真实 JDT 验证

- `cargo test --lib code_understanding -- --nocapture`：本批开发过程中执行，新增两项语义工具测试通过；最终全量 lib 见下节。
- `cargo test --lib lsp::tests -- --nocapture`：12 passed，0 failed，1 ignored。
- `cargo test --lib lsp::tests::real_jdt_service_resolves_cross_module_definition -- --ignored --nocapture`：1 passed，使用 `D:\khaslana\jdk-21.0.12.1+1` 与 `D:\khaslana\jdt-language-server`，约 8.39 秒。
- 真实测试除了 T1 五类底层查询，还建立真实代码索引并从 `UnderstandingTools` 的 `sr1:` call-site 查询跨模块 definition；目标仍精确落在 `LoginApplicationService.java:13`。`trace_calls(depth=3,both)` 的语义部分固定为一跳，真实返回非空 inbound/outbound 和可发放来源。
- 新增 fake backend 测试覆盖：tagged anchor、候选精确定位、来源白名单、limit、冻结 schema、禁用变化、连续失败抑制、6/40 RPC 守卫及 trace 双方向分配。
- 四遍实况和真实 JDT 结束后检查未发现本批 JDT `java.exe` 残留。

## 4. 同模型开/关对照

问题均为“登录逻辑怎么实现的？调用了什么，被什么调用？读写哪些表？”。四遍均使用 `gpt-5.6-luna`、temperature 0.3、max_tokens 4000、同一 40 工具/8K 单结果/120K 累计字符预算。独立期望清单检查 `LoginController`、`AdminLoginProbe`、`app_user`、`login_attempt` 及会话不是数据库表；四遍全部通过。

| 模式 | 遍次 | 耗时 | 工具调用 | 独立 `query_java_semantics` | 含 semantic 的 `trace_calls` | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 关闭 | 1 | 151171 ms | 17 | 0 | 0 | complete，全部期望通过 |
| 关闭 | 2 | 152813 ms | 20 | 0 | 0 | complete，全部期望通过 |
| 开启 | 1 | 157159 ms | 19 | 2 | 4 | partial，全部期望通过；对运行时入口/事务边界保守 |
| 开启 | 2 | 164174 ms | 17 | 1 | 3 | complete，全部期望通过 |

开启两遍都实际使用 JDT，而非只暴露工具名；模型核对了精确调用/引用位置后继续读取 Java/JDBC 源码，没有把语义候选当作 Spring 运行时实现，也没有把内存 Session 当数据库表。增强在这个小样例上没有稳定减少总工具数或耗时，价值主要体现为可核对的 call-site/定义/引用位置；不把答案更长或 completion=complete 当收益指标。

实况原始记录：`live-runs/JLS-T2-B01-off-run1.json`、`off-run2.json`、`on-run1.json`、`on-run2.json`。当前流式客户端不返回 provider token usage，所以 `model_input_tokens` 明确为 null；报告保留每个工具结果的前 400 字符，记录的下界分别为 6210、7550、7579、6779 字符。测试代码已增加完整 `tool_result_chars` 统计，后续重跑会直接写入。

## 5. 最终回归

- `cargo test --lib --quiet`：621 passed，0 failed，17 ignored。17 项包含既有 10 个 CU2 实况、4 个本批开/关实况、真实 JDT 等需要显式环境的测试。
- `cargo test --bin khaslana --quiet`：303 passed，0 failed。
- `cargo build --release`：成功，约 1 分 22 秒，零编译警告。
- `cargo clippy --lib --no-deps -- -D warnings`：仍被本批之外已有 22 个 lint 阻断，位置在 `ai/client.rs`、`code_index/*`、`credentials.rs`、`git*`、`storage.rs`、`syntax.rs`、`update.rs`；本批文件未出现在报告中，未顺手格式化或修改这些代码。

## 6. 保留限制与下一步

- T2 只接服务和 AI；没有原生源码动作、设置卡、安装/更新/卸载。当前手动路径仍不是最终用户交付。
- B01 是普通 Java 业务样例，真实模型对照证明来源与业务边界没有退化；Spring+MyBatis/JPA 的 B02/B03 基础模式已有历史双跑，本批没有再各做四遍增强对照。T3/T5 最终矩阵仍需覆盖 UI 与插件安装后的项目状态。
- 输入 token 数因流式 provider 不返回 usage 无法补造；现有字符下界和全部工具轨迹已保留。
- T0/T1 保留的独立 JDK 8/17 实机路径、Lombok/生成源码、反射/AOP 与缺依赖限制不因 T2 完成而消失。

下一步按状态进入 JLS-T3：先核对 CU2-T5 的实际原生源码视图接口，再接 UiEvent、能力动作和关系侧栏；若该基础 UI 尚未实现，按开发文档允许的顺序先做 JLS-T4 引擎包与运行环境服务，不另建 HTML 页面或第二套语义服务。
