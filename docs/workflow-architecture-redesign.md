# 工作流架构升级设计（讨论稿）

状态：设计与后续目标；初稿于 2026-09-27 核对远端模板仓库 `4fdce5adced2ee6f92bfb61ce8c7fabf472c13b8`。截至 2026-10-06，v2 执行内核、固定来源逐个下载、分支入口、本地 stdio MCP/JS、AI Skill、内置 Edge 运行组件以及设置中心的 Skill/MCP 管理均已有实现。实际交付与验收边界见 [开发计划](workflow-development-plan.md)，语法以 [工作流使用说明](workflows.md) 为准。下文未交付的来源更新管理等内容仍属于后续设计。

## 1. 目标与已知事实

目标是让模板可从远端取得、让分支右键直接进入对应工作流的运行页，并逐步把执行器扩展为客户端操作、MCP 工具、JavaScript 规则与 AI Skill 的统一编排器。创建和编辑过程应先让用户描述目标、核对步骤与输入，再处理高级配置。

当前实现的关键边界：

| 位置 | 当前行为 | 对设计的影响 |
| --- | --- | --- |
| `src/workflow.rs`、`src/workflow/extensions.rs` | 已接受 v1/v2；v2 的 `invoke` 可映射内置 `git.*` 动作，或调用本地 stdio MCP、受限 JS 与 AI Skill；v1 原路径保留。 | 扩展执行与浏览器样板已有实现，真实窗口验收仍需单独核对。 |
| `src/workflow_view.rs` | 扫描普通模板目录；固定 CNB 来源在弹框中列出，用户逐个下载，旧版批量同步文件只导入一次。外部步骤运行前逐次授权，使用独立任务池并以 `UiEvent` 回传。 | 后续需要来源版本与更新冲突界面。 |
| `src/main.rs` | `workflow_state` 属于 `RepoTabState`。 | 分支启动上下文也应随仓库标签页保存，避免切仓库串值。 |
| `src/sidebar_view.rs` | 本地与远端分支共用右键菜单对象，菜单保存分支名与类型。 | 可以从菜单对象构造精确的来源分支上下文，而不是读取此时的 HEAD 或侧栏选中态。 |
| `src/workflow_editor.rs`、`src/ai/prompt.rs` | 表单按固定步骤类型构造 v1；AI 收到整份工作流文档，只返回 JSON5 并回填表单。 | 新编辑器应由步骤描述生成字段；AI 先生成可检查草稿及差异，不直接运行。 |

远端地址 `https://cnb.cool/liuchenchen/work-studio/-/tree/main/work-studio-files` 对应 Git 仓库中的 3 个 v1 模板：`create-release-branch.json5`、`create-uat-branch.json5`、`merge-to-test-branch.json5`。三者的来源输入默认值都依赖 `${git.initialBranch}`。P2 已让分支右键入口固定该变量的来源引用。

代码知识图谱为 Tier 2 定向查询，证据文件均做了覆盖检查；索引记录无缺口，但文件元数据提示已变更，因此上述关键行为还用当前源码的定向读取核对。本文不据此作全库穷尽性结论。

## 2. 用户流程

### 2.1 模板发现与下载

以下是完整来源管理的后续目标。当前已实现固定 CNB 目录弹框、逐个下载到普通模板目录和同名保护；来源版本、差异与更新决策界面仍待实现。

1. 工作流模板栏展示“本地”“已下载”两个来源，提供“浏览远端模板”和“检查更新”。首次打开不自动下载、也不自动运行远端内容。
2. 从固定的 CNB 仓库读取 `main/work-studio-files` 的目录树，展示名称、提交版本、文件摘要与更新状态。用户选择模板后下载。后续可增添其他来源，但第一期不开放任意地址配置。
3. 下载时只接受该目录中的普通 `.json5` / `.jsonc` 文件，限定数量和大小，校验路径、UTF-8、JSON5 语法与工作流版本；不跟随目录外路径或符号链接。
4. 下载内容放在应用数据目录的受管来源目录，记录来源 URL、路径、提交 SHA、内容 SHA-256 和本地安装时间；写临时文件、校验后原子替换。同步失败保留上次可用版本。
5. 用户编辑已下载模板时创建本地副本；更新只替换未修改的受管版本。界面明确显示来源与版本，冲突时提供“查看差异 / 保留本地副本 / 更新受管版”。不静默覆盖用户改动。
6. 仅“下载模板”不意味着信任其未来引用的脚本、Skill 或 MCP。v2 模板的额外能力在运行前单独审核。

