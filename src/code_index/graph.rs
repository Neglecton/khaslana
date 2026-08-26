//! 内存图缓冲（参照 codebase-memory-mcp 的 `graph_buffer.c` / `cbm_gbuf_t`）。
//!
//! 索引期间全部节点与边驻留 RAM（索引期间不碰 SQLite），qualified_name 作为
//! 全局唯一键去重，边按 (source, target, type) 三元组去重。结束由
//! [`crate::code_index::store`] 一次性落盘。

use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::LangId;

pub type NodeId = u32;

/// 节点标签。字符串值与参考项目的 label 集合保持一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeLabel {
    Project,
    Branch,
    Folder,
    File,
    Module,
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Enum,
    Trait,
    Type,
    Field,
}

impl NodeLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Branch => "Branch",
            Self::Folder => "Folder",
            Self::File => "File",
            Self::Module => "Module",
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Class => "Class",
            Self::Struct => "Struct",
            Self::Interface => "Interface",
            Self::Enum => "Enum",
            Self::Trait => "Trait",
            Self::Type => "Type",
            Self::Field => "Field",
        }
    }

    /// 是否为「定义类」符号（统计口径：符号数）。
    pub fn is_symbol(self) -> bool {
        matches!(
            self,
            Self::Function
                | Self::Method
                | Self::Class
                | Self::Struct
                | Self::Interface
                | Self::Enum
                | Self::Trait
                | Self::Type
                | Self::Field
        )
    }
}

/// 边类型。字符串值与参考项目一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeType {
    ContainsFolder,
    ContainsFile,
    HasBranch,
    Defines,
    DefinesMethod,
    Calls,
    Imports,
    Inherits,
    Implements,
}

impl EdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContainsFolder => "CONTAINS_FOLDER",
            Self::ContainsFile => "CONTAINS_FILE",
            Self::HasBranch => "HAS_BRANCH",
            Self::Defines => "DEFINES",
            Self::DefinesMethod => "DEFINES_METHOD",
            Self::Calls => "CALLS",
            Self::Imports => "IMPORTS",
            Self::Inherits => "INHERITS",
            Self::Implements => "IMPLEMENTS",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: NodeId,
    pub label: NodeLabel,
    pub name: String,
    pub qualified_name: String,
    /// 相对仓库根路径；Project/Branch/Folder/Module 为空串。
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// 附加属性 JSON（CALLS 边的 confidence/strategy、File 的 line_count 等）。
    pub properties: String,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub source: NodeId,
    pub target: NodeId,
    pub etype: EdgeType,
    pub properties: String,
}

/// 内存图缓冲。
#[derive(Debug, Default)]
pub struct GraphBuffer {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    qn_index: HashMap<String, NodeId>,
    edge_keys: HashSet<(NodeId, NodeId, EdgeType)>,
}

