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
- **窗口自身的投影没有保留**：去掉 caption 就失去 DWM 阴影，样板窗口是一张无投影的圆角卡片。**已决（2026-09-21，M3 落地并复核）**：主工程已采用样板这条「透明背景 + 自绘圆角」路线——窗口 `WindowBackgroundAppearance::Transparent`、外壳 24px 圆角、`chrome_view::apply_window_chrome` 去 `WS_THICKFRAME | WS_CAPTION` 并触发 `border_offset` 重算（实测窗口样式 `caption=False thickframe=False`，客户区与窗口矩形等大，最小窗回到精确 860 × 520）。**投影仍不自绘**：接受无 DWM 阴影，也不在窗口内留边距，层次改由窗口内部的悬浮面板（面板投影 + 环境底）表达。圆角有一条实现约束：gpui 的 `overflow_hidden` 只裁矩形，所有铺满窗口的色块（根背景、顶栏上角、对话框遮罩、操作遮罩）必须各自画同样的圆角，且最大化时圆角要归零。
- **更正一条前轮结论**：本节原先写的「gpui 的拖动与缩放走自己的 WM_NCHITTEST，不受影响」只对**顶边**成立——gpui 在 `handle_hit_test_msg` 里只按 DPI 补了顶边与两个上角（`HTTOP/HTTOPLEFT/HTTOPRIGHT`），左右下三边原本靠 `WS_THICKFRAME` 由 `DefWindowProc` 给出缩放带。去掉边框后实测 `WM_NCHITTEST`：顶边 `HTTOP`，左/右/下均 `HTCLIENT`（无缩放带）。M3 的处理是补三条 6px 透明带 + 两个下角，按下时发 `WM_NCLBUTTONDOWN` + `HTLEFT/HTRIGHT/HTBOTTOM/HTBOTTOMLEFT/HTBOTTOMRIGHT` 把该边交回系统的模态缩放循环（无边框窗口恢复缩放的标准做法）。另外「只去掉 `WS_THICKFRAME` 不够」这句要补完：**只去掉 `WS_CAPTION` 也不够**——实测客户区仍内缩 14 × 7，圆角卡片会被一圈系统框包住，所以两者必须一起去掉。
- **最大化按钮必须有还原分支**：`zoom_window()` 只最大化，gpui 无取消最大化 API。主工程自绘标题栏走的是 `WindowControlArea::Max` 原生命中区——gpui-pre-windows 对 `HTMAXBUTTON` 的单击本来就是切换（`events.rs` 的 `handle_nc_mouse_up_msg`：`is_maximized()` → `SW_NORMAL`，否则 `SW_MAXIMIZE`），配合按 `window.is_maximized()` 切换图标即满足「最大化之后有还原」。样板工作区顶栏的三个窗口按钮当时是纯装饰（`window_control` 没有控制区也没有处理器），不作为实现参照；M3 起主工程按控制区方案落实并实测。

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

## 5. M1 收口验证（2026-09-21）

样板加入了 **M1 验证台**（工具栏「验证台」按钮，或用 `KHASLANA_SPIKE_VIEW=verify` 启动直达），
把计划 §5 M1 清单里的性能、主题、窗口三项做成可脚本化采证的页面；
`KHASLANA_SPIKE_SELFTEST=1` 触发无人值守自检，结果按行写 `kit-spike-verify.log`。

采证环境：Windows 10 19045、100% DPI、1440 × 1060 逻辑像素窗口。
**该会话的屏幕输出不可用**（RDP 桌面 DC 与 `PrintWindow` 都只能拿到空白），
因此本轮以「文件日志 + 运行时断言」代替截图，视觉部分仍以 §2 的三张验收图为准。

### 5.1 实测结果

