use std::collections::BTreeMap;
use tree_sitter::Node;

use super::types::{
    JavaFileFacts, JavaImportFact, JavaParameterFact, JavaSymbolFact, JavaVariableFact,
};

const TYPE_KINDS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "annotation_type_declaration",
];

/// 从已经由通用提取器生成的同一棵 Java AST 中提取语义事实。
pub fn extract_java(root: Node<'_>, source: &[u8]) -> JavaFileFacts {
    let package = find_first_kind(root, "package_declaration")
        .map(|node| package_text(node, source))
        .unwrap_or_default();
    let mut imports = Vec::new();
    collect_kind(root, "import_declaration", &mut |node| {
        if let Some(import) = import_fact(node, source) {
            imports.push(import);
        }
    });
    let declared_types = declared_type_names(root, source, &package);
    let resolver = TypeNameResolver {
        package: &package,
        imports: &imports,
        declared_types: &declared_types,
    };
    let mut facts = JavaFileFacts {
        package: package.clone(),
        imports: imports.clone(),
        ..Default::default()
    };
    let mut context = ExtractContext {
        source,
        resolver,
        type_chain: Vec::new(),
        callable_chain: Vec::new(),
        facts: &mut facts,
    };
    walk(root, &mut context);
    facts
}

struct ExtractContext<'a, 'facts> {
    source: &'a [u8],
    resolver: TypeNameResolver<'a>,
    type_chain: Vec<String>,
    callable_chain: Vec<String>,
    facts: &'facts mut JavaFileFacts,
}

fn walk(node: Node<'_>, context: &mut ExtractContext<'_, '_>) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        let Some(name_node) = node.child_by_field_name("name") else {
            recurse(node, context);
            return;
        };
        let name = text(name_node, context.source).to_string();
        let mut owner_chain = context.type_chain.clone();
        if let Some(callable) = context.callable_chain.last() {
            owner_chain.push(callable.clone());
        }
        let stability = if context.callable_chain.is_empty() {
            "stable"
        } else {
            "local"
        };
        let extends = type_list_from_field(node, "superclass", &context.resolver, context.source)
            .into_iter()
            .chain(type_list_from_field(
                node,
                "extends_interfaces",
                &context.resolver,
                context.source,
            ))
            .collect();
        let implements =
            type_list_from_field(node, "interfaces", &context.resolver, context.source);
        let permits = type_list_from_field(node, "permits", &context.resolver, context.source);
        context.facts.symbols.push(JavaSymbolFact {
            kind: match kind {
                "class_declaration" => "class",
                "interface_declaration" => "interface",
                "enum_declaration" => "enum",
                "record_declaration" => "record",
                _ => "annotation",
            }
            .into(),
            name: name.clone(),
            owner_chain: owner_chain.clone(),
            signature: String::new(),
            parameters: Vec::new(),
            return_type: None,
            raw_return_type: None,
            declared_type: None,
            raw_declared_type: None,
            modifiers: modifiers(node, context.source),
            annotations: annotations(node, context.source),
            javadoc: javadoc(node, context.source),
            extends,
            implements,
            permits,
            start_byte: node.start_byte() as u32,
            end_byte: node.end_byte() as u32,
            start_line: line_start(node),
            end_line: line_end(node),
            stability: stability.into(),
        });

        if kind == "record_declaration"
            && let Some(parameters_node) = node.child_by_field_name("parameters")
        {
            for component in parameters(parameters_node, &context.resolver, context.source) {
                let component_owner: Vec<String> = owner_chain
                    .iter()
                    .cloned()
                    .chain(std::iter::once(name.clone()))
                    .collect();
                context.facts.variables.push(JavaVariableFact {
                    name: component.name.clone(),
                    type_name: component.type_name.clone(),
                    raw_type: component.raw_type.clone(),
                    kind: "record_component".into(),
                    owner_chain: component_owner.clone(),
                    scope_start_byte: node.start_byte() as u32,
                    scope_end_byte: node.end_byte() as u32,
                    start_byte: component.start_byte,
                    end_byte: component.end_byte,
                });
                context.facts.symbols.push(JavaSymbolFact {
                    kind: "record_accessor".into(),
                    name: component.name,
                    owner_chain: component_owner,
                    signature: "()".into(),
                    parameters: Vec::new(),
                    return_type: Some(component.type_name),
                    raw_return_type: Some(component.raw_type),
                    declared_type: None,
                    raw_declared_type: None,
                    modifiers: vec!["public".into()],
                    annotations: Vec::new(),
                    javadoc: None,
                    extends: Vec::new(),
                    implements: Vec::new(),
                    permits: Vec::new(),
                    start_byte: component.start_byte,
                    end_byte: component.end_byte,
                    start_line: line_start(parameters_node),
                    end_line: line_end(parameters_node),
                    stability: stability.into(),
                });
            }
        }

        let old_types = std::mem::replace(&mut context.type_chain, owner_chain);
        context.type_chain.push(name);
        recurse(node, context);
        context.type_chain = old_types;
        return;
    }

    if matches!(kind, "method_declaration" | "constructor_declaration") {
        if let Some(symbol) = callable_symbol(node, context) {
            let callable_name = symbol.name.clone();
            let callable_identity = format!("{callable_name}{}", symbol.signature);
            let body_scope = node
                .child_by_field_name("body")
                .map(|body| (body.start_byte() as u32, body.end_byte() as u32))
                .unwrap_or((node.start_byte() as u32, node.end_byte() as u32));
            for parameter in &symbol.parameters {
                context.facts.variables.push(JavaVariableFact {
                    name: parameter.name.clone(),
                    type_name: parameter.type_name.clone(),
                    raw_type: parameter.raw_type.clone(),
                    kind: "parameter".into(),
                    owner_chain: context
                        .type_chain
                        .iter()
                        .cloned()
                        .chain(std::iter::once(callable_identity.clone()))
                        .collect(),
                    scope_start_byte: body_scope.0,
                    scope_end_byte: body_scope.1,
                    start_byte: parameter.start_byte,
                    end_byte: parameter.end_byte,
                });
            }
            context.facts.symbols.push(symbol);
            context.callable_chain.push(callable_identity);
            recurse(node, context);
            context.callable_chain.pop();
            return;
        }
    }

    if kind == "field_declaration" {
        extract_fields(node, context);
    } else if kind == "local_variable_declaration" {
        extract_local_variables(node, context, "local");
    } else if kind == "enhanced_for_statement" {
        extract_single_scoped_variable(node, context, "enhanced_for");
    } else if kind == "catch_formal_parameter" {
        extract_single_scoped_variable(node, context, "catch");
    }
    recurse(node, context);
}

