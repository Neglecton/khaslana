# Khaslana 工作流使用说明

Khaslana 工作流用于把一组 Git 操作按顺序自动执行，例如：基于 `master` 创建分支、合并另一个分支、再推送到远端。当前版本支持 JSON5/JSONC 风格的 v1 与 v2 工作流文件；v2 已接入本地 stdio MCP、受限 JavaScript 与 AI Skill，任意 shell 脚本尚未接入。

## 可视化创建与编辑模板（推荐新手）

不想手写 JSON5？在工作流页面点击模板栏的“新建”按钮，用表单式编辑器创建模板；已有模板可以在列表中右键选择“编辑此模板”、“复制为副本”或“删除模板...”：“编辑”会载入原内容到编辑器，保存后覆盖原文件；“复制为副本”同样载入原内容，但保存时会要求另存新文件名，不影响原文件；“删除”需在确认弹窗中二次确认，删除后无法恢复（只删模板文件，不影响仓库或远端）。

1. **从预设开始**（可选）：编辑器提供“同步当前分支”“新建功能分支并推送”“合并分支并推送”三个常用预设，点卡片一键载入后微调；也可以跳过直接添加步骤。
2. **填写基本信息**：模板名称（显示在列表中，可选）和保存文件名（必填）。
3. **添加步骤**：点击“添加步骤”，选择 Skill、JavaScript 或 Git 操作，参数在步骤下方展开填写；支持搜索、上移、下移、复制和删除。Skill 填写已安装的名称与任务描述；JavaScript 填写脚本与输入，工具按需添加。
4. （可选）在上方添加工作流输入（运行前让用户填写），展开“输入说明与自定义变量”补充说明或定义固定值。
5. 点击“保存模板”。Khaslana 会校验必填项、生成 JSON5 文件写入模板目录，并自动选中新模板——之后按下面的运行流程执行即可。

说明：

- **注释提示**：如果原文件包含手工写的 JSON5 注释，进入编辑模式前会弹窗告知——可视化编辑器保存时会重新生成文件，注释与排版无法保留（内容本身不受影响）。想保留注释请继续手工编辑文件，或使用“复制为副本”。
- 编辑模式下保存会**覆盖原文件**（即使原文件是 `.jsonc` 扩展名也会原地更新）；修改文件名后等同于重命名：写入新文件并删除旧文件。
- 切换步骤类型时已填写的同名参数（如分支名）会保留。
- “删除本地分支”等高风险步骤默认开启试运行模式。
- 新建模板默认使用 V2 文档编辑器，可添加 Skill、JavaScript 和 Git 操作。V1 继续使用原编辑器，也可「以 V2 编辑副本」另存。
- MCP 是 Skill／JavaScript 按需调用的工具，添加步骤时不单独提供 MCP。未选择工具时，「调用工具（可选）」默认收起；纯 JavaScript 无需配置 MCP。展开后从按服务分组的菜单添加工具，收起不会清空已选工具。旧模板中的独立 MCP 步骤保留兼容。

### AI 助手（AI 生成 / AI 编辑）

编辑器弹窗顶部提供“AI 助手”区块：用中文描述你想要的模板功能（如“基于 master 创建 release 分支并推送”），点击“AI 生成”，Khaslana 会把需求连同完整的工作流使用文档发给已配置的大模型，生成结果**直接填入下方表单**——你可以在表单里检查、微调每个步骤和参数，确认无误后点“保存模板”即可，无需接触 JSON5 原文。

说明：

- **编辑模式下**，AI 会基于当前模板内容按需求修改（只改需求涉及的部分），同样回填到表单；
- 生成需要先在设置中心配置并启用 AI 供应商，未配置时按钮显示“AI 生成（未配置）”；
- 对结果不满意可以直接调整描述再点一次“AI 生成”重新生成；
- AI 偶尔可能生成不合法内容——解析失败会提示错误且不影响当前表单，重试即可；
- 生成期间可以继续查看和编辑表单，但“AI 生成”按钮在完成前保持加载态。

## 如何运行

1. 打开目标仓库。
2. 点击右上模式切换中的“工作流”，进入工作流页面。
3. 在“模板导航”栏单击模板加载到右侧详情；也可以点击“选择文件”加载任意 `.json5` / `.jsonc` 工作流文件。
4. 如果工作流声明了运行前输入变量，在“变量输入”区域填写这些字符串变量。
5. 在“步骤预览”中确认变量展开后的步骤。
6. 点击“运行”。
7. 在“运行日志”中查看每一步执行状态。

工作流始终作用于当前激活仓库。涉及远端认证时，继续使用 Khaslana 现有凭据机制和认证弹窗。

也可以在本地或远端分支上点击右键，选择“运行工作流…”中的模板。点击后会进入工作流运行页，把右键分支作为来源分支显示；不会自动切换分支或运行。`${git.initialBranch}` 在该入口使用右键分支，普通入口仍使用工作流开始时的当前分支。运行前若该引用已不存在，会报错并停止。

