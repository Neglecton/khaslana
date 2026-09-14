//! Java 运行环境检测（JLS-T4，设计 §6.3）。
//!
//! 检测顺序固定：用户显式路径 → 本应用托管安装的私有 JDK → JAVA_HOME → PATH。
//! 只检查这些有限候选，不做全盘搜索；每个候选校验完整 JDK 布局（java/javac）
//! 并限时探测版本与架构；PATH 候选排除来自当前项目的可执行文件。
//! 本模块只读系统状态，不修改任何环境变量。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use thiserror::Error;

/// 版本探测超时（单候选）。设计要求“运行版本检查使用绝对路径并限时”。
pub const JDK_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JdkSource {
    /// 用户在设置里显式选择的路径。
    Explicit,
    /// 本应用托管安装且激活的私有 JDK（`runtimes/java/...`）。
    Managed,
    /// 系统 `JAVA_HOME` 环境变量。
    JavaHome,
    /// 系统 `PATH` 中发现的 `java`。
    Path,
}

impl JdkSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "手动指定",
            Self::Managed => "Khaslana 托管安装",
            Self::JavaHome => "系统 JAVA_HOME",
            Self::Path => "系统 PATH",
        }
    }
}

/// 一次限时版本探测的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JdkProbe {
    pub version: Option<String>,
    pub arch: Option<String>,
    pub error: Option<String>,
}

impl JdkProbe {
    pub fn major_version(&self) -> Option<u16> {
        parse_java_major(self.version.as_deref()?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JdkCandidate {
    pub source: JdkSource,
    pub java_home: PathBuf,
    /// 布局完整（java 与 javac 都存在），可作为完整 JDK 使用。
    pub complete: bool,
    pub probe: JdkProbe,
}

impl JdkCandidate {
    /// 是否满足指定引擎包的 JDK 范围。
    pub fn supports_jdk_range(&self, min: u16, max: u16) -> bool {
        self.probe
            .major_version()
            .is_some_and(|major| (min..=max).contains(&major))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JdkDetectionError {
    #[error("显式指定的 JDK 路径无效：{0}")]
    InvalidExplicitPath(String),
}

pub fn java_executable_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

pub fn javac_executable_name() -> &'static str {
    if cfg!(windows) {
        "javac.exe"
    } else {
        "javac"
    }
}

/// 候选 java_home 的完整 JDK 布局检查。
pub fn layout_complete(java_home: &Path) -> bool {
    java_home.join("bin").join(java_executable_name()).is_file()
        && java_home.join("bin").join(javac_executable_name()).is_file()
}

/// 解析 `java.version` 字符串的主版本号：`21.0.12` → 21，`1.8.0_392` → 8。
pub fn parse_java_major(version: &str) -> Option<u16> {
    let trimmed = version.trim();
    let base = trimmed
        .split(['-', '+', '_'])
        .next()
        .unwrap_or(trimmed);
    let major_part = if let Some(rest) = base.strip_prefix("1.") {
        rest.split('.').next()?
    } else {
        base.split('.').next()?
    };
    major_part.parse::<u16>().ok()
}

/// 运行 `<java_home>/bin/java -XshowSettings:properties -version` 并限时解析
/// `java.version` 与 `os.arch`。输出走 stderr（`-version` 的标准行为）。
pub fn probe_java_runtime(java_home: &Path, timeout: Duration) -> JdkProbe {
    let java = java_home.join("bin").join(java_executable_name());
    if !java.is_file() {
        return JdkProbe {
            error: Some(format!("未找到 {}", java.display())),
            ..JdkProbe::default()
        };
    }
    let mut child = match Command::new(&java)
        .arg("-XshowSettings:properties")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return JdkProbe {
                error: Some(format!("启动 {} 失败：{error}", java.display())),
                ..JdkProbe::default()
            }
        }
    };
    // 输出只有几 KB，先读完再等待，避免管道写满阻塞。
    let stderr_output = child
        .stderr
        .take()
        .map(|mut pipe| {
            use std::io::Read;
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            buf
        })
        .unwrap_or_default();
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return JdkProbe {
            error: Some(format!(
                "版本探测超过 {}s 限时，已终止",
                timeout.as_secs()
            )),
            ..JdkProbe::default()
        };
    };
    if !status.success() {
        return JdkProbe {
            error: Some(format!("java -version 退出码异常：{status}")),
            ..JdkProbe::default()
        };
    }
    parse_show_settings_output(&stderr_output)
}

/// 解析 `-XshowSettings:properties` 的属性行。
pub fn parse_show_settings_output(output: &str) -> JdkProbe {
    let mut probe = JdkProbe::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "java.version" {
            probe.version = Some(value.to_string());
        } else if key == "os.arch" {
            probe.arch = Some(normalize_arch(value));
        }
    }
    probe
}

