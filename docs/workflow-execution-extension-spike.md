# 工作流 MCP/JS 执行扩展技术验证

验证日期：2026-09-28。验证环境：Windows、`rustc 1.96.0`、release profile。该记录是 P5 接入前的选型验证；后续正式实现与使用方式见 [工作流使用说明](workflows.md)。

## 已验证

| 项目 | 结果 |
| --- | --- |
| MCP SDK | 隔离 Cargo 样例用 `rmcp 3.5.0`，关闭默认 feature，仅启用 `client` 与 `transport-child-process`；`TokioChildProcess::new` 在 Windows release 编译通过。SDK 需要 Tokio；客户端现有工作流执行器是同步接口，接入时需在外部执行任务中管理异步运行时。 |
| JS 引擎 | 隔离样例用 `rquickjs 0.14.0`，Windows release 编译并运行通过；完成表达式求值、8 MiB 内存上限、512 KiB 栈上限及中断回调终止无限循环。 |
| 样例体积 | 隔离样例的 release 可执行文件为 1,454,080 字节；它没有执行 MCP 握手，链接器可能移除未调用的 SDK 代码，因此此数字不能当作正式客户端增量。 |
| 现有执行接口 | P4b 的 `WorkflowRunControl` 可以传给扩展动作；动作可在执行中检查取消。取消 Git 内置步骤只在步骤边界生效。 |

验证样例放在本地忽略目录 `target/workflow-runtime-spike`，不属于发布产物。依赖版本与功能说明分别见 [官方 Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) 和 [Rquickjs](https://github.com/DelSkayn/rquickjs)；Rquickjs 上游将 Windows MSVC 支持标为实验性，本机通过一次编译和运行仍需后续持续构建验证。

## P5 接入方案

1. 先实现本地 stdio MCP 客户端：显式配置服务程序和参数，连接后握手、分页列工具并验证输入 schema。工具调用与服务进程的生命周期归外部任务管理，结果经带仓库、运行 ID 和模板代际的工作流事件返回。
2. 外部工具默认无执行权限。运行前展示将连接的服务、工具、参数摘要及读写能力；授权后调用。敏感字段不进入普通日志，非幂等写工具失败后不自动重试。
3. JS 运行时使用独立后台任务，每次运行创建受限上下文，设置内存、栈和执行时限。只向脚本提供明确授权的宿主动作/MCP 桥接，不提供任意文件系统、进程或网络 API。
4. MCP 调用和 JS 脚本都以 `WorkflowRunControl` 为取消信号；外部适配器必须主动终止或等待当前调用结束，再返回明确的取消或失败结果。步骤输出继续通过 `saveAs` 传给后续步骤。

## 接入前仍需验证

- 用本地测试 MCP 服务完成真实握手、工具列表、只读调用、受控写调用、取消与服务退出；本次只完成 SDK 子进程客户端的编译检查。
- 验证 HTTP 传输与应用代理配置、凭据处理和超时；本次没有启用 HTTP feature。
- 将 JS/MCP 桥接装入客户端后测量 Windows release 实际体积、长任务池占用、并发和窗口关闭时的收尾；隔离样例体积不能推算这些结果。
- 用真实 Edge MCP 服务验证页面识别、点击和填写，这属于 P6 样板验收。
