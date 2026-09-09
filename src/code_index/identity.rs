// 代码理解的身份与证据契约层（DEV-01，设计文档 §4）。
//
// 本模块定义跨代际稳定的符号键（SymbolKey）、源码证据引用（SourceRef）、
// 文件内容指纹与索引快照元数据。设计约束（design.md §4.2）：
//
// - 对外不暴露数据库整数主键；NodeId 随整库重写重排，QN 的 `#N` 重载消歧
//   也不适合作为长期引用身份，统一走内容派生的稳定键。
// - SymbolKey 是 `sk1:<sha256>` 形式的带版本字符串：`sk1` 是键格式版本前缀，
//   摘要覆盖「键材料本身」，换键规则（材料结构变化）时递增前缀即可让旧键
//   全部失效（NeedsRebuild 语义），不需要迁移。
// - 同一符号键材料反复构造必须幂等（构造一次与构造 N 次得到同一键），因此
//   构造器统一走哈希，禁止调用方手工拼串。

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};

use crate::types::{GitError, Result};

/// SymbolKey 的键格式版本前缀。键材料结构变化时递增（sk1 → sk2），
/// 旧前缀键在打开时被拒绝（视为过期，NeedsRebuild）。
pub const SYMBOL_KEY_VERSION: &str = "sk1";

/// 稳定符号键：`sk1:<64 位小写 hex sha256>`。
///
/// 由 [`symbol_key_material`] 生成的规范材料哈希而来；不暴露整数主键，
/// 跨代际、跨进程稳定。Clone/Hash/Ord 皆可用（HashMap 下标仅限单次图遍历）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct SymbolKey(String);

impl SymbolKey {
    /// 从已校验的键字符串构造（仅接受本模块 [`parse_symbol_key`] 的产物；
    /// 外部输入一律走 parse，避免伪造前缀）。
    pub(crate) fn from_validated(value: String) -> Self {
        debug_assert!(value.starts_with(SYMBOL_KEY_VERSION));
        Self(value)
    }

    /// 键字符串（`sk1:<hex>`）。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 摘要部分（hex，不含前缀与冒号）。
    pub fn digest(&self) -> &str {
        &self.0[SYMBOL_KEY_VERSION.len() + 1..]
    }
}

impl fmt::Display for SymbolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SymbolKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_symbol_key(&raw).map_err(serde::de::Error::custom)
    }
}

/// 键材料片段：每段使用 UTF-8 字节长度前缀编码，空串仍占一个字段。
/// 内部使用；调用方经 [`symbol_key_material`] 组装，字段内容本身不会改变边界。
#[derive(Debug, Default)]
pub struct KeyMaterial {
    encoded: String,
}

impl KeyMaterial {
    fn push(&mut self, part: &str) {
        use std::fmt::Write as _;

        let _ = write!(self.encoded, "{}:", part.len());
        self.encoded.push_str(part);
    }

    fn seal(self) -> String {
        self.encoded
    }
}

/// 符号身份的规范字段。Java/Kotlin 使用模块、源集、包和类型链；普通语言
/// 必须填写 `relative_path` 与容器链，避免不同文件中的同名符号碰撞。
#[derive(Clone, Copy, Debug)]
pub struct SymbolKeyMaterial<'a> {
    pub language: &'a str,
    pub project_key: &'a str,
    pub module: &'a str,
    pub source_set: &'a str,
    pub package: &'a str,
    pub relative_path: &'a str,
    pub type_chain: &'a str,
    pub kind: &'a str,
    pub name: &'a str,
    pub signature: &'a str,
}

/// 组装符号键材料。字段顺序即身份语义，调用方不得自行增删：
///
/// `language | project_key | module | source_set | package | relative_path |
/// type_chain | kind | name | signature`
///
/// - 普通语言：module/source_set/package 可留空，但 relative_path 必填。
/// - 构造器 kind 用 `<init>`，不写返回类型。
/// - 不可解析参数类型保留规范化原文并带 `?` 前缀（不完整标记）。
pub fn symbol_key_material(parts: &SymbolKeyMaterial<'_>) -> Result<String> {
    if parts.language.trim().is_empty()
        || parts.project_key.trim().is_empty()
        || parts.kind.trim().is_empty()
        || parts.name.trim().is_empty()
    {
        return Err(GitError::Message(
            "符号键材料缺少语言、项目、种类或名称".to_string(),
        ));
    }
    let is_java_family = matches!(parts.language, "java" | "kotlin");
    if is_java_family && !parts.relative_path.is_empty() {
        return Err(GitError::Message(
            "Java/Kotlin 符号键不得使用文件路径区分身份".to_string(),
        ));
    }
    if !is_java_family && !is_normalized_relative_path(parts.relative_path) {
        return Err(GitError::Message(
            "普通语言符号键必须包含规范的项目内相对路径".to_string(),
        ));
    }
    let mut m = KeyMaterial::default();
    m.push(parts.language);
    m.push(parts.project_key);
    m.push(parts.module);
    m.push(parts.source_set);
    m.push(parts.package);
    m.push(parts.relative_path);
    m.push(parts.type_chain);
    m.push(parts.kind);
    m.push(parts.name);
    m.push(parts.signature);
    Ok(m.seal())
}