fn recurse(node: Node<'_>, context: &mut ExtractContext<'_, '_>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    drop(cursor);
    for child in children {
        walk(child, context);
    }
}

fn callable_symbol(node: Node<'_>, context: &ExtractContext<'_, '_>) -> Option<JavaSymbolFact> {
    let constructor = node.kind() == "constructor_declaration";
    let source = context.source;
    let declared_name = text(node.child_by_field_name("name")?, source).to_string();
    let parameters_node = node.child_by_field_name("parameters")?;
    let parameters = parameters(parameters_node, &context.resolver, source);
    let signature = format!(
        "({})",
        parameters
            .iter()
            .map(|parameter| {
                if parameter.varargs {
                    format!("{}...", parameter.type_name.trim_end_matches("..."))
                } else {
                    parameter.type_name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    let (return_type, raw_return_type) = if constructor {
        (None, None)
    } else {
        node.child_by_field_name("type")
            .map(|kind| {
                let raw = text(kind, source).trim().to_string();
                (Some(context.resolver.resolve(&raw)), Some(raw))
            })
            .unwrap_or_else(|| (Some("void".into()), Some("void".into())))
    };
    Some(JavaSymbolFact {
        kind: if constructor { "constructor" } else { "method" }.into(),
        name: if constructor {
            "<init>".into()
        } else {
            declared_name
        },
        owner_chain: context.type_chain.clone(),
        signature,
        parameters,
        return_type,
        raw_return_type,
        declared_type: None,
        raw_declared_type: None,
        modifiers: modifiers(node, source),
        annotations: annotations(node, source),
        javadoc: javadoc(node, source),
        extends: Vec::new(),
        implements: Vec::new(),
        permits: Vec::new(),
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
        start_line: line_start(node),
        end_line: line_end(node),
        stability: if context.callable_chain.is_empty() {
            "stable".into()
        } else {
            "local".into()
        },
    })
}

fn parameters(
    node: Node<'_>,
    resolver: &TypeNameResolver<'_>,
    source: &[u8],
) -> Vec<JavaParameterFact> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for parameter in node.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "formal_parameter" | "spread_parameter" | "receiver_parameter"
        ) {
            continue;
        }
        let type_node = parameter.child_by_field_name("type").or_else(|| {
            let mut child_cursor = parameter.walk();
            parameter.named_children(&mut child_cursor).find(|child| {
                !matches!(
                    child.kind(),
                    "modifiers" | "annotation" | "marker_annotation" | "variable_declarator"
                )
            })
        });
        let name_node = parameter.child_by_field_name("name").or_else(|| {
            let declarator = {
                let mut child_cursor = parameter.walk();
                parameter
                    .named_children(&mut child_cursor)
                    .find(|child| child.kind() == "variable_declarator")
            }?;
            declarator.child_by_field_name("name")
        });
        let (Some(type_node), Some(name_node)) = (type_node, name_node) else {
            continue;
        };
        let raw_type = text(type_node, source).trim().to_string();
        let varargs =
            parameter.kind() == "spread_parameter" || text(parameter, source).contains("...");
        out.push(JavaParameterFact {
            name: text(name_node, source).to_string(),
            type_name: resolver.resolve(&raw_type),
            raw_type,
            varargs,
            start_byte: parameter.start_byte() as u32,
            end_byte: parameter.end_byte() as u32,
        });
    }
    out
}

fn extract_fields(node: Node<'_>, context: &mut ExtractContext<'_, '_>) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let raw_type = text(type_node, context.source).trim().to_string();
    let type_name = context.resolver.resolve(&raw_type);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        context.facts.symbols.push(JavaSymbolFact {
            kind: "field".into(),
            name: text(name_node, context.source).to_string(),
            owner_chain: context.type_chain.clone(),
            signature: String::new(),
            parameters: Vec::new(),
            return_type: None,
            raw_return_type: None,
            declared_type: Some(type_name.clone()),
            raw_declared_type: Some(raw_type.clone()),
            modifiers: modifiers(node, context.source),
            annotations: annotations(node, context.source),
            javadoc: javadoc(node, context.source),
            extends: Vec::new(),
            implements: Vec::new(),
            permits: Vec::new(),
            start_byte: child.start_byte() as u32,
            end_byte: child.end_byte() as u32,
            start_line: line_start(child),
            end_line: line_end(child),
            stability: "stable".into(),
        });
    }
}

