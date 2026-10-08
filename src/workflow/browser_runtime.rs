//! 工作流内置 Edge MCP 的按需运行组件。下载只在用户点击后执行。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use crate::proxy::NetworkProxySettings;
use crate::{GitError, Result};

pub const SERVER_ID: &str = "browser.edge";
pub const LEGACY_SERVER_ID: &str = "edgeDemo";
pub const COMMAND_MARKER: &str = "builtin:browser.edge";
const NODE_VERSION: &str = "24.21.0";
const MCP_VERSION: &str = "0.0.82";
const NODE_ARCHIVE_SHA256: &str = "158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541";
const NODE_ARCHIVE_URL: &str = "https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-x64.zip";
const MAX_NODE_ARCHIVE_BYTES: u64 = 80 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 250 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 80 * 1024 * 1024;

#[derive(Deserialize)]
struct PackageLock {
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    version: String,
    resolved: Option<String>,
    integrity: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct RuntimeManifest {
    node_path: PathBuf,
    node_version: String,
    mcp_version: String,
}

fn runtime_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("workflow-browser-runtime")
}

fn mcp_dir(data_dir: &Path) -> PathBuf {
    runtime_dir(data_dir).join(format!("mcp-{MCP_VERSION}"))
}

fn cli_path(data_dir: &Path) -> PathBuf {
    mcp_dir(data_dir).join("node_modules/@playwright/mcp/cli.js")
}

pub fn ready(data_dir: &Path) -> bool {
    launch_command(data_dir).is_ok()
}

#[derive(Clone, Debug)]
pub struct BrowserRuntimeInfo {
    pub node_path: Option<PathBuf>,
    pub node_version: Option<String>,
    pub node_source: Option<&'static str>,
    pub mcp_ready: bool,
    pub system_checked: bool,
}

/// 普通页面只读取安装记录，不为展示配置启动任何进程。
pub fn inspect_cached(data_dir: &Path) -> BrowserRuntimeInfo {
    let mcp_ready = ready(data_dir);
    if mcp_ready {
        if let Ok(bytes) = fs::read(runtime_dir(data_dir).join("install.json")) {
            if let Ok(manifest) = serde_json::from_slice::<RuntimeManifest>(&bytes) {
                let managed = manifest.node_path.starts_with(runtime_dir(data_dir));
                return BrowserRuntimeInfo {
                    node_path: Some(manifest.node_path),
                    node_version: Some(manifest.node_version),
                    node_source: Some(if managed { "应用数据目录" } else { "本机 PATH" }),
                    mcp_ready,
                    system_checked: false,
                };
            }
        }
    }
    let detected = managed_node(data_dir).map(|(path, version)|
        (path, version, "应用数据目录"));
    match detected {
        Some((path, version, source)) => BrowserRuntimeInfo {
            node_path: Some(path), node_version: Some(version),
            node_source: Some(source), mcp_ready, system_checked: false,
        },
        None => BrowserRuntimeInfo { node_path: None, node_version: None,
            node_source: None, mcp_ready, system_checked: false },
    }
}

/// 仅主动检测时检查 PATH；安装流程另行检查，普通页面不调用。
pub fn inspect(data_dir: &Path) -> BrowserRuntimeInfo {
    let mut info = inspect_cached(data_dir);
    if !info.mcp_ready && let Some((path, version)) = system_node() {
        info.node_path = Some(path);
        info.node_version = Some(version);
        info.node_source = Some("本机 PATH");
    }
    info.system_checked = true;
    info
}

pub fn launch_command(data_dir: &Path) -> Result<(PathBuf, Vec<String>)> {
    let path = runtime_dir(data_dir).join("install.json");
    let manifest: RuntimeManifest = serde_json::from_slice(&fs::read(&path)
        .map_err(|_| GitError::Message("浏览器 MCP 运行组件未安装，请点击下载并启用".into()))?)
        .map_err(|_| GitError::Message("浏览器 MCP 运行组件记录无效，请重新下载".into()))?;
    if manifest.mcp_version != MCP_VERSION
        || !Version::parse(&manifest.node_version).is_ok_and(|version| version.major >= 20)
        || !manifest.node_path.is_file() || !cli_path(data_dir).is_file() {
        return Err(GitError::Message("浏览器 MCP 运行组件缺失或版本不兼容，请重新下载".into()));
    }
    Ok((manifest.node_path, vec![
        cli_path(data_dir).to_string_lossy().into_owned(),
        "--browser".into(), "msedge".into(), "--isolated".into(),
        "--timeout-navigation".into(), "25000".into(),
    ]))
}

