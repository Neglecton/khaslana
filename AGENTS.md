# Khaslana 项目 Agent 手册

## 1. 项目定位

Khaslana 是一个使用 Rust 编写的桌面 Git 客户端，界面语言以中文为主。它基于 `gpui-ce` 和 `yororen_ui` 构建原生桌面 UI，基于 `git2` / libgit2 执行 Git 操作，并通过系统 Keyring 保存 Git 凭据。

当前项目不是简单演示应用，而是已经具备完整 Git 工作流的客户端：

- 多仓库标签页与会话恢复
- 仓库打开、克隆、刷新
- 本地/远端分支、标签、贮藏、远端管理
- 暂存、取消暂存、丢弃变更、提交
- fetch、pull、push、merge、checkout
- 提交历史、提交文件列表、历史 diff、提交图
- commit reset / revert / 撤销合并提交
- HTTPS 与 SSH 凭据管理、远端凭据绑定
- 网络代理设置，支持禁用、Git 配置/环境变量代理和自定义代理
- AI 辅助：大模型供应商配置（OpenAI Chat Completions 兼容）、commit message 生成、分支对比 AI code review
- diff 编码自动识别与手动选择，支持 UTF-8、GB18030/GBK、Big5

产品形态更接近“轻量但完整的 Git 桌面客户端”，适合继续补齐高频 Git 操作、冲突处理、搜索过滤和差异查看能力。

## 2. 技术栈

- 语言：Rust 2024 edition
- UI：`gpui-ce = 0.3`
- Git：`git2 = 0.21`，启用 `https` 和 `ssh`
- 凭据：`keyring = 4`、`keyring-core = 1`
- 异步/事件：`async-channel` + `std::thread`
- 序列化：`serde`、`serde_json`
- 错误：`thiserror`
- 编码检测：`chardetng`、`encoding_rs`
- 系统目录：`directories`
- 文件对话框：`rfd`
- 日志：`tracing`、`tracing-subscriber`
- Windows 资源：`embed-resource`，通过 `build.rs` 嵌入 `assets/app.ico`

## 3. 目录和文件职责