/// 由键材料生成稳定键（幂等：同一材料反复调用得到同一键）。
pub fn symbol_key_from_material(material: &str) -> SymbolKey {
    SymbolKey::from_validated(format!(
        "{SYMBOL_KEY_VERSION}:{}",
        sha256_hex(material.as_bytes())
    ))
}

/// 解析并验证外部输入的 SymbolKey。前缀不符或摘要非 64 位 hex 视为无效。
pub fn parse_symbol_key(raw: &str) -> Result<SymbolKey> {
    let digest = raw
        .strip_prefix(SYMBOL_KEY_VERSION)
        .and_then(|rest| rest.strip_prefix(':'))
        .ok_or_else(|| {
            GitError::Message(format!(
                "符号键格式无效（应为 {SYMBOL_KEY_VERSION}:<sha256>）：{raw}"
            ))
        })?;
    if !is_lower_hex_digest(digest) {
        return Err(GitError::Message(format!("符号键摘要无效：{raw}")));
    }
    Ok(SymbolKey::from_validated(raw.to_string()))
}

/// 字节序列的 SHA-256 小写 hex。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 内容指纹：解析字节的 SHA-256（小写 hex）。证据一致性的唯一依据；
/// mtime+size 只作快速筛选（store.rs FileHashRow 现状），不参与校验。
pub fn content_fingerprint(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// 源码证据引用（设计文档 §4.3）。指向某一代索引中某文件的字节/行区间。
///
/// - `start_byte/end_byte` 是原始文件字节的半开区间 `[start, end)`；
/// - 行号从 1 开始，`end_line` 为闭区间语义的最后一行；
/// - 范围不得跨文件（单条 SourceRef 只有一个路径）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceRef {
    pub project_key: String,
    /// 发布代际：引用绑定哪一次索引发布。
    pub generation: u64,
    /// 项目根内相对路径，`/` 分隔。
    pub relative_path: String,
    /// 引用时刻的内容指纹；读取时重新计算比对。
    pub content_sha256: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
}

/// SourceRef 构造材料。使用命名字段避免字节/行范围参数错位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRefParts {
    pub project_key: String,
    pub generation: u64,
    pub relative_path: String,
    pub content_sha256: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
}

impl SourceRef {
    /// 构造并校验区间不变量（半开字节区间、行号 ≥ 1、行区间闭合）。
    pub fn new(parts: SourceRefParts) -> Result<Self> {
        let reference = Self {
            project_key: parts.project_key,
            generation: parts.generation,
            relative_path: parts.relative_path,
            content_sha256: parts.content_sha256,
            start_byte: parts.start_byte,
            end_byte: parts.end_byte,
            start_line: parts.start_line,
            end_line: parts.end_line,
        };
        reference.validate()?;
        Ok(reference)
    }

    /// 区间不变量校验；任何违反即拒绝构造/反序列化（防伪造引用）。
    pub fn validate(&self) -> Result<()> {
        if self.end_byte < self.start_byte {
            return Err(GitError::Message(format!(
                "SourceRef 字节区间无效（end < start）：{} [{}..{})",
                self.relative_path, self.start_byte, self.end_byte
            )));
        }
        if self.start_line == 0 || self.end_line < self.start_line {
            return Err(GitError::Message(format!(
                "SourceRef 行区间无效（行号从 1 起）：{} {}..{}",
                self.relative_path, self.start_line, self.end_line
            )));
        }
        if !is_lower_hex_digest(&self.content_sha256) {
            return Err(GitError::Message(format!(
                "SourceRef 内容指纹无效：{}",
                self.relative_path
            )));
        }
        if self.project_key.trim().is_empty() || !is_normalized_relative_path(&self.relative_path) {
            return Err(GitError::Message("SourceRef 缺少项目键或路径".to_string()));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for SourceRef {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireSourceRef {
            project_key: String,
            generation: u64,
            relative_path: String,
            content_sha256: String,
            start_byte: u64,
            end_byte: u64,
            start_line: u32,
            end_line: u32,
        }

        let raw = WireSourceRef::deserialize(deserializer)?;
        SourceRef::new(SourceRefParts {
            project_key: raw.project_key,
            generation: raw.generation,
            relative_path: raw.relative_path,
            content_sha256: raw.content_sha256,
            start_byte: raw.start_byte,
            end_byte: raw.end_byte,
            start_line: raw.start_line,
            end_line: raw.end_line,
        })
        .map_err(serde::de::Error::custom)
    }
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_normalized_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':', '\0'])
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// 索引快照元数据：每次查询/回答必须绑定的版本身份（设计文档 §4.1）。
///
/// `generation` 是成功发布后递增的 u64，必须与数据写入同事务提交；
/// 时间戳不能替代代际。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexSnapshot {
    pub schema_version: u32,
    pub generation: u64,
    /// 各适配器（通用提取/Java/框架规则）的版本号表。
    pub adapter_versions: Vec<(String, String)>,
    /// 本代全部文件指纹的聚合摘要（防清单漂移）。
    pub manifest_digest: String,
    /// Unix 毫秒。
    pub indexed_at: u64,
    pub coverage: CoverageSummary,
}

