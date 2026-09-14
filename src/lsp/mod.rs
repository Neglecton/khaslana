//! 可选语言增强共用的 LSP 客户端和语义服务。
//!
//! 首版只编译注册官方 `java-jdtls` 适配器。插件描述不从项目目录或下载包加载，
//! 引擎路径只能由受控 provider 转换成 [`LaunchSpec`]。

mod client;
mod install;
mod plugins;
mod process;
mod protocol;
pub mod runtime;
mod service;

pub mod providers;

pub use client::{
    ClientEvent, LspClient, LspClientError, LspNotification, RequestCancellation,
    ServerRequestPolicy,
};
pub use install::{
    ArchiveFormat, EnginePackageSpec, InstallError, InstallOutcome, InstallPhase, InstallProgress,
    InstallSettings, InstallSnapshot, InstalledComponent, JDT_LS_1_60_0_WINDOWS_X86_64,
    LspRegistry, PackageComponent, PluginInstallRecord, RuntimeSelection,
    TEMURIN_JDK_21_WINDOWS_X86_64, TRUSTED_ARCHIVE_HOSTS, TRUSTED_REDIRECT_HOSTS,
    available_specs_for_platform, builtin_package_specs, check_managed_activation,
    engine_package_dir, find_builtin_spec, install_package, install_snapshot, java_runtime_dir,
    load_registry, lsp_data_root, managed_runtime_java_home, uninstall_package,
};
pub use plugins::{
    DESCRIPTOR_SCHEMA_VERSION, EngineCompatibility, LspPluginDescriptor, PluginRegistry,
    SemanticOperation, TransportKind, official_plugin_registry,
};
pub use process::{LaunchSpec, OwnedLspProcess};
pub use protocol::{
    CharacterEncoding, LspLocation, LspPosition, LspRange, MAX_LSP_FRAME_BYTES,
    byte_offset_to_lsp_position, file_uri_to_path, lsp_position_to_byte_offset, path_to_file_uri,
    read_lsp_frame, scalar_position_to_lsp, write_lsp_frame,
};
pub use service::{
    Availability, CallEndpoint, EnginePackageStatus, LspSemanticService, PluginState,
    SemanticAnchor, SemanticItem, SemanticLocation, SemanticQueryResult, SemanticServiceError,
    ServiceLimits, ServiceStatus,
};

#[cfg(test)]
#[path = "../tests/lsp.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/lsp_install.rs"]
mod install_tests;
