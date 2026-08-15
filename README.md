<p align="center">
  <img src="logo.png" alt="Khaslana logo" width="96" height="96">
</p>

<h1 align="center">Khaslana</h1>

<p align="center">
  一个使用 Rust、gpui-ce 和 libgit2 构建的轻量桌面 Git 客户端。
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> ·
  <a href="#功能特性">功能特性</a> ·
  <a href="#数据持久化">数据持久化</a> ·
  <a href="#测试">测试</a> ·
  <a href="#许可证">许可证</a>
</p>

## 简介

Khaslana 是一个面向日常开发工作流的桌面 Git 客户端。它使用 `gpui-ce` 和 `yororen_ui` 构建原生界面，使用 `git2` / libgit2 执行 Git 操作，并通过系统 Keyring 保存凭据密文。

项目目标不是替代所有 Git CLI 能力，而是把仓库打开、分支切换、暂存提交、远端同步、历史查看、凭据管理、代理设置、子模块更新和常用自动化工作流整合到一个轻量客户端里。

当前界面文案以中文为主，适合在 Windows 桌面环境下开发和使用。

## 功能特性

- 支持多仓库标签页、会话恢复和仓库快速打开。
- 支持克隆仓库，并默认递归克隆子模块。
- 支持工作区变更查看、暂存、取消暂存、丢弃更改和提交。
- 支持 fetch、pull、push，并显示当前分支 ahead / behind 状态。
- 支持本地分支、远端分支、标签、远端和贮藏管理。
- 支持子模块列表按需加载、同步父仓库记录版本，以及快进到子模块远端最新提交。
- 支持提交历史、提交图、提交文件列表和历史 diff。
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
| 冲突 | 识别冲突文件，进入冲突处理视图，并提供冲突解决相关操作入口。 |
| 凭据 | 管理 HTTPS / SSH 凭据，支持 Keyring 密文存储、凭据测试和远端绑定策略。 |
| 代理 | 为 clone、fetch、pull、push、子模块和工作流远端步骤应用统一代理策略。 |
| [工作流](docs/workflows.md) | 通过模板描述常用 Git 操作组合，支持变量、输入、预览和远端分支保护。 |

## 本地开发

环境要求：

- Rust 1.85+，项目使用 Rust 2024 edition。
- Windows 10 / 11 推荐；其他平台需确认 `gpui-ce`、系统 Keyring 和 libgit2 后端可用性。
- 可访问目标 Git 远端所需的网络和凭据环境。

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

SSH 远端默认不使用 HTTP / HTTPS 代理；仅在自定义模式配置 SOCKS5 代理时尝试应用 SOCKS5。

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