impl IndexSnapshot {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version == 0 || self.generation == 0 {
            return Err(GitError::Message(
                "索引快照缺少 schema 版本或发布代际".to_string(),
            ));
        }
        if !is_lower_hex_digest(&self.manifest_digest) {
            return Err(GitError::Message("索引清单摘要无效".to_string()));
        }
        let mut adapters = std::collections::HashSet::new();
        for (adapter, version) in &self.adapter_versions {
            if adapter.trim().is_empty()
                || version.trim().is_empty()
                || !adapters.insert(adapter.as_str())
            {
                return Err(GitError::Message(
                    "索引适配器版本为空或存在重复项".to_string(),
                ));
            }
        }
        self.coverage.validate()
    }
}

impl<'de> Deserialize<'de> for IndexSnapshot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireIndexSnapshot {
            schema_version: u32,
            generation: u64,
            adapter_versions: Vec<(String, String)>,
            manifest_digest: String,
            indexed_at: u64,
            coverage: CoverageSummary,
        }

        let raw = WireIndexSnapshot::deserialize(deserializer)?;
        let snapshot = Self {
            schema_version: raw.schema_version,
            generation: raw.generation,
            adapter_versions: raw.adapter_versions,
            manifest_digest: raw.manifest_digest,
            indexed_at: raw.indexed_at,
            coverage: raw.coverage,
        };
        snapshot.validate().map_err(serde::de::Error::custom)?;
        Ok(snapshot)
    }
}

/// 覆盖摘要（设计文档 §5.3）：计数必须覆盖被预算剪枝的情况。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CoverageSummary {
    pub files_discovered: u64,
    pub files_parsed: u64,
    pub files_partial: u64,
    pub files_skipped: u64,
    pub files_unreadable: u64,
    pub calls_total: u64,
    pub calls_resolved: u64,
    pub calls_ambiguous: u64,
    pub calls_unresolved: u64,
    pub calls_external: u64,
}

impl<'de> Deserialize<'de> for CoverageSummary {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireCoverageSummary {
            files_discovered: u64,
            files_parsed: u64,
            files_partial: u64,
            files_skipped: u64,
            files_unreadable: u64,
            calls_total: u64,
            calls_resolved: u64,
            calls_ambiguous: u64,
            calls_unresolved: u64,
            calls_external: u64,
        }

        let raw = WireCoverageSummary::deserialize(deserializer)?;
        let coverage = Self {
            files_discovered: raw.files_discovered,
            files_parsed: raw.files_parsed,
            files_partial: raw.files_partial,
            files_skipped: raw.files_skipped,
            files_unreadable: raw.files_unreadable,
            calls_total: raw.calls_total,
            calls_resolved: raw.calls_resolved,
            calls_ambiguous: raw.calls_ambiguous,
            calls_unresolved: raw.calls_unresolved,
            calls_external: raw.calls_external,
        };
        coverage.validate().map_err(serde::de::Error::custom)?;
        Ok(coverage)
    }
}

impl CoverageSummary {
    pub fn total_calls(&self) -> u64 {
        self.calls_total
    }

    pub fn validate(&self) -> Result<()> {
        let classified_files =
            self.files_parsed + self.files_partial + self.files_skipped + self.files_unreadable;
        if classified_files != self.files_discovered {
            return Err(GitError::Message(format!(
                "覆盖文件计数不一致：发现 {}，分类合计 {classified_files}",
                self.files_discovered
            )));
        }
        let classified_calls = self.calls_resolved
            + self.calls_ambiguous
            + self.calls_unresolved
            + self.calls_external;
        if classified_calls != self.calls_total {
            return Err(GitError::Message(format!(
                "覆盖调用计数不一致：总数 {}，分类合计 {classified_calls}",
                self.calls_total
            )));
        }
        Ok(())
    }
}

/// 项目上下文：从宿主仓库标签取得路径后立即脱离 Git 语义。
/// 8 位目录哈希仅作磁盘定位；打开库必须核对完整根路径防碰撞误用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectContext {
    /// 规范化项目键（与 repo_key 同源：小写折叠 + trim 尾分隔符）。
    pub project_key: String,
    /// 项目根目录绝对路径（canonical 形态）。
    pub canonical_root: String,
    /// 索引库文件路径。
    pub index_db_path: String,
}

#[cfg(test)]
#[path = "../tests/code_index_identity.rs"]
mod tests;
