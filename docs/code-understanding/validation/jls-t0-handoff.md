# JLS-T0 真实技术验证交接

任务号：JLS-T0。状态：**核心完成，可进入 JLS-T1**。日期：2026-09-11。

这里的“完成”仅表示真实 JDT LS 的核心语义收益门槛通过，不表示语义服务、AI 接线、原生 UI 或安装能力已经完成。JLS-T1～T5仍未开始。

## 1. 基线与环境

- 代码基线：`6f058b8d63a833a3a7ecef6a1c374494ebd96317`，分支 `dev_lcc`；验证在包含本批未提交 fixture/探针的工作区执行。
- 主机：Microsoft Windows 10 专业版 10.0.19045，x64；AMD Ryzen 5 5600，6核/12线程；31.9 GiB 内存。
- 服务 JDK：Eclipse Temurin 21.0.12.1+1，`D:\khaslana\jdk-21.0.12.1+1`；`java` 与 `javac` 均实测为 21.0.12.1。
- JDK 原始归档：`OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip`，205,073,461字节，SHA-256 `f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e`。官方固定下载 URL：`https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip`。
- JDT LS：实际启动日志自报 `1.61.0-SNAPSHOT`、OSGi `1.61.0.202609031315`、Git commit `08eafe6`；路径 `D:\khaslana\jdt-language-server`。Equinox launcher 为 `1.8.0.v20260804-1928`。
- JDT 原始归档：`jdt-language-server-1.61.0-202609031315 (1).tar.gz`，51,037,522字节，SHA-256 `338e7e73d61836651ba2453919a0d34fa763eb4e7c03342092309bffb8934c64`；官方 snapshot repository 同日列出 OSGi build `1.61.0.202609031315`。

JDT 包有一个必须保留的发行限制：本地包是带时间戳、可按散列复现的 Eclipse snapshot，不是已发布的 1.60.0 milestone。用户提供目录没有保存原始下载 URL，因此本报告只证明“本地归档散列 + 官方 snapshot build id”一致，不能证明具体下载链路。JLS-T1可继续锁定该本地路径与散列；JLS-T4正式下载清单不得直接采用 `latest` snapshot URL，必须换成当时实际复测的固定 milestone/发行归档并核对官方校验来源。

官方依据：

- JDT LS README 要求服务至少使用 Java 21，并给出本批实际采用的 `-jar/-configuration/-data` 启动形状：<https://github.com/eclipse-jdtls/eclipse.jdt.ls#requirements>
- Eclipse 项目页确认 1.60.0 是 2026-09-03 正式发行：<https://projects.eclipse.org/projects/eclipse.jdt.ls/releases/1.60.0>
- Eclipse snapshot repository 记录本批 1.61.0 OSGi build：<https://download.eclipse.org/oomph/archive/download.eclipse/jdtls/snapshots/repository/index.html>
- Temurin 21.0.12.1+1 官方发行页：<https://github.com/adoptium/temurin21-binaries/releases/tag/jdk-21.0.12.1%2B1>

## 2. 交付物与复现

- 独立样例：`src/tests/fixtures/java_semantic/`，没有改动原 CU2 fixture。
- 期望清单：各样例目录的 `expectations.json`；核心 Maven 清单位于 `java_semantic/expectations.json`。
- 真实 stdio 探针：`docs/code-understanding/validation/jls_t0_probe.py`。协议编解码、请求 ID、通知、取消、shutdown/exit 只依赖 Python 标准库；若环境有 `psutil`，额外按100ms采样 JDT 进程树 RSS。
- 原始摘要证据：`jls-t0-{maven,plain,gradle,spring,java8,java21}-run.json`。内容只涉及合成项目、工具路径、能力、通知、查询结果摘要和资源数据。

核心 Maven 复现命令（应对 fixture 使用临时副本，避免把 JDT 元数据留在源码目录）：

```powershell
Copy-Item -LiteralPath 'src\tests\fixtures\java_semantic\maven-multi' -Destination 'target\jls-t0-project-maven' -Recurse
python docs\code-understanding\validation\jls_t0_probe.py `
  --jdk 'D:\khaslana\jdk-21.0.12.1+1' `
  --jdt 'D:\khaslana\jdt-language-server' `
  --project 'target\jls-t0-project-maven' `
  --workspace 'target\jls-t0-workspace-maven' `
  --expectations 'src\tests\fixtures\java_semantic\expectations.json' `
  --output 'docs\code-understanding\validation\jls-t0-maven-run.json'
```

探针实际启动参数为独立 argv：绝对 `java.exe`、Eclipse application/product/defaultStartLevel、`-Dlog.level=ALL`、`-Xmx1G`、官方 README 要求的 module/open 参数、唯一 launcher、`config_win`、每项目独立 `-data`。连接使用 stdio `Content-Length` framing，没有设置 `CLIENT_PORT`/`CLIENT_HOST`。