- `Cargo.toml`：包元信息、依赖和构建依赖。
- `build.rs`：Windows 下嵌入应用图标资源。
- `assets/app.ico`：应用图标。
- `assets/icons/`：应用内自绘矢量图标，当前用于顶部操作栏和工作流入口，通过 `src/assets.rs` 嵌入到 GPUI asset source。
- `assets/windows/app.rc`：Windows 资源脚本。
- `logo.png`：项目 logo，目前未被源码直接引用。
- `src/lib.rs`：库入口，重新导出 Git、凭据和类型模块，供 `main.rs` 使用。
- `src/assets.rs`：应用自有静态资源入口，将 `assets/icons/` 与 Yororen 内置资源合并注册给 GPUI。
- `src/types.rs`：领域类型和错误类型的汇总入口；较独立的领域类型放到 `src/types/` 子目录，例如冲突解决类型在 `src/types/conflicts.rs`。
- `src/types/browse.rs`：分支浏览和分支比较模式领域类型，包括 `BrowseTarget`、`BrowseListMode`、`BrowseEntry`、`BrowseEntryKind`、`BrowseCompareFile` 和 `BrowseFileContent`。
- `src/git.rs`：核心 Git 服务层的汇总入口；大型或独立 Git 能力放到 `src/git/` 子目录，例如冲突解决服务在 `src/git/conflicts.rs`，贮藏服务在 `src/git/stash.rs`，变基服务在 `src/git/rebase.rs`。
- `src/git/submodule.rs`：子模块 Git 服务，包括状态读取、同步父仓库记录版本、快进到子模块远端最新以及递归子模块更新。
- `src/git/rebase.rs`：变基 Git 服务，包括 `rebase_branch`、`rebase_continue`、`rebase_skip`、`rebase_abort` 和 `pull_branch_rebase`。
- `src/git/browse.rs`：分支浏览/比较 Git 服务，包括引用解析（`resolve_browse_target`）、文件树遍历（`browse_tree_entries`）、差异文件列表（`browse_compare_files`）、文件内容读取（`browse_file_content`）和与 HEAD 差异（`browse_file_diff`）。
- `src/credentials.rs`：凭据存储、匹配、Keyring 读写、凭据测试、旧存储兼容迁移和单元测试。
- `src/ssh_credentials.rs`：本机 SSH 身份发现和凭据表单辅助，包括扫描 `~/.ssh` 私钥、解析 SSH config 的 `IdentityFile`、检测 SSH Agent 已加载身份和一键填入表单。
- `src/proxy.rs`：网络代理设置类型、代理 URL 校验、远端协议到代理 URL 的选择，以及 `git2::ProxyOptions` 接入 helper。
- `src/main.rs`：应用入口与主要 UI 状态机。包含 `RepositoryView`、多标签页状态、对话框、文本输入、事件泵、异步 Git 任务、工作区视图、diff、提交框、凭据/远端弹窗等。
- `src/main.rs`：应用入口与主要 UI 状态机。包含 `RepositoryView`、多标签页状态、对话框、文本输入、事件泵、异步 Git 任务、工作区视图、diff、提交框、凭据/远端弹窗、分支浏览模式等。
- `src/conflicts/`：冲突解决相关 UI、交互动作和轻量状态 helper，作为 `main.rs` 的子模块实现 `RepositoryView` 的冲突区域。
- `src/external_merge.rs`：外部合并工具适配，目前用于检测并调用 IntelliJ IDEA 命令行 merge，负责外部合并设置类型、命令解析、从 Git index 三方内容写临时文件、等待外部工具完成并读取合并结果。
- `src/external_merge_view.rs`：外部合并工具设置弹窗，支持启用/禁用 IntelliJ IDEA 外部合并、配置 IDEA 命令路径、检测命令并在检测按钮显示成功/失败状态，以及开启选中冲突文件后自动打开 IDEA。
- `src/proxy_view.rs`：网络代理设置弹窗，包括模式切换、自定义代理输入、保存和测试代理入口。
- `src/stash_view.rs`：贮藏完整工作流 UI，包括创建贮藏、查看贮藏文件、加载贮藏 diff 和删除确认。
- `src/rebase_view.rs`：变基 UI 模块，包括变基 handler（rebase_branch/continue/skip/abort）和变基状态条渲染（继续/跳过/中止按钮）。
- `src/submodule_view.rs`：子模块弹窗 UI 和按需加载/更新动作，包括远端超前/落后状态展示、同步记录版本、更新全部到远端最新和更新单个子模块到远端最新。
- `src/ui/`：前端设计系统适配层。`theme.rs` 定义 Khaslana 运行时语义色 token、浅色/深色色板和主题感知的 `rgb` / `rgba` 入口，`components.rs` 封装按钮、toast、tooltip、section header 等项目级 UI helper，`mod.rs` 统一导出。
- `src/theme_view.rs`：应用外观设置 UI 和运行时主题切换逻辑，支持跟随系统、浅色和深色，并同步更新 Yororen 全局主题。
- `src/sidebar_view.rs`：侧边栏 UI，包括本地分支、远端、远端分支、标签、贮藏和相关右键菜单。
- `src/history_view.rs`：提交历史 UI、提交图渲染、提交文件列表、历史 diff。
- `src/diff_view.rs`：差异区域全文/紧凑视图切换模块，包括切换按钮渲染、扇出重新加载和文件过大自动回退。
- `src/operation_blocker_view.rs`：高风险后台操作的交互遮罩层 UI 和轻量状态 helper，用于切换、合并、变基、提交、回滚、子模块更新和工作流等操作期间阻断普通交互。
- `src/browse_view.rs`：分支浏览模式 UI 模块，包括文件树展平函数 `flatten_browse_tree`、文件树浏览器渲染、只读内容视图和差异视图。
- `src/browse_compare_view.rs`：分支比较模式左侧差异文件树 UI，包括把扁平差异文件构造为目录嵌套文件树的 `flatten_compare_files`、默认全展开的 `all_compare_dirs`、文件名级重命名展示、差异文件状态徽标和列表空状态。
- `src/ui_helpers.rs`：通用 UI 常量、滚动条、列表行、diff 行号、作者头像等辅助渲染。

