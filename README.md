<p align="center">
  <img src="logo.png" alt="Khaslana logo" width="96" height="96">
</p>

<h1 align="center">Khaslana</h1>

<p align="center">
  一个使用 Rust、gpui-pre 和 libgit2 构建的轻量桌面 Git 客户端。
</p>

<p align="center">
  <a href="#下载与安装">下载与安装</a> ·
  <a href="#功能特性">功能特性</a> ·
  <a href="#数据持久化">数据持久化</a> ·
  <a href="#测试">测试</a> ·
  <a href="#许可证">许可证</a>
</p>

## 简介

Khaslana 是一个面向日常开发工作流的桌面 Git 客户端。它使用 `gpui-pre`（Cargo 别名 `gpui`）与 `gpui-kit` 构建原生界面，使用 `git2` / libgit2 执行 Git 操作，并通过系统 Keyring 保存凭据密文。

项目目标不是替代所有 Git CLI 能力，而是把仓库打开、分支切换、暂存提交、远端同步、历史查看、凭据管理、代理设置、子模块更新和常用自动化工作流整合到一个轻量客户端里。

当前界面文案以中文为主，适合在 Windows 桌面环境下开发和使用。

## 下载与安装

从发布页获取最新版本（两个渠道内容一致）：

- CNB（国内推荐）：`https://cnb.cool/suhoan/khaslana-release`
- GitHub Releases：`https://github.com/FuturePrayer/khaslana/releases`

| 产物 | 适用 |
| --- | --- |
| `khaslana-setup-v*` | **推荐**。双击安装到用户目录（`%LOCALAPPDATA%\Programs\Khaslana`），免管理员权限，自带卸载项；卸载不会删除 `data\` 里的应用数据。 |
| `khaslana-v*-windows-x86_64.zip` | 进阶用户。解压即用的便携版，应用数据保存在 exe 同级 `data\` 目录，整个目录可搬移。请不要把便携版放在微信/QQ 接收文件或浏览器临时目录里直接运行——这些目录可能被定期清理（应用检测到后会提示并引导移出）。 |

## 功能特性

- 支持多仓库标签页、会话恢复和仓库快速打开。
- 支持克隆仓库，并默认递归克隆子模块。
- 支持工作区变更查看、暂存、取消暂存、丢弃更改和提交。
- 支持 fetch、pull、push，并显示当前分支 ahead / behind 状态。
- 支持本地分支、远端分支、标签、远端和贮藏管理。
- 支持子模块列表按需加载、同步父仓库记录版本，以及快进到子模块远端最新提交。
- 支持提交历史、提交图、提交文件列表和历史 diff。
- 支持本地 Reflog 查询、操作前后提交预览，以及从完整 OID 创建恢复分支。
- 支持 reset、revert、merge、checkout tag 等常用历史操作。
- 支持冲突识别和冲突处理入口。
- 支持 UTF-8、GB18030 / GBK、Big5 等 diff 编码识别和手动选择。
- 支持 HTTPS 用户名密码 / PAT、SSH Key、SSH agent，并将密文保存到系统 Keyring。
- 支持远端凭据绑定策略，工作流远端步骤可复用同一凭据机制。
- 支持全局网络代理设置：禁用代理、使用 Git / 环境变量代理、自定义 HTTP / HTTPS / SOCKS5 代理。
- 使用 SQLite 保存应用配置。
- 支持 JSON5 / JSONC 工作流模板，详见 [docs/workflows.md](docs/workflows.md)。

## 功能模块

| 模块 | 说明 |
| --- | --- |
| 仓库 | 打开本地仓库、克隆远端仓库、多标签页、会话恢复、刷新状态。 |
| 工作区 | 查看暂存区和修改区，支持单选、多选、范围选择、暂存、取消暂存、丢弃和提交。 |
| 分支 | 创建、删除、重命名、切换本地分支，支持从远端分支 checkout 并设置 upstream。 |
| 远端 | 管理远端地址，执行 fetch、pull、push，支持删除远端分支。 |
| 子模块 | 克隆时递归拉取子模块，按需查看子模块列表，支持同步记录版本、更新全部子模块到远端最新，以及更新单个子模块到远端最新。 |
| 贮藏 | 创建 stash、查看 stash 文件和 diff、apply、pop、drop。 |
| 历史 | 查看提交列表、提交图、引用标签、提交文件和历史 diff，支持分页加载。 |
| 恢复 | 查询本地 Reflog，检查操作前后的提交，并在保留当前工作区状态的情况下创建恢复分支。 |
| 冲突 | 识别冲突文件，进入冲突处理视图，并提供冲突解决相关操作入口。 |
| 凭据 | 管理 HTTPS / SSH 凭据，支持 Keyring 密文存储、凭据测试和远端绑定策略。 |
| 代理 | 为 clone、fetch、pull、push、子模块和工作流远端步骤应用统一代理策略。 |
| [工作流](docs/workflows.md) | 通过模板描述常用 Git 操作组合，支持变量、输入、预览和远端分支保护。 |

## Reflog 与分支恢复

本地 Reflog 记录引用指向的变化，可用于定位误 reset、rebase 等操作后从当前分支历史中消失、但仍保存在本地对象库中的提交。Khaslana 支持按 HEAD、当前分支、指定本地分支或全部本地来源查看记录，结合时间与关键词筛选，并预览目标提交的文件和 diff，再创建新的本地分支保留该提交。

使用步骤：

1. 打开仓库，进入「提交记录 → 恢复记录」。默认查看当前工作区 HEAD 近 30 天的记录。
2. 选择来源和时间范围，或搜索操作说明、引用、操作者、邮箱及 OID。选中记录后，分别检查「操作前」和「操作后」的完整 OID，确认要恢复哪一侧的提交。
3. 目标提交可用时，点击「查看提交」核对提交信息、文件和 diff。
4. 点击「从此提交创建分支」，填写未被占用的本地分支名称并确认。成功后可点击「查看恢复分支」只读浏览内容；低高度窗口中的同一入口位于「操作」菜单。

恢复只新增分支引用，不覆盖已有分支，不自动切换分支或执行 reset，保留当前 HEAD、索引和工作区修改。Reflog 不是永久备份：对象被清理后无法仅凭日志重建，未提交修改通常也不在记录中。详细用法和支持边界见 [Reflog 与恢复分支](docs/reflog-recovery.md)。

## AI 连接

在「设置 → AI 设置 → 连接」中启用 AI，并选择 OpenAI Chat Completions、OpenAI Responses 或 Anthropic Messages。填写 Base URL、模型和 API Key；地址不需要包含 `/chat/completions`、`/responses` 或 `/messages`，客户端会按所选协议追加。

展开「高级设置」可配置温度（默认 `0.3`）、最大输出（默认 `16384 tokens`）、请求超时（默认 `60 秒`）和思考强度。思考强度是自由文本，留空时不发送该参数；非空值由模型服务校验，不会自动启用 Anthropic 的 thinking 模式。部分模型不接受自定义温度，例如部分 Claude 模型仅支持 `1`，请按模型要求调整。

高级参数对提交生成、工作流模板生成、代码评审、冲突合并和工作流 Skill 统一生效。最大输出按单次请求或每轮计算，不是整个多轮任务的累计上限；长文件合并需要适当提高，预算不足会报截断错误，不会保存部分结果。流式请求的超时表示读取空闲超时，非流式请求表示整体请求超时；Skill 仍保留独立的整体运行预算。

「测试连接」使用当前未保存的表单，不自动保存配置，也不证明模型支持工具调用或任意输出预算。设置保存后会在重启时保留。

## 本地开发

环境要求：

- Rust 1.85+，项目使用 Rust 2024 edition。
- Windows 10 / 11 推荐；其他平台需确认 `gpui-pre`、系统 Keyring 和 libgit2 后端可用性。
- 可访问目标 Git 远端所需的网络和凭据环境。

Windows x64 MSVC 首次构建前准备预编译 OpenSSL：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/prepare-openssl.ps1
```

