# Khaslana 前端 GPUI Kit 重构计划

状态：M1 已收口并给出 Go 结论；IME 与键盘交互已在第二轮可交互会话补验通过（Esc/焦点恢复待补），M2 依赖切换已完成，M3 壳层与首个设置页主体已落地，M4 普通输入/菜单/设置迁移已完成，M5 工作区与历史已收口（含页头/空状态统一与窄窗排布修正），M6 专业视图与工作流/AI 迁移已完成代码工作（五个批次），M7 清理与工程检查批次已完成（自绘输入通路整体删除、旧引用扫描零残留、工程检查四项全过、文档同步；真实前台键盘矩阵、旧 token 换色、性能与视觉实机验收、发布评估留待交互会话）。更新日期：2026-09-22。

基线：`dev_gpuikit` / `b4ec8914139ba768f7a6596e3f870173869eabfb`。组件调研与实拍证据见 [gpui-kit-research.md](gpui-kit-research.md)。

视觉方向已由用户提供的参考图确定为“轻盈悬浮工作台”，具体规则见 [视觉规范](gpui-kit-visual-spec.md)。该方向替换初版计划中的平面 Calm Technical 方案。

## 1. 目标与完成定义

让 Khaslana 成为视觉统一、层次清晰、中文易读的原生 Git 工作台，同时保留已有完整工作流。

最终应达到：

- 前端运行在 `gpui-kit` 提供的同一 GPUI 底层，删除 `yororen_ui` / `gpui-ce` 依赖与引用。
- 通用按钮、输入、选择、开关、菜单、弹窗、反馈由 Kit 组件及薄适配层提供；项目不再维护第二套同用途普通控件。
- 所有现有页面接入统一主题、尺寸、图标与状态规范；专业画布可使用 Kit 导出的 GPUI 自绘，但不保留旧库运行时。
- 打开/克隆/多仓库、暂存/提交、远端同步、历史、分支/标签/贮藏、合并/变基/冲突、工作流、AI、索引、设置及桌面行为保持等价。
- 完成视觉、功能、性能验收，`cargo build --release` 零错误零警告；“能编译”不代表“视觉达标”。

首板独立原生样板及接缝记录见 [原生样板验证报告](gpui-kit-native-spike.md)。本次仅同步计划与交互决策；后续以小批次可审查提交执行，不格式化无关文件，不夹带 Git、网络、索引或存储重构。

### “全部改成 Kit”的边界

建议理解为**全应用使用 Kit 底层和设计体系、通用组件全面替换**。Diff 行、提交泳道、追溯归属与冲突连接线没有可直接替换的通用成品，应继续作为 Khaslana 专业组件；它们同样使用 Kit 的 GPUI、主题和基础设施。

如果要求连这些领域绘制也完全禁止自定义，则当前调研没有发现足够的现成组件，需要另行收缩功能或扩大开发范围，不能承诺直接替换。

## 2. 已知约束与待决项

| 决策 | 本计划处理 | 影响阶段 |
| --- | --- | --- |
| 目标库 | Longbridge GPUI Kit；原型锁定 `=0.6.4`，实施前复核可用版本 | M1 |
| 键盘策略（已确认） | 2026-09-21 用户确认采用 Kit 标准交互，保留现有应用快捷键与业务语义；替代旧普通控件纯鼠标白名单 | M1、M2、全程 |
| 视觉方向 | 已选用户参考图：轻盈悬浮工作台；样板验证原生还原效果 | M0、M3 |
| 深浅色/强调色 | 保留跟随系统、浅色、深色和已有强调色偏好 | 全程 |
| 标题栏与设置 | 最新 Pencil 稿：60px 顶栏，仓库下拉仅显示仓库名，右侧命令中文；设置移至侧栏底部，紧凑窗口按钮靠右；已有命令保持可达 | M3 |
| Navigator 偏好 | 以当前源码的全局 visible 为准；跨仓库/模式共享；专业页保留窄条入口 | M3 |
| 自由 Dock / 内置编辑器 / JS 扩展 | 不纳入第一轮；固定分栏优先用 Resizable | 后续可选 |

当前 `docs/ui-design-system.md` 有“每仓库每模式分别保存导航偏好”的旧描述，而本分支代码与 AGENTS.md 指向全局共享值。这里仅记录差异，不改行为；M0 核对后在最终文档维护阶段修正文档。旧输入规则要求保留自绘实现，用户本次全面组件重构意味着需要评估替换，但不得在等价性验证前删除原实现。

### 已确认的 Kit 标准交互

2026-09-21 用户确认以下策略，实施时以此替代旧的普通控件纯鼠标／禁止键盘导航规则：

- 启用 Tab / Shift+Tab 焦点遍历、按钮 Enter / Space 激活、菜单与选择器方向键导航、Esc 关闭可取消浮层，以及关闭后的焦点恢复；保留清晰的键盘焦点指示。
- 保留应用级可配置快捷键、快捷键录制、动态工作流快捷键与现有启用条件；Ctrl+P 的检索、选择、确认打开追溯语义保持不变。
- 保留文本编辑、单行表单提交、多行 Enter 换行、变更列表 Shift/Ctrl 点选；中文 IME 组合期间 Enter 不得穿透触发表单或命令。
- 禁用、忙碌、危险操作确认与后台任务守卫对鼠标和键盘一致；同一次按键不得重复提交。Esc 不隐式取消不可取消任务或丢弃未保存内容。
- M1 验证输入、菜单、嵌套弹层与应用快捷键之间的事件优先级；不能把“采用标准交互”理解为允许组件覆盖已有业务语义。

该决策是实施目标，不表示当前主工程已经完成键盘行为迁移。

## 3. 视觉改造方向

用户已提供明确的视觉参考，不再生成三种方向重新选型。采用浅冷色环境底、柔和白色工作面板、大圆角、轻投影与少量弱轮廓，让组件呈现“漂浮在桌面上的轻量实体”感觉。完整参考图、token 候选和验收规则见 [视觉规范](gpui-kit-visual-spec.md)。原生实现仍需样板验证，不能把 Kit 默认外观视为最终效果。

### 3.1 保留工具型工作台，调整层级与空间

- 保留标题栏、单一上下文导航、文件/提交导航和主画布，不增加重复侧栏或首页仪表盘。
- 主画布优先分配宽度；导航、文件列设置合理初始宽度与最小值，记住用户拖拽。空列表不要因为双分组占据大量无信息区域。
- 导航、文件列表、diff 外壳和提交区使用柔和抬起的独立面板；留白代替大部分贯穿分割线。代码内部保持平整底色，输入框用轻微内凹感，按钮用细腻接触阴影体现厚度。
- 按最新设计，提交并推送为蓝色主动作，提交到当前分支为浅色次级动作，危险按钮独立语义。不要把所有命令都刷成主题色。
- “没有仓库”“工作区干净”“未选文件”“正在加载”“请求失败”分别设计文字与可执行入口；禁止共用一个空白占位。

### 3.2 Token 与密度候选

| 项目 | 现有 | 新样板验证目标 |
| --- | --- | --- |
| 正文 | 12px | 中文 13–14px 候选，先验证窄窗和高 DPI；代码正文单独测量 |
| 元信息 | 11px | 11–12px，关键状态不能只靠小号浅灰文字 |
| 标题 | 14 / 16px | 页面主标题验证 22–26px；面板标题保持紧凑，不层层放大 |
| 普通控件 | 28 / 32px | 工具栏 28–32px、表单 32–36px 候选，建立统一尺寸映射 |
| 行高 | 紧凑 28 / 常规 36px | 保留；历史/提交图谱继续专用 48px 双行 |
| 间距 | 4px 基线 | 保留 4 / 8 / 12 / 16 / 24 的节奏 |
| 圆角 | 6 / 8 / 10px | 控件 8–12px、工作面板 14–18px，避免逐行卡片化 |
| 深度 | 平面为主 | 工作面板轻投影、按钮接触阴影、浮层更高一级；低对比轮廓 |
| 主题 | 语义 token + 强调色 | 保留偏好，增加 Kit Theme 映射；Git 增删颜色独立于品牌色 |

