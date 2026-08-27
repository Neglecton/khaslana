# Khaslana UI 设计系统：Calm Technical

## 目标

Focus Workbench 使用「Calm Technical」风格：信息密度适合 Git 日常工作，但降低装饰竞争。界面以清晰的层级、稳定的空间节奏、可预测的悬停与选中反馈帮助用户持续定位，而不是用渐变、玻璃或堆叠卡片吸引注意力。

当前阶段已统一应用壳层、基础原语和主要专业页面：工作区、提交记录、工作流、分支浏览/比较、贮藏、追溯、冲突工作台与设置分类页均使用同一视觉语言，同时保留既有 Git、异步、虚拟列表和状态守卫。

## Shell 与导航

- 自定义 titlebar 高 **44px**：品牌、仓库切换、当前分支摘要、刷新/获取/拉取/推送/贮藏/子模块命令、「设置」按钮和原生窗口控制。「设置」固定在窗口控制区左侧（紧邻最小化按钮），全部命令在任何窗口宽度常驻内联，不设 overflow 收纳。
- Context Navigator 是唯一左侧列（无独立 Activity Rail）：展开态为「上下文导航」标题 + 模式按钮区（工作区/冲突处理(条件)/提交记录/工作流，图标+文字统一按钮）+ 仓库引用分组；收起态为 **48px 窄条**（顶部展开箭头 + 模式图标），任何页面常驻（专用页面的模式图标是返回主工作台的入口）。本地分支与远端/远端分支/标签/贮藏一样可折叠，且是唯一默认展开的分组。
- Navigator 承载仓库引用上下文。工作区默认可见，历史和工作流默认隐藏；每个仓库、每个模式分别保存用户偏好，模式切换不重置。窄窗口以覆盖层临时展开，不改写宽屏停靠偏好。
- 原生拖拽、最小化、最大化和关闭继续使用 `WindowControlArea`；仓库切换菜单仍由其已有锚点与外部点击关闭逻辑处理。
- 分支名区域可弹性收缩：空间充裕时最长 240px，空间紧张时先于拖拽区收窄截断，悬浮显示完整分支名；仓库切换按钮的仓库名同样悬浮显示完整路径。

## 响应式策略

策略在 `src/chrome_view.rs::shell_layout_policy` 中保持纯函数，并以 1120px 与 1440px 为边界：

| 宽度 | Context Navigator | 标题栏命令 |
| --- | --- | --- |
| `< 1120px` | 覆盖/收起，由按钮展开 | 全部命令（含“贮藏”“子模块”）常驻内联，隐藏当前分支摘要 |
| `1120–1439px` | 正常显示 | 全部命令（含“贮藏”“子模块”）常驻内联 |
| `≥ 1440px` | 正常显示 | 全部命令（含“贮藏”“子模块”）常驻内联 |

“设置”不受宽度档影响，始终固定在窗口控制区左侧。

运行时通过 `Window::viewport_size()` 读取 GPUI 逻辑像素宽度并传入同一纯函数，因此三档策略与测试使用完全相同的边界。窄窗不再挤压主画布：Navigator 由入口按钮打开为遮罩覆盖层；恢复到标准宽度后清除临时覆盖态，但保留各模式停靠偏好。

## 核心任务画布

- **Review Canvas（工作区）**：页头只表达当前分支与工作区语境；左侧暂存/未暂存变更使用虚拟列表，右侧差异画布取得剩余空间，提交框保持完整 Git 行为。部分暂存、编码、二进制、超大文件和差异缓存守卫不因视觉重排而降级。
- **History Inspector（提交记录）**：提交导航全高固定在左侧；导航行不画泳道（拓扑整体移入提交图谱页），行内引用标签最多 1 个（HEAD/首个本地分支优先，其余「+n」悬浮查看）；每条提交使用专用 48px 两行布局，第一行显示摘要/ref，第二行显示作者/avatar，完整容纳头像与引用徽标，不得套用全局常规 36px 行。右侧上方是可调高度的提交详情，下方为文件导航与占满余量的历史差异。默认详情高度必须与拖拽状态复用同一常量，避免首次拖拽跳变。
- **Commit Graph（提交图谱）**：拓扑专注型专用页——全高泳道列表（分支动向高亮：谱系外行泳道线降透明度 + 内容降不透明度；淡化合并提交开关；搜索过滤激活时泳道自动隐藏）+ 底部可折叠轻量详情卡；与主历史页共享提交列表与选中状态，「在提交记录页查看」跳转后经「图谱」按钮无损返回（高亮/开关/搜索词/滚动位置保留）。与 History 提交行共用 48px 两行结构与 `commit_row_content` 构建器。
- **Runbook Studio（工作流）**：模板导航、运行配置、步骤时间线和按需展开的控制台构成稳定工作台。模板导航采用轻量索引模型和单一 `uniform_list`，只在可视 range 内创建元素；Context Navigator 则是钉住分组标题 + 每分组独立虚拟列表。
- **专业次级视图**：Browse/Compare、Stash、Blame、Commit Graph 与 Conflict 保留各自领域布局、虚拟化、编码和异步代际守卫，但统一使用同一 surface/content/border/feedback token 与平面列表语言。

