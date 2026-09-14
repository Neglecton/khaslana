# Eclipse JDT LS 接入设计文档

版本：1.1。日期：2026-09-12。状态：按用户确认的“官方可选 LSP 插件”方向修订；JLS-T0真实验证核心完成，产品内插件服务、AI接线、UI和安装器待开发。适用范围：CU2基础问答之后的可选Java增强；不改变CU2-T5/T6的交付门槛。执行顺序与完成标准以独立的[开发文档](jdtls-development.md)为准。

输入说明：已完整阅读用户提供的“git图形化程序 - 代码理解视图设计.html”中的三轮对话，对应[分享链接](https://chatgpt.com/share/6aa3ac79-2288-83e8-83dd-a0698c08d4e0)。用户随后要求按建议完成接入设计与开发文档。本版落实“先验证 Java、减少自研语义解析、复用现有索引和 AI、最终提供易用安装入口”。对话中其他助手提到的 Kotlin、商业 Provider、具体 JBoss 环境不作为已确认范围。

## 1. 结论与产品定位

**采用官方可选LSP插件接入JDT LS：基础索引负责找候选，语言服务定位定义、实现和调用，AI阅读源码并解释业务。** 公共LSP客户端和JDT适配器随Khaslana编译发布；JDT/JDK引擎包独立按需安装。代码视图与AI通过同一语义服务查询。

这样能减少自研 Java 类型解析的投入，又不要求用户每次打开项目先等待 JVM 和项目导入。JDT LS 不可用时，现有索引、源码搜索和 AI 问答继续工作；回答应说明实际使用的范围和缺口。

“代码视图”在本设计中指当前工作区的只读源码浏览、方法关系列表与回答简图。历史提交、diff、blame 不接入本服务；也不将源码浏览升级成带补全、重构、运行和调试的编辑器。

### 1.1 交付范围

| 需求编号 | 本次后续开发必须交付 | 不纳入本次范围 |
| --- | --- | --- |
| JLS-R1 | 普通 Java 与 Spring 业务样例；定义、实现、引用、入/出调用 | 自研重载/类型推导引擎、运行时调用证明 |
| JLS-R2 | AI 使用语义定位与现有源码证据解释业务 | JDT 自动输出 SQL/业务图、完整 Spring 静态分析 |
| JLS-R3 | 原生源码查看、语义关系列表、继续追问 | 编辑器补全/重构/调试、全项目图谱 |
| JLS-R4 | 缺引擎、导入失败、取消、文件变化时明确降级 | 让 JDT 启动成为基础索引或问答前置条件 |
| JLS-R5 | 运行环境检测、复用已有 JDK、按需安装私有引擎、离线手动路径 | 把 JDK 塞进主安装包、修改系统 JAVA_HOME/PATH |
| JLS-R6 | 项目隔离、来源/版本校验、有界资源、可信项目导入 | Git 融合、数据库连接、AI 自行安装或执行命令 |
| JLS-R7 | 官方插件描述/静态注册、公共LSP客户端、JDT适配器、能力与版本校验 | 第三方插件市场、动态DLL/脚本宿主、任意VS Code扩展兼容 |

T1即拆分公共客户端和JDT适配器，并建立仅含一个官方条目的静态注册表；不等第二种语言出现才拆通信层。SemanticProvider只保留当前五类查询所需接口，不预建Kotlin/JetBrains/Oracle实现。已有Java提取和解析代码保留维护，停止扩展为完整语义引擎；不要为落实本设计删除已有实现。

“插件”是Khaslana自己的受控语言能力模块，不是把VS Code扩展或Eclipse内部扩展直接加载进GPUI。引擎包独立安装不代表适配器代码可独立更新；首版适配器修复随Khaslana版本发布，不做热加载。

## 2. 能解决什么

官方 JDT LS 提供定义导航、引用、调用层级、类型层级等能力，支持 Maven、Gradle 和独立 Java 文件。实际效果依赖工程导入和 classpath。[官方功能与启动说明](https://github.com/eclipse-jdtls/eclipse.jdt.ls)

| 用户问题/动作 | 预期补强 | 仍需保留的边界 |
| --- | --- | --- |
| 这个 `login` 到底调用哪个方法 | 按调用位置解析，区分同名方法和重载 | 依赖缺失时可能无法确定 |
| 谁调用它，它调用谁 | 查询静态调用方/被调用方及调用位置 | 不是运行时轨迹，也不是完整业务流程 |
| 接口背后的实现在哪里 | 返回实现候选，减少全仓库文本搜索 | 多个实现不等于实际注入了其中某一个 |
| 这个符号还在哪里使用 | 语义引用位置列表 | 引用可能是类型/字段使用，不能全部标为调用 |
| 看不懂源码，想跳转和继续追问 | 查看定义、实现、引用、方法大纲；从选中位置追问 | 解释仍由 AI 完成 |
| 登录读取/写入哪些表 | 更容易到达 Repository、Mapper、JDBC 调用处 | SQL/XML/实体映射、实际表名和操作由 AI 查证 |
| Spring 的事务、过滤器、代理有什么影响 | Java 类型与方法定位提供线索 | 不保证还原 AOP、条件 Bean、反射、动态代理和配置选择 |

尤其适合当前已经记录的缺口：Controller/Service 方法同名、receiver 类型不可见时，基础索引可能漏边。见 [CU2-T2/T3 交接](validation/cu2-t2-handoff.md)。本设计不承诺 JDT LS 自动解决所有这种情况，必须用实际样例对比。

## 3. 架构与最小接入面

| 层 | 保留/新增职责 |
| --- | --- |
| tree-sitter + SQLite/FTS | 保留现有多语言索引、符号候选、稳定身份、基础关系和离线检索 |
| LspClient / ProcessManager（拟新增） | stdio通信、请求配对、取消、进程所有权与退出；公共层不包含Java启动参数 |
| 官方插件描述 / JdtLsProvider（拟新增） | 静态注册Java能力；JDT启动、初始化配置、导入状态、版本适配和特有结果处理 |
| LspSemanticService（拟新增） | 按项目/语言路由到已启用插件，管理锚点、共享查询、能力校验、缓存及降级 |
| UnderstandingTools | 复用六工具与 SourceService；增添一个有界语义查询入口，Java 调用追踪可按条件增强 |
| 理解 agent / AnalysisResult | 继续负责业务解释、表读写、来源校验与本次简图 |
| 原生源码视图 | 调用LspSemanticService；显示定义/实现/引用/一跳调用，与AI共享来源规则，不自行启动插件 |

Rust 与 JDT LS 通过本地子进程的 stdio JSON-RPC/LSP 通信。JDT LS 官方支持标准输入输出；`-data` 使用独立的项目工作目录。[官方通信与工作目录说明](https://github.com/eclipse-jdtls/eclipse.jdt.ls#running-from-the-command-line)

拟新增目录统一为 `src/lsp/`：client/protocol/process负责通用通信，service负责语义查询，plugins负责描述与静态注册，providers/jdtls负责Java适配；T4再加入install与runtime/java。小型职责可以先合并文件，禁止新旧 `java_semantic` / `lsp` 两套服务并存。测试fixture仍保留原 `java_semantic` 路径，无需为目录命名重做T0。GUI不启动MCP，本阶段不扩展对外MCP协议。

### 3.1 插件描述与能力边界

采用编译期受控的 `LspPluginDescriptor`，可以是Rust常量或编译进程序的数据文件；用户项目内的manifest不能注册或覆盖它。

| 字段 | 首版规则 |
| --- | --- |
| plugin_id / display_name | 固定ID `java-jdtls`，名称“Java语言支持（JDT LS）” |
| descriptor_version / adapter_id | 描述schema版本与编译期 `jdtls` 适配器键；未知schema/键拒绝，不动态加载代码 |
| languages / operations | Java与五类查询；document_symbols仅作内部辅助能力 |
| transport / runtime | stdio、Java运行环境要求；参数由受控适配器生成，无任意shell模板 |
| engine_packages / compatibility | 指向EnginePackageSpec；包含已测引擎版本与平台/JDK组合 |

静态注册只提供按ID/语言查询，无目录扫描、外部脚本、联网插件目录或可执行manifest。未知插件ID、重复ID、同一语言冲突均显式失败，不能任选第一个插件。

有效能力由“描述声明 ∩ 适配器已实现 ∩ 服务器实际协商结果”决定。缺能力返回unsupported并降级，不生成另一种语义的替代结果；注册成功不表示引擎已安装或项目已ready。标准方法与位置类型尽量由公共层处理，JDT特有配置、通知和扩展只在provider中实现。

语义结果保留 `provider=jdtls`，另加由服务生成的 `plugin_id=java-jdtls`；事件、缓存与会话身份加入plugin_id及激活引擎版本。上层不读取JDT初始化参数、Java executable或内部URI。AI工具名 `query_java_semantics` 保留，它只是当前Java能力入口；内部统一路由，不要求模型选择插件。

### 3.2 与T0成果衔接

[T0交接](validation/jls-t0-handoff.md)已证明核心定位收益，直接进入T1，不因插件化重跑整套T0。T1用相同样例验证Rust客户端，补齐取消生效、位置转换、导入状态和进程回收；已有报告/探针保持历史证据。

T0使用带散列的本地JDT snapshot且缺完整原始下载链路，仅允许手动开发模式显式指定，不纳入托管发行清单。T4需选择固定官方发行包并复跑核心查询后收录；不得把开发例外变成任意引擎版本默认兼容。

T0还记录了工程元数据写入、依赖缓存写入、Gradle daemon残留与toolchain缺失。公共层管理进程句柄/预算，JDT适配器识别导入和构建进程上下文；只回收确认属于本应用的进程，不停止用户其他工具正在使用的共享daemon。先验证独立Gradle用户目录等隔离配置，不能把“杀所有java”当清理方案。插件化不改变原有资源预算和导入授权边界。

接入位置以 2026-09-11 HEAD `6f058b8d63a833a3a7ecef6a1c374494ebd96317` 为核查基线：

- [tools.rs](../../src/code_understanding/tools.rs)：现有 `RegisteredCandidate` 只有 qualified_name/label，需要按需补合法位置锚点；`TraceCallsResult` 是 hop 列表，不能直接冒充带边的语义图。
- [source.rs](../../src/code_understanding/source.rs)：继续负责允许文件、UTF-8、内容 hash、字节范围与 `sr1:` 来源。语义返回的路径不能绕过这里。
- [agent.rs](../../src/code_understanding/agent.rs)、[session.rs](../../src/code_understanding/session.rs)：沿用工具预算、路由代际、取消和追问；不复制另一套业务 agent。
- [analysis.rs](../../src/code_understanding/analysis.rs)：沿用现有答案结构。新的语义元数据先留在工具响应/会话中，不放松答案来源校验。

## 4. 第一阶段的查询契约

### 4.1 共享服务

对外暴露 `query(anchor, operation, limit, cancellation)`；operation 只包含 definition、implementations、references、incoming_calls、outgoing_calls。源码大纲通过内部 document_symbols 查询提供给视图，hover 留到后续体验优化。

LspSemanticService接口按职责分为 `status(project, language)`、`ensure_started(project, language)`、`query(...)`、`release(consumer)`。服务由静态注册和配置选择provider，JdtLsProvider在内部组装LaunchSpec交给公共进程层；UI/AI不能传启动参数。SemanticProvider约定能力、启动配置、导入状态解释和查询适配，返回owned DTO；UI不持有协议连接或Child，也不引入复杂泛型层级。

anchor 由服务注册，包含项目、相对路径、当前文件 hash 和精确位置。AI 输入为二选一的带 tag 结构：`{kind: candidate, candidate_id}` 或 `{kind: source, source_id, line, column}`；行列均从1开始、column按Unicode标量计数，必须落在已发放源码范围。适配层转换到 LSP 编码。UI 从当前已校验源码生成同类内部锚点。模型不直接传任意 URI、LSP 方法名或 executeCommand。

返回 `SemanticQueryResult`，至少包含：

| 字段 | 含义 |
| --- | --- |
| project_key / request_id | 绑定项目和本次请求 |
| session_epoch / workspace_revision | 绑定 JDT 进程会话、已知源码/构建配置变化 |
| provider / operation | 固定来源 `jdtls` 与操作，不让模型自行填写 |
| plugin_id / engine_version | 静态插件身份与本次激活引擎版本，由服务生成 |
| availability / coverage | ready、partial、unavailable；导入/依赖/同步缺口 |
| items | 定义或引用的位置；调用项含明确的 caller、callee、call_site |
| truncated / reason | 截断、超时、能力缺失或降级原因 |

区分“查询成功但没有结果”“项目未就绪”“不支持该查询”“范围外依赖”“文件已变化”。空列表只代表本次静态查询未找到，不能证明不存在调用。

错误分两类：anchor伪造/越界、项目错配、来源过期沿用现有来源与上下文错误；引擎未安装/未启用、busy、timeout、unsupported、import_failed、dependency_missing作为语义可用性原因，不能误报 IndexMissing 或让正常基础查询失败。来源不安全时拒绝该语义项，禁止降级成未经校验的源码读取。

### 4.2 映射到 JDT LS

定义/实现/引用分别映射 `textDocument/definition`、`textDocument/implementation`、`textDocument/references`；调用先 prepareCallHierarchy，再 incomingCalls/outgoingCalls。官方实现确有调用层级处理器，内部依赖 JDT 调用层级搜索。[CallHierarchyHandler 源码](https://raw.githubusercontent.com/eclipse-jdtls/eclipse.jdt.ls/main/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/handlers/CallHierarchyHandler.java)

实现时按服务端声明能力和锁定版本验证；保存 prepare 返回的原始 item，包括可能的 opaque data，再用于后续请求。处理 Location/LocationLink 差异、重复结果、null 和多个候选。

调用项的 fromRanges 是调用发生的位置：入调用在调用方文件，出调用在当前被查询文件。必须分别保留定义位置与调用位置，不互相替代；用这些显式端点构建一跳关系。递归或环只标记，不无限展开。JDT 返回的多态候选仍是静态候选。

位置适配遵循 [LSP 规范](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)：处理零起始行列、协商的字符编码（未协商时按 UTF-16）、范围结束位置不包含的语义，再转换为现有源码字节范围。中文、emoji、CRLF 都要验证。当前索引只有起始行的候选不能直接用“行首列 0”猜方法名位置；优先用 documentSymbol 的 selectionRange 匹配，遇到重载/歧义返回候选。

### 4.3 AI 工具变化

保留六个基础工具；开启 Java 增强的项目在一问开始时冻结工具列表，额外提供 `query_java_semantics`，参数为受限 operation、合法 anchor、limit。工具调用不会安装引擎、授权导入或轮询等待服务就绪；未ready时立即返回可用性原因。下一问可使用已经准备好的服务。这样不必为每个 LSP 方法增加一个 AI 工具。

`trace_calls` 的现有输入保持兼容：Java 且服务 ready 时补充一跳语义结果；超时/未就绪保留基础查询结果并说明原因。旧 callers/callees 保持导航含义；新增可选 `semantic` 部分承载明确端点、位置和来源，并标明 `depth=1`。即使原请求depth=3，也不暗示语义部分查了三跳；更深关系由AI继续查询。双方冲突时分别保留 provenance 并要求阅读调用位置，不静默覆盖或合并成确定边。新操作和内部多次 RPC 都受预算控制。

JDT LS 的定位结果先是线索。AI 对关键业务结论仍调用 get_symbol/read_file，由 SourceService 发放证据；调用目标在索引中不存在时，可以使用当前允许文件的源码锚点，不伪造 SQLite 节点或 `sc1:`。

## 5. 索引与缓存策略

**先做按需增强，不把 JDT LS 当全仓库图导出器。** 第一阶段不改索引 schema，不把 JDT 模型全量复制到 SQLite，也不在每次索引时对所有方法递归查调用。

索引页面分开显示“基础索引已就绪”和“Java 增强状态”，不能让用户误以为开关一开就已把全库关系升级。此时改善的是索引查询结果和定位能力，基础库本身未增加 JDT 关系。

语义缓存只驻内存，key至少包含plugin_id、激活引擎版本、项目、session_epoch、workspace_revision、anchor hash、operation和限额。每项目最多128个结果、总计16MiB（初始工程预算，后续实测调整）。基础库generation与JDT revision是两套身份，不能混用。

文件监听负责标脏；查询前核验锚点文件，必要时 didOpen/didChange 同步当前内容；其他源文件变化通过本次必须实现的 watched-files 同步路径处理。请求前后 revision 不一致就丢弃结果。监听丢事件、导入中或同步状态不明时返回 partial/unavailable，不能仅凭目标文件 hash 相同就断言跨文件关系仍有效。

pom/Gradle 配置、classpath、JDK 配置变化后，失效当前语义缓存并重新导入；不要对过期模型继续标 ready。重复 didOpen/didClose 与文档版本由共享服务统一管理，避免 UI 和 AI 各维护一份工作副本。

后续只有实测证明重启后的重复查询明显影响体验，才考虑独立、可丢弃的语义缓存存储；仍不覆盖基础 CALLS 边。跨文件依赖失效、来源和版本契约必须先明确，不能把落库变成第一阶段前置任务。

## 6. 运行环境与导入边界

### 6.1 可选启用

默认关闭 Java 增强，基础模式完全沿用 CU2 的只读边界。用户在项目设置开启后，首次 Java 查询或进入Java源码视图时异步启动服务；不因发现一个 `.java` 文件就启动 JVM。技术验证阶段采用用户指定的本地 JDK/JDT LS 路径；完整交付必须包含检测与按需安装，主安装包不捆绑JDK。安装完成与项目语义ready是不同状态。

调研时官方要求服务运行时至少 Java 21。服务 JDK 与项目目标 Java 版本需要分开配置，不能要求 Java 8/17 项目修改源码等级来运行服务。正式实施锁定一个已测发行包、校验值和对应 JDK 范围，不直接跟随 snapshots。[官方运行要求](https://github.com/eclipse-jdtls/eclipse.jdt.ls#requirements)

启动命令由 Rust Command 的独立参数组成，Windows 隐藏窗口；清除会改变 stdio 传输的 CLIENT_PORT 等继承变量。JDT `-data` 和可写运行配置放应用数据目录，按 canonical 项目路径的稳定 hash 隔离并核验完整路径归属；不复用 VS Code/Eclipse 的工作目录，不占用用户已开的语言服务。

### 6.2 “只读查询”不等于“导入零副作用”

完整 Java 语义通常要解析依赖和建立工程模型。VS Code 的 Java 文档也区分仅源码/JDK 的轻量模式和解析依赖、构建项目的标准模式；这些是其客户端的模式设计，不能直接把 VS Code 设置名当成独立 JDT LS 的通用启动开关。[官方模式说明](https://code.visualstudio.com/docs/java/java-project#lightweight-mode)

Gradle 配置阶段会求值构建脚本，即使没有主动执行测试任务，也可能执行项目提供的逻辑。[Gradle 构建生命周期](https://docs.gradle.org/current/userguide/build_lifecycle.html)。因此，开启完整导入会扩展原 CU2 的运行边界，必须作为产品中一次明确的按项目启用动作，解释依赖下载、缓存写入、构建配置/插件执行的可能性；AI 不能自行开启。本文只是方案，不授权本轮执行导入。

首版不另造“半安全 JDT 模式”：未启用时使用现有基础服务；已启用且用户信任该项目后才做工程导入。若用户要求绝不执行构建逻辑，保持基础模式，不承诺同等语义精度。

导入默认尽量关闭自动构建、注解处理和工程根目录元数据生成，但需要对锁定版本验证效果；这些选项不能充当执行沙箱。Lombok/生成源码依赖缺失时标 partial，不自行运行生成器。相关开关存在于 [JDT LS Preferences](https://raw.githubusercontent.com/eclipse-jdtls/eclipse.jdt.ls/main/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/preferences/Preferences.java) 与 [Java 客户端设置说明](https://github.com/redhat-developer/vscode-java#supported-vs-code-settings)。

基础问答仍不运行应用、SQL 或数据库连接；语义服务也不向模型暴露 shell、构建或任意 executeCommand。拒绝 workspace/applyEdit。外部依赖 URI（如 class 文件虚拟 URI）只显示依赖目标和不可读原因，首版不自动下载源码、不引入反编译器，也不允许它们绕过项目文件白名单。

JVM、Maven、Gradle 的网络配置不自动继承 ureq 的代理行为。首版分别检测依赖导入失败并提示配置，不声称支持与应用完全一致的代理；不得把 Git/AI 密钥复制进 Java 进程参数或日志。离线导入缺依赖应正常降级。

### 6.3 运行环境检测与选择

默认检测顺序：用户显式选择路径 → 已安装且验证通过的Khaslana私有环境 → JAVA_HOME → PATH中的java。只检查这些有限候选，不全盘搜索；PATH候选需排除当前项目提供的可执行文件，运行版本检查使用绝对路径并限时。显式路径无效时提示修正或切回自动检测，不能偷偷换另一套JDK。

每个候选检查完整JDK布局（含java/javac）、版本、架构和锁定JDT包的兼容范围，再做启动检查。产品文案可用“运行环境”，下载实现首版使用完整JDK，不假设裁剪JRE可用。技术验证优先JDK 21，最终支持矩阵由JLS-T0记录；“版本越新就必然兼容”不作为选择规则。

服务运行JDK与项目JDK分栏；项目可使用8/17/21，不能把服务JDK自动当项目目标。目标JDK缺失则显示依赖/运行库缺口并允许手动指定，不自动下载项目业务JDK。只修改本应用子进程环境，不修改系统环境或项目构建文件。

### 6.4 按需安装

用户点击“安装Java语义引擎”后先检测本地环境。可复用JDK时只下载JDT LS；缺少时展示运行环境与JDT LS的版本、下载体积和安装位置，用户确认后下载到应用私有数据目录。取消/失败可重试，基础模式继续可用；AI不触发该流程。

默认运行环境来源选择Eclipse Temurin官方归档，JDT LS使用Eclipse官方固定发行包。Temurin提供归档安装方式和程序化下载API；实现采用固定版本清单，不在运行时盲目解析“latest”。[Temurin安装说明](https://adoptium.net/installation)、[官方API文档](https://github.com/adoptium/api.adoptium.net/blob/main/docs/cookbook.adoc)

应用随版本维护一个小型内置 `EnginePackageSpec` 清单：组件、固定版本、OS/架构、HTTPS URL、可信SHA-256、压缩/展开限额、可执行入口、兼容JDK范围、许可证来源。T0先锁定测试版本，T4落入发行清单。缺少校验值/适配平台时不提供一键下载，保留明确的手动配置提示；不得用占位URL或跳过校验来通过验收。优先交付Windows x64，其他平台仅在实际验证且有清单时宣称支持托管安装。

使用应用现有directories目录定位方式，以 `<数据目录>/lsp/` 为根：`packages/java-jdtls/<version>/<platform>/`保存引擎，`runtimes/java/<distribution>/<version>/<platform>/`保存私有JDK，`staging/<install-id>/`保存临时文件，`workspaces/java-jdtls/<project-key>/`保存JDT状态。registry按plugin_id记录引擎版本、runtime选择与归属。Java运行环境不作为第二个可执行插件注册。产品存储尚未实现，无需为旧设计中的java-semantic路径编写迁移器；T0目录继续只作手动来源。

目录不得存入项目，保存原始canonical项目路径核验hash归属。全局只进行一个安装任务，跨进程也要防并发写同一版本。安装器只接受内置已注册插件的包清单；引擎版本不在当前适配器已测兼容清单时拒绝托管激活，提示升级Khaslana或选择已支持版本。

安装顺序固定为下载 `.part` → 核对实际大小/散列 → 有界解压到staging → 校验入口和运行环境 → 原子发布版本目录 → 更新激活记录。下载复用ureq和应用代理策略，禁止错误时静默直连；参考 [update.rs](../../src/update.rs) 的流式下载/散列模式，但其现有接口没有取消参数，不能直接当作完整安装器。

解压检查每个路径，拒绝绝对路径、`..`、根外链接/重解析点和重复覆盖目标；按实际展开量、单文件量和条目数限额，不只信归档头。只清理本次应用staging，失败保留已激活版本。下载超时/断网/磁盘不足/校验失败/解压失败分别给可操作原因，首版重试重新下载，不要求断点续传。

安装状态独立于语义进程：Missing、Detecting、Downloading、Verifying、Extracting、Installed、Failed、Cancelled；Installed只代表文件可用，不能显示“项目分析完成”。关闭设置页只隐藏进度，安装仍可在设置页继续查看/取消；退出应用则取消安装，清理临时任务并保留完整版本。

首版不实现自动更新调度和包管理平台。随应用清单更新提供显式更新入口；安装新版本失败保留旧版本，正在被进程使用的版本不覆盖/删除。卸载仅可移除应用拥有且未使用的包，经确认后执行，绝不能删除检测到的系统/用户JDK。

### 6.5 配置存储与职责

设置中心按plugin_id保存全局plugin_enabled、运行环境选择、JDT路径覆盖、托管安装激活版本；按项目+plugin_id保存project_enabled、完整根路径、项目JDK映射和导入授权状态。使用应用现有设置持久化方式，不混入code-index启用偏好，不把设置塞进AI答案或索引schema。初始plugin_enabled=true只表示允许使用已注册能力，project_enabled=false确保不会自动启动；未安装也不自动下载。

全局禁用会停止接受该插件的新查询，失效缓存和事件路由，取消在途查询并有界释放自有进程，保留引擎包和项目偏好。项目关闭只释放该项目消费者。重新启用不自行安装，仍要求项目已启用和既有导入授权。卸载只移除确认归属的包，不能删除编译进程序的适配器；UI回到“未安装”，已完成答案按原来源规则保留。

项目导入授权不等于安装授权；已授权项目重启不重复询问。新项目、canonical根路径变化或开启此前未授权的构建执行能力时重新解释范围；普通源码/pom更新只标脏并按已授权范围同步。引擎未启用时不启动导入。代理secret遵循既有存储方式，不复制进新配置或安装日志。

## 7. 生命周期与资源约束

服务状态采用 Off → Starting → Importing → Ready/Partial，失败进入 Unavailable，释放进入 Stopped。initialize 成功只证明协议握手完成；工程是否可用须结合锁定版本的导入完成通知、错误与代表性查询判断。

初始实现预算如下，全部是待基准验证的工程设定，不是 JDT LS 的性能承诺：

| 项目 | 初始约束 |
| --- | --- |
| 进程 | 全应用最多一个 JDT LS，当前使用项目优先；同项目多个消费者共享 |
| 查询 | 默认一跳/20 条，最多40条；一次语义操作含排队/内部查询总计3秒，超时取消并降级 |
| AI 语义 RPC | 单次工具最多6个查询 RPC、每问最多40个；计入现有工具8K/累计120K字符预算 |
| 初始化/导入 | 握手30秒、首次导入120秒期限；导入期间 AI 继续基础模式，不轮询占工具预算 |
| JVM | 初始 `-Xmx1G`，不是进程总内存上限；实测 JVM、构建子进程、RSS 和缓存大小 |
| 通信 | 单帧最大8MiB，按 Content-Length 在分配前检查；超大响应丢弃并降级 |
| 队列 | 最多16个待处理操作，单进程串行执行语义操作；超量立即说明繁忙 |
| 释放 | 无消费者且空闲5分钟关闭；应用退出完整回收；切项目不并行启动第二个 JVM |

限额由单一常量/配置对象维护。6个/40个RPC是额外计算预算；缓存命中不扣RPC，但仍扣一次AI工具和结果字符预算。初始化、通知、必要的服务端配置响应不计查询RPC，仍受握手/导入期限、帧大小和队列保护。模型和UI的查询共享进程及队列；UI自身不消耗AI预算。

串行执行包含 prepare + 后续调用查询整个操作，避免不同视图交错使用服务端调用层级状态。协议读循环必须独立持续处理响应/通知/服务端请求，不能因等待查询而阻塞；服务器请求的配置、能力注册和进度按实际声明实现，未实现的能力不宣称支持。

切项目/离页后先失效 UI 路由，再取消相关查询；旧项目任务仍占用进程时，新项目先用基础模式。shutdown/exit 有界等待后回收本应用拥有的进程；Windows 验证 Job Object 或等效清理方案，不能按 `java.exe` 名称全局杀进程。持续故障最多自动重启一次，然后等待手动重试。

服务常驻 I/O 不占 Ai/Long 池的任务线程；AI 本身仍沿用 Ai 池。取消必须贯穿排队、RPC、来源读取，迟到结果按 request/session/revision 丢弃。

## 8. 用户入口与代码视图

### 8.1 设置和状态

在现有“代码索引”项目卡内增加“Java 增强”设置，配置运行环境放可展开区域；与“基础索引”开关分开。启用说明仅第一次或信任范围变化时展示，不让用户每次查询重复确认。

设置中心的“语言支持”区域只显示一个官方Java插件卡，展示插件身份、引擎版本、安装/兼容状态与全局启停；项目卡显示项目启用与工程状态，避免重复安装。状态入口分别给“安装引擎”“重新检测”“重试导入”“高级设置”；无合适JDK时不要求先进入高级设置。全局禁用、卸载与项目关闭含义按第6.5节区分。一键安装和失败恢复复用现有反馈/进度，不新建插件商店页面。

问答主界面只保留一个小状态入口：“基础分析”“Java 增强可用”“Java 增强暂不可用”。详细原因按需展开；运行进度走既有底部状态栏。用户照常在同一个输入框问“登录逻辑怎么实现”，无需先选引擎。

### 8.2 源码与简图

来源点击打开只读源码；方法/符号位置提供鼠标操作“查看定义”“查看实现”“查找引用”“调用方”“被调用方”“解释此处”。查询结果在现有侧栏内切换为列表，提供返回；不新增一整套常驻工具窗口。

JDT LS 未安装或正在准备时，已知定义仍可按基础索引打开；无法做语义解析的动作给出原因，并允许源码搜索或 AI 继续调查。不得给模糊文本匹配贴“已解析”标签。

流程简图仍展示 AnalysisResult 的业务步骤。点击节点可看来源、展开一跳调用；默认展示少量业务节点，不自动变成全项目调用图。调用列表与业务流程保持各自语义，不能按引用顺序推导执行顺序。

继承项目 Focus Workbench 壳层、主题 token、源码虚拟列表与 syntect 高亮。第一阶段不引入 semantic tokens、不新增 F12/Ctrl+点击等键盘行为，遵循根 AGENTS.md 的键盘白名单。此处是交互规范，本轮未生成高保真稿或做原生 UI 验收。

## 9. “登录逻辑”闭环示例

以下是假想 Java 项目流程，不代表 Khaslana 内部存在相应业务：

1. 用户输入“登录逻辑怎么实现”。基础索引先找到 `LoginController.login`。
2. AI 阅读方法，JDT 定位 `authService.login(...)` 的声明目标；接口存在多个实现时返回候选。
3. AI 阅读候选实现和注入配置，有证据才关联当前入口；运行条件未知则保留候选说明。
4. 一跳查询找到调用方与下游 Mapper，读取调用位置，确认不是另一个同名方法。
5. AI 继续读取 Mapper XML/JDBC SQL/JPA 映射，列出用户表的读取、失败次数更新、日志表写入，并核对触发条件；缓存/外部认证另列。
6. 输出业务解释、读写对象清单、异常分支和简图，每个关键结论都有本次源码引用。

如果第2或第4步语义服务失败，继续现有 search_code/read_file 兜底，标明查询缺口。JDT 缺少 Spring 路由调用方也不能被解释为“这个登录方法无人调用”。

## 10. 开发拆分与验收

下表是阶段导航，任务依赖、文件职责、测试矩阵、状态台账及交接模板统一移至[开发文档](jdtls-development.md)。历史JLS-T0～T3编号保留，新增T4/T5；当前状态以开发文档为准，本文不把设计或技术验证完成写成接入完成。

使用独立任务号 `JLS-T*`，不改写已经完成的 CU2 服务层任务。T5/T6 基础页面可继续；语义按钮在服务适配完成后再接入。

| 任务 | 交付物 | 完成门槛 |
| --- | --- | --- |
| JLS-T0 技术验证 | 锁定版本/JDK；普通Java、Maven多模块、Gradle工程的真实请求与成本报告 | 五类查询结果可核对；记录导入写入/进程/网络边界；失败项明确，不以 mock 替代 |
| JLS-T1 公共客户端与JDT插件 | 官方静态注册、公共LSP客户端、JDT适配器、统一语义服务 | 注册/能力/版本身份与协议/来源/生命周期测试通过；基础模式回归通过 |
| JLS-T2 AI 接线 | query_java_semantics + trace_calls可选增强，保持既有答案契约 | 同模型、同问题、同预算开/关各两遍对照，关键语义零误认，记录工具数与耗时 |
| JLS-T3 原生入口 | 项目设置、源码语义动作、关系列表与简图联动 | 实际 GPUI 深浅主题、长路径、窄窗、IME、取消和切项目检查 |
| JLS-T4 插件引擎包与环境管理 | plugin_id归属、已测兼容清单、检测/安装/更新/卸载 | 固定发行包复测、下载/解压/取消/兼容检查；不修改用户JDK |
| JLS-T5 插件入口与整体交付 | 官方Java插件卡、全局启停/项目状态、一键安装与回归 | 无JDK环境闭环、禁用/卸载/版本切换后基础能力及来源保持有效 |

必须覆盖的样例：

- Controller/Service 同名 login、方法重载、跨模块调用：声明和调用位置正确，不串方法。
- 接口两个实现、继承/override、lambda/方法引用：可得关系与候选分别记录，不要求假装完整运行时解析。
- 普通 Java 源码项目和 Spring 项目均有主样例；Spring+MyBatis/JPA 保留表读写、失败路径、缓存的 AI 核对。
- classpath 缺失、离线依赖、Lombok/生成源码、反射/AOP：标注局限，不能把空结果作为否定证明。
- 文件变化、pom变化、切项目、同项目多个窗口消费者、超时后迟到回复：无过期定位和跨项目污染。
- 中文/emoji、CRLF、Windows中文与空格路径、项目外URI、根外链接：位置正确、白名单生效。
- 未启用/无JDK/进程崩溃时，基础索引与已有 B01～B05 问答路径保持可用。

JLS-T0 建议记录：冷启动、首次导入、热查询p50/p95、峰值RSS（含构建子进程）、磁盘缓存、基础索引耗时、AI工具次数/输入量、最终答案正确性。热查询目标p95≤3秒；超出后优先缩小增强范围，不提高所有问题的等待时间。锁定机器和项目规模后再判断收益，不提前承诺“秒懂任意项目”。

进入产品默认推荐前，JLS-T0 必须证明同名/跨模块样例有实际改善、导入边界可解释、失败降级可靠。若收益不足，保留可选实验能力，不把重写 Java 分析器作为补救排期。

实施批次按根 AGENTS.md 运行相关测试、库/主程序检查与 `cargo build --release`，零错误零警告。测试报告区分 fake LSP 协议测试、真实 JDT LS 查询、真实模型业务验收和人工 UI 验收。本轮只设计文档，未完成上述运行验证。
