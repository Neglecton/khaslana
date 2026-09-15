# CU2-T5 视觉与状态验收记录

基准设计稿（Pencil 文档 `pencil-new.pen`）：

- `ggnTD`「CU2-T5 · 用户流程与状态细化」：六种中央工作区可见状态（S1 / S3 / B2 / B3 / S4 / S5）、
  四条异常分支（B1～B4）、任务生命周期与实现约束。
- `bi8Au`「CU2-T5 · 代码理解」1440×900：完成/来源态的**结构基准**
  （§9.5 指定的实现顺序：40px 页面头 → 94px 唯一提问区 → 可滚动回答区；右侧 360px 来源侧栏）。
- `a6rLm`「页面状态细化」在文档中 `enabled: false` 且无子节点，不作为基准。

本轮改动全部落在中央工作区与来源侧栏，标题栏、Context Navigator、仓库切换器与底部状态栏保持现状。

## 1. 逐状态对照

| 设计节点 | 设计要点 | 实现位置 | 状态 |
| --- | --- | --- | --- |
| `bi8Au/Page Header` | 左「代码理解 / 用源码回答业务问题」，右「索引就绪」「基础分析」两个轻量徽标 | `code_understanding_view.rs` `render_understanding_header`、`understanding_index_state` / `UnderstandingIndexState::palette` | 已对齐 |
| `ggnTD/S1 Context Row` | 仓库引用一行 + 索引状态徽标 | `render_understanding_context_row` | 已对齐（新增） |
| `ggnTD/S1 Empty Copy` | 「你想了解这个项目的什么？」+「AI 会按需搜索并阅读有限源码，不会后台上传整个仓库。」 | `render_understanding_empty` | 已对齐（替换旧文案） |
| `ggnTD/S1 Question Composer` | 一体化边框容器：占位文案 + 「Enter 发送 · Shift+Enter 换行」+「发送」主按钮 | `render_understanding_input_area`（`show_hint` 分支）、`render_understanding_send_button` | 已对齐（新增占位与提示） |
| `ggnTD/S1 Examples` | 「试试这些问题」+ 可点选的 sunken 行 | `render_local_history` | 已按用户要求改为**本地历史**列表（示例问题整块移除） |
| `ggnTD/S1 History Restore` | 单张最近完成卡：问题 + 时间/来源数 + 「打开记录」 | `render_local_history` | 已合并进本地历史列表（原单卡恢复入口取消） |
| `bi8Au/Question Composer` | 94px 常驻提问区，右侧「提问」 | `render_understanding_finished` 顶部条 + `render_understanding_input_area(window, cx, "提问", false)` | 已对齐 |
| `ggnTD/S3 Timeline` | 17px 圆角动作标记 + 面向用户动作文案（无工具名/JSON） | `timeline_row`、`understanding_step_action`、`understanding_tool_action` | 已对齐（新增动作映射） |
| `ggnTD/S3 Question + Elapsed` | sunken 问题条 + `mm:ss` 耗时徽标 | `render_question_strip`、`understanding_elapsed_label` | 已对齐（新增） |
| `ggnTD/S3 Partial Answer` | 「正在生成回答」+「已读取 N 个来源」+ 增量正文 | `render_understanding_running` | 已对齐（新增来源计数与正文区） |
| `ggnTD/S3 Actions` | 「切换页面不会停止任务」+ 后台运行 / 取消 | `render_understanding_running` 底部动作条 | 已对齐 |
| `ggnTD/B2 Background Task Bar` | 中央工作区下方任务条：标记 + 「代码理解正在后台分析」+ 进度·耗时 + 查看进度 / 取消 | `render_understanding_task_bar`（挂载于 `main.rs` 主壳层，状态栏之上） | 已对齐（新增） |
| `ggnTD/B2 Global Status` | 状态栏显示「代码理解运行中」与「AI 任务 n/3」 | `repository_ui.rs` `render_status` | 已对齐（原状态栏内嵌查看/取消气泡已收敛为指示 + 全局名额） |
| `ggnTD/B3 Error Card` | 「本次分析未完成」+ 原因 + 「复制错误详情」/「重试」 | `render_understanding_failed` | 已对齐（新增复制详情） |
| `ggnTD/B3 Draft Rule` | 「问题已保留；失败结果不会写入历史」 | `render_understanding_failed` 尾部规则条 | 已对齐（新增） |
| `ggnTD/S4 Saved` | 「已保存到本地历史」 | `render_understanding_finished` 完成状态行 | 已对齐（新增） |
| `bi8Au/Answer Sections` | 完成状态行 → 结论概述 → 横向业务步骤 → 数据读写表 → 并排「调用入口 / 尚未确认」 | `render_understanding_finished`、`render_flow_section`、`render_data_section`、`render_boundary_note` | 已对齐（沿用既有实现） |
| `bi8Au/Source Sidebar` | 40px 文件头、38px 路径条、36px 语义页签、27px 源码行、86px 复验说明区 | `render_understanding_source_panel`、`render_source_line` | 已对齐 |
| `ggnTD/S5 Semantic Notice` | 「Java 语义增强暂不可用」+ 原因 + 「前往语言支持」 | `understanding_java_capability`、`render_understanding_source_panel` 通知块 | 已对齐（新增，T5 只预留接口） |
| `ggnTD/S5 Footer` | 「基础源码模式 · 只读」+「定义 · 不可用」「引用 · 不可用」+「解释此处」 | `render_understanding_source_panel` 底部区 | 已对齐（新增能力态行与显式按钮） |
| `ggnTD/B1` | 未打开仓库/无索引时保留问题并给出单一阻断说明 | `render_understanding_no_repo`、`render_understanding_notice` | 已对齐（沿用既有实现） |