这些值只是产品目标，不假定 Kit 默认 size 与像素一一对应。须以原生样板实际度量确认。用户本次要求覆盖旧的“面板平面化、阴影仅用于浮层、禁止装饰渐变”限制；允许极淡环境色过渡与面板层次。默认在正常原生窗口内实现，不预设系统透明或实时背景模糊；继续避免持续漂浮动画和 emoji 图标。

### 3.3 首批视觉样板

1. 工作区：至少包含暂存、未暂存、已选 diff、长路径、提交文本、禁用/忙碌状态。
2. 设置：主题或代理页；同时展示输入、选择、开关、说明、错误与保存动作。
3. 历史：双行提交、ref 标签、详情、文件列表与 diff。

取相同窗口尺寸和数据的旧/新截图对照，同时对照用户参考图检查悬浮层次、留白、圆角和控件厚度。保留 Khaslana 品牌和现有功能，不照搬图片中的 CodeFlow 名称、500 字限制或本分支未实现的功能。官方图库只说明控件能力，不作为最终整页验收目标。

## 4. 技术迁移结构

### 4.1 保持业务边界

GitService、类型、后台 TaskExecutor、UiEvent、仓库快照、AI 任务生命周期、Keyring 和 SQLite 保存策略继续由现有代码管理。组件只展示状态并发出动作，不在 UI 线程执行重 Git/IO 操作。

优先沿用 `impl RepositoryView` 的模块扩展方式；不同时引入全新的全局状态框架，也不把所有字段一次拆成大量 Entity。

### 4.2 基础层

建议按需要从 `src/ui/components.rs` 抽取薄适配模块，名称在实现时按实际职责调整：

| 模块 | 职责 |
| --- | --- |
| `ui/kit_theme.rs`（新） | 应用语义 token → Kit Theme；统一主题更新入口 |
| `ui/controls.rs`（新） | Button/Tooltip/Tag/Switch 等尺寸、业务 ID、禁用说明与交互策略 |
| `ui/fields.rs`（新） | FieldId 与 Input/Textarea Entity、订阅、取值/回填适配 |
| `ui/overlays.rs`（新） | Root 相关层、弹窗关闭策略、通知队列适配、焦点恢复 |
| `ui/components.rs`（保留入口） | 再导出及项目语义 helper；分批删去重复自绘实现 |
| `ui_helpers.rs`（收缩） | 专业画布和滚动兼容；通用实现通过验证后逐项删掉 |

这些是薄包装，不复制一套 Kit API。业务模块不要直接写零散颜色或绕过统一通知入口。

### 4.3 底层切换策略

1. M1 在隔离临时工程验证 Kit，与主工程的索引/关键依赖共同解析，主分支不先换包。
2. M2 用一个可构建提交统一 GPUI 类型、启动路径、资源和主题；同时移除 Yororen 的四处接入。不要留下“半个旧 GPUI、半个新 GPUI”。
3. 新运行时上可以暂留已适配的 div/canvas 自绘控件，随后分页面换 Kit 控件。
4. 使用 Kit re-export 与官方宏入口；若采用 crate alias 减少 import 改动，必须先验证 derive、actions! 与模块解析，不假定改个依赖别名就能通过。
5. 初始化保留 `khaslana mcp` 提前返回路径，避免无头 MCP 输出被 GUI 日志或初始化污染。

### 4.4 状态与资源迁移要点

- Input/Select/List 状态创建一次，订阅存储在拥有者上；对话框关闭或动态字段删除时清理；不可在 render 中重新创建。
- 按 repo/tab/字段身份隔离草稿；程序回填和用户编辑事件防循环；AI 完成一次回填提交信息的现有行为保留。
- ElementId 使用“页面 + 仓库/条目键 + 动作”，不以中文 label 单独作为多行按钮的 ID。
- 主题通过一个入口驱动 Kit 与 Git 专业语义色；主题切换仍触发语法高亮重算，不重新加载 Git。
- 默认 Kit assets 与品牌/自有图标合并并明确优先级，检查 Lucide 与 `assets/icons/` 同路径碰撞；保留 OAuth 品牌资源。
- 浮层只能有一个实际事件/显示宿主。保留业务 DialogState，视图适配到 Kit；避免 Kit 内部“已关闭”而业务状态仍“打开”。
- 通知沿用业务队列与动作语义，再由 Notification 展示；成功/失败只通知一次；状态栏继续承担进行中操作状态。

## 5. 分阶段实施

人日为熟悉 Rust/GPUI 的单人粗估，包含阶段内回归，不是承诺。M1 后重新估算。

| 阶段 | 预估 | 主要交付 | 通过门槛 |
| --- | --- | --- | --- |
| M0 基线与设计决策 | 1–2 人日 | 键盘策略、功能清单、旧版截图/性能基线、参考图样板范围 | 不存在影响实施的未决交互冲突 |
| M1 原生兼容验证 | 2–4 人日 | 独立 Kit 验证窗口和依赖兼容报告 | 输入/窗口/弹层/虚拟列表可用；明确 Go/No-Go |
| M2 统一底层与基础层 | 3–5 人日 | Kit runtime、Root、主题、资源、基础适配 | 全应用同一 GPUI；主要旧页面可运行 |
| M3 壳层与首个设置页 | 3–4 人日 | 新标题栏/导航、主题与代理设置样板 | 新旧截图对照通过；桌面行为与偏好保留 |
| M4 普通表单与弹层 | 4–6 人日 | 普通输入/Select/菜单/设置迁移 | FieldId、校验、secret、焦点/键盘交互、保存语义无回归 |
| M5 工作区与历史 | 3–5 人日 | 主使用路径视觉升级 | 部分暂存、提交、历史、长 diff 的操作与性能通过 |
| M6 专业视图与工作流/AI | 5–8 人日 | 全部剩余页面完成 Kit 适配 | 专业能力与后台任务生命周期无退化 |
| M7 清理与发布验收 | 3–5 人日 | 删除旧实现、文档、完整验收记录 | 无遗留依赖；release 零错误零警告 |

合计约 **24–39 人日**；若标准交互与现有业务需要较多适配，或 GPUI 平台 API 存在较多变化，另留约 20%–30% 余量。页面数量与涉及行数只用于范围估计，不据此线性计算工期。

### M0：基线与范围冻结

- 在临时 Git fixture 中准备普通变更、冲突、长路径、中文/GBK、二进制、Office、很多分支与提交。
- 记录工作区、历史、设置、浏览/比较、贮藏、追溯、提交图谱、冲突、工作流、AI 的深浅色截图。
- 记录 release 运行的启动到可操作时间、空闲/大型 diff 内存、滚动帧时间和包体积。
- 按已确认的 Kit 标准交互记录验收用例；核对现有设计文档与代码差异；按已选“轻盈悬浮工作台”参考冻结第一轮 token，原生样板校准阴影、圆角和阅读密度。
- 本轮调研只提供工作区取样，不能替代上述全量基线。

### M1：最高优先级的原生验证

独立窗口至少包含 Button、Input、Textarea、Select、Switch、Dialog、Notification、双向滚动的 10,000 行虚拟列表、自绘 canvas 和自定义标题栏。数据为本地模拟，不连接真实凭据或执行 Git 写操作。

