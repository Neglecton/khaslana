//! 官方插件引擎包与 Java 运行环境的安装管理（JLS-T4）。
//!
//! 设计见 `docs/code-understanding/jdtls-integration.md` 第 6.4/6.5 节：
//! 归档来源锁定内置固定清单（固定 URL + SHA-256 + 平台），域名白名单逐跳校验，
//! 下载 → 散列核对 → 有界解压 → 入口验证 → 原子发布 → 更新激活记录；
//! 失败保留已激活版本，卸载仅限本应用拥有且路径归属正确的包。
//!
//! 目录布局（以 `<数据目录>/lsp/` 为根）：
//! - `packages/<plugin_id>/<version>/<platform>/` 语义引擎
//! - `runtimes/java/<distribution>/<version>/<platform>/` 私有 JDK
//! - `staging/<install-id>/` 临时文件（成功或失败后清理）
//! - `registry.json` 按 plugin_id 记录激活的引擎与运行环境

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::proxy::NetworkProxySettings;

use super::plugins::{JAVA_JDTLS_PLUGIN, official_plugin_registry};

/// 当前编译平台对应的包平台键；内置清单只登记实测过的 `windows-x86_64`，
/// 其他平台不提供托管安装（保留手动配置路径）。
pub fn current_platform() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(target_os = "windows") {
        "windows-other"
    } else if cfg!(target_os = "macos") {
        "macos-other"
    } else {
        "linux-other"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageComponent {
    /// 语义引擎（JDT LS 发行归档）。
    Engine,
    /// Java 运行环境（完整 JDK）；独立记录但不是第二个插件。
    JavaRuntime,
}

impl PackageComponent {
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Engine => "packages",
            Self::JavaRuntime => "runtimes/java",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

/// 内置固定发行包清单条目。全部字段编译期锁定，禁止从模型/项目配置/下载包注入。
#[derive(Clone, Debug)]
pub struct EnginePackageSpec {
    /// 清单内唯一 ID，如 `jdtls-1.60.0.202606262232-windows-x86_64`。
    pub package_id: &'static str,
    /// 归属的官方插件 ID（运行环境也归属其使用的插件）。
    pub plugin_id: &'static str,
    pub component: PackageComponent,
    /// 组件键：引擎为 `jdtls`，运行环境为发行版名（如 `temurin`）。
    pub component_id: &'static str,
    /// 版本目录名与激活记录版本；引擎包与 `org.eclipse.jdt.ls.core_<version>.jar` 一致。
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub url: &'static str,
    /// 可选镜像源（项目自控发行通道，归档与 `url` 逐字节一致，官方 SHA-256 不变）；
    /// 下载先镜像后官方，镜像失败（404/网络/校验）自然滑落官方，对齐 update.rs 的多源模式。
    pub mirror_url: Option<&'static str>,
    pub sha256: &'static str,
    /// 归档精确字节数；下载后与实际字节数严格核对。
    pub size_bytes: u64,
    pub format: ArchiveFormat,
    /// 压缩归档读取上限（下载体积硬顶，防 Content-Length 缺失时无界下载）。
    pub archive_read_limit: u64,
    /// 解压展开总量上限（按实际写出字节计数）。
    pub extract_total_limit: u64,
    /// 单文件展开上限。
    pub extract_file_limit: u64,
    /// 归档条目数上限。
    pub max_entries: usize,
    /// 发布后包根内必须存在的入口（相对路径）。
    pub required_entries: &'static [&'static str],
    /// 引擎包兼容的服务 JDK 范围（运行环境包忽略）。
    pub min_service_jdk: u16,
    pub max_service_jdk: u16,
    pub license_name: &'static str,
    pub license_url: &'static str,
}

pub const LICENSE_JDT_LS: &str = "Eclipse Public License 2.0";
pub const LICENSE_JDT_LS_URL: &str =
    "https://projects.eclipse.org/projects/eclipse.jdt.ls";
pub const LICENSE_TEMURIN: &str = "GNU General Public License, version 2, with the Classpath Exception";
pub const LICENSE_TEMURIN_URL: &str = "https://adoptium.net/temurin/licenses/";

/// Eclipse 官方 JDT LS 固定 milestone 归档（2026-06-26 发布），
/// SHA-256 来自同目录官方 `.sha256` 伴随文件。
pub const JDT_LS_1_60_0_WINDOWS_X86_64: EnginePackageSpec = EnginePackageSpec {
    package_id: "jdtls-1.60.0.202606262232-windows-x86_64",
    plugin_id: "java-jdtls",
    component: PackageComponent::Engine,
    component_id: "jdtls",
    version: "1.60.0.202606262232",
    os: "windows",
    arch: "x86_64",
    url: "https://download.eclipse.org/jdtls/milestones/1.60.0/jdt-language-server-1.60.0-202606262232.tar.gz",
    mirror_url: Some(
        "https://cnb.cool/liuchenchen/LSP-Services/-/git/raw/main/jdt-language-server-1.60.0-202606262232.tar.gz",
    ),
    sha256: "e94c303d8198f977930803582738771fd18c52c5492878410bf222b1aa81ef1d",
    size_bytes: 50_925_681,
    format: ArchiveFormat::TarGz,
    archive_read_limit: 128 * 1024 * 1024,
    extract_total_limit: 1024 * 1024 * 1024,
    extract_file_limit: 64 * 1024 * 1024,
    max_entries: 30_000,
    required_entries: &["plugins", "config_win"],
    min_service_jdk: 21,
    max_service_jdk: 21,
    license_name: LICENSE_JDT_LS,
    license_url: LICENSE_JDT_LS_URL,
};

/// Eclipse Temurin JDK 21.0.12.1+1 Windows x64 官方 GitHub release 归档，
/// SHA-256 于 JLS-T0 实测核对（205,073,461 字节）。
pub const TEMURIN_JDK_21_WINDOWS_X86_64: EnginePackageSpec = EnginePackageSpec {
    package_id: "temurin-jdk-21.0.12.1+1-windows-x86_64",
    plugin_id: "java-jdtls",
    component: PackageComponent::JavaRuntime,
    component_id: "temurin",
    version: "21.0.12.1+1",
    os: "windows",
    arch: "x86_64",
    url: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip",
    mirror_url: Some(
        "https://cnb.cool/liuchenchen/LSP-Services/-/git/raw/main/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip",
    ),
    sha256: "f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e",
    size_bytes: 205_073_461,
    format: ArchiveFormat::Zip,
    archive_read_limit: 320 * 1024 * 1024,
    extract_total_limit: 1024 * 1024 * 1024,
    // 真实包校准：Temurin 21 的 lib/modules 约 140MB，是最大单文件。
    extract_file_limit: 256 * 1024 * 1024,
    max_entries: 30_000,
    required_entries: &["bin"],
    min_service_jdk: 21,
    max_service_jdk: 21,
    license_name: LICENSE_TEMURIN,
    license_url: LICENSE_TEMURIN_URL,
};

/// 全部内置固定清单；安装器只接受出现在这里（且平台匹配）的包。
pub fn builtin_package_specs() -> &'static [EnginePackageSpec] {
    &[JDT_LS_1_60_0_WINDOWS_X86_64, TEMURIN_JDK_21_WINDOWS_X86_64]
}