## 4. 核心架构

### 4.1 领域层

`src/types.rs` 定义应用内部统一的数据结构：

- `RepositorySnapshot` 是 UI 的主要仓库状态输入，包含路径、HEAD、分支、变更、远端、标签、贮藏、冲突和变基进行中标记（`rebase_in_progress`）。
- `WorktreeChange` 使用 `staged` 与 `unstaged` 两个字段表达同一路径在暂存区和工作区中的不同状态。
- `FileDiff` 包含路径、范围、二进制标记、编码信息和逐行 diff。
- `CommitInfo` 表示提交历史中的一行，包含 oid、短 oid、摘要、作者、时间、父提交和 ref 标签。
- `GitError` 是统一错误出口，用户可见文案大多为中文。
- `RebaseOutcome` 表示变基操作结果，区分 `Completed(快照)` 和 `Conflicts { 快照, 当前提交序号, 总数 }`，便于 UI 层无缝接入现有冲突工作台。

新增 Git 能力时应先判断是否需要扩展领域类型，再实现 `GitService`，最后接入 UI。较大的功能不要继续塞进 `types.rs`、`git.rs` 或 `main.rs`，而是按领域拆到同名子目录，再由入口文件 `mod` / `pub use` 汇总。

### 4.2 Git 服务层

`GitService` 是业务边界。UI 不应直接散落调用 libgit2 的复杂操作，除非是非常局部、只读且已有先例。

已有能力包括：

- 仓库：`open`、`open_fast`、`clone_repo`、`snapshot`、`snapshot_after_operation`
- 子模块：状态读取、递归克隆、递归同步父仓库记录版本、快进更新到子模块远端最新
- 状态：`status_fast`、`status_full`
- 分支：创建、删除、重命名、checkout、远端分支 checkout、merge、rebase
- 远端：列表、添加、更新、删除、fetch、pull、pull --rebase、push
- 标签：列表、checkout tag
- 贮藏：列表、save、apply、pop、drop、文件列表和 diff 预览
- 变更：stage、unstage、discard unstaged、discard all
- 提交：commit、commit history、commit graph、commit files、commit file diff
- 历史操作：reset、revert
- 变基：rebase_branch、rebase_continue、rebase_skip、rebase_abort、pull_branch_rebase
- diff：工作区 diff、历史 diff、编码识别
- 浏览：引用解析（分支/标签 → commit OID）、文件树遍历、差异文件列表、文件内容读取、与 HEAD 差异

Git 操作通常返回新的 `RepositorySnapshot`，让 UI 统一刷新状态。危险操作需要在 UI 层先确认。

### 4.3 UI 状态层

`RepositoryView` 是主状态容器，维护：

- 多仓库标签：`tabs`、`active_tab`、`RepoTabState`
- 每个仓库的快照、选中分支/远端、变更选择、diff、历史列表和历史 diff
- 对话框和右键菜单状态
- 凭据弹窗、凭据管理器、远端凭据策略
- 手写文本输入状态 `TextEditState` / `TextFieldState`
- 滚动条和分栏 resize 状态
- 异步任务队列和 UI 事件通道

`RepoTabState` 是每个仓库标签页的状态。新增 per-repository UI 状态时，应优先放入 `RepoTabState`，避免全局状态污染多仓库标签。

### 4.4 异步与事件流

UI 线程通过 `async-channel` 接收后台线程发回的 `UiEvent`。重型 Git 操作应继续沿用现有模式：

1. UI 方法收集当前 tab、repo path、参数。
2. 设置 busy/loading/status。
3. 后台线程打开仓库并调用 `GitService`。
4. 通过 `UiEvent` 返回成功快照、diff、历史数据或错误。
5. UI 处理事件，更新对应 tab。

仓库加载有并发限制：

- `MAX_CONCURRENT_REPO_LOADS = 2`

