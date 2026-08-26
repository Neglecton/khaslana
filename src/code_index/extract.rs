//! tree-sitter 符号提取（参照 codebase-memory-mcp 的 `cbm_extract_file` /
//! `extract_defs.c` / `extract_unified.c`）。
//!
//! 单遍 TreeCursor walk 同时收集 定义 / 导入 / 调用点 / 类型继承关系，
//! 作用域栈维护 class/impl 上下文（上下文内的 Function 定义升级为 Method）。
//! 所有产出均为 owned 数据，便于跨线程传递。

use std::collections::{HashMap, VecDeque};

use tree_sitter::{Node, Parser};

use super::graph::NodeLabel;
use super::lang_spec::{CallNameStrategy, DefKind, HeritageKind, LangSpec};
use super::{LangId, err};
use crate::types::Result;

/// 单文件提取结果。
#[derive(Debug, Default)]
pub struct FileExtractResult {
    pub defs: Vec<SymbolDef>,
    pub imports: Vec<ImportRef>,
    pub calls: Vec<CallSite>,
    pub type_refs: Vec<TypeRef>,
}

/// 一个定义符号。`scope` 是外层类/命名空间链（不含自身名）。
#[derive(Clone, Debug)]
pub struct SymbolDef {
    pub label: NodeLabel,
    pub name: String,
    pub scope: Vec<String>,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug)]
pub struct ImportRef {
    /// 清洗后的模块路径文本（如 `crate.git.service`、`os.path`）。
    pub module: String,
}

/// 调用点。`callee_display` 为原始表达式文本，`name` 为解析用末段名；
/// `owner` 记录捕获时的归属函数（class 链 + 函数名），用于把 CALLS 边的
/// source 定位到具体函数——字段初始化器等场景为 None（退化为文件级调用）。
#[derive(Clone, Debug)]
pub struct CallSite {
    pub callee_display: String,
    pub name: String,
    pub line: u32,
    pub owner: Option<OwnerFunction>,
}

/// 归属函数：class/impl 上下文链（不含函数名）+ 函数名。
#[derive(Clone, Debug)]
pub struct OwnerFunction {
    pub class_chain: Vec<String>,
    pub fn_name: String,
}

/// 类型关系引用（INHERITS / IMPLEMENTS 边的来源）。
#[derive(Clone, Debug)]
pub struct TypeRef {
    pub name: String,
    pub inherits: bool,
}

/// walk 深度上限：防御病态嵌套的表达式树。
const MAX_WALK_DEPTH: usize = 256;

/// 提取器：每线程一个实例，内部按语言复用 Parser 实例
/// （参照参考项目的线程本地 parser 复用）。
#[derive(Default)]
pub struct Extractor {
    parsers: HashMap<u32, Parser>,
}

impl Extractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析并提取单文件。初始化解析器失败返回错误；语法树不完整
    /// （has_error）仍照常提取——部分 AST 对符号检索同样有价值。
    pub fn extract(&mut self, lang: LangId, bytes: &[u8]) -> Result<Option<FileExtractResult>> {
        let parser = self.parser_for(lang)?;
        let Some(tree) = parser.parse(bytes, None) else {
            return Ok(None);
        };
        let spec = super::lang_spec::LangSpec::for_id(lang);
        let mut ctx = WalkContext {
            source: bytes,
            spec,
            result: FileExtractResult::default(),
            class_stack: Vec::new(),
            fn_stack: Vec::new(),
        };
        walk(tree.root_node(), 0, &mut ctx);
        Ok(Some(ctx.result))
    }

    fn parser_for(&mut self, lang: LangId) -> Result<&mut Parser> {
        let key = lang as u32;
        if !self.parsers.contains_key(&key) {
            let mut parser = Parser::new();
            let language = super::lang_spec::language_of(lang);
            parser
                .set_language(&language)
                .map_err(|e| err(format!("初始化 {} 解析器失败：{e}", lang.display_name())))?;
            self.parsers.insert(key, parser);
        }
        Ok(self.parsers.get_mut(&key).expect("key 已插入"))
    }
}

