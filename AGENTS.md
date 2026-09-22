# Khaslana 项目 Agent 手册

本文件只记录长期协作规则、架构边界与关键防回归约束。实现细节查源码，阶段进度查专题文档；不要把功能清单、修复流水账或未来路线图追加到这里。

## 1. 协作规则

- 使用简体中文回复；用户可见文案保持中文，blame 统一称「追溯」。代码注释用中文，重点解释约束与原因。
- 只修改任务涉及的代码，不顺手重构、修复无关空格或全库格式化；保留用户已有改动。
- 编码前明确假设。歧义会影响实现时列出解释并询问；发现互相矛盾的要求先澄清；存在明显更简单的方案时直接说明。
- 当前用户明确决策优先于旧设计记录。不能把历史计划当成待办，也不能把源码已实现或单测通过当成真实 UI 验收通过。
- 完成后说明改动、实际验证结果与未验证项。只有长期规则或架构边界改变时才更新本文件。

## 2. 代码发现

- 结构查询优先使用 codebase-memory-mcp。会话开始或上下文压缩后用 `list_projects` / `index_status` 确认项目与索引代际；未索引才运行 `index_repository`。
- 默认采用 Tier 2 验证：`search_graph` 定位符号，按需 `trace_path` 查调用关系，`get_code_snippet` 读实现；复杂关系用 `query_graph`，全局概览用 `get_architecture`。
- 检查分页；对所有作为依据的文件调用 `check_index_coverage`，否定或穷尽性结论还要检查对应 scope。覆盖正常不等于完整性证明；缺失、过期、跳过或未知部分必须回读源码。
- 字符串、配置、非代码文件或图工具不足时使用 `rg` / 文件读取。工具不可用时明确说明，再回退源码检索，不声称完成了图验证。
- 快速查找只能给暂定结论；全面审计需限定范围并披露覆盖限制。如需委派，传递项目、代际、范围、符号、路径、分页与覆盖证据；子代理无 MCP 时依据源码验证。

## 3. 项目与依赖

Khaslana 是 Rust 桌面 Git 客户端，支持多仓库、Git 工作流、差异与历史、冲突处理、工作流模板及 AI 辅助。

- Rust edition、依赖版本和 feature 以 `Cargo.toml` / `Cargo.lock` 为准，不在本文复制整张依赖表。
- UI 使用 `gpui-pre`（Cargo 别名 `gpui`）与 `gpui-kit`。当前精确锁定 `=0.3.5` / `=0.6.4`，不得随意放宽：该组合与 gpui-pre 0.3.6 存在已知编译不兼容。
- Git 使用 `git2` / libgit2；普通持久化使用 SQLite，凭据密文使用系统 Keyring。
- 保留 `ureq` 的 `socks-proxy` feature；网络请求遵循应用内代理设置，配置错误必须报错，不能静默改成直连或环境代理。

## 4. 架构与状态边界

| 入口 | 职责 |
| --- | --- |
| `src/types.rs`、`src/types/` | 领域类型与错误 |
| `src/git.rs`、`src/git/` | `GitService` 与 Git 业务 |
| `src/main.rs`、`src/repository_*.rs` | 应用组装、共享状态、动作与事件路由 |
| `src/*_view.rs`、`src/conflicts/` | 页面与交互 |
| `src/ui/` | 主题、Kit 桥接、通用组件和输入适配 |
| `src/tasks.rs` | 后台任务调度 |
| `src/ai/`、`src/workflow/`、`src/code_index/` | AI、工作流、代码索引服务 |
| `src/storage.rs`、`src/credentials.rs` | 数据存储与凭据 |
| `src/tests/` | 与源码结构对应的测试模块 |