后台阻塞任务统一通过 `src/tasks.rs` 的 `TaskExecutor` 调度，短任务池用于打开、刷新、状态、历史和 diff 等本地查询，长任务池用于 clone、fetch、pull、push、子模块远端检查和工作流等可能阻塞网络或凭据回调的操作。新增后台 Git / IO 任务不要直接散落 `thread::spawn`，文件选择对话框、UI tick 和测试线程除外。

历史分页大小：

- `HISTORY_PAGE_SIZE = 50`

历史分页会复用当前 tab 内存中的 refs/tags 映射缓存，仓库刷新、切换仓库或清理历史时失效。

超大 diff 缓存保护：

- `LARGE_DIFF_CACHE_LINE_LIMIT = 20_000`

diff 自动编码检测使用有限字节样本，UI 对最近查看的工作区 / 历史 / 贮藏 diff 使用有界 LRU 内存缓存；缓存不持久化，编码偏好变化或仓库加载代际变化时自然失效。

全文差异视图常量（在 `src/git.rs` 中定义）：

- `FULL_FILE_CONTEXT_LINES = 10_000_000`：全文视图拉满 diff 上下文行数，让 libgit2 输出整份文件作为上下文，改动行依旧高亮。不能使用 `u32::MAX`，libgit2 会将其当作 0。
- `FULL_FILE_MAX_BYTES = 3 * 1024 * 1024`：全文视图的字节预检阈值，新旧侧文件体积超过该值则不生成全文差异，避免超大文件在分配逐行 String 时内存暴涨。
- `FULL_FILE_TOO_LARGE_MESSAGE`：全文过大时返回的错误文案，UI 据此自动回退到紧凑差异。

### 4.5 持久化数据

应用使用 `directories::ProjectDirs::from("", "", "Khaslana")` 生成配置目录，主程序持久化数据统一写入 `khaslana.sqlite3`。当前数据库保存：

- 打开过的仓库路径和当前激活仓库。
- 每个仓库的 diff 编码偏好。
- 仓库远端到凭据策略的绑定。
- 全局网络代理设置，只保存模式和代理 URL，不拆分存储代理密文。
- 外部合并工具设置，包括是否启用 IntelliJ IDEA、是否选中冲突文件后自动打开以及可选 IDEA 命令路径。
- 全局主题偏好，支持跟随系统、浅色和深色。
- 凭据记录索引等非密元数据。

凭据密文不写入 SQLite，而是通过系统 Keyring 保存。`credentials.rs` 中的密钥服务名需要保持兼容，改动时必须加迁移或回归测试。旧版 JSON 文件不由主程序兼容读取，需要迁移时使用 `cargo run --bin migrate_storage` 一次性导入。

## 5. 当前用户可见功能

### 5.1 仓库和会话

- 打开本地仓库
- 克隆远端仓库，并根据 URL 推断目录名，默认递归克隆子模块
- 多仓库标签页
- 自动保存和恢复会话
- 刷新仓库状态
- 通过子模块弹窗按需查看状态，并在弹窗打开后后台检查子模块相对远端分支的超前/落后状态
- 可手动同步父仓库记录版本，也可全量或单个快进更新到子模块远端最新

### 5.2 工作区

- 展示暂存和未暂存变更
- 单选、多选、范围选择变更
- 暂存选中、暂存全部
- 取消暂存选中、取消暂存全部
- 丢弃单个、选中或全部变更
- 查看工作区 diff
- 差异区域支持全文/紧凑切换：切换按钮位于标题栏编码按钮旁，开启后展示整份文件并保留增删行高亮
- 大 diff 使用虚拟列表渲染
- diff 头部可折叠
- diff 编码可选
- diff 区域支持左右滑动查看长行
- 全文视图对超大文件（超过 `FULL_FILE_MAX_BYTES`）自动回退到紧凑差异并提示
- 提交信息输入和 commit
- 变基进行中时在工作区顶部显示变基状态条，提供「继续变基 / 跳过此提交 / 中止」操作；冲突解决后自动复用现有冲突工作台
- 冲突工作台支持「用 IntelliJ IDEA 解决」，自动检测 `idea64` / `idea` 命令或 `KHASLANA_IDEA_PATH`，通过外部 Merge Dialog 生成结果后写回并标记解决；更多菜单的「合并工具」可持久化 IDEA 路径，并可选择在选中冲突文件时自动打开 IDEA

