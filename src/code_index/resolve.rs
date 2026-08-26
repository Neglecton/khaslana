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

    /// 解析一个调用点。`qualifier` 是限定表达式的倒数第二段
    /// （`GitService::open` 的 GitService、`a.b.push` 的 b），可为 None。
    pub fn resolve_call(
        &self,
        name: &str,
        calling_file: &str,
        imports: &[String],
        qualifier: Option<&str>,
    ) -> Option<ResolvedTarget> {
        let candidates = self.name_index.get(name)?;
        let callables: Vec<&SymbolCandidate> = candidates.iter().filter(|c| c.callable).collect();
        if callables.is_empty() {
            return None;
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

        // 4. 限定调用且排除本文件后唯一（suffix 策略的省内存退化版：
        //    name_index 不缓存 QN，精度略降于参考项目 suffix_match）。
        if qualifier.is_some() {
            let others: Vec<&SymbolCandidate> = callables
                .iter()
                .copied()
                .filter(|c| c.file_path != calling_file)
                .collect();
            if others.len() == 1 {
                return Some(ResolvedTarget {
                    id: others[0].id,
                    confidence: 0.60,
                    strategy: "suffix",
                });
            }
        }

        None
    }

    /// 解析类型引用（INHERITS/IMPLEMENTS 目标）：优先容器类符号，
    /// 多候选取首个（发现顺序确定性）。
    pub fn resolve_type(&self, name: &str) -> Option<NodeId> {
        let candidates = self.name_index.get(name)?;
        candidates
            .iter()
            .find(|c| c.container)
            .or_else(|| candidates.first())
            .map(|c| c.id)
    }
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