pub fn find_builtin_spec(package_id: &str) -> Option<&'static EnginePackageSpec> {
    builtin_package_specs()
        .iter()
        .find(|spec| spec.package_id == package_id)
}

/// 初始下载主机白名单；重定向目标另见 [`TRUSTED_REDIRECT_HOSTS`]。
/// `cnb.cool` 是项目自有的发行仓库（应用更新清单同源），承载官方归档的逐字节镜像。
pub const TRUSTED_ARCHIVE_HOSTS: &[&str] =
    &["download.eclipse.org", "github.com", "cnb.cool"];
/// 归档下载必要的重定向目标（GitHub release 资产跳转到对象存储；
/// `release-assets` 为当前实测域名，`objects` 为旧资产域名，一并保留）。
pub const TRUSTED_REDIRECT_HOSTS: &[&str] = &[
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("数据目录不可用，无法安装插件引擎包")]
    DataDirUnavailable,
    #[error("平台不支持托管安装（包平台 {spec}，当前 {current}）；请使用手动配置路径")]
    PlatformUnsupported { spec: String, current: String },
    #[error("包 {0} 不在内置固定清单中，已拒绝安装")]
    UnknownPackage(String),
    #[error("另一个安装任务正在进行；如确认无安装，可稍后重试或清理陈旧锁")]
    LockHeld,
    #[error("下载失败：{0}")]
    Network(String),
    #[error("代理配置无效或代理失败，已中止下载：{0}")]
    Proxy(String),
    #[error("下载源 {url} 不在可信域名白名单中，已中止")]
    UntrustedHost { url: String },
    #[error("重定向次数超过上限（{0}），已中止")]
    TooManyRedirects(usize),
    #[error("归档体积异常：实际 {actual} 字节，期望 {expected} 字节")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("SHA-256 校验失败：期望 {expected}，实际 {actual}")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("归档条目 {0} 路径不安全（绝对路径、盘符、.. 或越界），已拒绝")]
    ArchivePathUnsafe(String),
    #[error("归档包含符号链接/硬链接条目 {0}，已拒绝")]
    LinkEntryRejected(String),
    #[error("归档条目 {0} 类型不支持，已拒绝")]
    EntryTypeRejected(String),
    #[error("归档条目数超过上限 {limit}")]
    EntryCountLimit { limit: usize },
    #[error("解压展开量超过上限（累计 {total} 字节 > {limit} 字节）")]
    ExtractSizeLimit { total: u64, limit: u64 },
    #[error("文件 {path} 展开体积超过单文件上限 {limit} 字节")]
    ExtractFileLimit { path: String, limit: u64 },
    #[error("归档存在重复条目 {0}，已拒绝覆盖")]
    DuplicateEntry(String),
    #[error("归档损坏：条目 {path} 实际展开 {actual} 字节与声明 {declared} 字节不符")]
    CorruptEntry { path: String, declared: u64, actual: u64 },
    #[error("版本 {0} 已安装，重复安装被拒绝")]
    AlreadyInstalled(String),
    #[error("安装入口缺失：包根内缺少 {0}")]
    EntryMissing(String),
    #[error("引擎版本核对失败：包内 {found} 与清单期望 {expected} 不一致")]
    EngineVersionMismatch { found: String, expected: String },
    #[error("卸载目标 {0} 缺少本应用所有权标记，已拒绝删除")]
    NotOwnedByApp(String),
    #[error("要卸载的包 {0} 不存在或未在激活记录中")]
    UnknownInstalledPackage(String),
    #[error("卸载失败：{0}")]
    UninstallFailed(String),
    #[error("安装已被取消")]
    Cancelled,
    #[error("IO 错误：{0}")]
    Io(String),
}

impl From<std::io::Error> for InstallError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// 安装进度（阶段 + 字节/条目计数）。UI 侧自行节流，这里按读取块节流回调。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallPhase {
    Downloading,
    Verifying,
    Extracting,
    Publishing,
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub entries_done: usize,
    pub entries_total: usize,
}

/// 安装参数；字段集中维护，避免散落魔法数。
#[derive(Clone, Debug)]
pub struct InstallSettings {
    pub proxy: NetworkProxySettings,
    /// 网络读空闲期限；只要数据持续到达就不中断（ureq 分阶段超时语义）。
    pub read_idle_timeout: Duration,
    pub max_redirect_hops: usize,
    /// 安装锁陈旧判定阈值；超龄锁文件视为崩溃残留并接管。
    pub stale_lock_after: Duration,
    /// 测试注入：覆盖初始下载主机白名单（None = 内置清单）。
    pub allowed_archive_hosts: Option<Vec<String>>,
    /// 测试注入：覆盖重定向主机白名单（None = 内置清单）。
    pub allowed_redirect_hosts: Option<Vec<String>>,
}

