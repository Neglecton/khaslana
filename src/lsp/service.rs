use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::code_index::{
    ProjectContext, SourceRef, content_fingerprint, discover_files, sha256_hex,
};
use crate::code_understanding::SourceService;

use super::plugins::{PluginRegistry, SemanticOperation, official_plugin_registry};
use super::protocol::{
    CharacterEncoding, LspPosition, LspRange, absolute_path_to_file_uri,
    byte_offset_to_lsp_position, file_uri_to_path, lsp_position_to_byte_offset, path_to_file_uri,
    scalar_position_to_lsp,
};
use super::providers::{ProviderError, SemanticProvider};
use super::{LspClientError, OwnedLspProcess, RequestCancellation, ServerRequestPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Off,
    Starting,
    Importing,
    Ready,
    Partial,
    Unavailable,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Ready,
    Partial,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePackageStatus {
    Missing,
    ManualConfigured,
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginState {
    pub plugin_id: String,
    pub registered: bool,
    pub enabled: bool,
    pub package_status: EnginePackageStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceLimits {
    pub query_timeout: Duration,
    pub initialize_timeout: Duration,
    pub import_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_queue: usize,
    pub default_items: usize,
    pub max_items: usize,
    pub max_rpc_per_query: usize,
    pub max_cache_entries: usize,
    pub max_cache_bytes: usize,
}

impl Default for ServiceLimits {
    fn default() -> Self {
        Self {
            query_timeout: Duration::from_secs(3),
            initialize_timeout: Duration::from_secs(30),
            import_timeout: Duration::from_secs(120),
            shutdown_timeout: Duration::from_secs(3),
            idle_timeout: Duration::from_secs(5 * 60),
            max_queue: 16,
            default_items: 20,
            max_items: 40,
            max_rpc_per_query: 6,
            max_cache_entries: 128,
            max_cache_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticAnchor {
    pub anchor_id: String,
    pub project_key: String,
    pub generation: u64,
    pub relative_path: String,
    pub content_sha256: String,
    pub line: u32,
    pub column: u32,
    pub allowed_start_byte: u64,
    pub allowed_end_byte: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticLocation {
    pub relative_path: Option<String>,
    pub external_uri_hint: Option<String>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub start_byte: Option<u64>,
    pub end_byte: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallEndpoint {
    pub name: String,
    pub detail: Option<String>,
    pub location: SemanticLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticItem {
    pub target: SemanticLocation,
    pub caller: Option<CallEndpoint>,
    pub callee: Option<CallEndpoint>,
    pub call_site: Option<SemanticLocation>,
    pub recursive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticQueryResult {
    pub project_key: String,
    pub request_id: u64,
    pub session_epoch: u64,
    pub workspace_revision: u64,
    pub provider: String,
    pub plugin_id: String,
    pub engine_version: String,
    pub operation: SemanticOperation,
    pub availability: Availability,
    pub coverage: Vec<String>,
    pub items: Vec<SemanticItem>,
    pub truncated: bool,
    pub reason: Option<String>,
    /// 本次实际发送给语言服务器的 RPC 数；缓存命中为 0。
    pub rpc_count: usize,
    pub cache_hit: bool,
}

#[derive(Debug, Error)]
pub enum SemanticServiceError {
    #[error("项目未注册：{0}")]
    UnknownProject(String),
    #[error("Java 增强未启用")]
    Disabled,
    #[error("项目尚未授权语义导入")]
    ImportNotAuthorized,
    #[error("另一个项目正在使用 JDT LS")]
    ProjectBusy,
    #[error("语义查询队列已满")]
    Busy,
    #[error("未知或已失效的语义锚点")]
    InvalidAnchor,
    #[error("语义锚点不属于当前项目")]
    ProjectMismatch,
    #[error("源码已变化：{0}")]
    SourceChanged(String),
    #[error("源码位置无效：{0}")]
    InvalidPosition(String),
    #[error("语义结果条数必须在 1 到 {max} 之间：{actual}")]
    InvalidLimit { actual: usize, max: usize },
    #[error("语义操作不受支持：{0:?}")]
    Unsupported(SemanticOperation),
    #[error("LSP provider 配置错误：{0}")]
    Provider(#[from] ProviderError),
    #[error("LSP 客户端错误：{0}")]
    Client(#[from] LspClientError),
    #[error("LSP 进程错误：{0}")]
    Process(String),
    #[error("源码访问被拒绝：{0}")]
    UnsafeSource(String),
    #[error("源码来源校验失败：{0}")]
    SourceValidation(String),
    #[error("语义服务尚未就绪")]
    NotReady,
}

struct ProjectState {
    context: ProjectContext,
    enabled: bool,
    import_authorized: bool,
    status: ServiceStatus,
    revision: u64,
    consumers: HashSet<String>,
    allowed_files: HashSet<String>,
    anchors: HashMap<String, SemanticAnchor>,
    restart_attempted: bool,
}

struct ActiveSession {
    project_key: String,
    epoch: u64,
    process: OwnedLspProcess,
    encoding: CharacterEncoding,
    effective_operations: BTreeSet<SemanticOperation>,
    document_symbols_supported: bool,
    engine_version: String,
    opened_documents: HashMap<String, (String, i32)>,
    idle_since: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    plugin_id: String,
    engine_version: String,
    project_key: String,
    epoch: u64,
    revision: u64,
    anchor_hash: String,
    operation: SemanticOperation,
    limit: usize,
}

#[derive(Default)]
struct ResultCache {
    entries: HashMap<CacheKey, (Arc<SemanticQueryResult>, usize)>,
    order: VecDeque<CacheKey>,
    bytes: usize,
}

impl ResultCache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<SemanticQueryResult>> {
        let result = self.entries.get(key).map(|(value, _)| Arc::clone(value));
        if result.is_some() {
            self.order.retain(|candidate| candidate != key);
            self.order.push_back(key.clone());
        }
        result
    }

    fn insert(&mut self, key: CacheKey, value: SemanticQueryResult, limits: &ServiceLimits) {
        let size = serde_json::to_vec(&value).map_or(0, |bytes| bytes.len());
        if size > limits.max_cache_bytes {
            return;
        }
        if let Some((_, old_size)) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old_size);
            self.order.retain(|candidate| candidate != &key);
        }
        self.bytes = self.bytes.saturating_add(size);
        self.order.push_back(key.clone());
        self.entries.insert(key, (Arc::new(value), size));
        while self.entries.len() > limits.max_cache_entries || self.bytes > limits.max_cache_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, old_size)) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(old_size);
            }
        }
    }

    fn clear_project(&mut self, project_key: &str) {
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|key| key.project_key == project_key)
            .cloned()
            .collect();
        for key in keys {
            if let Some((_, size)) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(size);
            }
            self.order.retain(|candidate| candidate != &key);
        }
    }
}

struct ServiceInner {
    registry: PluginRegistry,
    provider: Arc<dyn SemanticProvider>,
    plugin_enabled: bool,
    projects: HashMap<String, ProjectState>,
    active: Option<ActiveSession>,
    cache: ResultCache,
    next_epoch: u64,
    next_request: u64,
}

pub struct LspSemanticService {
    inner: Mutex<ServiceInner>,
    limits: ServiceLimits,
    queued: AtomicUsize,
}

impl LspSemanticService {
    pub fn new(provider: Arc<dyn SemanticProvider>) -> Result<Self, SemanticServiceError> {
        Self::with_limits(provider, ServiceLimits::default())
    }

    pub fn with_limits(
        provider: Arc<dyn SemanticProvider>,
        limits: ServiceLimits,
    ) -> Result<Self, SemanticServiceError> {
        let registry = official_plugin_registry();
        let descriptor = registry
            .get(provider.plugin_id())
            .map_err(|error| SemanticServiceError::Process(error.to_string()))?;
        if descriptor.adapter_id != provider.adapter_id() {
            return Err(SemanticServiceError::Process(format!(
                "插件 {} 的适配器身份不匹配",
                descriptor.plugin_id
            )));
        }
        Ok(Self {
            inner: Mutex::new(ServiceInner {
                registry,
                provider,
                plugin_enabled: true,
                projects: HashMap::new(),
                active: None,
                cache: ResultCache::default(),
                next_epoch: 1,
                next_request: 1,
            }),
            limits,
            queued: AtomicUsize::new(0),
        })
    }

    pub fn set_plugin_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        inner.plugin_enabled = enabled;
        if !enabled {
            stop_active(&mut inner, self.limits.shutdown_timeout);
            for project in inner.projects.values_mut() {
                project.status = ServiceStatus::Off;
                project.anchors.clear();
            }
            inner.cache = ResultCache::default();
        }
    }

    pub fn plugin_state(&self, plugin_id: &str) -> PluginState {
        let inner = self.inner.lock().expect("LSP 服务锁被污染");
        PluginState {
            plugin_id: plugin_id.to_string(),
            registered: inner.registry.get(plugin_id).is_ok(),
            enabled: inner.plugin_enabled && inner.registry.get(plugin_id).is_ok(),
            package_status: if inner.registry.get(plugin_id).is_ok() {
                EnginePackageStatus::ManualConfigured
            } else {
                EnginePackageStatus::Missing
            },
        }
    }

    pub fn configure_project(
        &self,
        context: ProjectContext,
        enabled: bool,
        import_authorized: bool,
    ) -> Result<(), SemanticServiceError> {
        let canonical_root = std::fs::canonicalize(&context.canonical_root)
            .map_err(|error| SemanticServiceError::UnsafeSource(error.to_string()))?;
        if !canonical_root.is_dir() {
            return Err(SemanticServiceError::UnsafeSource(
                "项目根路径不是目录".to_string(),
            ));
        }
        let allowed_files = discover_files(&canonical_root)
            .map_err(|error| SemanticServiceError::UnsafeSource(error.to_string()))?
            .files
            .into_iter()
            .map(|file| file.rel_path)
            .collect();
        let key = context.project_key.clone();
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        if !enabled
            && inner
                .active
                .as_ref()
                .is_some_and(|active| active.project_key == key)
        {
            stop_active(&mut inner, self.limits.shutdown_timeout);
        }
        inner.cache.clear_project(&key);
        inner.projects.insert(
            key,
            ProjectState {
                context,
                enabled,
                import_authorized,
                status: if enabled {
                    ServiceStatus::Stopped
                } else {
                    ServiceStatus::Off
                },
                revision: 1,
                consumers: HashSet::new(),
                allowed_files,
                anchors: HashMap::new(),
                restart_attempted: false,
            },
        );
        Ok(())
    }

    pub fn acquire(
        &self,
        project_key: &str,
        consumer: impl Into<String>,
    ) -> Result<(), SemanticServiceError> {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let project = inner
            .projects
            .get_mut(project_key)
            .ok_or_else(|| SemanticServiceError::UnknownProject(project_key.to_string()))?;
        project.consumers.insert(consumer.into());
        if let Some(active) = inner
            .active
            .as_mut()
            .filter(|active| active.project_key == project_key)
        {
            active.idle_since = None;
        }
        Ok(())
    }

    pub fn release(&self, project_key: &str, consumer: &str) {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let empty = inner.projects.get_mut(project_key).is_some_and(|project| {
            project.consumers.remove(consumer);
            project.consumers.is_empty()
        });
        if empty
            && let Some(active) = inner
                .active
                .as_mut()
                .filter(|active| active.project_key == project_key)
        {
            active.idle_since = Some(Instant::now());
        }
    }

    pub fn reap_idle(&self) {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let expired = inner
            .active
            .as_ref()
            .and_then(|active| active.idle_since)
            .is_some_and(|idle| idle.elapsed() >= self.limits.idle_timeout);
        if expired {
            stop_active(&mut inner, self.limits.shutdown_timeout);
        }
    }

    pub fn status(&self, project_key: &str, language: &str) -> ServiceStatus {
        if language != "java" {
            return ServiceStatus::Unavailable;
        }
        self.inner
            .lock()
            .expect("LSP 服务锁被污染")
            .projects
            .get(project_key)
            .map_or(ServiceStatus::Off, |project| project.status)
    }

    pub fn ensure_started(
        &self,
        project_key: &str,
        language: &str,
    ) -> Result<ServiceStatus, SemanticServiceError> {
        if language != "java" {
            return Err(SemanticServiceError::Process(format!(
                "未注册语言：{language}"
            )));
        }
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        validate_project_enabled(&inner, project_key)?;
        if let Some(active) = inner.active.as_mut() {
            if active.project_key != project_key {
                return Err(SemanticServiceError::ProjectBusy);
            }
            let exited = active.process.try_wait().ok().flatten().is_some();
            if !exited {
                refresh_status(&mut inner, project_key);
                return Ok(inner.projects[project_key].status);
            }
            let project = inner.projects.get_mut(project_key).unwrap();
            if project.restart_attempted {
                project.status = ServiceStatus::Unavailable;
                return Err(SemanticServiceError::Process(
                    "JDT LS 连续退出，等待手动重试".to_string(),
                ));
            }
            project.restart_attempted = true;
            stop_active(&mut inner, self.limits.shutdown_timeout);
        }

        let context = inner.projects[project_key].context.clone();
        inner.projects.get_mut(project_key).unwrap().status = ServiceStatus::Starting;
        let spec = inner.provider.launch_spec(&context)?;
        let params = inner.provider.initialize_params(&context)?;
        let settings = params
            .pointer("/initializationOptions/settings")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let policy = inner.provider.request_policy();
        let process = OwnedLspProcess::spawn(&spec, policy.clone())
            .map_err(|error| SemanticServiceError::Process(error.to_string()))?;
        let initialize = process.client().request(
            "initialize",
            params,
            self.limits.initialize_timeout,
            &RequestCancellation::default(),
        );
        let initialize = match initialize {
            Ok(value) => value,
            Err(error) => {
                inner.projects.get_mut(project_key).unwrap().status = ServiceStatus::Unavailable;
                let stderr = process.stderr_tail().join(" | ");
                if stderr.is_empty() {
                    return Err(error.into());
                }
                return Err(SemanticServiceError::Process(format!(
                    "initialize 失败：{error}；JDT stderr：{stderr}"
                )));
            }
        };
        process.client().notify("initialized", json!({}))?;
        process.client().notify(
            "workspace/didChangeConfiguration",
            json!({"settings": settings}),
        )?;
        let server_operations = server_operations(&initialize);
        let effective_operations = inner
            .registry
            .effective_operations(
                inner.provider.plugin_id(),
                &inner.provider.implemented_operations(),
                &server_operations,
            )
            .map_err(|error| SemanticServiceError::Process(error.to_string()))?;
        let encoding = CharacterEncoding::from_server(
            initialize
                .pointer("/capabilities/positionEncoding")
                .and_then(Value::as_str),
        );
        let document_symbols_supported = capability_enabled(
            initialize
                .get("capabilities")
                .and_then(|capabilities| capabilities.get("documentSymbolProvider")),
        );
        let engine_version = inner.provider.engine_version().to_string();
        let epoch = inner.next_epoch;
        inner.next_epoch = inner.next_epoch.saturating_add(1);
        inner.active = Some(ActiveSession {
            project_key: project_key.to_string(),
            epoch,
            process,
            encoding,
            effective_operations,
            document_symbols_supported,
            engine_version,
            opened_documents: HashMap::new(),
            idle_since: None,
        });
        inner.projects.get_mut(project_key).unwrap().status = ServiceStatus::Importing;

        let deadline = Instant::now() + self.limits.import_timeout;
        while Instant::now() < deadline {
            if policy.service_ready() {
                let status = if policy.has_diagnostic_errors() {
                    ServiceStatus::Partial
                } else {
                    ServiceStatus::Ready
                };
                inner.projects.get_mut(project_key).unwrap().status = status;
                return Ok(status);
            }
            if inner
                .active
                .as_mut()
                .and_then(|active| active.process.try_wait().ok().flatten())
                .is_some()
            {
                inner.projects.get_mut(project_key).unwrap().status = ServiceStatus::Unavailable;
                return Err(SemanticServiceError::Process(
                    "JDT LS 在工程导入期间退出".to_string(),
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
        inner.projects.get_mut(project_key).unwrap().status = ServiceStatus::Partial;
        Ok(ServiceStatus::Partial)
    }

    pub fn register_source_anchor(
        &self,
        source_service: &SourceService,
        source_id: &str,
        line: u32,
        column: u32,
    ) -> Result<SemanticAnchor, SemanticServiceError> {
        let source = source_service
            .validate_source(source_id)
            .map_err(|error| SemanticServiceError::SourceValidation(error.to_string()))?;
        self.register_verified_anchor(&source.project_key, &source, line, column)
    }

    pub(crate) fn register_verified_anchor(
        &self,
        project_key: &str,
        source: &SourceRef,
        line: u32,
        column: u32,
    ) -> Result<SemanticAnchor, SemanticServiceError> {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let project = inner
            .projects
            .get_mut(project_key)
            .ok_or_else(|| SemanticServiceError::UnknownProject(project_key.to_string()))?;
        if source.project_key != project_key {
            return Err(SemanticServiceError::ProjectMismatch);
        }
        if !project.allowed_files.contains(&source.relative_path) {
            return Err(SemanticServiceError::UnsafeSource(
                source.relative_path.clone(),
            ));
        }
        let (_, text) = read_project_text(project, &source.relative_path)?;
        if content_fingerprint(text.as_bytes()) != source.content_sha256 {
            return Err(SemanticServiceError::SourceChanged(
                source.relative_path.clone(),
            ));
        }
        if source.end_byte > text.len() as u64 {
            return Err(SemanticServiceError::SourceChanged(
                source.relative_path.clone(),
            ));
        }
        let byte = scalar_position_to_lsp(&text, line, column, CharacterEncoding::Utf8)
            .and_then(|position| {
                lsp_position_to_byte_offset(&text, position, CharacterEncoding::Utf8)
            })
            .map_err(SemanticServiceError::InvalidPosition)?;
        if byte < source.start_byte as usize || byte > source.end_byte as usize {
            return Err(SemanticServiceError::InvalidPosition(
                "位置不在已发放源码范围内".to_string(),
            ));
        }
        let material = format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            project_key,
            source.generation,
            source.relative_path,
            source.content_sha256,
            source.start_byte,
            source.end_byte,
            line,
            column,
            project.revision
        );
        let anchor = SemanticAnchor {
            anchor_id: format!("sa1:{}", &sha256_hex(material.as_bytes())[..16]),
            project_key: project_key.to_string(),
            generation: source.generation,
            relative_path: source.relative_path.clone(),
            content_sha256: source.content_sha256.clone(),
            line,
            column,
            allowed_start_byte: source.start_byte,
            allowed_end_byte: source.end_byte,
        };
        project
            .anchors
            .insert(anchor.anchor_id.clone(), anchor.clone());
        Ok(anchor)
    }

    /// 用 documentSymbol 的 selectionRange 把“只有行号”的索引候选变成精确锚点。
    /// 同行同名重载等情况会保留多个候选，由上层明确消歧。
    pub fn register_source_symbol_anchors(
        &self,
        source_service: &SourceService,
        source_id: &str,
        approximate_line: u32,
        expected_name: Option<&str>,
        cancellation: &RequestCancellation,
    ) -> Result<Vec<SemanticAnchor>, SemanticServiceError> {
        let source = source_service
            .validate_source(source_id)
            .map_err(|error| SemanticServiceError::SourceValidation(error.to_string()))?;
        self.register_verified_symbol_anchors(
            &source.project_key,
            &source,
            approximate_line,
            expected_name,
            cancellation,
        )
    }

    pub(crate) fn register_verified_symbol_anchors(
        &self,
        project_key: &str,
        source: &SourceRef,
        approximate_line: u32,
        expected_name: Option<&str>,
        cancellation: &RequestCancellation,
    ) -> Result<Vec<SemanticAnchor>, SemanticServiceError> {
        let _queue_guard = self.enter_queue()?;
        if approximate_line == 0 {
            return Err(SemanticServiceError::InvalidPosition(
                "源码行号从 1 开始".to_string(),
            ));
        }
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        refresh_status(&mut inner, project_key);
        let project = inner
            .projects
            .get(project_key)
            .ok_or_else(|| SemanticServiceError::UnknownProject(project_key.to_string()))?;
        if !matches!(
            project.status,
            ServiceStatus::Ready | ServiceStatus::Partial
        ) {
            return Err(SemanticServiceError::NotReady);
        }
        if source.project_key != project_key {
            return Err(SemanticServiceError::ProjectMismatch);
        }
        let (root, text) = read_project_text(project, &source.relative_path)?;
        if content_fingerprint(text.as_bytes()) != source.content_sha256 {
            return Err(SemanticServiceError::SourceChanged(
                source.relative_path.clone(),
            ));
        }
        let active = inner
            .active
            .as_mut()
            .ok_or(SemanticServiceError::NotReady)?;
        if active.project_key != project_key {
            return Err(SemanticServiceError::ProjectMismatch);
        }
        if !active.document_symbols_supported {
            return Err(SemanticServiceError::Process(
                "服务器未提供 documentSymbol 辅助能力".to_string(),
            ));
        }
        let uri = path_to_file_uri(&root.join(&source.relative_path))
            .map_err(SemanticServiceError::InvalidPosition)?;
        sync_document(active, &uri, &text)?;
        let response = active.process.client().request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":uri}}),
            self.limits.query_timeout,
            cancellation,
        )?;
        let encoding = active.encoding;
        let mut selections = Vec::new();
        collect_symbol_selections(
            &response,
            approximate_line.saturating_sub(1),
            expected_name,
            &mut selections,
        );
        drop(inner);

        let mut anchors = Vec::new();
        for position in selections {
            let byte = lsp_position_to_byte_offset(&text, position, encoding)
                .map_err(SemanticServiceError::InvalidPosition)?;
            let scalar = byte_to_scalar_position(&text, byte)
                .map_err(SemanticServiceError::InvalidPosition)?;
            if byte >= source.start_byte as usize && byte <= source.end_byte as usize {
                anchors.push(self.register_verified_anchor(
                    project_key,
                    source,
                    scalar.0,
                    scalar.1,
                )?);
            }
        }
        anchors.sort_by_key(|anchor| (anchor.line, anchor.column));
        anchors.dedup_by(|left, right| left.line == right.line && left.column == right.column);
        Ok(anchors)
    }

    pub fn query(
        &self,
        anchor_id: &str,
        operation: SemanticOperation,
        limit: Option<usize>,
        cancellation: &RequestCancellation,
    ) -> Result<SemanticQueryResult, SemanticServiceError> {
        self.query_with_rpc_budget(
            anchor_id,
            operation,
            limit,
            self.limits.max_rpc_per_query,
            cancellation,
        )
    }

    /// 在调用方给定的 RPC 额度内执行一次语义操作。
    ///
    /// 额度包含调用层级的 prepare 请求，且不会超过服务自己的单操作上限。
    pub fn query_with_rpc_budget(
        &self,
        anchor_id: &str,
        operation: SemanticOperation,
        limit: Option<usize>,
        rpc_budget: usize,
        cancellation: &RequestCancellation,
    ) -> Result<SemanticQueryResult, SemanticServiceError> {
        let _queue_guard = self.enter_queue()?;
        if rpc_budget == 0 {
            return Err(SemanticServiceError::Busy);
        }
        let limit = limit.unwrap_or(self.limits.default_items);
        if limit == 0 || limit > self.limits.max_items {
            return Err(SemanticServiceError::InvalidLimit {
                actual: limit,
                max: self.limits.max_items,
            });
        }
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let (project_key, anchor) = find_anchor(&inner, anchor_id)?;
        refresh_status(&mut inner, &project_key);
        let project_status = inner.projects[&project_key].status;
        if !matches!(
            project_status,
            ServiceStatus::Ready | ServiceStatus::Partial
        ) {
            return Err(SemanticServiceError::NotReady);
        }
        let revision = inner.projects[&project_key].revision;
        let (root, text) = {
            let project = &inner.projects[&project_key];
            read_project_text(project, &anchor.relative_path)?
        };
        if content_fingerprint(text.as_bytes()) != anchor.content_sha256 {
            return Err(SemanticServiceError::SourceChanged(anchor.relative_path));
        }
        let (active_project_key, active_engine_version, active_epoch, active_encoding, supports) = {
            let active = inner
                .active
                .as_ref()
                .ok_or(SemanticServiceError::NotReady)?;
            (
                active.project_key.clone(),
                active.engine_version.clone(),
                active.epoch,
                active.encoding,
                active.effective_operations.contains(&operation),
            )
        };
        if active_project_key != project_key {
            return Err(SemanticServiceError::ProjectMismatch);
        }
        if !supports {
            return Err(SemanticServiceError::Unsupported(operation));
        }
        let cache_key = CacheKey {
            plugin_id: inner.provider.plugin_id().to_string(),
            engine_version: active_engine_version.clone(),
            project_key: project_key.clone(),
            epoch: active_epoch,
            revision,
            anchor_hash: anchor.anchor_id.clone(),
            operation,
            limit,
        };
        if let Some(cached) = inner.cache.get(&cache_key) {
            let mut result = (*cached).clone();
            result.request_id = next_request_id(&mut inner);
            result.rpc_count = 0;
            result.cache_hit = true;
            return Ok(result);
        }

        let position = scalar_position_to_lsp(&text, anchor.line, anchor.column, active_encoding)
            .map_err(SemanticServiceError::InvalidPosition)?;
        let uri = path_to_file_uri(&root.join(&anchor.relative_path))
            .map_err(SemanticServiceError::InvalidPosition)?;
        sync_document(inner.active.as_mut().unwrap(), &uri, &text)?;
        let deadline = Instant::now() + self.limits.query_timeout;
        let query_context = QueryContext {
            active: inner.active.as_ref().unwrap(),
            uri: &uri,
            position,
            root: &root,
            allowed_files: &inner.projects[&project_key].allowed_files,
            deadline,
            cancellation,
        };
        let QueryOutcome {
            mut items,
            mut coverage,
            mut reason,
            rpc_count,
        } = match operation {
            SemanticOperation::Definition => {
                query_locations(&query_context, "textDocument/definition")?
            }
            SemanticOperation::Implementations => {
                query_locations(&query_context, "textDocument/implementation")?
            }
            SemanticOperation::References => query_references(&query_context)?,
            SemanticOperation::IncomingCalls | SemanticOperation::OutgoingCalls => query_calls(
                &query_context,
                operation,
                rpc_budget.min(self.limits.max_rpc_per_query),
            )?,
        };
        if project_status == ServiceStatus::Partial {
            coverage.push("工程存在诊断或同步缺口，本次结果不代表完整关系".to_string());
        }
        let before = items.len();
        items.truncate(limit);
        let truncated = before > items.len();
        if truncated {
            coverage.push(format!("结果按 {limit} 条预算截断"));
            reason.get_or_insert_with(|| "result_limit".to_string());
        }
        coverage.sort();
        coverage.dedup();
        if inner.projects[&project_key].revision != revision {
            return Err(SemanticServiceError::SourceChanged(anchor.relative_path));
        }
        let (session_epoch, engine_version) = {
            let active = inner.active.as_ref().unwrap();
            (active.epoch, active.engine_version.clone())
        };
        let request_id = next_request_id(&mut inner);
        let result = SemanticQueryResult {
            project_key,
            request_id,
            session_epoch,
            workspace_revision: revision,
            provider: "jdtls".to_string(),
            plugin_id: inner.provider.plugin_id().to_string(),
            engine_version,
            operation,
            availability: if coverage.is_empty() {
                Availability::Ready
            } else {
                Availability::Partial
            },
            coverage,
            items,
            truncated,
            reason,
            rpc_count,
            cache_hit: false,
        };
        inner.cache.insert(cache_key, result.clone(), &self.limits);
        Ok(result)
    }

    pub fn mark_file_changed(
        &self,
        project_key: &str,
        relative_path: &str,
        change_type: u8,
    ) -> Result<(), SemanticServiceError> {
        let mut inner = self.inner.lock().expect("LSP 服务锁被污染");
        let active_for_project = inner
            .active
            .as_ref()
            .is_some_and(|active| active.project_key == project_key);
        let project = inner
            .projects
            .get_mut(project_key)
            .ok_or_else(|| SemanticServiceError::UnknownProject(project_key.to_string()))?;
        if Path::new(relative_path).components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Err(SemanticServiceError::UnsafeSource(
                relative_path.to_string(),
            ));
        }
        project.revision = project.revision.saturating_add(1);
        project.anchors.clear();
        let build_change = is_build_configuration(relative_path);
        if build_change && active_for_project {
            project.status = ServiceStatus::Importing;
        } else if project.status == ServiceStatus::Ready {
            project.status = ServiceStatus::Partial;
        }
        let root = PathBuf::from(&project.context.canonical_root);
        if let Ok(discovered) = discover_files(&root) {
            project.allowed_files = discovered
                .files
                .into_iter()
                .map(|file| file.rel_path)
                .collect();
        }
        inner.cache.clear_project(project_key);
        if let Some(active) = inner
            .active
            .as_mut()
            .filter(|active| active.project_key == project_key)
        {
            let uri = absolute_path_to_file_uri(&root.join(relative_path))
                .map_err(SemanticServiceError::InvalidPosition)?;
            active.opened_documents.remove(&uri);
            active.process.client().notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes":[{"uri":uri,"type":change_type}]}),
            )?;
            if build_change {
                active.process.client().policy().mark_importing();
                active
                    .process
                    .client()
                    .notify("workspace/didChangeConfiguration", json!({"settings": {}}))?;
            }
        }
        Ok(())
    }

    fn enter_queue(&self) -> Result<QueueGuard<'_>, SemanticServiceError> {
        let old = self.queued.fetch_add(1, Ordering::AcqRel);
        if old >= self.limits.max_queue {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(SemanticServiceError::Busy);
        }
        Ok(QueueGuard(&self.queued))
    }
}

impl Drop for LspSemanticService {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            stop_active(&mut inner, self.limits.shutdown_timeout);
        }
    }
}

