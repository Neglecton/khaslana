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
//! 候选先按语言、接收者、类作用域和可见性过滤；解析失败不建边。

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
    language: Option<super::LangId>,
    private: bool,
    static_member: bool,
    file_path: Arc<str>,
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

#[derive(Default)]
pub(super) struct ImportContext {
    files: HashSet<Arc<str>>,
    static_members: HashMap<Arc<str>, HashSet<String>>,
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
            let properties: serde_json::Value = serde_json::from_str(&node.properties).unwrap_or_default();
            let scope: Vec<String> = properties["scope"].as_array().cloned()
                .unwrap_or_default().iter().filter_map(|part| part.as_str().map(str::to_string)).collect();
            let signature = properties["signature"].as_str().unwrap_or("");
            let (private, static_member) = signature.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .fold((false, false), |(private, static_member), word| (private || word == "private", static_member || word == "static"));
            let scope_name = scope.join(".");
            let stem = node.file_path.rsplit_once('.').map_or(node.file_path.as_ref(), |(stem, _)| stem)
                .replace(['/', '\\'], ".");
            let stem = stem.strip_suffix(".mod").unwrap_or(&stem);
            let logical_container = if scope.is_empty() { stem.to_string() } else { format!("{stem}.{scope_name}") };
            let candidate = SymbolCandidate { id: node.id,
                qualified_name: Arc::clone(&node.qualified_name), scope, scope_name, logical_container,
                language: super::graph::lang_of_rel_path(&node.file_path),
                private, static_member,
                file_path: Arc::clone(&node.file_path),
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
    pub(super) fn imported_files(&self, imports: &[String]) -> ImportContext {
        let mut context = ImportContext::default();
        for import in imports {
            let (module, member) = match import.strip_prefix("static:") {
                Some(value) => match value.rsplit_once('.') { Some((module, member)) => (module, Some(member)), None => continue },
                None => (import.as_str(), None),
            };
            let parts: Vec<_> = module.split('.').filter(|part| !part.is_empty()).map(str::to_ascii_lowercase).collect();
            let start = parts.iter().take_while(|part| RUST_PATH_ROOTS.contains(&part.as_str())).count();
            let parts = &parts[start..];
            if parts.is_empty() { continue; }
            for suffix in [Some(parts), (member.is_none() && parts.len() > 1).then(|| &parts[..parts.len() - 1])].into_iter().flatten() {
                if let Some(candidates) = self.import_files.get(&suffix.join(".")) {
                    for (file, _) in candidates.iter().filter(|(_, depth)| *depth >= parts.len()) {
                        context.files.insert(Arc::clone(file));
                        if let Some(member) = member { context.static_members.entry(Arc::clone(file)).or_default().insert(member.to_string()); }
                    }
                }
            }
        }
        context
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
        &self, name: &str, calling_file: &str, imports: &ImportContext, qualifier: Option<&str>, scope: &[String],
    ) -> Option<ResolvedTarget> {
        let bucket = self.name_index.get(name)?;
        let language = super::graph::lang_of_rel_path(calling_file);
        let java = language == Some(super::LangId::Java);
        let explicit_members = matches!(language,
            Some(super::LangId::Rust | super::LangId::Python | super::LangId::JavaScript | super::LangId::TypeScript | super::LangId::Tsx | super::LangId::Go));
        let compatible = |candidate: &&SymbolCandidate| same_language(language, candidate.language);
        let eligible = |candidate: &&SymbolCandidate| same_language(language, candidate.language)
            && if java { candidate.scope == scope || candidate.scope.is_empty() }
               else { !explicit_members || qualifier.is_some() || candidate.scope.is_empty() };
        let local = bucket.local.get(calling_file).map(Vec::as_slice).unwrap_or_default();
        let target = |candidate: &SymbolCandidate, confidence, strategy| ResolvedTarget { id: candidate.id, confidence, strategy };
        if let Some(qualifier) = qualifier {
            // this/self 没有当前类的定义时，不得跳到其他文件的同名类或唯一方法。
            if matches!(qualifier, "self" | "this" | "Self") {
                if scope.is_empty() { return None; }
                return unique(local.iter().map(|id| &bucket.candidates[*id]).filter(compatible)
                    .filter(|candidate| candidate.scope == scope)).map(|candidate| target(candidate, 0.95, "scope"));
            }
            let normalized = qualifier.replace("::", ".");
            let qualifier = normalized.trim_start_matches("crate.");
            let ids = bucket.qualified.get(qualifier.rsplit('.').next()?)?;
            let qualified = |candidate: &&SymbolCandidate| same_language(language, candidate.language)
                && qualifier_matches(candidate, qualifier)
                && (!java || candidate.static_member)
                && (!java || candidate.file_path.as_ref() == calling_file || imports.files.contains(&candidate.file_path)
                    || std::path::Path::new(candidate.file_path.as_ref()).parent() == std::path::Path::new(calling_file).parent())
                && (!java || !candidate.private || candidate.file_path.as_ref() == calling_file && candidate.scope == scope);
            if let Some(candidate) = unique(local.iter().map(|id| &bucket.candidates[*id]).filter(qualified)) {
                return Some(target(candidate, 0.90, "suffix"));
            }
            return unique(ids.iter().map(|id| &bucket.candidates[*id]).filter(qualified))
                .map(|candidate| target(candidate, 0.90, "suffix"));
        }
        if !scope.is_empty() {
            let mut candidates = local.iter().map(|id| &bucket.candidates[*id]).filter(eligible)
                .filter(|candidate| candidate.scope == scope).peekable();
            if candidates.peek().is_some() {
                return unique(candidates).map(|candidate| target(candidate, 0.95, "scope"));
            }
        }
        let mut candidates = local.iter().map(|id| &bucket.candidates[*id]).filter(eligible).peekable();
        if candidates.peek().is_some() {
            // 当前层存在歧义就停止，不能把本地重载错误转移到导入的同名方法。
            return unique(candidates).map(|candidate| target(candidate, 0.95, "local"));
        }
        // 只遍历可达文件中的同名符号，避免常见名字扫全仓库。
        if let Some(candidate) = unique(imports.files.iter().filter_map(|file| bucket.local.get(file.as_ref()))
            .flat_map(|ids| ids.iter().map(|id| &bucket.candidates[*id]))
            .filter(|candidate| same_language(language, candidate.language)
                && if java { candidate.static_member && !candidate.private
                    && imports.static_members.get(&candidate.file_path).is_some_and(|members| members.contains(name) || members.contains("*"))
                } else { eligible(candidate) })) {
            return Some(target(candidate, 0.90, "import_map"));
        }
        // Java 裸方法只能来自当前类或已导入的静态成员；全仓库唯一不代表可达。
        if java { return None; }
        let ids = if explicit_members { &bucket.bare } else { &bucket.callable };
        unique(ids.iter().map(|id| &bucket.candidates[*id]).filter(compatible))
            .map(|candidate| target(candidate, 0.80, "unique"))
    }

    /// 无同语言仓库定义与有候选但接收者/作用域不明确分开统计，不能把二者都当解析失败率。
    pub(super) fn has_call_candidate(&self, name: &str, calling_file: &str) -> bool {
        let language = super::graph::lang_of_rel_path(calling_file);
        self.name_index.get(name).is_some_and(|bucket| bucket.candidates.iter()
            .any(|candidate| (candidate.callable || candidate.container) && same_language(language, candidate.language)))
    }

    /// 解析类型引用（INHERITS/IMPLEMENTS 目标）：只接受唯一容器，歧义时不建边。
    pub fn resolve_type(&self, name: &str, calling_file: &str) -> Option<NodeId> {
        let bucket = self.name_index.get(name)?;
        let language = super::graph::lang_of_rel_path(calling_file);
        unique(bucket.candidates.iter().filter(|candidate| candidate.container && same_language(language, candidate.language))).map(|candidate| candidate.id)
    }
}

fn same_language(left: Option<super::LangId>, right: Option<super::LangId>) -> bool {
    use super::LangId::{C, Cpp, JavaScript, TypeScript, Tsx};
    left == right || matches!((left, right),
        (Some(JavaScript | TypeScript | Tsx), Some(JavaScript | TypeScript | Tsx)) | (Some(C | Cpp), Some(C | Cpp)))
}

fn qualifier_matches(candidate: &SymbolCandidate, qualifier: &str) -> bool {
    if qualifier.is_empty() { return false; }
    let qn = candidate.qualified_name.split('#').next().unwrap_or(&candidate.qualified_name);
    let parent = qn.rsplit_once('.').map_or("", |(parent, _)| parent);
    let matches = |value: &str| value == qualifier || value.strip_suffix(qualifier).is_some_and(|prefix| prefix.ends_with('.'));
    matches(parent) || matches(&candidate.logical_container) || candidate.scope_name == qualifier
}

const RUST_PATH_ROOTS: &[&str] = &["crate", "self", "super"];
