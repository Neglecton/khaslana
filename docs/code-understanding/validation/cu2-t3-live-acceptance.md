# CU2-T3 真实模型业务验收报告

日期：2026-09-10（B01～B03），2026-09-11 补充（B04/B05）。状态：B01～B05 全部样例各两遍，
全部通过关键项。

本报告对应开发文档 §7 的验收要求：同一供应商、模型、提示版本、样例版本，每个主样例
至少运行两遍并保留两遍结果，不展示"最好的一次"。

## 验收环境

| 项 | 值 |
| --- | --- |
| 供应商 | 本机已配置的 OpenAI Chat Completions 兼容端点 |
| 模型 | `gpt-5.6-luna` |
| 输出上限 | 客户端配置 4000（不新增旋钮） |
| 样例 | `src/tests/fixtures/code_understanding_business/B01`～`B05` |
| 提示版本 | `analysis_system_prompt()`（`src/code_understanding/analysis.rs`） |
| 问题 | "登录逻辑怎么实现的？调用了什么，被什么调用？读写哪些表？" |
| 执行 | `cargo test --lib live_tests -- --ignored --nocapture --test-threads=1` |
| 原始结果 | `validation/live-runs/{B01..B05}-run{1,2}.json`（含答案与完整工具轨迹） |

问题是所有样例共用的主问题（`expected.json` 中每个 case 的 `question`）。样例索引由
测试在临时目录内用真实索引管线（`run_index`）建立，源码读取走六个只读工具，无 mock。

## 逐项判读

判读依据 `expected.json` 的人工事实范围。五项检查对应开发文档 §7。

### B01 普通 Java 登录

| 检查 | run1 | run2 |
| --- | --- | --- |
| 定位 | 通过：`LoginController.login` + `AdminLoginProbe.verify` | 通过：同左 |
| 链路 | 通过：两个 direct 调用方 + 六个 direct callee | 通过：同左，另含 `PasswordHashLibrary.verify` |
| 读写 | 通过：`app_user` 读/写各列条、`login_attempt` insert；会话标 cache | 通过：同左 |
| 逻辑 | 通过：用户不存在 / 密码错误 / 成功三分支与异常包装 | 通过：同左（run2 带条件标签） |
| 来源 | 通过：全部 `sr1:` 来源经服务校验有效 | 通过 |

本轮 B01 使用真实 JDBC SQL：模型正确区分了 `app_user` 的读与两种写（累加失败次数 /
更新登录时间并清零），并把进程内 `ConcurrentHashMap` 会话列为 cache 而非表。

### B02 Spring + MyBatis

| 检查 | run1 | run2 |
| --- | --- | --- |
| 定位 | 通过：`POST /api/login` → `AuthService.login` | 通过 |
| 链路 | 通过：`LoginController.login` direct 调用方 + 六个 callee | 通过：同左 |
| 读写 | 通过：`app_user` 读+两种写、`login_log` insert，均引用 Mapper XML | 通过 |
| 逻辑 | 通过：USER_NOT_FOUND / DISABLED / BAD_PASSWORD / SUCCESS **四个**分支 | 通过：同左 |
| 来源 | 通过 | 通过 |

关键点：`app_user` 与 `login_log` 的表名来自 Mapper XML 的 `select/update/insert` id
与 `AuthService` 调用的对齐，不是仅凭方法名猜测。Redis 会话正确单列为 cache。

### B03 Spring + JPA

| 检查 | run1 | run2 |
| --- | --- | --- |
| 定位 | 通过：`POST /login` → `AuthService.login` | 通过 |
| 链路 | 通过：`LoginController.login` + 六个 callee | 通过：同左 |
| 读写 | 通过：`app_user` 读+写（引用 `@Table(name="app_user")`）；`LoginAudit` insert | 通过 |
| 逻辑 | 通过：用户不存在 / 密码错误 / 成功三分支 | 通过：同左 |
| 来源 | 通过 | 通过 |

关键点（对应 `expected.json` 的强制要求）：`LoginAudit` 没有显式 `@Table`，两遍都把它
的**物理表名标为 unknown**，没有推断成 `login_audit`；run2 还额外指出 `countLocked`
（`failedAttempts >= 5` 统计查询）并未被登录流程调用，与样例标注一致。token 归为
external，未混入表清单。

### B04 检索缺口（2026-09-11 补）

样例机制：`LoginController → AuthService.authenticate` 是索引可解析的普通调用边；
`LegacyJspLogin.handleLogin` 与 `PluginLoginBridge.signIn` 经反射调用同一方法，索引中
**没有**这两条边（离线测试 `b04_retrieval_gap_fixture_has_real_missing_edges` 先证明
缺口真实存在、且 `search_code` 能按类全名文本搜到两个反射入口），验收目标就是"AI 能
搜索源码补查并说明范围，不强依赖完整静态图"。

| 检查 | run1 | run2 |
| --- | --- | --- |
| 定位 | 通过：`POST /api/login` 入口 + 两个反射入口 | 通过：同左 |
| 链路 | 通过：`LoginController.login` direct + 两个反射入口 indirect + 四个 callee | 通过：同左（run1 callers 里 direct 是 HTTP 客户端、Controller 归入事实行） |
| 读写 | 通过：`report_user` read、`report_login_log` insert（三分支） | 通过：同左 |
| 逻辑 | 通过：三分支 + 插件入口的条件开关 | 通过：同左 |
| 来源 | 通过 | 通过 |

关键点：

- **两遍都靠文本搜索发现了全部两个反射入口**（轨迹：`search_code` 按类名/关键词扫到
  `legacy`、`plugin` 目录后 `read_file` 阅读源码），并把它们标为 indirect 调用方——
  这是 B04 的核心验收目标，索引缺口没有导致入口遗漏。