struct QueueGuard<'a>(&'a AtomicUsize);

impl Drop for QueueGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn validate_project_enabled(
    inner: &ServiceInner,
    project_key: &str,
) -> Result<(), SemanticServiceError> {
    if !inner.plugin_enabled {
        return Err(SemanticServiceError::Disabled);
    }
    let project = inner
        .projects
        .get(project_key)
        .ok_or_else(|| SemanticServiceError::UnknownProject(project_key.to_string()))?;
    if !project.enabled {
        return Err(SemanticServiceError::Disabled);
    }
    if !project.import_authorized {
        return Err(SemanticServiceError::ImportNotAuthorized);
    }
    Ok(())
}

fn refresh_status(inner: &mut ServiceInner, project_key: &str) {
    let policy = inner
        .active
        .as_ref()
        .filter(|active| active.project_key == project_key)
        .map(|active| active.process.client().policy().clone());
    if let Some(policy) = policy.filter(ServerRequestPolicy::service_ready)
        && let Some(project) = inner.projects.get_mut(project_key)
    {
        project.status = if policy.has_diagnostic_errors() {
            ServiceStatus::Partial
        } else {
            ServiceStatus::Ready
        };
    }
}

fn stop_active(inner: &mut ServiceInner, timeout: Duration) {
    if let Some(mut active) = inner.active.take() {
        active.process.shutdown(timeout);
        if let Some(project) = inner.projects.get_mut(&active.project_key) {
            project.status = ServiceStatus::Stopped;
        }
        inner.cache.clear_project(&active.project_key);
    }
}

