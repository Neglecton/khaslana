//! V2 代码理解 agent 的六个基础只读工具与可选 Java 语义入口。
//!
//! 索引结果只用于导航；所有源码正文都通过 [`SourceService`] 重新校验并发放
//! 来源 ID。候选 ID 属于本会话，模型不能用任意路径/行号伪造一个符号候选。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ai::ToolSchema;
use crate::ai::review_store::repo_key;
use crate::code_index::{
    DetailOutcome, ProjectContext, TraceDirection, TraceOutcome, open_read_only_if_exists,
    search_symbols_filtered, symbol_detail, trace_calls,
};
use crate::lsp::{
    Availability, LspSemanticService, PluginState, RequestCancellation, SemanticAnchor,
    SemanticOperation, SemanticQueryResult, SemanticServiceError, ServiceStatus,
};

use super::source::{
    FileTreeResult, SourceRead, SourceSearchResult, SourceService, UnderstandingResult,
};
use super::{ErrorCode, UnderstandingError};

const SYMBOL_SEARCH_DEFAULT_LIMIT: usize = 20;
const SYMBOL_SEARCH_MAX_LIMIT: usize = 50;
const SYMBOL_SEARCH_FETCH_LIMIT: usize = 1_000;
const TRACE_DEFAULT_DEPTH: u32 = 1;
const TRACE_MAX_DEPTH: u32 = 3;
const TRACE_DEFAULT_LIMIT: usize = 20;
const TRACE_MAX_LIMIT: usize = 40;
pub const JAVA_SEMANTIC_MAX_RPC_PER_TOOL: usize = 6;
pub const JAVA_SEMANTIC_MAX_RPC_PER_QUESTION: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEnvelope<T> {
    pub request_id: String,
    pub index_generation: u64,
    pub data: T,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSymbolsArgs {
    pub query: String,
    pub path_prefix: Option<String>,
    pub language: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolCandidateView {
    pub candidate_id: String,
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub relative_path: String,
    pub start_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSymbolsResult {
    pub candidates: Vec<SymbolCandidateView>,
    pub matched_before_scope_filter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetSymbolArgs {
    #[serde(default)]
    pub candidate_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRelationView {
    pub name: String,
    pub qualified_name: String,
    pub relative_path: String,
    pub hop: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolDetailView {
    pub candidate_id: String,
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub callers: Vec<SymbolRelationView>,
    pub callees: Vec<SymbolRelationView>,
    pub source: Option<SourceRead>,
    pub source_unavailable_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallTraceDirection {
    Inbound,
    Outbound,
    #[default]
    Both,
}

impl From<CallTraceDirection> for TraceDirection {
    fn from(value: CallTraceDirection) -> Self {
        match value {
            CallTraceDirection::Inbound => Self::Inbound,
            CallTraceDirection::Outbound => Self::Outbound,
            CallTraceDirection::Both => Self::Both,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCallsArgs {
    #[serde(default)]
    pub candidate_id: String,
    #[serde(default)]
    pub direction: CallTraceDirection,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCallsResult {
    pub function: String,
    pub direction: CallTraceDirection,
    pub callers: Vec<SymbolRelationView>,
    pub callees: Vec<SymbolRelationView>,
    /// 明确提醒 agent：无边不等于无调用，关键关系仍需 read/search 查证。
    pub coverage_note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticTraceResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum JavaSemanticAnchor {
    Candidate {
        candidate_id: String,
    },
    Source {
        source_id: String,
        line: u32,
        column: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryJavaSemanticsArgs {
    pub operation: SemanticOperation,
    pub anchor: JavaSemanticAnchor,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticSourceLink {
    pub role: String,
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub source_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaSemanticQueryResult {
    pub status: ServiceStatus,
    pub availability: Availability,
    pub operation: SemanticOperation,
    pub semantic: Option<SemanticQueryResult>,
    pub source_links: Vec<SemanticSourceLink>,
    pub message: String,
    pub rpc_budget_remaining: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticTraceResult {
    pub depth: u32,
    pub inbound: Option<JavaSemanticQueryResult>,
    pub outbound: Option<JavaSemanticQueryResult>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchCodeArgs {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFileArgs {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetFileTreeArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub depth: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
struct RegisteredCandidate {
    qualified_name: String,
    /// 索引标签（Class/Method/Field…）。`trace_calls` 只解析可调用符号，
    /// 需要据此把「不是函数/方法」与「候选已消失」区分开。
    label: String,
    name: String,
    relative_path: String,
    start_line: u32,
    end_line: u32,
    semantic_anchors: Vec<SemanticAnchor>,
}

/// 会话候选注册表：对外用短序号 `sc1:N`，而不是 64 位内容摘要。
///
/// 与来源 ID 同理（见 `source::source_id_of` 注释）：真实模型无法可靠转录
/// 长十六进制串，抄错后会被当成「不是本次搜索的候选」，`get_symbol` /
/// `trace_calls` 又把它报成疑似歧义，模型会反复重试同一个坏 ID。
#[derive(Default)]
struct CandidateRegistry {
    by_id: HashMap<String, RegisteredCandidate>,
    /// 同一（限定名, 路径）重复搜索返回同一 ID。
    by_key: HashMap<(String, String), String>,
    next: u64,
}

impl CandidateRegistry {
    fn register(
        &mut self,
        qualified_name: &str,
        label: &str,
        name: &str,
        file_path: &str,
        start_line: u32,
        end_line: u32,
    ) -> String {
        let key = (qualified_name.to_string(), file_path.to_string());
        if let Some(existing) = self.by_key.get(&key) {
            return existing.clone();
        }
        self.next += 1;
        let id = format!("sc1:{}", self.next);
        self.by_id.insert(
            id.clone(),
            RegisteredCandidate {
                qualified_name: qualified_name.to_string(),
                label: label.to_string(),
                name: name.to_string(),
                relative_path: file_path.to_string(),
                start_line,
                end_line,
                semantic_anchors: Vec::new(),
            },
        );
        self.by_key.insert(key, id.clone());
        id
    }

    fn get(&self, id: &str) -> Option<&RegisteredCandidate> {
        self.by_id.get(id)
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.by_id.keys()
    }

    fn set_semantic_anchors(&mut self, id: &str, anchors: Vec<SemanticAnchor>) {
        if let Some(candidate) = self.by_id.get_mut(id) {
            candidate.semantic_anchors = anchors;
        }
    }
}

pub(crate) trait JavaSemanticBackend: Send + Sync {
    fn plugin_state(&self) -> PluginState;
    fn status(&self, project_key: &str) -> ServiceStatus;
    fn register_source_anchor(
        &self,
        source_service: &SourceService,
        source_id: &str,
        line: u32,
        column: u32,
    ) -> Result<SemanticAnchor, SemanticServiceError>;
    fn register_source_symbol_anchors(
        &self,
        source_service: &SourceService,
        source_id: &str,
        approximate_line: u32,
        expected_name: Option<&str>,
        cancellation: &RequestCancellation,
    ) -> Result<Vec<SemanticAnchor>, SemanticServiceError>;
    fn query(
        &self,
        anchor_id: &str,
        operation: SemanticOperation,
        limit: Option<usize>,
        rpc_budget: usize,
        cancellation: &RequestCancellation,
    ) -> Result<SemanticQueryResult, SemanticServiceError>;
}

impl JavaSemanticBackend for LspSemanticService {
    fn plugin_state(&self) -> PluginState {
        self.plugin_state("java-jdtls")
    }

    fn status(&self, project_key: &str) -> ServiceStatus {
        self.status(project_key, "java")
    }

    fn register_source_anchor(
        &self,
        source_service: &SourceService,
        source_id: &str,
        line: u32,
        column: u32,
    ) -> Result<SemanticAnchor, SemanticServiceError> {
        self.register_source_anchor(source_service, source_id, line, column)
    }

    fn register_source_symbol_anchors(
        &self,
        source_service: &SourceService,
        source_id: &str,
        approximate_line: u32,
        expected_name: Option<&str>,
        cancellation: &RequestCancellation,
    ) -> Result<Vec<SemanticAnchor>, SemanticServiceError> {
        self.register_source_symbol_anchors(
            source_service,
            source_id,
            approximate_line,
            expected_name,
            cancellation,
        )
    }

    fn query(
        &self,
        anchor_id: &str,
        operation: SemanticOperation,
        limit: Option<usize>,
        rpc_budget: usize,
        cancellation: &RequestCancellation,
    ) -> Result<SemanticQueryResult, SemanticServiceError> {
        self.query_with_rpc_budget(anchor_id, operation, limit, rpc_budget, cancellation)
    }
}

#[derive(Default)]
struct SemanticQuestionBudget {
    rpc_calls: usize,
    consecutive_failures: usize,
}

struct SemanticToolSession {
    backend: Arc<dyn JavaSemanticBackend>,
    budget: Mutex<SemanticQuestionBudget>,
}

/// 单个问题/同一索引代际内使用的工具会话。
pub struct UnderstandingTools {
    context: ProjectContext,
    index_db_path: PathBuf,
    generation: u64,
    indexed_hashes: HashMap<String, String>,
    source: SourceService,
    candidates: Mutex<CandidateRegistry>,
    semantic: Option<SemanticToolSession>,
}

impl UnderstandingTools {
    pub fn open(repo_root: &Path, index_db_path: &Path) -> UnderstandingResult<Self> {
        Self::open_with_backend(repo_root, index_db_path, None)
    }

    pub fn open_with_semantic_service(
        repo_root: &Path,
        index_db_path: &Path,
        service: Arc<LspSemanticService>,
    ) -> UnderstandingResult<Self> {
        Self::open_with_backend(repo_root, index_db_path, Some(service))
    }

    pub(crate) fn open_with_backend(
        repo_root: &Path,
        index_db_path: &Path,
        backend: Option<Arc<dyn JavaSemanticBackend>>,
    ) -> UnderstandingResult<Self> {
        let canonical_root = std::fs::canonicalize(repo_root).map_err(|error| {
            UnderstandingError::new(
                ErrorCode::SourceMissing,
                format!("项目根目录不存在或无法读取：{error}"),
            )
        })?;
        let store = open_read_only_if_exists(index_db_path)
            .map_err(index_error)?
            .ok_or_else(|| UnderstandingError::new(ErrorCode::IndexMissing, "代码索引尚未建立"))?;
        let stats = store.read_stats().map_err(index_error)?.ok_or_else(|| {
            UnderstandingError::new(ErrorCode::IndexMissing, "代码索引为空，请先建立索引")
        })?;
        if !stats.repo_path.is_empty() {
            let indexed_root = std::fs::canonicalize(&stats.repo_path).map_err(|error| {
                UnderstandingError::new(
                    ErrorCode::SourceMissing,
                    format!("索引记录的项目根目录不可用：{error}"),
                )
            })?;
            if indexed_root != canonical_root {
                return Err(UnderstandingError::new(
                    ErrorCode::OutsideProject,
                    "索引库不属于当前项目",
                ));
            }
        }
        let generation = store.generation().map_err(index_error)?;
        let indexed_hashes = store
            .load_file_hashes()
            .map_err(index_error)?
            .into_iter()
            .map(|file| (file.rel_path, file.sha256))
            .collect();
        let project_key = repo_key(&canonical_root.to_string_lossy());
        let context = ProjectContext {
            project_key,
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            index_db_path: index_db_path.to_string_lossy().into_owned(),
        };
        let source = SourceService::open(context.clone(), generation)?;
        let semantic = backend.and_then(|backend| {
            let plugin = backend.plugin_state();
            let status = backend.status(&context.project_key);
            (plugin.registered && plugin.enabled && status != ServiceStatus::Off).then_some(
                SemanticToolSession {
                    backend,
                    budget: Mutex::new(SemanticQuestionBudget::default()),
                },
            )
        });
        Ok(Self {
            context,
            index_db_path: index_db_path.to_path_buf(),
            generation,
            indexed_hashes,
            source,
            candidates: Mutex::new(CandidateRegistry::default()),
            semantic,
        })
    }

    pub fn semantic_tool_enabled(&self) -> bool {
        self.semantic.is_some()
    }

    pub fn context(&self) -> &ProjectContext {
        &self.context
    }

    pub fn source_service(&self) -> &SourceService {
        &self.source
    }

    /// 本次工具会话固定的索引代际（答案由服务覆盖写入，模型不能自报）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 本会话已发放的来源（按路径与起始行排序）。
    pub fn issued_sources(&self) -> Vec<crate::code_index::SourceRef> {
        self.source.issued_sources()
    }

    pub fn search_symbols(
        &self,
        request_id: &str,
        args: SearchSymbolsArgs,
    ) -> UnderstandingResult<ToolEnvelope<SearchSymbolsResult>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        if args.query.trim().is_empty() {
            return Err(UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                "符号搜索关键词不能为空",
            ));
        }
        let requested_limit = args.limit.unwrap_or(SYMBOL_SEARCH_DEFAULT_LIMIT);
        let limit = requested_limit.clamp(1, SYMBOL_SEARCH_MAX_LIMIT);
        let (hits, raw_total) = search_symbols_filtered(
            &self.index_db_path,
            &args.query,
            None,
            SYMBOL_SEARCH_FETCH_LIMIT,
        )
        .map_err(index_error)?;
        let path_prefix = args
            .path_prefix
            .as_deref()
            .map(|value| value.trim_matches('/'))
            .filter(|value| !value.is_empty());
        let language = args
            .language
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut scoped = Vec::new();
        for hit in hits {
            if let Some(prefix) = path_prefix
                && !path_matches_prefix(&hit.file_path, prefix)
            {
                continue;
            }
            if let Some(language) = language
                && !path_matches_language(&hit.file_path, language)
            {
                continue;
            }
            let candidate_id = self
                .candidates
                .lock()
                .expect("候选注册表锁被污染")
                .register(
                    &hit.qualified_name,
                    &hit.label,
                    &hit.name,
                    &hit.file_path,
                    hit.start_line,
                    hit.start_line,
                );
            scoped.push(SymbolCandidateView {
                candidate_id,
                name: hit.name,
                label: hit.label,
                qualified_name: hit.qualified_name,
                relative_path: hit.file_path,
                start_line: hit.start_line,
            });
        }
        let scoped_total = scoped.len();
        scoped.truncate(limit);
        let mut reasons = Vec::new();
        if requested_limit > SYMBOL_SEARCH_MAX_LIMIT {
            reasons.push(format!(
                "符号搜索上限为 {SYMBOL_SEARCH_MAX_LIMIT} 条，已自动收紧"
            ));
        }
        if raw_total > SYMBOL_SEARCH_FETCH_LIMIT {
            reasons.push(format!(
                "索引命中超过内部候选上限 {SYMBOL_SEARCH_FETCH_LIMIT} 条，范围过滤基于前一批候选"
            ));
        }
        if scoped_total > limit {
            reasons.push(format!("范围内候选超过本次上限 {limit} 条"));
        }
        Ok(envelope(
            request_id,
            self.generation,
            SearchSymbolsResult {
                candidates: scoped,
                matched_before_scope_filter: raw_total,
            },
            reasons,
        ))
    }

    pub fn get_symbol(
        &self,
        request_id: &str,
        args: GetSymbolArgs,
    ) -> UnderstandingResult<ToolEnvelope<SymbolDetailView>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        let candidate = self.resolve_candidate(&args.candidate_id)?;
        let detail = match symbol_detail(&self.index_db_path, None, &candidate.qualified_name)
            .map_err(index_error)?
        {
            DetailOutcome::Found(detail) => detail,
            DetailOutcome::Ambiguous(_) => {
                return Err(UnderstandingError::new(
                    ErrorCode::AmbiguousSymbol,
                    "会话候选不再能唯一解析，请重新搜索",
                ));
            }
            DetailOutcome::NotFound => {
                return Err(UnderstandingError::new(
                    ErrorCode::CursorExpired,
                    "会话候选已不在当前索引中，请重新搜索",
                ));
            }
        };
        let (source, source_unavailable_reason) = if detail.file_path.is_empty()
            || detail.start_line == 0
        {
            (None, Some("该索引节点没有可读取的源码位置".to_string()))
        } else {
            match self.source.read_file(
                &detail.file_path,
                detail.start_line,
                detail.end_line.max(detail.start_line),
            ) {
                Ok(source)
                    if self
                        .indexed_hashes
                        .get(&detail.file_path)
                        .is_none_or(|hash| hash == &source.source_ref.content_sha256) =>
                {
                    (Some(source), None)
                }
                Ok(_) => (
                    None,
                    Some(
                        "[SourceChanged] 索引定义位置对应较旧文件版本；请更新索引，或用 search_code/read_file 查找当前源码"
                            .to_string(),
                    ),
                ),
                Err(error) => (None, Some(error.to_string())),
            }
        };
        let view = SymbolDetailView {
            candidate_id: args.candidate_id,
            name: detail.name,
            label: detail.label,
            qualified_name: detail.qualified_name,
            relative_path: detail.file_path,
            start_line: detail.start_line,
            end_line: detail.end_line,
            callers: detail.callers.into_iter().map(relation_view).collect(),
            callees: detail.callees.into_iter().map(relation_view).collect(),
            source,
            source_unavailable_reason,
        };
        Ok(envelope(request_id, self.generation, view, Vec::new()))
    }

    pub fn trace_calls(
        &self,
        request_id: &str,
        args: TraceCallsArgs,
    ) -> UnderstandingResult<ToolEnvelope<TraceCallsResult>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        let candidate = self.resolve_candidate(&args.candidate_id)?;
        let requested_depth = args.depth.unwrap_or(TRACE_DEFAULT_DEPTH);
        let depth = requested_depth.clamp(1, TRACE_MAX_DEPTH);
        let requested_limit = args.limit.unwrap_or(TRACE_DEFAULT_LIMIT);
        let limit = requested_limit.clamp(1, TRACE_MAX_LIMIT);
        let per_direction_limit = if args.direction == CallTraceDirection::Both {
            limit.div_ceil(2)
        } else {
            limit
        };
        let result = match trace_calls(
            &self.index_db_path,
            &candidate.qualified_name,
            args.direction.into(),
            depth,
            per_direction_limit,
        )
        .map_err(index_error)?
        {
            TraceOutcome::Found(result) => result,
            TraceOutcome::Ambiguous(_) => {
                return Err(UnderstandingError::new(
                    ErrorCode::AmbiguousSymbol,
                    "会话候选不再能唯一解析，请重新搜索",
                ));
            }
            TraceOutcome::NotFound => {
                // `trace_calls` 只解析函数/方法（callable_only）。候选是类、
                // 字段等非可调用节点时也会走这里，不能报成「索引代际变化」——
                // 那会把模型误导成索引失效，进而放弃调用链调查。
                let message = if is_callable_label(&candidate.label) {
                    "会话候选已不在当前索引中，请重新搜索"
                } else {
                    "该候选不是可调用符号（trace_calls 只追踪函数与方法）；请对其中的具体方法发起追踪"
                };
                return Err(UnderstandingError::new(ErrorCode::CursorExpired, message));
            }
        };
        let hit_limit = result.callers.len() >= per_direction_limit
            || result.callees.len() >= per_direction_limit;
        let mut reasons = Vec::new();
        if requested_depth > TRACE_MAX_DEPTH {
            reasons.push(format!("调用追踪深度上限为 {TRACE_MAX_DEPTH} 跳"));
        }
        if requested_limit > TRACE_MAX_LIMIT {
            reasons.push(format!("调用追踪符号上限为 {TRACE_MAX_LIMIT} 个"));
        }
        if hit_limit {
            reasons.push(format!("调用关系可能超过本次上限 {limit} 个"));
        }
        let semantic = self.semantic_trace(&args.candidate_id, &candidate, args.direction, limit);
        Ok(envelope(
            request_id,
            self.generation,
            TraceCallsResult {
                function: result.function,
                direction: args.direction,
                callers: result.callers.into_iter().map(relation_view).collect(),
                callees: result.callees.into_iter().map(relation_view).collect(),
                coverage_note: "这是当前索引中的调用线索；无边不表示不存在调用，关键关系请继续读取调用位置或搜索源码。".to_string(),
                semantic,
            },
            reasons,
        ))
    }

    pub fn query_java_semantics(
        &self,
        request_id: &str,
        args: QueryJavaSemanticsArgs,
    ) -> UnderstandingResult<ToolEnvelope<JavaSemanticQueryResult>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        if args
            .limit
            .is_some_and(|limit| !(1..=TRACE_MAX_LIMIT).contains(&limit))
        {
            return Err(UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                format!("query_java_semantics 的 limit 必须在 1 到 {TRACE_MAX_LIMIT} 之间"),
            ));
        }
        let semantic = self.semantic.as_ref().ok_or_else(|| {
            UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                "本问开始时未启用 Java 语义工具；请继续使用基础索引和源码工具",
            )
        })?;
        let result = self.run_semantic_query(
            semantic,
            args.operation,
            &args.anchor,
            args.limit,
            JAVA_SEMANTIC_MAX_RPC_PER_TOOL,
        )?;
        let reasons = result
            .semantic
            .as_ref()
            .map(|result| result.coverage.clone())
            .unwrap_or_default();
        Ok(envelope(request_id, self.generation, result, reasons))
    }

    fn semantic_trace(
        &self,
        candidate_id: &str,
        candidate: &RegisteredCandidate,
        direction: CallTraceDirection,
        limit: usize,
    ) -> Option<SemanticTraceResult> {
        let semantic = self.semantic.as_ref()?;
        if !candidate
            .relative_path
            .to_ascii_lowercase()
            .ends_with(".java")
        {
            return None;
        }
        let anchor = JavaSemanticAnchor::Candidate {
            candidate_id: candidate_id.to_string(),
        };
        let (inbound_budget, outbound_budget) = match direction {
            CallTraceDirection::Inbound => (JAVA_SEMANTIC_MAX_RPC_PER_TOOL, 0),
            CallTraceDirection::Outbound => (0, JAVA_SEMANTIC_MAX_RPC_PER_TOOL),
            // 候选首次精确定位最多占 1 RPC，剩余额度在两个方向间分配。
            CallTraceDirection::Both => (3, 3),
        };
        let inbound = (inbound_budget > 0).then(|| {
            self.run_semantic_query(
                semantic,
                SemanticOperation::IncomingCalls,
                &anchor,
                Some(limit.div_ceil(2)),
                inbound_budget,
            )
            .unwrap_or_else(|error| {
                self.semantic_error_result(
                    semantic,
                    SemanticOperation::IncomingCalls,
                    error.to_string(),
                )
            })
        });
        let outbound = (outbound_budget > 0).then(|| {
            self.run_semantic_query(
                semantic,
                SemanticOperation::OutgoingCalls,
                &anchor,
                Some(limit.div_ceil(2)),
                outbound_budget,
            )
            .unwrap_or_else(|error| {
                self.semantic_error_result(
                    semantic,
                    SemanticOperation::OutgoingCalls,
                    error.to_string(),
                )
            })
        });
        Some(SemanticTraceResult {
            depth: 1,
            inbound,
            outbound,
            note: "semantic 仅是一跳静态关系，和基础索引结果分别保留；冲突时必须读取 call_site，不得静默合并。".to_string(),
        })
    }

    fn run_semantic_query(
        &self,
        semantic: &SemanticToolSession,
        operation: SemanticOperation,
        anchor_input: &JavaSemanticAnchor,
        limit: Option<usize>,
        tool_rpc_budget: usize,
    ) -> UnderstandingResult<JavaSemanticQueryResult> {
        let project_key = &self.context.project_key;
        let status = semantic.backend.status(project_key);
        let plugin = semantic.backend.plugin_state();
        if !plugin.registered || !plugin.enabled {
            return Ok(self.semantic_unavailable_result(
                semantic,
                operation,
                status,
                "Java 语义插件已禁用或不再可用；本问继续基础路径",
            ));
        }
        if !matches!(status, ServiceStatus::Ready | ServiceStatus::Partial) {
            return Ok(self.semantic_unavailable_result(
                semantic,
                operation,
                status,
                format!("Java 语义服务当前状态为 {status:?}；工具不会等待、启动或重配服务"),
            ));
        }

        let mut used_by_tool = 0usize;
        let anchor = match anchor_input {
            JavaSemanticAnchor::Source {
                source_id,
                line,
                column,
            } => {
                let source = self.source.validate_source(source_id)?;
                if !source.relative_path.to_ascii_lowercase().ends_with(".java") {
                    return Err(UnderstandingError::new(
                        ErrorCode::AnswerInvalid,
                        "query_java_semantics 的 source anchor 必须指向 Java 文件",
                    ));
                }
                semantic
                    .backend
                    .register_source_anchor(&self.source, source_id, *line, *column)
                    .map_err(semantic_input_error)?
            }
            JavaSemanticAnchor::Candidate { candidate_id } => {
                let candidate = self.resolve_candidate(candidate_id)?;
                if !candidate
                    .relative_path
                    .to_ascii_lowercase()
                    .ends_with(".java")
                {
                    return Err(UnderstandingError::new(
                        ErrorCode::AnswerInvalid,
                        "query_java_semantics 的 candidate anchor 必须指向 Java 符号",
                    ));
                }
                if let [only] = candidate.semantic_anchors.as_slice() {
                    only.clone()
                } else {
                    if !candidate.semantic_anchors.is_empty() {
                        return Ok(self.semantic_unavailable_result(
                            semantic,
                            operation,
                            status,
                            "候选对应多个同名/重载位置；请读取源码后使用带行列的 source anchor 消歧",
                        ));
                    }
                    if !self.semantic_rpc_available(semantic, 1, tool_rpc_budget, used_by_tool) {
                        return Ok(self.semantic_unavailable_result(
                            semantic,
                            operation,
                            status,
                            "Java 语义 RPC 预算已用尽；请继续基础路径",
                        ));
                    }
                    let source = self.source.read_file(
                        &candidate.relative_path,
                        candidate.start_line.max(1),
                        candidate.end_line.max(candidate.start_line).max(1),
                    )?;
                    if self
                        .indexed_hashes
                        .get(&candidate.relative_path)
                        .is_some_and(|hash| hash != &source.source_ref.content_sha256)
                    {
                        return Ok(self.semantic_unavailable_result(
                            semantic,
                            operation,
                            ServiceStatus::Partial,
                            "[SourceChanged] 候选位置对应较旧文件版本；请更新索引或重新读取源码",
                        ));
                    }
                    used_by_tool += 1;
                    self.note_semantic_rpc(semantic, 1);
                    let anchors = semantic
                        .backend
                        .register_source_symbol_anchors(
                            &self.source,
                            &source.source_id,
                            candidate.start_line.max(1),
                            Some(&candidate.name),
                            &RequestCancellation::default(),
                        )
                        .map_err(semantic_input_error)?;
                    self.candidates
                        .lock()
                        .expect("候选注册表锁被污染")
                        .set_semantic_anchors(candidate_id, anchors.clone());
                    match anchors.as_slice() {
                        [only] => only.clone(),
                        [] => {
                            return Ok(self.semantic_unavailable_result(
                                semantic,
                                operation,
                                ServiceStatus::Partial,
                                "JDT 未在候选 selectionRange 找到精确位置；不得以行首或同名文本猜测",
                            ));
                        }
                        _ => {
                            return Ok(self.semantic_unavailable_result(
                                semantic,
                                operation,
                                ServiceStatus::Partial,
                                "候选对应多个同名/重载位置；请读取源码后使用带行列的 source anchor 消歧",
                            ));
                        }
                    }
                }
            }
        };

        let available_for_query = self.semantic_rpc_remaining(semantic);
        let rpc_budget = tool_rpc_budget
            .saturating_sub(used_by_tool)
            .min(available_for_query);
        if rpc_budget == 0 {
            return Ok(self.semantic_unavailable_result(
                semantic,
                operation,
                status,
                "Java 语义 RPC 预算已用尽；请继续基础路径",
            ));
        }
        let query = semantic.backend.query(
            &anchor.anchor_id,
            operation,
            limit,
            rpc_budget,
            &RequestCancellation::default(),
        );
        let query = match query {
            Ok(query) => {
                self.note_semantic_rpc(semantic, query.rpc_count);
                semantic
                    .budget
                    .lock()
                    .expect("语义预算锁被污染")
                    .consecutive_failures = 0;
                query
            }
            Err(error) => {
                // 失败响应无法可靠得知服务端已消费几次请求，按本次可用额度保守记账。
                self.note_semantic_rpc(semantic, rpc_budget);
                return Ok(self.semantic_error_result(semantic, operation, error.to_string()));
            }
        };
        let source_links = self.issue_semantic_sources(&query);
        let availability = query.availability;
        let message = if query.items.is_empty() {
            "语义查询未返回位置；这不是‘不存在调用/实现’的否定证明".to_string()
        } else {
            "语义位置是静态导航线索；业务结论仍须读取 source_links 指向的源码核对".to_string()
        };
        Ok(JavaSemanticQueryResult {
            status,
            availability,
            operation,
            semantic: Some(query),
            source_links,
            message,
            rpc_budget_remaining: self.semantic_rpc_remaining(semantic),
        })
    }

    pub fn search_code(
        &self,
        request_id: &str,
        args: SearchCodeArgs,
    ) -> UnderstandingResult<ToolEnvelope<SourceSearchResult>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        let data = self.source.search_code(
            &args.query,
            args.regex,
            args.path_prefix.as_deref(),
            args.suffix.as_deref(),
            args.limit.unwrap_or(20),
        )?;
        Ok(ToolEnvelope {
            request_id: request_id.to_string(),
            index_generation: self.generation,
            truncated: data.truncated,
            truncation_reasons: data.truncation_reasons.clone(),
            data,
        })
    }

    pub fn read_file(
        &self,
        request_id: &str,
        args: ReadFileArgs,
    ) -> UnderstandingResult<ToolEnvelope<SourceRead>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        let start = args.start_line.unwrap_or(1);
        let end = args.end_line.unwrap_or_else(|| start.saturating_add(199));
        let data = self.source.read_file(&args.path, start, end)?;
        let reasons = data.truncation_reason.clone().into_iter().collect();
        Ok(ToolEnvelope {
            request_id: request_id.to_string(),
            index_generation: self.generation,
            truncated: data.truncated,
            truncation_reasons: reasons,
            data,
        })
    }

    pub fn get_file_tree(
        &self,
        request_id: &str,
        args: GetFileTreeArgs,
    ) -> UnderstandingResult<ToolEnvelope<FileTreeResult>> {
        validate_request_id(request_id)?;
        self.ensure_generation()?;
        let data = self.source.get_file_tree(
            args.path.as_deref(),
            args.depth.unwrap_or(2),
            args.limit.unwrap_or(200),
        )?;
        let reasons = data.truncation_reason.clone().into_iter().collect();
        Ok(ToolEnvelope {
            request_id: request_id.to_string(),
            index_generation: self.generation,
            truncated: data.truncated,
            truncation_reasons: reasons,
            data,
        })
    }

    fn issue_semantic_sources(&self, result: &SemanticQueryResult) -> Vec<SemanticSourceLink> {
        let mut locations = Vec::new();
        for item in &result.items {
            locations.push(("target", &item.target));
            if let Some(caller) = item.caller.as_ref() {
                locations.push(("caller", &caller.location));
            }
            if let Some(callee) = item.callee.as_ref() {
                locations.push(("callee", &callee.location));
            }
            if let Some(call_site) = item.call_site.as_ref() {
                locations.push(("call_site", call_site));
            }
        }
        let mut links = Vec::new();
        for (role, location) in locations {
            let Some(path) = location.relative_path.as_ref() else {
                continue;
            };
            let start_line = location.line.max(1);
            let end_line = location.end_line.max(start_line);
            let (source_id, unavailable_reason) =
                match self.source.read_file(path, start_line, end_line) {
                    Ok(source) => (Some(source.source_id), None),
                    Err(error) => (None, Some(error.to_string())),
                };
            links.push(SemanticSourceLink {
                role: role.to_string(),
                relative_path: path.clone(),
                start_line,
                end_line,
                source_id,
                unavailable_reason,
            });
        }
        links.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.role.cmp(&right.role))
        });
        links.dedup_by(|left, right| {
            left.role == right.role
                && left.relative_path == right.relative_path
                && left.start_line == right.start_line
                && left.end_line == right.end_line
        });
        links
    }

    fn semantic_unavailable_result(
        &self,
        semantic: &SemanticToolSession,
        operation: SemanticOperation,
        status: ServiceStatus,
        message: impl Into<String>,
    ) -> JavaSemanticQueryResult {
        let message = self.note_semantic_failure(semantic, message.into());
        JavaSemanticQueryResult {
            status,
            availability: Availability::Unavailable,
            operation,
            semantic: None,
            source_links: Vec::new(),
            message,
            rpc_budget_remaining: self.semantic_rpc_remaining(semantic),
        }
    }

    fn semantic_error_result(
        &self,
        semantic: &SemanticToolSession,
        operation: SemanticOperation,
        message: String,
    ) -> JavaSemanticQueryResult {
        let status = semantic.backend.status(&self.context.project_key);
        self.semantic_unavailable_result(
            semantic,
            operation,
            status,
            semantic_service_message(&message),
        )
    }

    fn note_semantic_failure(&self, semantic: &SemanticToolSession, message: String) -> String {
        let mut budget = semantic.budget.lock().expect("语义预算锁被污染");
        budget.consecutive_failures = budget.consecutive_failures.saturating_add(1);
        if budget.consecutive_failures == 1 {
            message
        } else {
            "Java 语义增强连续不可用，详细原因已在上一条语义结果说明；本问不要重复调用，继续基础索引与源码路径。".to_string()
        }
    }

    fn semantic_rpc_available(
        &self,
        semantic: &SemanticToolSession,
        needed: usize,
        tool_limit: usize,
        tool_used: usize,
    ) -> bool {
        needed <= tool_limit.saturating_sub(tool_used)
            && needed <= self.semantic_rpc_remaining(semantic)
    }

    fn note_semantic_rpc(&self, semantic: &SemanticToolSession, count: usize) {
        let mut budget = semantic.budget.lock().expect("语义预算锁被污染");
        budget.rpc_calls = budget
            .rpc_calls
            .saturating_add(count)
            .min(JAVA_SEMANTIC_MAX_RPC_PER_QUESTION);
    }

    fn semantic_rpc_remaining(&self, semantic: &SemanticToolSession) -> usize {
        JAVA_SEMANTIC_MAX_RPC_PER_QUESTION
            .saturating_sub(semantic.budget.lock().expect("语义预算锁被污染").rpc_calls)
    }

    fn resolve_candidate(&self, candidate_id: &str) -> UnderstandingResult<RegisteredCandidate> {
        let registry = self.candidates.lock().expect("候选注册表锁被污染");
        let exact = registry.get(candidate_id).cloned();
        let resolved = exact.or_else(|| {
            // 模型可能抄错短 ID 末位：唯一前缀匹配兜底；多个候选取不到唯一
            // 前缀时仍拒绝，避免把符号错配到同类其它定义。
            let prefix = candidate_id.trim_start_matches("sc1:");
            if prefix.is_empty() {
                return None;
            }
            let matches: Vec<&RegisteredCandidate> = registry
                .keys()
                .filter(|key| key.starts_with("sc1:") && key[4..].starts_with(prefix))
                .filter_map(|key| registry.get(key))
                .collect();
            match matches.as_slice() {
                [only] => Some((*only).clone()),
                _ => None,
            }
        });
        resolved.ok_or_else(|| {
            UnderstandingError::new(
                ErrorCode::AmbiguousSymbol,
                "候选 ID 不是本次会话搜索结果，请先调用 search_symbols",
            )
        })
    }

    pub fn validate_source(
        &self,
        source_id: &str,
    ) -> UnderstandingResult<crate::code_index::SourceRef> {
        self.ensure_generation()?;
        self.source.validate_source(source_id)
    }

    fn ensure_generation(&self) -> UnderstandingResult<()> {
        let store = open_read_only_if_exists(&self.index_db_path)
            .map_err(index_error)?
            .ok_or_else(|| UnderstandingError::new(ErrorCode::IndexMissing, "代码索引已被移除"))?;
        let current = store.generation().map_err(index_error)?;
        if current != self.generation {
            return Err(UnderstandingError::new(
                ErrorCode::GenerationMismatch,
                format!(
                    "索引代际已从 {} 变为 {current}，本次工具会话需要重新开始",
                    self.generation
                ),
            ));
        }
        Ok(())
    }
}

pub fn tool_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "search_symbols",
            description: "按名称搜索当前项目的类、方法和其他符号。索引只作导航；没有命中时继续使用 search_code。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"符号关键词，中文问题通常需要换用代码中的英文词"},
                    "path_prefix":{"type":"string","description":"可选项目相对目录前缀"},
                    "language":{"type":"string","description":"可选语言名，如 java、rust"},
                    "limit":{"type":"integer","description":"默认 20，最大 50"}
                },
                "required":["query"]
            }),
        },
        ToolSchema {
            name: "get_symbol",
            description: "读取 search_symbols 返回的精确候选详情及定义源码。candidate_id 必须来自本次会话。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{"candidate_id":{"type":"string"}},
                "required":["candidate_id"]
            }),
        },
        ToolSchema {
            name: "trace_calls",
            description: "查询候选符号在当前索引中的调用方或被调用方。结果是检索线索，无边不等于无调用。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{
                    "candidate_id":{"type":"string"},
                    "direction":{"type":"string","enum":["inbound","outbound","both"]},
                    "depth":{"type":"integer","description":"默认 1，最大 3"},
                    "limit":{"type":"integer","description":"默认 20，最大 40"}
                },
                "required":["candidate_id","direction"]
            }),
        },
        ToolSchema {
            name: "search_code",
            description: "在允许的源码、配置、Mapper XML 和 SQL 文本中有界搜索。命中仅用于定位，关键结论需再调用 read_file。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string"},
                    "regex":{"type":"boolean"},
                    "path_prefix":{"type":"string"},
                    "suffix":{"type":"string","description":"可选文件后缀，如 .xml"},
                    "limit":{"type":"integer","description":"默认 20，最大 50"}
                },
                "required":["query"]
            }),
        },
        ToolSchema {
            name: "read_file",
            description: "读取允许范围内文件的指定行并发放可校验来源 ID。最多 200 行、8K 字符。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string","description":"项目内正斜杠相对路径"},
                    "start_line":{"type":"integer","description":"默认 1"},
                    "end_line":{"type":"integer","description":"默认起始行加 199"}
                },
                "required":["path"]
            }),
        },
        ToolSchema {
            name: "get_file_tree",
            description: "列出允许范围内的有界目录树，帮助定位 resources、mapper、model、security 等目录。",
            parameters: serde_json::json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string","description":"可选项目相对目录，默认根目录"},
                    "depth":{"type":"integer","description":"默认 2，最大 4"},
                    "limit":{"type":"integer","description":"默认/最大 200"}
                }
            }),
        },
    ]
}

