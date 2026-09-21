# GPUI Kit 原生工作区样板（M1 验证窗口）

日期：2026-09-20。基线：`dev_gpuikit`。对照设计稿：[Pencil 原生工作区验证稿](gpui-kit-pencil-design.md)。

本样板是[重构计划](gpui-kit-refactor-plan.md) §5 M1 的交付物：在**隔离工程**里把「轻盈悬浮工作台」用真实 GPUI Kit 渲染出来，用于视觉对照和接缝排查。它不修改主工程依赖，也不连接真实仓库。

## 1. 位置与运行

```
kit-spike/           独立 Cargo 工程（自带 [workspace]，不在主工程依赖图内）
├── Cargo.toml       只依赖 gpui-kit = "=0.6.4"
└── src/
    ├── main.rs      启动、窗口、资产注册
    ├── tokens.rs    设计 token（画板实测值）
    ├── theme.rs     token → Kit 主题映射
    └── workbench.rs 工作区视图（导航 / 文件列表 / Diff / 提交区 / 工具栏）
```

```powershell
cd kit-spike
cargo run            # 首次全量构建约 10 分钟，之后增量 5–10 秒
```

窗口按画板 1440 × 1060 逻辑像素打开，最小窗 860 × 520。右上角的「无暂存 / 有暂存」是样板自带的演示开关，用于切换画板 `Sgcwi` 与 `a4JtW` 两种状态。

## 2. 验收截图

| 状态 | 截图 |
| --- | --- |
| 有暂存（双列表，对照 `a4JtW`） | [wide-staged.png](assets/gpui-kit-research/kit-spike/wide-staged.png) |
| 无暂存（单列表，对照 `Sgcwi`） | [wide-unstaged.png](assets/gpui-kit-research/kit-spike/wide-unstaged.png) |
| 最小窗 860 × 520 | [compact-860x520.png](assets/gpui-kit-research/kit-spike/compact-860x520.png) |

三张截图取自 100% DPI 会话，窗口已应用 §3.5 的边框修正（窗口矩形即客户区，左上/右上圆角外为桌面，无白线）。

对照结论：

- 面板层级、圆角、投影、留白与画板一致：环境底 `#F1F5FD`，工作面板圆角 16 / 导航 18，面板阴影向下 5 px、模糊 22 px、冷色 6%，控件接触阴影向下 2 px、模糊 5 px。
- 主次动作分离成立：提交并推送为蓝色实面（含主色阴影），提交到分支与工具栏命令为白色薄实体，禁用态为浅灰实面。
- 红绿差异、行号、hunk 头、状态栏与画板同构；长路径按「硬裁剪不省略号」处理，未覆盖右侧统计。
- 窄窗下工具栏命令退化为图标 + tooltip、导航收成 48 px 图标栏、提交区降为 156 px，命令仍可达（视觉规范 §5）。

尚未在样板中还原（有意留给后续阶段）：菜单与弹层、真实输入法与滚动行为、深浅主题切换、DPI 100%–200% 的逐档核对、动画与过渡时长。

## 3. Kit 接缝与 API 事实（M2/M3 直接相关）

以下均在 `gpui-kit 0.6.4` 上实测得到，不是文档推断。

### 3.1 门面与初始化

- `gpui_kit::*` 就是 GPUI（`gpui-pre 0.3.5`）；`gpui_kit::component` 是 `gpui-component 0.6.4`，`gpui_kit::base` 是 `gpui-base`，`gpui_kit::assets` 是图标资产。
- `gpui_kit::init(cx)` **不注册 AssetSource**。必须 `application().with_assets(...)`，否则所有图标静默不渲染（按钮只剩文字）。
- 默认 `assets::Assets` 只嵌入 101 个组件图标；`git-branch`、`house`、`cloud`、`tag`、`archive`、`panel-left`、`file-code`、`list-filter` 等**不在其中**。本样板用 `assets::AllAssets`（1830 个，约 +1 MB）。主工程迁移时要单独决定图标来源与体积预算。

### 3.2 主题