| 项 | 方法 | 结果 |
| --- | --- | --- |
| 虚拟化是否只建可见行 | 10 000 行列表与 20 000 行差异列表各渲染一帧，统计闭包收到的行元素数 | **通过**：两者都只构建 **39 个**行元素（可见 17 行 + 缓冲），与总行数无关 |
| 大跨度滚动开销 | 60 次跨步滚动（每步跨 1/60 全表）+ 逐次重绘，release 构建 | **通过**：60 步共 802 ms / 808 ms，13.4 ms/步；扣掉每步 12 ms 的固定间隔，渲染开销约 1.4 ms/步 |
| 深浅主题切换 | 依次切深、切浅，各自走 `Theme::from(&colors)` 全量重建 | **通过**：两次切换均无 panic、无断言失败（视觉差异因无屏幕输出未在本轮取证） |
| `zoom_window()` 语义 | 连续调用两次并读 `is_maximized()` | **不是切换**：两次调用后 `maximized` 均为 true。源码同证：Windows 侧只发 `SW_MAXIMIZE`（`gpui-pre-windows/src/window.rs:929`），doc comment 写的 toggle 与实现不符 |
| 取消最大化 | — | **gpui 无对应 API**：`ShowWindow(SW_RESTORE)` 可复位（实测 `is_maximized` 变 false），自绘标题栏的最大化按钮必须自己做这条分支。**M3 落地做法**：主工程不用 `zoom_window()`，改用 `WindowControlArea::Max` 原生命中区——gpui-pre-windows 对 `HTMAXBUTTON` 的单击本身就是最大化↔还原切换；M3 向该位置投递 `WM_NCLBUTTONDOWN/UP(HTMAXBUTTON)` 复测：1936 × 1056 最大化、再次投递还原 1296 × 828，图标同步切到还原态（证据见重构计划 §8）。 |
| 中文 IME | 见 §5.6 第二轮补验 | **已实测通过**：真实 TSF 组合 + 候选提交，组合期间 Enter 不穿透 |
| 键盘遍历与激活 | 见 §5.6 第二轮补验 | **已实测通过**：Tab 焦点迁移、Enter/Space 语义、剪贴板/撤销 |
| DPI 125/150/200% | 需改系统缩放并重启会话 | **未实测**，本轮仅 100% |

### 5.2 未实测项的原因与已有证据

采样会话不提供可读的屏幕输出，`SetForegroundWindow` 恒失败（实测 `FOREGROUND_IS_TARGET=False`），
`PostMessage` 注入的 `WM_KEYDOWN` 也未能进入 gpui 的输入回调——**没有前台窗口就没有键盘通路**，
中文 IME、剪贴板/撤销、Tab 遍历与 Enter/Space 激活因此无法在本轮自动化验证。这部分需要在可交互的
会话里补一次人工验收。

已有的是**源码级证据**，说明实现路径完整而非缺失：

- `gpui-pre-windows/src/events.rs` 完整处理 `WM_IME_STARTCOMPOSITION`（候选框随光标）、
  `WM_IME_COMPOSITION`（`GCS_RESULTSTR` 提交串 + `GCS_COMPSTR` 组合串）、`GCS_CURSORPOS` 与
  `GCS_COMPATTR`，并在焦点变化时经 `update_ime_enabled` 启停 IME。
- `gpui-base` 的输入引擎有 IME 组合事务（`ime_marked_range`）与单测
  `test_ime_composition_undoes_as_one_unit`——一次组合在撤销栈里是一个单元。
- 键盘分发路径（`handle_keydown_msg` / `handle_keyup_msg`）没有窗口激活守卫，
  输入未被前台条件拦截；失败发生在更外层（窗口拿不到前台）。

### 5.3 Go / No-Go 结论

**Go**（有条件）。计划 §5 M1 列出的 No-Go 条件在本轮均未触发：

- 依赖可以统一，且**不需要逐个改写导入**（见下条）；
- 原生窗口可用，无边框圆角窗口的处理方式已在 §3.5 定案；
- 性能达标：虚拟化生效，20 000 行差异的滚动开销与列表同量级，没有出现按行数增长的成本；
- 未发现标准交互与既有快捷键的结构性冲突——但此项与 IME 同属未实测，M2 落地时要在真实会话复验。

放行的**前提条件**（M2 期间必须完成，不能推给后续阶段）：

1. ~~**IME 与键盘交互在可交互会话里补验一次**，这是 M1 唯一还悬着的 No-Go 项。~~
   **已在第二轮完成**（见 §5.6）：IME 组合/候选提交/组合期间 Enter 不穿透、Tab 遍历、
   Enter/Space 语义、剪贴板与撤销全部实测通过；仅 Esc 关闭浮层与焦点恢复受会话锁屏
   中断，留待下次可交互会话补测。
2. 主题切换的**视觉**结果需要截图确认（本轮只证明了重建路径不崩）。

### 5.4 M2 前置：别名方案已验证