pub fn tool_schemas_with_java_semantics() -> Vec<ToolSchema> {
    let mut schemas = tool_schemas();
    schemas.push(ToolSchema {
        name: "query_java_semantics",
        description: "对本问已登记的 Java 候选或 sr1 源码位置执行一次有界语义查询。结果是静态导航线索；不会安装、启动、等待或重配服务。",
        parameters: serde_json::json!({
            "type":"object",
            "properties":{
                "operation":{
                    "type":"string",
                    "enum":["definition","implementations","references","incoming_calls","outgoing_calls"]
                },
                "anchor":{
                    "oneOf":[
                        {
                            "type":"object",
                            "properties":{
                                "kind":{"const":"candidate"},
                                "candidate_id":{"type":"string","description":"来自本问 search_symbols 的 sc1 候选"}
                            },
                            "required":["kind","candidate_id"],
                            "additionalProperties":false
                        },
                        {
                            "type":"object",
                            "properties":{
                                "kind":{"const":"source"},
                                "source_id":{"type":"string","description":"来自本问 read_file/get_symbol 的 sr1 来源"},
                                "line":{"type":"integer","minimum":1,"description":"一基源码行"},
                                "column":{"type":"integer","minimum":1,"description":"一基 Unicode 标量列"}
                            },
                            "required":["kind","source_id","line","column"],
                            "additionalProperties":false
                        }
                    ]
                },
                "limit":{"type":"integer","minimum":1,"maximum":40,"description":"默认20，最大40"}
            },
            "required":["operation","anchor"],
            "additionalProperties":false
        }),
    });
    schemas
}