## 工作流模板目录

Khaslana 会在用户目录下准备一个可见的工作流模板目录：

```text
C:\Users\<用户名>\.khaslana\workflows
```

应用启动时会在后台扫描模板；你可以把常用工作流文件放进这个目录，模板栏会发现目录下一级的 `.json5` 和 `.jsonc` 文件。

模板区域支持：

- 点击“新建”打开可视化创建编辑器（见上文）。
- 右键模板行可“编辑此模板”、“复制为副本”或“删除模板...”（见上文）。
- 点击“刷新模板”重新扫描模板目录。
- 点击“打开目录”用系统文件管理器打开模板目录。
- 点击“下载模板”打开 [CNB 工作流模板目录](https://cnb.cool/liuchenchen/work-studio/-/tree/main/work-studio-files) 的模板列表，再逐个点击“下载”。只下载选中的模板；本地同名文件显示“已存在”，不会被覆盖。已下载文件保存在普通模板目录，离线时仍可使用。
- 单击模板行加载模板。加载后仍会进入变量输入、步骤预览和手动运行流程，不会直接执行 Git 操作。
- 运行中可点击“取消运行”；遮罩显示时可点击“停止后续步骤”。取消会等待当前 Git 步骤结束，再阻止后续步骤；已完成的步骤不会撤销。
- 解析失败的模板仍会显示在列表中，双击加载时会显示具体解析错误。

之前版本批量同步到 `remote-cnb` 的模板会一次性导入普通模板目录；同名本地文件优先，旧文件保留作备份但不重复显示，删除已导入的模板后也不会再次出现。下载和浏览不会保存凭据。仓库内的工作流文件不会被自动扫描；如果需要运行仓库中的临时工作流，请继续使用“选择文件”。

## 文件格式

工作流文件使用 JSON5，因此可以写注释、尾随逗号和未加引号的对象 key。

```json5
{
  version: 1,
  name: "基于 master 创建 A 并合并 B",

  defaults: {
    // 默认 true：运行前要求工作区干净
    requireCleanWorktree: true,
  },

  inputs: {
    target: {
      label: "目标分支",
      default: "A-${date:%Y%m%d}",
      required: true,
    },
    source: {
      label: "要合并的分支",
      default: "B",
      required: true,
    },
  },

  vars: {
    remote: "origin",
    base: "master",
  },

  steps: [
    { op: "checkout", branch: "${base}" },
    { op: "pull", remote: "${remote}" },
    { op: "createBranch", name: "${target}", from: "${base}", checkout: true },
    { op: "merge", branch: "${source}" },
    { op: "push", remote: "${remote}", branch: "${target}", setUpstream: true },
  ],
}
```

### 顶层字段

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `version` | 是 | 支持 `1` 和 `2`；新建模板使用 V2，编辑 V1 时保持原版本。 |
| `name` | 否 | 工作流显示名称。为空时显示“未命名工作流”。 |
| `defaults` | 否 | 默认行为设置。 |
| `inputs` | 否 | 运行前让用户在页面填写的字符串变量表。 |
| `vars` | 否 | 用户自定义变量表，值必须是字符串。 |
| `steps` | 是 | 要顺序执行的步骤数组，至少需要一个步骤。 |

### v2 可扩展步骤

v2 可以使用 `op: "invoke"`、唯一的 `id`、`uses` 和 `with` 参数调用动作。已内置 `git.checkout`、`git.fetch`、`git.pull`、`git.createBranch`、`git.merge`、`git.push`、`git.guardRemoteBranch`、`git.ensureClean`、`git.assertBranch`、`git.filterBranches`、`git.deleteBranches`，参数与下文同名 v1 步骤一致。v1 文件及步骤运行方式保持原样。

```json5
{
  version: 2,
  steps: [
    { op: "invoke", id: "clean", uses: "git.ensureClean" },
    { op: "invoke", id: "create", uses: "git.createBranch",
      with: { name: "demo", from: "${run.sourceBranch}", checkout: false } },
  ],
}
```

`with` 默认是空对象。`${run.sourceBranch}` 表示本次选定的来源分支，`${run.sourceKind}` 返回 `local` 或 `remote`；从普通入口运行时来源分支为启动时的当前分支。扩展动作可通过 `saveAs` 把 JSON 结果传给后续步骤（如 `${result.branch}`）。内置 `git.*` 动作不支持 `saveAs`，已有输出参数仍按 v1 用法填写。

#### 本地 MCP 服务与 JavaScript

自定义 MCP 服务可在当前数据目录的 `workflow-mcp.json5` 中显式配置；工作流模板本身不能指定启动命令。内置 `browser.edge` 服务无需此配置。可点击工作流页的“目录”打开模板目录，再到上一级数据目录。自定义服务配置示例：

```json5
{
  servers: {
    localTools: {
      command: "C:\\path\\to\\mcp-server.exe",
      args: [],
      tools: {
        read_page: { access: "read" },
        submit_form: { access: "write" },
      },
    },
  },
}
```

`access` 由本地配置者指定，不采用服务自行声明的只读提示。当前只支持本地 stdio 服务；自定义服务的进程环境继承客户端。需要认证或网络代理的自定义服务须自行处理，客户端不会把自己的代理与凭据隐式传给子进程。配置中的命令及参数不要写入 secret。内置 `browser.edge` 使用客户端代理设置；浏览器代理暂不支持账号密码或 HTTP、HTTPS 分别设置不同代理。

在「设置 → AI 设置 → MCP → 添加服务」中可直接填写 `npx`，参数每行一个，例如 `-y` 和 `chrome-devtools-mcp@latest`。Windows 自动解析 PATH 中的可执行程序及 `.cmd` / `.bat` 入口，保留完整路径与原有 `npx.cmd` 配置；标准 npm / npx 入口优先由同目录的 Node 直接执行。npx 默认使用当前应用数据目录下的 `workflow-npm-cache`，与其它客户端隔离；显式设置 `npm_config_cache` 时沿用该目录。首次连接可能下载 npm 包，连接测试最多等待 120 秒，只读取工具列表，不调用工具。自定义 `npx` 服务需要本机安装 Node.js（含 npm），安装后重启客户端；「运行环境」中的内置浏览器组件不提供通用 npm / npx 安装。

填写服务 ID、命令和参数即可保存，无需先安装或测试连接。新服务默认自动发现工具；配置文件省略 `tools` 时也采用此策略。未知工具按写入请求本次运行授权，服务的只读声明不能代替授权。`enabled: false` 禁用服务；`autoDiscover: false` 则只允许 `tools` 白名单。旧配置显式填写的 `tools` 保持白名单含义，不会自动扩张权限。

「测试连接」只读取工具列表；成功后保存的 `cachedTools` 是名称展示缓存，不能代替真实工具定义或授予权限。「高级配置（可选）」可调整读写提示；关闭个别工具会切换为白名单模式，重新测试保留已有选择。进入 AI 设置、运行环境或工作流编辑器只读取本地记录，不启动 Node 或 MCP；检测系统 Node 需在运行环境页点击「重新检测」，安装时也会主动检查。

旧模板直接调用 MCP 工具的兼容格式如下；新建流程请在 Skill 或 JavaScript 步骤内调用工具：

```json5
{ op: "invoke", id: "read", uses: "mcp.call", saveAs: "page",
  with: { server: "localTools", tool: "read_page", arguments: { url: "https://example.com" } } }
```

服务名和工具名必须是固定值；`arguments` 是对象，可引用工作流变量。运行时会连接服务、读取工具清单、根据工具的 JSON Schema 校验参数，之后调用一次；服务声明了输出 schema 时也校验结构化结果。同一工作流运行内会复用同一服务连接与工具定义缓存，重新连接时重新发现；结束、取消或失败后关闭。工具返回的结构化结果优先作为 `saveAs` 输出；没有结构化结果时保存内容数组。出错后停止后续步骤，不自动重试写入工具。预览阶段无法知道工具结果，后续引用 `saveAs` 的表达式会原样显示，运行时才展开。

JavaScript 规则以函数体形式编写，可读取 `input`，通过 `return` 返回 JSON 值。仅当 `tools` 明确列出服务和工具时，才可调用 `mcp.call(server, tool, arguments)`：

```json5
{ op: "invoke", id: "transform", uses: "js.run", saveAs: "result",
  with: {
    input: { text: "hello" },
    tools: [{ server: "localTools", tool: "read_page" }],
    script: "const page = mcp.call('localTools', 'read_page', {url: input.text}); return {page};",
  } }
```

脚本没有文件、网络或进程 API；JavaScript 源码中的 `${...}` 保留为 JS 模板字符串，`input` 字段可使用工作流变量。每段脚本限制 8 MiB 内存、512 KiB 栈、40 秒、256 KiB 输入与 1 MiB 输出；MCP 参数限制 256 KiB、输出限制 1 MiB。运行前弹框展示脚本摘要及每个 MCP 工具的读写权限，授权只对这一次运行有效；关闭弹框或取消即不执行。MCP/JS 步骤运行中可取消，已完成的写入不会撤销。运行日志只记录服务、工具、状态与耗时，不记录传入参数与输出内容。

#### 本地 AI Skill

将已检查的 Skill 包放在当前数据目录的 `workflow-skills/<名称>/SKILL.md`；这是显式本地安装，不会自动从网络下载或更新。名称只允许小写字母、数字和连字符。`SKILL.md` 顶部需要简短清单，`name` 必须与目录名一致：

```markdown
---
name: edge-web-form
description: 读取公开演示表单并填写文本框。
---

这里写给 AI 的步骤说明。
```

可在同包的 `references/` 放最多 16 个 `.md` 或 `.txt` 文件，运行前与主文件一起读取；包内容总量最多 128 KiB。链接、嵌套资源和其他文件类型不会作为资源读取。预览会校验已安装的包与 AI 配置，运行确认前冻结所用包内容并展示摘要指纹。确认只针对本次运行，安装目录后续变化不会改变这次运行的指令。

`skill.run` 使用当前启用的 AI 供应商。`task` 是本次任务；`tools` 声明模型能调用的 MCP 工具，须符合对应服务的工具策略（自动发现或白名单），并经过本次运行确认：

```json5
{ op: "invoke", id: "assistant", uses: "skill.run", saveAs: "answer",
  with: { skill: "edge-web-form", task: "读取页面并填写示例内容",
    tools: [{ server: "browser.edge", tool: "browser_snapshot" }] } }
```

模型最多运行 6 轮、调用工具 8 次；每轮输出上限为当前供应商设置与 4096 token 中的较小值，整体不超过 180 秒，提示与工具结果累计限制 256 KiB，单个工具结果限制 32 KiB。每次工具调用在宿主侧重新核对步骤白名单和 MCP schema。工具失败不自动重试；流式 AI 请求支持取消与截止时间检查，网络读空闲最长 30 秒。运行日志逐条记录 AI 给出的简短操作说明及工具名、状态和耗时，不记录工具参数或完整工具结果。`saveAs` 保存最终文字。

运行 Skill 前，宿主读取本步骤已授权工具的实际参数定义并提供给模型，避免对不同 MCP 的参数格式作猜测。只读取工具列表，不调用工具；工具定义最多 64 KiB，计入整体上下文预算，定义本身不能扩大授权范围。读取定义与后续调用复用本次运行的 MCP 会话。

#### 新建分支、合并并填写浏览器

[分支填写样板](examples/branch-to-browser/README.md)提供可导入的 v2 模板与 Skill 包：创建并切换到新分支，合并指定来源，核对当前分支，再通过 Chrome DevTools MCP 将 `${target}` 自动传给 Skill 并填入公开表单的 Text input。无需系统剪贴板。模板只使用 `new_page`、`take_snapshot`、`fill`，不提交表单。Git 步骤失败或合并发生冲突时停止，不执行浏览器步骤。

浏览器场景可在 `with.browserGuard` 设置固定 HTTPS `url`、页面快照必须包含的 `contains` 文本和允许填写的 `target`。有守卫时仅允许导航、快照和填写工具；宿主会阻止错误网址、未验证页面、错误输入目标和缺少 `submit: false` 的填写，并要求填写后快照能读到填写内容。页面不匹配或超时会停止工作流。

#### Edge 公开演示样板

[样板目录](examples/workflow-edge-demo/)提供确定性 JS 工作流和 AI Skill 工作流。安装 Microsoft Edge 后，把两个 `.json5` 模板复制到 `workflows/`；运行时若缺少浏览器 MCP 组件，点击“下载并启用”，客户端会先检查本机 Node，必要时将 Node 与固定版本的 [Playwright MCP](https://github.com/microsoft/playwright-mcp) 下载到当前应用数据目录。下载完成后继续本次运行授权。如需 AI 版本，还需将 `workflow-skills/edge-web-form/` 复制到数据目录的同名位置并启用 AI。系统不需要预先安装 npm 或 npx；原样板中的 `edgeDemo` 配置仍可被客户端识别为内置服务，用户自定义过的同名配置保持原样。

两个模板都只访问 [Selenium 公开表单](https://www.selenium.dev/selenium/web/web-form.html)，读取页面并填写 `Text input`，不点击 Submit，也不提交或保存。MCP 白名单只有 `browser_navigate`、`browser_snapshot` 和 `browser_type`。`edge-web-form.json5` 使用 JS 固定校验页面与输入框后填写；`edge-web-form-skill.json5` 让 AI 按包说明操作，同时由页面守卫在宿主侧核对 URL、页面文本、输入目标和填写后的快照。真实页面可能更新；识别失败时应检查演示页结构再修改样板。

### defaults

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `requireCleanWorktree` | `true` | 运行前检查工作区是否干净。若存在未提交更改，工作流会拒绝运行。 |

### inputs

`inputs` 用来声明运行工作流前需要用户填写的字符串变量。每个 key 会成为同名变量，可直接用 `${变量名}` 引用。

```json5
inputs: {
  target: {
    label: "目标分支",
    description: "例如 feature/demo",
    default: "feature/${date:%Y%m%d}",
    required: true,
  },
  source: {
    label: "来源分支",
    default: "B",
  },
}
```

字段：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `label` | 变量名 | 输入框显示名称。 |
| `description` | 无 | 输入框下方的说明文字。 |
| `default` | 空字符串 | 输入框初始值，支持 `${...}` 插值。 |
| `required` | `true` | 是否必填；必填项为空时无法预览和运行。 |

页面输入的值只在本次预览和运行中生效，不会写回工作流文件或用户配置。输入值会覆盖同名 `vars` 值；如果没有同名 `vars`，输入值本身也可以直接作为变量使用。

## 支持的步骤

所有步骤都使用 `op` 字段声明类型。字符串字段都支持 `${...}` 变量插值。

### checkout

切换到本地分支。

```json5
{ op: "checkout", branch: "master" }
```

字段：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `branch` | 是 | 本地分支名。 |

### fetch

获取远端引用。

```json5
{ op: "fetch", remote: "origin" }
```

字段：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `remote` | 否 | 远端名。省略时使用当前选中的远端；如果没有选中远端则使用 `origin`。 |

### pull

从远端拉取当前分支对应的上游分支。

```json5
{ op: "pull", remote: "origin" }
```

字段：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `remote` | 否 | 远端名。省略时使用当前选中的远端；如果没有选中远端则使用 `origin`。 |

### createBranch

创建本地分支，可指定起点，并可创建后立即切换过去。

```json5
{ op: "createBranch", name: "feature/demo", from: "master", checkout: true }
```

字段：

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `name` | 是 | 无 | 新分支名。 |
| `from` | 否 | 当前 `HEAD` | 起点分支或引用。 |
| `checkout` | 否 | `true` | 创建后是否切换到新分支。 |

如果创建的新分支后续会推送到远端，建议先用 `guardRemoteBranch` 检查远端同名分支是否已存在：

```json5
steps: [
  { op: "fetch", remote: "${remote}" },
  { op: "guardRemoteBranch", remote: "${remote}", branch: "${target}", fetch: false },
  { op: "createBranch", name: "${target}", from: "${base}", checkout: true },
]
```

### guardRemoteBranch

检查远端分支是否存在，可用于在创建本地分支或推送前提前停止工作流。

```json5
{ op: "guardRemoteBranch", remote: "origin", branch: "feature/demo" }
```

字段：

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `remote` | 否 | 当前选中远端或 `origin` | 要检查的远端。 |
| `branch` | 是 | 无 | 远端分支名，不带 `origin/` 这类远端名前缀。 |
| `fetch` | 否 | `true` | 检查前是否先获取该远端。 |
| `onExists` | 否 | `"fail"` | 远端分支存在时的行为，可选 `"fail"` 或 `"continue"`。 |
| `onMissing` | 否 | `"continue"` | 远端分支不存在时的行为，可选 `"fail"` 或 `"continue"`。 |

默认行为是：远端已有同名分支时停止，远端没有该分支时继续。若要检查远端分支必须存在，可写：

```json5
{ op: "guardRemoteBranch", remote: "origin", branch: "release/demo", onExists: "continue", onMissing: "fail" }
```

### merge

把指定分支合并到当前分支。

```json5
{ op: "merge", branch: "B" }
```

字段：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `branch` | 是 | 要合并进当前分支的本地分支、远端分支或引用。 |

如果合并产生冲突，工作流会停止，并保留冲突状态供用户处理。

### push

推送本地分支到远端同名分支。

```json5
{ op: "push", remote: "origin", branch: "feature/demo", setUpstream: true }
```

字段：

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `remote` | 否 | 当前选中远端或 `origin` | 目标远端。 |
| `branch` | 否 | 当前分支 | 要推送的本地分支。 |
| `setUpstream` | 否 | `true` | 推送成功后是否设置 upstream。 |

省略 `branch` 时，会使用执行到该步骤时的当前分支；如果前面有 `checkout` 或 `createBranch` 且 `checkout: true`，步骤预览也会按这个分支推断显示。

### ensureClean

检查工作区是否干净。

```json5
{ op: "ensureClean" }
```

如果存在未提交更改，工作流会停止。即使 `defaults.requireCleanWorktree` 被设置为 `false`，也可以在关键步骤前手动插入这个检查。

### assertBranch

确认当前分支符合预期。

```json5
{ op: "assertBranch", branch: "release/${date:%Y%m%d}" }
```

字段：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `branch` | 是 | 期望的当前分支名。 |

如果当前分支不一致，工作流会停止。

### filterBranches

按命名规则筛选符合条件的本地分支，把结果作为一个**数组变量**写入步骤输出，供后续步骤消费。该步骤**只读**，不改动仓库；预览阶段即可看到命中清单。

```json5
{
  op: "filterBranches",
  output: "out.staleBranches",
  pattern: "^(dev|uat|release)_[^_]+_(?P<date>\\d{8})_",
  dateFormat: "%Y%m%d",
  dateGroup: "date",
  olderThanMonths: "3",
  skipCurrent: true,
}
```

字段：

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `output` | 是 | — | 输出变量名，筛选命中的分支数组写入此处（供后续步骤通过 `${output}` 消费）。 |
| `pattern` | 是 | — | 正则表达式，需包含至少一个日期捕获组。优先取 `dateGroup` 指定的命名组，否则回退到第一个捕获组。 |
| `dateFormat` | 否 | `%Y%m%d` | chrono 日期格式，用于解析捕获组里的日期。 |
| `dateGroup` | 否 | `date` | 日期命名捕获组名。 |
| `olderThanMonths` | 是 | — | 距今月数阈值，插值后解析为整数；分支日期距今月数**大于**该值才入选。 |
| `skipCurrent` | 否 | `true` | 是否把当前分支排除在结果之外。 |

筛选逻辑：遍历所有本地分支，对分支名执行 `pattern` 匹配；匹配成功后用 `dateFormat` 解析日期捕获组，计算该日期距今的**日历月数差**（`(今天.年 - 分支.年) * 12 + (今天.月 - 分支.月)`），大于 `olderThanMonths` 才入选。日期无法解析的分支会被归入"跳过（日期无法解析）"，未超期的归入"跳过（未超过阈值）"，这两类都不会出现在输出数组里。

配合字符串方法 `alt` 可以把逗号分隔的前缀枚举转成正则交替分组，避免硬编码：

```json5
inputs: {
  prefixes: { label: "前缀（逗号分隔）", default: "dev,uat,release" },
},
vars: {
  // 注意：管道方法必须整体写在 ${...} 内部，写成 "${prefixes|alt}"，
  // 不能写成 "${prefixes}|alt"（那样 |alt 会变成字面文本，不会被执行）。
  prefixAlt: "${prefixes|alt}",  // → "(dev|uat|release)"
},
steps: [
  {
    op: "filterBranches",
    output: "out.staleBranches",
    pattern: "^${prefixAlt}_[^_]+_(?P<date>\\d{8})_",
    olderThanMonths: "${months}",
  },
],
```

> 命名捕获组 `(?P<date>...)` 在 JSON5 里反斜杠需要写成 `\\d`。

输出变量是数组语义，后续可以直接用数组方法消费，例如 `${out.staleBranches}|join:", "` 生成摘要文案，或传给下面的 `deleteBranches`。

### deleteBranches

删除一批本地分支（仅本地，不涉及远端）。通常配合 `filterBranches` 使用：把上一步的数组输出传给 `branches`。

```json5
{
  op: "deleteBranches",
  branches: "${out.staleBranches}",
  dryRun: true,
  skipCurrent: true,
}
```

字段：

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `branches` | 是 | — | 分支列表。可以是数组变量（如 `${out.staleBranches}`），也可以是换行分隔的字符串。 |
| `dryRun` | 否 | `true` | 试运行模式：只列出将要删除的分支，不真正删除。建议先 `true` 预览清单，确认后改 `false` 正式删除。 |
| `skipCurrent` | 否 | `true` | 是否自动跳过当前分支（即使它出现在列表里也不删除）。 |

`branches` 既支持数组变量，也支持字符串。如果传入的是数组变量（如 `${out.staleBranches}`），会原样作为分支列表；如果是字符串，会按换行切分并清理空行。删除是危险操作，默认 `dryRun: true`；正式删除时批量执行，仅刷新一次快照。

## 变量与字符串拼接

任意字符串字段都支持 `${...}` 插值。变量可以和普通文本拼接：

```json5
{
  vars: {
    target: "release/${date:%Y%m%d}-${git.initialBranch}",
  },
  steps: [
    { op: "createBranch", name: "${target}", from: "master" },
  ],
}
```

### 用户变量

用户变量定义在 `vars` 中：

```json5
vars: {
  base: "master",
  target: "feature/${git.repoName}-${date:%Y%m%d}",
}
```

使用方式：

```json5
{ op: "checkout", branch: "${base}" }
{ op: "createBranch", name: "${target}" }
```

用户变量可以引用其他变量或内置变量。循环引用会报错并停止解析，例如 `a -> b -> a`。

如果同名变量同时出现在 `inputs` 和 `vars` 中，页面输入值优先：

```json5
{
  inputs: {
    target: { label: "目标分支", default: "feature/demo" },
  },
  vars: {
    target: "fallback",
  },
  steps: [
    { op: "createBranch", name: "${target}" },
  ],
}
```

上例中 `${target}` 会使用用户在页面输入的值，而不是 `fallback`。

`inputs` 不能使用内置变量命名空间，例如 `git.currentBranch`、`git.repoName`、`run.id`、`run.startedAt:*` 或 `date:*`。

### 日期变量

| 写法 | 说明 | 示例结果 |
| --- | --- | --- |
| `${date:%Y%m%d}` | 工作流启动日期 | `20260609` |
| `${date:%Y-%m-%d}` | 年月日 | `2026-06-09` |
| `${date:%Y%m%d-%H%M%S}` | 日期和时间 | `20260609-142530` |

日期格式使用 Rust `chrono` 的格式语法。常用片段：

| 片段 | 含义 |
| --- | --- |
| `%Y` | 四位年份 |
| `%m` | 两位月份 |
| `%d` | 两位日期 |
| `%H` | 两位小时，24 小时制 |
| `%M` | 两位分钟 |
| `%S` | 两位秒 |

### 运行变量

| 变量 | 说明 |
| --- | --- |
| `${run.id}` | 本次运行 ID，基于启动时间毫秒。 |
| `${run.startedAt:%Y%m%d}` | 本次运行启动时间，可自定义日期格式。 |
| `${run.sourceBranch}` | 本次运行的来源分支；从分支右键启动时取右键分支。 |
| `${run.sourceKind}` | 来源分支种类，`local` 或 `remote`。 |

### Git 变量

| 变量 | 说明 |
| --- | --- |
| `${git.initialBranch}` | 普通入口取工作流开始时的当前分支；分支右键入口取右键分支。运行过程中不会变化。 |
| `${git.currentBranch}` | 每个步骤执行前读取到的当前分支。切换分支后会变化。 |
| `${git.head}` | 当前 `HEAD` 指向的提交 SHA。 |
| `${git.repoName}` | 当前仓库目录名。 |

注意：如果当前处于 detached HEAD，`${git.initialBranch}` 或 `${git.currentBranch}` 可能无法解析，工作流会报错。

步骤预览会基于前置 `checkout` 和 `createBranch checkout: true` 推断 `${git.currentBranch}`，但不会模拟 fetch、pull 或 merge 的真实 Git 结果。

### 内置方法

`${...}` 表达式支持管道方法，用来对变量结果做简单处理：

```json5
vars: {
  shortHead: "${git.head | truncate:7}",
  sourceName: "${git.initialBranch | split:'/' | last}",
  target: "feature/${git.initialBranch | split:'/' | last | slug | truncate:24}",
}
```

管道从左到右执行。方法参数用 `:` 分隔；如果参数中包含 `:` 或 `|`，请用单引号或双引号包裹，例如 `replace:'|':'-'`。

字符串方法：

| 方法 | 说明 |
| --- | --- |
| `trim` | 去掉首尾空白。 |
| `lower` / `upper` | 转为小写 / 大写。 |
| `replace:from:to` | 字面量替换所有 `from` 为 `to`。 |
| `truncate:n` | 保留前 `n` 个字符。 |
| `suffix:n` | 保留后 `n` 个字符。 |
| `default:value` | 当前值为空白时使用 `value`。 |
| `slug` | 转小写，将非 ASCII 字母数字替换为 `-`，并合并连续 `-`。 |
| `split:delimiter` | 按字面量分隔符拆成数组。 |
| `alt` | 把逗号分隔列表转成正则交替分组，便于嵌入 `pattern`。例：`"dev,uat,release" \| alt` → `"(dev|uat|release)"`。 |

数组方法：

| 方法 | 说明 |
| --- | --- |
| `compact` | 移除空白元素。 |
| `first[:default]` | 取第一个元素，数组为空时可使用默认值。 |
| `last[:default]` | 取最后一个元素，数组为空时可使用默认值。 |
| `nth:index[:default]` | 取第 `index` 个元素，`index` 从 0 开始。 |
| `join:delimiter` | 用分隔符把数组拼回字符串。 |

`split` 产生的数组只能在管道中临时使用，最终必须通过 `first`、`last`、`nth` 或 `join` 转回字符串。内置方法本身不支持循环、条件或方法参数里的 `${...}` 嵌套插值。

> **管道方法必须整体写在 `${...}` 内部。** 写 `${prefixes|alt}` 会对 `prefixes` 应用 `alt` 方法；而 `${prefixes}|alt` 会把 `${prefixes}` 插值后，把 `|alt` 当作字面文本原样拼接，方法不会被执行。其他方法（`split`、`join`、`slug` 等）同理。

#### 步骤输出

部分步骤会把结果作为**数组变量**写入步骤输出，供后续步骤消费：

- `filterBranches` 通过 `output` 字段指定输出变量名，筛选命中的分支数组会写入该变量。

后续步骤可以直接引用这些变量。当某个 `${...}` 占位整段就是一个数组变量时（如 `branches: "${out.staleBranches}"`），会保留数组语义，无需先 `join` 回字符串；也可以继续用数组方法处理，例如 `${out.staleBranches}|join:", "`。

## 完整示例

### 示例 1：从 master 拉取后创建发布分支

```json5
{
  version: 1,
  name: "创建当天发布分支",
  vars: {
    remote: "origin",
    base: "master",
    release: "release/${date:%Y%m%d}",
  },
  steps: [
    { op: "checkout", branch: "${base}" },
    { op: "pull", remote: "${remote}" },
    { op: "guardRemoteBranch", remote: "${remote}", branch: "${release}", fetch: false },
    { op: "createBranch", name: "${release}", from: "${base}", checkout: true },
    { op: "push", remote: "${remote}", branch: "${release}", setUpstream: true },
  ],
}
```

### 示例 2：基于 master 创建 A，合并 B，再推送 A

```json5
{
  version: 1,
  name: "创建 A 并合并 B",
  inputs: {
    target: { label: "目标分支", default: "A" },
    source: { label: "要合并的分支", default: "B" },
  },
  vars: {
    remote: "origin",
    base: "master",
  },
  steps: [
    { op: "checkout", branch: "${base}" },
    { op: "guardRemoteBranch", remote: "${remote}", branch: "${target}" },
    { op: "createBranch", name: "${target}", from: "${base}", checkout: true },
    { op: "merge", branch: "${source}" },
    { op: "assertBranch", branch: "${target}" },
    { op: "push", remote: "${remote}", branch: "${target}", setUpstream: true },
  ],
}
```

### 示例 3：用仓库名和当前分支生成临时分支

```json5
{
  version: 1,
  name: "创建临时工作分支",
  vars: {
    branch: "tmp/${git.repoName}-${git.initialBranch}-${run.startedAt:%Y%m%d-%H%M%S}",
  },
  steps: [
    { op: "ensureClean" },
    { op: "createBranch", name: "${branch}", checkout: true },
  ],
}
```

### 示例 4：清理过期命名分支

按 `(dev|uat|release)_xxx_yyyyMMdd_xxx` 这类命名规则筛选超过 N 个月的本地分支并删除。先用 `filterBranches` 筛选，再用 `deleteBranches` 删除，两步解耦，筛选结果也可用于其他用途。

```json5
{
  version: 1,
  name: "清理过期命名分支",
  defaults: { requireCleanWorktree: true },
  inputs: {
    prefixes: {
      label: "前缀（逗号分隔）",
      description: "只筛选这些前缀开头的分支",
      default: "dev,uat,release",
      required: true,
    },
    months: {
      label: "超过多少个月",
      description: "分支名里的日期距今超过该月数才会被选中",
      default: "3",
      required: true,
    },
  },
  vars: {
    // 注意：管道方法必须整体写在 ${...} 内部（"${prefixes|alt}"），
    // 否则 |alt 会被当作字面文本，不会被当作方法执行。
    prefixAlt: "${prefixes|alt}",  // → "(dev|uat|release)"
  },
  steps: [
    {
      op: "filterBranches",
      output: "out.staleBranches",
      // 插值 + alt 后变为: ^(dev|uat|release)_[^_]+_(?P<date>\d{8})_
      pattern: "^${prefixAlt}_[^_]+_(?P<date>\\d{8})_",
      dateFormat: "%Y%m%d",
      dateGroup: "date",
      olderThanMonths: "${months}",
      skipCurrent: true,
    },
    {
      op: "deleteBranches",
      branches: "${out.staleBranches}",
      dryRun: true,   // 先 true 预览清单；确认无误后改 false 正式删除
      skipCurrent: true,
    },
  ],
}
```

运行后预览和日志里会逐行列出命中的分支（如 `命中：dev_wzf_20250418_测试系统`）。确认清单无误后把 `dryRun` 改成 `false` 重新运行即可真正删除。筛选出的 `${out.staleBranches}` 也能用 `${out.staleBranches}|join:", "` 生成摘要文案。

## 当前限制

- 只支持顺序执行，不支持条件、循环、并发或手动暂停。
- 不支持任意 shell、Python 脚本；JavaScript 仅可通过 v2 `js.run` 在受限运行时执行。
- 不支持跨仓库编排；工作流只作用于当前激活仓库。
- 不支持行级别或 hunk 级别操作。
- 内置方法只支持字符串处理和临时数组取值，不支持循环、条件；正则匹配只在 `filterBranches` 步骤的 `pattern` 中支持，内置管道方法本身不支持正则。
- 取消运行目前只在步骤之间有意义；正在执行的 Git 远程操作不会被强制中断。
- 用户模板目录会自动发现 `.json5` / `.jsonc` 文件；仓库内工作流不会自动扫描，需要通过“选择文件”手动选择。

## 建议

- 默认保持 `requireCleanWorktree: true`，避免自动流程覆盖或混入本地未提交修改。
- 对关键流程添加 `assertBranch`，防止工作流在意外分支上继续执行。
- 使用变量生成分支名时，优先包含日期或运行时间，避免重复分支名。
- 推送前先确认远端和凭据配置，远端步骤会使用 Khaslana 当前的远端和凭据机制。