## 2. 程序化验证

- 新增单测 `src/tests/code_understanding_view.rs`（5 项，全部通过）：
  耗时徽标格式、时间线动作映射（不泄露工具名、失败原因入 detail）、索引状态徽标文案与配色、
  Java 能力仅作用于 `.java` 且恒为降级态并给出可读原因。
- `cargo test`：`665 passed / 0 failed`（lib）+ `308 passed / 0 failed`（bin）。
- `cargo build --release`：无错误、无警告。
  说明：全量重编译 lib 时会有一条既有告警 `src/ai/merge.rs:155: unexpected character `）``
  （rustdoc 注释字符告警，属既有代码，本轮未改动该文件，因此未处理）。
- 结构自检：来源侧栏在 280～480px 宽度范围内的页签与能力标签均为硬裁剪/固定高度，
  不依赖内容宽度；后台任务条在任意主模式下都挂在状态栏之上。
- 占位文案依赖问题框输入层透明（`repository_ui.rs` 的问题框分支背景改为透明，
  底色由提问容器提供），因此占位可画在输入层之下而不被不透明底色盖住。

## 3. 待人工验收（P0）

§9.5 要求以**同一窗口尺寸、同一页面状态**分别捕获 Pencil 与实际 GPUI 截图逐项核对。
本轮的自动化环境无法操作原生窗口（无可用桌面会话、且完成态需要真实仓库 + 索引 + AI 供应商
与一次真实分析），因此以下项目**尚未通过截图对照**，不得视为已验收：

| 待验收项 | 需要的最小条件 |
| --- | --- |
| S1 空态布局（上下文行、文案、问题框、示例行、恢复卡）间距与对齐 | 打开任一已索引仓库，进入代码理解页 |
| S3 生成态（问题条耗时、时间线密度、来源计数、动作条） | 配置可用 AI 供应商，提交一次问题 |
| B2 后台态（工作区任务条 + 状态栏名额） | 生成中切换主模式或点「后台运行」 |
| B3 失败态（错误卡、复制详情、规则条） | 使一次请求失败（断网或错误 Base URL） |
| S4 完成态（完成状态行、已保存标记、四大区块间距） | 一次成功分析 |
| S5 来源态（Java 通知、页签能力态、底部能力行） | 打开一个 `.java` 来源 |
| 深浅主题 / 窄窗（<720px 回答宽度）/ 长路径 / IME | 逐项手工切换 |

验收方式：按上表分别截图 Pencil 对应节点与 GPUI 实际窗口，逐项核对布局、间距、字体、颜色、
图标、状态和交互，把 P0/P1/P2 差异写入本文件；P0/P1 清零后才允许交接。

## 4. 实测反馈修正（用户上手后）

