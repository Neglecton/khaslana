// 代码理解核心类型契约（DEV-01）。
//
// 依据 design.md §4.4（关系模型）、§4.3（证据）、§7（GraphSlice）、
// §8.2（AnswerDocument）、§12（错误码）。关键设计约束：
//
// - EntityKey 是带类型前缀的联合体（symbol/callsite/entry/boundary），
//   不是任意字符串；边界节点显式建模，未解析调用不虚构真实符号。
// - certainty（syntactic/inferred/unknown）与 resolution_state
//   （resolved/ambiguous/unresolved/external）是两个正交维度，
//   不得互相替代（§4.4：确定存在接口调用 ≠ 确定运行时实现）。
// - AI 只引用 evidence_id；AnswerDocument 校验失败不是成功。
// - 相邻步骤不保证存在调用边，不能以步骤顺序自动连线。

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::code_index::identity::SymbolKey;
use crate::types::{GitError, Result};

/// AnswerDocument 协议版本。协议字段结构变化时递增。
pub const ANSWER_PROTOCOL_VERSION: u32 = 1;

pub fn protocol_version() -> u32 {
    ANSWER_PROTOCOL_VERSION
}

/// 统一错误码（design.md §12）。错误包含可展示中文说明与是否可重试，
/// 不泄露密钥/代理 URL。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IndexMissing,
    IndexBusy,
    NeedsRebuild,
    GenerationMismatch,
    CursorExpired,
    SourceChanged,
    SourceMissing,
    OutsideProject,
    ExcludedPath,
    EncodingUnsupported,
    BudgetExceeded,
    AmbiguousSymbol,
    ProviderUnavailable,
    ProviderToolUnsupported,
    AnswerInvalid,
    Cancelled,
}

impl ErrorCode {
    /// 默认可重试性；调用方可按场景覆盖（如 IndexBusy 实际等待后重试）。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::IndexBusy
                | Self::GenerationMismatch
                | Self::CursorExpired
                | Self::SourceChanged
                | Self::BudgetExceeded
                | Self::Cancelled
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::IndexMissing => "索引尚未建立",
            Self::IndexBusy => "索引任务正在进行",
            Self::NeedsRebuild => "索引需要重建",
            Self::GenerationMismatch => "索引已更新，结果基于较早版本",
            Self::CursorExpired => "分页游标已过期",
            Self::SourceChanged => "源码已变化",
            Self::SourceMissing => "源码文件不存在",
            Self::OutsideProject => "路径超出项目范围",
            Self::ExcludedPath => "路径被排除规则拒绝",
            Self::EncodingUnsupported => "编码不受支持",
            Self::BudgetExceeded => "预算已用尽",
            Self::AmbiguousSymbol => "符号存在多个候选",
            Self::ProviderUnavailable => "AI 供应商不可用",
            Self::ProviderToolUnsupported => "供应商不支持工具调用",
            Self::AnswerInvalid => "回答无法通过校验",
            Self::Cancelled => "请求已取消",
        };
        f.write_str(text)
    }
}

/// 理解服务统一错误：错误码 + 中文说明 + 可重试性。
/// 业务层错误出口（GitError 之上的领域封装）。
#[derive(Clone, Debug)]
pub struct UnderstandingError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable_override: Option<bool>,
}

impl UnderstandingError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable_override: None,
        }
    }

    pub fn retryable(&self) -> bool {
        self.retryable_override
            .unwrap_or_else(|| self.code.retryable())
    }
}

impl fmt::Display for UnderstandingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for UnderstandingError {}

impl From<UnderstandingError> for GitError {
    fn from(value: UnderstandingError) -> Self {
        GitError::Message(format!("{value}"))
    }
}