验证清单：

1. `gpui-kit = "=0.6.4"` 默认 component/assets 与本项目关键依赖可解析；检查 `cargo tree -d`，证明没有两套 GPUI 类型；暂不启用 Kit tree-sitter features。
2. Windows MSVC + 当前静态 CRT 配置能够 debug/release 编译并启动；记录实际 Rust/toolchain 要求。
3. 中文拼音候选、组合期间 Enter、中文标点、emoji/代理对、选择替换、剪贴板、secret、长多行和撤销行为。
4. 对话框内 Select、边缘 Popover、遮罩点击、阻塞操作、输入焦点恢复；验证选定键盘策略。
5. 100%/125%/150%/200% DPI、窗口拖动/缩放、最小化/最大化、托盘关闭/恢复、系统主题切换。
6. 2 万行专业 diff 样例、很长单行、横向滚动、拖拽分栏；不预建所有行元素。
7. 验证 Root 宿主后原有弱引用、窗口关闭回调、应用快捷键与 MCP 无头模式的接入方案。

**No-Go 条件：**依赖不能统一、IME 数据损坏、原生窗口/托盘不可用、标准交互与既有快捷键或业务守卫存在未解决冲突。出现时停止主工程底层切换，记录最小复现与替代版本/API，不把失败留给后续页面阶段。

### M2：统一底层与主题

涉及：`Cargo.toml`/`Cargo.lock`、35 个涉及 GPUI 的实际文件、`main.rs::main`、`assets.rs`、`theme_view.rs`、`remote_branch_operation.rs`、`ui/`。

- 根据 M1 结果统一导入、宏与平台 API；Root 包住已有 RepositoryView（**本轮 M3 已补**，见 §8）。
- 完成 Kit 初始化、中文 locale、assets 合并、浅深主题与强调色映射。
- 首批 Button/Tooltip/Tag/Progress 适配保留现有业务 helper 的启用守卫和反馈。
- 将两处 Yororen Select 替换；连同主题、资源、图标/动画接入一次去掉旧库。
- 列出剩余自绘通用组件迁移清单；避免以“旧依赖已删除”提前宣布重构完成。

### M3：壳层与首个设置页面

涉及：`chrome_view.rs`、`sidebar_view.rs`、`main.rs::render_settings_center_overlay`、`theme_view.rs`、`proxy_view.rs`。

- 做出可运行的新壳层，使用统一控件高度、图标与导航行；工作面板增加一致的轻投影和留白，分栏拖拽区默认融入间隙、hover 时显示指示。
- 顶栏采用最新 Pencil 稿的 60px 高度；最小窗口 860×520，适配 1120/1440 宽度并保持命令可达。窗口按钮 32×32、间距 2px、右边距 12px；DPI 均按逻辑像素。
- 搜索框左侧为仅显示仓库名的仓库下拉；右侧使用刷新/获取/拉取/推送等中文文案。设置位于侧栏底部，移除本地仓库提示块，保留展开/收起入口。已有贮藏、子模块等操作不能因静态稿省略而被删除。
- Navigator 全局偏好、48px 窄条入口、窄窗覆盖层行为保留。
- 首先迁移主题和代理设置，以较小范围验证输入/选择/开关/说明/测试/保存。
- 固定分栏优先用 Resizable；原布局偏好经适配继续有效，不随组件重建跳回默认值。
- 原生对照通过后再扩散新视觉，避免每个页面独立定一套风格。

### M4：普通输入、菜单与设置

涉及：`text_input.rs`、`main.rs` 中 FieldId/field()/field_mut()/表单/菜单/弹窗方法、`remote_branch_operation.rs`、`code_palette_view.rs`、各设置 view。

- 先建立字段适配与回归用例，再按页面迁移。普通单行 Input、多行 Textarea、secret 字段分别处理。
- 覆盖克隆、分支/标签/远端、凭据/OAuth/SSH、pull/push/upstream、reset/revert、合并/变基、贮藏、子模块及错误确认。
- 覆盖九个设置分类：凭据、代理、AI、外部合并、代码索引、主题、更新、快捷键、关于。
- 代码索引多仓库卡保留稳定 ID、显式 repo_key、单任务守卫和删除确认；不用通用 Settings 自动保存替代业务逻辑。
- Ctrl+P 继续按现有代际搜索、预览和打开追溯；快捷键录制与普通控件事件不冲突。
- 普通字段全部迁完并验证后，才删除其自绘 Element/IME 路径；冲突草稿编辑能力另在 M6 收尾。

### M5：工作区、历史与共享 diff

涉及：`worktree_view.rs`、`history_view.rs`、`main.rs::render_virtual_diff/render_diff_row`、`diff_view.rs`、`ui_helpers.rs`。

- 统一文件行、状态标签、搜索/过滤、空状态和页头；文件列表占满左列，提交区位于右侧 Diff 下方。无暂存时只显示未暂存列表，有暂存时显示未暂存/已暂存两个列表。
- 提交到分支与提交并推送并列，后者为蓝色主按钮；禁用条件以真实业务守卫为准，不能将无暂存示例的禁用态套用于所有 amend 场景。
- 把 main.rs 中共享 diff 渲染抽到专用模块时保持数据模型不变；避免在视觉迁移中改 patch 算法。
- 保留双向部分暂存、行/块选择、语法高亮、编码选择、全文/紧凑切换、二进制占位、Office 文本预览和大文件守卫。
- 保留横向滚动宽度测量、滚动位置、虚拟 range 渲染；改变字号必须同步调整测量与命中区域。
- 历史列表保留 48px 双行、引用折叠、详情拖拽、文件历史过滤；提交框保留 amend 与 AI 一次回填。

### M6：专业画布、工作流与 AI

| 范围 / 文件 | 改造内容 | 不可丢失的能力 |
| --- | --- | --- |
| `browse_view.rs` / `browse_compare_view.rs` | 文件树、比较列表、标题与筛选控件 | 三点比较、重命名展示、目录展开、编码与异步守卫 |
| `stash_view.rs` / `submodule_view.rs` | 列表、状态、表单与弹窗 | pop/drop 确认、子模块同步策略、凭据/代理复用 |
| `blame_view.rs` | 归属栏、代码区和控制条 | 未提交行、行号/归属对齐、编码、长行滚动 |
| `commit_graph_view.rs` | 工具栏、选择菜单和详情 | 泳道拓扑、高亮/淡化、过滤、48px 行、往返状态 |
| `conflicts/mod.rs` | 操作区、三栏容器、只读结果区（M6 决策：草稿编辑不再作为产品路径） | ours/theirs、连接线、同步滚动、按块接受、完成/中止、AI 合并建议回填 |
| `workflow_view.rs` / `workflow_editor.rs` | 模板导航、向导、步骤卡、动态字段、控制台 | JSON5 注释确认、变量校验、字段保值、运行顺序、后台日志 |
| `ai_view.rs` / `markdown_view.rs` | 思考弹窗、时间线、Markdown、历史 | 固定 420px 思考窗、滚动跟随、取消/后台分离、并发上限、记录落盘 |

冲突草稿若尝试 Kit Editor，应独立验证选区、修改块位置、滚动和高亮对齐；第一轮不接入无关 LSP/全语言包。若保留定制草稿画布，须记录它为必要领域组件，不能把普通表单输入也永久留在旧实现中。

Markdown 替换必须验证流式半截输入、复制最终原文、历史重放与长文本性能。若 Kit 默认带链接或远程图片行为，应显式保持现有产品策略，避免无意增加网络请求。

### M7：清理与验收