Git 仓库可访问性已通过 `git ls-remote` 和只读浅克隆核对；CNB 页面本身未能通过网页读取工具打开。实现时先做 `git2` 拉取目录树的可行性验证，复用现有 Git 凭据与代理设置；若服务器不支持所需 Git 传输方式，再确认有文档保证的只读下载接口，不能推测 URL 拼接规则。

### 2.2 分支右键进入运行页

右键本地或远端分支显示“在此分支运行工作流 ▸”，子菜单列出可用模板（数量多时提供搜索或二级选择页）。点击模板只做如下动作：保存 `{repo_tab_id, repo_path, branch_kind, branch_ref, template_id}`，切到工作流页，加载模板，解析输入并展示预览；**不会自动切分支，也不会自动运行**。运行按钮前显式显示“来源分支 → 模板 → 目标分支/远端”的摘要。

来源分支以右键菜单捕获的引用为准：本地如 `dev/a`，远端如 `origin/dev/a`。打开页和点击运行时都检查仓库标签页仍相同、引用仍存在；远端引用若需要更新，必须由模板中的 `fetch` 步骤或用户动作完成。预览若因引用变化失效，要求重新预览。模板若只接受本地分支，页面给出明确错误，不自动 checkout。

兼容规则：普通页面启动的 v1 模板中，`${git.initialBranch}` 继续表示启动时的 HEAD；从分支右键启动时，它表示捕获的来源分支，且在一次运行内保持不变。`${git.currentBranch}` 始终表示运行过程中真正的当前 HEAD。v2 新模板使用语义更明确的 `${run.sourceBranch}` 和 `${run.sourceKind}`；旧模板无须立即修改。执行开始时冻结该上下文，输入默认值、预览与运行使用同一份快照，避免“预览是 A、实际是 B”。

### 2.3 创建、编辑与 AI

本节是早期交互讨论稿；编辑器简化已从当前后续开发计划中移除。

编辑器主流程收敛为“选择起点 → 编排步骤 → 检查并保存”。起点是空白、预设、复制现有模板或 AI 描述。步骤卡片只展示动作、关键参数、输入输出和风险提示；常用字段先显示，高级选项按需展开。运行页负责输入与预览，编辑页不混入运行日志。

AI 生成返回结构化草稿，先做版本/步骤/变量/能力校验；页面展示新增、删除、修改的步骤，用户接受后才写入编辑态。AI 编辑以稳定步骤 ID 定位修改，保留其他字段与用户尚未保存的输入；AI 任务继续沿用 `start_ai_thinking_task` 和会话 ID 守卫。允许“解释失败原因”“只修改选中步骤”“从运行错误提出修复草稿”，但都不自动保存或执行。

UI 首先核对项目锁定的 `gpui-kit = 0.6.4`，再按该版本能力使用 Kit 的菜单、表单、输入和基础按钮。官方当前文档的 Form、Menu、Stepper、Editor 示例只能作为交互参考，不能直接当成 0.6.4 可用 API；不满足已确认焦点、输入和窄窗约束时沿用项目现有薄适配层。

## 3. 执行模型

### 3.1 版本与步骤契约

v1 解析、序列化和运行语义保留。v2 采用稳定步骤 ID、`uses` 类型、`with` 参数、`saveAs` 输出和声明式能力；文件转换必须显式进行，不能保存 v1 时暗中升级。示意：

```json5
{
  version: 2,
  name: "合并来源分支并通知",
  inputs: {},
  steps: [
    { op: "invoke", id: "fetch", uses: "git.fetch", with: { remote: "origin" } },
    { op: "invoke", id: "merge", uses: "git.merge", with: { branch: "${run.sourceBranch}" } },
    { op: "invoke", id: "notify", uses: "mcp.call", with: {
      server: "example-notifier", tool: "post_message",
      arguments: { text: "已合并 ${run.sourceBranch}" }
    } },
  ],
}
```

三步均可运行，但 `example-notifier / post_message` 必须先由用户写入本地 `workflow-mcp.json5` 的工具白名单。当前 `WorkflowActionRegistry` 提供预览、执行与 JSON 输出；MCP/JS 动作在运行前单次授权，设置大小、时间与取消边界。步骤出错默认停止后续步骤；已完成的 Git 或外部动作不会自动回滚。

### 3.2 四类能力