/// 实体键联合体（design.md §4.5）：带类型前缀，不是任意字符串。
///
/// - `symbol` 携带稳定符号键（identity::SymbolKey）；
/// - `callsite` / `entry` / `boundary` 使用各自命名空间的带版本 id
///   （由服务发放，格式 `cs1:`/`ep1:`/`bn1:` + hex 摘要）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum EntityKey {
    Symbol(SymbolKey),
    CallSite(String),
    Entry(String),
    Boundary(String),
}

impl EntityKey {
    /// 带版本前缀校验地构造调用点键（服务发放，`cs1:<hex>`）。
    pub fn callsite(raw: &str) -> Result<Self> {
        validate_namespaced_id(raw, "cs1").map(Self::CallSite)
    }

    /// 带版本前缀校验地构造入口键（`ep1:<hex>`）。
    pub fn entry(raw: &str) -> Result<Self> {
        validate_namespaced_id(raw, "ep1").map(Self::Entry)
    }

    /// 带版本前缀校验地构造边界节点键（`bn1:<hex>`）。
    pub fn boundary(raw: &str) -> Result<Self> {
        validate_namespaced_id(raw, "bn1").map(Self::Boundary)
    }

    /// 调用点稳定键：owner + 路径 + 半开字节范围 + 同位置序号。
    pub fn callsite_from_parts(
        owner: &SymbolKey,
        relative_path: &str,
        start_byte: u64,
        end_byte: u64,
        ordinal: u32,
    ) -> Self {
        Self::CallSite(derive_namespaced_id(
            "cs1",
            &[
                owner.as_str(),
                relative_path,
                &start_byte.to_string(),
                &end_byte.to_string(),
                &ordinal.to_string(),
            ],
        ))
    }

    /// 入口稳定键：入口种类 + 模块 + 声明实体 + 入口判别串（路由/statement 等）。
    pub fn entry_from_parts(
        entry_kind: &str,
        module_key: &str,
        declaring_entity: &EntityKey,
        discriminator: &str,
    ) -> Self {
        Self::Entry(derive_namespaced_id(
            "ep1",
            &[entry_kind, module_key, declaring_entity.id(), discriminator],
        ))
    }

    /// 查询期边界键：只在指定代际和来源关系内稳定，不持久化为真实源码实体。
    pub fn boundary_from_parts(
        project_key: &str,
        generation: u64,
        source_relation_id: &str,
        reason: &str,
        ordinal: u32,
    ) -> Self {
        Self::Boundary(derive_namespaced_id(
            "bn1",
            &[
                project_key,
                &generation.to_string(),
                source_relation_id,
                reason,
                &ordinal.to_string(),
            ],
        ))
    }

    /// 反序列化入口：按变体做前缀/格式校验，拒绝伪造 ID。
    pub fn from_wire(value: &str) -> Result<Self> {
        if let Ok(key) = crate::code_index::identity::parse_symbol_key(value) {
            return Ok(Self::Symbol(key));
        }
        Self::callsite(value)
            .or_else(|_| Self::entry(value))
            .or_else(|_| Self::boundary(value))
    }

    /// id 部分（symbol 变体为 `sk1:<hex>` 原文）。
    pub fn id(&self) -> &str {
        match self {
            Self::Symbol(key) => key.as_str(),
            Self::CallSite(id) | Self::Entry(id) | Self::Boundary(id) => id,
        }
    }
}

impl<'de> Deserialize<'de> for EntityKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", content = "id", rename_all = "snake_case")]
        enum WireEntityKey {
            Symbol(SymbolKey),
            CallSite(String),
            Entry(String),
            Boundary(String),
        }

        let wire = WireEntityKey::deserialize(deserializer)?;
        match wire {
            WireEntityKey::Symbol(key) => Ok(Self::Symbol(key)),
            WireEntityKey::CallSite(raw) => Self::callsite(&raw),
            WireEntityKey::Entry(raw) => Self::entry(&raw),
            WireEntityKey::Boundary(raw) => Self::boundary(&raw),
        }
        .map_err(serde::de::Error::custom)
    }
}

