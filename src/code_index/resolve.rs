//! 跨文件符号解析（参照 codebase-memory-mcp 的 `pipeline/registry.c`）。
//!
//! 参考项目使用六级策略链（import_map → import_map_suffix → same_module →
//! unique_name → suffix_match → fuzzy）。Phase 1 移植为四级，跳过模糊匹配：
//!
//! 1. `local`（0.95）：调用方同文件的唯一同名定义；
//! 2. `import_map`（0.90）：名字命中且定义文件路径与调用文件的一条导入
//!    尾段匹配（`use crate::git::service` ↔ `src/git/service.rs`）；
//! 3. `unique`（0.80）：全仓库唯一同名定义；
//! 4. `suffix`（0.60）：限定调用（`GitService::open` / `a.b.c()`）的限定段
//!    与候选 QN 尾部吻合。
//!
//! 解析失败不建边——宁缺毋滥，边上的 confidence/strategy 忠实记录来源。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::graph::{GraphBuffer, NodeId, NodeLabel};

/// 可被解析为调用目标的符号候选。
#[derive(Clone, Debug)]
struct SymbolCandidate {
    id: NodeId,
    callable: bool,
    container: bool,
    qualified_name: Arc<str>,
    scope: Vec<String>,
    logical_container: String,
    scope_name: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub id: NodeId,
    pub confidence: f32,
    pub strategy: &'static str,
}

#[derive(Debug, Default)]
struct NameBucket {
    candidates: Vec<SymbolCandidate>,
    local: HashMap<Arc<str>, Vec<usize>>,
    qualified: HashMap<String, Vec<usize>>,
    bare: Vec<usize>,
    callable: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Registry {
    name_index: HashMap<String, NameBucket>,
    import_files: HashMap<String, Vec<(Arc<str>, usize)>>,
}

fn unique<'a>(mut candidates: impl Iterator<Item = &'a SymbolCandidate>) -> Option<&'a SymbolCandidate> {
    let first = candidates.next()?;
    candidates.next().is_none().then_some(first)
}

impl Registry {
    /// 从图缓冲构建名字索引（只收定义类符号）。
    pub fn build(graph: &GraphBuffer) -> Self {
        let mut registry = Self::default();
        let mut files = HashSet::new();
        for node in &graph.nodes {
            if !node.label.is_symbol() || node.label == NodeLabel::Field { continue; }
            let scope: Vec<String> = serde_json::from_str::<serde_json::Value>(&node.properties).ok()
                .and_then(|properties| properties["scope"].as_array().cloned())
                .unwrap_or_default().iter().filter_map(|part| part.as_str().map(str::to_string)).collect();
            let scope_name = scope.join(".");
            let stem = node.file_path.rsplit_once('.').map_or(node.file_path.as_ref(), |(stem, _)| stem)
                .replace(['/', '\\'], ".");
            let stem = stem.strip_suffix(".mod").unwrap_or(&stem);
            let logical_container = if scope.is_empty() { stem.to_string() } else { format!("{stem}.{scope_name}") };
            let candidate = SymbolCandidate { id: node.id,
                qualified_name: Arc::clone(&node.qualified_name), scope, scope_name, logical_container,
                callable: matches!(node.label, NodeLabel::Function | NodeLabel::Method),
                container: matches!(node.label, NodeLabel::Class | NodeLabel::Struct | NodeLabel::Interface | NodeLabel::Trait | NodeLabel::Enum) };
            if files.insert(Arc::clone(&node.file_path)) {
                let stem = node.file_path.rsplit_once('.').map_or(node.file_path.as_ref(), |(stem, _)| stem);
                let segments: Vec<_> = stem.split(['/', '\\']).filter(|part| !part.is_empty()).map(str::to_ascii_lowercase).collect();
                for start in 0..segments.len() {
                    registry.import_files.entry(segments[start..].join(".")).or_default().push((Arc::clone(&node.file_path), segments.len()));
                }
            }
            let bucket = registry.name_index.entry(node.name.clone()).or_default();
            let id = bucket.candidates.len();
            if candidate.callable {
                bucket.callable.push(id);
                bucket.local.entry(Arc::clone(&node.file_path)).or_default().push(id);
                if candidate.scope.is_empty() { bucket.bare.push(id); }
                let parent = candidate.qualified_name.split('#').next().unwrap_or(&candidate.qualified_name)
                    .rsplit_once('.').map_or("", |(parent, _)| parent);
                let aliases: HashSet<_> = [parent.rsplit('.').next().unwrap_or(""),
                    candidate.logical_container.rsplit('.').next().unwrap_or(""), candidate.scope_name.rsplit('.').next().unwrap_or("")]
                    .into_iter().filter(|alias| !alias.is_empty()).collect();
                for alias in aliases { bucket.qualified.entry(alias.to_string()).or_default().push(id); }
            }
            bucket.candidates.push(candidate);
        }
        registry
    }