### 5.3 分支、远端、标签、贮藏

- 本地分支列表、创建、删除、重命名、切换
- 远端分支列表，checkout 后创建/复用本地跟踪分支
- 远端列表、选择、添加、编辑、删除
- fetch、pull、push
- pull 对话框提供「用变基代替合并」勾选框，默认不勾选，勾选后执行 pull --rebase
- 全局刷新仅刷新本地状态；远端刷新通过工具栏”获取”或远端列表右键”刷新”显式触发
- 设置/修改本地分支 upstream
- 本地分支显示相对 upstream 的待推送/待拉取提交数
- 本地分支右键可单独拉取；非当前分支仅在可快进时直接更新
- 删除远端分支，右键复制远端分支名称和 checkout 命令
- 分支右键「变基到当前分支」，将选中分支的提交变基到当前分支之上
- tag 列表和 checkout tag
- stash 列表、创建、apply、pop、drop、文件列表和 diff 预览

### 5.4 历史

- 当前分支 / 所有分支提交历史
- 拓扑排序提交图
- 提交引用标签，包括本地分支、远端分支、tag、HEAD
- 分页加载更多
- 查看提交文件列表
- 查看指定提交文件 diff
- 右键提交可复制 SHA、reset、revert、撤销合并提交等

### 5.5 分支浏览

- 不切换分支查看其他分支/标签的完整代码
- 从侧边栏本地分支、远端分支和标签的右键菜单进入「浏览此分支 / 浏览此标签」
- 从侧边栏本地分支和远端分支右键进入「与当前分支比较」，不切换分支列出目标分支相对当前 HEAD 有差异的文件
- 左侧文件树浏览器：可展开/折叠的目录树，按目录懒加载
- 右侧默认显示目标分支上文件的只读原始内容（含行号和编码识别）
- 顶部可一键切换到「与当前 HEAD 的差异」视图
- 分支比较模式左侧以目录嵌套文件树展示差异文件（默认全部展开，可折叠），右侧同样支持目标分支全文内容和与当前 HEAD 的差异视图
- 支持切换 diff 编码
- 整个过程不执行 checkout，不改动工作区
- 二进制文件提示「无法预览」，超大文件自动报错提示
- 子模块条目仅展示，不可下钻

### 5.6 凭据

- HTTPS 用户名 + 密码/PAT
- SSH key + passphrase
- 可使用 SSH agent
- 新增 SSH 凭据时自动检测 `~/.ssh`、SSH config 和 SSH Agent，可一键选择已有身份，也可通过文件框手动选择私钥
- 凭据保存到系统 Keyring
- 凭据记录管理、删除、测试连接
- SSH 凭据测试优先使用 libgit2；Windows 下遇到 OpenSSH 私钥兼容问题时，使用系统 Git/OpenSSH 严格校验 `known_hosts` 并复核所选私钥
- 远端凭据策略：自动匹配、不使用凭据、绑定指定记录

### 5.7 网络代理

- 全局代理设置：不使用代理、使用系统代理、自定义代理
- “使用系统代理”基于 libgit2 的 `GIT_PROXY_AUTO`，读取 Git 代理配置和 `http_proxy` / `https_proxy` 环境变量，不读取系统 UI 代理或 PAC
- 自定义代理支持 HTTP、HTTPS、SOCKS5 URL；代理认证第一版写在 URL 中
- clone、fetch、pull、push、删除远端分支和工作流远端步骤共用同一代理策略

### 5.8 外观主题

- 支持跟随系统、浅色和深色三种主题模式
- 主题可在“外观”设置中即时切换并持久化
- 跟随系统模式会响应操作系统窗口外观变化
- Khaslana 语义色、自绘输入框和 Yororen 组件使用一致的深浅色模式

## 6. 开发命令

常用命令：

```powershell
cargo fmt
cargo test
cargo run
```

可选性能日志：

