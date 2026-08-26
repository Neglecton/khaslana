//! 每语言一张节点类型规格表 + 通用 AST walk（参照 codebase-memory-mcp 的
//! `lang_specs.c` 思路：不用 .scm query，用节点类型字符串匹配，跨语言零查询文件维护成本）。
//!
//! 表内容依据各语法 crate 的 node-types.json 核对：
//! - 定义类节点统一取标准 `name` 字段；C/C++ 的 function_definition 名字藏在
//!   `declarator` 字段里，走专门的挖掘函数；
//! - JS/TS 的 export_statement、Python 的 decorated_definition 是「透明包装」，
//!   walk 时直接下穿到内部声明。

use std::collections::HashMap;

use tree_sitter::Language;

use super::LangId;

/// 调用点 callee 名字的提取方式（各语法的 call 节点字段不统一）。
#[derive(Clone, Copy, Debug)]
pub enum CallNameStrategy {
    /// 取指定 field 的文本（如 rust/go/js/ts/c/cpp/c# 的 `function`、java/php 的 `name`）。
    Field(&'static str),
    /// 无字段标注的语法（Kotlin ng）：取第一个 identifier 后代。
    FirstIdentifier,
}

/// 类型继承关系的来源描述（用于 INHERITS / IMPLEMENTS 边）。
#[derive(Clone, Copy, Debug)]
pub enum HeritageKind {
    /// 单一父类型字段（java superclass、python 第一个基类）→ INHERITS。
    FieldInherit(&'static str),
    /// 接口列表字段（java interfaces）→ IMPLEMENTS。
    FieldImplement(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct LangSpec {
    pub language: fn() -> Language,
    pub extensions: &'static [&'static str],
    /// 定义 → (节点类型, 标签)。Method 由运行时上下文判定（在 class 上下文内的
    /// Function 类定义自动升级为 Method），表里只写基础标签。
    pub def_types: &'static [(&'static str, DefKind)],
    /// 进入后视为 class 上下文的节点类型（其内 Function 定义升级为 Method，
    /// FQN 追加类型名）。上下文名取该节点的 `name` 字段或 [`DefKind`] 特判。
    pub class_context_types: &'static [&'static str],
    /// walk 时直接下穿的透明包装（export/decorated/template 等）。
    pub transparent_types: &'static [&'static str],
    /// 字段定义节点类型（struct/class body 内），标签 Field。
    pub field_types: &'static [&'static str],
    /// 调用点节点类型。
    pub call_types: &'static [(&'static str, CallNameStrategy)],
    /// 导入节点类型：取整节点文本清洗后作为模块路径。
    pub import_types: &'static [&'static str],
    /// 需要走 declarator 链挖掘函数名的定义节点类型（仅 C/C++：
    /// 其 function_definition/type_definition 的名字藏在 declarator 字段里；
    /// 注意 python 的 function_definition 与 C 同名但走标准 name 字段）。
    pub declarator_dig_types: &'static [&'static str],
    /// 继承关系来源。
    pub heritage: &'static [HeritageKind],
}

/// 定义的基础标签（Method 在 walk 时按上下文动态升级）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Function,
    Class,
    Struct,
    Interface,
    Enum,
    Trait,
    Type,
}

impl LangSpec {
    /// 按扩展名识别语言（小写比较）。
    pub fn detect(extension: &str) -> Option<LangId> {
        let ext = extension.to_ascii_lowercase();
        LANG_SPECS
            .iter()
            .find(|(_, spec)| spec.extensions.iter().any(|e| *e == ext))
            .map(|(id, _)| *id)
    }

    pub fn for_id(id: LangId) -> &'static LangSpec {
        &LANG_SPECS
            .iter()
            .find(|(lid, _)| *lid == id)
            .expect("LangId 必有规格表")
            .1
    }
}

macro_rules! language_fn {
    ($crate_name:ident, $const_name:ident) => {
        || ::tree_sitter::Language::from($crate_name::$const_name)
    };
}