fn find_anchor(
    inner: &ServiceInner,
    anchor_id: &str,
) -> Result<(String, SemanticAnchor), SemanticServiceError> {
    for (project_key, project) in &inner.projects {
        if let Some(anchor) = project.anchors.get(anchor_id) {
            return Ok((project_key.clone(), anchor.clone()));
        }
    }
    Err(SemanticServiceError::InvalidAnchor)
}

fn read_project_text(
    project: &ProjectState,
    relative_path: &str,
) -> Result<(PathBuf, String), SemanticServiceError> {
    if !project.allowed_files.contains(relative_path) {
        return Err(SemanticServiceError::UnsafeSource(
            relative_path.to_string(),
        ));
    }
    let root = std::fs::canonicalize(&project.context.canonical_root)
        .map_err(|error| SemanticServiceError::UnsafeSource(error.to_string()))?;
    let path = root.join(relative_path);
    let canonical = std::fs::canonicalize(&path)
        .map_err(|error| SemanticServiceError::UnsafeSource(error.to_string()))?;
    if !canonical.starts_with(&root) {
        return Err(SemanticServiceError::UnsafeSource(
            relative_path.to_string(),
        ));
    }
    let bytes = std::fs::read(canonical)
        .map_err(|error| SemanticServiceError::UnsafeSource(error.to_string()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| SemanticServiceError::UnsafeSource("源码不是 UTF-8".to_string()))?;
    Ok((root, text))
}

fn sync_document(active: &mut ActiveSession, uri: &str, text: &str) -> Result<(), LspClientError> {
    let hash = content_fingerprint(text.as_bytes());
    match active.opened_documents.get_mut(uri) {
        None => {
            active.process.client().notify(
                "textDocument/didOpen",
                json!({"textDocument":{"uri":uri,"languageId":"java","version":1,"text":text}}),
            )?;
            active.opened_documents.insert(uri.to_string(), (hash, 1));
        }
        Some((known_hash, version)) if known_hash != &hash => {
            *version = version.saturating_add(1);
            active.process.client().notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":uri,"version":*version},"contentChanges":[{"text":text}]}),
            )?;
            *known_hash = hash;
        }
        Some(_) => {}
    }
    Ok(())
}