- 新功能按领域拆分；不要继续把完整实现塞入 `main.rs`、`git.rs` 或渲染函数。可用子模块中的 `impl RepositoryView` / `impl GitService` 扩展现有类型。
- 每仓库状态放 `RepoTabState`；跨仓库偏好放 `RepositoryView`。模式切换、仓库切换不得污染其他仓库状态或重置全局布局偏好。
- Git / IO 重任务经 `TaskExecutor` 调度、通过 `UiEvent` 回传，不能阻塞 UI 或随意新建线程。保留短任务、长任务、AI 评审和索引的调度隔离。
- 异步结果必须校验仓库、请求代际及业务对象身份；成功、失败、取消与 panic 均需正确收尾，迟到结果不能覆盖新会话或清除新任务的 loading。
- Git 操作后的刷新沿用现有事件链，保留缓存失效、请求序号、选中项与历史刷新语义。

## 5. Git、数据与 AI 防回归约束

- 危险 Git 操作保留确认和业务守卫。新增能力先完善领域类型与 `GitService`，再接 UI；路径优先使用 `Path` / `PathBuf`，输入复用验证函数。
- 工作区写入必须复用 `src/git/worktree_compat.rs`，不可绕过 checkout、reset、revert、rebase、stash、子模块更新的包装。只容忍被占用空目录的删除失败，锁定文件、冲突与本地修改保护仍须报错。
- 所有网络 Git 操作（含子模块）复用凭据回调与代理策略。secret 不写普通配置或日志；代理认证沿用现有 URL 配置，不另拆密文存储。
- 存储路径通过现有数据目录解析入口取得，不硬编码为 exe 旁或系统目录。变更 SQLite schema、Keyring 服务名、数据迁移或更新清单时保持旧用户兼容并补回归验证。
- Diff 改动需考虑编码、二进制、大文件、虚拟列表和部分暂存守卫；共享差异渲染影响工作区、历史、贮藏、浏览多个入口。语法高亮复用 `src/syntax.rs` 与既有异步回填、身份校验机制。
- 冲突工作台结果区保持只读文档视图，通过按块接受或 AI 回填更新；不要重新引入已删除的自绘编辑器。
- 一次性 AI 生成复用 `start_ai_thinking_task`。互斥依据任务运行态；「后台运行」只隐藏弹窗。增量、成功、失败按任务 id 路由，工作流回填还须匹配编辑会话。
- AI 评审切换目标时只分离展示，取消才终止任务；保留后台完成落盘、并发限制与代际守卫。空正文、流截断不得当作成功；预算、重试和超时策略以实现与测试为准。
- 更新渠道保持兼容：正式清单仅指向正式版，测试版发布不得污染正式渠道；发布流程以 `.github/workflows/release.yml` 为准。

## 6. UI 与输入规则

- 视觉以当前 Pencil 稿、`docs/gpui-kit-visual-spec.md` 与重构计划中的最新已确认决策为准。壳层采用 60px 顶栏、仅仓库名的切换下拉、中文命令、导航底部设置、紧凑窗口按钮；提交区位于 Diff 下方。
- 窄窗也要保持业务命令可达；响应式计算使用逻辑像素和可测试纯函数，临时覆盖导航不改写全局展开偏好。
- 复用 `src/ui/components.rs` 与 `src/ui/theme.rs`。业务视图不新增零散十六进制颜色或旧 `COLOR_*` 引用；语义色通过主题感知 `rgb` / `rgba` 转换，Kit 主题由桥接统一更新。
- 普通按钮使用 Kit 基础按钮；开关统一经 `toggle_switch` / `toggle_row` 使用 Kit `Switch`。回调写回受控值，不在外层重复挂同一点击动作；重复列表项的交互 id 必须按业务身份隔离。
- 普通控件支持 Tab/Shift+Tab、按钮 Enter/Space、菜单方向键与 Enter。可取消浮层支持 Esc；保留模态焦点隔离、顶层关闭顺序与逐层焦点恢复，不能穿透到背景操作。
- 输入通过 `src/ui/fields.rs` 适配：`TextFieldState` 是业务真值，Kit 负责编辑、光标、选区、IME 与剪贴板，不合并两套状态、不新增自绘输入。静态字段注册到 `DEDICATED_FIELDS`；动态字段按业务身份重绑，孤儿回调不得回退写入其他字段。
- 多行 Enter 换行、Ctrl/Cmd+Enter 提交；单行 Enter 提交所在表单并遵守模态归属。动态重绑快捷键只替换应用绑定，保留 Kit 键位和既有快捷键录制、Ctrl+P、文本编辑语义。
- 图标按钮有可访问标签和 tooltip，禁用态解释原因。反馈、错误和进度复用项目 helper；通知统一右下角队列，操作状态在底部状态栏。
- 大列表保持虚拟化。滚动区复用有界外层 → `scrollable_frame_when` → 内容滚动 div 的结构，不增加破坏高度约束的中间 flex 层；分栏边界不重复画线。