fn validate_namespaced_id(raw: &str, prefix: &str) -> Result<String> {
    let digest = raw
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(':'))
        .ok_or_else(|| {
            GitError::Message(format!("实体键格式无效（应为 {prefix}:<hex>）：{raw}"))
        })?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(GitError::Message(format!("实体键摘要无效：{raw}")));
    }
    Ok(raw.to_string())
}

fn derive_namespaced_id(prefix: &str, parts: &[&str]) -> String {
    let material = encode_identity_parts(parts);
    format!(
        "{prefix}:{}",
        crate::code_index::identity::sha256_hex(material.as_bytes())
    )
}

fn encode_identity_parts(parts: &[&str]) -> String {
    use std::fmt::Write as _;

    let mut material = String::new();
    for part in parts {
        let _ = write!(material, "{}:", part.len());
        material.push_str(part);
    }
    material
}

/// 关系种类（design.md §4.4 表）。语义边界写进文档与工具描述，
/// 类型只保证种类正确，不保证语义不被误读。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// 模块/类型 → 符号：结构包含，不是调用。
    Contains,
    /// 调用者 → 声明方法/构造器（实例调用仍可能动态分派）。
    Calls,
    Inherits,
    Implements,
    Overrides,
    /// 调用点 → 候选实现方法（不替代 CALLS 声明边）。
    DispatchCandidate,
    /// 注入点 → Bean 候选（框架规则推断，不是方法调用）。
    InjectsCandidate,
    /// 路由声明 → handler（不代表运行时路由已注册）。
    Handles,
    /// Mapper 方法 → XML/注解 SQL 声明（不代表 SQL 已执行）。
    MapsTo,
}

impl RelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Calls => "calls",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::Overrides => "overrides",
            Self::DispatchCandidate => "dispatch_candidate",
            Self::InjectsCandidate => "injects_candidate",
            Self::Handles => "handles",
            Self::MapsTo => "maps_to",
        }
    }
}

/// 关系稳定 ID：端点、关系种类、规则、候选组和条件共同决定身份；证据列表
/// 不参与身份，允许同一关系在重建后补充证据而不换键。
pub fn relation_id_from_parts(
    source_key: &EntityKey,
    target_key: Option<&EntityKey>,
    kind: RelationKind,
    rule_id: Option<&str>,
    candidate_group: Option<&str>,
    conditions: &[String],
) -> String {
    let mut parts = vec![
        source_key.id().to_string(),
        target_key.map(EntityKey::id).unwrap_or("").to_string(),
        kind.as_str().to_string(),
        rule_id.unwrap_or("").to_string(),
        candidate_group.unwrap_or("").to_string(),
    ];
    parts.extend(conditions.iter().cloned());
    let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
    derive_namespaced_id("r1", &borrowed)
}

/// 语义可信度（design.md §4.4）：`syntactic` 源码可证实 / `inferred`
/// 有限静态规则推断 / `unknown` 未解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    Syntactic,
    Inferred,
    Unknown,
}

/// 解析状态：resolved / ambiguous / unresolved / external。
/// 与 [`Certainty`] 正交（如「确定存在的接口调用」resolved + syntactic，
/// 但运行时实现仍是候选）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionState {
    Resolved,
    Ambiguous,
    Unresolved,
    External,
}

/// 关系记录（design.md §4.4）。持久层可用 `target_key=None` 表示未解析/外部目标；
/// 进入 GraphSlice 前必须投影为显式 `Boundary` 端点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationRecord {
    pub relation_id: String,
    pub source_key: EntityKey,
    pub target_key: Option<EntityKey>,
    pub kind: RelationKind,
    pub certainty: Certainty,
    pub resolution_state: ResolutionState,
    /// 产生该关系的规则 id（如 `spring.qualifier`）；syntactic 可为空。
    pub rule_id: Option<String>,
    pub evidence_ids: Vec<String>,
    /// 同一组候选共享的分组标识（dispatch/inject 候选集合）。
    pub candidate_group: Option<String>,
    /// 条件表达式原文（Profile/动态 SQL if 等），未知条件原样保留。
    pub conditions: Vec<String>,
    /// 旧启发式得分只作排序参考，不是正确率（兼容投影用）。
    pub heuristic_score: Option<f64>,
}