struct WalkContext<'a> {
    source: &'a [u8],
    spec: &'static LangSpec,
    result: FileExtractResult,
    class_stack: Vec<String>,
    /// 当前所在的函数/方法链（内层优先），调用点归属用。
    fn_stack: Vec<String>,
}

fn walk(node: Node, depth: usize, ctx: &mut WalkContext) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let kind = node.kind();
    // 透明包装（export / @decorator / template 等）：下穿到内部声明。
    if ctx.spec.transparent_types.contains(&kind) {
        recurse_children(node, depth, ctx);
        return;
    }

    // 调用点：记录后继续下穿（嵌套调用 f(g(x)) 都要命中）。
    if let Some((_, strategy)) = ctx.spec.call_types.iter().find(|(t, _)| *t == kind) {
        if let Some(mut site) = extract_call_site(node, *strategy, ctx.source) {
            site.owner = ctx.fn_stack.last().map(|fn_name| OwnerFunction {
                class_chain: ctx.class_stack.clone(),
                fn_name: fn_name.clone(),
            });
            ctx.result.calls.push(site);
        }
    }

    // 导入。
    if ctx.spec.import_types.contains(&kind) {
        let text = node.utf8_text(ctx.source).unwrap_or_default();
        let module = clean_import_text(text);
        if !module.is_empty() {
            ctx.result.imports.push(ImportRef { module });
        }
    }

    // 字段定义。
    if ctx.spec.field_types.contains(&kind) {
        if let Some(name) = child_name_text(node, ctx.source) {
            ctx.result.defs.push(SymbolDef {
                label: NodeLabel::Field,
                name,
                scope: ctx.class_stack.clone(),
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
        }
        // 初始化器里的调用点由下方通用递归覆盖，此处不 return。
    }

    // 类型声明节点上的继承关系字段。
    if is_type_decl_kind(kind, ctx.spec) {
        collect_heritage(node, ctx.source, ctx.spec, &mut ctx.result.type_refs);
    }

    // 定义节点。
    if let Some(&(node_kind, def_kind)) = ctx.spec.def_types.iter().find(|(t, _)| *t == kind) {
        if let Some(def) = extract_def(node, node_kind, def_kind, ctx) {
            ctx.result.defs.push(def.clone());
            let container = is_container_def(def_kind, kind, ctx.spec);
            let is_fn_def = matches!(def.label, NodeLabel::Function | NodeLabel::Method);
            let class_len_before = ctx.class_stack.len();
            if container {
                ctx.class_stack.push(def.name.clone());
            }
            if is_fn_def {
                ctx.fn_stack.push(def.name.clone());
            }
            recurse_children(node, depth, ctx);
            if is_fn_def {
                ctx.fn_stack.pop();
            }
            if container {
                ctx.class_stack.truncate(class_len_before);
            }
            return;
        }
    }

    // class 上下文但不是定义的节点（rust impl_item、cpp namespace_definition）：
    // 取上下文名入栈后递归。
    if ctx.spec.class_context_types.contains(&kind) {
        let context_name = context_name_of(node, ctx.source);
        let scope_len_before = ctx.class_stack.len();
        if let Some(name) = context_name {
            ctx.class_stack.push(name);
        }
        recurse_children(node, depth, ctx);
        ctx.class_stack.truncate(scope_len_before);
        return;
    }

    recurse_children(node, depth, ctx);
}

fn recurse_children(node: Node, depth: usize, ctx: &mut WalkContext) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    drop(cursor);
    for child in children {
        walk(child, depth + 1, ctx);
    }
}

fn is_container_def(def_kind: DefKind, kind: &str, spec: &LangSpec) -> bool {
    matches!(
        def_kind,
        DefKind::Class | DefKind::Struct | DefKind::Interface | DefKind::Trait | DefKind::Enum
    ) || spec.class_context_types.contains(&kind) && !matches!(def_kind, DefKind::Function)
}