pub static LANG_SPECS: &[(LangId, LangSpec)] = &[
    (
        LangId::Rust,
        LangSpec {
            language: language_fn!(tree_sitter_rust, LANGUAGE),
            extensions: &["rs"],
            // macro_definition 归 Macro 之外的 Function 类不合适，v1 不提取宏定义。
            def_types: &[
                ("function_item", DefKind::Function),
                ("struct_item", DefKind::Struct),
                ("enum_item", DefKind::Enum),
                ("trait_item", DefKind::Trait),
                ("union_item", DefKind::Struct),
                ("type_item", DefKind::Type),
            ],
            // impl_item / trait_item 内的 function_item 视为方法，上下文名取 type/trait 字段。
            class_context_types: &["impl_item", "trait_item"],
            transparent_types: &[],
            field_types: &["field_declaration"],
            call_types: &[("call_expression", CallNameStrategy::Field("function"))],
            import_types: &["use_declaration"],
            heritage: &[],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Python,
        LangSpec {
            language: language_fn!(tree_sitter_python, LANGUAGE),
            extensions: &["py", "pyi"],
            def_types: &[
                ("function_definition", DefKind::Function),
                ("class_definition", DefKind::Class),
            ],
            class_context_types: &["class_definition"],
            // @decorator 包装的函数在下穿后正常命中。
            transparent_types: &["decorated_definition"],
            field_types: &[],
            call_types: &[("call", CallNameStrategy::Field("function"))],
            import_types: &["import_statement", "import_from_statement"],
            // class A(B, C)：argument_list 里第一个标识符视为父类。
            heritage: &[HeritageKind::FieldInherit("argument_list")],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::JavaScript,
        LangSpec {
            language: language_fn!(tree_sitter_javascript, LANGUAGE),
            extensions: &["js", "jsx", "mjs", "cjs"],
            def_types: &[
                ("function_declaration", DefKind::Function),
                ("generator_function_declaration", DefKind::Function),
                ("class_declaration", DefKind::Class),
                ("method_definition", DefKind::Function),
                // const foo = () => {} / const bar = function() {}
                ("variable_declarator", DefKind::Function),
            ],
            class_context_types: &["class_declaration", "class"],
            transparent_types: &["export_statement"],
            field_types: &["property_signature"],
            call_types: &[
                ("call_expression", CallNameStrategy::Field("function")),
                ("new_expression", CallNameStrategy::Field("constructor")),
            ],
            import_types: &["import_statement"],
            heritage: &[HeritageKind::FieldInherit("class_heritage")],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::TypeScript,
        LangSpec {
            language: language_fn!(tree_sitter_typescript, LANGUAGE_TYPESCRIPT),
            extensions: &["ts", "mts", "cts"],
            def_types: &[
                ("function_declaration", DefKind::Function),
                ("generator_function_declaration", DefKind::Function),
                ("class_declaration", DefKind::Class),
                ("abstract_class_declaration", DefKind::Class),
                ("method_definition", DefKind::Function),
                ("interface_declaration", DefKind::Interface),
                ("type_alias_declaration", DefKind::Type),
                ("enum_declaration", DefKind::Enum),
                ("variable_declarator", DefKind::Function),
            ],
            class_context_types: &["class_declaration", "abstract_class_declaration", "class"],
            transparent_types: &["export_statement"],
            field_types: &["property_signature"],
            call_types: &[
                ("call_expression", CallNameStrategy::Field("function")),
                ("new_expression", CallNameStrategy::Field("constructor")),
            ],
            import_types: &["import_statement"],
            heritage: &[
                HeritageKind::FieldInherit("extends_clause"),
                HeritageKind::FieldImplement("implements_clause"),
            ],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Tsx,
        LangSpec {
            language: language_fn!(tree_sitter_typescript, LANGUAGE_TSX),
            extensions: &["tsx"],
            def_types: &[
                ("function_declaration", DefKind::Function),
                ("generator_function_declaration", DefKind::Function),
                ("class_declaration", DefKind::Class),
                ("abstract_class_declaration", DefKind::Class),
                ("method_definition", DefKind::Function),
                ("interface_declaration", DefKind::Interface),
                ("type_alias_declaration", DefKind::Type),
                ("enum_declaration", DefKind::Enum),
                ("variable_declarator", DefKind::Function),
            ],
            class_context_types: &["class_declaration", "abstract_class_declaration", "class"],
            transparent_types: &["export_statement"],
            field_types: &["property_signature"],
            call_types: &[
                ("call_expression", CallNameStrategy::Field("function")),
                ("new_expression", CallNameStrategy::Field("constructor")),
            ],
            import_types: &["import_statement"],
            heritage: &[
                HeritageKind::FieldInherit("extends_clause"),
                HeritageKind::FieldImplement("implements_clause"),
            ],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Go,
        LangSpec {
            language: language_fn!(tree_sitter_go, LANGUAGE),
            extensions: &["go"],
            def_types: &[
                ("function_declaration", DefKind::Function),
                // 方法名 = receiver 类型 + "." + name，在提取层特判拼接。
                ("method_declaration", DefKind::Function),
                // type X struct{...} / interface{...} / 其他别名
                ("type_spec", DefKind::Type),
            ],
            class_context_types: &[],
            transparent_types: &[],
            field_types: &["field_declaration"],
            call_types: &[("call_expression", CallNameStrategy::Field("function"))],
            import_types: &["import_spec"],
            heritage: &[],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Java,
        LangSpec {
            language: language_fn!(tree_sitter_java, LANGUAGE),
            extensions: &["java"],
            def_types: &[
                ("class_declaration", DefKind::Class),
                ("interface_declaration", DefKind::Interface),
                ("enum_declaration", DefKind::Enum),
                ("record_declaration", DefKind::Class),
                ("method_declaration", DefKind::Function),
                ("constructor_declaration", DefKind::Function),
            ],
            class_context_types: &[
                "class_declaration",
                "interface_declaration",
                "enum_declaration",
                "record_declaration",
            ],
            transparent_types: &[],
            field_types: &["field_declaration"],
            call_types: &[
                ("method_invocation", CallNameStrategy::Field("name")),
                (
                    "object_creation_expression",
                    CallNameStrategy::Field("type"),
                ),
            ],
            import_types: &["import_declaration"],
            heritage: &[
                HeritageKind::FieldInherit("superclass"),
                HeritageKind::FieldImplement("interfaces"),
            ],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::C,
        LangSpec {
            language: language_fn!(tree_sitter_c, LANGUAGE),
            extensions: &["c", "h"],
            def_types: &[
                ("function_definition", DefKind::Function),
                ("struct_specifier", DefKind::Struct),
                ("union_specifier", DefKind::Struct),
                ("enum_specifier", DefKind::Enum),
                ("type_definition", DefKind::Type),
            ],
            class_context_types: &[],
            transparent_types: &[],
            field_types: &["field_declaration"],
            call_types: &[("call_expression", CallNameStrategy::Field("function"))],
            import_types: &["preproc_include"],
            heritage: &[],
            declarator_dig_types: &["function_definition", "type_definition"],
        },
    ),
    (
        LangId::Cpp,
        LangSpec {
            language: language_fn!(tree_sitter_cpp, LANGUAGE),
            extensions: &["cc", "cpp", "cxx", "hh", "hpp", "hxx"],
            def_types: &[
                ("function_definition", DefKind::Function),
                ("class_specifier", DefKind::Class),
                ("struct_specifier", DefKind::Struct),
                ("enum_specifier", DefKind::Enum),
                ("type_definition", DefKind::Type),
            ],
            // namespace 进入后同样视为 class 上下文（FQN 前缀 + 方法判定）。
            class_context_types: &[
                "class_specifier",
                "struct_specifier",
                "namespace_definition",
            ],
            transparent_types: &["template_declaration", "linkage_specification"],
            field_types: &["field_declaration"],
            call_types: &[("call_expression", CallNameStrategy::Field("function"))],
            import_types: &["preproc_include"],
            heritage: &[HeritageKind::FieldInherit("base_class_clause")],
            declarator_dig_types: &["function_definition", "type_definition"],
        },
    ),
    (
        LangId::CSharp,
        LangSpec {
            language: language_fn!(tree_sitter_c_sharp, LANGUAGE),
            extensions: &["cs"],
            def_types: &[
                ("class_declaration", DefKind::Class),
                ("interface_declaration", DefKind::Interface),
                ("struct_declaration", DefKind::Struct),
                ("enum_declaration", DefKind::Enum),
                ("record_declaration", DefKind::Class),
                ("method_declaration", DefKind::Function),
                ("constructor_declaration", DefKind::Function),
            ],
            class_context_types: &[
                "class_declaration",
                "interface_declaration",
                "struct_declaration",
                "record_declaration",
            ],
            transparent_types: &[],
            field_types: &[],
            call_types: &[
                ("invocation_expression", CallNameStrategy::Field("function")),
                (
                    "object_creation_expression",
                    CallNameStrategy::Field("type"),
                ),
            ],
            import_types: &["using_directive"],
            // base_list 首个类型视为父类，其余接口——启发式，无法静态区分。
            heritage: &[HeritageKind::FieldInherit("base_list")],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Php,
        LangSpec {
            language: language_fn!(tree_sitter_php, LANGUAGE_PHP),
            extensions: &["php"],
            def_types: &[
                ("function_definition", DefKind::Function),
                ("method_declaration", DefKind::Function),
                ("class_declaration", DefKind::Class),
                ("interface_declaration", DefKind::Interface),
                ("trait_declaration", DefKind::Trait),
                ("enum_declaration", DefKind::Enum),
            ],
            class_context_types: &[
                "class_declaration",
                "interface_declaration",
                "trait_declaration",
                "enum_declaration",
            ],
            transparent_types: &[],
            field_types: &["property_declaration"],
            call_types: &[
                (
                    "function_call_expression",
                    CallNameStrategy::Field("function"),
                ),
                ("member_call_expression", CallNameStrategy::Field("name")),
                ("scoped_call_expression", CallNameStrategy::Field("name")),
            ],
            import_types: &["namespace_use_clause"],
            heritage: &[
                HeritageKind::FieldInherit("base_clause"),
                HeritageKind::FieldImplement("class_interface_clause"),
            ],
            declarator_dig_types: &[],
        },
    ),
    (
        LangId::Kotlin,
        LangSpec {
            language: language_fn!(tree_sitter_kotlin_ng, LANGUAGE),
            extensions: &["kt", "kts"],
            def_types: &[
                ("function_declaration", DefKind::Function),
                ("class_declaration", DefKind::Class),
                ("object_declaration", DefKind::Class),
            ],
            class_context_types: &["class_declaration", "object_declaration"],
            transparent_types: &[],
            field_types: &["property_declaration"],
            call_types: &[("call_expression", CallNameStrategy::FirstIdentifier)],
            // kotlin-ng 的 import 节点无名字段，导入边留空由调用解析兜底。
            import_types: &[],
            heritage: &[HeritageKind::FieldInherit("delegation_specifier")],
            declarator_dig_types: &[],
        },
    ),
];

/// 语言工厂缓存：`set_language` 需要 `&Language`，Parser 复用时按语言缓存。
pub(crate) fn language_of(id: LangId) -> Language {
    static CACHE: std::sync::OnceLock<HashMap<LangId, Language>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        LANG_SPECS
            .iter()
            .map(|(id, spec)| (*id, (spec.language)()))
            .collect()
    });
    cache[&id].clone()
}