- 全量扫描生产代码及依赖：无 Yororen/gpui-ce 引用，无双运行时，无孤立旧初始化。
- 删除迁移完的旧控件与适配过渡代码；必要自绘专业组件列明原因、维护边界和测试。
- 清理 main.rs 已迁走的 UI 方法，不做无关格式化或全面业务拆分。
- 更新 README、AGENTS.md 与 `ui-design-system.md` 的真实技术栈、键盘决策、组件规范、截图和测试说明。
- 执行下节验收，保存报告；完成 release 构建后按原发布流程评估便携包/安装器，不在此计划中自动发版。

## 6. 验收矩阵

### 6.1 功能

| 场景 | 必验结果 |
| --- | --- |
| 多仓库切换、重启恢复 | 仓库/分支/草稿/滚动/布局偏好正确，后台结果不串 tab |
| 暂存、取消暂存、部分暂存 | 单/多选与按块/按行两方向正确，刷新后选择与 diff 一致 |
| 提交、amend、提交并推送 | 校验、忙碌、凭据、错误、成功反馈正确且不重复 |
| 分支、远端、标签、stash、reset/revert/cherry-pick | 完整操作可达，危险确认仍有效 |
| fetch/pull/push、merge/rebase | 任务不阻塞 UI；冲突后的继续、完成、跳过/中止按已有能力工作 |
| diff / browse / history / blame / graph | 编码、二进制/Office、大文件、行号、分页、过滤、横向滚动正确 |
| 冲突工作台 | 三栏滚动/连接线一致，结果区只读 + 按块接受/AI 回填（M6 决策），AI 不用截断结果覆盖草稿 |
| 工作流 | 编辑/变量/校验/执行/取消/后台日志与原版等价 |
| AI | 流式、后台分离、取消、代际、并发限制、历史持久化不受换 UI 影响 |
| 设置/索引/MCP | 九类设置保存语义正确；索引开关/任务正确；MCP stdout 保持协议输出 |
| 桌面 | 启动、窗口控制、系统主题、DPI、多显示器、托盘、关闭策略正常 |

### 6.2 视觉与交互

- 860×520 最小窗、1280×820 默认窗、1440×900、1920×1080，深浅主题各验；100/125/150/200% DPI 检查主要页面。
- 按钮文案、中文基线、图标、圆角、轻投影和禁用状态统一；长路径/长分支名不覆盖主操作。普通面板避免贯穿分割线、强矩形边框及逐行投影，代码内容保持平整清晰。
- 菜单不越界；输入候选与弹窗焦点正常；滚动不穿透遮罩；嵌套浮层关闭顺序正确。
- 专业代码颜色与 diff 背景可读，状态有文字/图标辅助，不只依赖红绿。
- 常规正文对比度目标 4.5:1，控件边界/关键图形目标 3:1；实际测量后记录，不仅凭截图认定合规。
- 对照已选新版视觉样板与实现截图逐屏验收；控件迁移不作为视觉一致性的替代证明。

### 6.3 性能

同一机器、同一 release profile、同一 fixture，至少 3 次取中位数。以下为初始预算，M0 记录基线后确认：

- 10,000 行列表和 20,000 行 diff 滚动：60Hz 目标下 p95 帧时间 ≤16.7ms；若旧版已超标，新版不得退化并另记问题。
- 启动到可操作时间较基线增加不超过 15%；峰值内存增加不超过 20%；包体积记录变化与依赖来源，不凭官网帧率宣传承诺结果。
- 虚拟列表只构建可见元素；长行宽度与语法计算不能每帧扫描全文件；后台 Git/AI 仍走任务池。
- AI 流式与大列表并行时可继续滚动和切换；手动向上滚动不会被钉底逻辑持续抢回。

### 6.4 工程检查

按阶段先运行受影响模块测试，底层切换和最终交付运行完整回归：

```powershell
cargo check --all-targets
cargo test --lib
cargo test --bin khaslana
cargo build --release
```

对实际新增二进制/feature 补充对应命令；GPU/窗口类测试以 Windows 原生验证为准。遵循 AGENTS.md：实施完成时 release 必须零错误零警告。验证报告写实际执行结果、环境与未通过项，不把计划命令写成“已通过”。

## 7. 回退与提交安排

- M1 保持隔离，可直接放弃实验而不影响当前产品。
- M2 是底层切换边界，保持完整可构建提交；之后逐页迁移，不在同一元素树保留双 GPUI 作为运行期回退。
- 每个阶段记录基线 commit、变更范围、截图和测试结果。共享分支需要回退时使用可审查 revert，不使用 reset --hard 清工作区。
- 重构不修改凭据格式、仓库数据或持久化 schema；如后续确需变更布局格式，单独设计兼容读写后才纳入。
- 已经迁到新 runtime 的页面出问题时，回退该页组件实现到同 runtime 的上一版；底层问题则回退 M2 及依赖它的后续提交。

## 8. 当前进度与下一批工作

截至 2026-09-21（本轮更新）：

**M1 已收口，结论 Go（有条件）。** 样板扩容为可脚本化采证的验证台（10 000 行虚拟列表、
20 000 行差异列表、输入与键盘事件日志、深浅主题、窗口控制），实测结果与 Go/No-Go 判据见
[原生样板验证报告](gpui-kit-native-spike.md) §5。三项关键结论：

1. **虚拟化与性能达标**：20 000 行差异与 10 000 行列表都只构建 39 个行元素，大跨度滚动的
   端到端开销同量级，没有随行数增长的成本。
2. **放行前提（第二轮已补验）**：中文 IME 与键盘交互已在真实输入通路下实测——IME 组合与
   候选提交、组合期间 Enter 不穿透（0 次提交事件）、Tab 焦点迁移、Enter/Ctrl+Enter 语义、
   剪贴板与撤销全部通过；方法与证据见验证报告 §5.6。Esc 关闭浮层与焦点恢复因会话被锁屏
   中断，留待 M3 期间在与真实控件同屏的会话里再补一次。
3. **M2 前置已验证**：`gpui = { package = "gpui-pre" }` 别名让现有 `use gpui::…` 与
   `gpui::actions!` 原样可用，别名与 `gpui-kit` 指向同一实例，类型直接互传。

**M2 依赖切换已完成**（本轮）：主工程改用 `gpui-pre =0.3.5` + `gpui-kit =0.6.4`，
`yororen_ui` 与 `gpui-ce` 已从依赖与源码中完全移除；新增 `src/ui/kit_theme.rs` 做语义 token →
Kit 主题的单向映射；窗口初始化改用 `gpui_kit::application()` + `gpui_kit::init`。验收结果：
`cargo check --all-targets` 零错误零警告，829 个测试（lib 526 + bin 303）全部通过，
`cargo tree -d` 无第二套 GPUI。实际 API 差异清单见验证报告 §5.5。

后续顺序（沿用原计划，按依赖调整）：

1. ~~**补验 M1 悬空项**：在可交互会话里验证中文 IME（组合期间 Enter 不穿透）、剪贴板/撤销、
   Tab 遍历与 Enter/Space 激活、嵌套弹层与 Esc；顺带复测 release 帧时间绝对值。~~
   **已完成（部分）**：IME、剪贴板/撤销、Tab 遍历、Enter 语义、帧时间复测均通过（报告 §5.6）；
   Esc 关闭浮层/焦点恢复因会话锁屏未测完，与 M3 设置页键盘验收合并执行。
2. ~~**M2 遗留缺口（M3 开工前补）**~~ **已补（本轮）**：主工程窗口根视图已接入 Kit `Root`
   （`cx.new(|cx| gpui_kit::component::Root::new(view, window, cx).bordered(false))`，
   M4 的 Kit 弹层/Notification 由此可用）；中文 locale 与推送弹窗下拉残留此前已修。
   主题映射改为随组件补齐：本轮新增 `WB_*` 悬浮工作台语义层（环境底/顶栏/导航/面板/输入面/
   悬停/阴影，深浅两套）并接进 Kit 主题的 base/sidebar/title_bar/status_bar/list 系列。