fn def_to_label(def_kind: DefKind) -> NodeLabel {
    match def_kind {
        DefKind::Function => NodeLabel::Function,
        DefKind::Class => NodeLabel::Class,
        DefKind::Struct => NodeLabel::Struct,
        DefKind::Interface => NodeLabel::Interface,
        DefKind::Enum => NodeLabel::Enum,
        DefKind::Trait => NodeLabel::Trait,
        DefKind::Type => NodeLabel::Type,
    }
}

/// 该节点类型是否为「类型定义」（继承关系字段的宿主）。
fn is_type_decl_kind(kind: &str, spec: &LangSpec) -> bool {
    spec.def_types.iter().any(|(t, dk)| {
        *t == kind
            && matches!(
                dk,
                DefKind::Class | DefKind::Struct | DefKind::Interface | DefKind::Trait
            )
    })
}

/// 提取定义符号；匿名定义返回 None。
fn extract_def(
    node: Node,
    node_kind: &'static str,
    def_kind: DefKind,
    ctx: &WalkContext,
) -> Option<SymbolDef> {
    let source = ctx.source;

    // —— 语言特判 ——

    // Go 方法：名字 = receiver 类型 + "." + 方法名，恒为 Method。
    if node_kind == "method_declaration" {
        let receiver = node
            .child_by_field_name("receiver")
            .and_then(|r| {
                // 优先类型名（*Client），避免命中参数模式里的变量名（c）。
                first_descendant_of_kinds(r, &["type_identifier"])
                    .or_else(|| first_descendant_of_kinds(r, &["generic_type"]))
                    .or_else(|| first_descendant_of_kinds(r, &["identifier"]))
            })
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string());
        let method = clean_identifier(node.child_by_field_name("name")?.utf8_text(source).ok()?);
        let name = match receiver {
            Some(r) if !r.is_empty() => format!("{r}.{method}"),
            _ => method,
        };
        return Some(SymbolDef {
            label: NodeLabel::Method,
            name,
            scope: ctx.class_stack.clone(),
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
    }

    // Go type_spec：按子节点区分 struct/interface/别名。
    if node_kind == "type_spec" {
        let name = clean_identifier(node.child_by_field_name("name")?.utf8_text(source).ok()?);
        let label = match node.child_by_field_name("type")?.kind() {
            "struct_type" => NodeLabel::Struct,
            "interface_type" => NodeLabel::Interface,
            _ => NodeLabel::Type,
        };
        return Some(SymbolDef {
            label,
            name,
            scope: ctx.class_stack.clone(),
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
    }

    // JS/TS 的 const foo = () => {}：仅当 value 是函数形态才算定义。
    if node_kind == "variable_declarator" {
        let value = node.child_by_field_name("value")?;
        if !matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "function"
        ) {
            return None;
        }
    }

    // C/C++：函数名藏在 declarator 链里（declarator_dig_types 门控，避免与
    // python 同名的 function_definition 混淆）；typedef 取最右侧声明名。
    // 匿名 struct/union/enum/class（无 name 字段）直接跳过，防止兜底逻辑误吞字段名。
    let name = if ctx.spec.declarator_dig_types.contains(&node_kind) {
        let declarator = node.child_by_field_name("declarator")?;
        dig_declarator_name(declarator, source)?
    } else if matches!(
        node_kind,
        "struct_specifier" | "union_specifier" | "enum_specifier" | "class_specifier"
    ) {
        let named = node.child_by_field_name("name")?;
        clean_identifier(named.utf8_text(source).ok()?)
    } else {
        child_name_text(node, source)?
    };

    if name.is_empty() {
        return None;
    }

    let mut label = def_to_label(def_kind);
    // class 上下文内的函数定义升级为 Method；方法/构造器节点恒为 Method。
    if label == NodeLabel::Function
        && (!ctx.class_stack.is_empty()
            || matches!(node_kind, "method_declaration" | "constructor_declaration"))
    {
        label = NodeLabel::Method;
    }

    Some(SymbolDef {
        label,
        name,
        scope: ctx.class_stack.clone(),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
    })
}

/// 标准 `name` 字段；缺失时按常见标识符子节点兜底。
fn child_name_text(node: Node, source: &[u8]) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let text = name_node.utf8_text(source).ok()?;
        return Some(clean_identifier(text));
    }
    let fallback_kinds = [
        "identifier",
        "property_identifier",
        "field_identifier",
        "type_identifier",
    ];
    first_descendant_of_kinds(node, &fallback_kinds)
        .map(|n| clean_identifier(n.utf8_text(source).unwrap_or_default()))
}