fn server_operations(initialize: &Value) -> BTreeSet<SemanticOperation> {
    let capabilities = initialize.get("capabilities").unwrap_or(&Value::Null);
    let mut operations = BTreeSet::new();
    let fields = [
        ("definitionProvider", SemanticOperation::Definition),
        ("implementationProvider", SemanticOperation::Implementations),
        ("referencesProvider", SemanticOperation::References),
    ];
    for (field, operation) in fields {
        if capability_enabled(capabilities.get(field)) {
            operations.insert(operation);
        }
    }
    if capability_enabled(capabilities.get("callHierarchyProvider")) {
        operations.insert(SemanticOperation::IncomingCalls);
        operations.insert(SemanticOperation::OutgoingCalls);
    }
    operations
}

fn capability_enabled(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)) | Some(Value::Object(_)))
}

struct QueryOutcome {
    items: Vec<SemanticItem>,
    coverage: Vec<String>,
    reason: Option<String>,
    rpc_count: usize,
}

struct QueryContext<'a> {
    active: &'a ActiveSession,
    uri: &'a str,
    position: LspPosition,
    root: &'a Path,
    allowed_files: &'a HashSet<String>,
    deadline: Instant,
    cancellation: &'a RequestCancellation,
}

fn query_locations(
    context: &QueryContext<'_>,
    method: &str,
) -> Result<QueryOutcome, SemanticServiceError> {
    let value = context.active.process.client().request(
        method,
        json!({"textDocument":{"uri":context.uri},"position":context.position}),
        remaining(context.deadline)?,
        context.cancellation,
    )?;
    let mut coverage = Vec::new();
    let mut items = Vec::new();
    for location in location_values(&value) {
        match parse_location(
            location,
            context.root,
            context.allowed_files,
            context.active.encoding,
        ) {
            Ok(target) => {
                note_external_location(&target, &mut coverage);
                items.push(SemanticItem {
                    target,
                    caller: None,
                    callee: None,
                    call_site: None,
                    recursive: false,
                });
            }
            Err(reason) => coverage.push(reason),
        }
    }
    deduplicate(&mut items);
    Ok(QueryOutcome {
        items,
        coverage,
        reason: None,
        rpc_count: 1,
    })
}

