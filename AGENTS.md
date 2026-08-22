# Khaslana 项目 Agent 手册

## 1. 项目定位

Khaslana 是一个使用 Rust 编写的桌面 Git 客户端，界面语言以中文为主。它基于 `gpui-ce` 和 `yororen_ui` 构建原生桌面 UI，基于 `git2` / libgit2 执行 Git 操作，并通过系统 Keyring 保存 Git 凭据。

当前项目不是简单演示应用，而是已经具备完整 Git 工作流的客户端：

- 多仓库并存（仓库切换下拉）与会话恢复
- 仓库打开、克隆、刷新
- 本地/远端分支、标签、贮藏、远端管理
- 暂存、取消暂存、丢弃变更、提交、修补提交（amend）、拣选提交（cherry-pick）
- fetch、pull、push、merge、checkout；普通合并采用 IDEA 风格闭环，冲突后可完成或中止
- 提交历史、提交文件列表、历史 diff、提交图
- 文件历史（历史页按路径过滤，只显示改动过该文件的提交）与文件追溯（blame，UI 术语统一为「追溯」：逐行归属标注的独立视图，支持工作区未提交行标注与编码切换）
- commit reset / revert / 撤销合并提交
- HTTPS 与 SSH 凭据管理、远端凭据绑定
- 网络代理设置，支持禁用、Git 配置/环境变量代理和自定义代理
- AI 辅助：大模型供应商配置（OpenAI Chat Completions 兼容）、commit message 生成、分支对比 AI code review（Diff-first Agentic：覆盖比较列表全部差异文件而非仅选中文件，初始输入只含变更文件清单 + 预算内 diff，模型按需调用 `read_lines`/`read_diff`/`get_file_tree`/`get_file_history`/`get_blame`/`search_code` 六个内置工具深入仓库代码；轮次 120/工具总次数 120/单结果 8K/累计结果 400K 字符守卫（轮数 ≤ 调用数+1，轮次线不先于调用线触顶；累计体积按 ~200K token 上下文估算；初始 diff 30K 预算超限后逐文件截 4K 且总量二次封顶，耗尽后降级为仅清单 + read_diff 引导；单轮批量 tool_calls 逐个检查额度，超限回填「预算已用尽」tool 消息不执行），触顶后强制收尾轮先注入 user 指令「预算已用尽（指明具体限额），立即输出最终评审」，并省略 tools 字段；收尾轮模型仍吐 tool_calls 时正文非空则宽容接受为结论，空正文才报错且文案指明触顶限额；每一轮均走流式 SSE（超时为「读空闲」语义而非整体限时，长思维链只要持续出数据不超时；**单轮流式瞬态失败自动重试**：不含首次最多再试 3 次、指数退避，可重试 = 网络类错误（Io/DNS/连接失败/连接被重置等非 StatusCode 变体，wildcard 兜底含 `#[non_exhaustive]` 未来变体）/408/429/5xx/流读取失败/流中 error 事件/截断（`finish_reason=length` 触及 max_tokens 上限 **或** EOF 无 `[DONE]`——两者无论已产出多少正文都判不完整，放行会出现「评审看似完成实为半句话且无报错」；空回合另按 0 块/仅思考区分文案；length 型对同一请求基本确定性，重试上限单独压到 1 次），URL/代理配置类与 400/404/422 不重试直接失败；重试提示走 Progress 事件并复位上一尝试的 live 流式文本；SSE 行解析接受 `data:` 后无空格（SSE 规范允许，跳过会丢 tool_calls 分片损坏参数 JSON），配置类 4xx 不重试直接失败；重试提示走 Progress 事件并复位上一尝试的 live 流式文本），评审 agent 跑在**独立 ai 线程池**（`TaskKind::Ai`，3 线程对齐并发上限，不占 long 池饿死 fetch/push），流式增量实时回传 UI——思维链在时间线尾部 live 区「思考中…」边生成边显示，轮次落定后折叠为「思考：{首行摘要}」正式行，中间轮 assistant 的非思考链正文以 Message 步骤**不折叠整段直出**（不被下一个工具消息覆盖），最终正文边生成边按 Markdown 渲染、完成定格（支持「复制结论」一键复制）；工具调用轨迹 Codex/ZCode 式时间线展示，生成中可收起到底部条（进度 + 取消）；**任务生命周期**：事件携带代际，切换比较目标/退出浏览只分离显示（任务后台继续执行直到完成并由任务线程落盘，完成后 toast 提示），「取消」置位标志在轮次边界退出（不落盘不提示失败），同时进行的任务上限 3 个（含后台分离的，超出阻止新开并提示）；**评审记录本地持久化**：完成后写 `<数据目录>/ai-reviews/<repo哈希8>/<毫秒>.json`（FNV-1a 哈希、按仓库保留最近 30 条、坏文件跳过），「历史」弹窗列出最近 20 条（时间/目标/模型/步骤数），点击载入面板展示（历史标签替代完成文案）；不支持工具调用的端点按 HTTP 400/404/422 直接报错引导更换供应商（400 文案双因：模型名/参数错误 或 不支持工具调用），不做兼容降级）、冲突工作台 AI 合并建议（diff3 文本喂 LLM 生成合并草稿：整文件单请求优先，超限按冲突块边界分段逐段生成并携带滑动窗口对话历史，拼接整份文件一次性回填草稿区）。生成结果空正文（完全为空/纯空白/仅返回思考过程）按错误处理并经 `validate_generated_content` 给出区分文案；AI 请求失败在状态栏与右下角 toast 双通道展示；SSE 流解析失败行会计数并记 warn 日志，0 有效块与「有块但无内容」错误文案不同
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
- 语法高亮：`syntect 5`（`default-fancy` feature，纯 Rust fancy-regex 免 oniguruma C 依赖），内置语法/主题 dump，追溯、浏览内容、冲突工作台与差异全文视图共用
- Markdown 解析：`pulldown-cmark 13`（`default-features = false`，只用 parser，关掉默认 html 渲染器/getopts），AI 评审结果的富文本展示使用
- 系统目录：`directories`
- 文件对话框：`rfd`
- 日志：`tracing`、`tracing-subscriber`
- HTTP 客户端：`ureq 3`（http-crate 风格 API，启用 `json` + `socks-proxy` feature；`socks-proxy` 缺失时 SOCKS5 代理会静默退化为直连，不可移除）。代理 URL 解析失败时各调用方报错而非静默直连；未配置代理时显式传 `None` 关闭 ureq 3 默认的环境变量代理自动检测，保证应用内代理设置优先
- 随机数：`getrandom`，OAuth CSRF state 用 OS CSPRNG
- `yororen_ui` 暂留 `0.2` 系：`0.3` 重构为 headless + renderer 架构（`component` 模块移除、Theme 结构重排），升级需重写组件接入层
- Windows 资源：`embed-resource`，通过 `build.rs` 嵌入 `assets/app.ico`

## 3. 目录和文件职责

