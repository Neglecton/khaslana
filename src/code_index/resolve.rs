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

use std::collections::HashMap;

use super::graph::{GraphBuffer, NodeId, NodeLabel};

/// 可被解析为调用目标的符号候选。
#[derive(Clone, Debug)]
struct SymbolCandidate {
    id: NodeId,
    file_path: String,
    callable: bool,
    container: bool,
    qualified_name: String,
    scope: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub id: NodeId,
    pub confidence: f32,
    pub strategy: &'static str,
}

#[derive(Debug, Default)]
pub struct Registry {
    name_index: HashMap<String, Vec<SymbolCandidate>>,
}

impl Registry {
    /// 从图缓冲构建名字索引（只收定义类符号）。
    pub fn build(graph: &GraphBuffer) -> Self {
        let mut name_index: HashMap<String, Vec<SymbolCandidate>> = HashMap::new();
        for node in &graph.nodes {
            if !node.label.is_symbol() || node.label == NodeLabel::Field {
                continue;
            }
            name_index
                .entry(node.name.clone())
                .or_default()
                .push(SymbolCandidate {
                    id: node.id,
                    file_path: node.file_path.clone(),
                    qualified_name: node.qualified_name.clone(),
                    scope: serde_json::from_str::<serde_json::Value>(&node.properties).ok()
                        .and_then(|properties| properties["scope"].as_array().cloned())
                        .unwrap_or_default().iter().filter_map(|part| part.as_str().map(str::to_string)).collect(),
                    callable: matches!(node.label, NodeLabel::Function | NodeLabel::Method),
                    container: matches!(
                        node.label,
                        NodeLabel::Class
                            | NodeLabel::Struct
                            | NodeLabel::Interface
                            | NodeLabel::Trait
                            | NodeLabel::Enum
                    ),
                });
        }
        Self { name_index }
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
        let candidates = self.name_index.get(name)?;
        // 这些语言调用成员需要显式接收者，裸函数名不能猜成类中的同名方法。
        let explicit_members = matches!(super::graph::lang_of_rel_path(calling_file),
            Some(super::LangId::Rust | super::LangId::Python | super::LangId::JavaScript | super::LangId::TypeScript | super::LangId::Tsx | super::LangId::Go));
        let callables: Vec<&SymbolCandidate> = candidates.iter().filter(|c| c.callable
            && (qualifier.is_some() || !explicit_members || c.scope.is_empty())).collect();
        if callables.is_empty() {
            return None;
        }

        // 限定调用必须先证明限定段与模块或容器相符，不能退回全局同名猜测。
        if let Some(qualifier) = qualifier {
            let qualifier = match qualifier {
                "self" | "this" | "Self" => scope.last()?.as_str(),
                qualifier => qualifier,
            };
            let qualified: Vec<_> = callables.iter().copied()
                .filter(|candidate| qualifier_matches(candidate, qualifier, name)).collect();
            let local: Vec<_> = qualified.iter().copied().filter(|candidate| candidate.file_path == calling_file).collect();
            let chosen = if local.len() == 1 { &local } else { &qualified };
            return (chosen.len() == 1).then(|| ResolvedTarget {
                id: chosen[0].id, confidence: 0.90, strategy: "suffix",
            });
        }

        if !scope.is_empty() {
            let scoped: Vec<_> = callables.iter().filter(|candidate|
                candidate.file_path == calling_file && candidate.scope == scope).collect();
            if scoped.len() == 1 {
                return Some(ResolvedTarget { id: scoped[0].id, confidence: 0.95, strategy: "scope" });
            }
        }

        // 1. 同文件唯一同名。
        let locals: Vec<&&SymbolCandidate> = callables
            .iter()
            .filter(|c| c.file_path == calling_file)
            .collect();
        if locals.len() == 1 {
            return Some(ResolvedTarget {
                id: locals[0].id,
                confidence: 0.95,
                strategy: "local",
            });
        }

        // 2. 导入尾段匹配且唯一。
        if !imports.is_empty() {
            let via_imports: Vec<&&SymbolCandidate> = callables
                .iter()
                .filter(|c| {
                    imports
                        .iter()
                        .any(|imp| import_tail_matches(imp, &c.file_path))
                })
                .collect();
            if via_imports.len() == 1 {
                return Some(ResolvedTarget {
                    id: via_imports[0].id,
                    confidence: 0.90,
                    strategy: "import_map",
                });
            }
        }

        // 3. 全仓库唯一。
        if callables.len() == 1 {
            return Some(ResolvedTarget {
                id: callables[0].id,
                confidence: 0.80,
                strategy: "unique",
            });
        }

        None
    }