/// C/C++ declarator 链挖掘：pointer/array/function declarator 层层包裹，
/// 标识符在最内层（`*foo(...)` 的函数名是 foo；typedef 的终点是
/// type_identifier）。
fn dig_declarator_name(mut node: Node, source: &[u8]) -> Option<String> {
    for _ in 0..16 {
        if matches!(
            node.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node.utf8_text(source).ok()?.to_string());
        }
        if let Some(next) = node
            .child_by_field_name("declarator")
            .or_else(|| node.child_by_field_name("name"))
        {
            node = next;
            continue;
        }
        let wrappers = [
            "function_declarator",
            "pointer_declarator",
            "array_declarator",
            "parenthesized_declarator",
            "reference_declarator",
            "identifier",
            "destructor_name",
            "operator_name",
        ];
        match first_descendant_of_kinds(node, &wrappers) {
            Some(inner) if inner.id() != node.id() => node = inner,
            _ => return None,
        }
    }
    None
}

/// 在子树中找第一个指定 kind 的节点（广度优先并限量，避免扎进超深分支）。
fn first_descendant_of_kinds<'tree>(node: Node<'tree>, kinds: &[&str]) -> Option<Node<'tree>> {
    let mut queue = VecDeque::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        queue.push_back(child);
    }
    drop(cursor);
    let mut visited = 0usize;
    while let Some(current) = queue.pop_front() {
        visited += 1;
        if visited > 512 {
            return None;
        }
        if kinds.contains(&current.kind()) {
            return Some(current);
        }
        let mut child_cursor = current.walk();
        for child in current.children(&mut child_cursor) {
            queue.push_back(child);
        }
    }
    None
}

/// class 上下文名：rust impl_item 取 `type` 字段，其余走标准 name 字段兜底。
fn context_name_of(node: Node, source: &[u8]) -> Option<String> {
    if let Some(t) = node.child_by_field_name("type") {
        let text = t.utf8_text(source).unwrap_or_default();
        let cleaned = clean_identifier(text);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    let name = child_name_text(node, source)?;
    (!name.is_empty()).then_some(name)
}

fn extract_call_site(node: Node, strategy: CallNameStrategy, source: &[u8]) -> Option<CallSite> {
    let callee_display = match strategy {
        CallNameStrategy::Field(field) => {
            let target = node.child_by_field_name(field)?;
            let mut text = target.utf8_text(source).ok()?.to_string();
            // java 对象创建：type 字段只有类型名，统一剥掉 new 前缀。
            if let Some(stripped) = text.strip_prefix("new ") {
                text = stripped.trim().to_string();
            }
            text
        }
        CallNameStrategy::FirstIdentifier => {
            // kotlin-ng 无字段标注：跳过参数列表找第一个标识符。
            let ident = first_call_identifier(node)?;
            ident.utf8_text(source).ok()?.to_string()
        }
    };
    let name = callee_display
        .split(['.', ':', '>', ' '])
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or_default()
        .to_string();
    if callee_display.is_empty() || name.is_empty() {
        return None;
    }
    Some(CallSite {
        callee_display,
        name,
        line: node.start_position().row as u32 + 1,
        owner: None,
    })
}

fn first_call_identifier(node: Node) -> Option<Node> {
    let mut queue = VecDeque::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "value_arguments" | "annotated_lambda" | "lambda_literal"
        ) {
            continue;
        }
        queue.push_back(child);
    }
    drop(cursor);
    let mut visited = 0usize;
    while let Some(current) = queue.pop_front() {
        visited += 1;
        if visited > 64 {
            return None;
        }
        if current.kind() == "identifier" {
            return Some(current);
        }
        let mut child_cursor = current.walk();
        for child in current.children(&mut child_cursor) {
            if matches!(
                child.kind(),
                "value_arguments" | "annotated_lambda" | "lambda_literal"
            ) {
                continue;
            }
            queue.push_back(child);
        }
    }
    None
}