初始化固定项：`workspaceFolders/rootUri` 指向单一项目；`java.autobuild.enabled=false`；Maven/Gradle importer开启；`java.configuration.runtimes` 显式把服务 JDK 作为 `JavaSE-21`。本批未收到服务端反向 request，只收到通知；T1仍须按设计实现 `workspace/configuration`、动态注册、progress及拒绝 `workspace/applyEdit`，不能据此删掉协议分支。

## 3. 核心查询结果与基础索引对照

真实 Maven 三模块样例包含 API、core、web，共12个被基础索引发现的文件。五类查询全部通过：

| 查询 | 期望与实际 |
| --- | --- |
| 跨模块定义 | `LoginController.service.login(String)` 唯一定位到 core 的 `LoginApplicationService.login(String)` |
| 重载引用 | 目标声明 + `LoginController` + `BatchLoginJob` 共3处；没有串入 `AdminLoginController.login`，也没有串到双参数重载 |
| 接口实现 | `AuthProvider.login(String,char[])` 返回 `LocalAuthProvider`、`SsoAuthProvider` 正好2个候选，没有把接口声明当成已确定运行时实现 |
| 入调用 | `LoginApplicationService.login(String)` 返回 controller 的 `login(String)` 与 batch job 的 `run()` 共2个调用方，并带调用位置 |
| 出调用 | `LoginApplicationService.login(String)` 返回 API `AuthProvider.login(String,char[])` 共1个被调用方，并带调用位置 |

项目自身基础索引对照命令：

```powershell
cargo test --lib jls_t0_baseline_index_cannot_disambiguate_login_by_source_position -- --nocapture
```

实际1个测试通过。基础索引统计为12 files / 25 symbols / 48 nodes / 58 edges / 3 calls；裸名称 `login` 返回7个候选并进入 `Ambiguous`，没有源码位置参数可用于区分同名方法和重载。相同 fixture 上，JDT 按 URI + UTF-16 position 返回上述唯一目标或正确候选集。因此本批证明了至少一个真实增益：**源码位置驱动的同名、重载和跨模块类型消歧**，不是用 mock 或主观答案长度替代。

## 4. 样例与兼容矩阵

| 样例 | 项目目标/构建 | 真实结果 | 边界 |
| --- | --- | --- | --- |
| `maven-multi` | Maven，多模块，release 17 | 五类核心查询通过 | 无外部依赖 |
| `plain-java` | 普通源码/JDK，无构建文件 | 定义、引用通过 | JDT 使用 invisible project；项目目录零新增/修改 |
| `gradle-java` | Gradle 8.9，targetCompatibility 17 | 定义、引用通过 | 执行了 Gradle 配置/同步并启动 daemon |
| `spring-data` | Maven 17；Spring 6.2.11、MyBatis 3.5.19、Jakarta Persistence 3.2.0 | Mapper/JPA repository 定义定位通过 | 只证明 Java 静态位置，不证明 Spring 运行时注入、SQL/表读写或 AOP |
| `maven-java8` | Maven release 8 | 定义通过 | 服务仍由 JDK 21 启动，项目源码级别没有改成21 |
| `maven-java21` | Maven release 21，含 record | 定义通过 | Windows x64实测 |

Gradle 首次失败样例同样有价值：原 fixture 请求 Java 17 toolchain，而机器只有显式提供的 JDK 21，Buildship 报 `NoToolchainAvailableException`，并说明未配置 toolchain download repository。随后把 fixture 改为 `sourceCompatibility/targetCompatibility=17`，由服务 JDK 21 编译目标17，导入和查询通过。产品必须把“服务 JDK”“项目目标版本”“项目指定 toolchain”分开；缺项目 toolchain 时应标 partial/依赖缺口，不能偷偷下载或把目标版本改成21。

未覆盖项不算通过：真实 JDK 8/17 安装路径映射、继承/lambda/方法引用、缺依赖/离线、Lombok/生成源码、反射/AOP、根外依赖 URI、中文/emoji/CRLF 位置换算和文件变化。这些转入 T1 真实/fake 分层测试与限制台账。T2仍需做真实模型开/关对照；本批 AI 调用数为0。

## 5. 性能、进程与写入副作用

所有 p50/p95 均为每个样例完成首次导入后的20次轮询式热查询；调用层级操作包含 prepare + hierarchy 两个 RPC。目标 p95≤3秒，本批均远低于目标。