fn envelope<T>(
    request_id: &str,
    generation: u64,
    data: T,
    truncation_reasons: Vec<String>,
) -> ToolEnvelope<T> {
    ToolEnvelope {
        request_id: request_id.to_string(),
        index_generation: generation,
        truncated: !truncation_reasons.is_empty(),
        truncation_reasons,
        data,
    }
}

fn validate_request_id(request_id: &str) -> UnderstandingResult<()> {
    if request_id.trim().is_empty() {
        return Err(UnderstandingError::new(
            ErrorCode::AnswerInvalid,
            "工具 request_id 不能为空",
        ));
    }
    Ok(())
}

fn relation_view(hop: crate::code_index::TraceHop) -> SymbolRelationView {
    SymbolRelationView {
        name: hop.name,
        qualified_name: hop.qualified_name,
        relative_path: hop.file_path,
        hop: hop.hop,
    }
}

/// 索引标签是否为可调用符号（`trace_calls` 的解析范围）。
///
/// 与 `code_index::queries::resolve_in_graph(callable_only=true)` 的过滤条件
/// 对齐：只有 Function/Method 能被调用追踪解析。
pub(crate) fn is_callable_label(label: &str) -> bool {
    matches!(label, "Function" | "Method")
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn path_matches_language(path: &str, language: &str) -> bool {
    let language = language.to_ascii_lowercase();
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match language.as_str() {
        "rust" => extension == "rs",
        "python" => extension == "py",
        "javascript" => extension == "js",
        "typescript" => matches!(extension.as_str(), "ts" | "tsx"),
        "c++" | "cpp" => matches!(extension.as_str(), "cc" | "cpp" | "cxx" | "hpp"),
        "c#" | "csharp" => extension == "cs",
        "kotlin" => matches!(extension.as_str(), "kt" | "kts"),
        other => extension == other,
    }
}

fn index_error(error: crate::types::GitError) -> UnderstandingError {
    let message = error.to_string();
    let code = if message.contains("NeedsRebuild") {
        ErrorCode::NeedsRebuild
    } else if message.contains("不存在") || message.contains("尚未建立") {
        ErrorCode::IndexMissing
    } else {
        ErrorCode::IndexBusy
    };
    UnderstandingError::new(code, message)
}

fn semantic_input_error(error: SemanticServiceError) -> UnderstandingError {
    let code = match error {
        SemanticServiceError::SourceChanged(_) => ErrorCode::SourceChanged,
        SemanticServiceError::InvalidAnchor
        | SemanticServiceError::ProjectMismatch
        | SemanticServiceError::InvalidPosition(_)
        | SemanticServiceError::InvalidLimit { .. }
        | SemanticServiceError::UnsafeSource(_)
        | SemanticServiceError::SourceValidation(_) => ErrorCode::AnswerInvalid,
        _ => ErrorCode::ProviderUnavailable,
    };
    UnderstandingError::new(code, error.to_string())
}

fn semantic_service_message(message: &str) -> String {
    if message.contains("源码已变化") {
        format!("[SourceChanged] {message}；请重新读取源码后建立新锚点")
    } else if message.contains("不受支持") {
        format!("{message}；本次有效能力不包含该操作，请继续基础路径")
    } else {
        format!("Java 语义查询不可用：{message}；本问继续基础路径")
    }
}