/// 证据种类（design.md §4.3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Declaration,
    CallSite,
    Annotation,
    SqlMapping,
    ControlFlow,
    ModuleAggregation,
}

impl EvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::CallSite => "call_site",
            Self::Annotation => "annotation",
            Self::SqlMapping => "sql_mapping",
            Self::ControlFlow => "control_flow",
            Self::ModuleAggregation => "module_aggregation",
        }
    }
}

/// 证据记录：服务创建，AI 只能引用 `evidence_id`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub evidence_id: String,
    pub kind: EvidenceKind,
    pub source_refs: Vec<crate::code_index::identity::SourceRef>,
    pub rule_id: Option<String>,
    /// 该证据支撑的关系 id（多对多经 relation_evidence 表连接）。
    pub relation_ids: Vec<String>,
    /// 证据内容摘要（防同一 ID 换内容）。
    pub excerpt_digest: String,
}

/// 证据稳定 ID：证据类型、规则、摘要和排序后的 SourceRef 身份共同决定。
pub fn evidence_id_from_parts(
    kind: EvidenceKind,
    source_refs: &[crate::code_index::identity::SourceRef],
    rule_id: Option<&str>,
    excerpt_digest: &str,
) -> String {
    let mut refs: Vec<String> = source_refs
        .iter()
        .map(|source_ref| {
            let generation = source_ref.generation.to_string();
            let start_byte = source_ref.start_byte.to_string();
            let end_byte = source_ref.end_byte.to_string();
            let start_line = source_ref.start_line.to_string();
            let end_line = source_ref.end_line.to_string();
            encode_identity_parts(&[
                &source_ref.project_key,
                &generation,
                &source_ref.relative_path,
                &source_ref.content_sha256,
                &start_byte,
                &end_byte,
                &start_line,
                &end_line,
            ])
        })
        .collect();
    refs.sort();
    let mut parts = vec![
        kind.as_str().to_string(),
        rule_id.unwrap_or("").to_string(),
        excerpt_digest.to_string(),
    ];
    parts.extend(refs);
    let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
    derive_namespaced_id("e1", &borrowed)
}

/// 一次回答可引用的全部证据（EvidenceBundle）：AnswerDocument 校验的
/// 封闭集合——所有 ID 必须来自本次 bundle，不跨代拼证据。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub records: Vec<EvidenceRecord>,
}

impl EvidenceBundle {
    pub fn contains(&self, evidence_id: &str) -> bool {
        self.records.iter().any(|r| r.evidence_id == evidence_id)
    }

    pub fn get(&self, evidence_id: &str) -> Option<&EvidenceRecord> {
        self.records.iter().find(|r| r.evidence_id == evidence_id)
    }
}

/// 图节点（GraphSlice 内）：实体 + 展示元数据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSliceNode {
    pub key: EntityKey,
    /// 展示名（短名）。
    pub name: String,
    /// 可读限定名 / 路径。
    pub qualified_name: String,
    pub kind_label: String,
    pub module: Option<String>,
}

/// 未解析/外部边界节点：图中明确呈现「未知」，不虚构真实符号。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryNode {
    pub key: EntityKey,
    pub name: String,
    /// 边界成因（未解析调用 / 外部类型 / 预算截断）。
    pub reason: String,
}

/// 关系方向过滤器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationDirection {
    Outbound,
    Inbound,
    Both,
}

