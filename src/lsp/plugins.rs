use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DESCRIPTOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticOperation {
    Definition,
    Implementations,
    References,
    IncomingCalls,
    OutgoingCalls,
}

impl SemanticOperation {
    pub const ALL: [Self; 5] = [
        Self::Definition,
        Self::Implementations,
        Self::References,
        Self::IncomingCalls,
        Self::OutgoingCalls,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Stdio,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LspPluginDescriptor {
    pub plugin_id: &'static str,
    pub display_name: &'static str,
    pub descriptor_version: u32,
    pub adapter_id: &'static str,
    pub languages: &'static [&'static str],
    pub operations: &'static [SemanticOperation],
    pub transport: TransportKind,
    pub runtime: &'static str,
    pub engine_packages: &'static [&'static str],
    pub compatibility: &'static [EngineCompatibility],
}

/// 兼容清单条目：`development_build` 标记 T0 手动开发例外（本地 snapshot），
/// 托管安装只允许 `false` 的正式发行组合。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineCompatibility {
    pub engine_version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub min_service_jdk: u16,
    pub max_service_jdk: u16,
    pub development_build: bool,
}

/// 正式发行组合（JLS-T4）：Eclipse milestones 1.60.0 固定归档，JLS-T4 复跑核心查询后收录。
const JDT_1_60_0_RELEASE: EngineCompatibility = EngineCompatibility {
    engine_version: "1.60.0.202606262232",
    os: "windows",
    arch: "x86_64",
    min_service_jdk: 21,
    max_service_jdk: 21,
    development_build: false,
};

/// T0 手动开发例外：本地 snapshot（带散列），只允许显式指定路径的开发环境，
/// 不进入托管发行清单。
const JDT_1_61_0_SNAPSHOT: EngineCompatibility = EngineCompatibility {
    engine_version: "1.61.0.202609031315",
    os: "windows",
    arch: "x86_64",
    min_service_jdk: 21,
    max_service_jdk: 21,
    development_build: true,
};

const JDT_COMPATIBILITY: &[EngineCompatibility] =
    &[JDT_1_60_0_RELEASE, JDT_1_61_0_SNAPSHOT];

/// 托管安装包 ID（对应 `src/lsp/install.rs` 内置固定清单）。
const MANAGED_ENGINE_PACKAGES: &[&str] = &[
    "jdtls-1.60.0.202606262232-windows-x86_64",
    "temurin-jdk-21.0.12.1+1-windows-x86_64",
];

pub static JAVA_JDTLS_PLUGIN: LspPluginDescriptor = LspPluginDescriptor {
    plugin_id: "java-jdtls",
    display_name: "Java语言支持（JDT LS）",
    descriptor_version: DESCRIPTOR_SCHEMA_VERSION,
    adapter_id: "jdtls",
    languages: &["java"],
    operations: &SemanticOperation::ALL,
    transport: TransportKind::Stdio,
    runtime: "java>=21",
    // T4：固定正式发行包 + 私有 JDK 的内置清单 ID；snapshot 不在其中。
    engine_packages: MANAGED_ENGINE_PACKAGES,
    compatibility: JDT_COMPATIBILITY,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginRegistryError {
    #[error("未知插件描述版本：{0}")]
    UnknownSchema(u32),
    #[error("未知 LSP 适配器：{0}")]
    UnknownAdapter(String),
    #[error("插件 ID 无效：{0}")]
    InvalidPluginId(String),
    #[error("重复插件 ID：{0}")]
    DuplicatePluginId(String),
    #[error("语言 {language} 同时由 {first} 与 {second} 注册")]
    LanguageConflict {
        language: String,
        first: String,
        second: String,
    },
    #[error("未知插件 ID：{0}")]
    UnknownPlugin(String),
}

#[derive(Clone, Debug)]
pub struct PluginRegistry {
    by_id: BTreeMap<&'static str, &'static LspPluginDescriptor>,
    by_language: BTreeMap<&'static str, &'static str>,
}

impl PluginRegistry {
    pub fn new(
        descriptors: impl IntoIterator<Item = &'static LspPluginDescriptor>,
    ) -> Result<Self, PluginRegistryError> {
        let mut by_id = BTreeMap::new();
        let mut by_language = BTreeMap::new();
        for descriptor in descriptors {
            validate_descriptor(descriptor)?;
            if by_id.insert(descriptor.plugin_id, descriptor).is_some() {
                return Err(PluginRegistryError::DuplicatePluginId(
                    descriptor.plugin_id.to_string(),
                ));
            }
            for &language in descriptor.languages {
                if let Some(first) = by_language.insert(language, descriptor.plugin_id) {
                    return Err(PluginRegistryError::LanguageConflict {
                        language: language.to_string(),
                        first: first.to_string(),
                        second: descriptor.plugin_id.to_string(),
                    });
                }
            }
        }
        Ok(Self { by_id, by_language })
    }

    pub fn get(
        &self,
        plugin_id: &str,
    ) -> Result<&'static LspPluginDescriptor, PluginRegistryError> {
        self.by_id
            .get(plugin_id)
            .copied()
            .ok_or_else(|| PluginRegistryError::UnknownPlugin(plugin_id.to_string()))
    }

    pub fn for_language(&self, language: &str) -> Option<&'static LspPluginDescriptor> {
        self.by_language
            .get(language)
            .and_then(|plugin_id| self.by_id.get(plugin_id))
            .copied()
    }

    pub fn effective_operations(
        &self,
        plugin_id: &str,
        adapter: &BTreeSet<SemanticOperation>,
        server: &BTreeSet<SemanticOperation>,
    ) -> Result<BTreeSet<SemanticOperation>, PluginRegistryError> {
        let descriptor = self.get(plugin_id)?;
        Ok(descriptor
            .operations
            .iter()
            .copied()
            .filter(|operation| adapter.contains(operation) && server.contains(operation))
            .collect())
    }
}

pub fn official_plugin_registry() -> PluginRegistry {
    PluginRegistry::new([&JAVA_JDTLS_PLUGIN]).expect("内置 LSP 插件描述必须有效")
}

fn validate_descriptor(descriptor: &LspPluginDescriptor) -> Result<(), PluginRegistryError> {
    if descriptor.descriptor_version != DESCRIPTOR_SCHEMA_VERSION {
        return Err(PluginRegistryError::UnknownSchema(
            descriptor.descriptor_version,
        ));
    }
    if descriptor.adapter_id != "jdtls" {
        return Err(PluginRegistryError::UnknownAdapter(
            descriptor.adapter_id.to_string(),
        ));
    }
    if descriptor.plugin_id.is_empty()
        || !descriptor.plugin_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(PluginRegistryError::InvalidPluginId(
            descriptor.plugin_id.to_string(),
        ));
    }
    Ok(())
}