fn query_references(context: &QueryContext<'_>) -> Result<QueryOutcome, SemanticServiceError> {
    let value = context.active.process.client().request(
        "textDocument/references",
        json!({
            "textDocument":{"uri":context.uri},
            "position":context.position,
            "context":{"includeDeclaration":true}
        }),
        remaining(context.deadline)?,
        context.cancellation,
    )?;
    let mut coverage = Vec::new();
    let mut items = Vec::new();
    for location in location_values(&value) {
        match parse_location(
            location,
            context.root,
            context.allowed_files,
            context.active.encoding,
        ) {
            Ok(target) => {
                note_external_location(&target, &mut coverage);
                items.push(SemanticItem {
                    target,
                    caller: None,
                    callee: None,
                    call_site: None,
                    recursive: false,
                });
            }
            Err(reason) => coverage.push(reason),
        }
    }
    deduplicate(&mut items);
    Ok(QueryOutcome {
        items,
        coverage,
        reason: None,
        rpc_count: 1,
    })
}

fn query_calls(
    context: &QueryContext<'_>,
    operation: SemanticOperation,
    max_rpc: usize,
) -> Result<QueryOutcome, SemanticServiceError> {
    let prepared = context.active.process.client().request(
        "textDocument/prepareCallHierarchy",
        json!({"textDocument":{"uri":context.uri},"position":context.position}),
        remaining(context.deadline)?,
        context.cancellation,
    )?;
    let prepared = prepared.as_array().cloned().unwrap_or_default();
    if prepared.is_empty() {
        return Ok(QueryOutcome {
            items: Vec::new(),
            coverage: Vec::new(),
            reason: None,
            rpc_count: 1,
        });
    }
    let method = match operation {
        SemanticOperation::IncomingCalls => "callHierarchy/incomingCalls",
        SemanticOperation::OutgoingCalls => "callHierarchy/outgoingCalls",
        _ => unreachable!(),
    };
    let mut items = Vec::new();
    let mut coverage = Vec::new();
    let mut rpc_count = 1;
    for (used_rpc, source_item) in (1usize..).zip(prepared) {
        if used_rpc >= max_rpc {
            coverage.push(format!("调用层级候选超过 {max_rpc} RPC 预算"));
            break;
        }
        let response = context.active.process.client().request(
            method,
            json!({"item":source_item}),
            remaining(context.deadline)?,
            context.cancellation,
        )?;
        rpc_count += 1;
        for call in response.as_array().into_iter().flatten() {
            let (caller_value, callee_value, range_uri, ranges) = match operation {
                SemanticOperation::IncomingCalls => (
                    call.get("from"),
                    Some(&source_item),
                    call.pointer("/from/uri").and_then(Value::as_str),
                    call.get("fromRanges").and_then(Value::as_array),
                ),
                SemanticOperation::OutgoingCalls => (
                    Some(&source_item),
                    call.get("to"),
                    source_item.get("uri").and_then(Value::as_str),
                    call.get("fromRanges").and_then(Value::as_array),
                ),
                _ => unreachable!(),
            };
            let (Some(caller_value), Some(callee_value)) = (caller_value, callee_value) else {
                coverage.push("调用层级响应缺少端点".to_string());
                continue;
            };
            let caller = match parse_call_endpoint(
                caller_value,
                context.root,
                context.allowed_files,
                context.active.encoding,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    coverage.push(reason);
                    continue;
                }
            };
            let callee = match parse_call_endpoint(
                callee_value,
                context.root,
                context.allowed_files,
                context.active.encoding,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    coverage.push(reason);
                    continue;
                }
            };
            let call_ranges = ranges.cloned().unwrap_or_default();
            if call_ranges.is_empty() {
                coverage.push("调用层级响应缺少调用位置".to_string());
            }
            for range in call_ranges {
                let Some(range_uri) = range_uri else {
                    coverage.push("调用层级响应缺少调用文件".to_string());
                    continue;
                };
                let location = json!({"uri":range_uri,"range":range});
                match parse_location(
                    &location,
                    context.root,
                    context.allowed_files,
                    context.active.encoding,
                ) {
                    Ok(call_site) => {
                        let recursive = caller.location == callee.location;
                        items.push(SemanticItem {
                            target: callee.location.clone(),
                            caller: Some(caller.clone()),
                            callee: Some(callee.clone()),
                            call_site: Some(call_site),
                            recursive,
                        });
                    }
                    Err(reason) => coverage.push(reason),
                }
            }
        }
    }
    deduplicate(&mut items);
    Ok(QueryOutcome {
        items,
        coverage,
        reason: None,
        rpc_count,
    })
}

