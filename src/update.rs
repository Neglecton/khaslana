//! 版本自动更新核心模块。
//!
//! 负责更新清单解析、版本比较、下载、SHA-256 校验和 staging 解压。
//! 不涉及 UI 状态和交互，由 `main.rs` 通过后台任务调用。

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use semver::Version;

use crate::proxy::NetworkProxySettings;
use crate::types::{GitError, Result};

// ── 数据结构 ──────────────────────────────────────────────────────────────

/// 从 `khaslana-update.json` 反序列化的更新清单。
#[derive(Clone, Debug, serde::Deserialize)]
pub struct UpdateManifest {
    pub schema: u32,
    pub channel: String,
    /// 语义版本号字符串（不带 `v`），例如 `"0.2.0"`。
    pub version: String,
    pub published_at: String,
    pub notes: String,
    /// 平台到下载资产的映射，键如 `"windows-x86_64"`。
    pub platforms: HashMap<String, UpdatePlatformAsset>,
}

/// 单个平台的下载资产信息。
#[derive(Clone, Debug, serde::Deserialize)]
pub struct UpdatePlatformAsset {
    /// CNB 主下载地址。
    pub archive_url: String,
    /// GitHub 兜底下载地址。
    pub fallback_archive_url: String,
    /// zip 文件的 SHA-256 小写十六进制摘要。
    pub sha256: String,
    /// zip 文件字节大小。
    pub size: u64,
}

/// 更新检查结果。
#[derive(Clone, Debug)]
pub enum UpdateCheckResult {
    /// 有可用更新。
    UpdateAvailable {
        manifest: UpdateManifest,
        asset: UpdatePlatformAsset,
    },
    /// 当前已是最新版本。
    UpToDate,
    /// 用户已跳过此版本。
    SkippedVersion,
}

// ── 公开函数 ──────────────────────────────────────────────────────────────

/// 当前应用的语义版本号，来自 Cargo 编译时注入。
pub fn current_version() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .unwrap_or(Version::new(0, 0, 0))
}

/// 默认 manifest 下载源列表，CNB 优先，GitHub 兜底。
pub fn default_manifest_sources() -> Vec<String> {
    vec![
        "https://cnb.cool/suhoan/khaslana-release/-/git/raw/master/khaslana-update.json".to_string(),
        "https://github.com/FuturePrayer/khaslana/releases/latest/download/khaslana-update.json".to_string(),
    ]
}

/// 检查是否有可用更新。
///
/// 逐源尝试 GET manifest JSON；第一个成功返回即停止。
/// 解析后与 `current_version()` 比较，跳过已忽略版本。
pub fn check_for_update(
    sources: &[String],
    preferences: &crate::storage::UpdatePreferences,
    proxy_settings: &NetworkProxySettings,
) -> Result<UpdateCheckResult> {
    let manifest = fetch_manifest(sources, proxy_settings)?;

    // 清单格式校验
    if manifest.schema != 1 {
        return Err(GitError::Message(format!(
            "不支持的更新清单格式（schema={}），当前仅支持 schema=1",
            manifest.schema
        )));
    }

    // 平台校验
    let asset = manifest.platforms.get("windows-x86_64").cloned();
    let asset = asset.ok_or_else(|| {
        GitError::Message("当前平台不支持自动更新（缺少 windows-x86_64 下载项）".to_string())
    })?;

    // 版本比较
    let remote_version: Version = manifest.version.parse().map_err(|err| {
        GitError::Message(format!("更新清单版本号格式错误：{err}"))
    })?;
    let current = current_version();

    if remote_version <= current {
        return Ok(UpdateCheckResult::UpToDate);
    }

    // 跳过版本检查
    if let Some(ref skipped) = preferences.skipped_version {
        let skipped: Version = skipped.parse().map_err(|err| {
            GitError::Message(format!("已跳过版本号格式错误：{err}"))
        })?;
        if remote_version == skipped {
            return Ok(UpdateCheckResult::SkippedVersion);
        }
    }

    Ok(UpdateCheckResult::UpdateAvailable { manifest, asset })
}

/// 下载更新 zip 到配置目录下的 `updates/downloads/`。
///
/// 先尝试 CNB 主地址，失败后尝试 GitHub 兜底地址。
/// 流式写入 `.part` 临时文件，完成后 rename 为正式文件。
/// 同时计算 SHA-256 摘要，返回 zip 路径和实际摘要。
///
/// 如果传入 `on_progress`，下载过程中会定期回调 `(已下载字节, 总字节)`。
pub fn download_update(
    asset: &UpdatePlatformAsset,
    config_dir: &Path,
    proxy_settings: &NetworkProxySettings,
    on_progress: Option<&dyn Fn(u64, u64)>,
) -> Result<(PathBuf, String)> {
    let downloads_dir = config_dir.join("updates").join("downloads");
    fs::create_dir_all(&downloads_dir)?;

    let zip_filename = format!(
        "khaslana-v{}-windows-x86_64.zip",
        asset.archive_url
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
    );
    let zip_path = downloads_dir.join(&zip_filename);
    let part_path = zip_path.with_extension("zip.part");

    // 尝试主 URL，失败后兜底
    let urls = [&asset.archive_url, &asset.fallback_archive_url];
    let mut last_err = None;

    for url in urls {
        match download_file(url, &part_path, proxy_settings, asset.size, on_progress) {
            Ok(digest_hex) => {
                // 成功，rename .part → 正式文件
                if zip_path.exists() {
                    fs::remove_file(&zip_path)?;
                }
                fs::rename(&part_path, &zip_path)?;
                return Ok((zip_path, digest_hex));
            }
            Err(err) => {
                last_err = Some(err);
                // 清理失败的部分文件
                let _ = fs::remove_file(&part_path);
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| GitError::Message("下载更新包失败：无可用源".to_string())))
}

/// 独立校验文件的 SHA-256 摘要。
///
/// `expected` 为小写十六进制字符串。
pub fn verify_sha256(path: &Path, expected: &str) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let actual = format!("{:x}", digest);
    Ok(actual == expected)
}