fn extract_local_variables(node: Node<'_>, context: &mut ExtractContext<'_, '_>, kind: &str) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let raw_type = text(type_node, context.source).trim().to_string();
    let inferred = raw_type == "var";
    let type_name = if inferred {
        infer_initializer_type(node, &context.resolver, context.source)
            .unwrap_or_else(|| "?var".into())
    } else {
        context.resolver.resolve(&raw_type)
    };
    let (scope_start_byte, scope_end_byte) = lexical_scope(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        context.facts.variables.push(JavaVariableFact {
            name: text(name_node, context.source).to_string(),
            type_name: type_name.clone(),
            raw_type: raw_type.clone(),
            kind: kind.into(),
            owner_chain: context
                .type_chain
                .iter()
                .cloned()
                .chain(context.callable_chain.last().cloned())
                .collect(),
            scope_start_byte,
            scope_end_byte,
            start_byte: child.start_byte() as u32,
            end_byte: child.end_byte() as u32,
        });
    }
}

fn extract_single_scoped_variable(
    node: Node<'_>,
    context: &mut ExtractContext<'_, '_>,
    kind: &str,
) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let raw_type = text(type_node, context.source).trim().to_string();
    let (scope_start_byte, scope_end_byte) = lexical_scope(node);
    context.facts.variables.push(JavaVariableFact {
        name: text(name_node, context.source).to_string(),
        type_name: context.resolver.resolve(&raw_type),
        raw_type,
        kind: kind.into(),
        owner_chain: context
            .type_chain
            .iter()
            .cloned()
            .chain(context.callable_chain.last().cloned())
            .collect(),
        scope_start_byte,
        scope_end_byte,
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
    });
}

fn infer_initializer_type(
    declaration: Node<'_>,
    resolver: &TypeNameResolver<'_>,
    source: &[u8],
) -> Option<String> {
    let mut cursor = declaration.walk();
    for declarator in declaration.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let value = declarator.child_by_field_name("value")?;
        if value.kind() == "object_creation_expression" {
            let ty = value.child_by_field_name("type")?;
            return Some(resolver.resolve(text(ty, source)));
        }
    }
    None
}