fn parse_call_endpoint(
    value: &Value,
    root: &Path,
    allowed_files: &HashSet<String>,
    encoding: CharacterEncoding,
) -> Result<CallEndpoint, String> {
    let location = parse_location(value, root, allowed_files, encoding)?;
    Ok(CallEndpoint {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("未知符号")
            .to_string(),
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        location,
    })
}

fn location_values(value: &Value) -> Vec<&Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(values) => values.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    }
}

fn parse_location(
    value: &Value,
    root: &Path,
    allowed_files: &HashSet<String>,
    encoding: CharacterEncoding,
) -> Result<SemanticLocation, String> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))
        .and_then(Value::as_str)
        .ok_or_else(|| "LSP 位置缺少 URI".to_string())?;
    let range_value = value
        .get("selectionRange")
        .or_else(|| value.get("targetSelectionRange"))
        .or_else(|| value.get("range"))
        .or_else(|| value.get("targetRange"))
        .ok_or_else(|| "LSP 位置缺少 range".to_string())?;
    let range: LspRange = serde_json::from_value(range_value.clone())
        .map_err(|error| format!("LSP range 无效：{error}"))?;
    if !uri.starts_with("file://") {
        return Ok(SemanticLocation {
            relative_path: None,
            external_uri_hint: Some(uri.to_string()),
            line: range.start.line + 1,
            column: range.start.character + 1,
            end_line: range.end.line + 1,
            end_column: range.end.character + 1,
            start_byte: None,
            end_byte: None,
        });
    }
    let path = file_uri_to_path(uri)?;
    let canonical =
        std::fs::canonicalize(&path).map_err(|error| format!("定位目标不存在：{error}"))?;
    let canonical_root =
        std::fs::canonicalize(root).map_err(|error| format!("项目根目录无效：{error}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Ok(SemanticLocation {
            relative_path: None,
            external_uri_hint: Some(uri.to_string()),
            line: range.start.line + 1,
            column: range.start.character + 1,
            end_line: range.end.line + 1,
            end_column: range.end.character + 1,
            start_byte: None,
            end_byte: None,
        });
    }
    let relative = canonical
        .strip_prefix(&canonical_root)
        .map_err(|_| "定位目标超出项目范围".to_string())?
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string();
    if !allowed_files.contains(&relative) {
        return Err(format!("定位目标未通过源码白名单：{relative}"));
    }
    let text = std::fs::read_to_string(&canonical)
        .map_err(|error| format!("定位目标不是可读 UTF-8 文本：{error}"))?;
    let start = lsp_position_to_byte_offset(&text, range.start, encoding)?;
    let end = lsp_position_to_byte_offset(&text, range.end, encoding)?;
    let start_scalar = byte_to_scalar_position(&text, start)?;
    let end_scalar = byte_to_scalar_position(&text, end)?;
    Ok(SemanticLocation {
        relative_path: Some(relative),
        external_uri_hint: None,
        line: start_scalar.0,
        column: start_scalar.1,
        end_line: end_scalar.0,
        end_column: end_scalar.1,
        start_byte: Some(start as u64),
        end_byte: Some(end as u64),
    })
}