/// 解压更新 zip 到 staging 目录。
///
/// staging 目录为 `config_dir/updates/staging/v{version}/`。
/// 验证解压后包含 `khaslana.exe` 和 `khaslana_updater.exe`。
pub fn prepare_staging(zip_path: &Path, version: &str, config_dir: &Path) -> Result<PathBuf> {
    let staging_dir = config_dir
        .join("updates")
        .join("staging")
        .join(format!("v{version}"));

    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)?;
    }
    fs::create_dir_all(&staging_dir)?;

    // 解压 zip
    let mut archive = zip::ZipArchive::new(fs::File::open(zip_path)?)
        .map_err(|err| GitError::Message(format!("更新包解压失败：{err}")))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|err| {
            GitError::Message(format!("更新包读取条目失败：{err}"))
        })?;
        let entry_name = entry.name().to_string();

        // 安全检查：跳过绝对路径和路径遍历
        if entry_name.starts_with('/') || entry_name.contains("..") {
            continue;
        }

        let out_path = staging_dir.join(&entry_name);

        // 确保父目录存在
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            let mut out_file = fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
    }

    // 验证关键文件存在
    let exe_path = staging_dir.join("khaslana.exe");
    let updater_path = staging_dir.join("khaslana_updater.exe");

    if !exe_path.exists() {
        return Err(GitError::Message(
            "更新包缺少 khaslana.exe".to_string(),
        ));
    }
    if !updater_path.exists() {
        return Err(GitError::Message(
            "更新包缺少 khaslana_updater.exe".to_string(),
        ));
    }

    Ok(staging_dir)
}

// ── 内部函数 ──────────────────────────────────────────────────────────────

/// 从多个源逐个尝试 GET manifest JSON，返回第一个成功解析的结果。
fn fetch_manifest(
    sources: &[String],
    proxy_settings: &NetworkProxySettings,
) -> Result<UpdateManifest> {
    let mut last_err = None;

    for source in sources {
        let proxy_url = proxy_settings.proxy_url_for_target(source);
        let agent = build_agent(proxy_url, 15);

        match agent.get(source).call() {
            Ok(response) => {
                match response.into_json::<UpdateManifest>() {
                    Ok(manifest) => return Ok(manifest),
                    Err(err) => {
                        last_err = Some(GitError::Message(format!(
                            "更新清单解析失败（{source}）：{err}"
                        )));
                        continue;
                    }
                }
            }
            Err(err) => {
                last_err = Some(GitError::Message(format!(
                    "更新清单下载失败（{source}）：{err}"
                )));
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| GitError::Message("无法获取更新清单".to_string())))
}

/// 构建带代理和超时的 ureq Agent。
fn build_agent(proxy_url: Option<String>, timeout_secs: u64) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_read(std::time::Duration::from_secs(timeout_secs))
        .timeout_write(std::time::Duration::from_secs(timeout_secs));

    if let Some(url) = proxy_url {
        if let Ok(proxy) = ureq::Proxy::new(&url) {
            builder = builder.proxy(proxy);
        }
    }

    builder.build()
}

/// 流式下载文件并同时计算 SHA-256。
///
/// 返回 SHA-256 小写十六进制摘要。
/// `total_size` 用于进度回调；如果未知则传 0。
/// `on_progress` 每写入约 64KB 时回调 `(已下载字节, 总字节)`。
fn download_file(
    url: &str,
    dest: &Path,
    proxy_settings: &NetworkProxySettings,
    total_size: u64,
    on_progress: Option<&dyn Fn(u64, u64)>,
) -> Result<String> {
    let proxy_url = proxy_settings.proxy_url_for_target(url);
    // 下载可能较慢，使用更长超时（120s 读）
    let agent = build_agent(proxy_url, 120);

    let response = agent.get(url).call().map_err(|err| {
        GitError::Message(format!("下载更新包失败（{url}）：{err}"))
    })?;

    // 尝试从 Content-Length 获取总大小
    let content_length = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(total_size);

    let mut file = fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut reader = response.into_reader();
    let mut buf = [0u8; 8192];
    let mut downloaded: u64 = 0;
    let mut progress_counter: u64 = 0;
    // 每 8 次 read（约 64KB）回调一次进度，避免过于频繁
    const PROGRESS_REPORT_INTERVAL: u64 = 8;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 { break; }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        progress_counter += 1;
        if progress_counter >= PROGRESS_REPORT_INTERVAL {
            progress_counter = 0;
            if let Some(cb) = on_progress {
                cb(downloaded, content_length);
            }
        }
    }

    // 最终进度回调
    if let Some(cb) = on_progress {
        cb(downloaded, content_length);
    }

    let digest = hasher.finalize();
    Ok(format!("{:x}", digest))
}

// ── 单元测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