pub fn direct_proxy_config(data_dir: &Path) -> Result<PathBuf> {
    const CONTENT: &str = r#"{"browser":{"launchOptions":{"args":["--no-proxy-server"]}}}"#;
    let path = runtime_dir(data_dir).join("direct-browser.json");
    match fs::read_to_string(&path) {
        Ok(content) if content == CONTENT => return Ok(path),
        Ok(_) => return Err(GitError::Message("浏览器 MCP 直连配置已被修改，请检查数据目录".into())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
        Err(err) => return Err(err.into()),
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let temp = runtime_dir(data_dir).join(format!(".direct-{}-{nonce}.json", std::process::id()));
    fs::write(&temp, CONTENT)?;
    if let Err(err) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        if fs::read_to_string(&path).ok().as_deref() != Some(CONTENT) {
            return Err(err.into());
        }
    }
    Ok(path)
}

/// 仅从 PATH 中选取满足版本要求的真实 node.exe。
fn system_node() -> Option<(PathBuf, String)> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let node = dir.join("node.exe");
        if !node.is_file() { continue; }
        let mut command = Command::new(&node);
        crate::process::hide_console(&mut command);
        let mut child = match command.arg("--version")
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
            Ok(child) => child,
            Err(_) => continue,
        };
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < Duration::from_secs(5) =>
                    std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        if !status.is_some_and(|status| status.success()) { continue; }
        let mut output = String::new();
        if child.stdout.take().and_then(|mut stdout| stdout.read_to_string(&mut output).ok()).is_none() {
            continue;
        }
        let version = output.trim().trim_start_matches('v').to_owned();
        if Version::parse(&version).is_ok_and(|version| version.major >= 20) {
            return Some((node, version));
        }
    }
    None
}

fn managed_node(data_dir: &Path) -> Option<(PathBuf, String)> {
    let root = runtime_dir(data_dir).join(format!("node-{NODE_VERSION}"));
    let node = root.join("node.exe");
    node.is_file().then(|| (node, NODE_VERSION.to_owned()))
}

pub fn install(data_dir: &Path, proxy_settings: &NetworkProxySettings,
    progress: impl Fn(String)) -> Result<()> {
    install_with_node_detection(data_dir, proxy_settings, progress, true)
}

#[cfg(test)]
pub(crate) fn install_without_system_node(data_dir: &Path, proxy_settings: &NetworkProxySettings,
    progress: impl Fn(String)) -> Result<()> {
    install_with_node_detection(data_dir, proxy_settings, progress, false)
}

fn install_with_node_detection(data_dir: &Path, proxy_settings: &NetworkProxySettings,
    progress: impl Fn(String), use_system_node: bool) -> Result<()> {
    if ready(data_dir) { return Ok(()); }
    proxy_settings.validate()?;
    let root = runtime_dir(data_dir);
    fs::create_dir_all(&root)?;
    progress("正在检查本机 Node 环境".into());
    let (node, node_version) = match (if use_system_node { system_node() } else { None })
        .or_else(|| managed_node(data_dir)) {
        Some(found) => found,
        None => {
            progress("正在下载 Node 运行环境".into());
            download_managed_node(data_dir, proxy_settings, &progress)?
        }
    };

    if !cli_path(data_dir).is_file() {
        progress("正在下载并安装 Playwright MCP 依赖".into());
        install_mcp(data_dir, proxy_settings, &progress)?;
    }
    if !cli_path(data_dir).is_file() {
        return Err(GitError::Message("浏览器 MCP 安装后缺少入口文件".into()));
    }
    let manifest = RuntimeManifest { node_path: node, node_version, mcp_version: MCP_VERSION.into() };
    let temp = root.join("install.json.tmp");
    fs::write(&temp, serde_json::to_vec(&manifest)
        .map_err(|err| GitError::Message(format!("浏览器 MCP 安装记录生成失败：{err}")))?)?;
    let installed = root.join("install.json");
    if installed.exists() { fs::remove_file(&installed)?; }
    fs::rename(&temp, installed)?;
    progress("浏览器 MCP 运行组件已就绪".into());
    Ok(())
}