3. **M3 壳层与首个设置页（本轮落地主体）**：
   - 窗口两项决策见上（圆角采用样板路线、不自绘投影；最大化 ↔ 还原走 `WindowControlArea::Max`）。
     **实测**：向自绘按钮位置投递 `WM_NCLBUTTONDOWN/UP(HTMAXBUTTON)`（与真实点击同一条
     gpui 代码路径）→ 最大化（填满屏幕 1920 × 1080，圆角归零）；再次投递 → 还原 1296 × 828；
     最大化态图标切到还原态。窗口样式实测 `caption=False thickframe=False`，
     客户区与窗口矩形等大（旧的 16 × 8 边框偏移已消除），最小窗精确 860 × 520。
   - 窗口形态：透明背景 + 外壳自绘 24px 圆角（`RADIUS_WINDOW`）+ `chrome_view::apply_window_chrome`
     去边框并触发 gpui 重算偏移（只去 caption 的方案已实测否决：客户区仍内缩 14 × 7）；
     根背景、顶栏上角、`dialog_overlay()`、操作遮罩四处同步圆角
     （gpui 的 `overflow_hidden` 只裁矩形），状态栏改透明底色让根圆角透出来。
     **Kit `Root` 的底色必须覆盖成透明**（`.bg(rgba(0x00000000))`）：`Root` 默认铺
     `theme.tokens.background`，漏掉这一步圆角外一圈就是实色、屏幕上“看不到圆角”
     （首次落地即踩，实测覆盖前四角 `#FFFFFF`、覆盖后该处无任何绘制；
     `GWL_EXSTYLE` 的 `WS_EX_NOREDIRECTIONBITMAP` 为真证明走 DWM 直合成、透明有效）。
     去边框的副作用是左右下三边的系统缩放带一起消失（gpui 只补了顶边），
     `render_window_resize_bands` 补 6px 透明带并在按下时发 `WM_NCLBUTTONDOWN` + 边界命中码
     把缩放交回系统的模态循环；**真机鼠标拖拽仍待你实测一次**（本会话无前台输入通路）。
   - 顶栏 60px：品牌（窄档只留标志）、仅仓库名的仓库下拉（白底薄实体 + 文件夹图标）、
     全局搜索入口（点击开 Ctrl+P 面板，面板仍持有输入状态）、刷新/获取/拉取/推送中文命令
     （窄档退化为图标 + tooltip）、贮藏/子模块恒为图标按钮、32 × 32 窗口按钮（间距 2、右边距 12）。
   - 导航面板：展开态 = 模式按钮 + 分组列表 + 底部「设置 / 收起」；收起态 = 48px 窄条
     （展开箭头 + 模式图标 + 设置）；移除了「上下文导航」标题行（画板无此行）。
   - 内容区 16px 留白，导航与页面各成悬浮面板；分栏拖拽区默认无可见分割线、悬停/拖拽才显示指示。
   - 设置中心壳层 + 主题页 + 代理页改用 `settings_card` 样板（浅凹分段选择器底色、色块行）。
     **代理/主题页的文本输入仍走项目自己的 `FieldId` 文本框**：Kit Input 的字段适配器
     （`ui/fields.rs`：Entity、订阅、取值/回填）是 M4 的首批交付，M3 不做半套适配。
   - 验证：`cargo check --all-targets` 零错误零警告；832 个测试（lib 526 + bin 306）全部通过；
     实机截图见 [m3 证据目录](assets/gpui-kit-research/m3/)（浅色/深色圆角窗、860 × 520 窄档、
     最大化方形角、设置页浅/深、Ctrl+P 输入）。
     过程中修掉两个新引入的渲染缺陷：8 位带 alpha 的 token 误经 `rgb()`（gpui 按 (G, B, A)
     解析，白色面板渲染成奶油色）与 `rgb(0x00000000)` 当作透明（实为不透明黑）；
     两个单测守住「表面色不带 alpha / 阴影色必须带 alpha」。
   - 未完成：Esc 关闭浮层与关闭后焦点恢复仍缺真实前台会话的补验（本轮采证会话无前台键盘通路，
     与 M1 的 §5.2 限制相同）；表格/瀑布式重排不在本轮（工作区的 Diff 下方提交区与单双列表
     切换仍属 M5）。
4. **M4 代码迁移已完成**，随后按 M5 → M6 → M7 推进（M5 第一批量见上条）。
   - `src/ui/fields.rs` 保留 `TextFieldState` 业务真值，并以 Kit `InputState` / `TextareaState`
     承担渲染与编辑；`ensure_kit_fields` 自动覆盖 `DEDICATED_FIELDS` 中除冲突草稿外的全部
     静态字段。工作流动态字段与冲突草稿按计划留到 M6。
   - 用户编辑经 `InputEvent::Change` 写回真值，程序回填下一帧推给 Kit；`focused_field` 在
     Kit 字段聚焦时让位，避免双写。多行普通 Enter 只换行；Ctrl/Cmd+Enter 由 Input 上的
     `TextSubmit` 覆盖绑定在 Kit 插入换行前提交。
   - `register_all_key_bindings` 刷新时保留 Kit 组件键位，仅剔除 Khaslana 自身 action；项目级
     通用按钮改用 Kit 基础按钮，具备 Tab 与 Enter/Space 语义。
   - 远端操作与标签推送的两处简单选择器改用 Kit `DropdownMenu`，具备方向键、Esc、外部点击
     关闭、滚动与触发器焦点恢复；根层补充自绘可取消浮层的 Esc 关闭和稳定焦点恢复。
   - 单测覆盖静态字段迁移边界、多行 PressEnter 不提交，以及动态刷新快捷键不误删 Kit action。
     真实前台的完整键盘/焦点矩阵仍在 M7 验收；注入 WM_KEYDOWN 时使用 `PostMessage`。
5. **M5 已开工（本轮，第一批）**：工作区与历史接入悬浮工作台外壳，并把共享 diff 渲染归位。
   - **单双列表切换（计划 §5 的行为要求，原实现一直是恒双列表）**：`change_sections_layout`
     新增 `show_staged`，无暂存内容时不渲染「已暂存变更」分区，未暂存列表独占左列剩余高度；
     暂存区加载期间仍先渲染占位避免布局跳动，加载完仍为空则整段收起。新增 3 个单测
     （`staged_section_is_hidden_when_index_is_clean`、
     `staged_section_appears_while_loading_then_hides_when_still_empty`、
     `clean_worktree_keeps_both_sections_compact`）守住这条策略。
   - **视觉分层**：变更分区标题行与差异区标题行从「`CARD` 底 + `border_b_1` 贯穿分割线」
     改为分组底色 `WB_SECTION_HEADER`（新增 token），分割线换成留白；提交区从
     `border_t_1 + CARD` 改为独立抬起的提交条（`WB_COMMIT_BAR` + `RADIUS_MD` + 外边距 +
     `control_shadow()`）；diff 正文区改用 `WB_DIFF_SURFACE`，保持平整底色、不随面板悬浮感
     加投影/圆角；历史页的提交导航列与提交文件列改用分组底色，容器不再自铺 `CARD`。
     三个新 token 都进 `workbench_surface_tokens_stay_opaque_for_rgb_consumption` 单测
     （表面色不得带 alpha——`rgb()` 会把 8 位值按 (G,B,A) 解析）。
   - **共享 diff 渲染归位**：`render_encoding_dropdown` / `render_virtual_diff` /
     `syntax_spans_for_diff` / `render_diff_row` / `diff_section_header` / `encoding_button`
     从 `repository_ui.rs` 搬到 `diff_view.rs`（475 行，**逐字节原样移动**，已用
     `Compare-Object` 核对两侧行集合完全一致，除模块文档/导入与两处有意改样式的行外零差异）。
     `render_column_splitter` 属通用分栏而非差异渲染，已留在 `repository_ui.rs`。
     数据模型、虚拟列表、宽度测量、语法高亮槽位、部分暂存交互全部未改。
   - 验证：`cargo check --all-targets` 零错误零警告；841 个测试（lib 526 + bin 315）全部通过。
   - **未完成（M5 剩余）**：~~`worktree_view.rs` / `history_view.rs` 的页头与空状态统一、
     提交区主次按钮在窄窗下的排布复验、长 diff 性能与视觉实机对照截图~~ → **已由
     M5 收口批次完成**（见下条第 6 项）。