fn byte_to_scalar_position(text: &str, offset: usize) -> Result<(u32, u32), String> {
    let utf32 = byte_offset_to_lsp_position(text, offset, CharacterEncoding::Utf32)?;
    Ok((utf32.line + 1, utf32.character + 1))
}

fn remaining(deadline: Instant) -> Result<Duration, SemanticServiceError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(LspClientError::Timeout.into())
}

fn deduplicate(items: &mut Vec<SemanticItem>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn note_external_location(location: &SemanticLocation, coverage: &mut Vec<String>) {
    if location.external_uri_hint.is_some() {
        coverage.push("目标位于项目外部依赖，仅保留不可读位置提示".to_string());
    }
}

fn next_request_id(inner: &mut ServiceInner) -> u64 {
    let id = inner.next_request;
    inner.next_request = inner.next_request.saturating_add(1);
    id
}

fn is_build_configuration(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(
        name,
        "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | ".classpath"
            | ".project"
    )
}

fn collect_symbol_selections(
    value: &Value,
    approximate_line: u32,
    expected_name: Option<&str>,
    output: &mut Vec<LspPosition>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_symbol_selections(value, approximate_line, expected_name, output);
            }
        }
        Value::Object(object) => {
            let name_matches = expected_name.is_none_or(|expected| {
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|actual| {
                        actual == expected
                            || actual
                                .strip_prefix(expected)
                                .is_some_and(|tail| tail.starts_with('('))
                    })
            });
            let covers_line = object
                .get("range")
                .or_else(|| {
                    object
                        .get("location")
                        .and_then(|location| location.get("range"))
                })
                .and_then(|range| serde_json::from_value::<LspRange>(range.clone()).ok())
                .is_some_and(|range| {
                    range.start.line <= approximate_line && approximate_line <= range.end.line
                });
            if name_matches
                && covers_line
                && let Some(position) = object
                    .get("selectionRange")
                    .and_then(|range| serde_json::from_value::<LspRange>(range.clone()).ok())
                    .map(|range| range.start)
                    .or_else(|| {
                        object
                            .get("location")
                            .and_then(|location| location.get("range"))
                            .and_then(|range| {
                                serde_json::from_value::<LspRange>(range.clone()).ok()
                            })
                            .map(|range| range.start)
                    })
            {
                output.push(position);
            }
            if let Some(children) = object.get("children") {
                collect_symbol_selections(children, approximate_line, expected_name, output);
            }
        }
        _ => {}
    }
}