fn lexical_scope(node: Node<'_>) -> (u32, u32) {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if matches!(
            parent.kind(),
            "block"
                | "constructor_body"
                | "lambda_expression"
                | "for_statement"
                | "enhanced_for_statement"
        ) {
            return (parent.start_byte() as u32, parent.end_byte() as u32);
        }
        current = parent;
    }
    (node.start_byte() as u32, node.end_byte() as u32)
}

struct TypeNameResolver<'a> {
    package: &'a str,
    imports: &'a [JavaImportFact],
    declared_types: &'a BTreeMap<String, Option<String>>,
}

fn type_list_from_field(
    node: Node<'_>,
    field: &str,
    resolver: &TypeNameResolver<'_>,
    source: &[u8],
) -> Vec<String> {
    let Some(host) = node.child_by_field_name(field) else {
        return Vec::new();
    };
    let raw = text(host, source).trim();
    let raw = ["extends", "implements", "permits"]
        .into_iter()
        .find_map(|keyword| raw.strip_prefix(keyword).map(str::trim))
        .unwrap_or(raw);
    split_top_level(raw, ',')
        .into_iter()
        .filter(|value| !value.is_empty())
        .map(|value| resolver.resolve(value))
        .collect()
}

impl TypeNameResolver<'_> {
    fn resolve(&self, raw: &str) -> String {
        let raw = raw.trim();
        if raw.is_empty() {
            return "?".into();
        }
        if let Some(inner) = raw.strip_suffix("[]") {
            return format!("{}[]", self.resolve(inner));
        }
        if let Some(inner) = raw.strip_suffix("...") {
            return format!("{}...", self.resolve(inner));
        }
        if let Some((base, arguments)) = split_generic(raw) {
            let resolved_arguments = split_top_level(arguments, ',')
                .into_iter()
                .map(|argument| self.resolve_type_argument(argument))
                .collect::<Vec<_>>()
                .join(",");
            return format!("{}<{resolved_arguments}>", self.resolve_simple(base));
        }
        self.resolve_type_argument(raw)
    }

    fn resolve_type_argument(&self, raw: &str) -> String {
        let raw = raw.trim();
        if raw == "?" {
            return raw.into();
        }
        if let Some(rest) = raw.strip_prefix("? extends ") {
            return format!("? extends {}", self.resolve(rest));
        }
        if let Some(rest) = raw.strip_prefix("? super ") {
            return format!("? super {}", self.resolve(rest));
        }
        self.resolve_simple(raw)
    }

    fn resolve_simple(&self, raw: &str) -> String {
        if matches!(
            raw,
            "byte" | "short" | "int" | "long" | "float" | "double" | "boolean" | "char" | "void"
        ) {
            return raw.into();
        }
        if raw.contains('.') {
            let first = raw.split('.').next().unwrap_or(raw);
            if let Some(Some(declared)) = self.declared_types.get(first) {
                return format!("{}{}", declared, &raw[first.len()..]);
            }
            if let Some(import) = self.imports.iter().find(|import| {
                !import.is_static
                    && !import.is_wildcard
                    && import.path.rsplit('.').next() == Some(first)
            }) {
                return format!("{}{}", import.path, &raw[first.len()..]);
            }
            return raw.into();
        }
        if let Some(Some(declared)) = self.declared_types.get(raw) {
            return declared.clone();
        }
        if matches!(
            raw,
            "String"
                | "Integer"
                | "Long"
                | "Short"
                | "Byte"
                | "Boolean"
                | "Character"
                | "Double"
                | "Float"
                | "Object"
                | "Class"
                | "Throwable"
                | "Exception"
                | "RuntimeException"
                | "Iterable"
                | "Enum"
                | "Record"
        ) {
            return format!("java.lang.{raw}");
        }
        if let Some(import) = self.imports.iter().find(|import| {
            !import.is_static && !import.is_wildcard && import.path.rsplit('.').next() == Some(raw)
        }) {
            return import.path.clone();
        }
        if raw.len() == 1 && raw.as_bytes()[0].is_ascii_uppercase() {
            return format!("?{raw}");
        }
        let wildcard_count = self
            .imports
            .iter()
            .filter(|import| !import.is_static && import.is_wildcard)
            .count();
        if wildcard_count > 0 {
            return format!("?{raw}");
        }
        if self.package.is_empty() {
            format!("?{raw}")
        } else {
            format!("{}.{raw}", self.package)
        }
    }
}