- `Cargo.toml`：包元信息、依赖和构建依赖。
- `build.rs`：Windows 下嵌入应用图标资源。
- `installer/khaslana.iss`：Inno Setup 7 安装器脚本（用户级安装，见 §6 发版产物说明），版本经 `/DAppVersion` 注入。
- `assets/app.ico`：应用图标。
- `assets/icons/`：应用内自绘矢量图标，当前用于顶部操作栏和工作流入口，通过 `src/assets.rs` 嵌入到 GPUI asset source。
- `assets/windows/app.rc`：Windows 资源脚本。
- `logo.png`：项目 logo，目前未被源码直接引用。
- `src/lib.rs`：库入口，重新导出 Git、凭据和类型模块，供 `main.rs` 使用。
- `src/assets.rs`：应用自有静态资源入口，将 `assets/icons/` 与 Yororen 内置资源合并注册给 GPUI。
- `src/types.rs`：领域类型和错误类型的汇总入口；较独立的领域类型放到 `src/types/` 子目录，例如冲突解决类型在 `src/types/conflicts.rs`。
- `src/types/browse.rs`：分支浏览和分支比较模式领域类型，包括 `BrowseTarget`、`BrowseListMode`、`BrowseEntry`、`BrowseEntryKind`、`BrowseCompareFile` 和 `BrowseFileContent`。
- `src/types/blame.rs`：文件追溯领域类型，包括 `BlameCommitInfo`（hunk 归属提交的 owned 展示信息）、`BlameHunkInfo`（连续同源行段，`commit: None` 表示工作区未提交行）、`BlameView`（路径 + 解码行 + hunk + `line_hunk` 行索引映射 + 实际使用的编码）。
- `src/git.rs`：核心 Git 服务层的汇总入口；大型或独立 Git 能力放到 `src/git/` 子目录，例如冲突解决服务在 `src/git/conflicts.rs`，贮藏服务在 `src/git/stash.rs`，变基服务在 `src/git/rebase.rs`。
- `src/git/submodule.rs`：子模块 Git 服务，包括状态读取、同步父仓库记录版本、快进到子模块远端最新以及递归子模块更新。
- `src/git/rebase.rs`：变基 Git 服务，包括 `rebase_branch`、`rebase_continue`、`rebase_skip`、`rebase_abort` 和 `pull_branch_rebase`。
- `src/git/worktree_compat.rs`：工作区写入兼容层。Windows 下为 checkout、merge/pull、hard reset、revert、rebase、stash 和子模块更新统一附加 `GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES`，避免编辑器占用空目录导致 Git 操作失败；其他平台保持 git2 默认行为。
- `src/git/partial_stage.rs`：按块/按行部分暂存服务（双向），含部分 patch 构造纯函数（未选中 +/- 行降级/丢弃、hunk 头重算、反向交换）与守卫、选择类型（`SelectedDiffLine`/`LineSelection`）。hunk 头重算指 post 侧 `new_start` 必须按「实际输出补丁的后镜像」坐标重算（= preimage 首行在目标初始内容中的行号 + 先前已输出块的累计净行数变化）：libgit2 的 apply 以 `new_start` 在被先前块逐步改写过的目标内容中精确定位且无偏移搜索，直接透传源 diff 原始 `new_start` 时，一旦丢弃或按行改写了前面的块（净行数变化），后续块会定位错位并报 `ApplyFail`（仅前序块无净行数变化时碰巧不错位）。
- `src/git/browse.rs`：分支浏览/比较 Git 服务，包括引用解析（`resolve_browse_target`）、文件树遍历（`browse_tree_entries`）、差异文件列表（`browse_compare_files`，三点比较 `merge_base..target`，仅列目标分支领先当前分支的提交所改动的文件）、文件内容读取（`browse_file_content`）和与 HEAD 差异（`browse_file_diff`）。
- `src/git/search.rs`：代码搜索 Git 服务（`search_code`，供 AI 评审 agent 的 `search_code` 工具使用，近似符号查找）。在指定提交的文件树里按子串/正则逐行搜索，支持可选目录前缀（`path_prefix` 定位到子树再遍历，前缀外整体剪掉；前缀不存在或指向文件**显式报中文错误**而非静默空结果——模型会把「前缀写错」误读成「标识符不存在」），返回 `CodeSearchMatch { path, lineno, line }`；守卫：扫描文件数 ≤1000（`walk_tree` 顶部按 scanned 剪枝，树遍历本身也停，不只是跳过 blob 读取；计数只统计真正付出 IO 的文件——二进制扩展名检查是纯字符串判断先于计数，否则大量二进制/生成产物会把名额吃光）、二进制扩展名兜底 + 前 8KB NUL 嗅探跳过、单 blob >1MB 先读对象头判体积不整体加载、命中行按 200 字符截断入库（无 NUL 的压缩/生成产物超长单行不再产生内存峰值）、命中达 max_results 立即剪枝；正则编译失败与空白查询返回中文错误；UTF-8 有损解码（搜索面向定位，非 UTF-8 行的替换符不影响定位价值）。
- `src/syntax.rs`：语法高亮纯函数层（lib crate）。`highlight(path, lines, dark)` 把已解码文本行映射为「行内 utf8 字节区间 + RGB」span 序列（`SyntaxSpans`，与源行严格索引对齐、相邻同色合并）；`highlight_diff_lines` 为 FileDiff 全文行做同样映射（文件头/hunk 头/EOFNL 行产空 vec）。语言检测按扩展名 → 文件名 token 兜底；守卫 >1MB 或 >20K 行返回 None（渲染侧回退纯文本）；深浅主题二选一内置主题（浅 InspiredGitHub / 深 base16-ocean.dark），`SyntaxSet`/`Theme` 经 OnceLock 全局一次初始化（首次约百毫秒，全部调用点在后台线程）。UI 侧接入模式：各视图内容落位后经 `schedule_syntax_highlight`（`SyntaxSlot` 槽位枚举）后台补算，`SyntaxHighlighted` 事件按 (Arc 地址, 行数) 身份守卫回填；冲突工作台走 `ConflictSyntaxHighlighted`（ours/theirs 只读一次、draft 随按块接受/AI 生成重算，seq 防乱序）；主题深浅切换经 `invalidate_and_refresh_syntax_highlights` 清空并从现存 Arc 补算（不做 git 重载）。渲染用 gpui `StyledText::with_highlights`（`syntax_styled_text` helper），宽度测量与既有 String 子元素同路径，横向滚动机制零改动。
- `src/markdown_view.rs`：Markdown 渲染模块（bin crate）。纯解析层 `parse_markdown_blocks` 把 Markdown 文本映射为块/行/行内 span 数据结构（`MdBlock`/`MdLine`/`MdInlineSpan`，可单测），渲染层 `render_markdown` 复用 `StyledText::with_highlights` 做行内样式（加粗/斜体/删除线/行内代码 chip：TILE 底 + PRIMARY 字）；列表扁平化为带 muted 前缀与缩进的行，引用块递归，代码块 TILE 底等宽逐行。链接/图片仅保留文字，表格/脚注不启用；流式半截输入（未闭合围栏/加粗）由解析器在 EOF 自动收尾。当前用于 AI 评审面板的正文渲染。
- `src/git/blame.rs`：文件历史与文件追溯 Git 服务。`file_history` 按路径过滤提交（libgit2 revwalk 不支持 pathspec，全量迭代 + 逐提交 first-parent tree-diff 单 pathspec 判断，分页作用于过滤后 OID 流；单 pathspec tree-diff 有剪枝，超大仓库后续再下沉缓存）；`blame_file` 基于 HEAD 计算 blame（守卫：HEAD 无该路径报中文错误、blob 超 `FULL_FILE_MAX_BYTES` 报过大、8KB NUL 嗅探报二进制），工作区文件存在且非二进制时经 `blame_buffer` 纳入未提交改动（差异行零 OID → `commit: None`）；`git_blame_buffer` 对 blob 与 buffer 做裸字节 diff、不经 core.autocrlf/gitattributes 过滤（libgit2-sys 未暴露过滤器 API），Windows 常见「工作区 CRLF、blob LF」会被判成整文件新增（全行零 OID → 全行「未提交」），故先经 `align_line_endings_to_blob` 把工作区字节按 blob 换行风格对齐：与 HEAD 一致（含换行差异）直接用 blob blame、有真实改动才进 buffer 路径、工作区被清空返回空视图（git_blame_buffer 断言 buffer 非空）。hunk 的作者/时间/摘要直接取 libgit2 填充的 final 签名与 summary，不逐提交查库。v1 不追踪 rename/copy，也不支持对任意提交版本 blame（`BlameOptions::newest_commit` 留作后续）。
- `src/credentials.rs`：凭据存储、匹配、Keyring 读写、凭据测试、旧存储兼容迁移和单元测试。
- `src/ssh_credentials.rs`：本机 SSH 身份发现和凭据表单辅助，包括扫描 `~/.ssh` 私钥、解析 SSH config 的 `IdentityFile`、检测 SSH Agent 已加载身份和一键填入表单。
- `src/oauth.rs`：OAuth 快速登录服务层（GitHub Device Flow + Gitee 授权码流）。GitHub 走设备流（设备码请求、令牌轮询含 `authorization_pending`/`slow_down`/取消/过期、用令牌换取登录名；轮询 Agent 关闭 `http_status_as_error`，因 GitHub 待定/过期返回 400 且详情在响应体里）；Gitee 走授权码流（`gitee_run_code_flow`：本地 `127.0.0.1:17890` 回调服务器用 `std::net` 手写、循环读齐请求头、校验 Host 为本机回调地址、state 用 OS CSPRNG 128 位随机 → 收 code → POST 给 broker 换 token → 取登录名，登录名请求的 token 走 `Authorization` 头而非 URL query）。纯同步 `ureq 3`，复用全局代理设置，不引入异步运行时。令牌作为 `GitCredential::UserPass` 的 secret 复用现有 Keyring 存储与 git2 认证路径，无需改动凭据数据模型。客户端不含 Gitee `client_secret`（公开分发会泄露），token 交换由部署在边缘平台的 broker 代办（见独立仓库 [khaslana-broker](https://github.com/Neglecton/khaslana-broker) 的 `edge-functions/gitee.js`）。
- Gitee OAuth 令牌交换 broker：实现位于独立仓库 [khaslana-broker](https://github.com/Neglecton/khaslana-broker)（`edge-functions/gitee.js`，部署到腾讯云 EdgeOne 边缘函数或任意 serverless 平台），持有 `GITEE_CLIENT_SECRET` 环境变量，替客户端用授权码向 Gitee 换 access_token。客户端只持 `GITEE_OAUTH_CLIENT_ID` + `GITEE_BROKER_URL`（`src/oauth.rs` 常量，二者均非空时 Gitee 登录按钮启用）。后续若新增其它 serverless 服务，也统一放进该仓库。
- `assets/icons/github_lockup_{light,dark}.svg`、`assets/icons/gitee_lockup_{light,dark}.svg`：GitHub/Gitee 带文字品牌图标，按主题选浅/深变体，用于「添加凭据」的 OAuth 快速登录按钮（`OauthBrand`，`src/ui/icons.rs`）。品牌图标用固定 fill（Gitee 为红 + 黑/白多色），GPUI 的 `svg()` 只能做单色 alpha 蒙版会丢色，故按钮通过 `img()`（`render_single_frame`）渲染以保留原始配色，由 `ui_theme::active_variant()` 决定取哪个文件。
- `src/proxy.rs`：网络代理设置类型、代理 URL 校验、远端协议到代理 URL 的选择，以及 `git2::ProxyOptions` 接入 helper。
- `src/ai/merge.rs`：AI 冲突合并建议纯函数层：diff3 文本按「上下文行/完整冲突块」原子单元分段（绝不切开冲突块、拼接逐字节恒等、纯上下文段标记透传不送模型，`split_diff3_text`）、分段请求的滑动窗口对话组装（`build_segment_messages`，预算内从新到旧保留历史回合）、响应清洗（`strip_code_fence` 剥代码块围栏，保留尾部换行配合分段拼接）与冲突标记残留检测（`response_contains_conflict_markers`，`=======` 不单判避免误伤正文）。长度阈值常量（整文件 60K / 段 24K / 滑窗 150K 字符，按 ~200K token 上下文与 3-4 字符/token 保守估算）也在此定义。
- `src/ai/review_agent.rs`：Diff-first Agentic 评审（lib crate）。工具注册表（六个内置工具的中文描述 + 手写 JSON Schema；search_code 带可选 `path_prefix` 目录限定）、预算守卫（`ToolBudget`：轮次 120 / 工具总次数 120 / 单结果 8K / 累计 400K 字符——轮数 ≤ 调用数+1 使轮次线不先于调用线触顶，累计体积按 ~200K token 上下文估算是真正的成本闸门；`limit_reason()` 命名首个触顶限额。强制收尾 = 注入 user 指令（指明限额、要求立即给结论）+ 省略 tools；收尾轮仍吐 tool_calls 时正文非空宽容接受为结论，空正文才报错并指明限额——旧版三线共用一句「超出工具调用限额」文案 + 收尾轮硬报错，造成「没到调用次数上限却报限额错误」的误伤）、初始上下文装配（变更文件清单 + 预算内 diff，总量 ≤30K 全量给、超限逐文件截 4K **且总量二次封顶**——旧版文件数 × 4K 线性膨胀（200 文件 ≈ 800K）会撑爆上下文，预算耗尽后降级为仅清单 + read_diff 引导；记账含每文件头部与截断标注的实际发出字符，总量严格不超预算）、`run_review_agent` 多轮循环（`is_cancelled: &AtomicBool` 在轮次边界检查（重试退避前与循环退出后返回前各再查一次——末轮流式恰在取消后才完成时按取消收尾不落盘），取消返回 `Ok(None)` 不算失败不触发 Done；每轮 `request_agent_stream` 流式：正文/思考链增量经 `AgentEvent::Delta` 实时回传 UI，tool_calls 分片按 index 聚合；**单轮流式请求带自动重试**（不含首次最多再试 3 次，指数退避 1s/2s/4s，退避前复查取消标志）：仅重试瞬态故障（`AgentStreamError::retryable`：网络 IO、408/429/5xx、流读取失败、流中 error 事件、无正文且无工具调用的无效回合），配置类错误（400/404/422 等）直接失败；重试提示走 `AgentEvent::Progress`（UI 侧 Progress 会清空上一轮 live 流式文本，半截思考随之复位，用户看到「响应中断（原因），正在重试第 N/3 次…」）；重试耗尽后错误附「已自动重试 N 次仍失败」；有 tool_calls 时先落 Reasoning/Message 步骤（中间轮非思考链正文以 `AiReviewStep::Message` 入时间线，UI 不折叠直出）再逐个执行回填 tool 消息——**逐个检查额度**，模型一轮批量发起时超限调用不执行、回填「工具预算已用尽（{限额}）」tool 消息（OpenAI 协议要求每个 tool_call_id 配对）；无 tool_calls 则校验正文非空结束；`AgentEvent::{Step,Progress,Delta,Done}` 回调供 UI 映射事件）与工具执行分发（read_lines 钳 400 行窗口 + 1MB 预检、read_diff 复用比较差异、get_file_tree 截 300 条、get_file_history 钳 20 条、get_blame 压缩为行段摘要 + 结果前缀注明「基于当前 HEAD 与工作区，非目标分支版本」、search_code 截 50 命中；工具失败不终止评审，错误文本作为结果回填模型）。system prompt 要求同轮批量发起独立调查、小改动可零调查直接结论、严重问题引用行号证据。`file_diff_to_patch_text`（FileDiff → patch 文本）也在此导出，commit message 生成与 read_diff 共用。流式工具协议（`request_agent_stream` 单次尝试 + `StreamingToolCallAccumulator` 按 index 聚合分片：id/name 首片、arguments 逐片追加；**截断检测**：`finish_reason=length`（触及 max_tokens，文案含上限数值）或 EOF 无 `[DONE]` 时**无论已产出多少正文/工具调用都判不完整**（可重试）——放行会出现两种静默坏结局：「思考一半就停」（半截思考链被当合法轮次）与「结论半句话却显示完成」；流中 `error` 事件同样立即失败；纯函数 `agent_stream_truncation_message`（length / 无 DONE 两路）与 `agent_turn_empty_failure_message`（0 块 / 仅思考两路）分工给出区分文案）与错误分流（`classify_agent_http_error`：408/429/5xx + 网络 IO 标记可重试，其余 4xx 沿用 `agent_request_error` 文案直接失败——HTTP 404/422 → 更换供应商提示、400 双因并提「模型名/参数错误 或 不支持工具调用」，无降级）在 `src/ai/client.rs`。评审输出 token 上限 `REVIEW_MAX_TOKENS = 8192`（reasoning 模型思考与正文共用 max_tokens 预算，4000 会让长思维链中途触顶）。
- `src/ai/review_store.rs`：评审记录本地持久化。`AiReviewRecord`（repo_path/target/model/耗时/文件数/完整 `AiReviewResult` 含轨迹）JSON 落盘到 `<数据目录>/ai-reviews/<repo哈希8>/<毫秒>.json`（repo 哈希用手写 FNV-1a 保证跨版本稳定，std DefaultHasher 不保证；哈希前先对路径做**小写折叠**——Windows 路径大小写不敏感，`D:\Repo` 与 `d:\repo` 是同一仓库，不折叠会因大小写变化让旧记录「失联」；`list_review_records` 兼容读折叠前的旧键目录，写入只走新键；同毫秒文件名加 `-2` 后缀；按仓库只保留最近 30 条，文件名毫秒前缀字典序即时间序）；`list_review_records` 按文件名倒序只解析最新 `limit` 条（单条记录可达数百 KB，不全量读入），坏 JSON 跳过继续下一条、仅 warn。数据目录经 `storage::active_data_dir()` 解析（与 DB 同一套便携/旧目录激活规则）。选 JSON 文件而非 SQLite：记录可达数百 KB，文件天然隔离、便于备份清理。写入由任务线程在生成完成后执行——UI 分离（切目标/退出浏览）不影响落盘。
- `src/main.rs`：应用入口与主要 UI 状态机。包含 `RepositoryView`、多仓库并存状态（仓库切换下拉替代标签页行）、设置中心（独立 `settings_center` 状态，与 `active_dialog` 解耦，凭据子弹窗可叠加）、对话框、文本输入、事件泵、异步 Git 任务、工作区视图、diff、提交框、凭据/远端弹窗、分支浏览模式、快捷键动作定义与分发（`ShortcutAction` 枚举 + `actions!` 宏 + `register_all_key_bindings`）等。
- `src/conflicts/`：冲突解决相关 UI、交互动作和轻量状态 helper，作为 `main.rs` 的子模块实现 `RepositoryView` 的冲突区域。
- `src/external_merge.rs`：外部合并工具适配，目前用于检测并调用 IntelliJ IDEA 命令行 merge，负责外部合并设置类型、命令解析、从 Git index 三方内容写临时文件、等待外部工具完成并读取合并结果。
- `src/external_merge_view.rs`：外部合并工具设置弹窗，支持启用/禁用 IntelliJ IDEA 外部合并、配置 IDEA 命令路径、检测命令并在检测按钮显示成功/失败状态，以及开启选中冲突文件后自动打开 IDEA。
- `src/proxy_view.rs`：网络代理设置弹窗，包括模式切换、自定义代理输入、保存和测试代理入口。
- `src/stash_view.rs`：贮藏完整工作流 UI，包括创建贮藏、查看贮藏文件、加载贮藏 diff 和删除确认。
- `src/workflow_editor.rs`：工作流模板可视化创建/编辑器。**编辑模式（v2）**：模板列表行右键菜单「编辑此模板 / 复制为副本 / 删除模板...」（`WorkflowTemplateContextMenu`，按项目惯例登记 close_popups / any_popup_menu_open / mouse_down_inside_context_menu / 根层 capture 两处链）。删除经 `DialogState::ConfirmDeleteWorkflowTemplate` 确认弹窗（danger 按钮）后同步 `fs::remove_file`；若删除的是当前加载的工作流则同时清空详情区与选中态，再刷新模板列表。反映射纯函数 `workflow_editor_data_from_definition`（领域定义 → 编辑数据，11 种步骤逐字段回填、inputs 含 description）+ 注释检测 `workflow_content_has_comments`（跳过字符串字面量扫描 `//` 与 `/* */`，字符串内 `//` 误报只多一次确认，安全方向）；原文件含注释先弹 `DialogState::ConfirmWorkflowEditComments` 确认丢失风险（暂存经 `pending_workflow_edit`，确认后 `confirm_workflow_edit_comments` 进编辑器），副本不弹（原文件不动，强制 `-copy` 后缀另存）。保存：重名校验排除自身（否则未改名保存必误报）、写盘目标经 `workflow_editor_save_target` 纯函数决策——按**文件主干**（大小写不敏感）比较而非全名（`.jsonc` 原件也原地覆盖并保留原扩展名；按全名比较会让非 `.json5` 模板每次保存都误判为改名多出一个新文件），主干不同（改名）写新路径并删除旧文件（重命名语义，写成功后才删，删除失败仅记日志不阻断）；保存成功后重定位模板目录并全量刷新列表；标题按 `editing_path` 动态显示「编辑/新建」。输入变量行支持 description 往返（`WorkflowInputPart::Description`，空→None）。分两层：纯数据层 `WorkflowEditorData`（可单测）持有字符串/布尔形态编辑数据，`build_workflow_definition`（逐项中文校验：步骤必填槽、inputs/vars 键非空/不重复/不撞 `git.`/`run.`/`date:` 保留前缀，最后复用 `validate_definition` 收口）、`workflow_editor_file_name`（非法字符/重名/后缀规范化）、`workflow_step_draft_summary`（步骤草稿摘要）与 3 个内置预设（同步当前分支/新建功能分支并推送/合并分支并推送，预设仅在步骤为空时展示防误覆盖）都在这层；UI 状态层 `WorkflowEditorState` 包一层文本框（`TextFieldState` 需要 GPUI Context 构造）。步骤文本槽跨类型复用（checkout 切 merge 分支名保留）；文本框惰性创建统一前移到 `RepositoryView::render` 顶部的 `ensure_workflow_editor_fields_inited`（`field_mut` 无 cx 不能建框，text_input paint 路径依赖字段已存在），`workflow_editor_field_ref_mut` 只寻址不创建，`workflow_editor_field_or_fallback` 自由函数隔离 `field_mut` 编辑器分支的两次可变借用。保存闭环：`sync_from_fields` -> 校验 -> `json5::to_string` 序列化 -> `parse_workflow_json5` 回读守卫 -> 写 `workflow_templates_dir()` -> `refresh_workflow_templates()` + 选中加载 + toast。步骤类型下拉按「常用 6 种在前、高级 5 种在后」排序；inputs/vars 收进「高级」折叠区。
- `src/rebase_view.rs`：变基 UI 模块，包括变基 handler（rebase_branch/continue/skip/abort）和变基状态条渲染（继续/跳过/中止按钮）。
- `src/submodule_view.rs`：子模块弹窗 UI 和按需加载/更新动作，包括远端超前/落后状态展示、同步记录版本、更新全部到远端最新和更新单个子模块到远端最新。
- `src/ui/`：前端设计系统适配层。`theme.rs` 定义 Khaslana 运行时语义色 token、浅色/深色色板和主题感知的 `rgb` / `rgba` 入口，`components.rs` 封装按钮、toast、tooltip、section header 等项目级 UI helper，`mod.rs` 统一导出。
- `src/theme_view.rs`：应用外观设置 UI 和运行时主题切换逻辑，支持跟随系统、浅色和深色、主题色更换，并同步更新 Yororen 全局主题（含聚焦边框跟随主题色）。
- `src/sidebar_view.rs`：侧边栏 UI，包括本地分支、远端、远端分支、标签、贮藏和相关右键菜单。
- `src/shortcuts_view.rs`：快捷键设置页 UI，包括 `format_keystroke`（keystroke → 显示文本）、录制态交互（按下组合键录入，冲突拒绝并提示）和恢复默认。
- `src/history_view.rs`：提交历史 UI、提交图泳道分配与可调宽度渲染、提交文件列表、历史 diff。
- `src/diff_view.rs`：差异区域全文/紧凑视图切换模块，包括切换按钮渲染、扇出重新加载和文件过大自动回退。
- `src/operation_blocker_view.rs`：高风险后台操作的交互遮罩层 UI 和轻量状态 helper，用于切换、合并、变基、提交、回滚、子模块更新和工作流等操作期间阻断普通交互。
- `src/browse_view.rs`：分支浏览模式 UI 模块，包括文件树展平函数 `flatten_browse_tree`、文件树浏览器渲染、只读内容视图和差异视图。
- `src/browse_compare_view.rs`：分支比较模式左侧差异文件树 UI，包括把扁平差异文件构造为目录嵌套文件树的 `flatten_compare_files`、默认全展开的 `all_compare_dirs`、文件名级重命名展示、差异文件状态徽标和列表空状态；虚拟化列表行数取展平可见行数 `compare_visible_row_count`（目录节点 + 文件叶子），不能用差异文件数，否则深层目录与文件叶子会因超出行数而不渲染。
- `src/blame_view.rs`：文件追溯视图 UI 模块（独立 `MainMode::Blame`，头部「关闭」返回工作区，无顶栏药丸）：三列布局（注释栏 | 行号 | 内容），**不用任何分割线**——注释栏整列铺 `DIFF_HEADER_BG` 微灰底形成「注释侧栏 | 代码区」IDE 分区，分组由「仅块首行有注释」传达；注释栏内部按固定宽度分列对齐（哈希 56px 主色等宽 / 作者 72px truncate / 日期 64px 含年份 yyyy-mm-dd / 摘要 flex truncate），栏宽 300px，悬浮注释栏显示完整提交信息（短 oid + 作者 + 精确到秒时间 + 完整摘要，作者/摘要被截断时的兜底查看入口）；行号 48px 右对齐 + 两侧内边距；未提交行整行 `COLOR_WARNING` 打底 + `COLOR_WARNING_FOREGROUND` 文字 + 「未提交」徽标且不做语法高亮，已提交行内容列带语法高亮。`uniform_list` 虚拟渲染（行高 18px、横向 Unconstrained + 最宽行测量缓存），支持双向滚动条与编码切换重载。
- `src/ui_helpers.rs`：通用 UI 常量、滚动条、列表行、diff 行号、作者头像（`author_avatar`）、仓库头像（`repo_avatar`/`repo_initials`，圆角方形首字母缩写色块）等辅助渲染。
- `src/tests/`：测试代码目录，存放所有从源文件通过 `#[path]` 属性外移的单元测试模块。目录结构映射源文件结构，例如 `src/tests/git/browse.rs` 对应 `src/git/browse.rs` 的测试、`src/tests/ai/client.rs` 对应 `src/ai/client.rs` 的测试。外移的测试模块通过 `use super::*` 仍可访问源文件的私有项。
- `src/git/test_support.rs`：Git 测试共享辅助模块，提供 `service()`、`init_repo()`、`configure_user()`、`write_file()`、`write_bytes()`、`assert_file_text()`、`commit_all()`、`path_url()` 等公共 fixture 函数，供 `git`、`workflow` 等模块的测试复用，消除各 `mod tests` 中的重复定义。

## 4. 核心架构

### 4.1 领域层

`src/types.rs` 定义应用内部统一的数据结构：

- `RepositorySnapshot` 是 UI 的主要仓库状态输入，包含路径、HEAD、分支、变更、远端、标签、贮藏、冲突、普通合并状态（`merge_in_progress` / `merge_message`）和变基进行中标记（`rebase_in_progress`）。
- `WorktreeChange` 使用 `staged` 与 `unstaged` 两个字段表达同一路径在暂存区和工作区中的不同状态。
- `FileDiff` 包含路径、范围、二进制标记、未跟踪标记、编码信息和逐行 diff。
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
- 分支：创建、删除、重命名、checkout、远端分支 checkout、merge、完成/中止冲突合并、rebase
- 远端：列表、添加、更新、删除、fetch、pull、pull --rebase、push
- 标签：列表、checkout tag
- 贮藏：列表、save、apply、pop、drop、文件列表和 diff 预览
- 变更：stage、unstage、discard unstaged、discard all
- 部分暂存：stage_lines / unstage_lines（`src/git/partial_stage.rs`，按块/按行双向）。以 `diff.print` 原始字节重建部分 patch，经 `Diff::from_buffer` + `repo.apply(ApplyLocation::Index)` 应用（等价 `git apply --cached`，不触工作区）；取消暂存走文本反转（git2 0.21 未暴露 reverse 标志）。选择以行号定位（Added=new_lineno / Removed=old_lineno），服务端重生成 diff 为权威。输出 hunk 头 post 侧 `new_start` 按输出补丁后镜像坐标重算（libgit2 apply 用其精确定位且无偏移搜索，丢弃/改写前序块后原始坐标会错位报 ApplyFail）。守卫：冲突/二进制/整文件增删改名拒绝部分操作；含无尾换行标记（EOFNL）的块拒绝按行部分选择（可整块）
- 提交：commit、amend（保留原作者/父提交，经手动前移分支引用绕过 git_commit_create 的首父校验）、cherry-pick（保留原作者与提交信息，冲突进现有闭环，空结果拒绝）、commit history、commit graph、commit files、commit file diff
- 历史操作：reset、revert
- 变基：rebase_branch、rebase_continue、rebase_skip、rebase_abort、pull_branch_rebase
- diff：工作区 diff、历史 diff、编码识别
- 浏览：引用解析（分支/标签 → commit OID）、文件树遍历、差异文件列表、文件内容读取、与 HEAD 差异

Git 操作通常返回新的 `RepositorySnapshot`，让 UI 统一刷新状态。危险操作需要在 UI 层先确认。

### 4.3 UI 状态层

`RepositoryView` 是主状态容器，维护：

- 多仓库并存：`tabs`、`active_tab`、`RepoTabState`；仓库切换下拉（IDEA 式三区：功能区/打开项目/最近项目）替代标签页行
- 设置中心：`settings_center: Option<SettingsCategory>`，独立于 `active_dialog`，凭据子弹窗可叠加
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

后台阻塞任务统一通过 `src/tasks.rs` 的 `TaskExecutor` 调度：短任务池（2-4 线程）用于打开、刷新、状态、历史和 diff 等本地查询，长任务池（2 线程）用于 clone、fetch、pull、push、子模块远端检查和工作流等可能阻塞网络或凭据回调的操作，ai 池专供分钟级评审 agent——它若复用 long 池，两个并发评审就会把网络 Git 操作饿死在队列里；线程数直接引用 `MAX_CONCURRENT_AI_REVIEWS` 常量（不双写，脱钩会让任务在 rayon 队列排队）。评审任务体自带第二层 `catch_unwind`：panic 补发该代际 `AiReviewFailed` 精确归位并发计数与加载标志（全局 `BackgroundTaskPanicked` 无法区分任务种类，不碰评审状态）。任务体统一包一层 `catch_unwind`：rayon 会静默吞掉任务 panic，若不兜底，对应 tab 的 busy/加载标志和仓库加载槽位会永久卡死；panic 时发 `UiEvent::BackgroundTaskPanicked` 由 UI 统一复位并提示。新增后台 Git / IO 任务不要直接散落 `thread::spawn`，文件选择对话框、UI tick 和测试线程除外。

历史分页大小：

- `HISTORY_PAGE_SIZE = 50`

历史分页（append）会复用当前 tab 内存中的 refs/tags 映射缓存；全量刷新（append=false）不复用、总是重建 refs，保证切换分支、提交等操作后 HEAD/分支/标签徽章与最新仓库状态一致。

提交历史自动刷新策略（不需要人工点击刷新）：

- 会创建/移动提交或 HEAD 的操作（提交、提交并推送、合并、变基、reset、revert、uncommit，见 `operation_affects_commit_history` 消息名单）完成后，经 `OperationFinished` 调用 `reload_history_after_change`：正在查看历史页或已有历史列表时无论当前视图都立即后台重载；历史列表为空且不在历史页时不预加载，等进入历史页再拉。
- 引用类操作（切换分支、拉取、推送、分支增删改等，见 `operation_requires_repository_refresh`）走完整仓库重载，由 `RepositoryFastLoaded` 统一调用 `reload_history_after_change` 刷新历史。
- `ensure_history_loaded` 在进入历史页时若历史被标记陈旧（`history_refreshing`）也会重新拉取，兜底失败重试与跨标签页陈旧；刷新期间旧列表保持可见（stale-while-revalidate）。
- 刷新期间选中提交的文件列表与差异也保留展示（按 commit oid 不可变，与提交列表同一套 stale-while-revalidate 策略）：`refresh_history` 不清空 `history_files`/`history_selected_file`/`history_diff`；`HistoryCommitsLoaded` 全量替换时选中仍在新列表则保留三区展示（文件为空时经 `select_history_commit` 自愈重载，非空幂等跳过），选中被新列表丢弃才连同清空——避免出现"详情显示选中提交、文件/差异永远空占位"的不一致。
- `RepoTabState.history_load_seq` 是提交列表请求序号，每次 `load_history_page` 递增并随 `HistoryCommitsLoaded` 事件回传：仅最新一代请求（seq 匹配）能应用数据和复位加载标志，旧一代晚到的结果被丢弃；`HistoryLoadFailed` 无论 load_id 是否匹配都复位加载标志，避免操作作废在飞请求后加载标志永久卡死、后续历史加载被静默吞掉。

超大 diff 缓存保护：

- `LARGE_DIFF_CACHE_LINE_LIMIT = 20_000`

diff 自动编码检测使用有限字节样本，UI 对最近查看的工作区 / 历史 / 贮藏 diff 使用有界 LRU 内存缓存；缓存不持久化，编码偏好变化或仓库加载代际变化时自然失效。

全文差异视图常量（在 `src/git.rs` 中定义）：

- `FULL_FILE_CONTEXT_LINES = 10_000_000`：全文视图拉满 diff 上下文行数，让 libgit2 输出整份文件作为上下文，改动行依旧高亮。不能使用 `u32::MAX`，libgit2 会将其当作 0。
- `FULL_FILE_MAX_BYTES = 3 * 1024 * 1024`：全文视图的字节预检阈值，新旧侧文件体积超过该值则不生成全文差异，避免超大文件在分配逐行 String 时内存暴涨。预检经 `delta_side_size` 读 ODB 对象头取真实大小——树对树 diff（历史/贮藏/分支比较）的 delta 自带 size 恒为 0，直接用会让预检形同虚设。
- `FULL_FILE_TOO_LARGE_MESSAGE`：全文过大时返回的错误文案，UI 据此自动回退到紧凑差异。

### 4.5 持久化数据

默认情况下，主程序持久化数据存放在**可执行文件同级的 `data/` 目录**下的 `khaslana.sqlite3`（便携目录，由 `std::env::current_exe` 推导）。为兼容老版本升级并防止数据落入易失目录，启动时按以下优先级解析当前应使用的库路径（`default_database_path` / `pick_active_path`）：

1. exe 旁便携库已存在 → 便携（真正在用的便携安装，含 U 盘场景，零打扰）；
2. 「上次数据目录」指针（`%LOCALAPPDATA%\Khaslana\last-data-home.txt`，`record_last_data_home` 在每次启动后 best-effort 写入）指向的库仍存在 → 指针目录（exe 被手动挪走或旧位置副本再次运行时延续数据，指针失效即忽略）；
3. 旧目录（`directories::ProjectDirs::from("", "", "Khaslana")` 的 `config_dir`，Windows 下为 `%APPDATA%\Khaslana`）库文件存在且无 `.migrated_to_portable` 标记 → 继续使用旧路径（老用户兼容，不被动迁移）；
4. 已迁移标记 → 不回读已弃用的旧库，按新装流程；
5. 全部无既有数据时：exe 位置安全 → 便携；**exe 位于危险/下载目录 → 固定目录**（`%LOCALAPPDATA%\Khaslana`，`fixed_database_dir`）——数据永不落在可能被清理的位置。

**exe 位置风险分级**（`classify_exe_location`，按路径组件小写整体匹配，`Templates` 等不误伤）：`Volatile`（`temp`/`tmp`/`$recycle.bin`/`inetcache` 组件或 `WeChat Files`/`Tencent Files`/`Telegram Desktop` 等聊天软件接收目录）、`Downloads`（`downloads` 组件）、`Safe`。风险只影响「无既有数据的新家选择」与搬迁提示，绝不改判已有数据的归属。

**程序搬迁**：exe 位于危险/下载目录时，启动就绪后弹「移动到安全目录」对话框（`DialogState::ExeRelocationPrompt`，dismiss 键 `exe_relocation_dismissed` 存 `schema_meta`；设置中心「更新设置」页在风险存在时常驻手动入口）。确认后 `request_exe_relocation` 把目标路径写入**固定目录下**的 `pending-relocation` 标记（绝不能写进危险目录）并重启；下次启动最早期（便携迁移之后、开库之前）`apply_pending_exe_relocation` 复制 exe 并把 exe 旁 `data/` staging 拷贝 + 验证库可读 + rename 到目标（`%LOCALAPPDATA%\Programs\Khaslana`），删除标记后从新位置启动新实例并退出；失败记日志删标记照常启动。搬迁后旧位置 exe 再被运行时经指针重定向到新家，数据不散。

C 盘已有旧数据的老用户首次进入便携版本时，会在启动就绪后弹窗询问是否「迁移到便携目录」（与搬迁提示同轮只弹一个，便携迁移优先）。用户确认后写入待迁移标记 `.pending_portable_migration` 并重启应用，下次启动最早期（打开任何连接之前）由 `apply_pending_portable_migration` 把旧库与 `updates/` 目录搬运到便携目录，验证新库可读后删除旧数据并写入 `.migrated_to_portable` 标记；用户选择「保持现状」则把 `portable_migration_dismissed` 写入 `schema_meta`，永久不再提示。工作流模板目录跟随实际激活的数据目录（`active_data_dir().join("workflows")`，与 DB / ai-reviews 同源），首次加载时若该目录为空且旧目录 `~/.khaslana/workflows` 存在模板则一次性拷贝。

当前数据库保存：

- 打开过的仓库路径和当前激活仓库。
- 最近打开过的仓库路径及最后打开时间（`recent_repositories` 表，供仓库切换下拉排序）。
- 每个仓库的 diff 编码偏好。
- 仓库远端到凭据策略的绑定。
- 全局网络代理设置，只保存模式和代理 URL，不拆分存储代理密文。
- 外部合并工具设置，包括是否启用 IntelliJ IDEA、是否选中冲突文件后自动打开以及可选 IDEA 命令路径。
- 全局主题偏好，支持跟随系统、浅色和深色。
- 快捷键绑定（`shortcut_bindings` 表，单行 JSON payload，action_id → keystroke 映射）。
- 凭据记录索引等非密元数据。
- 便携迁移相关标记（`portable_migration_dismissed`、`exe_relocation_dismissed` 等），存在 `schema_meta` 表。

数据库之外，同一数据目录还保存非 DB 数据：AI 评审记录落在 `<数据目录>/ai-reviews/<repo哈希8>/<毫秒>.json`（见 `src/ai/review_store.rs`，按仓库保留最近 30 条），工作流模板在 `data/workflows/`。

凭据密文不写入 SQLite，而是通过系统 Keyring 保存。`credentials.rs` 中的密钥服务名需要保持兼容，改动时必须加迁移或回归测试。最早的 JSON 文件存储及其一次性迁移工具 `migrate_storage` 已移除，主程序只认 SQLite（历史版本的 JSON 数据无法再导入）。

## 5. 当前用户可见功能

### 5.1 仓库和会话

- 打开本地仓库
- 克隆远端仓库，并根据 URL 推断目录名，默认递归克隆子模块
- 多仓库并存（仓库切换下拉：置顶克隆/打开 + 搜索仓库 + 打开项目[活动置顶] + 最近项目，按最近时间排序，下拉项可关闭已打开仓库）。「搜索仓库」位于打开仓库下面，默认为按钮，点击展开为输入框 + 小叉（收起恢复按钮并取消过滤）；每输入/删除一个字符实时过滤打开/最近两区（名称或完整路径子串匹配，大小写不敏感，`filter_repo_switcher_sections`，区内名称命中排在仅路径命中之前）；无结果显示「没有匹配的仓库」占位。键盘导航：↑↓ 在过滤结果上环绕移动高亮（`repo_switcher_highlight` 扁平索引，文本变化即复位），回车切换/打开高亮项（无高亮取第一项），Esc 关闭下拉。下拉菜单固定锚定在触发器按钮正下方（非鼠标位置），点击菜单外部或再次点击触发器按钮均关闭，复用根层 `capture_any_mouse_down` 的点击外部关闭机制；命中判定带 4px 边缘容差（`point_in_repo_switcher`），避免菜单左缘与侧边栏分栏分割线重合时，点在边框上的点击被误判为外部而关闭菜单并触发分割线拖拽（分割线的 `start_resize_column` 不再调用 `close_popups`，统一由根层捕获关闭）；弹层菜单（`any_popup_menu_open`：仓库切换下拉、各类右键菜单、编码菜单）打开期间全部分割线（列分割线与提交图列宽分割条）不响应鼠标、不显示拖拽光标（`column_splitter_accepts_mouse_events` 双参数门控），遮挡层打开也会中止进行中的拖拽。
- 自动保存和恢复会话
- 关闭主窗口时可选择直接退出或缩小到系统托盘；托盘支持恢复主窗口和退出应用
- 刷新仓库状态
- 通过子模块弹窗按需查看状态，并在弹窗打开后后台检查子模块相对远端分支的超前/落后状态
- 可手动同步父仓库记录版本，也可全量或单个快进更新到子模块远端最新

### 5.2 工作区

- 展示暂存和未暂存变更
- 已暂存和未暂存变更列表使用虚拟化渲染，上万文件时仅创建可见行
- 单选、多选、范围选择变更
- 暂存选中、暂存全部
- 按块/按行部分暂存（双向，仅工作区差异视图）：点击 +/- 行选择（Ctrl/Cmd 多选、Shift 范围，选中行整行半透明主题色打底 + 左缘 2px 主题色条；高亮层渲染在行背景之后，否则会被行自身不透明背景遮盖），hunk 分隔行右侧「暂存此块/取消暂存此块」按钮（`diff_hunk_action_button`，紧凑无边框样式，总高不超过 22px 的 hunk 行高），选区非空时差异标题栏出现「暂存选中行(N)/取消暂存选中行(N)」按钮（与「全文/编码」按钮同规格，不撑高标题栏）；按当前差异 scope 决定方向（DiffLine.hunk_index 提供块分组）。暂存/取消暂存（整文件或按块/按行，消息名单 `operation_refreshes_worktree_diff`）完成后差异面板跟随刷新（`refresh_diff_after_stage_change`）：文件在当前 scope 仍有改动时原位重载，整文件挪到对侧列表时清空，避免残留失效的按块按钮；存在性判定 `diff_scope_still_present` 对「未暂存 scope 且路径在快照中完全缺失」视为仍存在（未跟踪文件不在操作快照的 fast 状态里）。该刷新在 `OperationFinished` 中必须先于全量状态补全/分支同步请求执行，且这些请求改按刷新后的最新 `repository_load_id` 发起——`load_diff` 会经 spawn_operation 递增代际（diff 缓存失效机制），沿用旧代际的结果会被代际守卫丢弃，变更列表将停留在不含未跟踪文件的操作快照上。可折叠 diff 头部只折叠纯文件头（diff --git / index / --- / +++），首个 `@@` hunk 头虽同为 Header kind 但始终留在正文渲染，折叠时不能吞掉（否则首个 hunk 缺「暂存此块」入口且行号跳变，回归测试 `diff_render_rows_keep_first_hunk_header_in_body`）
- 取消暂存选中、取消暂存全部
- 暂存区文件右键可复制绝对路径或打开文件所在目录
- 丢弃单个、选中或全部变更
- 查看工作区 diff；选中未跟踪文件（？）时差异区展示整份文件内容（`show_untracked_content`），但白底显示不标绿（SourceTree 式：`FileDiff.untracked` 标记 + 纯函数 `display_diff_line_kind` 把 Added 行映射为 Context 配色，仅影响显示，服务层行 kind 保持 Added、部分暂存守卫不受影响）
- 差异区域支持全文/紧凑切换：切换按钮位于标题栏编码按钮旁，开启后展示整份文件并保留增删行高亮；全文模式（工作区/历史/贮藏/浏览四个差异视图共用）带语法高亮——文本色来自 syntect span、行背景仍按增删/上下文 kind 表达（GitHub 式），紧凑差异块不高亮；语言未识别或 >1MB/20K 行回退纯文本；块状态/部分暂存选中层等交互不受影响
- 大 diff 使用虚拟列表渲染
- 选中二进制文件时差异区域显示居中信息占位卡片（`binary_diff_placeholder`，`src/ui_helpers.rs`）：说明无法以文本展示差异，并按新增/删除/修改给出文件大小（`FileDiff.old_size`/`new_size`，由 `file_diff_from_diff` 按 delta 状态填充，Added/Untracked 旧侧为 None、Deleted 新侧为 None；oid 侧读 blob 对象头，工作区侧用 stat 尺寸）；同时隐藏无意义的「全文切换」「编码」按钮。工作区、历史、贮藏和分支比较的差异区域共用同一渲染，行为一致。二进制判定有三路：`diff.print` 回调里的 `DiffFlags::BINARY`/`'B'` 行（补丁生成时才可靠）、未跟踪文件的 8KB NUL 嗅探 `workdir_file_is_binary`（`show_untracked_content` 已加载内容，嗅探保留兜底）、以及已知二进制扩展名兜底 `path_has_binary_extension`（内容检测对空文件无能为力，如右键新建即空的 .docx）
- diff 头部可折叠
- hunk 分隔行视觉增强：`@@ 行号范围 @@` 行使用独立底色 `DIFF_HUNK_BG` + 上下边框 + 圆角胶囊 + 行号列同底色（文件头行保持原浅色），深浅主题下均与底色明显区分
- diff 编码可选
- diff 区域支持左右滑动查看长行
- 全文视图对超大文件（超过 `FULL_FILE_MAX_BYTES`）自动回退到紧凑差异并提示
- 提交信息输入（多行框固定可视高度 `MULTILINE_LINE_HEIGHT * MULTILINE_MIN_LINES`（5 行），内容超出滚动并显示自绘滚动条；`MultiLineInputElement` prepaint 带光标跟随滚动，不要求聚焦，AI 流式回填也会滚到最新内容；跟随以跟随键（光标字节, 内容长度）门控——仅键变化（光标移动或内容改变）且光标行越界时才滚动，键不变视为用户手动滚动、不回弹抢夺视口，决策纯函数 `multiline_caret_follow_decision` 可单测）和 commit；「修补上次提交」开关（IDEA 式，位于输入框上方靠右）：开启后主按钮变「修补提交」、「提交并推送」变「修补提交并推送」（修补后推送，组合错误提示与提交并推送一致），输入框为空自动预填 HEAD 完整提交信息（优先用内存历史数据，未加载过历史时经后台任务读 HEAD 回填，不依赖进入过提交记录页；关闭开关时清除未被用户编辑的预填内容），输入框为空则修补保留原信息，以当前暂存区重写 HEAD；HEAD 已推送（branch_sync_status ahead==0 且有 upstream）时弹强推后果确认（按入口区分确认后动作）
- 变基进行中时在工作区顶部显示变基状态条，提供「继续变基 / 跳过此提交 / 中止」操作；冲突解决后自动复用现有冲突工作台
- 普通合并要求工作区干净；无冲突的非快进合并自动提交（双父提交），不保留 merge 会话。发生冲突时保留 Git Merge 状态，停留在工作区，通过合并状态条可直接调用 IDEA；冲突清零后状态条隐藏，右下角可编辑提交信息并「完成合并」，也可确认后「中止合并」恢复到合并前 HEAD
- 冲突工作台支持「用 IntelliJ IDEA 解决」，自动检测 `idea64` / `idea` 命令或 `KHASLANA_IDEA_PATH`，通过外部 Merge Dialog 生成结果后写回并标记解决；设置中心「合并工具」可持久化 IDEA 路径，并可选择在选中冲突文件时自动打开 IDEA
- 冲突工作台三栏（当前版本/结果区/传入版本）带语法高亮：块状态背景保留为行背景、语法色做前景，结果区随按块接受/AI 生成重算（后台补算 + seq 防乱序）；底部 base 折叠面板 v1 不高亮
- 冲突工作台支持「AI 合并建议」（工具栏按钮，仅文本冲突且 AI 已配置启用）：后台读取该文件 diff3 原文（`GitService::conflict_diff3_text`），整文件 ≤60K 字符单请求，超限按块边界分段逐段生成（进度显示「第 i/N 段」）并携带滑动窗口对话历史；全部段成功后拼接整份文件经 `set_merged_draft` 一次性填入结果区（不做流式增量回填，内部与 `set_draft` 共享区间平移算法）。**所有**冲突块标记 `ConflictBlockStatus::Merged`——AI 对整份文件做出完整合并决定，内容与当前侧一致、未落入 diff 改动区的块（AI 选择保留当前侧）与「输出与草稿完全一致」的早退分支同样标记，否则这些块会永远停留在未处理（Merged 为绿色「已合并」徽标、结果区选中绿底，不计入未处理、不触发手工修改横幅与解决确认弹窗；`has_local_edits` 仍为 true，重复生成仍弹覆盖确认；手动编辑路径 `set_draft` 行为不变），沿用现有「应用到工作区/应用并标记已解决」闭环。任一段失败整体失败不写入草稿；响应经 `strip_code_fence` 清洗与冲突标记残留检测（残留即放弃）。草稿已有块处理/手工编辑（`has_local_edits`）时先弹覆盖确认弹窗；未配置 AI 时按钮显示「AI 合并建议（未配置）」禁用态（按钮 helper 不支持 tooltip）。AI 未配置或生成中不借用 busy，其它冲突操作保持可用
- 冲突工作台三栏之间的 IDEA 式连线 overlay（`render_conflict_connectors`，`src/conflicts/mod.rs`）：三栏行容器 `relative` + 末位绝对定位 canvas（纯绘制不注册鼠标事件），从 ours 右缘/theirs 左缘向结果区左右缘画 S 形三次贝塞尔曲线，指示各冲突块采用后内容落点。坐标换算：每块行区间经 `conflict_document_byte_range` + `conflict_byte_range_to_lines` 构建时预计算，paint 时从各栏 `UniformListScrollHandle` 读视口 bounds、滚动 offset 与行高（`ItemSize.contents.height / 总行数`，兜底 18px），每帧重绘自动跟随滚动；非选中块 `MUTED_FOREGROUND` 实色（`BORDER` 与背景融为一体不可用）、选中块 `ACCENT` 加粗，块在任一端整段滚出视口即跳过该线（部分可见钳到可视段中点，纯函数 `conflict_block_y_range`/`conflict_connector_anchor_y` 可单测）。三栏视口顶部不对齐（ours/theirs 有操作按钮行），连线斜向是「落到哪里」的正确语义。**三栏同步滚动**也在该 canvas 的 prepaint 完成：与上帧 offset 记录（`RepositoryView.conflict_pane_scroll_sync`，Rc 共享给闭包，选中冲突文件时重置）比较，恰好一栏变化（用户滚轮/拖滚动条，`conflict_scroll_sync_source`）时把它作为源、其余两栏钳制到各自 `max_offset` 后设同一竖直 offset（横向不联动）；多栏同时变化（程序化三栏联动 scrollToItem）不同步。set_offset 只写值不触发重绘，故在 prepaint 期写入同帧生效并经 `App::refresh_windows` 补一帧（无变化即收敛），paint 期写入则本帧读不到且 refresh 是 no-op

### 5.3 分支、远端、标签、贮藏

- 本地分支列表、创建、删除、重命名、切换
- 远端分支列表，checkout 后创建/复用本地跟踪分支
- 远端列表、选择、添加、编辑、删除
- fetch、pull、push
- pull 对话框提供「用变基代替合并」勾选框，默认不勾选，勾选后执行 pull --rebase
- pull（含当前分支的单独拉取）的非快进干净合并与显式合并一致：自动提交双父提交，不保留待确认 merge 会话；有冲突时才保留 Git Merge 状态进入冲突闭环
- 推送/删除远端分支被拒绝时显式报错：常规 non-fast-forward（客户端预检查与服务器状态报告两条路径）统一映射为「请先拉取并解决后再推送」中文引导（`NON_FAST_FORWARD_PUSH_MESSAGE`），hook/保护分支等其它服务器拒绝经 `push_update_reference` 回调透传原因（libgit2 对按引用拒绝默认静默，回调不注册会假报成功）
- 全局刷新仅刷新本地状态；远端刷新通过工具栏”获取”或远端列表右键”刷新”显式触发
- 设置/修改本地分支 upstream
- 本地分支显示相对 upstream 的待推送/待拉取提交数
- 本地分支右键可单独拉取；非当前分支仅在可快进时直接更新
- 删除远端分支，右键复制远端分支名称和 checkout 命令
- 分支右键「变基到当前分支」，将选中分支的提交变基到当前分支之上
- 切换、创建、重命名、删除、拉取、推送等分支引用变动操作完成后自动完整刷新仓库状态
- tag 列表和 checkout tag
- 标签管理：创建（附注/轻量，目标默认 HEAD 或历史页右键指定提交）、删除本地标签、推送到指定远端、删除远端标签（确认弹窗）；标签区常驻显示，空标签时区头“+”仍可创建
- stash 列表、创建、apply、pop、drop、文件列表和 diff 预览
- 工作流模板可视化创建与编辑：「新建」按钮打开单页编辑器（预设卡片 / 常用步骤优先 / 高级折叠区，见 `src/workflow_editor.rs`）；模板行右键「编辑此模板」（含注释的文件先弹确认告知保存会丢注释）/「复制为副本」（载入内容强制另存新文件名）/「删除模板...」（确认弹窗，删除的是当前加载工作流时同时清空详情区）；保存经序列化回读守卫后写模板目录并刷新列表自动选中加载

### 5.4 历史

- 当前分支 / 所有分支提交历史
- 拓扑排序提交图
- 提交图列宽可拖拽调整（`ResizeTarget::HistoryGraph`，双击分割条复位），可见泳道数随列宽动态计算，超出以省略号提示
- 提交图泳道不按加载窗口剪枝，保证线条跨行连续、跨页加载时上方布局不抖动
- 提交引用标签，包括本地分支、远端分支、tag、HEAD
- 切换分支、提交、合并、变基、拉取、推送等操作完成后自动后台刷新提交记录和引用标签（含 HEAD），无需人工刷新
- 分页加载更多
- 提交详情区（历史页左列上半部，四象限布局：详情上、文件列表下、差异右侧全高）：展示选中提交的摘要与引用徽章、完整提交信息（多行正文）、完整 SHA（可一键复制）、作者（含邮箱）、提交者（仅与作者不同时显示）、提交时间、父提交关系（双父标注合并提交）；默认与文件列表上下对半分（`history_details_height = None`，双方各占 flex_1），拖拽分割条后固化为绝对高度（`ResizeTarget::HistoryDetails`，对半分基准经 1px 标记 canvas 记录的左列顶部坐标推导），双击分割条复位回对半分；可折叠，内容区带自绘滚动条（`scrollable_frame_when`），无选中提交时不渲染。数据来自 `CommitInfo.message/author_email/committer/committer_email`（`collect_commit_infos` 一次性填充），无额外服务层调用
- 查看提交文件列表
- 查看指定提交文件 diff
- 提交文件列表右键可复制绝对路径或打开文件所在目录
- 右键提交可复制 SHA、reset、revert、撤销合并提交、拣选提交到当前分支（合并提交暂禁用）、在此提交上创建标签等
- 文件历史（路径过滤）：工作区变更（已暂存/未暂存两分支）与历史页提交文件右键「查看文件历史」，只显示改动过该文件的提交；过滤状态 `RepoTabState.history_file_filter`，`clear_history` 不清过滤器（用户意图，切 scope/切分支/刷新均保留，仅显式点标题栏过滤 chip 的 × 清除，per-tab 生命周期随 tab 销毁）；`HistoryCommitsLoaded` 携带 `path_filter`，应用守卫扩为 scope + path_filter 双比较（`RepoTabState::history_commits_event_matches`），防切换过滤后旧请求覆盖新数据；过滤模式下隐藏提交图形列与列宽分割条（过滤后中间提交缺失，泳道线会断裂）、标题栏显示「文件：<basename>」chip（hover 完整路径）；`HistoryFilesLoaded` 自动选中首个文件时若 filter 激活且列表含该路径则优先选它（diff 立即可见）
- 文件追溯（blame，UI 术语统一为「追溯」）：工作区变更两分支右键、历史页提交文件右键「追溯此文件」与工作区 diff 标题栏「追溯」按钮（规格复用「全文/编码」工具按钮，仅工作区且非二进制显示）进入独立 `MainMode::Blame` 视图；基于 HEAD + 工作区未提交改动（历史页入口对 HEAD 版本追溯），三列布局（注释栏/行号/内容，无分割线、注释栏微灰底侧栏 + 块首行固定宽度分列对齐），未提交行整行警告色打底 + 「未提交」徽标与已提交行区分，已提交行内容列带语法高亮（未提交行保持警告前景色纯色），支持双向滚动与编码切换重载；检出分支/标签时因 HEAD 变化自动关闭回工作区

### 5.5 分支浏览

- 不切换分支查看其他分支/标签的完整代码
- 从侧边栏本地分支、远端分支和标签的右键菜单进入「浏览此分支 / 浏览此标签」
- 从侧边栏本地分支和远端分支右键进入「与当前分支比较」，不切换分支列出目标分支领先当前分支的提交所改动的文件（三点比较，当前分支独有改动不显示）
- 左侧文件树浏览器：可展开/折叠的目录树，按目录懒加载
- 右侧默认显示目标分支上文件的只读原始内容（含行号、编码识别与语法高亮；与 HEAD 差异视图在全文模式下同样高亮）
- 顶部可一键切换到「与当前 HEAD 的差异」视图；比较模式带 AI 评审面板（Diff-first Agentic，覆盖全部差异文件，见 §1 与 `src/ai/review_agent.rs`）。面板两态：生成中/展开为**全区域模式**（替换右侧内容/差异视图占满整区，标题栏显示轮次进度与「复制结论」按钮（有结果时一键复制最终评审正文），「收起」回底部条；步骤时间线 Codex/ZCode 式——左侧竖轨 + 节点圆点，已完成步骤各一行：思维链「思考：{首行摘要}」与工具调用「▸ read_lines src/x.rs:100-180」点击展开 TILE 底等宽详情块（错误步骤警告色），中间轮 assistant 正文为「❝ + 全文」**不折叠整段直出**（模型明确说的话，不被下一个工具消息覆盖）；流式期间的**live 行**挂在时间线尾部——「✻ 思考中…」+ 思维链全文灰色小字实时变长，轮次落定后折叠为正式摘要行；最终正文边生成边按 Markdown 渲染（半截文档解析器 EOF 收尾）、完成定格）；收起为底部单行条（80 字符正文预览、历史标签或生成进度；生成中提供「展开/取消」，完成后「历史/展开/重新生成」）。正文与时间线在同一滚动容器（内容 div 自带 `overflow_y_scroll` + `track_scroll`，`scrollable_frame_when` 只画滚动条覆盖层——滚动容器必须由内容 div 创建），完成态按 Markdown 渲染（`render_markdown`）。生成中可收起（不再强制全区域）。**任务与记录**：全区域标题栏与收起条均有「历史」按钮打开评审历史弹窗（最近 20 条：完成时间/目标分支/模型/步骤数，点击行载入面板并以「历史 · 时间 · 目标」标签展示；背景点击或 ✕ 关闭）；切换比较目标/退出浏览**只分离展示**（`reset_ai_review_state` 置 `ai_review_active_generation = None`，任务后台继续执行、完成落盘后 toast「后台 AI 评审完成」）；「取消」按钮置位 AtomicBool 标志，任务在轮次边界退出不落盘；同时进行的任务上限 3 个（`MAX_CONCURRENT_AI_REVIEWS`，超出点「生成」时 toast 阻止；已知的取舍：分离到后台的旧任务没有单独取消入口，会跑满至完成并占用并发名额之一，`ai_review_cancel` 只持有当前附着任务的标志）；所有评审事件携带代际，非当前附着代际的 Step/Progress/Delta 丢弃、Generated/Failed 只做后台提示
- 分支比较模式左侧以目录嵌套文件树展示差异文件（默认全部展开，可折叠），右侧同样支持目标分支全文内容和与当前 HEAD 的差异视图
- 支持切换 diff 编码
- 整个过程不执行 checkout，不改动工作区
- 二进制文件提示「无法预览」，超大文件自动报错提示
- 子模块条目仅展示，不可下钻

### 5.6 凭据

- HTTPS 用户名 + 密码/PAT
- OAuth 快速登录：在添加 HTTPS 凭据时点击品牌矩形按钮（GitHub/Gitee 带文字 logo，按当前主题选浅/深变体），浏览器授权后自动取登录名并保存，无需手动录入 PAT。
  - GitHub：OAuth Device Flow（请求设备码 → 自动打开浏览器并预填验证码 → 后台轮询拿 access_token → 取登录名）。`GITHUB_OAUTH_CLIENT_ID` 为编译期常量，需维护者在 GitHub 注册 OAuth App 并勾选 Device Flow 后填入 `src/oauth.rs`。scope `repo workflow`，覆盖私有/公有仓库读写与工作流文件推送。受限组织（开启 OAuth App Access Restrictions）的私有库需组织 Owner 审批该 OAuth App 后才能访问。
  - Gitee：授权码流（`gitee_run_code_flow`：本地 `127.0.0.1:17890` 回调收 code → POST 给 broker 换 token → 取登录名）。Gitee 不支持 Device Flow/PKCE，公开客户端不能内置 `client_secret`，故 token 交换由部署在边缘平台的 broker 代办（独立仓库 [khaslana-broker](https://github.com/Neglecton/khaslana-broker) 的 `edge-functions/gitee.js`），客户端只持 `GITEE_OAUTH_CLIENT_ID` + `GITEE_BROKER_URL`（二者均非空时 Gitee 按钮启用）。scope `projects user_info`。**Gitee access_token 约 1 天过期（平台限制）**，过期后 git 操作会认证失败，重新点击 Gitee 登录即可刷新（MVP 手动续期，自动刷新留作后续）。
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
- 主题色更换：在外观设置中可从 9 种预置主题色（靛蓝、紫罗兰、玫红、橙、青、翠绿、石墨、金棕、天蓝）中选择，默认为靛蓝。主题色影响主色族（按钮、选中态、链接、输入框聚焦边框/选区、HEAD 标签、进度条等），Yororen 组件的聚焦边框也跟随主题色。主题色预设定义在 `src/ui/theme.rs` 的 `ACCENT_PRESETS`，运行时通过 `ACTIVE_ACCENT` 原子和 `resolve_accent_token` 动态解析，业务 view 无需感知。主题色索引持久化到 `theme_preferences.accent` 列。

### 5.9 设置中心

- 工具栏「设置」按钮打开设置中心弹窗（独立 `settings_center: Option<SettingsCategory>` 状态，不占 `active_dialog`）。
- 左侧 7 分类导航：凭据管理、网络代理、AI 设置、合并工具、外观、更新设置、快捷键。
- 右侧内容面板渲染对应分类的表单/列表。设置中心只通过右上角「×」（及背景点击、Esc）关闭，各分类页面不再有「关闭/取消」按钮。
- 保存按钮统一逻辑：只保存当前分类页内容，按 `last_error` 经 `notify_settings_save` 提示成功/失败（失败 toast 带具体错误信息），成功后不关闭页面。基础 save 方法（如 `save_network_proxy_settings`）同时被输入框回车静默自动保存复用，提示逻辑只放在保存按钮闭包里。
- 外观、更新、快捷键页为即时生效（无保存按钮），凭据管理为列表页（顶部「添加凭据/刷新」），均只靠「×」关闭。
- 关闭设置中心时清掉可能残留的外部合并「保存并继续」待处理冲突路径（`external_merge_view::clear_pending_external_merge_path`）。
- 凭据管理的子弹窗（详情/表单/删除确认）经 `active_dialog` 分发，可叠加在设置中心之上；关闭子弹窗后回到设置中心。
- 原工具栏的 6 个独立设置按钮已移除，统一由设置中心承载。

### 5.10 快捷键

- 基于 GPUI 的 Action + `bind_keys` 机制（与文本输入框同一套键位系统）。
- 应用级快捷键通过 `actions!(app_action, [...])` 定义 12 个动作，在 `register_all_key_bindings` 中用 `!TextInput` 谓词注册（输入框内不劫持）。
- 一期覆盖 12 个动作：刷新(F5)、获取(Ctrl+Shift+F)、拉取(Ctrl+Shift+L)、推送(Ctrl+Shift+P)、贮藏(Ctrl+Shift+S)、子模块(Ctrl+Shift+M)、设置(Ctrl+,)、工作区(Ctrl+1)、提交记录(Ctrl+2)、工作流(Ctrl+3)、在资源管理器中打开仓库(Ctrl+Shift+O)、以浏览器打开远端(Ctrl+Shift+B)。
- 用户可在设置中心「快捷键」分类中录制新快捷键（按下组合键即录入）；冲突时拒绝并提示已被哪个动作占用；每条可单独恢复默认。
- 快捷键绑定持久化到 `shortcut_bindings` 表（JSON payload），启动时加载并用默认值补齐缺失动作；用户修改后实时 `clear_key_bindings` + 重注册。
- 新增快捷键动作时：在 `ShortcutAction` 枚举加变体 + `action_id`/`label`/`default_keystroke` + `actions!` 宏加对应 GPUI action + `register_all_key_bindings` 加 match 分支 + 根 render 加 `on_action`。

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

每次实现完计划后必须执行 `cargo build --release`，并修复所有出现的错误和警告（无论相关代码是不是本次写的）。只有 release 构建零错误零警告才算实现完成。

正式发布使用 `release-perf` profile（fat LTO + codegen-units=1，见 Cargo.toml；产物在 `target/release-perf/`），GitHub 发布工作流 `.github/workflows/release.yml` 按 tag 触发并用该 profile 构建打包。发版产物（双渠道同步上传：GitHub Releases + CNB `khaslana-release` 仓库）：便携 zip（`khaslana-v*-windows-x86_64.zip`）、**Inno Setup 7 安装器**（`khaslana-setup-v*-windows-x86_64.exe`，脚本 `installer/khaslana.iss`：用户级安装免管理员，默认目录 `%LOCALAPPDATA%\Programs\Khaslana` 与应用内「移动到安全目录」一致，卸载不删运行期 `data\`；CI 从官方 GitHub Release 下载 `innosetup-7.1.0-x64.exe` 静默安装后经 `C:\Program Files\Inno Setup 7\ISCC.exe` 编译，升 Inno 版本时改 URL；架构标识用 `x64compatible`——Inno 7 中 `x64` 已弃用）、两者各自的 `.sha256` 与 `khaslana-update.json`（更新清单只引用 zip——自更新走 zip 原位替换，安装器仅首次安装用）。安装器本地构建：**`cargo setup` 一键完成**（`.cargo/config.toml` 的 alias → `src/bin/khaslana_packager.rs`，纯 std：release-perf 构建 -> 组装 `dist/package/` -> 探测 ISCC（Inno 7 优先）编译，版本取编译期 `CARGO_PKG_VERSION`）；或按 CI 相同步骤手动执行（Git Bash 下调 ISCC 需 `MSYS_NO_PATHCONV=1` 防止 `/D` 被转成路径，打包器 bin 无此问题）。发布工作流不复用打包器（显式分步执行，产物内容一致）。发版流程：改 `Cargo.toml` version 与 `src/tests/update.rs` 的版本断言 -> 提交 -> 打 `v*` tag 推送。

## 7. 测试现状

项目已有较多单元测试，重点覆盖：

- `src/tests/git.rs` 及 `src/tests/git/` 子目录：Git 操作、分支、远端、stage/unstage/discard、提交、历史、reset/revert、编码、冲突保护等；其中包含 Windows 目录占用回归测试，通过不共享删除权限的目录句柄模拟 VS Code/终端占用，覆盖分支切换、快进 pull、stash 保存和 stash 应用。
- `src/tests/credentials.rs`：凭据匹配、Keyring/内存存储逻辑、URL 规范化、记录排序、兼容性判断等。
- `src/tests/main.rs`：会话 JSON、路径去重、编码偏好、远端凭据绑定、克隆路径推断、文本输入状态、diff 渲染模型、分支浏览状态切换与缓存清理等。
- `src/tests/git/browse.rs`：分支浏览引用解析（本地/远端分支、标签）、文件树遍历、文件内容读取（编码检测与二进制判定）、与 HEAD 差异，以及子模块条目识别等基于 `tempfile` 的仓库级单测。
- `src/tests/git/blame.rs`：文件历史路径过滤（只返回触及该文件的提交、分页基于过滤后流、CurrentBranch/AllRefs 两 scope、未触及路径空结果）与文件追溯（行号内容对齐、多 hunk 分组、工作区改动行 `commit: None`、HEAD 无路径与二进制守卫）的仓库级单测。
- `src/tests/syntax.rs`：语法高亮纯函数单测——span 拼接与原行字节恒等（含中文）、相邻同色合并且无零长度、扩展名/文件名检测（.rs/.py/.md 命中、未知扩展与 Makefile/GNUmakefile 兜底）、深浅主题产出不同颜色、体积/行数守卫返回 None、空文件与空行安全、diff 全文行索引对齐且文件头/hunk 头/EOFNL 行产空 vec。
- 普通合并测试覆盖快进、无冲突结果暂存、显式完成后的双父提交、冲突状态恢复、冲突后完成、确认中止、脏工作区拒绝和重新打开仓库后继续合并。
- `src/tests/browse_view.rs`：文件树展平纯函数 `flatten_browse_tree`（展开/折叠/嵌套）单测。
- `src/tests/markdown_view.rs`：Markdown 纯解析层单测——标题级别、软换行分行、加粗/斜体/删除线/行内代码样式标记、围栏代码块保行、有序/无序列表前缀与嵌套缩进、引用块、分隔线、链接仅保留文字、流式半截文档安全（未闭合围栏/加粗不 panic）、span 拼接还原原文。
- `src/tests/git/search.rs`：代码搜索单测——子串跨文件命中与行号、正则匹配与编译错误中文文案、max_results 截断、空白查询拒绝、无扩展名二进制经 NUL 嗅探跳过、tree OID 当提交解析报错不 panic、`path_prefix` 目录限定（含斜杠/空白归一化、不存在前缀与文件前缀的中文报错）、超长命中行截断到约 200 字符。
- `src/tests/ai/review_agent.rs`：Agentic 评审纯函数单测——read_lines 行窗口钳制（默认整文件/400 行上限/越界钳制/空文件）、结果截断标注、初始上下文装配（预算内全量 vs 超限逐文件截断 vs **200 文件总量二次封顶**）、`ToolBudget` 三路上限触发强制收尾与 `limit_reason` 三路命名（轮次 > 次数 > 体积优先级，引用常量上限调整自动适配）、追溯块/搜索命中/历史条目格式化、工具参数一行摘要（含坏 JSON 容错）、`file_diff_to_patch_text` 行 kind 映射与二进制占位。agent 协议序列化/流式解析/错误分流单测在 `src/tests/ai/client.rs`（assistant 带 tool_calls 的 content=null、tool 消息形态、tools 省略、`StreamingToolCallAccumulator` 分片聚合——跨 chunk 拼 arguments、id 仅首片、index 排序、无 name 丢弃、id 合成、含 tool_calls 的 SSE 行解析、HTTP 400 双因与 404/422 → 更换供应商文案分流、`classify_agent_http_error` 可重试分流（408/429/5xx 可重试、配置类 4xx 不重试）、`agent_turn_empty_failure_message` 四路截断归因文案、流中 error 事件与 finish_reason 解析）。`src/tests/ai/review.rs`：时间线步骤单测——Reasoning 首行派生/超长截断/空白兜底、Message 摘要与 `is_collapsible`（Message 不折叠、Reasoning/ToolCall 折叠）、ToolCall 摘要透传与错误标记。`src/tests/ai/review_store.rs`：记录落盘单测——保存/列表往返（倒序、完整轨迹）、同毫秒后缀 id、30 条清理保留最新、坏 JSON 跳过、repo 哈希稳定（含 FNV-1a 已知向量）。

- `src/tests/workflow.rs` 补充：工作流定义序列化 round-trip（全部 11 种步骤变体 + 含 inputs/vars 完整定义，json5::to_string -> parse_workflow_json5 结构恒等）与 None 字段省略（生成模板不出现 null 噪音键）。
- `src/tests/workflow_editor.rs`：模板创建器纯函数——定义构建校验（空步骤/必填槽缺失指明序号与槽名/inputs 与 vars 键空值保留字重复/空白 trim）、可选字段 None 省略、文件名校验（空名/非法字符/后缀剥离/大小写不敏感重名拒绝）、3 个预设生成合法定义且序列化回读 round-trip 一致、FeatureBranch 预设的 `${target}` 引用、步骤草稿摘要未填占位、步骤类型元数据一致性（常用在前/op_name 互逆）、切换类型槽保值、guardRemoteBranch/deleteBranches 默认值跟随领域约定。v2 编辑功能：注释检测（无注释/行/块/字符串内 `//` 不算/转义引号）、反映射 round-trip（11 变体 + description + vars 定义→编辑数据→定义恒等）、空名称往返、重名排除自身、保存目标决策（`.jsonc` 原件原地覆盖/大小写不敏感主干比较/改名删旧/新建相对名）。

测试代码组织方式：所有单元测试通过 `#[cfg(test)] #[path = "..."] mod tests;` 外移到 `src/tests/` 目录，目录结构映射源文件结构（子目录源文件经 `#[path = "../tests/..."]` 引用）。外移的测试模块通过 `use super::*` 仍可访问源文件的私有项。Git 相关测试共享 `src/git/test_support.rs` 中的 fixture 函数（`init_repo()`、`service()`、`commit_all()` 等），通过 `use crate::git::test_support::git_test_support as git_support;` 引用。

新增 Git 业务能力时，优先在 `src/tests/git.rs` 增加基于 `tempfile` 的仓库级单元测试，使用 `git_support::init_repo()` 等共享 fixture。新增纯 UI 状态逻辑时，优先拆成可测试的小函数，测试放在 `src/tests/` 对应文件中。

## 8. 编码和设计约定

- 代码修改要有中文注释，完成后应当检查`AGENTS.md`内容是否需要调整。
- 用户可见文案保持中文；blame 功能的 UI 术语统一用「追溯」，不直接暴露英文 blame。
- 语法高亮统一经 `src/syntax.rs`（syntect）计算、`syntax_styled_text` 渲染；颜色来自 syntect 内置主题按深浅二选一，不自建 scope 映射，也不把 span 颜色混入 `ui/theme.rs` 语义 token 体系。新视图接入按「内容落位 → `schedule_syntax_highlight` → `SyntaxHighlighted` 回填（Arc 身份守卫）」模式。
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
- 新增或修改会写入工作区的 Git 操作时，必须复用 `src/git/worktree_compat.rs`，不能直接调用 `checkout_tree`、`checkout_index`、`checkout_head`，也不能绕过其中对 reset、revert、rebase、stash 和子模块更新的包装。Windows 下只允许跳过“受 Git 管理的文件已正确处理、但空父目录因占用无法删除”的情况；锁定文件、冲突和本地修改保护仍必须报错。
- 右键菜单和弹窗位置应复用现有菜单定位/对话框样式。
- 可滚动面板的结构参照仓库切换下拉（`render_ai_review_panel` 全区域模式同构）：外层有界（flex_1/flex_none + min_h 0 或 max_h）+ `scrollable_frame_when` 作直接子元素（内部自带 flex_1 + min_h 0）+ 内容 div 只挂 `.id().overflow_y_scroll().track_scroll()`。不要在内容 div 上再叠 flex_1/min_h 或插入额外包裹层——高度约束不会沿多层 flex 收缩链传递确定高度，多一层滚动就失效。

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

### 9.6 Windows 子目录占用与 libgit2 工作区写入

已修复问题：当 VS Code、终端或语言服务打开仓库中的某个子目录，而切换分支、pull 或其他 Git 操作需要删除该目录中的最后一个受管文件时，libgit2 会继续尝试删除空父目录。Windows 会因目录句柄未共享删除权限而返回 `could not rmdir ... 另一个程序正在使用此文件`，并让整个操作失败；SourceTree/系统 Git 对这种情况通常会保留空目录并继续完成操作。

修复结果：

- `src/git/worktree_compat.rs` 统一配置 libgit2 的 `GIT_CHECKOUT_SKIP_LOCKED_DIRECTORIES`。
- 已接入显式分支/标签切换、普通及冲突合并、快进 pull、pull --rebase、hard reset、revert、rebase 的开始/继续/跳过/中止、stash 保存/应用/pop、丢弃修改、冲突版本选择和子模块更新。
- Git 仍会正确删除或更新受管文件并推进 HEAD/引用；只有被占用且已为空的目录可能暂时保留，待占用释放后可由用户或后续操作清理。
- 锁定文件本身、未提交修改覆盖风险和 Git 冲突不会被忽略。
- Windows 回归测试使用真实目录句柄验证分支切换、快进拉取、stash 保存和 stash 应用，完整 `cargo test --lib` 已通过。

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

### P1：文件历史和 blame（已完成）

理由：当前历史页已经有 commit graph、commit files 和 commit file diff，继续扩展到“选中文件的历史”非常顺手，而且是 Git 客户端高频需求。

已完成范围（2026-08）：

- 工作区变更文件（已暂存/未暂存）和历史文件右键菜单「查看文件历史」。
- 历史页文件路径过滤模式（`RepoTabState.history_file_filter`，`clear_history` 不清过滤器；过滤模式隐藏提交图形列；标题栏过滤 chip 可清除，hover 显示完整路径）。
- 显示某文件相关提交列表和该文件在每次提交中的 diff（复用现有文件列表 + 差异视图；`HistoryFilesLoaded` 自动优先选中被过滤路径）。
- blame 视图（UI 术语「追溯」）：独立 `MainMode::Blame`，基于 HEAD + 工作区未提交行（`blame_buffer`），IDE 风格分组注释栏 + 虚拟列表 + 编码切换；入口为工作区右键、历史页提交文件右键与工作区 diff 标题栏「追溯」按钮。

实现说明：

- `GitService::file_history`（`src/git/blame.rs`）按路径过滤：revwalk 不支持 pathspec，全量迭代 + 逐提交 first-parent tree-diff 单 pathspec 判断，分页作用于过滤后 OID 流。
- `GitService::blame_file`：HEAD blob 守卫（未提交报错/过大/二进制）+ `blame_buffer` 纳入未提交改动；hunk 作者/时间/摘要直接取 libgit2 填充的 final 签名与 summary。
- rename/copy 追踪（`--follow` / -M -C）与对任意提交版本 blame（`BlameOptions::newest_commit`）留作后续迭代。

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

已完成基础范围（历史页左列上半部四象限布局，见 §5.4）：完整 SHA、父提交（双父标注合并提交）、作者（含邮箱）、提交者（与作者不同时）、时间、完整 message、SHA/提交信息一键复制、高度可拖拽与折叠。实现采用扩展 `CommitInfo`（`message`/`author_email`/`committer`/`committer_email`）而非新增 `CommitDetails`，`collect_commit_infos` 一次填充，无额外 IO。

建议后续范围：

- 引用徽章点击跳转（如点击分支名定位到该分支）。
- 父提交短 oid 点击后在历史列表中定位该提交。

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