    /// 解析类型引用（INHERITS/IMPLEMENTS 目标）：只接受唯一容器，歧义时不建边。
    pub fn resolve_type(&self, name: &str) -> Option<NodeId> {
        let candidates = self.name_index.get(name)?;
        let containers: Vec<_> = candidates.iter().filter(|candidate| candidate.container).collect();
        (containers.len() == 1).then(|| containers[0].id)
    }
}

fn qualifier_matches(candidate: &SymbolCandidate, qualifier: &str, name: &str) -> bool {
    let qualifier = qualifier.replace("::", ".");
    let qualifier = qualifier.trim_start_matches("crate.");
    if qualifier.is_empty() { return false; }
    let qn = candidate.qualified_name.split('#').next().unwrap_or(&candidate.qualified_name);
    if qn.ends_with(&format!(".{qualifier}.{name}")) { return true; }
    let file = candidate.file_path.rsplit_once('.').map_or(candidate.file_path.as_str(), |(stem, _)| stem)
        .replace(['/', '\\'], ".");
    let file = file.strip_suffix(".mod").unwrap_or(&file);
    let container = if candidate.scope.is_empty() { file.to_string() } else { format!("{file}.{}", candidate.scope.join(".")) };
    container == qualifier || container.ends_with(&format!(".{qualifier}")) || candidate.scope.join(".") == qualifier
}

/// Rust 路径根段：`use crate::git::service` 的 crate 段在文件路径里对应
/// 仓库名/`src`，永远对不上，剥掉后再比较。
const RUST_PATH_ROOTS: &[&str] = &["crate", "self", "super"];

/// 导入尾段匹配：导入路径（剥根段后）与定义文件路径（去扩展名、统一分隔符）
/// 的尾段一致。`use crate::git::service` 剥 crate 后 `git.service` ↔
/// `src/git/service.rs` 尾两段 ✓；`os.path` ↔ `os/path.py` ✓。
/// 若整段尾匹配不中，再回退「导入路径去掉末段」与文件尾段比较
/// （`use crate::git::browse` 指向目录，能命中目录下任意文件，对齐参考
/// 项目 is_import_reachable 的前缀可达语义：导入是容器路径时可达其成员）。
fn import_tail_matches(import: &str, file_path: &str) -> bool {
    let mut import_segs: Vec<String> = import
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    while import_segs
        .first()
        .is_some_and(|seg| RUST_PATH_ROOTS.contains(&seg.as_str()))
    {
        import_segs.remove(0);
    }
    if import_segs.is_empty() {
        return false;
    }
    let without_ext = file_path.rsplit_once('.').map_or(file_path, |(b, _)| b);
    let file_segs: Vec<String> = without_ext
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if !import_segs.is_empty() && import_segs.len() <= file_segs.len() {
        if file_seg_ends_with(&file_segs, &import_segs) {
            return true;
        }
        // 回退：剥掉导入末段（模块名）后作为容器路径匹配文件路径。
        // 至少保留一段，避免单段 `crate` 之类剥完匹配整个仓库。
        if import_segs.len() >= 2 {
            let container = &import_segs[..import_segs.len() - 1];
            if file_seg_ends_with(&file_segs, container) {
                return true;
            }
        }
    }
    false
}

fn file_seg_ends_with(file_segs: &[String], import_segs: &[String]) -> bool {
    let start = file_segs.len() - import_segs.len();
    file_segs[start..] == *import_segs
}