/// 图切片（design.md §7）：有界查询输出。不变量（slice_invariants 校验）：
/// 全部关系端点必须出现在 nodes 或 boundary_nodes 中；BFS 前驱不能代替实际边。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphSlice {
    pub snapshot: crate::code_index::identity::IndexSnapshot,
    pub nodes: Vec<GraphSliceNode>,
    pub relations: Vec<RelationRecord>,
    pub boundary_nodes: Vec<BoundaryNode>,
    pub coverage: crate::code_index::identity::CoverageSummary,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
    /// 下一页游标（生成期随机或排序键摘要）；条件/代际变化即过期。
    pub next_cursor: Option<String>,
}

impl GraphSlice {
    /// 结构不变量校验（§7：所有关系端点有效，无伪全量结果）。
    pub fn slice_invariants(&self) -> Result<()> {
        self.snapshot.validate()?;
        self.coverage.validate()?;
        if self.coverage != self.snapshot.coverage {
            return Err(GitError::Message(
                "GraphSlice 覆盖摘要与索引快照不一致".to_string(),
            ));
        }
        let mut known: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for node in &self.nodes {
            if matches!(node.key, EntityKey::Boundary(_)) {
                return Err(GitError::Message(format!(
                    "边界实体不得放入普通节点集合：{}",
                    node.key.id()
                )));
            }
            if !known.insert(node.key.id()) {
                return Err(GitError::Message(format!(
                    "GraphSlice 节点键重复：{}",
                    node.key.id()
                )));
            }
        }
        for boundary in &self.boundary_nodes {
            if !matches!(boundary.key, EntityKey::Boundary(_)) {
                return Err(GitError::Message(format!(
                    "boundary_nodes 包含非边界实体：{}",
                    boundary.key.id()
                )));
            }
            if !known.insert(boundary.key.id()) {
                return Err(GitError::Message(format!(
                    "GraphSlice 节点键重复：{}",
                    boundary.key.id()
                )));
            }
        }
        let mut relation_ids = std::collections::HashSet::new();
        for relation in &self.relations {
            validate_namespaced_id(&relation.relation_id, "r1")?;
            if !relation_ids.insert(relation.relation_id.as_str()) {
                return Err(GitError::Message(format!(
                    "GraphSlice 关系 ID 重复：{}",
                    relation.relation_id
                )));
            }
            if !known.contains(relation.source_key.id()) {
                return Err(GitError::Message(format!(
                    "GraphSlice 关系 {} 的 source 不在节点集合中：{}",
                    relation.relation_id,
                    relation.source_key.id()
                )));
            }
            let target = relation.target_key.as_ref().ok_or_else(|| {
                GitError::Message(format!(
                    "GraphSlice 关系 {} 缺少 target；未知目标必须投影为边界节点",
                    relation.relation_id
                ))
            })?;
            if !known.contains(target.id()) {
                return Err(GitError::Message(format!(
                    "GraphSlice 关系 {} 的 target 不在节点集合中：{}",
                    relation.relation_id,
                    target.id()
                )));
            }
        }
        if !self.truncated && (!self.truncation_reasons.is_empty() || self.next_cursor.is_some()) {
            return Err(GitError::Message(
                "未截断的 GraphSlice 不得携带截断原因或下一页游标".to_string(),
            ));
        }
        Ok(())
    }
}

/// 检索过滤条件（design.md §7 search）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilter {
    pub module: Option<String>,
    pub path_prefix: Option<String>,
    /// source_set 过滤（如 main/test）；None 表示不过滤。
    pub source_set: Option<String>,
    /// 符号种类标签（Function/Method/Class/…）。
    pub kind_label: Option<String>,
}

/// claim 性质：observed（有直接证据）/ inferred（规则推断）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nature {
    Observed,
    Inferred,
}

/// 回答中的单条结论。`evidence_ids` 为空即无证据 claim，
/// 校验阶段拒绝其进入关键结论（§8.2）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerClaim {
    pub id: String,
    pub text: String,
    pub evidence_ids: Vec<String>,
    pub nature: Nature,
}