6. **M5 收口批次（本轮）**：页头/空状态统一、窄窗排布修正、守卫语义纯函数化。
   - **「没有仓库」页面级空态**：工作区与历史页在 `repo_path.is_none()` 时不再渲染
     空变更列表 + 禁用提交条（或孤立占位行），改为整页 `EmptyState`：说明文字 +
     「打开仓库… / 克隆仓库…」两个动作按钮（复用 `browse_open` / `open_clone_dialog`
     既有入口，不新增业务路径）。视觉规范 §2「无仓库与干净工作区分别设计文字与
     可执行入口」由此落实。
   - **干净工作区专属文案**：新增纯函数 `unstaged_empty_text(loading, peer_has_content)`，
     三态区分——加载中 / 未暂存空但暂存区有货（「未暂存变更已全部暂存」，流程没结束）
     / 两边都空（「工作区干净，没有待提交的改动」）。此前只有一句「暂无未暂存变更」。
   - **历史页列表基线对齐**：两个列表 wrapper 的 `p_2()` 改为 `py_2()`（水平留白交给
     列表行自己），提交文件行从 `px_2()` 改为 `pl(SPACE_3) + pr_2`，使行文本左缘与
     `panel_section_header` 默认 `padding_x(SPACE_3)` 落在同一条基线（此前提交行错位
     8px、文件行错位 4px）；「加载更多」行补 `pl(SPACE_3)` 保持同一基线。工作区变更
     列表的空态占位新增 `panel_empty_row_aligned` 变体，占位文字与行内 `px(16)`
     及标题 `padding_x(SPACE_4)` 三者对齐。
   - **提交条窄窗排布修正**：底部行原为 `justify_between + flex_wrap` + 无开关时塞
     `div()` 空槽占位——`justify_between` 在换行后会把零宽空槽当作两端之一，
     实际动作组贴左而非靠右。改为「开关槽（`flex_none`）+ 动作组 `ml_auto`」：
     单行时开关居左、动作靠右；窄窗换行时动作组整体落第二行且仍靠右。
     动作组自身的 `flex_wrap + justify_end` 保留（按钮组内部换行时逐行靠右）。
   - **守卫语义纯函数化**（计划 §5「禁用条件以真实业务守卫为准」）：新增
     `commit_action_enabled` / `commit_and_push_enabled` 两个 `const fn`，把
     「普通提交不因空暂存禁用（amend 允许只改信息）」「推送要求远端存在」
     「合并中透传 `merge_can_finish`」三条规则从 render 内联表达式提升为可测签名；
     render 改为调用纯函数，行为等价。
   - 新增 6 个单测（`unstaged_empty_text_separates_clean_worktree_from_one_side_empty`、
     `commit_action_enabled_ignores_staging_state`、`commit_and_push_requires_remote_and_hides_during_merge`、
     `empty_state_builder_keeps_detail_and_actions_optional`、`empty_state_shortcut_stays_non_filling_without_actions`
     及本批守卫测试组）。测试编写中发现并修正了守卫函数对 busy 透传的语义混淆
     （merge 分支的 busy 由 `merge_can_finish` 统一把关，纯函数不再重复判定）。
   - 验证：`cargo check --all-targets` 零错误零警告；`cargo test --lib` 526 通过；
     `cargo test --bin khaslana` 325 通过（+6）；`cargo build --release` 成功。
   - **仍未完成**：长 diff 性能与视觉实机对照截图（860×520 窄窗提交条换行、
     M5 各页深浅主题）需要真实前台会话，留待 M7 验收矩阵执行。