脚本从 [TaurusTLS OpenSSL Distribution](https://github.com/TaurusTLS-Developers/OpenSSL-Distribution/releases/tag/v3.5.9) 下载固定的 OpenSSL 3.5.9，校验 SHA-256 后解压到 `target/deps/`，不安装到系统。`.cargo/config.toml` 指向包内的静态库，供内置 libssh2 支持 Ed25519、ECDSA 和 RSA-SHA2；不从源码编译 OpenSSL。发布程序无需额外安装 OpenSSL 或携带其 DLL，分发时保留 `OPENSSL-LICENSE.txt`。需要自备库时，可用目标前缀环境变量覆盖目录，库必须与 x64 MSVC、静态 CRT 匹配。

本地运行：

```powershell
cargo run
```

构建发布版本：

```powershell
cargo build --release
```

Windows 下项目会通过 `.cargo/config.toml` 为 `x86_64-pc-windows-msvc` 启用静态 CRT 链接，发布给未安装 VC++ 运行库的机器时可减少 `VCRUNTIME140_1.dll` 缺失问题。

构建后的主程序位于：

```text
target/release/khaslana.exe
```

如需极致优化的发布级构建（fat LTO + 单编译单元，编译显著变慢）：

```powershell
cargo build --profile release-perf
```

产物位于 `target/release-perf/khaslana.exe`，官方发布流程（`.github/workflows/release.yml`）即使用该 profile。

本地一键打包安装器（自动完成 release-perf 构建、组装打包目录并调用 Inno Setup 编译 setup，需本机安装 [Inno Setup 7](https://jrsoftware.org/isdl.php)）：

```powershell
cargo setup
```

产物位于 `dist\khaslana-setup-v<版本>-windows-x86_64.exe`。

## 数据持久化

Khaslana 使用 `directories::ProjectDirs::from("", "", "Khaslana")` 获取系统配置目录，并将应用数据保存到 SQLite 数据库：

```text
khaslana.sqlite3
```

SQLite 中保存：

- 已打开仓库会话和当前激活仓库。
- diff 编码偏好。
- 远端凭据绑定策略。
- 网络代理设置。
- 凭据记录索引。

凭据密文不会写入 SQLite，而是保存在系统 Keyring 中；SQLite 只保存非敏感索引和匹配信息。

## 配置

### 凭据

Khaslana 支持三类凭据策略：

- 自动匹配：根据远端 URL 和凭据记录自动选择可用凭据。
- 不使用凭据：对指定远端跳过已保存凭据。
- 绑定指定记录：为仓库远端固定使用某条凭据记录。

HTTPS 凭据支持用户名 + 密码 / PAT；SSH 凭据支持私钥、passphrase 和 SSH agent。

在「设置 → 凭据」中可生成应用托管的 Ed25519 SSH 公私钥，无需安装 `ssh`、`ssh-keygen` 或运行 SSH agent。私钥和 `.pub` 公钥保存在当前应用数据目录的 `ssh/keys/<key-id>/` 中；推荐设置私钥口令，无口令需要显式选择。生成后复制公钥并添加到远端 Git 平台，再将密钥用于 SSH 凭据。私钥不写入 SQLite，保存凭据时口令沿用系统 Keyring。取消表单不会删除已生成密钥；删除凭据也不会删除密钥，仍被凭据引用的托管密钥不可直接删除。

没有 `known_hosts` 时，首次 SSH 连接会显示远端主机与 SHA256 指纹。请通过独立可信渠道核验后选择信任并保存；应用将主机信任记录保存在自身数据库，不写入系统 `known_hosts`。已信任主机更换密钥时会阻止连接，必须先独立核验，再管理旧信任记录。已有系统 `known_hosts` 仅只读检查；无法安全识别的格式会报错，不会静默绕过。应用的信任记录不会同步给外部 OpenSSH。

添加 HTTPS 凭据时可通过浏览器一键登录 GitHub / Gitee 自动获取登录名与令牌，免去手动录入 PAT。GitHub 走 OAuth Device Flow，无需额外服务即可使用；Gitee 走授权码流，因公开分发的客户端不能内置 `client_secret`，需要自部署一个持有 secret 的 broker，部署步骤见独立仓库 [khaslana-broker](https://github.com/Neglecton/khaslana-broker)。未部署 broker 时 Gitee 一键登录不可用，但仍可手动填写 PAT。

### 网络代理

代理设置为全局应用配置，支持：

| 模式 | 说明 |
| --- | --- |
| 不使用代理 | Git 操作显式不应用代理。 |
| 使用系统代理 | 使用 libgit2 自动代理语义：优先读取 Git 配置，其次读取 `http_proxy` / `https_proxy` 等环境变量；不读取系统 UI 代理或 PAC。 |
| 自定义代理 | 支持 HTTP、HTTPS、SOCKS5 / SOCKS5H URL；代理认证第一版通过 URL 内用户名密码表达。 |

自定义代理字段：

- HTTP 代理 URL：允许 `http://`、`https://`。
- HTTPS 代理 URL：允许 `http://`、`https://`。
- SOCKS5 代理 URL：允许 `socks5://`、`socks5h://`。

当前内置 SSH 后端不支持代理传输。若有效代理策略要求 SSH 使用代理，应用会在连接前报错，不会静默改为直连；未配置 SSH 有效代理时可直接连接。HTTP / HTTPS 远端继续使用上述代理设置。

## 开发命令

格式化：

```powershell
cargo fmt
```

检查：

```powershell
cargo check
```

运行：

```powershell
cargo run
```

开启性能日志运行：

```powershell
$env:KHASLANA_PERF_LOG='1'
cargo run
```

性能日志会输出仓库打开、metadata、status、历史分页、diff 解码等阶段耗时，字段包含阶段名、耗时和关键数量信息。定位大仓库卡顿时，建议先开启该日志；需要进一步分析时，Windows 可结合 Windows Performance Analyzer / PerfView，Rust 层可用 `cargo flamegraph` 或 `samply` 采样。

## 测试

运行全部测试：

```powershell
cargo test
```

运行指定测试：

```powershell
cargo test toolbar
cargo test submodule
cargo test proxy
```

## 许可证

Khaslana 使用 [Apache License 2.0](LICENSE) 开源协议。