/// 规范化架构名：JDK 可能报 `amd64`（Windows x64 常见）或 `x86_64`。
pub fn normalize_arch(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x86-64" => "x86_64".to_string(),
        other => other.to_string(),
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// 发现候选时使用的环境快照；测试可注入固定值而无需修改进程环境变量。
#[derive(Clone, Debug, Default)]
pub struct JdkDiscoveryEnv {
    pub java_home: Option<PathBuf>,
    pub path_entries: Vec<PathBuf>,
}

/// 从当前进程环境读取 JAVA_HOME 与 PATH（生产入口）。
pub fn system_discovery_env() -> JdkDiscoveryEnv {
    JdkDiscoveryEnv {
        java_home: std::env::var_os("JAVA_HOME").map(PathBuf::from),
        path_entries: std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .collect(),
    }
}

/// 按固定顺序发现可用 JDK 候选：explicit → managed → JAVA_HOME → PATH。
///
/// `project_root` 用于排除当前项目目录内的 PATH 候选（设计 §6.3）；
/// `probe` 支持测试注入假探测。同一 java_home 只保留优先级最高的来源。
pub fn discover_java_homes(
    explicit: Option<&Path>,
    managed: Option<&Path>,
    project_root: Option<&Path>,
    env: &JdkDiscoveryEnv,
    probe: &dyn Fn(&Path) -> JdkProbe,
) -> Result<Vec<JdkCandidate>, JdkDetectionError> {
    let mut candidates: Vec<JdkCandidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let project_canonical = project_root.map(canonical_or_self);

    let push = |source: JdkSource,
                    java_home: PathBuf,
                    candidates: &mut Vec<JdkCandidate>,
                    seen: &mut Vec<PathBuf>| {
        let canonical = canonical_or_self(&java_home);
        if seen.iter().any(|path| *path == canonical) {
            return;
        }
        if let Some(root) = &project_canonical {
            if canonical.starts_with(root) {
                // 项目内提供的可执行文件不是可信运行环境（设计 §6.3）。
                return;
            }
        }
        seen.push(canonical);
        let complete = layout_complete(&java_home);
        let probed = if java_home.join("bin").join(java_executable_name()).is_file() {
            probe(&java_home)
        } else {
            JdkProbe {
                error: Some("缺少 java 可执行文件".to_string()),
                ..JdkProbe::default()
            }
        };
        candidates.push(JdkCandidate {
            source,
            java_home,
            complete,
            probe: probed,
        });
    };

    if let Some(path) = explicit {
        if !path.is_dir() {
            // 显式路径无效时明确报错，不静默换另一套 JDK。
            return Err(JdkDetectionError::InvalidExplicitPath(
                path.display().to_string(),
            ));
        }
        push(JdkSource::Explicit, path.to_path_buf(), &mut candidates, &mut seen);
    }
    if let Some(path) = managed {
        if path.is_dir() {
            push(JdkSource::Managed, path.to_path_buf(), &mut candidates, &mut seen);
        }
    }
    if let Some(java_home) = env
        .java_home
        .clone()
        .filter(|path| path.is_dir())
    {
        push(JdkSource::JavaHome, java_home, &mut candidates, &mut seen);
    }
    for entry in &env.path_entries {
        let java = entry.join(java_executable_name());
        if !java.is_file() {
            continue;
        }
        // `<dir>/java.exe` → java_home = `<dir>/..`。
        let Some(java_home) = java.parent().and_then(Path::parent) else {
            continue;
        };
        push(JdkSource::Path, java_home.to_path_buf(), &mut candidates, &mut seen);
    }
    Ok(candidates)
}

/// 生产探测入口：限时 10 秒。
pub fn default_probe(java_home: &Path) -> JdkProbe {
    probe_java_runtime(java_home, JDK_PROBE_TIMEOUT)
}