| 样例 | initialize | 首次可查询 | 热查询 p50/p95 | JDT工作区 | 峰值RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Maven多模块 | 2.44s | 2.51s | 18.40/58.16ms | 40.96MiB | 972.0MiB（JDT + conhost） |
| 普通Java | 2.76s | 3.54s | 11.36/20.96ms | 40.88MiB | 本次早期运行未启用采样 |
| Gradle | 2.46s | 9.11s | 11.07/17.02ms | 40.89MiB | 1,310.7MiB（JDT + conhost + Gradle daemon） |
| Spring数据形状 | 2.48s | 4.05s | 5.57/10.73ms | 48.31MiB | 972.6MiB（JDT + conhost） |
| Java 8目标 | 2.66s | 3.61s | 3.40/5.73ms | 41.30MiB | 本次早期运行未启用采样 |
| Java 21目标 | 2.88s | 3.50s | 4.96/6.40ms | 40.91MiB | 本次早期运行未启用采样 |

资源结论：`-Xmx1G` 下小项目 JDT 工作集已接近1GiB；Gradle首次配置时进程树峰值约1.28GiB，T1不能把 Xmx 当成总RSS上限。工作区缓存即使对小样例也约41～48MiB。

导入副作用已通过导入前后 SHA-256 快照核对：

- Maven 多模块副本新增21个 Eclipse/M2E元数据文件，包括根/模块 `.project`、模块 `.classpath` 和 `.settings`；业务源码零修改。
- Gradle副本新增13个文件，包括 `.project/.classpath/.settings` 与项目内 `.gradle/8.9` 锁/缓存；会求值 `build.gradle` 并启动 Gradle 8.9 daemon。
- Spring Maven副本新增5个 Eclipse/M2E元数据文件；首次导入在用户 Maven repository 写入了 Spring/MyBatis/Jakarta 及传递依赖，实际时间戳与运行吻合，证明会联网/写全局依赖缓存。
- 普通Java项目目录零新增、零修改；状态写入独立 `-data` 工作区。
- 每次探针都收到 `shutdown` 响应，`exit` 后 JDT 进程退出码0。Gradle daemon不会随 JDT `shutdown/exit` 自动退出；本批检测到后按精确 PID 和命令行归属停止，最终没有残留 `java.exe`。T1必须明确构建子进程的所有权、复用和回收策略，不能只管理语言服务器主进程。
- 探针发出一次 `$/cancelRequest`；本批小查询均在取消生效前正常返回，因此只证明客户端可发取消和服务仍可关闭，不把它计作取消语义通过。迟到结果、强制回收和崩溃恢复属于T1。

本批未运行应用、测试任务、SQL或数据库连接；但 Maven/Gradle工程导入确实扩大到依赖下载、构建脚本配置和全局缓存写入，产品必须继续要求按项目授权。

## 6. 进入 T1 的结论

JLS-T0核心门槛逐项满足：真实进程、Maven导入、同名/重载、实现候选、跨模块、入/出调用、可核对位置、可发送取消、可正常关闭、基础索引真实缺口均有证据。因此允许开始 JLS-T1，不等待 UI 或安装器。

T1应直接复用/固定：

1. `LaunchSpec` 接收 JDK、JDT、launcher、config、data 的绝对路径；当前手动环境默认使用本报告版本和 SHA，不从 PATH 猜测。
2. 每项目独立且归属可核验的 `-data`；预期工程目录可能被 JDT/M2E/Buildship写元数据，启用前必须得到项目授权。
3. initialize设置、真实五操作名称、URI + UTF-16 position锚点、20/40结果预算。
4. Ready判定不能只看 initialize。本探针以代表性 workspace symbol 可查询且 progress 无活动作为验证近似；T1需结合锁定版本的 `language/status`、progress、diagnostics/import error形成明确 Ready/Partial。
5. Gradle缺toolchain要显式 Partial；Gradle daemon残留要纳入进程树策略。Spring依赖下载与 Maven全局缓存写入要在授权文案/日志中可解释。
6. JDT snapshot只能用于当前手动环境开发。T4清单必须补齐固定官方发行归档 URL、SHA、许可证与平台信息。

## 7. 本批测试

- `python -m py_compile docs/code-understanding/validation/jls_t0_probe.py`：通过。
- 六组真实 JDT probe：全部期望通过；不是 fake LSP。
- `cargo test --lib jls_t0_baseline_index_cannot_disambiguate_login_by_source_position -- --nocapture`：1 passed，618 filtered out。
- `cargo test --lib`：607 passed，0 failed，12 ignored（含本批新增基础索引对照测试）。
- `cargo test --bin khaslana`：303 passed，0 failed，0 ignored。
- `cargo build --release`：通过，0错误、0警告。

已知限制按性质分类：

- 实现缺陷：尚无产品内 JavaSemanticService；这是 JLS-T1 正常任务，不是 T0 探针缺陷。
- 环境/发行缺口：JDT 为 snapshot 且原始下载链路未保存；Java 8/17项目目标已测，但独立JDK 8/17路径映射未测。
- 后续验收：V02剩余继承/lambda/方法引用，V04～V07协议/变化/边界，V03真实模型业务对照，V08～V11安装/UI。它们没有被本报告写成通过。