### 4.1 P0：已建好的索引仍提示「没有基础索引」

**根因**：仓库键不一致。索引库目录、`code_index_stats`、`code_index_preferences`、
`normalize_repo_path` 全部以 **canonicalize 后的小写路径**（Windows 为 `\\?\d:\...`）为键，
而 T5 页面此前用 `self.repo_path.display().to_string()` 原始字符串寻址。会话里保存的路径可能是
正斜杠（`D:/workspace/.../khaslana/`），于是同一仓库被算成两个哈希：

| 写法 | FNV-1a 键 | `code-index/<键>/index.db` |
| --- | --- | --- |
| `\\?\d:\workspace\codex-workspace\khaslana`（索引写入） | `31b69cef` | 存在 |
| `d:/workspace/codex-workspace/khaslana`（页面读取） | `45e8c20b` | 不存在（仅被 `create_dir_all` 建出空目录） |

**修复**：`understanding_project_key()` 改为 `crate::normalize_repo_path`，
并让所有派生点（索引状态判定、副标题统计、`cancel_understanding_task_for`、
`show_understanding_task` 的 tab 匹配、任务分离判定、历史目录）统一走该键。
新增回归测试 `understanding_repo_key_matches_the_code_index_key`：同一仓库的正/反斜杠写法
必须折叠到同一个键，且与设置页 `index_db_path` 一致。

顺带修正了 `code_index_task.repo_path == key` 的忙碌判定——此前同样是 canonical 与原始路径比较，
索引进行中不会显示「索引中」，且会误判为「未建立索引」。

### 4.2 空态右侧留白

空态内容节点此前带 `max_w(760)`，问题框被截到 760px 宽，右侧大块留白。
已去掉该上限并让内容 `w_full`：空态问题框铺满可用宽度；完成后来源侧栏打开时才按 360px 收起
（`understanding_source_width`，可拖拽 280～480px）。

### 4.3 示例问题 → 本地历史

「试试这些问题」整块移除（`EXAMPLE_QUESTIONS` 常量一并删除），该位置改为**本地历史**：
列出本机该仓库最近 5 条完成记录（问题摘要 + 完成徽标 + 时间/来源数），点击进入只读完成态，
标题行右侧「查看全部」打开页头的完整历史弹窗（最多 20 条）。无记录时显示一行说明占位。

### 4.4 P0：预算触顶 + 收尾 JSON 格式错误导致整轮分析作废

用户实测报错：`[BudgetExceeded] 工具结果累计体积上限已用尽 … 原因：[AnswerInvalid] … unknown variant type, expected one of observed, inferred, unknown`。

两处根因，都已修：

1. **收尾轮跳过了格式修复**。`agent.rs` 里守卫是 `if force_finish || repairs_used >= MAX_FORMAT_REPAIRS`，
   于是最需要修复的收尾轮（模型正被要求「立即输出 JSON」）反而没有任何补救机会，一次字段名写错
   就让整轮分析作废、连 `Partial` 都拿不到。改为：收尾轮同样允许一次修复，仅当**轮次上限**已触顶
   （`ToolBudget::has_repair_round_headroom`）时才不追加请求——轮次上限是硬约束，其余限额触顶时
   轮次通常仍有大量余量。
2. **证据状态字段名 `state` 被写成 `type`**。系统提示里嵌的是完整 JSON Schema，正文满是
   `"type": "string"` 这类关键字，模型按惯例把 `state` 写成了 `type`。取值域被枚举限死、语义无歧义，
   故 `AnalysisFinding::state` / `DataAccess::state` 用 `#[serde(alias = "type")]` 兼容接受；
   同时在输出协议里显式写明「证据状态字段名是 state，schema 里的 type 是 JSON Schema 自身的类型关键字」。

新增测试：`parse_accepts_type_as_the_evidence_state_field`（并确认取值域外的 `"probably"` 仍被拒）、
`protocol_names_the_evidence_state_field_explicitly`、
`forced_finish_still_gets_one_format_repair`（构造「累计体积触顶 → 收尾轮给不合法 JSON → 修复后返回
合法 `Partial`」的真实路径，断言修复请求带上了具体校验问题且指令是最后一条消息）。

### 4.5 提问框单行自适应