/// 在子树中按类型查找节点（限深，防误入参数列表等无关分支）。
fn find_descendant_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
    max_depth: usize,
) -> Option<Node<'tree>> {
    if max_depth == 0 {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = find_descendant_kind(child, kind, max_depth - 1) {
            return Some(found);
        }
    }
    None
}

/// 收集继承关系引用。查找策略双轨：优先按字段名命中（java superclass），
/// 失败则按同名子节点类型命中（python argument_list / ts extends_type_clause）。
fn collect_heritage(node: Node, source: &[u8], spec: &LangSpec, out: &mut Vec<TypeRef>) {
    for entry in spec.heritage {
        let field = match entry {
            HeritageKind::FieldInherit(f) | HeritageKind::FieldImplement(f) => f,
        };
        // 双轨查找：字段名优先（java superclass）；失败按子节点类型找，
        // 允许隔着包装层（TS 的 implements_clause 在 class_heritage 内）。
        let host = node
            .child_by_field_name(field)
            .or_else(|| find_descendant_kind(node, field, 3));
        let Some(host) = host else { continue };
        let inherits = matches!(entry, HeritageKind::FieldInherit(_));
        collect_type_identifiers(host, source, inherits, out);
    }
}

fn collect_type_identifiers(node: Node, source: &[u8], inherits: bool, out: &mut Vec<TypeRef>) {
    const LEAF_KINDS: [&str; 3] = ["type_identifier", "identifier", "type_ref"];
    if LEAF_KINDS.contains(&node.kind()) {
        let name = clean_identifier(node.utf8_text(source).unwrap_or_default());
        if !name.is_empty() && !is_primitive_type_name(&name) {
            out.push(TypeRef { name, inherits });
        }
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    drop(cursor);
    for child in children {
        collect_type_identifiers(child, source, inherits, out);
    }
}

fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "String"
            | "Vec"
            | "Option"
            | "Result"
            | "int"
            | "float"
            | "double"
            | "long"
            | "short"
            | "byte"
            | "void"
            | "object"
            | "string"
            | "dynamic"
            | "Any"
            | "Self"
    )
}

/// 导入文本清洗成模块路径串（Module 节点名，仅用于展示与 IMPORTS 边）。
fn clean_import_text(raw: &str) -> String {
    let mut text = raw.trim().trim_end_matches(';').trim().to_string();
    for prefix in [
        "use ",
        "import ",
        "from ",
        "#include ",
        "using ",
        "namespace ",
    ] {
        if let Some(stripped) = text.strip_prefix(prefix) {
            text = stripped.trim().to_string();
            break;
        }
    }
    // java: import static x.y.Z.method
    if let Some(stripped) = text.strip_prefix("static ") {
        text = stripped.trim().to_string();
    }
    // go import_spec / c include 的引号与尖括号
    text = text
        .trim_start_matches(['"', '<'])
        .trim_end_matches(['"', '>'])
        .trim()
        .to_string();
    // 统一分隔符为 '.'
    text = text.replace("::", ".").replace('\\', ".");
    // python 多名字导入取首个；from X import y 只留来源 X
    if let Some(first) = text.split(',').next() {
        text = first.trim().to_string();
    }
    if let Some(pos) = text.find(" import ") {
        text = text[..pos].trim().to_string();
    }
    text.trim_matches('.').trim().to_string()
}

/// 标识符清洗：取限定表达式的末段（`std::vec::Vec` → Vec）。
fn clean_identifier(text: &str) -> String {
    let trimmed = text.split('<').next().unwrap_or(text);
    trimmed
        .split(['.', ':', '>'])
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}