fn download_managed_node(data_dir: &Path, proxy_settings: &NetworkProxySettings,
    progress: &impl Fn(String)) -> Result<(PathBuf, String)> {
    let root = runtime_dir(data_dir);
    let stage = stage_dir(&root, "node")?;
    let result = (|| -> Result<()> {
        let archive_path = stage.join("node.zip");
        download_node_archive(&archive_path, proxy_settings, progress)?;
        let mut archive = zip::ZipArchive::new(File::open(&archive_path)?)
            .map_err(|err| GitError::Message(format!("Node 压缩包无效：{err}")))?;
        let prefix = format!("node-v{NODE_VERSION}-win-x64/");
        let output_root = stage.join("node");
        let mut total = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)
                .map_err(|err| GitError::Message(format!("读取 Node 压缩包失败：{err}")))?;
            let Some(relative) = entry.name().strip_prefix(&prefix) else { continue };
            if relative != "node.exe" && relative != "LICENSE" {
                continue;
            }
            let relative = Path::new(relative);
            if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
                return Err(GitError::Message("Node 压缩包包含不安全路径".into()));
            }
            total = total.saturating_add(entry.size());
            if total > MAX_EXTRACTED_BYTES {
                return Err(GitError::Message("Node 解压体积超过限制".into()));
            }
            let destination = output_root.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&destination)?;
            } else {
                if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
                std::io::copy(&mut entry, &mut File::create(destination)?)?;
            }
        }
        if !output_root.join("node.exe").is_file() {
            return Err(GitError::Message("Node 压缩包缺少运行文件".into()));
        }
        let final_dir = root.join(format!("node-{NODE_VERSION}"));
        if final_dir.exists() { remove_stage(&root, &final_dir); }
        fs::rename(&output_root, &final_dir)?;
        Ok(())
    })();
    remove_stage(&root, &stage);
    result?;
    managed_node(data_dir).ok_or_else(|| GitError::Message("Node 运行环境安装不完整".into()))
}

fn download_node_archive(path: &Path, proxy_settings: &NetworkProxySettings,
    progress: &impl Fn(String)) -> Result<()> {
    let proxy_url = proxy_settings.proxy_url_for_target(NODE_ARCHIVE_URL);
    let proxy = proxy_url.as_deref().map(ureq::Proxy::new).transpose()
        .map_err(|err| GitError::Message(format!("代理配置无效，已取消 Node 下载：{err}")))?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .proxy(proxy).build().new_agent();
    let response = agent.get(NODE_ARCHIVE_URL).call()
        .map_err(|err| GitError::Message(format!("下载 Node 失败：{err}")))?;
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(path)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut last_report = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 { break; }
        count += read as u64;
        if count > MAX_NODE_ARCHIVE_BYTES {
            return Err(GitError::Message("Node 下载文件超过大小限制".into()));
        }
        file.write_all(&buffer[..read])?;
        hash.update(&buffer[..read]);
        if count - last_report >= 2 * 1024 * 1024 {
            progress(format!("正在下载 Node：{} MiB", count / 1024 / 1024));
            last_report = count;
        }
    }
    let digest = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if digest != NODE_ARCHIVE_SHA256 {
        return Err(GitError::Message("Node 下载校验失败，请重试".into()));
    }
    Ok(())
}

fn install_mcp(data_dir: &Path, proxy_settings: &NetworkProxySettings,
    progress: &impl Fn(String)) -> Result<()> {
    let root = runtime_dir(data_dir);
    let stage = stage_dir(&root, "mcp")?;
    let result = (|| -> Result<()> {
        fs::write(stage.join("package.json"), include_str!("../../resources/browser-mcp/package.json"))?;
        let lock_content = include_str!("../../resources/browser-mcp/package-lock.json");
        fs::write(stage.join("package-lock.json"), lock_content)?;
        let lock: PackageLock = serde_json::from_str(lock_content)
            .map_err(|err| GitError::Message(format!("内置 MCP 依赖清单无效：{err}")))?;
        if lock.packages.len() != 4 {
            return Err(GitError::Message("内置 MCP 依赖清单包含未知包".into()));
        }
        for (name, version) in [
            ("@playwright/mcp", MCP_VERSION),
            ("playwright", "1.64.0-alpha-1789764292000"),
            ("playwright-core", "1.64.0-alpha-1789764292000"),
        ] {
            let key = format!("node_modules/{name}");
            let package = lock.packages.get(&key).ok_or_else(||
                GitError::Message(format!("内置 MCP 依赖清单缺少 {name}")))?;
            if package.version != version {
                return Err(GitError::Message(format!("内置 MCP 依赖 {name} 版本不匹配")));
            }
            progress(format!("正在下载 {name} {version}"));
            install_locked_package(&stage, name, package, proxy_settings)?;
        }
        if !stage.join("node_modules/@playwright/mcp/cli.js").is_file() {
            return Err(GitError::Message("MCP 依赖安装后缺少入口文件".into()));
        }
        let final_dir = mcp_dir(data_dir);
        if final_dir.exists() { remove_stage(&root, &final_dir); }
        fs::rename(&stage, final_dir)?;
        Ok(())
    })();
    if result.is_err() { remove_stage(&root, &stage); }
    result
}

