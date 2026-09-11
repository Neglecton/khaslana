//! V2 代码理解 agent 的六个只读工具。
//!
//! 索引结果只用于导航；所有源码正文都通过 [`SourceService`] 重新校验并发放
//! 来源 ID。候选 ID 属于本会话，模型不能用任意路径/行号伪造一个符号候选。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ai::ToolSchema;
use crate::ai::review_store::repo_key;
use crate::code_index::{
    DetailOutcome, ProjectContext, TraceDirection, TraceOutcome, open_read_only_if_exists,
    search_symbols_filtered, symbol_detail, trace_calls,
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
    fn register(&mut self, qualified_name: &str, label: &str, file_path: &str) -> String {
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
}

/// 单个问题/同一索引代际内使用的工具会话。
pub struct UnderstandingTools {
    context: ProjectContext,
    index_db_path: PathBuf,
    generation: u64,
    indexed_hashes: HashMap<String, String>,
    source: SourceService,
    candidates: Mutex<CandidateRegistry>,
}

impl UnderstandingTools {
    pub fn open(repo_root: &Path, index_db_path: &Path) -> UnderstandingResult<Self> {
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
        Ok(Self {
            context,
            index_db_path: index_db_path.to_path_buf(),
            generation,
            indexed_hashes,
            source,
            candidates: Mutex::new(CandidateRegistry::default()),
        })
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
                .register(&hit.qualified_name, &hit.label, &hit.file_path);
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
        Ok(envelope(
            request_id,
            self.generation,
            TraceCallsResult {
                function: result.function,
                direction: args.direction,
                callers: result.callers.into_iter().map(relation_view).collect(),
                callees: result.callees.into_iter().map(relation_view).collect(),
                coverage_note: "这是当前索引中的调用线索；无边不表示不存在调用，关键关系请继续读取调用位置或搜索源码。".to_string(),
            },
            reasons,
        ))
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