主工程以 `gpui = { package = "gpui-ce" }` 引用旧运行时，39 个文件写 `use gpui::…`。
在样板里加入 `gpui = { package = "gpui-pre", version = "0.3.5" }` 后实测：

- `&mut gpui::Window` 可以直接赋给 `&mut gpui_kit::Window`，`App` 同理——
  别名与门面指向**同一个 gpui-pre 实例**，类型体系统一，不存在两套 GPUI 类型；
- `gpui::actions!` 与 `#[derive(Action)] #[action(namespace = …)]` 在别名路径下正常展开。

结论：**M2 的导入改写量几乎为零**，改动集中在 `Cargo.toml`、启动路径、资产、主题与
四处 `yororen_ui` 接入点。这显著低于计划 §5 M2 原先「35 个文件涉及 GPUI」的预期工作量。

### 5.5 M2 实施记录：gpui-ce → gpui-pre 的实际 API 差异

主工程已完成依赖切换（`Cargo.toml`、`main.rs` 启动路径、`assets.rs`、`theme_view.rs`、
`remote_branch_operation.rs`、`dialog_view.rs` 的两处 Select）。全目标编译零错误零警告，
829 个测试（lib 526 + bin 303）通过。以下是这次切换真正踩到的差异，M3/M4 会继续遇到：

| 差异 | 表现 | 处理 |
| --- | --- | --- |
| **版本必须精确锁 0.3.5** | `version = "0.3.5"` 是 `^0.3.5`，Cargo 解析到 0.3.6 后 `gpui-component` 直接编译失败（`inspector.rs` 的 `register_inspector_element` 签名对不上） | 写成 `=0.3.5`。**gpui-pre 的补丁版本之间有破坏性变更**，不能放宽 |
| `Window::focus` 多一个参数 | `focus(&mut self, handle, cx: &mut App)`，旧签名只有 handle | 18 处调用补 `cx`；3 处所在方法本身要加 `cx: &mut Context<Self>` 形参并更新调用点；`view.read(cx)` 与 `focus(.., cx)` 同表达式会双重借用，需先取出焦点句柄 |
| `ScrollHandle::max_offset()` 返回 `Point<Pixels>` | 旧类型有 `width`/`height`，现在只有 `x`/`y` | 6 处字段改名 |
| `ShapedLine::paint` 多两个参数 | 新签名是 `(origin, line_height, align, align_width, window, cx)` | 3 处补 `TextAlign::Left, None` |
| 没有 `Application::new()` | 入口由平台层提供 | 改用 `gpui_kit::application()` |
| `easing` 模块是私有的 | `gpui::ease_out_quint` 不对外导出（`mod easing` 非 `pub`） | 内联同一条曲线 |
| Yororen Select 的替代 | 两处远端选择（远端操作弹窗、推送标签弹窗） | 按项目已有的自绘下拉模式重写（触发器 + `deferred` 浮层 + `occlude`），未引入 Kit Select——它的 `SearchableListDelegate` 对这两个简单列表过重，留给 M4 统一评估 |
| Yororen 图标 | `icon(IconName::Arrow/Check)` | 换成应用自绘的 `ToolbarIcon::ChevronRight`（旋转 90°）与文本 `✓` |
| Yororen 主题桥接 | `GlobalTheme` / `ThemeSet` / `ActiveTheme` | 新增 `src/ui/kit_theme.rs`：Khaslana 语义 token → `ThemeColor` → `Theme::from` 重建 |

`cargo tree -d` 复核：gpui 相关只有 `gpui-pre 0.3.5` 一族与 `gpui-base/component/kit 0.6.4`，
没有第二套 GPUI 类型；`Cargo.lock` 中 `gpui-ce` 与 `yororen_ui` 均为 0 条。


### 5.6 M1 悬空项补验：IME 与键盘交互（第二轮，可交互会话）

§5.2 遗留的「需前台窗口交互」项在**有真实输入通路的会话**里补验。本轮不改样板代码，
证据来自验证台的事件日志（`KHASLANA_SPIKE_LOG=1` 写 `kit-spike-verify.log`）与 UIA 值回读。