/// 回答步骤。相邻步骤不保证存在调用边；`relation_ids` 才是图关联依据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerStep {
    pub id: String,
    pub title: String,
    pub claim_ids: Vec<String>,
    pub entity_keys: Vec<EntityKey>,
    pub relation_ids: Vec<String>,
}

/// 不确定性类型（design.md §8.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    AmbiguousCandidates,
    SourceUnreadable,
    ParseGap,
    ExternalDependency,
    BudgetTruncated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uncertainty {
    pub kind: UncertaintyKind,
    pub message: String,
}

/// 完成状态：complete / partial。失败与取消是任务状态，
/// 不伪装成 AnswerDocument。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Complete,
    Partial,
}

/// 回答文档（design.md §8.2 最小字段集）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerDocument {
    pub protocol_version: u32,
    pub project_key: String,
    pub generation: u64,
    pub request_id: String,
    /// 检索范围摘要（模块/源集/过滤条件）。
    pub scope: String,
    pub summary: String,
    pub claims: Vec<AnswerClaim>,
    pub steps: Vec<AnswerStep>,
    pub focus_entity_keys: Vec<EntityKey>,
    pub evidence_ids: Vec<String>,
    pub uncertainties: Vec<Uncertainty>,
    pub coverage: crate::code_index::identity::CoverageSummary,
    pub completion_status: CompletionStatus,
}