## 7. Windows 窗口已确认决策

- 透明窗口背景 + 外壳自绘 24px 圆角；Kit `Root` 显式 `.bordered(false).bg(rgba(0x00000000))`，否则圆角外会被主题底色填满。
- `apply_window_chrome` 去除 `WS_THICKFRAME | WS_CAPTION` 并刷新边框偏移；不要回退为仅去 caption。保留自绘缩放带与系统缩放循环，弹窗打开时仍可缩放。
- 铺满窗口的背景与遮罩共用 `window_radius()`；最大化时圆角归零并隐藏缩放带。`overflow_hidden` 不能代替圆角绘制。
- 最大化按钮使用 `WindowControlArea::Max` 支持最大化/还原，不改成只最大化的 `zoom_window()`。
- 不补自绘窗口投影、不为投影增加窗口内边距；内部面板层次按当前视觉规范处理。

## 8. 验证与交付

按改动范围执行检查，报告真实结果：

```powershell
cargo check --all-targets
cargo test --lib --bin khaslana --quiet
cargo build --release
git diff --check
```

- 代码实现完成需 release 构建零错误零警告；发现既有无关问题时报告，不擅自扩大修改范围。新增二进制或 feature 补对应检查。
- 纯文档修改检查内容、路径与 diff 即可，无需编译。不要把 `cargo fmt` 当默认收尾步骤；必要格式调整限定本次修改范围。
- 测试放 `src/tests/` 对应模块，经 `#[path]` 挂载；Git 测试复用 `src/git/test_support.rs` 与临时仓库。优先覆盖业务行为与回归场景。
- UI 修改按重构计划验收矩阵验证键盘、焦点、IME、深浅主题、窄窗和 DPI；未做实机验证就明确标注未验证。Windows 注入 `WM_KEYDOWN` 使用 `PostMessage`，避免同步重入丢键。
- 本地运行使用 `cargo run --bin khaslana`；正式打包 profile 为 `release-perf`，`cargo setup` 构建安装器（需 Inno Setup）。构建验证不意味着自动提交、打 tag 或发布。

## 9. 按需阅读与维护

| 任务 | 参考 |
| --- | --- |
| 项目使用与能力 | [README](README.md) |
| UI 目标、迁移边界与验收矩阵 | [GPUI Kit 重构计划](docs/gpui-kit-refactor-plan.md)、[视觉规范](docs/gpui-kit-visual-spec.md)、[Pencil 设计说明](docs/gpui-kit-pencil-design.md) |
| 焦点、输入与 AI 生命周期修正记录 | [2026-09-22 二次审查及修正](docs/gpui-kit-refactor-recheck-2026-09-22.md)；区分早期发现与末尾修正、未验收项 |
| 工作流语法与约束 | [工作流文档](docs/workflows.md) |
| 发布 | [发布工作流](.github/workflows/release.yml)、[版本说明](RELEASE_NOTES.md)、[安装器脚本](installer/khaslana.iss) |

文档中的旧阶段状态、测试数量、函数位置与产品路线可能落后于代码；按当前任务核对，不当成事实直接复述。算法细节与局部陷阱优先放模块注释和回归测试，专题过程写入对应文档。本文件不重复保存整份实现说明，历史内容可从 Git 历史查阅。