7. **M6 专业视图与工作流/AI 迁移（本轮，五个批次）**：
   - **批次 1 · 工作流动态字段迁 Kit（M4 遗留项收口）**：`WorkflowInput(usize)` 与
     `WorkflowEditor(WorkflowEditorFieldId)` 两类动态字段此前被 `kit_field_migrated`
     排除、走自绘渲染，本批迁入 Kit。动态字段不进 `DEDICATED_FIELDS`，宿主生命周期
     由 `ensure_kit_fields` 末段新增的 `reconcile_dynamic_kit_fields` 维护：
     每帧先按 `active_dynamic_field_ids` 枚举当前应存活的动态字段集合
     （运行配置 inputs + 编辑器已创建的步骤槽/变量行文本框），对宿主列表
     「多退少补」——退场字段整条移除（drop 即退订），新字段补建宿主。
     核心安全点是**严格寻址**：新增 `try_field` / `try_field_mut`（无越界兜底），
     Kit 的 `Change` / `PressEnter` 回调改走严格版——动态字段被重建/删除后，
     残留事件静默丢弃，不会经 `field()` 的 `branch_name` 兜底把孤儿输入写进
     无关表单。工作流输入即查预览、编辑器 `sync_from_fields` 的通知链路
     （`notify_text_field_changed`）零改动。
   - **批次 1 · 冲突结果区死路径删除**：`conflict_result_pane_uses_editor()` 恒
     `false` 使自绘 `conflict_editor_input`（约 113 行）、
     `sync_conflict_editor_into_state` 写回逻辑、`highlight_selected_conflict_block`
     的编辑器高亮分支、`multiline_input_should_scroll` / `multiline_scroll_handle_id` /
     `is_multiline_field` 的 `ConflictEditor` 分支全部不可达。本批删除渲染死路径
     与死开关函数，`conflict_editor` 字段保留为焦点锚点（工作台两处点击仍
     `window.focus(&this.conflict_editor.focus, cx)` 把键盘交给工作台），
     `sync_conflict_editor_into_state` 留空操作并注释「恢复可编辑结果区时的接回点」。
     计划 §M6「若保留定制草稿画布，须记录它为必要领域组件」的决策落为：
     结果区 = 只读文档视图 + 按块接受/AI 回填，草稿编辑不再作为产品路径。
   - **批次 2 · submodule_view 旧 token 迁移**：`submodule_status_pill` 的 5 处
     `COLOR_SUCCESS/WARNING/ERROR(+_FOREGROUND)` 实心状态色改为 `FEEDBACK_*`
     BG/BORDER/TEXT 配对 token（与冲突工作台徽章同一套语义）；全文件旧一代
     token（`BORDER/CARD/FOREGROUND/MUTED_FOREGROUND/SECONDARY`）换为
     `BORDER_MUTED/SURFACE_BASE/CONTENT_*/STATE_HOVER`，字号 10/11/12/14
     字面值换 `TYPE_META/TYPE_BODY/TYPE_TITLE`。列表行为普通 div 滚动
     （非 uniform_list），`truncate()` 保留不触测量陷阱。
   - **批次 3 · browse/blame 视觉收口**：分支浏览与分支比较的「文件树」
     「差异文件 · N」分组标题从自绘 `border_b_1` 行迁到 `panel_section_header`
     （`padding_x(SPACE_4)` 保持与列内边距一致）；分支比较面板去掉自绘
     `border_r_1`（右侧分隔线由列分割条统一绘制，与分支浏览同一约定）。
     blame 注释栏四处行内 `truncate()`（哈希/作者/摘要列）改为
     `overflow_hidden + whitespace_nowrap` 硬裁剪——这些文本在 `uniform_list`
     行内，坍缩测量会把省略号固化成「永远只显示 …」； blame 头部自绘关闭
     按钮改用 `self.button`（编码按钮保留自绘：它切换菜单而非执行业务动作）。
     清理 `ROW_HEIGHT_REGULAR` 常量（分组标题迁移后生产代码无引用，
     history 测试改与 `ROW_HEIGHT_COMPACT` 对比，theme 测试改字面断言）。
   - **批次 4 · commit_graph_view 工具行与详情卡**：工具行分段控件（scope
     当前分支/所有分支）、分支高亮触发器、泳道圆点外圈、下拉菜单项的旧 token
     批量换为语义 token（选中态底从 `ACCENT` 改为 `WB_SECTION_HEADER` 分组带，
     与 M5 变更分区同一语义）；详情卡空态从自绘居中文字改为 `panel_empty_row`
     （列表/区块级占位）。复制 SHA / 复制信息按钮经评估保留自绘：项目级
     `button` 的回调是 `Fn` 签名，收不下需要 `cx.listener`（FnMut）的
     clipboard + toast 组合——已在该处注释记录原因，避免后人反复重试。
   - **批次 5 · ai_view / workflow_view / conflicts 收尾**：评审历史弹窗的
     加载/错误/空三态从纯文字 div 改为 `panel_empty_row` + `inline_error_bubble`；
     全文件旧 token 与字号字面值 token 化。Runbook Studio 模板导航列头
     （标题 + 新建/刷新/目录三按钮）从自绘 `justify_between + border_b_1` 迁到
     `panel_section_header(...).action(×3)`，目录标签行 `truncate()` 改硬裁剪。
     冲突工作区变更摘要「存在 N 个冲突文件」迁到 `panel_section_header`，
     与工作台其他面板头统一；`conflict_row`（变更列表卡片行，非 uniform_list）
     保留边框卡片形态——平面列表行规则只约束虚拟列表与设置表单行。
   - 验证：`cargo check --all-targets` 零错误零警告；`cargo test --lib` 526 通过；
     `cargo test --bin khaslana` 322 通过；五个批次之间逐批编译 + 测试回归。
   - **仍未完成（M6 剩余）**：① 真实前台的 Kit 输入键盘/焦点矩阵（工作流编辑器
     动态字段的 Tab 序、多行 AiDescription 的 Enter/Ctrl+Enter、picker 搜索框）
     需按 M7 验收矩阵复验；② 工作流编辑器第 2 步卡片内多步骤同屏渲染时动态宿主
     的增删性能未实测；③ conflicts 三栏同步滚动、workflow 控制台与运行配置的
     视觉实机对照留待 M7。

8. **M7 清理与验收（本轮，工程检查批次）**：
   - **旧引用扫描（计划 §M7 第 1 项）**：`grep` 全项目 + `Cargo.toml` + `build.rs`，
     `yororen` / `gpui-ce` 仅剩注释与测试字符串（如 `tests/code_index_view.rs` 的
     "gpui-ce" 是数据样本）；`cargo tree` 确认单一 GPUI 家族（gpui-pre 0.3.5 +
     gpui-kit 0.6.4，`-d` 的多路径是同一 crate 复用，无第二运行时版本）；
     `Application::new()` 等孤立旧初始化无残留。
   - **自绘输入通路整体删除（计划 §M7 第 2 项）**。M6 收口后全部字段（静态 +
     工作流动态）均已有 Kit 宿主，自绘路径成为不可达死代码：
     - `repository_ui.rs`：删 `input()` 的自绘回退分支与 `single_line_input` /
       `multi_line_input`（约 250 行）；无宿主改为 `debug_assert!` + 空占位，
       不再静默回退第二套输入实现。
     - `repository_core.rs`：删 16 个自绘 `text_*` handler（约 190 行）；
       `submit_focused_field` 的 `ConflictEditor` 分支随 M6 结果区只读化已删。
     - `main.rs`：删 `impl EntityInputHandler for RepositoryView`（约 110 行，
       自绘输入时代的 IME 通路——Kit `InputState` 自带 handler）；删
       `actions!` 中 16 个自绘 action 与 22 条 "TextInput" context 键位
       （自绘输入删除后无任何元素声明该 context），只留 `TextSubmit`
       （"Input" context 由 Kit 输入组件内部声明，Ctrl/Cmd+Enter 与
       secondary-enter 提交路径不变）。
     - `text_input.rs`：1183 行重建为 108 行业务真值容器——`TextFieldState`
       （focus + placeholder + `TextEditState` 值/密文/光标）保留（约两百处
       表单读写点直接访问，Kit 宿主每帧经 `ensure_kit_fields` 对齐），
       自绘输入元素、29 个编辑/IME/utf16 方法、`multiline_caret_follow_decision`
       与 `EntityInputHandler` 依赖的私有函数全部删除；配套单测文件
       `tests/text_input.rs`（11 个自绘编辑行为测试）随之删除。
     - 死开关链收尾：`sync_conflict_editor_into_state` 空操作与 7 个调用点删除
       （M6 已注释「恢复可编辑结果区时的接回点」，删除不影响任何行为）。
     - `ui_helpers.rs`：7 个旧 `COLOR_*` 兼容别名从 `pub(crate) use` 降为私有
       `use`（使用全在文件内部，AGENTS.md「不得作为新 UI 代码导入来源」）。
   - **工程检查（计划 §6.4 四项，全部实跑）**：`cargo check --all-targets`
     零错误零警告；`cargo test --lib` 526 通过 / 2 ignored；`cargo test
     --bin khaslana` 310 通过 / 0 失败；`cargo build --release` 成功。
     测试数变化：bin 322 → 310（删除 12 个随自绘通路移除的测试：text_input
     11 个 + main.rs 的 multiline/conflict 开关 2 个，另 fields.rs 迁移
     断言改写为「唯一例外 ConflictEditor」）；lib 526 不变。
   - **文档同步（计划 §M7 第 4 项）**：README 技术栈改 gpui-pre + gpui-kit；
     AGENTS.md 重写 text_input 相关三条（自绘通路已删、只剩业务真值、目录
     清单补新职责条目）、键盘交互条目标注 `text_input::` 仅存 `TextSubmit`；
     `docs/ui-design-system.md` 的「Yororen 桥接」改「Kit 主题桥接」、
     `icon_command_button` 与「所有按钮纯鼠标」两条旧规则更新为 Kit 标准
     交互（覆盖声明见两份文档开头）。
   - **遗留项（需真实前台会话或实机对照，不在本计划自动发版）**：
     ① 约 400 处旧一代 token 引用（`FOREGROUND`/`MUTED_FOREGROUND`/`CARD`/
     `SECONDARY`/`BORDER`，集中于 dialog_view / workflow_editor /
     repository_ui 等）换 `CONTENT_*`/`SURFACE_*`/`STATE_*` 语义层——新旧
     token 色值不同（如 FOREGROUND `0x2A2933` vs CONTENT_PRIMARY `0x20232B`），
     属视觉变更，须按 §6.2 逐屏对照后执行；
     ② 工作流编辑器动态字段真实键盘矩阵（Tab 序、AiDescription Enter 语义、
     picker 搜索框）与 M5/M6 各页视觉实机验收；
     ③ 性能验收（§6.3：20,000 行 diff / 10,000 行列表 p95 ≤16.7ms、启动与
     内存对比基线）；
     ④ 便携包 / 安装器按原发布流程评估（`installer/khaslana.iss`），本计划
     不自动发版。