```powershell
$env:KHASLANA_PERF_LOG='1'
cargo run
```

检查目标平台资源时：

```powershell
cargo build
```

Windows MSVC target 通过 `.cargo/config.toml` 启用静态 CRT 链接，发布 `khaslana.exe` 时优先避免依赖目标机器已安装 VC++ 运行库。

## 7. 测试现状

项目已有较多单元测试，重点覆盖：

- `src/git.rs`：Git 操作、分支、远端、stage/unstage/discard、提交、历史、reset/revert、编码、冲突保护等。
- `src/credentials.rs`：凭据匹配、Keyring/内存存储逻辑、URL 规范化、记录排序、兼容性判断等。
- `src/main.rs`：会话 JSON、路径去重、编码偏好、远端凭据绑定、克隆路径推断、文本输入状态、diff 渲染模型、分支浏览状态切换与缓存清理等。
- `src/git/browse.rs`：分支浏览引用解析（本地/远端分支、标签）、文件树遍历、文件内容读取（编码检测与二进制判定）、与 HEAD 差异，以及子模块条目识别等基于 `tempfile` 的仓库级单测。
- `src/browse_view.rs`：文件树展平纯函数 `flatten_browse_tree`（展开/折叠/嵌套）单测。

新增 Git 业务能力时，优先在 `src/git.rs` 增加基于 `tempfile` 的仓库级单元测试。新增纯 UI 状态逻辑时，优先拆成可测试的小函数，放在 `main.rs` 或对应 view 模块的 `#[cfg(test)]` 中测试。

## 8. 编码和设计约定

- 代码修改要有中文注释，完成后应当检查`AGENTS.md`内容是否需要调整。
- 用户可见文案保持中文。
- Git 业务能力优先放在 `GitService`。
- UI 只负责状态、交互、确认和渲染，避免把复杂 Git 流程直接写进渲染函数。
- 前端通用视觉逻辑放入 `src/ui/`：颜色、边框、状态色、hover/disabled token 放 `src/ui/theme.rs`；可复用控件和 Yororen/GPUI 桥接 helper 放 `src/ui/components.rs`；view 文件只组合业务布局。
- 新增或改造 UI 时优先使用 `src/ui/theme.rs` 的语义 token，例如 `SURFACE`、`BORDER`、`TEXT_MUTED`、`ACCENT`、`DANGER`，不要在业务 view 中新增零散十六进制色值。
- 业务 UI 的语义色必须通过 `src/ui/theme.rs` 导出的主题感知 `rgb` / `rgba` 转换，不能直接调用 GPUI 同名函数解析主题 token。
- 主界面、弹框和输入框外壳应优先复用 `src/ui/components.rs` 的项目级 helper，例如 `app_panel`、`dialog_panel`、`dialog_overlay`、`input_frame`、`segmented_button`、`list_row_surface`、`status_pill`。业务 view 不应重复实现这些通用外壳。
- 反馈、toast、错误提示和加载进度必须走 `src/ui/components.rs` 的项目级 helper，例如 `feedback_bubble`、`feedback_stack`、`inline_error_bubble`、`bottom_progress_bar`；操作状态文字只在底部状态栏展示，不再叠加悬浮加载框。业务 view 不应直接使用 Yororen 默认 `notification_host` 或另写零散提示样式。
- 按钮默认不为 enabled 状态显示 tooltip；只有禁用原因或特殊风险说明才显示提示文字。点击反馈应写入项目级反馈队列，轻量提示放左下角，失败/冲突/凭据等重要提示放右下角。
- 自绘输入框的编辑、IME、选区和光标逻辑保留在 `src/text_input.rs`，但颜色必须来自 `src/ui/theme.rs`，不要在输入框绘制代码里硬编码色值。
- v4 之后业务 view 禁止新增 `COLOR_*` 引用；`main.rs`、`sidebar_view.rs`、`history_view.rs`、`workflow_view.rs`、`text_input.rs` 和 `src/conflicts/` 应直接使用 `ui::theme` 或 `src/ui/components.rs`。
- `ui_helpers.rs` 中旧 `COLOR_*` 兼容导出只允许底层 helper 内部过渡使用，不能作为新 UI 代码的导入来源。
- 顶层大文件只保留共享骨架和模块汇总。新增领域功能时按层拆分到文件夹：领域类型放 `src/types/<feature>.rs`，Git 服务放 `src/git/<feature>.rs`，UI 放 `src/<feature>/mod.rs` 或对应 view 模块。
- 子模块可以用 `impl RepositoryView` 或 `impl GitService` 扩展既有类型；入口文件只通过一行调用接入，避免把完整功能实现写回 `main.rs`。
- 每个仓库独有状态放入 `RepoTabState`。
- 跨仓库或全局偏好放入 `RepositoryView`。
- 危险操作必须有确认弹窗，例如 hard reset、discard、delete remote 等。
- 后台任务必须用 `UiEvent` 回到 UI，不要在 UI 线程执行重型 Git 操作。
- 文件路径传给 Git 前尽量使用 `Path` / `PathBuf`，展示时再转字符串。
- 远端、分支名、URL 等输入要复用或补充验证函数。
- diff 相关功能要考虑编码、二进制文件、大文件和虚拟列表。
- 凭据逻辑要避免把 secret 写入普通配置文件或日志。
- 代理设置不要把代理 secret 拆分写入普通配置；如需认证，第一版只接受用户写在代理 URL 中。
- 子模块的克隆和更新必须复用现有凭据回调和代理策略，不能绕开 `GitService` 直接使用裸 libgit2 默认网络选项。
- 右键菜单和弹窗位置应复用现有菜单定位/对话框样式。

