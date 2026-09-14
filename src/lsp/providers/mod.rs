use std::collections::BTreeSet;

use serde_json::Value;
use thiserror::Error;

use crate::code_index::ProjectContext;

use super::{LaunchSpec, SemanticOperation, ServerRequestPolicy};

pub mod jdtls;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("LSP 配置无效：{0}")]
    InvalidConfiguration(String),
    #[error("LSP 路径访问失败：{0}")]
    Io(#[from] std::io::Error),
}

/// 编译期 provider 边界。启动参数只由实现生成，上层不能注入命令行。
pub trait SemanticProvider: Send + Sync {
    fn plugin_id(&self) -> &'static str;
    fn adapter_id(&self) -> &'static str;
    fn engine_version(&self) -> &str;
    fn implemented_operations(&self) -> BTreeSet<SemanticOperation>;
    fn launch_spec(&self, project: &ProjectContext) -> Result<LaunchSpec, ProviderError>;
    fn initialize_params(&self, project: &ProjectContext) -> Result<Value, ProviderError>;
    fn request_policy(&self) -> ServerRequestPolicy;
}