任何可能达到成千上万项的导航或变更列表都不得在 render 中预构造 `Vec<AnyElement>`；应先生成轻量索引/行模型，再由 `uniform_list` 可视回调读取当前快照。每个滚动容器必须有界：Context Navigator 的分组列表为钉住标题 + 每分组独立有界虚拟列表（条目少按内容定高、条目多平分剩余空间），分组标题不得随条目滚出视口，也不能回到预建全部行元素。

## Token

所有业务 UI 通过 `src/ui/theme.rs` 的 `rgb` / `rgba` 使用 token；禁止在业务 view 新增硬编码颜色。

- **Surface**：`SURFACE_CANVAS`、`SURFACE_BASE`、`SURFACE_RAISED`、`SURFACE_SUNKEN`、`SURFACE_OVERLAY`。
- **Content / border**：`CONTENT_PRIMARY`、`CONTENT_SECONDARY`、`CONTENT_TERTIARY`、`BORDER_MUTED`、`BORDER_STRONG`。
- **交互状态**：`STATE_HOVER`、`STATE_SELECTION`、`STATE_FOCUS_RING`；强调色继续由九个 accent 预设驱动 `PRIMARY` / `PRIMARY_SUBTLE`。
- **状态与层级**：Git、diff、反馈 token 保持兼容；`SHADOW_ELEVATION_1/2` 仅用于浮层和对话框。
- **间距**：4px 基线，`SPACE_1..SPACE_6`（4/8/12/16/20/24）。
- **排版**：`TYPE_META` 11px、`TYPE_BODY` 12px、`TYPE_TITLE` 14px、`TYPE_PAGE_TITLE` 16px。中文正文优先清晰而非压缩字距。
- **密度**：紧凑控件 28px、常规控件 32px；全局紧凑行 28px、全局常规行 36px。History 提交导航与提交图谱页的每条提交固定使用专用 48px 两行布局（第一行摘要/ref，第二行作者/avatar）；该例外不改变其他列表的 36px 常规行规范。
- **圆角 / 动效**：圆角以 6/8/10px 为主；只允许 120/180ms 的 hover/active 瞬态反馈，不加入持续装饰动画。

## 组件原语

`src/ui/components.rs` 提供并保持以下约束：

- `list_row_surface`：默认无完整边框、无阴影；选中态为淡强调色背景与左侧 2px 指示条。
- `icon_button`：无状态的图标按钮视觉底座，统一尺寸、悬停、禁用原因 tooltip 和标签 tooltip；它不创建临时 `FocusHandle`。
- `icon_command_button`：壳层图标命令按钮（原 `focusable_icon_button`），**纯鼠标交互**——不可聚焦、无 Enter/Space 激活（gpui-ce 会对聚焦元素在 Enter/Space 松开时合成点击且鼠标按下自动聚焦，剥夺可聚焦性是唯一根治手段；键盘白名单见 AGENTS.md §8）。已全局移除键盘焦点可视环（`focus_visible`）：按 Ctrl/Alt/空格/Tab 等修饰键不再让控件出现额外边框或背景突变。不能在 render 中临时创建焦点句柄。
- `page_header`、`command_group`、`empty_state`：页面级标题、命令排列和空状态的基础结构。
- 现有 `button`、`input_frame`、`dialog_*`、反馈 API 保持兼容。自定义文本输入继续只由 `src/text_input.rs` 管理编辑、IME、选区与光标逻辑。

图标沿用嵌入式 SVG，单色图标继承语义前景色；品牌 logo 保持原始多色资源渲染。不要用 emoji 作为产品图标。

## 阴影、渐变与可访问性

- 默认页面、列表、行和导航使用平面 surface；阴影只用于菜单、对话框、toast 和遮罩上方的明确浮层。
- 不新增装饰性渐变、玻璃态或多层卡片边框。
- 所有按钮纯鼠标交互：不可聚焦、无键盘激活、无焦点环（键盘白名单见 AGENTS.md §8——仅保留应用级可配置快捷键、文本框内编辑/提交、变更列表 Shift/Ctrl 点选）。后续新控件同样不得添加 `focus_visible` 样式、`track_focus`/`tab_index` 或按键激活。
- 禁用控件保留可解释 tooltip；普通可点击按钮不强制 tooltip，图标-only 控件提供标签。
- 保持键盘快捷键和原有点击目标；Context Navigator 的模式按钮只是增加入口，不能移除原业务操作。

## Windows、DPI 与滚动

- 尺寸均使用 GPUI `px` 逻辑像素，让框架处理 DPI 缩放；不要按物理像素写平台分支。
- 窄窗口优先收纳而不是压缩 titlebar 或覆盖窗口控制区。
- 浮层仍须遵循根层捕获、菜单锚定、overlay 顺序和 blocker 约束。
- 可滚动内容继续采用项目规定的有界外层、`scrollable_frame_when` 与内容滚动容器结构；壳层不改变页面内部滚动/虚拟化策略。

## Yororen 桥接

`yororen_ui 0.2` 的公开 `Theme` API 提供 `surface`、`content`、`border`、`action`、`status` 与 `shadow` 字段。`src/theme_view.rs` 直接以 Khaslana token 和当前 accent 填充这些字段，不使用私有 API、hack 或依赖升级。若未来 0.2 API 改为私有，应保留 Khaslana token 与默认 Yororen fallback，并记录限制，而不是绕过可见性。