fn install_locked_package(stage: &Path, name: &str, package: &LockedPackage,
    proxy_settings: &NetworkProxySettings) -> Result<()> {
    let url = package.resolved.as_deref().ok_or_else(||
        GitError::Message(format!("内置 MCP 依赖 {name} 缺少下载地址")))?;
    let integrity = package.integrity.as_deref().ok_or_else(||
        GitError::Message(format!("内置 MCP 依赖 {name} 缺少校验值")))?;
    if !url.starts_with("https://registry.npmjs.org/") || !integrity.starts_with("sha512-") {
        return Err(GitError::Message(format!("内置 MCP 依赖 {name} 来源无效")));
    }
    let proxy_url = proxy_settings.proxy_url_for_target(url);
    let proxy = proxy_url.as_deref().map(ureq::Proxy::new).transpose()
        .map_err(|err| GitError::Message(format!("代理配置无效，已取消 MCP 下载：{err}")))?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .proxy(proxy).build().new_agent();
    let response = agent.get(url).call()
        .map_err(|err| GitError::Message(format!("下载 MCP 依赖 {name} 失败：{err}")))?;
    let archive_path = stage.join(format!("{}.tgz", name.replace('/', "-")));
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(&archive_path)?;
    let mut hash = Sha512::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 { break; }
        count += read as u64;
        if count > MAX_PACKAGE_BYTES {
            return Err(GitError::Message(format!("MCP 依赖 {name} 下载体积超过限制")));
        }
        file.write_all(&buffer[..read])?;
        hash.update(&buffer[..read]);
    }
    drop(file);
    if STANDARD.encode(hash.finalize()) != integrity.trim_start_matches("sha512-") {
        return Err(GitError::Message(format!("MCP 依赖 {name} 校验失败")));
    }
    let output_root = stage.join("node_modules").join(name);
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(&archive_path)?));
    let mut total = 0u64;
    for entry in archive.entries().map_err(|err|
        GitError::Message(format!("MCP 依赖 {name} 解压失败：{err}")))? {
        let mut entry = entry.map_err(|err|
            GitError::Message(format!("MCP 依赖 {name} 解压失败：{err}")))?;
        let path = entry.path().map_err(|err|
            GitError::Message(format!("MCP 依赖 {name} 路径无效：{err}")))?;
        let relative = path.strip_prefix("package").map_err(|_| 
            GitError::Message(format!("MCP 依赖 {name} 包含不安全路径")))?;
        if relative.as_os_str().is_empty() { continue; }
        if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
            return Err(GitError::Message(format!("MCP 依赖 {name} 包含不安全路径")));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err(GitError::Message(format!("MCP 依赖 {name} 解压体积超过限制")));
        }
        let destination = output_root.join(relative);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(destination)?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
            std::io::copy(&mut entry, &mut File::create(destination)?)?;
        } else {
            return Err(GitError::Message(format!("MCP 依赖 {name} 包含非普通文件")));
        }
    }
    fs::remove_file(archive_path)?;
    Ok(())
}

fn stage_dir(root: &Path, kind: &str) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path = root.join(format!(".{kind}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path)?;
    Ok(path)
}

fn remove_stage(root: &Path, path: &Path) {
    // 只清理数据目录内已解析的具体子目录，不跟随外部符号链接。
    if let (Ok(root), Ok(path)) = (root.canonicalize(), path.canonicalize()) {
        if path.parent() == Some(root.as_path()) { let _ = fs::remove_dir_all(path); }
    }
}

#[cfg(test)]
#[path = "../tests/browser_runtime.rs"]
mod tests;