| 能力 | 作用与边界 |
| --- | --- |
| 客户端动作 | 注册命名的 Git/客户端操作，走现有 `GitService`、`TaskExecutor`、刷新事件、凭据与业务守卫；避免暴露任意 UI 内部方法。 |
| MCP | 连接用户显式配置的服务，读取工具清单与输入 schema；工作流按 `server + tool + arguments` 调用，记录工具名、参数摘要、结果和耗时。MCP 服务负责其外部程序能力。 |
| JavaScript 规则 | 用于数据变换、条件判断和编排调用，输入输出必须可序列化；通过宿主提供的受限 API 访问已授权的客户端动作或 MCP，不直接获取文件系统、网络、进程和凭据。限制运行时间、内存、输出量并支持取消。 |
| Skill | 作为 AI 可阅读的指令与资源包，驱动现有 AI 在受限工具集合内完成目标；与确定性的 JS 步骤分开。需要明确模型、预算、工具清单、停止条件和人工确认点；不能把 `SKILL.md` 当可执行脚本。 |

Edge 自动化应由专门的浏览器 MCP 服务提供 `navigate / click / fill / read` 等工具，并用浏览器会话标识和可核对的页面目标传递状态；工作流只调用已授权工具。GPUI Shell 的 JavaScript `window` 是 GPUI 视图宿主，**没有浏览器 DOM**，不能据此控制 Edge。

### 3.3 GPUI Shell 可行性与运行线程

当前仓库锁定 `gpui-kit = 0.6.4`、`gpui-pre = 0.3.5`。官方 0.6.4 `gpui-kit` 清单没有 `shell` feature 或 `gpui-shell` 依赖；当前主线 Shell 文档描述的是后续版本的扩展运行时。P5 采用独立的 `rquickjs` 后台规则运行时，并只通过受限宿主桥接 MCP。是否复用新版 Shell 仍需单独做依赖兼容与线程模型验证。

## 4. 权限、生命周期与数据

- 模板声明能力是申请，不是授权。P5 每次运行显示 JS 数量和 MCP 服务/工具及读写级别，授权仅限当次；模板变化后自然需要重新授权。Git 写操作仍走现有业务守卫。展示外部目标及持久授权属于后续设计，不当成当前功能。
- P5 的本地 stdio MCP 连接配置保存在数据目录，配置文件不得放 secret；子进程自行处理认证与代理。日志仅记服务、工具与状态，不记录参数或输出。网络 MCP 传输及与客户端代理/Keyring 的对接是后续设计。
- 每次运行有 `run_id + repo_tab_id + repo_path + definition_hash + launch_generation`。进度、完成、失败、取消和 panic 回传都按这些身份匹配；旧结果不得覆盖新标签页状态或清掉新运行的 loading。外部动作超时后显示“结果未知”时不能自动重试非幂等操作。
- 本地受管模板目录、来源元数据、授权记录、运行日志分别保存；不要把 secret 写入模板或普通配置。下载模板不触发执行，卸载来源不删除用户复制出的本地模板。

## 5. 待确认的产品决策

这些问题不阻碍先实施模板下载与分支入口，但会改变后续能力边界：

1. **远端模板更新**：建议用户手动检查并确认更新；是否需要后台提示有新版本？
2. **第三方扩展执行**：建议模板下载后仍需按能力授权；是否允许用户安装非官方 JS/Skill 包，还是第一版只允许本地手工导入？
3. **Edge 控制范围**：建议先实现“已有 Edge 会话中的页面导航、读取、点击和填写”，提交表单、上传文件、下载文件及登录流程分别授权；是否有必须支持的首个真实流程？
4. **AI Skill 运行**：建议第一版只对每个 AI 步骤配置模型、工具白名单和上限，遇到高风险工具暂停待确认；是否需要完全无人值守？

## 6. 参考

- 远端模板：[CNB 工作流目录](https://cnb.cool/liuchenchen/work-studio/-/tree/main/work-studio-files)，核对提交 `4fdce5adced2ee6f92bfb61ce8c7fabf472c13b8`。
- [GPUI Kit 0.6.4 清单](https://github.com/longbridge/gpui-kit/blob/v0.6.4/crates/kit/Cargo.toml)、[GPUI Shell 概览](https://gpui-kit.com/shell/)、[Shell 宿主](https://gpui-kit.com/shell/hosting/)、[Shell 能力](https://gpui-kit.com/shell/capabilities/)。
- [Kit Menu](https://gpui-kit.com/zh-CN/component/menu/)、[Kit Form](https://gpui-kit.com/zh-CN/component/form/)、[Kit Stepper](https://gpui-kit.com/zh-CN/component/stepper/)、[Kit Editor](https://gpui-kit.com/zh-CN/component/editor/)；实施时须切到与仓库相同的版本核对。
- [MCP 工具规范](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)、[MCP 传输规范](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)。