- 组件外观读的是 `Theme.tokens`，它由 `ThemeColor` **一比一派生**（`ThemeTokens::from(&colors)`）。只改 `Theme::global_mut(cx).colors.*` 而不重建，按钮等组件仍用旧色。
- 正确做法：改 `ThemeColor` 后 `Theme::from(&colors)` 重建整个 `Theme`，再覆盖 `radius` / `font_size` / `scrollbar_mode` 等度量字段（`Theme::from` 会把这些重置为默认值）。
- 按钮变体色来自 `button` / `button_primary` / `button_secondary` 等 **组件级 token**，不是 `primary`。只设 `primary` 会让主按钮保持 Kit 默认的深色。
- `colors.input` 同时是**复选框未选中态的边框色**。把它设成输入框底色（近白）会让复选框在面板上几乎不可见；样板改取描边档，自绘输入外壳不依赖该 token。
- `ScrollbarMode` 默认 `Scrolling`（滚动时出现后淡出），画板无滚动条，样板设为 `Hover`。

### 3.3 组件 API

- `Button`：变体方法来自 `ButtonVariants` trait（`primary()` / `secondary()` / `ghost()` …），`disabled()` 来自 `Disableable`，两者都必须显式导入。
- 尺寸：`Size::Medium` 与 `Size::Large` 都是 32 px 高（`h_8`），圆角由 `theme.radius` 驱动。要 36 px 需 `Size::Size(px(36.))` 或接受 32 px。画板 36 px 的工具栏控件在本样板中按 Kit 默认呈现。
- `Button` 在 `theme.shadow` 为真时自动加 `shadow_xs`；更强的自定义阴影可用 `Styled::shadow(Vec<BoxShadow>)` 覆盖。
- `Icon` 的 `.text_color()` 与 `.size()` 走 `Styled` 的实现（内部有专门分支），可直接使用。
- 输入：`InputState::new(window, cx)` / `TextareaState::new(window, cx)`，`Input::new(&state).appearance(false)` 得到无边框、融入自绘外壳的输入。
- `Root` 必须包裹视图（`cx.new(|cx| Root::new(view, window, cx))`），否则弹层类组件不可用。

### 3.4 键盘策略（2026-09-21 已决）

用户已确认采用 Kit 标准交互，保留现有应用快捷键与业务语义，覆盖旧的普通控件纯鼠标白名单。本样板保持 Kit 默认；仍需验证焦点遍历、激活、嵌套弹层、IME 与业务快捷键的事件优先级，默认行为不等于已验收。

### 3.5 Windows 无边框圆角窗口

样板用「透明窗口背景 + 自绘 24 px 圆角」还原画板的悬浮卡片形态，这个过程暴露了一组 Windows 特有的接缝，M2 的窗口初始化必须一并处理：

