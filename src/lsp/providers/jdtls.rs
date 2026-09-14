use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::code_index::ProjectContext;
use crate::lsp::plugins::JAVA_JDTLS_PLUGIN;
use crate::lsp::{LaunchSpec, SemanticOperation, ServerRequestPolicy, path_to_file_uri};

use super::{ProviderError, SemanticProvider};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualJdtLsConfig {
    pub java_home: PathBuf,
    pub jdtls_home: PathBuf,
    pub workspace_root: PathBuf,
    pub project_runtimes: Vec<JdtRuntime>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JdtRuntime {
    pub name: String,
    pub path: PathBuf,
    pub default: bool,
}

#[derive(Clone, Debug)]
pub struct JdtLsProvider {
    config: ManualJdtLsConfig,
    java_executable: PathBuf,
    launcher_jar: PathBuf,
    configuration_dir: PathBuf,
    engine_version: String,
}

impl JdtLsProvider {
    pub fn new(config: ManualJdtLsConfig) -> Result<Self, ProviderError> {
        let java_executable =
            config
                .java_home
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" });
        let javac_executable =
            config
                .java_home
                .join("bin")
                .join(if cfg!(windows) { "javac.exe" } else { "javac" });
        if !java_executable.is_file() || !javac_executable.is_file() {
            return Err(ProviderError::InvalidConfiguration(format!(
                "服务 JDK 不完整：{}",
                config.java_home.display()
            )));
        }
        let plugins_dir = config.jdtls_home.join("plugins");
        let launcher_jar = find_launcher(&plugins_dir)?;
        let core_jar = find_named_jar(&plugins_dir, "org.eclipse.jdt.ls.core_")?;
        let configuration_dir = config.jdtls_home.join(if cfg!(target_os = "windows") {
            "config_win"
        } else if cfg!(target_os = "macos") {
            "config_mac"
        } else {
            "config_linux"
        });
        if !configuration_dir.is_dir() {
            return Err(ProviderError::InvalidConfiguration(format!(
                "缺少平台配置目录：{}",
                configuration_dir.display()
            )));
        }
        std::fs::create_dir_all(&config.workspace_root)?;
        let engine_version = core_jar
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("org.eclipse.jdt.ls.core_"))
            .and_then(|name| name.strip_suffix(".jar"))
            .unwrap_or("unknown")
            .to_string();
        Ok(Self {
            config,
            java_executable,
            launcher_jar,
            configuration_dir,
            engine_version,
        })
    }

    pub fn development_snapshot(&self) -> bool {
        JAVA_JDTLS_PLUGIN
            .compatibility
            .iter()
            .any(|entry| entry.development_build)
    }

    fn project_workspace(&self, project: &ProjectContext) -> PathBuf {
        self.config.workspace_root.join(&project.project_key)
    }

    fn settings(&self) -> Value {
        let runtimes = if self.config.project_runtimes.is_empty() {
            vec![JdtRuntime {
                name: "JavaSE-21".to_string(),
                path: self.config.java_home.clone(),
                default: true,
            }]
        } else {
            self.config.project_runtimes.clone()
        };
        json!({
            "java": {
                "autobuild": {"enabled": false},
                "import": {
                    "gradle": {"enabled": true},
                    "maven": {"enabled": true}
                },
                "configuration": {
                    "updateBuildConfiguration": "automatic",
                    "runtimes": runtimes.iter().map(|runtime| json!({
                        "name": runtime.name,
                        "path": runtime.path.to_string_lossy(),
                        "default": runtime.default
                    })).collect::<Vec<_>>()
                },
                "format": {"enabled": false},
                "saveActions": {"organizeImports": false},
                "signatureHelp": {"enabled": false}
            }
        })
    }
}