| 项 | 方法 | 结果 |
| --- | --- | --- |
| 中文 IME 组合 | 把窗口线程键盘布局经 `WM_INPUTLANGCHANGEREQUEST` 切到微软拼音，逐键打拼音 | **通过**：输入框出现 `ni'hao`（音节分隔符 = 真实组合态），空格提交后值为 `你好 `，CJK 无损坏 |
| 组合期间 Enter 不穿透 | 组合态下按 Enter | **通过**：整轮 IME 序列 **0 次** `PressEnter` 事件；Enter 被 IME 消化为「提交拼音原文」（值变 `你好 shijie`） |
| 非组合态 Enter | 组合结束后按 Enter | **通过**：产生 `PressEnter #1 secondary=false shift=false`，与组合态形成对照 |
| Ctrl+Enter 语义 | 多行框按 Ctrl+Enter | **通过**：`PressEnter secondary=true shift=false` |
| 剪贴板 | 输入框 Ctrl+A / Ctrl+C / Ctrl+V | **通过**：系统剪贴板得到 `你好 shijie`（含 IME 提交的中文），粘贴回填空格与内容无损 |
| 撤销 | Ctrl+Z | **通过**：`你好 shijie` 撤回到 `你好 `——一次 IME 组合是一个撤销单元 |
| Tab 焦点遍历 | 输入框内按 Tab | **通过**：`Focus #N` → `Blur #N+1` 序列证明焦点离开输入框移向下一控件 |
| 多行换行 | 多行框按 Enter | **通过**：值出现换行（`\n\nhello ime test`） |
| 按钮激活 | UIA `AXPress`（InvokePattern） | **通过**：Kit Button 的 `on_click` 真实触发（计数 +1 并写事件日志） |
| 滚动压测（复测） | 10 000 行列表 / 20 000 行差异各 60 步 | **通过**：790 ms / 801 ms，每步 13.2 ms / 13.4 ms，两次都只构建 **39 个**行元素 |
| 深浅主题 | 走 `Theme::from` 重建 | **通过**：深、浅两次切换无 panic |
| 窗口最大化语义 | `zoom_window()` 连续两次 | **不是切换**（两次后 `maximized=true`）；`SW_RESTORE` 可复位——与 §5.1 首轮一致 |

**方法学（M3/M4 复验可复用）**：

- **前台激活**：单独调用 `SetForegroundWindow` 会失败（前台为 NULL）。把调用线程
  `AttachThreadInput` 附加到目标窗口线程后再 `SetForegroundWindow` 即可成功。
- **IME 切换**：`LoadKeyboardLayout(…, KLF_ACTIVATE)` 只影响调用线程；须发
  `WM_INPUTLANGCHANGEREQUEST`（`0x50`，lParam = HKL）给目标窗口，再用
  `GetKeyboardLayout(target_thread)` 确认，微软拼音 HKL 为 `0x0804xxxx`。
- **合成输入与 IME**：一次性注入整串文本（`typeText`）**不会**触发 TSF 组合管线，
  只有逐键击键才会走 `WM_IME_COMPOSITION`；验证 IME 必须逐键。
- **UIA 路径**：`set_value` 能写入中文并经 `InputEvent::Change` 落库（可信的文本通路证据）；
  `AXPress` 走 InvokePattern，可验证按钮激活但不经过键盘分发。
- **锁屏会切断键盘通路**：`LockApp.exe` 运行时没有前台窗口，所有键盘注入（含 UIA 键路径）均被拒。

**样板本轮未完成**：Esc 关闭对话框与关闭后焦点恢复、遮罩点击、边缘 Popover——验证会话被
锁屏（`LockApp.exe` 出现，前台窗口归零）中断。主工程 M4 已以 Kit `DropdownMenu` 和根层 Esc
收口补齐实现，真实前台完整矩阵留在 M7 复验；上述 IME/键盘结论不受影响。


## 6. 已知限制

- 样板是视觉与接缝验证，不含业务：无 Git 调用、无凭据、无事件流、无后台任务。
- 主题只在启动时设置一次，未接系统主题跟随；切换深浅色需要重建 `Theme` 的完整路径（见 §3.2）。
- 输入框、按钮保持 Kit 默认，符合已选交互方向；IME、剪贴板/撤销与键盘遍历已在
  §5.6 补验。主工程 M4 已实现 Esc 关闭与焦点恢复，真实前台复验留在 M7。
- §5.1 的性能数字取自上文所述的采证环境：release 构建、无 vsync 节流的窗口。
  该环境下的 GPU 合成是否真实发生无法确认，数字可作为**相对对照**（虚拟化有效、无行数相关成本），
  不宜当作绝对帧时间指标；计划的 60Hz 绝对值验收仍应在本机可交互会话里复测。