- **窗口样式**：gpui 创建窗口时只设 `WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`，但**系统在窗口显示后会补上 `WS_CAPTION`（= `WS_BORDER | WS_DLGFRAME`）**。实测样式为 `0x14CB0000`，客户区比窗口矩形小 16 × 8（`ClientToScreen` 偏移 `L=8 T=0 R=8 B=8`）。
- **故障表现**：带 caption 的窗口由 DWM 沿顶边绘制 1 px 边框。圆角外区域是透明的，这条线只能从那里透出来，所以看到的是「顶部圆角上一道白色横线」——圆角内部被工具栏内容盖住，故障只剩圆角处的两小段。`PrintWindow`（`PW_RENDERFULLCONTENT`）抓到的窗口内容里没有这条线，可用于区分「内容渲染问题」和「系统边框问题」。
- **修复**：窗口创建后立刻移除 `WS_THICKFRAME | WS_CAPTION` 并 `SetWindowPos(SWP_FRAMECHANGED)`，下一帧再执行一次兜底。修复后客户区偏移归零（`L=T=R=B=0`），窗口矩形与客户区一致，且样式不会反弹。只去掉 `WS_THICKFRAME` 不够：即便没有 thick frame 位，caption 仍会让窗口保留那 8 px 不可见缩放边框。
- **取舍**：失去 caption 就等于失去 DWM 窗口阴影、Aero Snap 和双击标题栏最大化。gpui 的拖动与缩放走自己的 `WM_NCHITTEST`，不受影响；窗口自身的投影若要保留需自绘（样板未做）。
- **Win11 属性不可依赖**：`DWMWA_BORDER_COLOR`（边框色）与 `DWMWA_WINDOW_CORNER_PREFERENCE`（系统圆角）在 Windows 10 19045 上返回 `E_INVALIDARG`，只能用上面的样式方案。
- **gpui 的边框偏移会过期**：gpui-pre 只在 `WM_DPICHANGED` 与 `WM_SETTINGCHANGE` 时重算 `border_offset`，而它是在窗口创建、caption 尚在时记下的 16 × 8。去掉边框后该偏移仍参与尺寸换算，窗口最小尺寸会被放大同样的量（设 860 × 520 只能得到 876 × 528）。去边框后补一条 `WM_SETTINGCHANGE` 即可触发重算（gpui 对非滚轮类 action 是空操作），实测最小窗恢复为 860 × 520。
- **窗口自身的投影没有保留**：去掉 caption 就失去 DWM 阴影，样板窗口是一张无投影的圆角卡片。若产品要保留「漂浮」感，需要在窗口内留出边距自绘窗口投影（M2 待决）。

### 3.6 Rust 2024 与元素 API 细节

- `ElementId` 不接受 `(&str, &str)`；可用 `(&'static str, usize/u32/u64/EntityId)`，或 `String` / `SharedString`。
- Rust 2024 下，返回 `impl IntoElement` 且借用 `&mut Context` 的方法在循环里逐次调用会重复可变借用。行列表场景返回 `AnyElement`（`.into_any_element()`）即可。
- 自定义 `BoxShadow`：`BoxShadow::new(px(0.), px(5.), color).blur_radius(px(22.))`，经 `.shadow(vec![...])` 施加。
- 窗口尺寸是逻辑像素：1440 × 1060 在 125% DPI 下为 1818 × 1334 物理像素；非 DPI 感知的截图脚本会得到错误坐标（需 `SetProcessDPIAware`）。

## 4. 依赖共同解析验证

在样板工程临时加入主工程的关键依赖后执行 `cargo metadata` 与 `cargo tree -d`，验证能否与 Kit 共存于同一依赖图（验证后已还原为最小依赖）：

| 依赖 | 结果 |
| --- | --- |
| `tree-sitter 0.25.10` | 解析通过（Kit 的 tree-sitter feature 默认关闭，无 0.26 冲突） |
| `git2 0.21` / `libgit2-sys`、`rusqlite 0.40`、`ureq 3`、`syntect 5`、`keyring 4`、`zip 8` | 解析通过 |
| `gpui-ce 0.3` | **失败：0.3.2 / 0.3.3 已在 crates.io 被 yank**，新工程无法解析 |
| `yororen_ui 0.2` | **失败：其依赖 `gpui-ce ^0.3` 无法解析** |

两点结论：

1. Kit 与主工程的索引、Git、存储、网络依赖可共存，M2 的依赖切换没有版本级阻塞。
2. `gpui-ce` 与 `yororen_ui` 这条链路只能靠既有 `Cargo.lock` 维持，任何重新解析都会失败。**渐进迁移在依赖层面不可行**，M2 必须一次替换底层，这与计划 §4.3 的判断一致。

## 5. 已知限制

- 样板是视觉与接缝验证，不含业务：无 Git 调用、无凭据、无事件流、无后台任务。
- 主题只在启动时设置一次，未接系统主题跟随；切换深浅色需要重建 `Theme` 的完整路径（见 §3.2）。
- 未验证中文 IME、剪贴板、双向滚动性能、20K 行 diff 与 100%–200% DPI 逐档表现，这些属于 M1 清单中尚未完成的部分。
- 输入框、按钮保持 Kit 默认，符合已选交互方向，但与业务快捷键、IME 和弹层的集成验证尚未完成。
