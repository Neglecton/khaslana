# JLS-T1 公共 LSP 客户端与 JDT 插件适配交接

日期：2026-09-13。状态：核心完成，可进入 JLS-T2。这里的“完成”指手动环境下公共协议、官方静态插件描述、JDT provider、统一语义服务和真实查询门槛通过；不表示 AI 接线、原生 UI、正式引擎包或一键安装已经完成。

## 1. 本批交付

- 新增 `src/lsp/`，公共协议、客户端、进程、插件注册、provider 与语义服务分层；`src/lib.rs` 只增加模块导出。
- `protocol.rs` 实现 Content-Length 帧、8 MiB 分配前上限、分片/连帧读取、LSP 零起始位置与源码一基 Unicode 标量列转换、UTF-8/16/32 编码和 Windows/中文/空格 file URI。
- `client.rs` 使用独立读线程和串行写锁，按数值 ID 配对请求；支持乱序回复、RPC 错误、超时、`$/cancelRequest`、迟到回复丢弃、通知及反向请求。只读策略处理 configuration、progress、watched-files 注册/撤销和 showMessage，明确拒绝 applyEdit，未实现动态能力返回错误。
- `process.rs` 只持有本次启动的 Child。Windows 隐藏窗口并把进程放入带 `KILL_ON_JOB_CLOSE` 的 Job Object，shutdown/exit 超时后只终止这个进程树，不按 `java.exe` 名称清理。
- `plugins.rs` 只编译注册 `java-jdtls`：descriptor schema 1、adapter `jdtls`、Java、stdio 和五类操作。注册表拒绝未知 schema/adapter/ID、重复 ID 与语言冲突；有效能力是描述、适配器和服务器能力的交集。T0 snapshot 只记为 development compatibility，不进入托管包清单。
- `providers/jdtls.rs` 校验 JDK/JDT 基本布局，生成固定参数数组，不接受 shell 模板；服务 JDK、项目 runtime 映射和 JDT `-data` 分离。每项目独立 workspace，并设置私有 `GRADLE_USER_HOME`，清除会把 stdio 切到 socket 的环境变量。
- `service.rs` 提供项目配置、消费者 acquire/release、状态、启动、来源锚点、documentSymbol 辅助定位、五类查询、文件变化和空闲回收。单实例只持有一个活动 JDT，队列 16、查询 3 秒、每操作最多 6 RPC、结果 1～40 条、缓存每项目 128 项/16 MiB。

## 2. 统一查询与来源契约

公开查询只接受服务发放的 `sa1:` 锚点和 `SemanticOperation`，不接受 URI、LSP 方法或启动参数。AI 路径应调用 `register_source_anchor` / `register_source_symbol_anchors`：它们先通过现有 `SourceService` 校验本会话 `sr1:` 来源、项目、generation、文件 hash 和已发放字节范围；低层已验证 SourceRef 入口仅为 crate 内部测试/适配使用。

结果包含 project/request、session epoch、workspace revision、provider、plugin ID、精确 engine version、operation、availability/coverage、截断原因和位置。调用层级项分别保存 caller、callee、call_site；prepare 返回的完整 opaque item原样送回后续请求。项目内位置必须再次通过发现白名单和 canonical 根路径校验；项目外/JDT 虚拟 URI只保留不可读提示，不成为源码读取入口。

文件打开由服务统一管理 didOpen/didChange 与版本。文件变化递增 workspace revision、清锚点/缓存并发送 didChangeWatchedFiles；构建配置变化复位 ready，等待新的 ServiceReady。publishDiagnostics 有 error 时状态/结果标 partial，空结果不解释为“不存在”。禁用插件会取消后续使用、清缓存并有界关闭自有进程，包状态与项目语义状态分开。

## 3. 真实 JDT 验证

环境沿用 T0 手动开发路径：

- 服务 JDK：`D:\khaslana\jdk-21.0.12.1+1`，Temurin 21.0.12.1+1。
- JDT LS：`D:\khaslana\jdt-language-server`，core `1.61.0.202609031315`，服务自报 `1.61.0-SNAPSHOT`。这是 development snapshot，不是 T4 托管版本。
- 样例：`src/tests/fixtures/java_semantic/maven-multi` 的临时副本；JDT workspace 和工程元数据都写入测试临时目录。

显式运行：

```powershell
cargo test --lib lsp::tests::real_jdt_service_resolves_cross_module_definition -- --ignored --nocapture
```

实际通过：

- `LoginController` 跨模块调用定义到 `LoginApplicationService.java:13`。
- `LoginApplicationService.login(String)` 引用 3 个，不串入无关 `AdminLoginController.login`。
- `AuthProvider.login` 实现候选 2 个。
- service login 入调用 2 个，出调用 1 个；每项都有 caller/callee/call_site。
- documentSymbol 把 `LoginController.java:13` 精确定位到方法名第 24 列。
- 修改临时源码并发送 watched-files 后，旧锚点失效，新结果 revision 增长；同项目两个消费者共享会话。
- 测试结束后 `Get-CimInstance Win32_Process -Filter "Name='java.exe'"` 返回空，无本批 Java/Gradle 残留。

## 4. 自动测试与构建

- `cargo test --lib lsp::tests -- --nocapture`：12 通过，0 失败，1 个真实 JDT 测试默认 ignored。
- 上述真实 JDT 测试：1 通过，覆盖五类查询、documentSymbol 和文件变化。
- `cargo test --lib`：619 通过，0 失败，13 ignored。
- `cargo test --bin khaslana`：303 通过，0 失败。
- `cargo build --release`：成功，0 warning。
- 新增文件逐个 `rustfmt --edition 2024 --check` 通过；`git diff --check` 通过。全仓库 `cargo clippy -- -D warnings` 仍会命中本批之外已有的 clippy 项，本批未修改这些文件。

协议测试覆盖分片/连帧、缺失长度、超大帧、乱序响应、服务端反向请求、配置 section、动态 watcher、只读 applyEdit、取消与迟到回复、UTF-16 emoji/中文/CRLF、file URI、插件冲突、能力交集、来源伪造/变化和精确子进程回收。

## 5. 明确保留的限制

- 只有用户给出的 JDK 21 路径可实机验证。`JdtRuntime` 已支持把项目 JDK 8/17/21 映射与服务 JDK 分开，单测验证生成配置，但独立 JDK 8/17 路径未提供，不能宣称对应安装组合已实测。
- T1 不下载、不安装、不修改系统环境。T4 必须换成固定官方正式发行包、URL 和 SHA-256，再复跑核心查询后才能写托管清单；当前 snapshot 的 `engine_packages` 保持空。
- 本批没有 AI/模型调用；`query_java_semantics`、`trace_calls` 可选增强、40 RPC/问预算及开关对照属于 JLS-T2。
- 原生源码动作、设置卡和安装状态 UI 属于 T3/T5。Lombok/生成源码、反射/AOP、缺依赖和离线项目仍应按 partial/unknown 展示，不能把 JDT 空结果当否定证明。

## 6. 下一步

按开发状态进入 JLS-T2：在现有 `UnderstandingTools`/agent/session 上接 `LspSemanticService`，保留 `query_java_semantics` 单一工具名、基础 `trace_calls` 结果和既有来源校验；先完成协议/预算/降级测试，再做同模型同问题开关各两遍的业务对照。不要在 T2 引入第二套 agent、动态插件加载或安装器。
