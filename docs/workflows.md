# Khaslana 工作流使用说明

Khaslana 工作流用于把一组 Git 操作按顺序自动执行，例如：基于 `master` 创建分支、合并另一个分支、再推送到远端。当前版本只支持 JSON5/JSONC 风格的结构化工作流文件，不支持 YAML、JavaScript、Python 或任意 shell 脚本。

## 如何运行

1. 打开目标仓库。
2. 点击右上模式切换中的“工作流”，进入工作流页面。
3. 在中间“工作流模板”栏单击模板可选中，双击模板会加载到右侧详情；也可以点击“选择文件”加载任意 `.json5` / `.jsonc` 工作流文件。
4. 如果工作流声明了运行前输入变量，在“变量输入”区域填写这些字符串变量。
5. 在“步骤预览”中确认变量展开后的步骤。
6. 点击“运行”。
7. 在“运行日志”中查看每一步执行状态。

工作流始终作用于当前激活仓库。涉及远端认证时，继续使用 Khaslana 现有凭据机制和认证弹窗。

## 工作流模板目录

Khaslana 会在用户目录下准备一个可见的工作流模板目录：

```text
C:\Users\<用户名>\.khaslana\workflows
```

当前版本会在进入工作流页面时自动创建这个目录。你可以把常用工作流文件放进去，中间“工作流模板”栏会自动发现目录下一级的 `.json5` 和 `.jsonc` 文件。

模板区域支持：

- 点击“刷新模板”重新扫描模板目录。
- 点击“打开目录”用系统文件管理器打开模板目录。
- 单击模板行只选中模板；双击模板行加载模板。加载后仍会进入变量输入、步骤预览和手动运行流程，不会直接执行 Git 操作。
- 解析失败的模板仍会显示在列表中，双击加载时会显示具体解析错误。

模板目录只用于用户主动管理的工作流文件，不存放凭据、会话或应用内部配置。仓库内的工作流文件不会被自动扫描；如果需要运行仓库中的临时工作流，请继续使用“选择文件”。

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
| `version` | 是 | 当前只支持 `1`。 |
| `name` | 否 | 工作流显示名称。为空时显示“未命名工作流”。 |
| `defaults` | 否 | 默认行为设置。 |
| `inputs` | 否 | 运行前让用户在页面填写的字符串变量表。 |
| `vars` | 否 | 用户自定义变量表，值必须是字符串。 |
| `steps` | 是 | 要顺序执行的步骤数组，至少需要一个步骤。 |

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

### Git 变量

| 变量 | 说明 |
| --- | --- |
| `${git.initialBranch}` | 工作流开始时的当前分支。运行过程中不会变化。 |
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
- 不支持执行 shell、JavaScript、Python 等脚本。
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