    /// 导入可达文件只算一次，调用点复用；保留旧策略的路径深度约束。
    pub(super) fn imported_files(&self, imports: &[String]) -> HashSet<Arc<str>> {
        let mut files = HashSet::new();
        for import in imports {
            let parts: Vec<_> = import.split('.').filter(|part| !part.is_empty()).map(str::to_ascii_lowercase).collect();
            let start = parts.iter().take_while(|part| RUST_PATH_ROOTS.contains(&part.as_str())).count();
            let parts = &parts[start..];
            if parts.is_empty() { continue; }
            for suffix in [Some(parts), (parts.len() > 1).then(|| &parts[..parts.len() - 1])].into_iter().flatten() {
                if let Some(candidates) = self.import_files.get(&suffix.join(".")) {
                    files.extend(candidates.iter().filter(|(_, depth)| *depth >= parts.len()).map(|(file, _)| Arc::clone(file)));
                }
            }
        }
        files
    }

    /// 解析一个调用点。`qualifier` 是限定表达式的完整前缀
    /// （`GitService::open` 的 GitService、`a.b.push` 的 a.b），可为 None。
    pub fn resolve_call(
        &self,
        name: &str,
        calling_file: &str,
        imports: &[String],
        qualifier: Option<&str>,
    ) -> Option<ResolvedTarget> {
        self.resolve_call_in_scope(name, calling_file, imports, qualifier, &[])
    }

    pub(super) fn resolve_call_in_scope(
        &self, name: &str, calling_file: &str, imports: &[String], qualifier: Option<&str>, scope: &[String],
    ) -> Option<ResolvedTarget> {
        self.resolve_prepared(name, calling_file, &self.imported_files(imports), qualifier, scope)
    }

    pub(super) fn resolve_prepared(
        &self, name: &str, calling_file: &str, imports: &HashSet<Arc<str>>, qualifier: Option<&str>, scope: &[String],
    ) -> Option<ResolvedTarget> {
        let bucket = self.name_index.get(name)?;
        let explicit_members = matches!(super::graph::lang_of_rel_path(calling_file),
            Some(super::LangId::Rust | super::LangId::Python | super::LangId::JavaScript | super::LangId::TypeScript | super::LangId::Tsx | super::LangId::Go));
        let eligible = |candidate: &&SymbolCandidate| !explicit_members || qualifier.is_some() || candidate.scope.is_empty();
        let local = bucket.local.get(calling_file).map(Vec::as_slice).unwrap_or_default();
        let target = |candidate: &SymbolCandidate, confidence, strategy| ResolvedTarget { id: candidate.id, confidence, strategy };
        if let Some(qualifier) = qualifier {
            let qualifier = match qualifier { "self" | "this" | "Self" => scope.last()?.as_str(), value => value };
            let normalized = qualifier.replace("::", ".");
            let qualifier = normalized.trim_start_matches("crate.");
            let ids = bucket.qualified.get(qualifier.rsplit('.').next()?)?;
            if let Some(candidate) = unique(local.iter().map(|id| &bucket.candidates[*id]).filter(|candidate| qualifier_matches(candidate, qualifier))) {
                return Some(target(candidate, 0.90, "suffix"));
            }
            return unique(ids.iter().map(|id| &bucket.candidates[*id]).filter(|candidate| qualifier_matches(candidate, qualifier)))
                .map(|candidate| target(candidate, 0.90, "suffix"));
        }
        if !scope.is_empty() {
            if let Some(candidate) = unique(local.iter().map(|id| &bucket.candidates[*id]).filter(eligible).filter(|candidate| candidate.scope == scope)) {
                return Some(target(candidate, 0.95, "scope"));
            }
        }
        if let Some(candidate) = unique(local.iter().map(|id| &bucket.candidates[*id]).filter(eligible)) {
            return Some(target(candidate, 0.95, "local"));
        }
        // 只遍历可达文件中的同名符号，避免常见名字扫全仓库。
        if let Some(candidate) = unique(imports.iter().filter_map(|file| bucket.local.get(file.as_ref()))
            .flat_map(|ids| ids.iter().map(|id| &bucket.candidates[*id])).filter(eligible)) {
            return Some(target(candidate, 0.90, "import_map"));
        }
        let ids = if explicit_members { &bucket.bare } else { &bucket.callable };
        (ids.len() == 1).then(|| target(&bucket.candidates[ids[0]], 0.80, "unique"))
    }

    /// 解析类型引用（INHERITS/IMPLEMENTS 目标）：只接受唯一容器，歧义时不建边。
    pub fn resolve_type(&self, name: &str) -> Option<NodeId> {
        let bucket = self.name_index.get(name)?;
        unique(bucket.candidates.iter().filter(|candidate| candidate.container)).map(|candidate| candidate.id)
    }
}

fn qualifier_matches(candidate: &SymbolCandidate, qualifier: &str) -> bool {
    if qualifier.is_empty() { return false; }
    let qn = candidate.qualified_name.split('#').next().unwrap_or(&candidate.qualified_name);
    let parent = qn.rsplit_once('.').map_or("", |(parent, _)| parent);
    let matches = |value: &str| value == qualifier || value.strip_suffix(qualifier).is_some_and(|prefix| prefix.ends_with('.'));
    matches(parent) || matches(&candidate.logical_container) || candidate.scope_name == qualifier
}

const RUST_PATH_ROOTS: &[&str] = &["crate", "self", "super"];