impl SemanticProvider for JdtLsProvider {
    fn plugin_id(&self) -> &'static str {
        "java-jdtls"
    }

    fn adapter_id(&self) -> &'static str {
        "jdtls"
    }

    fn engine_version(&self) -> &str {
        &self.engine_version
    }

    fn implemented_operations(&self) -> BTreeSet<SemanticOperation> {
        SemanticOperation::ALL.into_iter().collect()
    }

    fn launch_spec(&self, project: &ProjectContext) -> Result<LaunchSpec, ProviderError> {
        let workspace = self.project_workspace(project);
        std::fs::create_dir_all(&workspace)?;
        Ok(LaunchSpec {
            executable: self.java_executable.clone(),
            args: vec![
                "-Declipse.application=org.eclipse.jdt.ls.core.id1".to_string(),
                "-Dosgi.bundles.defaultStartLevel=4".to_string(),
                "-Declipse.product=org.eclipse.jdt.ls.core.product".to_string(),
                "-Dlog.level=INFO".to_string(),
                "-Xmx1G".to_string(),
                "--add-modules=ALL-SYSTEM".to_string(),
                "--add-opens".to_string(),
                "java.base/java.util=ALL-UNNAMED".to_string(),
                "--add-opens".to_string(),
                "java.base/java.lang=ALL-UNNAMED".to_string(),
                "-jar".to_string(),
                self.launcher_jar.to_string_lossy().into_owned(),
                "-configuration".to_string(),
                self.configuration_dir.to_string_lossy().into_owned(),
                "-data".to_string(),
                workspace.to_string_lossy().into_owned(),
            ],
            env: BTreeMap::from([
                (
                    "JAVA_HOME".to_string(),
                    self.config.java_home.to_string_lossy().into_owned(),
                ),
                // 隔离本应用启动的 Gradle daemon 与缓存，不碰用户共享 daemon。
                (
                    "GRADLE_USER_HOME".to_string(),
                    workspace
                        .join("gradle-user-home")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ]),
            // JDT stdio 模式不能继承会切换到 socket 传输的变量。
            env_remove: vec!["CLIENT_PORT".to_string(), "CLIENT_HOST".to_string()],
            current_dir: Some(PathBuf::from(&project.canonical_root)),
        })
    }

    fn initialize_params(&self, project: &ProjectContext) -> Result<Value, ProviderError> {
        let root_uri = path_to_file_uri(Path::new(&project.canonical_root))
            .map_err(ProviderError::InvalidConfiguration)?;
        Ok(json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "Khaslana", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": project.project_key}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16", "utf-8"]},
                "workspace": {
                    "configuration": true,
                    "didChangeWatchedFiles": {"dynamicRegistration": true},
                    "workspaceFolders": true
                },
                "textDocument": {
                    "definition": {"dynamicRegistration": false, "linkSupport": true},
                    "implementation": {"dynamicRegistration": false, "linkSupport": true},
                    "references": {"dynamicRegistration": false},
                    "documentSymbol": {"dynamicRegistration": false, "hierarchicalDocumentSymbolSupport": true},
                    "callHierarchy": {"dynamicRegistration": false}
                },
                "window": {"workDoneProgress": true}
            },
            "initializationOptions": {
                "settings": self.settings(),
                "extendedClientCapabilities": {
                    "progressReportProvider": true,
                    "classFileContentsSupport": true
                },
                "bundles": []
            },
            "trace": "off"
        }))
    }

    fn request_policy(&self) -> ServerRequestPolicy {
        ServerRequestPolicy::new(self.settings())
    }
}

fn find_named_jar(plugins_dir: &Path, prefix: &str) -> Result<PathBuf, ProviderError> {
    let mut matches = std::fs::read_dir(plugins_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".jar"))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.pop().ok_or_else(|| {
        ProviderError::InvalidConfiguration(format!(
            "{} 中缺少 {prefix}*.jar",
            plugins_dir.display()
        ))
    })
}

fn find_launcher(plugins_dir: &Path) -> Result<PathBuf, ProviderError> {
    find_named_jar(plugins_dir, "org.eclipse.equinox.launcher_")
}