- `session-<id>` 返回值两遍都诚实标注"未发现持久化/缓存写入"，未编造会话存储。
- run2 发现了两处样例真实细节：`status` 字段被查询但未过滤、`PasswordVerifier` 的
  `storedHash` 参数未参与比较；run1 发现反射入口经无参构造创建服务实例而源码只有
  三参构造器，推断其在运行时可能失败（标 inferred）。均与样例代码一致。

### B05 边界变体（2026-09-11 补）

样例机制五项：按月分表（`portal_user_${month}`）、存储过程（`sp_archive_inactive_users`，
未被登录调用）、无实现接口（`SsoAuthPort`）、条件多实现（`CredentialChecker` 两实现按
`portal.auth.mode` 配置选择）、复杂 SQL（`StatsMapper` 的 CTE + JOIN，不在登录流程）。

| 检查 | run1 | run2 |
| --- | --- | --- |
| 定位 | 通过：`POST /api/auth` → `AuthService.authenticate` | 通过：同左 |
| 链路 | 通过：Controller direct + 六个 callee | 通过：同左 |
| 读写 | 通过：分月表 read/insert（动态表名说明）、MQ message、SSO external | 通过：同左 |
| 逻辑 | 通过：SSO / 用户不存在 / 密码错误 / 成功四分支 | 通过：同左 |
| 来源 | 通过 | 通过 |

关键点（对应 `expected.json` 的强制要求）：

- **动态表名**：两遍都以 `portal_user_${month}` 原样记录对象并明确说明物理表名由
  运行时月份拼接、无法由源码确认唯一表名；没有把 `portal_user` 当作已确认物理表。
- **存储过程**：两遍都完整读过含 `sp_archive_inactive_users` 的 Mapper XML（1-120 行），
  答案既未编造其内部读写，也未把它算进登录数据访问（它未被登录流程调用）。
- **复杂 SQL**：`StatsMapper` 的 CTE `active_users` 与表别名未被当作物理表；整个运营
  统计查询被正确排除在登录答案之外（范围控制正确）。
- **无实现接口**：`SsoAuthPort` 归 external 且操作 unknown，两遍都指出"没有实现类，
  外部行为未知"。
- **条件多实现**：两遍都指出 `LocalCredentialChecker` / `LegacyCredentialChecker`
  哪个生效由 `portal.auth.mode` 配置决定，未武断选择。
- MQ 通知归 message 类（`auth.login.succeeded` / `auth.login.failed`），与表清单分离。

B04/B05 合计四遍：**工具失败 0 次**（B04 24/28 次、B05 23/29 次工具调用全部成功）。

## 验收中暴露并修复的两个真实缺陷

两遍运行的价值在此体现——首轮失败暴露了两个只在真实模型交互下才出现的问题：

### 1. 非可调用候选被误报为"索引代际变化"

`trace_calls` 只解析函数/方法（`resolve_in_graph(callable_only=true)`）。模型对类节点
（如 `AuthService`）发起追踪时返回 `NotFound`，旧代码报
`[GenerationMismatch] 会话候选已不在当前索引中`。模型据此认为索引失效并放弃调用链调查。
修复：候选注册表记录索引标签，非可调用候选给出专门文案（"该候选不是可调用符号"），
错误码改为 `CursorExpired`。回归测试
`trace_on_a_non_callable_candidate_explains_the_real_cause`。

### 2. 模型无法可靠转录长 ID（来源与候选各一处）

旧来源 ID 是 `sr1:` + 64 位内容 SHA-256；真实模型两次抄错（一次 65 字符、一次 15 字符），
导致**本来有效的证据被拒**，整个答案无法交付。候选 ID（`sc1:` + 64 位）同理，B02 run1
出现五次 `get_symbol/trace_calls` 因抄错 ID 失败并反复重试。

修复：两类 ID 都改为服务发放的短确定性 ID（来源 8 位摘要、候选短序号），并允许
**唯一前缀兜底匹配**——抄错末位仍可解析，但多候选不唯一时拒绝，未发放过的 ID 仍被拒。
完整性不变（只有实际发放过的 ID 能通过），同时完整内容 hash 仍保存在 `SourceRef` 内。
回归测试 `source_ids_are_short_and_accept_a_unique_prefix`、
`candidate_ids_are_short_and_accept_a_unique_prefix`。

修复后重跑六遍：**工具失败 0 次**（修复前 B02 run1 为 5 次）。

## 结论

- 五个样例（B01～B05）各两遍运行均通过五项关键检查；无伪造表名、无伪造调用关系、
  无把推断当确定结果。
- 缓存 / 消息 / 外部调用与数据库表清单分离正确；动态表名、未映射对象、存储过程内部、
  无实现接口、条件多实现的选择结果均标 unknown 或如实说明，未编造。
- 检索缺口样例（B04）证明 agent 不强依赖完整静态图：反射调用边缺失时能靠文本搜索
  补查入口并说明范围限制。
- 所有关键结论的来源均由服务校验为本次会话实际发放（`AnalysisResult` 校验强制），这是
  结构保证，非人工抽样。
- 真实模型对 `completion` 的自评有时为 `partial`（如 B01 run2、B02 run1），内容本身完整；
  partial 表达的是模型对未查证范围的诚实声明，符合设计预期而非缺陷。

## 尚未完成

- 追问语义（多轮上下文是否串项目、追问能否深入同一逻辑）属 T4，未做真实模型验收。
- 原生 UI（问题框、读写表、来源侧栏、步骤简图）属 T5，未实现也未验收。
- 冷启动约 2.5～3 分钟/问（十遍合计约 26 分钟），在 40 次工具预算内完成；未做性能剖分。
