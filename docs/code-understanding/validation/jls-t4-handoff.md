# JLS-T4 插件引擎包与运行环境管理交接

任务号：JLS-T4。状态：**核心完成，可进入 JLS-T3/T5**。日期：2026-09-13。

发行通道说明：托管下载采用「CNB 镜像优先 + 官方源兜底」（与 update.rs 的应用更新多源模式一致）。
CNB 镜像仓库为 `https://cnb.cool/liuchenchen/LSP-Services`（main 分支根目录），由用户运维；
归档与官方逐字节一致（官方 SHA-256 全量校验强制，不匹配自动滑落官方源），不构成信任放宽。
实测 CNB 下载 255MB 全套组件 25.94s（此前 Eclipse 官方源国际线路仅 ~36KB/s，需 25 分钟以上且
存在间歇性 Connection refused/零数据超时——裸 TCP 与 curl 对照确认系网络环境而非代码问题）。

## 1. 实际修改

全部为新增（叠加在 JLS-T1/T2 的未提交工作区之上，未清理其他任务变更）：

- `src/lsp/install.rs`：引擎包安装器。
  - 内置固定清单 `EnginePackageSpec`（编译期常量，禁止模型/项目配置注入）：
    - `jdtls-1.60.0.202606262232-windows-x86_64`：Eclipse milestones 1.60.0 固定归档
      （`https://download.eclipse.org/jdtls/milestones/1.60.0/jdt-language-server-1.60.0-202606262232.tar.gz`，
      50,925,681 字节，SHA-256 `e94c303d8198f977930803582738771fd18c52c5492878410bf222b1aa81ef1d`，
      来自同目录官方 `.sha256` 伴随文件）；CNB 镜像
      `https://cnb.cool/liuchenchen/LSP-Services/-/git/raw/main/jdt-language-server-1.60.0-202606262232.tar.gz`；
    - `temurin-jdk-21.0.12.1+1-windows-x86_64`：Temurin JDK 21.0.12.1+1 官方 GitHub release
      归档（205,073,461 字节，SHA-256 `f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e`，
      JLS-T0 已实测核对）；CNB 镜像
      `https://cnb.cool/liuchenchen/LSP-Services/-/git/raw/main/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip`。
  - **多源下载**：`mirror_url`（CNB `liuchenchen/LSP-Services` 项目自有发行仓库）
    优先、官方源兜底；每个源独立执行「下载 → 实际大小核对 → SHA-256 核对」，镜像 404/网络失败/散列不匹配
    自然滑落官方源。域名白名单 `TRUSTED_ARCHIVE_HOSTS = {download.eclipse.org, github.com, cnb.cool}`，
    重定向白名单 `TRUSTED_REDIRECT_HOSTS = {objects.githubusercontent.com, release-assets.githubusercontent.com}`
    （后者为真实验证发现：GitHub release 资产现跳转 `release-assets`，非旧 `objects` 域；逐跳校验
    原样拒绝并要求补录，机制按设计工作）。重定向手动逐跳处理（ureq `max_redirects(0)`），
    HTTPS 强制（官方插件 spec），同源绝对路径 Location 解析。
  - 安装顺序固定：下载 `.part` → 大小核对 → 散列核对 → 有界解压到 `staging/<install-id>/extract/`
    → 定位包根（唯一顶层目录剥除）→ 入口验证（JDT：`plugins` + `config_win` + core jar 版本与清单一致；
    JDK：`bin`）→ 原子 rename 发布到版本目录 → 写所有权标记 `.khaslana-owned.json` → 原子更新
    `registry.json` 激活记录 → 清理 staging。失败保留已激活版本与旧目录。
  - 有界解压：路径安全化（拒绝绝对路径/盘符/`..`/NUL，`\` 归一为 `/`）、符号链接与硬链接拒绝、
    未知 tar 条目类型拒绝、条目数/单文件/展开总量三重限额（按实际写出字节计数，非归档头声明）、
    重复条目拒绝、tar 声明尺寸与实际展开字节核对。限额按真实包校准：Temurin 21 的
    `lib/modules` 约 140MB，单文件上限 256MiB（初始 64MiB 被真实安装拒绝后校准）。
  - 跨进程互斥：`staging/install.lock`（O_EXCL 原子创建，内容含 pid 与时间戳；
    超过 `stale_lock_after`（默认 2h）视为崩溃残留接管）。
  - 卸载保护：仅删除 registry 有记录、路径位于对应组件目录内且带本应用所有权标记的包；
    canonical 路径越界/标记缺失/`..` 一律拒绝（防误删用户 JDK）；OS 层占用（进程持有包内文件）
    时删除失败并转明确错误。运行中语义服务应先停止（T5 编排），再调用卸载。
  - 托管激活校验 `check_managed_activation`：引擎版本必须在兼容清单且 `development_build=false`
    （T0 的 1.61.0-SNAPSHOT 被显式排除），平台匹配、JDK 在兼容范围。
- `src/lsp/runtime/java.rs`：JDK 检测（设计 §6.3）。
  - 固定顺序：显式路径（无效即报错，不静默替换）→ Khaslana 托管私有 JDK → `JAVA_HOME` → `PATH`；
    只检查有限候选，PATH 候选排除当前项目目录内；同一路径按优先级去重。
  - 完整布局校验（`bin/java` + `bin/javac`）；限时 10s 探测
    `java -XshowSettings:properties -version`（子进程轮询 + 超时 kill），解析 `java.version`/
    `os.arch`（`amd64` 归一为 `x86_64`），主版本解析兼容 `1.8.0_x` 旧式与 `21.0.12` 新式。
  - 环境读取参数化为 `JdkDiscoveryEnv`（生产入口 `system_discovery_env()`），测试注入不触碰进程环境。
- `src/lsp/plugins.rs`：兼容清单新增正式发行条目 `1.60.0.202606262232`（`development_build: false`），
  保留 T0 snapshot 条目（`development_build: true`，仅手动开发模式）；`engine_packages` 填入两个
  内置清单 ID。
- `src/lsp/mod.rs`：导出 install/runtime API；测试模块挂载 `install_tests`。
- `Cargo.toml`：新增 `tar 0.4` 与显式 `flate2 1`（flate2 已是依赖树成员，无新增编译体量）；
  JDT LS 发行归档只有 tar.gz 形态，属开发文档允许的「确需支持 tar 才增加小型依赖」。
- `src/tests/lsp_install.rs`：38 个测试（见 §3）；`src/tests/lsp.rs` 的 `real_anchor`/`copy_tree`/
  `collect_named_files` 改为 `pub(super)` 供验收测试复用。

## 2. 目录布局（设计 §6.4）

`<数据目录>/lsp/` 为根：

- `packages/java-jdtls/<version>/windows-x86_64/` —— 语义引擎
- `runtimes/java/temurin/<version>/windows-x86_64/` —— 私有 JDK（独立记录，不注册为第二个插件）
- `staging/<install-id>/` + `staging/install.lock` —— 临时文件与跨进程锁
- `registry.json` —— 按 plugin_id 记录激活引擎版本与运行环境选择（`Managed`/`External`），
  写入走 `registry.json.tmp` + rename 原子替换

## 3. 自动验证（fake 层，38 通过 / 0 失败 / 4 ignored）

`cargo test --lib install_tests`：

- 下载：SHA 不匹配拒绝、断流失败且 staging 清理、声明的 Content-Length 超上限提前拒绝、
  下载中取消（慢速流 + 取消标志）、白名单外重定向拒绝、白名单内重定向链成功、超量重定向拒绝、
  **镜像优先且镜像成功时官方源零请求、镜像 404 滑落官方兜底**。
- 解压：tar/zip 路径穿越（`../evil`、绝对路径、`C:/`、`..\`）拒绝（恶意名经 GNU header 原始字节
  构造，绕过 tar::Builder 自身的防护，确保被测的是本应用守卫）、符号链接拒绝、条目数限额、
  重复条目拒绝、展开总量限额、缺入口失败、core jar 版本与清单不一致失败。
- 状态：陈旧锁接管、新鲜锁拒绝并发、重复安装拒绝、卸载成功/所有权标记缺失拒绝/未知包拒绝、
  registry 读写回环、External 运行环境不解析为托管路径、托管激活拒绝 snapshot 与未登记版本、
  内置清单与 descriptor.engine_packages 一一对应、正式发行条目注册、清单字段完整性。
- JDK 检测：四源顺序与去重、无效显式路径报错、项目内候选排除、不完整布局标记、
  版本解析（新式/旧式/EA）、`-XshowSettings` 输出解析、本机 JAVA_HOME 真实探测（条件执行）。

## 4. 真实官方包验收（全部通过）

- `jls_t4_real_install_runtime`：CNB 镜像真实下载 Temurin JDK 21.0.12.1+1（205MB）+ JDT LS
  1.60.0（50MB），SHA-256 全量核对通过，有界解压 → 入口验证 → 原子发布 → registry 激活
  全链路成功，**25.94s**；私有 JDK 探测 `java.version=21.0.12`、`os.arch=x86_64`。
- `jls_t4_real_download_install_engine`：JDT 引擎单独安装验收（tempdir 干净环境），**12.26s**，
  `engine_version=1.60.0.202606262232` 与清单一致，`check_managed_activation` 通过。
- `jls_t4_real_managed_service_answers_queries`：**托管安装产物 → 真实 JDT 1.60.0 启动 →
  Maven fixture 五类核心查询复验，全部符合 T0 期望**（与 T1 手动环境同一组断言）：
  1. 跨模块定义：`LoginController.service.login` 唯一定位 core `LoginApplicationService.login`
     （core/.../LoginApplicationService.java:13）；
  2. 引用：`service.login` 共 3 处；
  3. 入调用：2 处（controller + batch job），caller/callee/call_site 端点齐全；
  4. 出调用：1 处（`AuthProvider.login` 接口）；
  5. 实现候选：`AuthProvider` 恰好 2 个（Local/Sso），不假装唯一。
  查询结果 `engine_version=1.60.0.202606262232`（托管正式发行包，非 T0 snapshot），
  `plugin_id=java-jdtls`。总耗时 **10.26s**（含 JDT 导入 Maven 样例）。
- 产物缓存：`target/jls-t4-acceptance/`（registry 已安装时重跑跳过下载，复用验证亦通过）。
- 网络环境备注：本机对国际线路存在间歇性干扰（Connection refused / 零数据超时，同请求重跑即恢复；
  裸 TCP 与 curl 对照确认非代码问题），CNB 镜像作为主源即针对此环境。

## 5. 全量回归与构建

- `cargo test --lib`：659 passed，0 failed，21 ignored（ignored 均为有意保留的真实环境/诊断测试）。
- `cargo test --bin khaslana`：303 passed，0 failed。
- `cargo check --all-targets`：零错误零警告。
- `cargo build --release`：通过，零错误零警告（1m19s）。
- `git diff --check`：通过。

## 6. 已知限制与边界

- Windows x64 是唯一实测托管平台；`current_platform()` 对其他组合返回非清单键，
  `check_managed_activation` 拒绝激活，安装器保留手动配置路径（设计要求）。
- 更新 = 安装新版本目录 + 激活记录原子替换；「旧进程退出后再切换、递增会话身份并失效缓存」
  的进程编排属 T5（T4 保证新版本装失败不影响旧目录与激活记录，退出前结果不引用新目录）。
- 「占用中的版本禁止卸载」由 OS 文件锁语义兜底（Windows 上删除被打开文件报 sharing violation
  并转明确错误）；registry 层不感知进程，服务停止由 T5 编排。
- 首版不做断点续传（设计允许）：失败后调用方重新下载。
- 安装进度回调已按 256KB 节流；设置页 UI 展示与状态机（Missing/Detecting/Downloading/...）
  属 T5。

## 7. 验收矩阵覆盖

- V08（无 JDK 首装/已有 JDK 复用）：检测顺序与复用已实现并测试；**真实首装闭环已通过**
  （CNB 下载 → 安装 → 托管 JDK 探测 → JDT 启动，§4）；干净机器上的一键安装 UI 编排属 T5。
- V09（断网/代理/坏散列/归档/取消）：fake 层全覆盖（§3 下载组）；真实断网场景由多源滑落 +
  明确错误覆盖，磁盘失败经由标准 IO 错误映射。
- V10（更新/占用/卸载/重启恢复）：版本目录独立 + 原子 registry + 卸载保护已测；重启恢复
  （registry 重读）已测（产物缓存复用即重启恢复路径）。
- V13 插件描述部分：兼容清单双条目（正式 + snapshot 例外）、托管激活拒绝 snapshot、
  清单与 descriptor 一致性已测。

## 8. 下一任务

JLS-T3（原生语义视图）仍被 CU2-T5（基础源码视图）阻塞；JLS-T5（插件管理入口）依赖
T3 与 CU2-T5/T6。服务层剩余工作（随 T5 接线）：安装状态机接入设置中心 UI、
「旧进程退出后切换激活版本 + 会话身份递增 + 缓存失效」的进程编排、卸载前停止语义服务。