此前问题框可视高度写死 2 行。改为默认 **1 行**、随内容自增（逻辑行与上次测量出的自动换行行数取大，
钳在 `1..=QUESTION_INPUT_MAX_LINES`(6)），超过上限后框内滚动——与提交信息框的固定 5 行互不影响。

实现要点：
- `multiline_input_visible_lines` / `multiline_input_should_scroll` 增加「上次自动换行行数」入参，
  只剩纯数据入参（保持可单测）；
- `MultiLineInputElement::request_layout` 的最小行数按字段取（问题框 1 行，其余 `MULTILINE_MIN_LINES`），
  否则元素自身高度下限会把滚动范围撑歪；
- `TextFieldState::with_compact_multiline()` 把自动换行行数初值从 5 复位成 1（固定高度框仍用 5），
  避免首帧就撑到 5 行。

顺带修掉一处重复实现：占位文案此前是我在提问容器里叠的一层绝对定位文本（并把输入框背景改透明），
而框架本身已支持 `TextFieldState::placeholder` 且元素已经渲染它——现在统一用框架能力，
输入框背景恢复不透明，占位文案在 `repository_core` 初始化时取自同一常量。

### 4.6 执行过程最多 5 行，超出后自动滚动

生成态（与失败态）的时间线此前直接铺在外层滚动内容里，步骤越多把下方「正在生成回答」挤得越远。
现在时间线包在独立视口里：`max_h = 5 行 × 25px + 4 × 2px`，内部滚动，并用末位零绘制 canvas 的
prepaint 按「时间线行数」变化键钉底（`UnderstandingTimelineFollowState`，与 AI 思考弹窗同一套模式）——
键不变说明是用户自己滚动，不回弹抢视口。

### 4.7 左侧导航器仍显示「代码索引未就绪」

`chrome_view.rs` 的导航器索引卡是上一轮索引键修复的漏网处：它同样用
`self.repo_path.display().to_string()` 查 `code_index_stats` / `code_index_enabled_cache` /
`code_index_task.repo_path`，而这三处的键都是 `normalize_repo_path` 的 canonical 形式。
已统一改用 `crate::normalize_repo_path`。

另外补上切仓库时的懒加载：`inherit_main_mode` 继承到代码理解页时，同步调用
`ensure_understanding_index_stats` / `ensure_understanding_history_loaded`（与 `set_main_mode` 一致），
否则切换仓库后页头与导航器会停在空状态直到用户重新点一次模式入口。

## 5. 已知取舍

1. **B3 保留提问框**：设计 B3 面板只画了问题条、时间线与错误卡，未再画提问框；但 §9.1 要求
   「保留问题」且「编辑问题回 S1」，因此失败态在底部保留预填提问框（按钮为「提问」），
   问题条与规则条同时说明「问题已保留」。
2. **S4「已保存到本地历史」位置**：设计 S4 把该文案放在底部动作行；§9.5 明确禁止在回答区
   追加底部大区块，故并入顶部完成状态行（与「分析完成」「N 个来源 · N 次工具调用」同排）。
3. **来源侧栏页签文案**：设计 S5 在 504px 示意图里用「定义 · 不可用」长标签；侧栏最小 280px
   时五个长标签会横向溢出，故页签保持短标签并以 `MUTED_FOREGROUND` + tooltip 表达不可用，
   能力标签下沉到 86px 复验说明区的能力行。
4. **S1 本地历史取代示例问题**：§9.5 把示例问题列为 S1 内容之一，用户实测后要求改为本地历史，
   故以用户口径为准（设计稿的「试试这些问题」不再渲染）。
5. **后台任务条高度**：设计 B2 示意为 72px；实现取 56px 以支持多仓库多任务堆叠，
   标记、文案层级与动作保持一致。
6. **S5 语义动作**：T5 只预留接口。`understanding_java_capability` 依据 LSP 安装登记表区分
   「未安装」与「已安装未就绪」，但 `ready` 恒为 false；JLS-T3 接入语义服务后在此接口上继续。
7. **状态栏**：原内嵌「查看/取消」气泡已移到中央工作区的后台任务条，状态栏只留
   「代码理解运行中」与「AI 任务 n/3」，与 B2 的状态行一致。
