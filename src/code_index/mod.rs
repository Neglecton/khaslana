// 代码索引引擎。
//
// 架构参照开源项目 codebase-memory-mcp（tree-sitter AST → 内存图缓冲 → SQLite
// 知识图谱）：
//
// 1. [`discover`] 文件发现：gitignore 感知遍历 + 跳过目录/后缀黑名单；
// 2. [`extract`] tree-sitter 提取：每语言一张「节点类型字符串表 + 通用 walk」
//    （不使用 .scm query），产出单文件的 定义 / 导入 / 调用点 / 类型关系；
// 3. [`graph`] 内存图缓冲：全部节点/边驻留 RAM，qualified_name 全局去重，
//    索引期间不碰 SQLite，结束一次性落盘；
// 4. [`resolve`] 调用解析：符号注册表 + 多级策略链把调用点解析成 CALLS 边，
//    边属性记录 confidence/strategy（解析是概率性的，与参考项目一致）；
// 5. [`store`] SQLite 存储：nodes/edges/file_hashes + FTS5 全文索引，schema 与
//    参考项目同构（单仓库单库文件，去掉 project 列）；
// 6. [`pipeline`] 编排全量与增量索引。增量 = mtime+size 对比 file_hashes 分类 +
//    入边快照恢复 + 按文件清除重解析 + 整库重写。
//
//! Phase 1 明确不做（后续阶段的扩展点）：Embedding 向量、LSP 类型级解析、
//! git 耦合边（FILE_CHANGES_WITH）、相似度边、USAGE 引用边（schema 已支持该
//! type 字符串）、路由/基建节点。

mod discover;
mod extract;
mod graph;
mod lang_spec;
pub mod mcp;
mod pipeline;
mod queries;
mod resolve;
mod store;

pub use discover::{DiscoverOutcome, DiscoveredFile, discover_files};
pub use extract::{
    CallSite, Extractor, FileExtractResult, ImportRef, OwnerFunction, SymbolDef, TypeRef,
};
pub use graph::{
    EdgeType, GraphBuffer, GraphEdge, GraphNode, NodeId, NodeLabel, calls_edge_properties,
    file_qualified_name,
};
pub use pipeline::{
    IncrementalOutcome, IndexRunStats, PipelineOptions, RunOutcome, run_incremental_if_stale,
    run_index,
};
pub use queries::{
    DetailOutcome, Hotspot, ImpactReport, IndexOverview, SourceSnippet, SymbolCandidate,
    SymbolDetail, SymbolResolution, TraceDirection, TraceHop, TraceOutcome, TraceResult,
    changed_files_via_git, find_symbol_candidates, impacted_symbols, impacted_symbols_for_files,
    index_overview, symbol_detail, trace_calls,
};
pub use resolve::Registry;
pub use store::{
    CODE_INDEX_SCHEMA_VERSION, CodeIndexMeta, CodeIndexStore, FileHashRow, IndexStats, SearchHit,
    camel_split, open_index_db_path, read_index_stats, search_symbols,
};

/// 索引阶段（进度事件文案用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexPhase {
    /// 正在扫描文件树。
    Discover,
    /// tree-sitter 解析提取中。
    Parse,
    /// 符号注册表解析调用关系。
    Resolve,
    /// 写入 SQLite。
    Write,
}

impl IndexPhase {
    pub fn display(self) -> &'static str {
        match self {
            Self::Discover => "扫描文件",
            Self::Parse => "解析符号",
            Self::Resolve => "解析调用关系",
            Self::Write => "写入索引库",
        }
    }
}

/// 管线进度快照。
#[derive(Clone, Debug)]
pub struct IndexProgress {
    pub phase: IndexPhase,
    pub done: usize,
    pub total: usize,
    pub message: String,
}

use crate::types::GitError;

/// 单仓库索引的符号提取语言集合（12 种核心语言）。
/// 其余文本文件仍登记为 File 节点（计入统计）但不解析符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LangId {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Php,
    Kotlin,
}

/// 文件数上限（对齐参考项目 auto_index_limit 默认值）：超出直接报错终止，
/// 避免超大仓库在桌面端长时间占用 CPU 与内存。
pub const MAX_INDEX_FILES: usize = 50_000;

/// 单文件解析字节上限：超过则只登记 File 节点不提取符号
/// （生成产物/压缩数据常见巨文件，解析耗时且无检索价值）。
pub const PARSE_MAX_BYTES: u64 = 1024 * 1024;

impl LangId {
    /// 语言中文名（进度文案用）。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Php => "PHP",
            Self::Kotlin => "Kotlin",
        }
    }
}

pub(crate) fn err(message: impl Into<String>) -> GitError {
    GitError::Message(message.into())
}

#[cfg(test)]
#[path = "../tests/code_index.rs"]
mod tests;