impl GraphBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.label.is_symbol()).count()
    }

    pub fn get(&self, id: NodeId) -> &GraphNode {
        &self.nodes[id as usize]
    }

    pub fn find_by_qn(&self, qualified_name: &str) -> Option<NodeId> {
        self.qn_index.get(qualified_name).copied()
    }

    pub fn find_by_label_and_name(&self, label: NodeLabel, name: &str) -> Option<NodeId> {
        // 线性扫描足够：调用方只在解析阶段少量使用；
        // 大规模查找走 store 载入后重建的 name_index（见 resolve.rs）。
        self.nodes
            .iter()
            .find(|n| n.label == label && n.name == name)
            .map(|n| n.id)
    }

    /// 插入结构类节点（Project/Folder/File/Branch），QN 相同视为同一节点（幂等）。
    pub fn upsert_node(
        &mut self,
        label: NodeLabel,
        name: impl Into<String>,
        qualified_name: impl Into<String>,
        file_path: impl Into<String>,
        start_line: u32,
        end_line: u32,
        properties: String,
    ) -> NodeId {
        let qualified_name = qualified_name.into();
        if let Some(&existing) = self.qn_index.get(&qualified_name) {
            return existing;
        }
        let id = self.nodes.len() as NodeId;
        self.nodes.push(GraphNode {
            id,
            label,
            name: name.into(),
            qualified_name: qualified_name.clone(),
            file_path: file_path.into(),
            start_line,
            end_line,
            properties,
        });
        self.qn_index.insert(qualified_name, id);
        id
    }

    /// 插入定义类符号节点。与结构节点不同：同 QN 的后续插入追加 `#2`/`#3`…
    /// 消歧后缀生成新节点（Java/C++ 重载、同名嵌套函数），不合并。
    pub fn add_symbol(
        &mut self,
        label: NodeLabel,
        name: impl Into<String>,
        base_qualified_name: String,
        file_path: impl Into<String>,
        start_line: u32,
        end_line: u32,
        properties: String,
    ) -> NodeId {
        let mut qualified_name = base_qualified_name.clone();
        let mut suffix = 2u32;
        while self.qn_index.contains_key(&qualified_name) {
            qualified_name = format!("{base_qualified_name}#{suffix}");
            suffix += 1;
        }
        let id = self.nodes.len() as NodeId;
        self.nodes.push(GraphNode {
            id,
            label,
            name: name.into(),
            qualified_name: qualified_name.clone(),
            file_path: file_path.into(),
            start_line,
            end_line,
            properties,
        });
        self.qn_index.insert(qualified_name, id);
        id
    }

    /// 插入边；重复三元组静默忽略（参考项目 insert_edge 同语义，增量恢复依赖幂等）。
    /// 返回是否真正插入。
    pub fn add_edge(
        &mut self,
        source: NodeId,
        target: NodeId,
        etype: EdgeType,
        properties: String,
    ) -> bool {
        if source == target {
            return false;
        }
        if !self.edge_keys.insert((source, target, etype)) {
            return false;
        }
        self.edges.push(GraphEdge {
            source,
            target,
            etype,
            properties,
        });
        true
    }

    /// 删除一组文件相关的全部节点（File 及其 DEFINES 的符号），边随级联清除
    /// （模拟 SQLite ON DELETE CASCADE，内存版）。幸存节点 id 顺序重排，
    /// 维持「id == nodes 下标」不变量。返回被删节点数。
    pub fn purge_files(&mut self, rel_paths: &HashSet<String>) -> usize {
        let before = self.nodes.len();
        self.nodes
            .retain(|n| n.file_path.is_empty() || !rel_paths.contains(&n.file_path));
        let removed = before - self.nodes.len();
        if removed == 0 {
            return 0;
        }
        // 重排 id 并重建索引；引用了已删节点的边一并清除。
        for (index, node) in self.nodes.iter_mut().enumerate() {
            node.id = index as NodeId;
        }
        self.qn_index = self
            .nodes
            .iter()
            .map(|n| (n.qualified_name.clone(), n.id))
            .collect();
        let alive: HashSet<NodeId> = self.qn_index.values().copied().collect();
        self.edges
            .retain(|e| alive.contains(&e.source) && alive.contains(&e.target));
        self.edge_keys = self
            .edges
            .iter()
            .map(|e| (e.source, e.target, e.etype))
            .collect();
        removed
    }
}

/// 计算仓库内相对路径的 File 节点 QN：`项目名.目录段.文件名`，
/// 与参考项目 `cbm_pipeline_fqn_compute` 的 project.dir.parts.name 结构一致。
/// 符号 QN 在此之上追加作用域链。
pub fn file_qualified_name(project: &str, rel_path: &str) -> String {
    let mut parts: Vec<&str> = vec![project];
    parts.extend(rel_path.split('/'));
    parts.join(".")
}

/// 目录节点的 QN（空 rel_dir 返回项目名本身）。
pub fn folder_qualified_name(project: &str, rel_dir: &str) -> String {
    if rel_dir.is_empty() {
        project.to_string()
    } else {
        format!("{project}.{}", rel_dir.replace('/', "."))
    }
}

/// 构造 File 节点属性 JSON。
pub fn file_properties(line_count: usize) -> String {
    json!({ "line_count": line_count }).to_string()
}

/// 构造 CALLS 边属性 JSON（对齐参考项目 {callee, confidence, strategy}）。
pub fn calls_edge_properties(callee: &str, confidence: f32, strategy: &str) -> String {
    json!({ "callee": callee, "confidence": confidence, "strategy": strategy }).to_string()
}

/// 从扩展名推断语言失败时返回 None 的便捷封装（discover 层已过滤二进制，
/// 这里兜底处理无扩展名文件）。
pub fn lang_of_rel_path(rel_path: &str) -> Option<LangId> {
    let ext = rel_path.rsplit('.').next()?;
    if ext.len() == rel_path.len() {
        return None; // 无扩展名
    }
    super::lang_spec::LangSpec::detect(ext)
}