/// 回答文档的结构校验（§8.2）：证据来自本次 bundle，实体与关系来自权威
/// GraphSlice，源码引用的项目/代际一致。格式错误最多一次修复请求；仍失败
/// 保留本地结果，不转成无引用成功回答。
pub fn validate_answer_document(
    document: &AnswerDocument,
    bundle: &EvidenceBundle,
    graph: &GraphSlice,
) -> Result<()> {
    graph.slice_invariants()?;
    if document.protocol_version != ANSWER_PROTOCOL_VERSION {
        return Err(GitError::Message(format!(
            "回答协议版本不符：期望 {ANSWER_PROTOCOL_VERSION}，实际 {}",
            document.protocol_version
        )));
    }
    if document.generation != graph.snapshot.generation {
        return Err(GitError::Message(format!(
            "回答代际不符：期望 {}，实际 {}",
            graph.snapshot.generation, document.generation
        )));
    }
    if document.project_key.trim().is_empty()
        || document.request_id.trim().is_empty()
        || document.scope.trim().is_empty()
        || document.summary.trim().is_empty()
    {
        return Err(GitError::Message("回答缺少项目键或请求 ID".to_string()));
    }
    document.coverage.validate()?;
    if document.coverage != graph.coverage {
        return Err(GitError::Message(
            "回答覆盖摘要与权威 GraphSlice 不一致".to_string(),
        ));
    }
    if document.completion_status == CompletionStatus::Complete
        && !document.uncertainties.is_empty()
    {
        return Err(GitError::Message(
            "complete 回答不得同时声明未解决的不确定性".to_string(),
        ));
    }

    let mut evidence_by_id = std::collections::HashMap::new();
    for record in &bundle.records {
        validate_namespaced_id(&record.evidence_id, "e1")?;
        if evidence_by_id
            .insert(record.evidence_id.as_str(), record)
            .is_some()
        {
            return Err(GitError::Message(format!(
                "证据 ID 重复：{}",
                record.evidence_id
            )));
        }
        if record.source_refs.is_empty() {
            return Err(GitError::Message(format!(
                "证据 {} 没有源码引用",
                record.evidence_id
            )));
        }
        if !is_lower_hex_64(&record.excerpt_digest) {
            return Err(GitError::Message(format!(
                "证据 {} 的内容摘要无效",
                record.evidence_id
            )));
        }
        for source_ref in &record.source_refs {
            source_ref.validate()?;
            if source_ref.project_key != document.project_key
                || source_ref.generation != document.generation
            {
                return Err(GitError::Message(format!(
                    "证据 {} 的项目或代际与回答不一致",
                    record.evidence_id
                )));
            }
        }
    }

    let relation_by_id: std::collections::HashMap<&str, &RelationRecord> = graph
        .relations
        .iter()
        .map(|relation| (relation.relation_id.as_str(), relation))
        .collect();
    for relation in &graph.relations {
        for evidence_id in &relation.evidence_ids {
            if !evidence_by_id.contains_key(evidence_id.as_str()) {
                return Err(GitError::Message(format!(
                    "关系 {} 引用了本次证据集之外的证据：{evidence_id}",
                    relation.relation_id
                )));
            }
        }
    }

    let known_entities: std::collections::HashSet<&str> = graph
        .nodes
        .iter()
        .map(|node| node.key.id())
        .chain(graph.boundary_nodes.iter().map(|node| node.key.id()))
        .collect();

    let mut claim_ids = std::collections::HashSet::new();
    let top_level_evidence: std::collections::HashSet<&str> =
        document.evidence_ids.iter().map(String::as_str).collect();
    if top_level_evidence.len() != document.evidence_ids.len() {
        return Err(GitError::Message(
            "回答的 evidence_ids 存在重复项".to_string(),
        ));
    }
    for evidence_id in &document.evidence_ids {
        if !evidence_by_id.contains_key(evidence_id.as_str()) {
            return Err(GitError::Message(format!(
                "回答引用了本次证据集之外的证据：{evidence_id}"
            )));
        }
    }
    for claim in &document.claims {
        if claim.id.trim().is_empty() || !claim_ids.insert(claim.id.as_str()) {
            return Err(GitError::Message(format!(
                "claim ID 为空或重复：{}",
                claim.id
            )));
        }
        if claim.text.trim().is_empty() || claim.evidence_ids.is_empty() {
            return Err(GitError::Message(format!(
                "claim {} 缺少结论文本或证据",
                claim.id
            )));
        }
        for evidence_id in &claim.evidence_ids {
            if !evidence_by_id.contains_key(evidence_id.as_str()) {
                return Err(GitError::Message(format!(
                    "claim {} 引用了本次证据集之外的 ID：{evidence_id}",
                    claim.id
                )));
            }
            if !top_level_evidence.contains(evidence_id.as_str()) {
                return Err(GitError::Message(format!(
                    "claim {} 的证据未列入回答 evidence_ids：{evidence_id}",
                    claim.id
                )));
            }
        }
    }

    let mut step_ids = std::collections::HashSet::new();
    for step in &document.steps {
        if step.id.trim().is_empty()
            || step.title.trim().is_empty()
            || !step_ids.insert(step.id.as_str())
        {
            return Err(GitError::Message(format!(
                "步骤 ID/标题为空或 ID 重复：{}",
                step.id
            )));
        }
        for claim_id in &step.claim_ids {
            if !claim_ids.contains(claim_id.as_str()) {
                return Err(GitError::Message(format!(
                    "步骤 {} 引用了不存在的 claim：{claim_id}",
                    step.id
                )));
            }
        }
        for entity_key in &step.entity_keys {
            if !known_entities.contains(entity_key.id()) {
                return Err(GitError::Message(format!(
                    "步骤 {} 引用了 GraphSlice 之外的实体：{}",
                    step.id,
                    entity_key.id()
                )));
            }
        }
        for relation_id in &step.relation_ids {
            if !relation_by_id.contains_key(relation_id.as_str()) {
                return Err(GitError::Message(format!(
                    "步骤 {} 引用了未知关系：{relation_id}",
                    step.id
                )));
            }
        }
    }
    for entity_key in &document.focus_entity_keys {
        if !known_entities.contains(entity_key.id()) {
            return Err(GitError::Message(format!(
                "回答焦点引用了 GraphSlice 之外的实体：{}",
                entity_key.id()
            )));
        }
    }
    Ok(())
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