fn declared_type_names(
    root: Node<'_>,
    source: &[u8],
    package: &str,
) -> BTreeMap<String, Option<String>> {
    fn visit(
        node: Node<'_>,
        source: &[u8],
        package: &str,
        owner_chain: &mut Vec<String>,
        out: &mut BTreeMap<String, Option<String>>,
    ) {
        let is_type = TYPE_KINDS.contains(&node.kind());
        let mut pushed = false;
        if is_type && let Some(name_node) = node.child_by_field_name("name") {
            let name = text(name_node, source).to_string();
            let qualified = if package.is_empty() {
                owner_chain
                    .iter()
                    .chain(std::iter::once(&name))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(".")
            } else {
                format!(
                    "{package}.{}",
                    owner_chain
                        .iter()
                        .chain(std::iter::once(&name))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(".")
                )
            };
            match out.get(&name) {
                None => {
                    out.insert(name.clone(), Some(qualified));
                }
                Some(Some(existing)) if existing != &qualified => {
                    out.insert(name.clone(), None);
                }
                _ => {}
            }
            owner_chain.push(name);
            pushed = true;
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        drop(cursor);
        for child in children {
            visit(child, source, package, owner_chain, out);
        }
        if pushed {
            owner_chain.pop();
        }
    }

    let mut out = BTreeMap::new();
    visit(root, source, package, &mut Vec::new(), &mut out);
    out
}

fn import_fact(node: Node<'_>, source: &[u8]) -> Option<JavaImportFact> {
    let raw = text(node, source).trim().trim_end_matches(';').trim();
    let raw = raw.strip_prefix("import")?.trim();
    let (is_static, raw) = raw
        .strip_prefix("static")
        .map(|rest| (true, rest.trim()))
        .unwrap_or((false, raw));
    let is_wildcard = raw.ends_with(".*");
    let path = raw.trim_end_matches(".*").trim().to_string();
    (!path.is_empty()).then_some(JavaImportFact {
        path,
        is_static,
        is_wildcard,
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
        start_line: line_start(node),
        end_line: line_end(node),
    })
}

fn package_text(node: Node<'_>, source: &[u8]) -> String {
    text(node, source)
        .trim()
        .strip_prefix("package")
        .unwrap_or_default()
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string()
}

fn modifiers(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(modifiers) = node.child_by_field_name("modifiers").or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| child.kind() == "modifiers")
    }) else {
        return Vec::new();
    };
    let raw = text(modifiers, source);
    [
        "public",
        "protected",
        "private",
        "static",
        "final",
        "abstract",
        "synchronized",
        "native",
        "strictfp",
        "default",
        "sealed",
        "non-sealed",
        "transient",
        "volatile",
    ]
    .into_iter()
    .filter(|modifier| raw.split_whitespace().any(|token| token == *modifier))
    .map(str::to_string)
    .collect()
}

fn annotations(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "modifiers" {
            continue;
        }
        let mut modifier_cursor = child.walk();
        for modifier in child.named_children(&mut modifier_cursor) {
            if matches!(modifier.kind(), "annotation" | "marker_annotation") {
                out.push(text(modifier, source).trim().to_string());
            }
        }
    }
    out
}

fn javadoc(node: Node<'_>, source: &[u8]) -> Option<String> {
    let previous = node.prev_named_sibling()?;
    if previous.kind() != "block_comment" {
        return None;
    }
    let value = text(previous, source).trim();
    value.starts_with("/**").then(|| value.to_string())
}

fn split_generic(raw: &str) -> Option<(&str, &str)> {
    let start = raw.find('<')?;
    let end = raw.rfind('>')?;
    (end > start).then_some((raw[..start].trim(), raw[start + 1..end].trim()))
}

fn split_top_level(raw: &str, delimiter: char) -> Vec<&str> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (index, ch) in raw.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                out.push(raw[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(raw[start..].trim());
    out
}

fn find_first_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_first_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn collect_kind(node: Node<'_>, kind: &str, visit: &mut impl FnMut(Node<'_>)) {
    if node.kind() == kind {
        visit(node);
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    drop(cursor);
    for child in children {
        collect_kind(child, kind, visit);
    }
}

fn text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or_default()
}

fn line_start(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn line_end(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}