## 9. 已知风险和维护重点

### 9.1 `src/main.rs` 过大

`src/main.rs` 目前承担入口、状态机、文本输入、异步任务、弹窗和大部分 UI。后续新增较大功能时，建议顺手按领域拆分，例如：

- `worktree_view.rs`
- `dialogs.rs`
- `remote_view.rs`
- `text_input.rs`
- `app_state.rs`

拆分要小步进行，避免和功能开发混成大重构。

### 9.2 冲突处理需要持续完善

底层能识别 `conflicts`，部分危险操作会拒绝冲突文件，UI 已有冲突工作台、三栏文本预览、块级接受/忽略、应用草稿、标记解决和 IntelliJ IDEA 外部合并流程。文本冲突视图使用虚拟列表渲染，避免几千行冲突文件卡顿。变基冲突复用同一套冲突工作台：`RebaseOutcome::Conflicts` 转换为 `Err(GitError::Conflicts(...))` 后由 `with_repo` 自动展示冲突工作台，解决后通过变基状态条继续。后续仍需继续完善更细粒度编辑体验、复杂冲突类型和外部编辑器协作。

### 9.3 历史探索能力仍偏基础

已有提交图、分页、文件 diff，但缺少搜索、过滤、按文件历史、按作者过滤、提交详情等高频能力。

### 9.4 大仓库性能需要持续关注

已经有 `open_fast`、加载队列、分页历史和大 diff 缓存保护。新增功能时要避免一次性扫描所有 refs、所有文件或完整历史。

### 9.5 UI 自动化测试缺失

当前测试主要是单元层。GPUI 桌面 UI 的端到端自动化较难，但新增复杂交互时至少应把状态计算逻辑拆出来测。

## 10. 推荐的新功能路线

### P0：冲突解决中心后续增强

理由：项目已经支持 pull、merge、revert、discard，并且已有冲突工作台。后续重点是把现有冲突闭环打磨到更强的编辑和审阅体验。

已完成基础范围：

- 在工作区顶部展示冲突状态入口。
- 单独列出冲突文件。
- 为冲突文件提供“使用当前版本 / 使用传入版本 / 标记为已解决 / 打开文件所在目录”等操作。
- 对文本冲突提供三段式预览：ours、theirs、base 或至少冲突标记高亮。
- 大冲突文本使用虚拟列表渲染，避免全量行元素和隐藏全文编辑器导致卡顿。

建议后续范围：