impl Default for InstallSettings {
    fn default() -> Self {
        Self {
            proxy: NetworkProxySettings::default(),
            read_idle_timeout: Duration::from_secs(30),
            max_redirect_hops: 5,
            stale_lock_after: Duration::from_secs(2 * 60 * 60),
            allowed_archive_hosts: None,
            allowed_redirect_hosts: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledComponent {
    pub package_id: String,
    pub component_id: String,
    pub version: String,
    pub platform: String,
    /// 相对 lsp 根的包目录路径；激活与卸载都据此核验归属。
    pub path: String,
    pub installed_at_unix: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeSelection {
    /// 本应用托管安装的私有 JDK。
    Managed(InstalledComponent),
    /// 用户手动指定的外部 JDK（只记录路径，绝不卸载/修改）。
    External { java_home: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallRecord {
    pub plugin_id: String,
    pub engine: Option<InstalledComponent>,
    pub runtime: Option<RuntimeSelection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRegistry {
    pub version: u32,
    pub plugins: std::collections::BTreeMap<String, PluginInstallRecord>,
}

pub const REGISTRY_VERSION: u32 = 1;
/// 包目录内的所有权标记；卸载前必须校验，防止误删非本应用安装的目录。
const OWNERSHIP_MARKER: &str = ".khaslana-owned.json";

pub fn lsp_data_root() -> Result<PathBuf, InstallError> {
    crate::storage::active_data_dir()
        .map(|dir| dir.join("lsp"))
        .ok_or(InstallError::DataDirUnavailable)
}

pub fn registry_path(lsp_root: &Path) -> PathBuf {
    lsp_root.join("registry.json")
}

/// 引擎包的发布目录：`packages/<plugin_id>/<version>/<platform>/`。
pub fn engine_package_dir(lsp_root: &Path, plugin_id: &str, version: &str, platform: &str) -> PathBuf {
    lsp_root
        .join("packages")
        .join(plugin_id)
        .join(version)
        .join(platform)
}

/// 运行环境的发布目录：`runtimes/java/<distribution>/<version>/<platform>/`。
pub fn java_runtime_dir(
    lsp_root: &Path,
    distribution: &str,
    version: &str,
    platform: &str,
) -> PathBuf {
    lsp_root
        .join("runtimes")
        .join("java")
        .join(distribution)
        .join(version)
        .join(platform)
}

pub fn load_registry(lsp_root: &Path) -> Result<LspRegistry, InstallError> {
    let path = registry_path(lsp_root);
    if !path.exists() {
        return Ok(LspRegistry {
            version: REGISTRY_VERSION,
            plugins: Default::default(),
        });
    }
    let text = fs::read_to_string(&path)?;
    serde_json::from_str(&text).map_err(|error| {
        InstallError::Io(format!("激活记录 registry.json 解析失败：{error}"))
    })
}

fn save_registry(lsp_root: &Path, registry: &LspRegistry) -> Result<(), InstallError> {
    let path = registry_path(lsp_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(registry).map_err(|error| {
        InstallError::Io(format!("激活记录序列化失败：{error}"))
    })?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// 测试专用：直接写入激活记录（生产路径只能经 install/uninstall 修改）。
#[cfg(test)]
pub(crate) fn save_registry_for_test(
    lsp_root: &Path,
    registry: &LspRegistry,
) -> Result<(), InstallError> {
    save_registry(lsp_root, registry)
}

fn upsert_plugin_record<'a>(
    registry: &'a mut LspRegistry,
    plugin_id: &str,
) -> &'a mut PluginInstallRecord {
    registry.plugins.entry(plugin_id.to_string()).or_insert_with(
        || PluginInstallRecord {
            plugin_id: plugin_id.to_string(),
            engine: None,
            runtime: None,
        },
    )
}

/// 当前插件托管的 Java 运行环境 java_home（Managed 时返回其目录）。
pub fn managed_runtime_java_home(
    lsp_root: &Path,
    plugin_id: &str,
) -> Result<Option<PathBuf>, InstallError> {
    let registry = load_registry(lsp_root)?;
    Ok(registry
        .plugins
        .get(plugin_id)
        .and_then(|record| record.runtime.as_ref())
        .and_then(|selection| match selection {
            RuntimeSelection::Managed(component) => {
                Some(lsp_root.join(&component.path))
            }
            RuntimeSelection::External { .. } => None,
        }))
}

/// 安装结果快照（T5 设置页消费）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallSnapshot {
    pub engine: Option<InstalledComponent>,
    pub runtime: Option<RuntimeSelection>,
}

pub fn install_snapshot(
    lsp_root: &Path,
    plugin_id: &str,
) -> Result<InstallSnapshot, InstallError> {
    let registry = load_registry(lsp_root)?;
    Ok(registry
        .plugins
        .get(plugin_id)
        .map(|record| InstallSnapshot {
            engine: record.engine.clone(),
            runtime: record.runtime.clone(),
        })
        .unwrap_or(InstallSnapshot {
            engine: None,
            runtime: None,
        }))
}

struct InstallLockGuard {
    lock_path: PathBuf,
}

impl Drop for InstallLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// 跨进程互斥：整个安装流程（含 registry 写入）持有 `staging/install.lock`。
/// 锁文件内容记录持有者与时间戳；超过 `stale_after` 视为崩溃残留并接管。
fn acquire_install_lock(
    lsp_root: &Path,
    install_id: &str,
    stale_after: Duration,
) -> Result<InstallLockGuard, InstallError> {
    let staging = lsp_root.join("staging");
    fs::create_dir_all(&staging)?;
    let lock_path = staging.join("install.lock");
    let payload = serde_json::json!({
        "install_id": install_id,
        "pid": std::process::id(),
        "started_unix": unix_now(),
    });
    for _ in 0..2 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                file.write_all(payload.to_string().as_bytes())?;
                return Ok(InstallLockGuard { lock_path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let age = fs::metadata(&lock_path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map(|modified| unix_now().saturating_sub(modified.as_secs()))
                    .unwrap_or(0);
                if Duration::from_secs(age) >= stale_after {
                    // 陈旧锁接管：删除后重试一次 O_EXCL 创建。
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                return Err(InstallError::LockHeld);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(InstallError::LockHeld)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallOutcome {
    pub package_id: String,
    pub plugin_id: String,
    pub component: PackageComponent,
    pub version: String,
    /// 发布后的包根绝对路径。
    pub package_root: PathBuf,
    /// 引擎包解出的 `org.eclipse.jdt.ls.core_<version>` 版本串；运行环境为 None。
    pub engine_version: Option<String>,
}

/// 安装并激活一个内置清单包。成功后 registry 的对应组件记录被替换为本次版本。
///
/// 更新语义：不同版本发布到独立目录，安装失败不影响旧版本目录与激活记录；
/// 旧进程退出前收到的结果不会引用新版本目录（激活记录原子替换）。
pub fn install_package(
    spec: &EnginePackageSpec,
    lsp_root: &Path,
    settings: &InstallSettings,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<InstallOutcome, InstallError> {
    ensure_spec_supported(spec)?;
    let registry_before = load_registry(lsp_root)?;
    if let Some(record) = registry_before.plugins.get(spec.plugin_id) {
        let already = match spec.component {
            PackageComponent::Engine => record.engine.as_ref(),
            PackageComponent::JavaRuntime => record.runtime.as_ref().and_then(|selection| {
                match selection {
                    RuntimeSelection::Managed(component) => Some(component),
                    RuntimeSelection::External { .. } => None,
                }
            }),
        }
        .is_some_and(|component| component.version == spec.version);
        if already {
            return Err(InstallError::AlreadyInstalled(spec.version.to_string()));
        }
    }

    let install_id = format!(
        "{}-{}",
        spec.component_id,
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0)
    );
    let _lock = acquire_install_lock(lsp_root, &install_id, settings.stale_lock_after)?;
    let staging_dir = lsp_root.join("staging").join(&install_id);
    // 清理同名残留（极端情况下旧崩溃留下的空壳）后重建。
    let _ = fs::remove_dir_all(&staging_dir);
    fs::create_dir_all(&staging_dir)?;

    let result = install_package_inner(spec, lsp_root, &staging_dir, settings, cancel, on_progress);
    // 无论成败都清理本次 staging；失败保留已激活版本（registry 未动或已原子替换）。
    let _ = fs::remove_dir_all(&staging_dir);
    result
}

fn install_package_inner(
    spec: &EnginePackageSpec,
    lsp_root: &Path,
    staging_dir: &Path,
    settings: &InstallSettings,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<InstallOutcome, InstallError> {
    check_cancel(cancel)?;
    let part_path = staging_dir.join(format!("{}.part", spec.component_id));
    let downloaded = download_archive(spec, &part_path, settings, cancel, on_progress)?;

    // 大小/散列已在 download_archive 内逐源核对；此处进入解压阶段。
    emit(
        on_progress,
        InstallProgress {
            phase: InstallPhase::Verifying,
            downloaded_bytes: downloaded,
            total_bytes: spec.size_bytes,
            entries_done: 0,
            entries_total: 0,
        },
    );

    // 有界解压到 staging/<install-id>/extract/。
    check_cancel(cancel)?;
    let extract_dir = staging_dir.join("extract");
    fs::create_dir_all(&extract_dir)?;
    let entries_total = match spec.format {
        ArchiveFormat::Zip => extract_zip(&part_path, &extract_dir, spec, cancel, on_progress)?,
        ArchiveFormat::TarGz => {
            extract_tar_gz(&part_path, &extract_dir, spec, cancel, on_progress)?
        }
    };

    // 定位包根：唯一顶层目录即包根，否则 extract 本身。
    let package_root = locate_package_root(&extract_dir)?;

    // 入口验证。
    for entry in spec.required_entries {
        let path = package_root.join(entry);
        if !path.exists() {
            return Err(InstallError::EntryMissing(entry.to_string()));
        }
    }
    let engine_version = match spec.component {
        PackageComponent::Engine => {
            Some(verify_jdtls_engine_version(&package_root, spec.version)?)
        }
        PackageComponent::JavaRuntime => None,
    };

    // 原子发布：staging 内包根 rename 到版本目录（同卷 rename）。
    check_cancel(cancel)?;
    emit(
        on_progress,
        InstallProgress {
            phase: InstallPhase::Publishing,
            downloaded_bytes: downloaded,
            total_bytes: spec.size_bytes,
            entries_done: entries_total,
            entries_total,
        },
    );
    let target_dir = match spec.component {
        PackageComponent::Engine => {
            engine_package_dir(lsp_root, spec.plugin_id, spec.version, &spec_platform_key(spec))
        }
        PackageComponent::JavaRuntime => java_runtime_dir(
            lsp_root,
            spec.component_id,
            spec.version,
            &spec_platform_key(spec),
        ),
    };
    if let Some(parent) = target_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    if target_dir.exists() {
        return Err(InstallError::AlreadyInstalled(spec.version.to_string()));
    }
    std::fs::rename(package_root, &target_dir)?;
    write_ownership_marker(&target_dir, spec)?;

    // 更新激活记录（原子写 registry.json）。
    let component = InstalledComponent {
        package_id: spec.package_id.to_string(),
        component_id: spec.component_id.to_string(),
        version: spec.version.to_string(),
        platform: spec_platform_key(spec),
        path: target_dir
            .strip_prefix(lsp_root)
            .map_err(|error| {
                InstallError::Io(format!("发布目录偏离 lsp 根：{error}"))
            })?
            .to_string_lossy()
            .into_owned(),
        installed_at_unix: unix_now(),
    };
    let mut registry = load_registry(lsp_root)?;
    {
        let record = upsert_plugin_record(&mut registry, spec.plugin_id);
        match spec.component {
            PackageComponent::Engine => record.engine = Some(component.clone()),
            PackageComponent::JavaRuntime => {
                record.runtime = Some(RuntimeSelection::Managed(component.clone()))
            }
        }
    }
    registry.version = REGISTRY_VERSION;
    save_registry(lsp_root, &registry)?;

    emit(
        on_progress,
        InstallProgress {
            phase: InstallPhase::Completed,
            downloaded_bytes: downloaded,
            total_bytes: spec.size_bytes,
            entries_done: entries_total,
            entries_total,
        },
    );
    tracing::info!(
        target: "khaslana::lsp::install",
        "已安装插件包 {} 到 {}",
        spec.package_id,
        target_dir.display()
    );
    Ok(InstallOutcome {
        package_id: spec.package_id.to_string(),
        plugin_id: spec.plugin_id.to_string(),
        component: spec.component,
        version: spec.version.to_string(),
        package_root: target_dir,
        engine_version,
    })
}

fn spec_platform_key(spec: &EnginePackageSpec) -> String {
    format!("{}-{}", spec.os, spec.arch)
}

fn ensure_spec_supported(spec: &EnginePackageSpec) -> Result<(), InstallError> {
    // 官方插件的托管安装只接受内置固定清单；测试通过非官方 plugin_id 绕行注入 fake 源。
    if find_builtin_spec(spec.package_id).is_none() && is_builtin_plugin(spec.plugin_id) {
        return Err(InstallError::UnknownPackage(spec.package_id.to_string()));
    }
    let current = current_platform();
    let matches = (spec.os == "windows" && spec.arch == "x86_64" && current == "windows-x86_64")
        || cfg!(test); // 测试环境允许 fake spec 指向任意平台。
    if !matches {
        return Err(InstallError::PlatformUnsupported {
            spec: format!("{}-{}", spec.os, spec.arch),
            current: current.to_string(),
        });
    }
    // 官方插件的托管安装强制 HTTPS 归档；非官方 plugin_id 是测试注入通道，
    // 允许本地 fake HTTP 服务（127.0.0.1）。
    if is_builtin_plugin(spec.plugin_id) && !spec.url.starts_with("https://") {
        return Err(InstallError::UntrustedHost {
            url: spec.url.to_string(),
        });
    }
    Ok(())
}

fn is_builtin_plugin(plugin_id: &str) -> bool {
    official_plugin_registry().get(plugin_id).is_ok()
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), InstallError> {
    if cancel.load(Ordering::Relaxed) {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}

fn emit(on_progress: Option<&dyn Fn(InstallProgress)>, progress: InstallProgress) {
    if let Some(callback) = on_progress {
        callback(progress);
    }
}

fn build_download_agent(
    proxy_url: Option<String>,
    read_idle_timeout: Duration,
) -> Result<ureq::Agent, InstallError> {
    let proxy = proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| {
            ureq::Proxy::new(url)
                .map_err(|error| InstallError::Proxy(format!("{error}")))
        })
        .transpose()?;
    let builder = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(read_idle_timeout))
        .timeout_recv_body(Some(read_idle_timeout))
        .timeout_send_body(Some(read_idle_timeout))
        // 重定向逐跳校验：禁用自动跟随，手动处理每个 3xx。
        .max_redirects(0)
        .proxy(proxy);
    Ok(builder.build().new_agent())
}

fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    allowed
        .iter()
        .any(|candidate| candidate.to_ascii_lowercase() == host)
}

fn settings_archive_hosts(settings: &InstallSettings) -> Vec<String> {
    settings
        .allowed_archive_hosts
        .clone()
        .unwrap_or_else(|| TRUSTED_ARCHIVE_HOSTS.iter().map(|host| host.to_string()).collect())
}

fn settings_redirect_hosts(settings: &InstallSettings) -> Vec<String> {
    let mut hosts = settings
        .allowed_redirect_hosts
        .clone()
        .unwrap_or_else(|| {
            TRUSTED_REDIRECT_HOSTS
                .iter()
                .map(|host| host.to_string())
                .collect()
        });
    // 重定向允许回到初始归档主机（同站跳转）。
    hosts.extend(settings_archive_hosts(settings));
    hosts
}

fn validate_url_host(url: &str, allowed: &[String], hop: usize) -> Result<(), InstallError> {
    let host = extract_host(url).ok_or_else(|| InstallError::UntrustedHost {
        url: url.to_string(),
    })?;
    if !host_allowed(&host, allowed) {
        tracing::warn!(
            target: "khaslana::lsp::install",
            "拒绝白名单外的下载主机（第 {hop} 跳）：{host}"
        );
        return Err(InstallError::UntrustedHost {
            url: url.to_string(),
        });
    }
    Ok(())
}

fn extract_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    // 去掉端口（IPv6 字面量保底处理）。
    let host = if host.starts_with('[') {
        host.split(']').next()?.strip_prefix('[')?.to_string()
    } else {
        host.split(':').next()?.to_string()
    };
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn resolve_redirect(current: &str, location: &str) -> Result<String, InstallError> {
    if location.starts_with("https://") || location.starts_with("http://") {
        Ok(location.to_string())
    } else if location.starts_with('/') {
        // 同源绝对路径：authority + 原始 location（保留开头斜杠）。
        let scheme_end = current
            .find("://")
            .ok_or_else(|| InstallError::Network(format!("无法解析重定向目标 {location}")))?;
        let authority_start = scheme_end + 3;
        let authority_len = current[authority_start..]
            .find('/')
            .unwrap_or(current.len() - authority_start);
        Ok(format!(
            "{}{}{}",
            &current[..authority_start],
            &current[authority_start..authority_start + authority_len],
            location
        ))
    } else {
        Err(InstallError::Network(format!(
            "不支持的重定向目标格式：{location}"
        )))
    }
}

/// 流式下载归档到 `.part` 文件，同时计算 SHA-256。
/// 返回实际下载字节数；镜像源优先、官方源兜底；每个源内逐跳校验白名单；
/// 30 秒读空闲；取消在读边界生效。
fn download_archive(
    spec: &EnginePackageSpec,
    part_path: &Path,
    settings: &InstallSettings,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<u64, InstallError> {
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(mirror) = spec.mirror_url {
        candidates.push(mirror);
    }
    candidates.push(spec.url);
    let mut last_error: Option<InstallError> = None;
    for candidate in candidates {
        check_cancel(cancel)?;
        match download_from(
            candidate,
            part_path,
            spec,
            settings,
            cancel,
            on_progress,
        ) {
            Ok(downloaded) => {
                if downloaded != spec.size_bytes {
                    last_error = Some(InstallError::SizeMismatch {
                        expected: spec.size_bytes,
                        actual: downloaded,
                    });
                    continue;
                }
                let actual_sha = verify_sha256_file(part_path)?;
                if actual_sha != spec.sha256 {
                    tracing::warn!(
                        target: "khaslana::lsp::install",
                        "下载源 {} 散列不匹配，尝试下一来源",
                        candidate
                    );
                    last_error = Some(InstallError::Sha256Mismatch {
                        expected: spec.sha256.to_string(),
                        actual: actual_sha,
                    });
                    continue;
                }
                return Ok(downloaded);
            }
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(error) => {
                tracing::warn!(
                    target: "khaslana::lsp::install",
                    "下载源 {} 失败（{error}），尝试下一来源",
                    candidate
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| InstallError::Network("无可用下载源".to_string())))
}

/// 从单个 URL 下载（含逐跳重定向白名单与体积上限）。
fn download_from(
    base_url: &str,
    part_path: &Path,
    spec: &EnginePackageSpec,
    settings: &InstallSettings,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<u64, InstallError> {
    let mut current_url = base_url.to_string();
    let archive_hosts = settings_archive_hosts(settings);
    let redirect_hosts = settings_redirect_hosts(settings);
    let mut downloaded: u64 = 0;
    // 进度节流：按 256KB 粒度回调。
    let mut last_reported: u64 = 0;
    const PROGRESS_STEP: u64 = 256 * 1024;

    for hop in 0..=settings.max_redirect_hops {
        check_cancel(cancel)?;
        // 归档白名单只约束每个来源的初始 URL；重定向目标已在上一跳经 redirect 白名单校验。
        if hop == 0 {
            validate_url_host(&current_url, &archive_hosts, hop)?;
        }
        let proxy_url = settings.proxy.proxy_url_for_target(&current_url);
        let agent = build_download_agent(proxy_url, settings.read_idle_timeout)?;
        let response = agent.get(&current_url).call().map_err(|error| {
            let classified = classify_download_error(
                error,
                settings.proxy.proxy_url_for_target(&current_url),
            );
            match classified {
                InstallError::Network(text) => InstallError::Network(format!(
                    "{text}（请求 {current_url}）"
                )),
                other => other,
            }
        })?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| InstallError::Network(format!(
                    "第 {hop} 跳重定向缺少 Location 头"
                )))?;
            let next_url = resolve_redirect(&current_url, location)?;
            validate_url_host(&next_url, &redirect_hosts, hop + 1)?;
            tracing::info!(
                target: "khaslana::lsp::install",
                "下载重定向第 {} 跳：{} -> {}",
                hop + 1,
                current_url,
                next_url
            );
            current_url = next_url;
            continue;
        }
        if status != 200 {
            return Err(InstallError::Network(format!(
                "下载源返回 HTTP {status}"
            )));
        }
        // Content-Length 若存在则提前拦截超限响应。
        if let Some(length) = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
        {
            if length > spec.archive_read_limit {
                return Err(InstallError::ExtractSizeLimit {
                    total: length,
                    limit: spec.archive_read_limit,
                });
            }
        }

        let mut file = fs::File::create(part_path)?;
        let mut hasher = Sha256::new();
        let mut body = response.into_body();
        let mut reader = body.as_reader();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = reader.read(&mut buffer).map_err(|error| {
                InstallError::Network(format!("下载数据读取失败：{error}"))
            })?;
            if read == 0 {
                break;
            }
            downloaded += read as u64;
            if downloaded > spec.archive_read_limit {
                return Err(InstallError::ExtractSizeLimit {
                    total: downloaded,
                    limit: spec.archive_read_limit,
                });
            }
            file.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            if downloaded - last_reported >= PROGRESS_STEP {
                last_reported = downloaded;
                emit(
                    on_progress,
                    InstallProgress {
                        phase: InstallPhase::Downloading,
                        downloaded_bytes: downloaded,
                        total_bytes: spec.size_bytes,
                        entries_done: 0,
                        entries_total: 0,
                    },
                );
            }
        }
        return Ok(downloaded);
    }
    Err(InstallError::TooManyRedirects(settings.max_redirect_hops))
}

fn classify_download_error(error: ureq::Error, _proxy_url: Option<String>) -> InstallError {
    match error {
        ureq::Error::StatusCode(code) => {
            InstallError::Network(format!("下载源返回 HTTP {code}"))
        }
        other => InstallError::Network(other.to_string()),
    }
}

fn verify_sha256_file(path: &Path) -> Result<String, InstallError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

/// 归档条目相对路径安全化：拒绝绝对路径、盘符、`..`、非 UTF-8 名称；
/// 统一使用 `/` 分隔并压缩空段与 `.` 段。
fn sanitize_entry_path(raw: &str) -> Result<String, InstallError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(InstallError::ArchivePathUnsafe(raw.to_string()));
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(InstallError::ArchivePathUnsafe(raw.to_string()));
    }
    // Windows 盘符（`C:/` 或 `C:\` 形式在替换后是 `C:/`）。
    if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        return Err(InstallError::ArchivePathUnsafe(raw.to_string()));
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(InstallError::ArchivePathUnsafe(raw.to_string())),
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        // 纯 "."/"//" 条目视为目录占位，交由调用方按空路径处理。
        return Err(InstallError::ArchivePathUnsafe(raw.to_string()));
    }
    Ok(segments.join("/"))
}

struct ExtractBudget {
    max_entries: usize,
    file_limit: u64,
    total_limit: u64,
    entries_done: usize,
    total_written: u64,
    written: BTreeSet<String>,
}

impl ExtractBudget {
    fn new(spec: &EnginePackageSpec) -> Self {
        Self {
            max_entries: spec.max_entries,
            file_limit: spec.extract_file_limit,
            total_limit: spec.extract_total_limit,
            entries_done: 0,
            total_written: 0,
            written: BTreeSet::new(),
        }
    }

    /// 登记一个新文件条目：条目数限额 + 路径安全化 + 重复目标检查。
    fn begin_entry(&mut self, raw: &str) -> Result<String, InstallError> {
        self.entries_done += 1;
        if self.entries_done > self.max_entries {
            return Err(InstallError::EntryCountLimit {
                limit: self.max_entries,
            });
        }
        let rel = sanitize_entry_path(raw)?;
        if !self.written.insert(rel.clone()) {
            return Err(InstallError::DuplicateEntry(rel));
        }
        Ok(rel)
    }

    /// 登记目录条目：同一相对路径上目录与文件条目共存时宽容跳过。
    fn begin_dir_entry(&mut self, raw: &str) -> Result<Option<String>, InstallError> {
        let rel = sanitize_entry_path(raw).unwrap_or_default();
        if rel.is_empty() {
            return Ok(None);
        }
        if self.written.contains(&rel) {
            return Ok(None);
        }
        self.entries_done += 1;
        if self.entries_done > self.max_entries {
            return Err(InstallError::EntryCountLimit {
                limit: self.max_entries,
            });
        }
        self.written.insert(rel.clone());
        Ok(Some(rel))
    }

    /// 按实际写出字节的增量累计展开总量。
    fn account_increment(&mut self, increment: u64) -> Result<(), InstallError> {
        self.total_written += increment;
        if self.total_written > self.total_limit {
            return Err(InstallError::ExtractSizeLimit {
                total: self.total_written,
                limit: self.total_limit,
            });
        }
        Ok(())
    }

    fn check_file_limit(&self, rel: &str, written: u64) -> Result<(), InstallError> {
        if written > self.file_limit {
            return Err(InstallError::ExtractFileLimit {
                path: rel.to_string(),
                limit: self.file_limit,
            });
        }
        Ok(())
    }
}

/// 确保解压目标落在 extract 目录内（rename/create 前的最后防线）。
fn ensure_within_extract(extract_dir: &Path, rel: &str) -> Result<PathBuf, InstallError> {
    let target = extract_dir.join(rel);
    let canonical_base = extract_dir
        .canonicalize()
        .map_err(|error| InstallError::Io(error.to_string()))?;
    // 目标可能尚不存在，校验其父链。
    let mut check = target.clone();
    while !check.exists() {
        check = check
            .parent()
            .ok_or_else(|| InstallError::ArchivePathUnsafe(rel.to_string()))?
            .to_path_buf();
    }
    let canonical_check = check.canonicalize()?;
    if !canonical_check.starts_with(&canonical_base) {
        return Err(InstallError::ArchivePathUnsafe(rel.to_string()));
    }
    Ok(target)
}

fn extract_zip(
    archive_path: &Path,
    extract_dir: &Path,
    spec: &EnginePackageSpec,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<usize, InstallError> {
    let file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| InstallError::Io(format!("归档打开失败：{error}")))?;
    if archive.len() > spec.max_entries {
        return Err(InstallError::EntryCountLimit {
            limit: spec.max_entries,
        });
    }
    let mut budget = ExtractBudget::new(spec);
    for index in 0..archive.len() {
        check_cancel(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| InstallError::Io(format!("归档条目读取失败：{error}")))?;
        let raw_name = entry.name().to_string();
        if entry.is_dir() {
            // 目录条目末尾带 '/'；sanitize 会裁剪空段。
            let rel_path = budget.begin_dir_entry(raw_name.trim_end_matches('/'))?;
            if let Some(rel) = rel_path {
                let target = ensure_within_extract(extract_dir, &rel)?;
                fs::create_dir_all(target)?;
            }
            continue;
        }
        let rel = budget.begin_entry(&raw_name)?;
        // 符号链接：zip 的 unix mode 高位 S_IFLNK。
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(InstallError::LinkEntryRejected(raw_name));
            }
        }
        let declared = entry.size();
        if declared > budget.file_limit {
            return Err(InstallError::ExtractFileLimit {
                path: rel.clone(),
                limit: budget.file_limit,
            });
        }
        let target = ensure_within_extract(extract_dir, &rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&target)?;
        let mut copied: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            copied += read as u64;
            budget.check_file_limit(&rel, copied)?;
            budget.account_increment(read as u64)?;
            out.write_all(&buffer[..read])?;
        }
    }
    emit(
        on_progress,
        InstallProgress {
            phase: InstallPhase::Extracting,
            downloaded_bytes: 0,
            total_bytes: 0,
            entries_done: budget.entries_done,
            entries_total: budget.entries_done,
        },
    );
    Ok(budget.entries_done)
}

fn extract_tar_gz(
    archive_path: &Path,
    extract_dir: &Path,
    spec: &EnginePackageSpec,
    cancel: &AtomicBool,
    on_progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<usize, InstallError> {
    let file = fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut archive = tar::Archive::new(decoder);
    // 不还原 unix 权限/属主；Windows 上也无意义。
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    let mut budget = ExtractBudget::new(spec);
    let mut entries = archive
        .entries()
        .map_err(|error| InstallError::Io(format!("归档读取失败：{error}")))?;
    while let Some(entry) = entries.next() {
        check_cancel(cancel)?;
        let mut entry = entry.map_err(|error| {
            InstallError::Io(format!("归档条目读取失败：{error}"))
        })?;
        let raw_name = entry
            .path()
            .map_err(|error| InstallError::Io(error.to_string()))?
            .to_string_lossy()
            .into_owned();
        let entry_type = entry.header().entry_type();
        match entry_type {
            tar::EntryType::Regular => {}
            tar::EntryType::Directory => {
                let rel_path = budget.begin_dir_entry(raw_name.trim_end_matches('/'))?;
                if let Some(rel) = rel_path {
                    let target = ensure_within_extract(extract_dir, &rel)?;
                    fs::create_dir_all(target)?;
                }
                continue;
            }
            tar::EntryType::Symlink | tar::EntryType::Link => {
                return Err(InstallError::LinkEntryRejected(raw_name));
            }
            _ => return Err(InstallError::EntryTypeRejected(raw_name)),
        }
        let rel = budget.begin_entry(&raw_name)?;
        let declared = entry.header().size()?;
        if declared > budget.file_limit {
            return Err(InstallError::ExtractFileLimit {
                path: rel.clone(),
                limit: budget.file_limit,
            });
        }
        let target = ensure_within_extract(extract_dir, &rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&target)?;
        let mut copied: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            copied += read as u64;
            budget.check_file_limit(&rel, copied)?;
            budget.account_increment(read as u64)?;
            out.write_all(&buffer[..read])?;
        }
        if copied != declared {
            return Err(InstallError::CorruptEntry {
                path: rel,
                declared,
                actual: copied,
            });
        }
    }
    emit(
        on_progress,
        InstallProgress {
            phase: InstallPhase::Extracting,
            downloaded_bytes: 0,
            total_bytes: 0,
            entries_done: budget.entries_done,
            entries_total: budget.entries_done,
        },
    );
    Ok(budget.entries_done)
}

/// 解压结果中定位包根：唯一顶层目录即包根，否则 extract 目录本身。
fn locate_package_root(extract_dir: &Path) -> Result<PathBuf, InstallError> {
    let mut top_entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(extract_dir)? {
        top_entries.push(entry?.path());
    }
    if top_entries.len() == 1 && top_entries[0].is_dir() {
        Ok(top_entries.into_iter().next().expect("len checked"))
    } else {
        Ok(extract_dir.to_path_buf())
    }
}

/// 校验 JDT 引擎包：`plugins/org.eclipse.jdt.ls.core_<version>.jar` 必须与清单版本一致。
fn verify_jdtls_engine_version(
    package_root: &Path,
    expected: &str,
) -> Result<String, InstallError> {
    let plugins_dir = package_root.join("plugins");
    let mut found: Option<String> = None;
    for entry in fs::read_dir(&plugins_dir)? {
        let name = entry?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if let Some(version) = name
            .strip_prefix("org.eclipse.jdt.ls.core_")
            .and_then(|rest| rest.strip_suffix(".jar"))
        {
            found = Some(version.to_string());
            break;
        }
    }
    let found = found.ok_or_else(|| InstallError::EntryMissing(
        "plugins/org.eclipse.jdt.ls.core_*.jar".to_string(),
    ))?;
    if found != expected {
        return Err(InstallError::EngineVersionMismatch {
            found,
            expected: expected.to_string(),
        });
    }
    Ok(found)
}

fn write_ownership_marker(target_dir: &Path, spec: &EnginePackageSpec) -> Result<(), InstallError> {
    let marker = serde_json::json!({
        "package_id": spec.package_id,
        "plugin_id": spec.plugin_id,
        "component": spec.component,
        "installed_at_unix": unix_now(),
    });
    fs::write(
        target_dir.join(OWNERSHIP_MARKER),
        serde_json::to_vec_pretty(&marker).map_err(|error| InstallError::Io(error.to_string()))?,
    )?;
    Ok(())
}

fn read_ownership_marker(package_dir: &Path) -> Result<String, InstallError> {
    let path = package_dir.join(OWNERSHIP_MARKER);
    let text = fs::read_to_string(&path).map_err(|error| InstallError::NotOwnedByApp(format!(
        "{}（读取所有权标记失败：{error}）",
        package_dir.display()
    )))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        InstallError::NotOwnedByApp(format!("{}（标记解析失败：{error}）", package_dir.display()))
    })?;
    value
        .get("package_id")
        .and_then(|id| id.as_str())
        .map(|id| id.to_string())
        .ok_or_else(|| {
            InstallError::NotOwnedByApp(package_dir.display().to_string())
        })
}

/// 校验候选包目录归属：必须位于 lsp 根内、在对应组件目录下，且带所有权标记。
fn verify_owned_layout(
    lsp_root: &Path,
    component_dir: &Path,
    package_id: &str,
    installed_path: &str,
) -> Result<PathBuf, InstallError> {
    let candidate = lsp_root.join(installed_path);
    // 规范化检查：拒绝 `..`、绝对路径与越界 rel path。
    if Path::new(installed_path)
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(InstallError::NotOwnedByApp(installed_path.to_string()));
    }
    if !candidate.starts_with(lsp_root) || !candidate.starts_with(component_dir) {
        return Err(InstallError::NotOwnedByApp(installed_path.to_string()));
    }
    let marker_id = read_ownership_marker(&candidate)?;
    if marker_id != package_id {
        return Err(InstallError::NotOwnedByApp(installed_path.to_string()));
    }
    Ok(candidate)
}

/// 卸载一个已安装组件（引擎或托管运行环境）。
///
/// 保护：只删除 registry 有记录、目录带本应用所有权标记且位于组件目录内的包；
/// 操作系统层占用（进程打开着包内文件）时删除失败并转成明确错误。
/// 运行中的语义服务应先停止（T5 编排），再调用本函数。
pub fn uninstall_package(
    lsp_root: &Path,
    plugin_id: &str,
    package_id: &str,
) -> Result<(), InstallError> {
    let mut registry = load_registry(lsp_root)?;
    let record = registry
        .plugins
        .get_mut(plugin_id)
        .ok_or_else(|| InstallError::UnknownInstalledPackage(package_id.to_string()))?;

    enum Target {
        Engine,
        Runtime,
    }
    let (target, component) = if record
        .engine
        .as_ref()
        .is_some_and(|engine| engine.package_id == package_id)
    {
        (Target::Engine, record.engine.clone().expect("checked"))
    } else if record
        .runtime
        .as_ref()
        .is_some_and(|selection| matches!(selection, RuntimeSelection::Managed(managed) if managed.package_id == package_id))
    {
        (
            Target::Runtime,
            match record.runtime.clone().expect("checked") {
                RuntimeSelection::Managed(managed) => managed,
                RuntimeSelection::External { .. } => unreachable!("guarded above"),
            },
        )
    } else {
        return Err(InstallError::UnknownInstalledPackage(package_id.to_string()));
    };

    let component_root = match target {
        Target::Engine => lsp_root.join("packages").join(plugin_id),
        Target::Runtime => lsp_root.join("runtimes").join("java"),
    };
    let package_dir =
        verify_owned_layout(lsp_root, &component_root, package_id, &component.path)?;

    fs::remove_dir_all(&package_dir).map_err(|error| {
        InstallError::UninstallFailed(format!(
            "删除 {} 失败（若语义服务或其它进程仍在使用该版本，请先停止后重试）：{error}",
            package_dir.display()
        ))
    })?;

    match target {
        Target::Engine => record.engine = None,
        Target::Runtime => record.runtime = None,
    }
    save_registry(lsp_root, &registry)?;
    tracing::info!(
        target: "khaslana::lsp::install",
        "已卸载插件包 {package_id}"
    );
    Ok(())
}

/// 按官方插件描述核验引擎版本组合：版本在已测兼容清单（非开发构建）、平台匹配、
/// JDK 在兼容范围。未知组合拒绝托管激活，提示升级 Khaslana 或改用手动配置。
pub fn check_managed_activation(
    engine_version: &str,
    service_jdk_major: u16,
) -> Result<(), InstallError> {
    let descriptor = &JAVA_JDTLS_PLUGIN;
    let platform = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        ("windows", "x86_64")
    } else {
        ("other", "other")
    };
    let matched = descriptor.compatibility.iter().any(|entry| {
        entry.engine_version == engine_version
            && !entry.development_build
            && entry.os == platform.0
            && entry.arch == platform.1
            && (entry.min_service_jdk..=entry.max_service_jdk).contains(&service_jdk_major)
    });
    if matched {
        Ok(())
    } else {
        Err(InstallError::EngineVersionMismatch {
            found: format!("{engine_version}（JDK {service_jdk_major}）"),
            expected: "当前适配器已测兼容清单内组合".to_string(),
        })
    }
}

/// 当前内置清单中与当前平台匹配、可提供托管安装的包（T5 设置页展示用）。
pub fn available_specs_for_platform() -> Vec<&'static EnginePackageSpec> {
    builtin_package_specs()
        .iter()
        .filter(|spec| {
            spec.os == "windows"
                && spec.arch == "x86_64"
                && current_platform() == "windows-x86_64"
        })
        .collect()
}