阶段编号沿用原计划；24–39 人日是初始总量估算，不代表当前剩余工期。M2 的实际改动量远低于
原预期（依赖切换 + 约 60 处 API 适配，而非 35 个文件的导入重写），M3 之后可按实际差异重估。

---

## 9. 2026-09-22 总审查修正轮（R1–R8）

> 二次审查后的实现已更新：焦点返回改为逐层栈、在新元素树挂载后设置焦点；评审历史纳入模态隔离，受跟踪菜单独立接管焦点；工作流 AI 回填增加编辑会话身份。具体修正与验证边界见 [二次审查修正记录](gpui-kit-refactor-recheck-2026-09-22.md#本轮代码修正记录)。下文为第一轮实施记录，完整前台验收仍未完成。

依据 [GPUI Kit 重构总审查](gpui-kit-refactor-review-2026-09-22.md) 执行 A–D 四批修正，
只改相关代码与文档，不夹带格式化或业务架构重写。

**批次 A · 模态与关闭安全（R1 / R4 / R7）**
- 统一模态宿主：普通对话框、设置中心、「需要凭据」面板、AI 思考窗、全局符号搜索面板
  的遮罩经 Kit `focus_trap` 挂焦点圈（`dialog_focus` / `settings_center_focus` /
  `credential_prompt_focus` / `ai_thinking_focus` / `code_palette_focus`）；打开时
  `maintain_overlay_focus` 把焦点移入圈内，Tab/Shift+Tab 经 Kit Root 的 trap 逻辑
  在圈内循环，不漏到遮罩下层。
- Esc 关闭顺序改为按实际绘制层级自顶到底（待输凭据 → 无遮罩弹层 → AI 思考窗 →
  代码面板 → 评审历史 → 对话框 → 设置中心）；AI 思考窗 Esc 复用「后台运行」语义
  （只收起不终止，绝不穿透关闭父层）；`close_dialog` 对工作流编辑器改走
  `close_workflow_editor` 完整状态清理。
- 焦点归还进入浮层前的触发器（`OverlayFocusReturn` 弱句柄；目标已销毁回退父层
  焦点圈，最后回应用根）；鼠标关闭（遮罩按钮、点外部关菜单）与 Esc 同一策略。
- `text_submit` 增加顶层模态归属校验：焦点不在最上层模态内时忽略 Enter/Ctrl+Enter。

**批次 B · 输入与任务身份（R5 / R8）**
- `TextFieldState` 新增构造期 uid；Kit 宿主记录业务身份（uid + placeholder），
  业务对象被整体更换（模板加载 / 编辑器重建 / 步骤交换 / AI 回填）时
  `rebind_kit_field` 完整重绑：占位符、焦点指向重指、强制 `set_value` 清撤销
  历史——位置相同、文本也相同的不一定是同一个输入框。
- 一次性 AI 生成任务拆为「运行态」（`ai_thinking_task`，持互斥）与「可见态」
  （`ai_thinking_overlay`）：后台运行只收起弹窗，互斥到任务完成/失败才释放；
  增量/成功/失败事件携带任务 id 按身份寻址，迟到事件不影响其他任务；共用
  `AiRequestFailed` 按任务归属只复位本业务 loading（连接测试失败拆为独立事件
  `AiConnectionTestFailed`）。

**批次 C · 组件与主题收口（R2 / R3 / R6）**
- `kit_theme::apply` 重建主题后按变体显式写入 Kit `ThemeMode`（`Theme::from` 恒写
  Light，深色下 `Theme::is_dark()` 分支此前全错）。
- 大弹窗面板尺寸统一经 `dialog_panel_size` 按视口钳制 `min(设计尺寸, 可用空间)`
  （安全边距 24px、下限 480×320）：设置中心、工作流编辑器、远端管理、代码面板、
  AI 思考窗；最小窗与高 DPI 下不再越界被根圆角裁掉。
- 右键菜单键盘模型：九个上下文菜单打开时构建条目清单（稳定业务 id + 动作闭包），
  ↑/↓ 循环选择（跳禁用项）、Enter 执行选中项、Esc 关闭、焦点归还触发器；菜单容器
  挂焦点圈。设置中心九类分类行与关闭按钮改用 Kit 基础按钮（Tab 聚焦 + Enter/Space）。

**批次 D · 视觉 token 迁移与文档收口**
- 旧一代 token 引用全量迁移：文字色 `FOREGROUND`/`MUTED_FOREGROUND` 及
  `*_FOREGROUND` 族 → `CONTENT_PRIMARY`/`CONTENT_SECONDARY`（237 处）；表面色
  `CARD`→`WB_PANEL`、`SECONDARY`→`WB_ROW_HOVER`、`BORDER`→`BORDER_MUTED`、
  `ACCENT`→`STATE_HOVER`、`TILE`→`SURFACE_SUNKEN`、`POPOVER`→`SURFACE_OVERLAY`
  （171 处，17 个文件）。逐屏修正：分段按钮/筛选 chip/代码面板选中行改用
  `STATE_SELECTION`（与悬停区分），Kit Switch 轨道取 `BORDER_STRONG`（与 input
  同档）。语义色（`FEEDBACK_*`/`REF_*`/`COLOR_*`/`DESTRUCTIVE*`/`TOOLTIP_*`）不在
  本轮范围，保持原_token。
- 文档同步：`ui/fields.rs` 模块文档改为「全部字段已迁入 Kit（含动态字段 + uid 重绑）」；
  计划 §6.1 功能/验收矩阵的冲突工作台两行改为「只读结果区 + 按块接受/AI 回填」。

**剩余自绘通用组件与保留理由**（不进 Kit 迁移清单）：
- `Notification`/toast 气泡栈（`ui/components.rs`）：应用自有生命周期（过期回收、
  可点击动作、双通道错误提示），Kit Notification 无等价语义，保留自绘。
- `tooltip_text`：窄档图标命令的轻量提示，Kit Tooltip 需要实体宿主，保留自绘。
- `scrollable_frame_when`/自绘滚动条：统一悬停出现与主题 token，保留。
- 右键菜单容器与条目：业务动作寻址与锚定策略特殊，保留自绘 + 自建键盘模型
  （见批次 C）；迁移到 Kit PopupMenu 需重做锚定与视觉，列为后续评估项。

**工程检查**：`cargo check --all-targets` 零错误零警告；`cargo test --lib` 526 通过 /
2 ignored；`cargo test --bin khaslana` 317 通过 / 0 失败（较审查基线 310 新增 7 个
守卫测试：浮层层级序、模态分类、打开/替换判定各一组、Kit mode 映射、面板钳制、
宿主身份重绑判定）；`cargo build --release` 成功；`cargo fmt --check` 通过。

**仍需真实前台/实机验收（不以源码推断代替通过）**：R1 焦点圈两轮往返 Tab 与嵌套
弹窗关闭回父层、R5 双模板同位置字段切换后 placeholder/Ctrl+Z、R6 菜单方向键循环
与九类设置键盘遍历、R7 各关闭路径焦点归还、R8 后台运行互斥假流；§6.2 视觉矩阵
（860×520 / 1280×820 / 1440×900 / 最大化 × 深浅 × 100–200% DPI）与 §6.3 性能
矩阵；IME、多显示器、托盘、安装包验证。