- 更细粒度的块内编辑或外部编辑器协作。
- 冲突未解决时禁用 commit 之外的危险操作，或给出明确提示。

实现提示：

- `GitService` 已有 `conflicts(repo)` 和冲突保护逻辑，可先扩展为冲突文件状态查询。
- UI 可先在 `RepoTabState.snapshot.conflicts` 基础上做最小闭环。
- 测试重点放在 merge/revert 产生冲突、标记解决后的状态变化。

### P1：文件历史和 blame

理由：当前历史页已经有 commit graph、commit files 和 commit file diff，继续扩展到“选中文件的历史”非常顺手，而且是 Git 客户端高频需求。

建议范围：

- 在工作区变更文件和历史文件右键菜单增加“查看文件历史”。
- 历史页增加文件路径过滤模式。
- 显示某文件相关提交列表和该文件在每次提交中的 diff。
- 后续再加 blame 视图。

实现提示：

- 在 `GitService` 增加按 path 过滤的 revwalk/diff 查询。
- UI 可复用 `HistoryScope` 思路扩展为 `HistoryFilter`。
- 注意 rename 跟踪可以后续迭代，第一版先做当前路径历史。

### P2：提交历史搜索和过滤

理由：历史页已有分页和图形渲染，但仓库稍大时缺少定位能力。

建议范围：

- 按提交信息搜索。
- 按作者过滤。
- 按分支 / tag / remote ref 过滤。
- 快捷清除过滤。

实现提示：

- 第一版可以仅过滤已加载 commits，成本低。
- 第二版再下沉到 `GitService`，在 revwalk 过程中过滤并分页。

### P2：提交详情面板

理由：当前提交行信息较紧凑，选中提交后主要看文件和 diff，缺少完整详情。

建议范围：

- 显示完整 SHA、父提交、作者、提交者、时间、完整 message。
- 支持复制字段。
- merge commit 显示父提交关系。

实现提示：

- 可扩展 `CommitInfo` 或新增 `CommitDetails`。
- UI 可在历史 diff 上方增加紧凑详情区，避免新开大弹窗。

### P2：远端分支管理增强

理由：已有远端和远端分支列表，第一版已补齐 upstream 管理、远端分支删除和远端分支右键复制能力；后续重点是 push 目标选择的持续优化和 ahead/behind 展示。

已完成第一版：

- 设置/修改本地分支 upstream。
- 本地分支显示相对 upstream 的 ahead/behind 数量。
- 本地分支右键单独拉取，非当前分支采用安全快进策略。
- 删除远端分支。
- 远端分支右键复制名称、复制 checkout 命令。

建议后续范围：

- push 时可选择远端和目标分支的体验继续打磨。

实现提示：

- ahead/behind 对侧边栏和工具栏很有价值，但计算要注意性能。
- 删除远端分支是危险操作，需要确认；当前实现已通过确认弹窗执行。

### P3：差异查看增强

理由：现有 diff 已可用，但开发者日常需要更强的审阅体验。

建议范围：

- 行内 word diff。
- 文件内搜索。
- 忽略空白差异开关。
- 二进制文件信息展示。
- 图片 diff 预览。

实现提示：

- word diff 可以先在 UI 层处理 `DiffLine` 内容。
- 忽略空白需要下沉到 `DiffOptions`。
- 图片 diff 可先只显示 before/after 基础预览。

## 11. 建议的下一步

最建议先做“冲突解决中心”。它和现有能力衔接最紧：当前应用已经能触发会产生冲突的操作，也已经能识别冲突，但用户缺少解决冲突的完整路径。补上这个功能后，Khaslana 从“能执行 Git 操作”会更接近“能陪用户走完真实 Git 工作流”。

一个务实的第一阶段可以这样切：

1. 在工作区变更面板顶部增加冲突摘要。
2. 冲突文件单独分组展示。
3. 支持对单个冲突文件执行“标记已解决”和“打开所在目录”。
4. 为 conflicted 文件增加专门 diff/文本预览提示。
5. 增加 `GitService` 单元测试，覆盖冲突检测和标记解决后的快照变化。

这条路线改动范围可控，又能明显提升产品完成度。

