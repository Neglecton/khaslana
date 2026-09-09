// 代码索引引擎测试（src/tests/code_index.rs，#[path] 挂载于 src/lib.rs 对应模块）。

use super::*;

// ---------------------------------------------------------------------------
// 提取层：验证各语言节点类型表与真实语法的行为
// ---------------------------------------------------------------------------

fn extract_defs(lang: LangId, source: &str) -> FileExtractResult {
    let mut extractor = Extractor::new();
    extractor
        .extract(lang, source.as_bytes())
        .expect("解析器初始化失败")
        .expect("应产出提取结果")
}

#[test]
fn extract_python_symbols() {
    let result = extract_defs(
        LangId::Python,
        r#"
import os.path
from collections import OrderedDict

class Service:
    def __init__(self):
        self.count = 0

    def run(self):
        return OrderedDict()

@decorator
def top_level():
    os.path.join("a", "b")
"#,
    );
    assert!(
        result
            .defs
            .iter()
            .any(|d| d.name == "Service" && d.label == NodeLabel::Class)
    );
    let run = result.defs.iter().find(|d| d.name == "run").expect("run");
    assert_eq!(label_str(run.label), "Method");
    assert_eq!(run.scope, vec!["Service".to_string()]);
    // @decorator 包装下的函数仍被提取。
    assert!(result.defs.iter().any(|d| d.name == "top_level"));
    // 调用点包含 join 与 OrderedDict。
    let calls: Vec<&str> = result.calls.iter().map(|c| c.name.as_str()).collect();
    assert!(calls.contains(&"join"), "{calls:?}");
    assert!(calls.contains(&"OrderedDict"));
    assert!(result.imports.iter().any(|i| i.module.contains("os.path")));
}

#[test]
fn extract_go_methods_with_receiver() {
    let result = extract_defs(
        LangId::Go,
        r#"
package service

import "fmt"

type Client struct {
	Endpoint string
}

func (c *Client) Push(name string) error {
	fmt.Println(name)
	return nil
}

func NewClient() *Client {
	return &Client{}
}
"#,
    );
    let push = result
        .defs
        .iter()
        .find(|d| d.name == "Client.Push")
        .expect("Push");
    assert_eq!(label_str(push.label), "Method");
    assert!(
        result
            .defs
            .iter()
            .any(|d| d.name == "Client" && d.label == NodeLabel::Struct)
    );
    assert!(result.defs.iter().any(|d| d.name == "NewClient"));
    let calls: Vec<&str> = result.calls.iter().map(|c| c.name.as_str()).collect();
    assert!(calls.contains(&"Println"), "{calls:?}");
}

#[test]
fn extract_typescript_interfaces_and_tsx() {
    for lang in [LangId::TypeScript, LangId::Tsx] {
        let result = extract_defs(
            lang,
            r#"
export interface Props {
    title: string;
}

export type Alias = string | number;

export class Panel implements Widget {
    render(): void {}
}

const handler = () => { Panel; };
"#,
        );
        assert!(
            result
                .defs
                .iter()
                .any(|d| d.name == "Props" && d.label == NodeLabel::Interface),
            "{lang:?} 缺 interface"
        );
        assert!(
            result
                .defs
                .iter()
                .any(|d| d.name == "Alias" && d.label == NodeLabel::Type)
        );
        // export_statement 透明下穿 + implements 关系。
        let panel = result
            .defs
            .iter()
            .find(|d| d.name == "Panel")
            .expect("Panel");
        assert_eq!(label_str(panel.label), "Class");
        assert!(
            result
                .type_refs
                .iter()
                .any(|t| t.name == "Widget" && !t.inherits),
            "{lang:?} 缺 implements 引用"
        );
        // const handler = () => {} 记为 Function。
        assert!(result.defs.iter().any(|d| d.name == "handler"));
    }
}

#[test]
fn extract_java_inheritance() {
    let result = extract_defs(
        LangId::Java,
        r#"
import java.util.List;

public class Main extends Base implements Runnable, Closeable {
    private int count;

    public void run() {
        List.of(1);
    }
}
"#,
    );
    assert!(
        result
            .defs
            .iter()
            .any(|d| d.name == "Main" && d.label == NodeLabel::Class)
    );
    let run = result.defs.iter().find(|d| d.name == "run").expect("run");
    assert_eq!(label_str(run.label), "Method");
    assert!(
        result
            .type_refs
            .iter()
            .any(|t| t.name == "Base" && t.inherits)
    );
    assert!(
        result
            .type_refs
            .iter()
            .any(|t| t.name == "Runnable" && !t.inherits)
    );
    let calls: Vec<&str> = result.calls.iter().map(|c| c.name.as_str()).collect();
    assert!(calls.contains(&"of"), "{calls:?}");
    let list_of = result
        .calls
        .iter()
        .find(|call| call.name == "of")
        .expect("List.of");
    assert_eq!(list_of.receiver_expr, "List");
    assert_eq!(list_of.argument_exprs, vec!["1"]);
}

#[test]
fn dev04_extracts_java_signatures_import_kinds_nested_types_and_scopes() {
    let result = extract_defs(
        LangId::Java,
        r#"
package com.example.demo;

import static java.lang.Math.max;
import java.util.*;
import java.util.ArrayList;

public class Outer {
    private String name;

    public String f(int value) { return String.valueOf(value); }
    public String f(String value) { return value; }
    public String f(long base, int... extras) { return name; }

    class Inner {
        void use(String name) {
            var values = new ArrayList<String>();
            values.add(name);
        }
    }
}
"#,
    );
    let java = result.java.as_ref().expect("Java 专用事实");
    assert_eq!(java.package, "com.example.demo");
    assert!(java.imports.iter().any(|import| {
        import.path == "java.lang.Math.max" && import.is_static && !import.is_wildcard
    }));
    assert!(
        java.imports.iter().any(|import| {
            import.path == "java.util" && !import.is_static && import.is_wildcard
        })
    );

    let signatures: Vec<&str> = java
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "f")
        .map(|symbol| symbol.signature.as_str())
        .collect();
    assert_eq!(
        signatures,
        vec!["(int)", "(java.lang.String)", "(long,int...)"]
    );
    let inner = java
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Inner")
        .expect("Inner");
    assert_eq!(inner.owner_chain, vec!["Outer"]);
    let use_method = java
        .symbols
        .iter()
        .find(|symbol| symbol.name == "use")
        .expect("use");
    assert_eq!(use_method.owner_chain, vec!["Outer", "Inner"]);
    assert!(java.variables.iter().any(|variable| {
        variable.name == "name"
            && variable.kind == "parameter"
            && variable.type_name == "java.lang.String"
    }));
    assert!(java.variables.iter().any(|variable| {
        variable.name == "values"
            && variable.kind == "local"
            && variable.type_name == "java.util.ArrayList<java.lang.String>"
            && variable.scope_end_byte > variable.scope_start_byte
    }));
}

#[test]
fn dev04_extracts_java_heritage_and_record_components() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/tests/fixtures/code_understanding/java/J08/com/example/modern/Order.java"),
    )
    .unwrap();
    let result = extract_defs(LangId::Java, &source);
    let java = result.java.as_ref().expect("Java 专用事实");

    let item = java
        .symbols
        .iter()
        .find(|symbol| symbol.kind == "record" && symbol.name == "Item")
        .expect("Item record");
    assert_eq!(item.owner_chain, vec!["Order"]);
    let sku = java
        .symbols
        .iter()
        .find(|symbol| symbol.kind == "record_accessor" && symbol.name == "sku")
        .expect("record accessor sku()");
    assert_eq!(sku.owner_chain, vec!["Order", "Item"]);
    assert_eq!(sku.signature, "()");
    assert_eq!(sku.return_type.as_deref(), Some("java.lang.String"));
    assert!(java.variables.iter().any(|variable| {
        variable.kind == "record_component"
            && variable.name == "sku"
            && variable.owner_chain == ["Order", "Item"]
    }));

    let reply = java
        .symbols
        .iter()
        .find(|symbol| symbol.kind == "interface" && symbol.name == "Reply")
        .expect("Reply interface");
    assert_eq!(
        reply.permits,
        vec![
            "com.example.modern.Order.Ok",
            "com.example.modern.Order.Err"
        ]
    );
    let ok = java
        .symbols
        .iter()
        .find(|symbol| symbol.kind == "record" && symbol.name == "Ok")
        .expect("Ok record");
    assert_eq!(ok.implements, vec!["com.example.modern.Order.Reply"]);
}

#[test]
fn dev04_persists_overloads_and_maven_module_source_set_isolation() {
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = target.join(entry.file_name());
            if source_path.is_dir() {
                copy_tree(&source_path, &target_path);
            } else {
                std::fs::copy(source_path, target_path).unwrap();
            }
        }
    }

    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/code_understanding/java");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("java-project");
    copy_tree(&fixtures.join("J09"), &root);
    let test_named = root.join("app-core/src/test/java/com/example/core/Named.java");
    std::fs::create_dir_all(test_named.parent().unwrap()).unwrap();
    std::fs::copy(
        root.join("app-core/src/main/java/com/example/core/Named.java"),
        &test_named,
    )
    .unwrap();
    let overloads = fixtures.join("J02/com/example/overload/Overloads.java");
    let overload_target = root.join("app-core/src/main/java/com/example/overload/Overloads.java");
    std::fs::create_dir_all(overload_target.parent().unwrap()).unwrap();
    std::fs::copy(overloads, &overload_target).unwrap();

    let db_path = temp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let connection = rusqlite::Connection::open(&db_path).unwrap();

    let mut named = connection
        .prepare(
            "SELECT s.module_key,s.source_set,s.package,s.signature,s.type_metadata_json \
             FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key \
             WHERE n.name='Named' ORDER BY s.module_key,s.source_set",
        )
        .unwrap();
    let named_rows: Vec<(String, String, String)> = named
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        named_rows,
        vec![
            ("app-core".into(), "main".into(), "com.example.core".into()),
            ("app-core".into(), "test".into(), "com.example.core".into()),
            ("app-web".into(), "main".into(), "com.example.core".into()),
        ]
    );
    let test_source_set: String = connection
        .query_row(
            "SELECT s.source_set FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key WHERE n.name='CoreRunnerTest'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(test_source_set, "test");

    let overload_signatures: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT s.signature FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key WHERE n.file_path LIKE '%Overloads.java' AND n.name IN ('f','g') ORDER BY n.start_line",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        overload_signatures,
        vec![
            "(int)",
            "(java.lang.String)",
            "(long,int...)",
            "(java.lang.Integer)",
            "(long)",
        ]
    );
    let distinct_keys: i64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT n.symbol_key) FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key WHERE n.file_path LIKE '%Overloads.java' AND n.name IN ('f','g')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(distinct_keys, 5);
    let overload_keys_before: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT n.symbol_key FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key WHERE n.file_path LIKE '%Overloads.java' ORDER BY n.name,s.signature",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    drop(named);
    drop(connection);

    let moved_dir = root.join("app-core/src/main/java/relocated");
    std::fs::create_dir_all(&moved_dir).unwrap();
    std::fs::rename(&overload_target, moved_dir.join("Overloads.java")).unwrap();
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    let overload_keys_after: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT n.symbol_key FROM symbol_semantics s JOIN nodes n ON n.symbol_key=s.symbol_key WHERE n.file_path LIKE '%Overloads.java' ORDER BY n.name,s.signature",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        overload_keys_after, overload_keys_before,
        "Java 稳定身份不应因源根内文件移动而变化"
    );
}

#[test]
fn extract_c_function_definition_declarator() {
    let result = extract_defs(
        LangId::C,
        r#"
#include <stdio.h>

struct Config {
    int width;
};

typedef struct Config ConfigT;

int *allocate(size_t n) {
    return malloc(n);
}
"#,
    );
    assert!(
        result
            .defs
            .iter()
            .any(|d| d.name == "Config" && d.label == NodeLabel::Struct)
    );
    assert!(
        result
            .defs
            .iter()
            .any(|d| d.name == "ConfigT" && d.label == NodeLabel::Type)
    );
    assert!(
        result.defs.iter().any(|d| d.name == "allocate"),
        "指针返回值函数名挖掘失败"
    );
    assert!(result.imports.iter().any(|i| i.module.contains("stdio")));
}

#[test]
fn extract_php_kotlin_basic() {
    let php = extract_defs(
        LangId::Php,
        r#"<?php
namespace App\Service;

class Mailer {
    public function send(string $to): bool {
        return mail($to);
    }
}
"#,
    );
    assert!(
        php.defs
            .iter()
            .any(|d| d.name == "Mailer" && d.label == NodeLabel::Class)
    );
    let send = php.defs.iter().find(|d| d.name == "send").expect("send");
    assert_eq!(label_str(send.label), "Method");

    let kt = extract_defs(
        LangId::Kotlin,
        r#"
import kotlinx.coroutines.runBlocking

class Repo {
    fun load(): String {
        return build()
    }
}
"#,
    );
    assert!(
        kt.defs
            .iter()
            .any(|d| d.name == "Repo" && d.label == NodeLabel::Class)
    );
    let load = kt.defs.iter().find(|d| d.name == "load").expect("load");
    assert_eq!(label_str(load.label), "Method");
    assert!(kt.calls.iter().any(|c| c.name == "build"));
}

fn label_str(label: NodeLabel) -> &'static str {
    label.as_str()
}

// ---------------------------------------------------------------------------
// discover 过滤 / camel_split / QN / 图缓冲
// ---------------------------------------------------------------------------

#[test]
fn discover_skips_ignored_dirs_and_suffixes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("src/logo.png"), "binary").unwrap();
    std::fs::write(root.join("target/artifact.rlib"), "").unwrap();
    std::fs::write(root.join("node_modules/index.js"), "").unwrap();

    let outcome = discover_files(root).unwrap();
    let paths: Vec<&str> = outcome.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(paths, vec!["src/main.rs"], "{paths:?}");
    assert!(outcome.excluded_count >= 3);
}

#[test]
fn discover_respects_gitignore_and_submodule_marker() {
    let (_dir, _git_repo, _service) = crate::git::test_support::git_test_support::init_repo();
    let root = _dir.path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "generated/\n*.tmp\n").unwrap();
    std::fs::create_dir_all(root.join("generated")).unwrap();
    std::fs::write(root.join("keep.txt"), "x").unwrap();
    std::fs::write(root.join("skip.tmp"), "x").unwrap();
    std::fs::write(root.join("generated/out.txt"), "x").unwrap();
    // 子模块标记目录：.git 是文件。
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/.git"), "gitdir: ../.git/modules/sub").unwrap();
    std::fs::write(root.join("sub/code.py"), "print(1)").unwrap();
    // 不提交也可：发现基于工作区 + .gitignore，无需索引库状态。

    let outcome = discover_files(&root).unwrap();
    let paths: Vec<&str> = outcome.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert!(paths.contains(&"keep.txt"), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("generated")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.ends_with(".tmp")), "{paths:?}");
    assert!(
        !paths.iter().any(|p| p.starts_with("sub/")),
        "子模块目录应剪枝"
    );
}

#[test]
fn camel_split_tokens() {
    assert_eq!(
        camel_split("getRepositorySnapshot"),
        "get repository snapshot"
    );
    assert_eq!(camel_split("HTTPServer"), "http server");
    assert_eq!(camel_split("snake_case_name"), "snake case name");
    assert_eq!(camel_split("src/git.rs"), "src git rs");
    assert_eq!(camel_split("XMLHttpRequest"), "xml http request");
}

#[test]
fn graph_upsert_and_overload_suffix() {
    let mut g = GraphBuffer::new();
    let a = g.add_symbol(
        NodeLabel::Function,
        "foo",
        "proj.a.rs.foo".to_string(),
        "a.rs",
        1,
        2,
        "{}".to_string(),
    );
    let b = g.add_symbol(
        NodeLabel::Function,
        "foo",
        "proj.a.rs.foo".to_string(),
        "a.rs",
        5,
        9,
        "{}".to_string(),
    );
    assert_ne!(a, b);
    assert_eq!(g.get(b).qualified_name, "proj.a.rs.foo#2");

    // 幂等结构节点。
    let f1 = g.upsert_node(
        NodeLabel::File,
        "a.rs",
        "proj.a.rs",
        "a.rs",
        0,
        0,
        "{}".to_string(),
    );
    let f2 = g.upsert_node(
        NodeLabel::File,
        "a.rs",
        "proj.a.rs",
        "a.rs",
        0,
        0,
        "{}".to_string(),
    );
    assert_eq!(f1, f2);
}

#[test]
fn resolve_strategy_chain() {
    let mut g = GraphBuffer::new();
    for (label, qn, file) in [
        (NodeLabel::File, "p.a.rs", "a.rs"),
        (NodeLabel::File, "p.b.rs", "b.rs"),
        (NodeLabel::File, "p.c.rs", "c.rs"),
    ] {
        g.upsert_node(label, file, qn, file, 0, 0, "{}".to_string());
    }
    // helper 仅在 a.rs；only_one 全局唯一；work 在 a.rs 有两个重载、b.rs 一个。
    let helper = g.add_symbol(
        NodeLabel::Function,
        "helper",
        "p.a.rs.helper".to_string(),
        "a.rs",
        1,
        2,
        "{}".to_string(),
    );
    let only_one = g.add_symbol(
        NodeLabel::Function,
        "only_one",
        "p.c.rs.only_one".to_string(),
        "c.rs",
        1,
        2,
        "{}".to_string(),
    );
    let _overload_a1 = g.add_symbol(
        NodeLabel::Function,
        "work",
        "p.a.rs.work".to_string(),
        "a.rs",
        3,
        4,
        "{}".to_string(),
    );
    let overload_a2 = g.add_symbol(
        NodeLabel::Function,
        "work",
        "p.a.rs.work".to_string(),
        "a.rs",
        7,
        9,
        "{}".to_string(),
    );
    let remote_work = g.add_symbol(
        NodeLabel::Function,
        "work",
        "p.b.rs.work".to_string(),
        "b.rs",
        5,
        6,
        "{}".to_string(),
    );

    let registry = Registry::build(&g);

    // 1. 同文件唯一 -> local。
    let hit = registry.resolve_call("helper", "a.rs", &[], None).unwrap();
    assert_eq!(hit.id, helper);
    assert_eq!(hit.strategy, "local");

    // 2. 调用方没有本地定义且全仓库唯一 -> unique。
    let hit = registry.resolve_call("helper", "b.rs", &[], None).unwrap();
    assert_eq!(hit.strategy, "unique");
    assert_eq!(hit.id, helper);
    let hit = registry
        .resolve_call("only_one", "a.rs", &[], None)
        .unwrap();
    assert_eq!(hit.strategy, "unique");
    assert_eq!(hit.id, only_one);

    // 3. 本地重载多候选 + 限定调用 + 排除本文件唯一 -> suffix。
    let hit = registry
        .resolve_call("work", "a.rs", &[], Some("B"))
        .unwrap();
    assert_eq!(hit.id, remote_work);
    assert_eq!(hit.strategy, "suffix");

    // 4. 无限定时本地多候选无法消解 -> None。
    assert!(registry.resolve_call("work", "a.rs", &[], None).is_none());

    // 5. 无候选 -> None。
    assert!(
        registry
            .resolve_call("missing", "a.rs", &[], None)
            .is_none()
    );

    // 重载后缀确认。
    assert_eq!(g.get(overload_a2).qualified_name, "p.a.rs.work#2");
}

#[test]
fn store_roundtrip_and_fts_search() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    {
        let mut g = GraphBuffer::new();
        let project = g.upsert_node(
            NodeLabel::Project,
            "demo",
            "demo",
            "",
            0,
            0,
            "{}".to_string(),
        );
        let file = g.upsert_node(
            NodeLabel::File,
            "svc.rs",
            "demo.svc.rs",
            "svc.rs",
            0,
            0,
            "{}".to_string(),
        );
        let target = g.add_symbol(
            NodeLabel::Function,
            "pushBranch",
            "demo.svc.rs.pushBranch".to_string(),
            "svc.rs",
            10,
            20,
            "{}".to_string(),
        );
        let caller = g.add_symbol(
            NodeLabel::Function,
            "main",
            "demo.svc.rs.main".to_string(),
            "svc.rs",
            1,
            30,
            "{}".to_string(),
        );
        g.add_edge(file, target, EdgeType::Defines, "{}".to_string());
        g.add_edge(
            caller,
            target,
            EdgeType::Calls,
            calls_edge_properties("pushBranch", 0.95, "local"),
        );
        let _ = project;

        let mut store = CodeIndexStore::open(&db_path).unwrap();
        let hashes = vec![FileHashRow {
            rel_path: "svc.rs".to_string(),
            sha256: crate::code_index::content_fingerprint(b"fn pushBranch() {}"),
            mtime_ns: 1,
            size: 2,
        }];
        store
            .replace_all(&g, &hashes, &CodeIndexMeta::default())
            .unwrap();
    }
    let store = CodeIndexStore::open(&db_path).unwrap();
    // 驼峰拆分检索：查 push branch 应命中 pushBranch。
    let hits = store.search_symbols("push branch", 10).unwrap();
    assert!(hits.iter().any(|h| h.name == "pushBranch"), "hits empty");
    let stats = store.read_stats().unwrap().unwrap();
    assert_eq!(stats.nodes, 4);
    assert_eq!(stats.calls, 1);
    assert_eq!(stats.files, 1);
}

#[test]
fn v3_schema_enforces_facts_constraints_and_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    drop(CodeIndexStore::open(&db_path).unwrap());
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let hex = "0".repeat(64);

    assert!(
        conn.execute(
            "INSERT INTO entities(entity_key,entity_kind) VALUES(?1,'symbol')",
            rusqlite::params![format!("sk1:{}", "A".repeat(64))],
        )
        .is_err()
    );

    assert!(conn.execute(
        "INSERT INTO call_sites(callsite_key,owner_key,rel_path,start_byte,end_byte,start_line,end_line,content_sha256,callee_expr,resolution) VALUES(?1,'owner','path.rs',-1,0,1,1,?2,'call','{}')",
        rusqlite::params![format!("cs1:{hex}"), hex],
    ).is_err());
    assert!(conn.execute(
        "INSERT INTO call_sites(callsite_key,owner_key,rel_path,start_byte,end_byte,start_line,end_line,content_sha256,callee_expr,resolution) VALUES(?1,'owner','path.rs',0,0,0,1,?2,'call','{}')",
        rusqlite::params![format!("cs1:{}", "1".repeat(64)), "0".repeat(64)],
    ).is_err());
    assert!(conn.execute(
        "INSERT INTO call_sites(callsite_key,owner_key,rel_path,start_byte,end_byte,start_line,end_line,content_sha256,callee_expr,resolution) VALUES(?1,'owner','path.rs',0,0,1,1,'bad','call','{}')",
        rusqlite::params![format!("cs1:{}", "2".repeat(64))],
    ).is_err());
    assert!(conn.execute(
        "INSERT INTO relations(relation_id,source_key,kind,certainty,resolution_state) VALUES(?1,'source','calls','syntactic','unresolved')",
        rusqlite::params![format!("r1:{}", "A".repeat(64))],
    ).is_err());
    assert!(conn.execute(
        "INSERT INTO evidence(evidence_id,kind,excerpt_digest,refs_json) VALUES(?1,'unknown','digest','[]')",
        rusqlite::params![format!("e1:{hex}")],
    ).is_err());
    assert!(
        conn.execute(
            "INSERT INTO evidence(evidence_id,kind,excerpt_digest,refs_json) VALUES(?1,'declaration','digest','[]')",
            rusqlite::params![format!("e1:{}", "A".repeat(64))],
        )
        .is_err()
    );

    let mut stmt = conn
        .prepare("SELECT name FROM pragma_index_list('relations')")
        .unwrap();
    let relation_indexes: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        relation_indexes
            .iter()
            .any(|name| name == "idx_relations_source")
    );
    assert!(
        relation_indexes
            .iter()
            .any(|name| name == "idx_relations_target")
    );
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_index_list('call_sites')")
        .unwrap();
    let callsite_indexes: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        callsite_indexes
            .iter()
            .any(|name| name == "idx_callsites_owner")
    );
    assert!(
        callsite_indexes
            .iter()
            .any(|name| name == "idx_callsites_path")
    );
}

#[test]
fn v3_meta_uses_dev01_coverage_wire_format() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    let mut graph = GraphBuffer::new();
    let caller = graph.add_symbol(
        NodeLabel::Function,
        "caller",
        "demo.caller".to_string(),
        "src.rs",
        1,
        1,
        "{}".to_string(),
    );
    let target = graph.add_symbol(
        NodeLabel::Function,
        "target",
        "demo.target".to_string(),
        "src.rs",
        2,
        2,
        "{}".to_string(),
    );
    graph.add_edge(caller, target, EdgeType::Calls, "{}".to_string());
    let hashes = vec![FileHashRow {
        rel_path: "src.rs".to_string(),
        sha256: content_fingerprint(b"fn caller() { target() }"),
        mtime_ns: 1,
        size: 24,
    }];
    let mut store = CodeIndexStore::open(&db_path).unwrap();
    store
        .replace_all(&graph, &hashes, &CodeIndexMeta::default())
        .unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let coverage_json: String = conn
        .query_row("SELECT value FROM meta WHERE key='coverage'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let coverage: CoverageSummary = serde_json::from_str(&coverage_json).unwrap();
    assert_eq!(coverage.files_discovered, 1);
    assert_eq!(coverage.files_parsed, 1);
    assert_eq!(coverage.calls_total, 1);
    assert_eq!(coverage.calls_resolved, 1);
    assert_eq!(
        coverage.calls_ambiguous + coverage.calls_unresolved + coverage.calls_external,
        0
    );
    let adapters_json: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='adapter_versions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let adapters: Vec<(String, String)> = serde_json::from_str(&adapters_json).unwrap();
    assert_eq!(adapters, vec![("generic".to_string(), "1".to_string())]);
}

#[test]
fn stored_symbol_key_normalizes_the_project_identity() {
    fn key_for(db_path: &std::path::Path, repo_path: &str) -> String {
        let mut graph = GraphBuffer::new();
        graph.add_symbol(
            NodeLabel::Function,
            "work",
            "demo.work".to_string(),
            "src/work.rs",
            1,
            1,
            "{}".to_string(),
        );
        let hashes = vec![FileHashRow {
            rel_path: "src/work.rs".to_string(),
            sha256: content_fingerprint(b"fn work() {}"),
            mtime_ns: 1,
            size: 12,
        }];
        let mut store = CodeIndexStore::open(db_path).unwrap();
        store
            .replace_all(
                &graph,
                &hashes,
                &CodeIndexMeta {
                    repo_path: repo_path.to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        drop(store);
        rusqlite::Connection::open(db_path)
            .unwrap()
            .query_row(
                "SELECT symbol_key FROM nodes WHERE symbol_key IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    let tmp = tempfile::tempdir().unwrap();
    let first = key_for(&tmp.path().join("first.db"), r"D:\Demo\Repo\");
    let second = key_for(&tmp.path().join("second.db"), r"d:\demo\repo");
    assert_eq!(first, second);
}

#[test]
fn read_only_v2_or_corrupt_database_requires_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let v2 = tmp.path().join("v2.db");
    let conn = rusqlite::Connection::open(&v2).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2');").unwrap();
    drop(conn);
    let open_error = match open_read_only_if_exists(&v2) {
        Ok(_) => panic!("v2 库不得被当作兼容只读库打开"),
        Err(error) => error.to_string(),
    };
    for error in [
        open_error,
        read_index_stats(&v2).unwrap_err().to_string(),
        search_symbols(&v2, "anything", 10).unwrap_err().to_string(),
    ] {
        assert!(error.contains("NeedsRebuild"), "{error}");
    }
    let corrupt = tmp.path().join("corrupt.db");
    std::fs::write(&corrupt, b"not sqlite").unwrap();
    assert!(
        read_index_stats(&corrupt)
            .unwrap_err()
            .to_string()
            .contains("NeedsRebuild")
    );
    let missing = tmp.path().join("missing.db");
    assert!(open_read_only_if_exists(&missing).unwrap().is_none());
    assert!(read_index_stats(&missing).unwrap().is_none());
}

#[test]
fn v2_store_is_not_deleted_and_reports_needs_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("v2.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2'); CREATE TABLE preserved(value TEXT);").unwrap();
    drop(conn);
    let error = match CodeIndexStore::open(&db_path) {
        Ok(_) => panic!("v2 库不得被当作 v3 打开"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("NeedsRebuild"), "{error}");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM preserved", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn v2_rebuild_publishes_v3_in_one_transaction() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("v2-rebuild.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2');").unwrap();
    drop(conn);
    let mut graph = GraphBuffer::new();
    graph.add_symbol(
        NodeLabel::Function,
        "work",
        "demo.work".to_string(),
        "work.rs",
        1,
        1,
        "{}".to_string(),
    );
    let hashes = vec![FileHashRow {
        rel_path: "work.rs".to_string(),
        sha256: crate::code_index::content_fingerprint(b"fn work() {}"),
        mtime_ns: 1,
        size: 12,
    }];
    CodeIndexStore::rebuild_incompatible(
        &db_path,
        &graph,
        &hashes,
        &CodeIndexMeta {
            repo_path: "demo".to_string(),
            ..Default::default()
        },
    )
    .unwrap();
    let store = CodeIndexStore::open(&db_path).unwrap();
    assert_eq!(store.generation().unwrap(), 1);
    assert_eq!(
        store.load_file_hashes().unwrap()[0].sha256,
        hashes[0].sha256
    );
}

#[test]
fn pipeline_v2_rebuild_publishes_once_at_generation_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_repo_file(&root, "src/main.rs", "fn main() {}\n");
    let db_path = tmp.path().join("v2.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2');").unwrap();
    drop(conn);
    assert!(matches!(
        run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap(),
        RunOutcome::Completed(_)
    ));
    let store = CodeIndexStore::open(&db_path).unwrap();
    assert_eq!(store.generation().unwrap(), 1);
    assert_eq!(store.read_stats().unwrap().unwrap().mode, "rebuild");
}

#[test]
fn stale_generation_publish_rolls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    let mut first = CodeIndexStore::open(&db).unwrap();
    let mut second = CodeIndexStore::open(&db).unwrap();
    let graph = GraphBuffer::new();
    second
        .replace_all_at_generation(&graph, &[], &CodeIndexMeta::default(), 0)
        .unwrap();
    assert!(
        first
            .replace_all_at_generation(&graph, &[], &CodeIndexMeta::default(), 0)
            .is_err()
    );
    assert_eq!(CodeIndexStore::open(&db).unwrap().generation().unwrap(), 1);
}

#[test]
fn cancelled_v2_rebuild_preserves_old_database() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_repo_file(&root, "src/main.rs", "fn main() {}\n");
    let db_path = tmp.path().join("old.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2'); CREATE TABLE keep_me(value TEXT);").unwrap();
    drop(conn);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut options = PipelineOptions::new(cancel, Box::new(|_| {}));
    assert!(matches!(
        run_index(&root, &db_path, true, &mut options).unwrap(),
        RunOutcome::Cancelled
    ));
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "2"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM keep_me", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn failed_v2_rebuild_rolls_back_old_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("failed-rebuild.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','2'); CREATE TABLE keep_me(value TEXT);").unwrap();
    drop(conn);
    let graph = GraphBuffer::new();
    let invalid_hash = vec![FileHashRow {
        rel_path: "bad.rs".to_string(),
        sha256: "not-a-sha".to_string(),
        mtime_ns: 0,
        size: 0,
    }];
    assert!(
        CodeIndexStore::rebuild_incompatible(
            &db_path,
            &graph,
            &invalid_hash,
            &CodeIndexMeta::default()
        )
        .is_err()
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "2"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM keep_me", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

// ---------------------------------------------------------------------------
// 管线集成：全量 → 增量（修改/新增/删除）→ 取消
// ---------------------------------------------------------------------------

fn write_repo_file(root: &std::path::Path, rel: &str, body: &str) {
    let full = root.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

fn no_cancel_options() -> PipelineOptions {
    PipelineOptions::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Box::new(|_| {}),
    )
}

#[test]
fn pipeline_full_then_incremental() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("demo");
    std::fs::create_dir_all(&root).unwrap();
    // git init 让 ignore crate 的 gitignore 生效路径一致（非必需但贴近真实）。
    let _ = git2::Repository::init(&root);
    write_repo_file(
        &root,
        "src/lib.rs",
        "fn alpha() {
    beta();
}

",
    );
    write_repo_file(
        &root,
        "src/beta.rs",
        "fn beta() {}
",
    );
    write_repo_file(
        &root,
        "README.md",
        "# demo
",
    );
    let db_path = tmp.path().join("index.db");

    // 全量。
    let outcome = run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let RunOutcome::Completed(stats) = outcome else {
        panic!("应完成");
    };
    assert_eq!(stats.files, 3);
    assert!(stats.symbols >= 2, "{stats:?}");
    assert!(stats.calls >= 1, "{stats:?}");

    let store = CodeIndexStore::open(&db_path).unwrap();
    assert!(
        store
            .search_symbols("alpha", 10)
            .unwrap()
            .iter()
            .any(|h| h.name == "alpha")
    );
    drop(store);

    // 无变化增量：零写入快速返回。
    match run_incremental_if_stale(&root, &db_path, &mut no_cancel_options()).unwrap() {
        IncrementalOutcome::NoChange => {}
        other => panic!("应为 NoChange，实际 {other:?}"),
    }

    // 修改 beta.rs + 新增 gamma.py + 删除 README.md。
    write_repo_file(
        &root,
        "src/beta.rs",
        "fn beta_renamed() {
    alpha();
}
",
    );
    write_repo_file(
        &root,
        "src/gamma.py",
        "def gamma():
    pass
",
    );
    std::fs::remove_file(root.join("README.md")).unwrap();

    match run_incremental_if_stale(&root, &db_path, &mut no_cancel_options()).unwrap() {
        IncrementalOutcome::Updated(stats) => {
            assert_eq!(stats.files, 3, "{stats:?}");
        }
        other => panic!("应为 Updated，实际 {other:?}"),
    }

    let store = CodeIndexStore::open(&db_path).unwrap();
    // 变更文件重建：旧符号消失、新符号存在。
    assert!(
        !store
            .search_symbols("beta", 50)
            .unwrap()
            .iter()
            .any(|h| h.qualified_name.contains("beta()"))
    );
    assert!(
        store
            .search_symbols("beta renamed", 50)
            .unwrap()
            .iter()
            .any(|h| h.name == "beta_renamed"),
        "变更文件重解析失败"
    );
    assert!(
        store
            .search_symbols("gamma", 50)
            .unwrap()
            .iter()
            .any(|h| h.name == "gamma")
    );
    // 未变文件的符号仍在（QN 稳定性）。
    let graph = store.load_graph().unwrap();
    assert!(
        graph.find_by_qn("demo.src.lib.rs.alpha").is_some(),
        "未变文件符号丢失"
    );
    // 删除文件已清除。
    assert!(graph.find_by_qn("demo.README.md").is_none());
    // 跨文件调用边：beta_renamed -> alpha（同仓库唯一）。
    let calls = graph
        .edges
        .iter()
        .filter(|e| e.etype == EdgeType::Calls)
        .count();
    assert!(calls >= 1, "应有跨文件调用边");
}

#[test]
fn dev03_incremental_modify_delete_matches_fresh_full_snapshot() {
    fn rows(conn: &rusqlite::Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("equivalent");
    std::fs::create_dir_all(&root).unwrap();
    write_repo_file(&root, "src/a.rs", "fn a() { b(); }\n");
    write_repo_file(&root, "src/b.rs", "fn b() {}\n");
    let incremental_db = tmp.path().join("incremental.db");
    run_index(&root, &incremental_db, true, &mut no_cancel_options()).unwrap();

    write_repo_file(&root, "src/a.rs", "fn a() { c(); missing(); }\n");
    std::fs::remove_file(root.join("src/b.rs")).unwrap();
    write_repo_file(&root, "src/c.rs", "fn c() {}\n");
    assert!(matches!(
        run_incremental_if_stale(&root, &incremental_db, &mut no_cancel_options()).unwrap(),
        IncrementalOutcome::Updated(_)
    ));

    let full_db = tmp.path().join("full.db");
    run_index(&root, &full_db, true, &mut no_cancel_options()).unwrap();
    let incremental = rusqlite::Connection::open(&incremental_db).unwrap();
    let full = rusqlite::Connection::open(&full_db).unwrap();
    for sql in [
        "SELECT label||char(31)||name||char(31)||qualified_name||char(31)||file_path||char(31)||start_line||char(31)||end_line||char(31)||properties FROM nodes ORDER BY qualified_name,label,start_line,end_line",
        "SELECT s.qualified_name||char(31)||t.qualified_name||char(31)||e.type||char(31)||e.properties FROM edges e JOIN nodes s ON s.id=e.source_id JOIN nodes t ON t.id=e.target_id ORDER BY 1",
        "SELECT rel_path||char(31)||sha256||char(31)||size FROM file_hashes ORDER BY rel_path",
        "SELECT rel_path||char(31)||sha256||char(31)||parse_status||char(31)||facts_json FROM file_facts ORDER BY rel_path",
        "SELECT callsite_key||char(31)||owner_key||char(31)||rel_path||char(31)||start_byte||char(31)||end_byte||char(31)||callee_expr||char(31)||resolution FROM call_sites ORDER BY callsite_key",
        "SELECT relation_id||char(31)||source_key||char(31)||COALESCE(target_key,'')||char(31)||kind||char(31)||certainty||char(31)||resolution_state||char(31)||COALESCE(rule_id,'')||char(31)||COALESCE(heuristic_score,'') FROM relations ORDER BY relation_id",
        "SELECT rel_path||char(31)||adapter||char(31)||code||char(31)||detail||char(31)||range_json FROM diagnostics ORDER BY rel_path,adapter,code,detail",
        "SELECT value FROM meta WHERE key='coverage'",
    ] {
        assert_eq!(rows(&incremental, sql), rows(&full, sql), "SQL: {sql}");
    }
    for table in ["evidence", "relation_evidence"] {
        let sql = format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}");
        assert_eq!(
            rows(&incremental, &sql),
            rows(&full, &sql),
            "table: {table}"
        );
    }
}

#[test]
fn pipeline_cancel_discards_result() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cancelrepo");
    std::fs::create_dir_all(&root).unwrap();
    let _ = git2::Repository::init(&root);
    write_repo_file(
        &root,
        "a.rs",
        "fn keep() {}
",
    );
    let db_path = tmp.path().join("idx.db");

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut options = PipelineOptions::new(cancel, Box::new(|_| {}));
    let outcome = run_index(&root, &db_path, false, &mut options).unwrap();
    assert!(matches!(outcome, RunOutcome::Cancelled));
    // 未落盘：库文件不存在或无节点。
    if db_path.exists() {
        if let Ok(Some(stats)) = read_index_stats(&db_path) {
            assert_eq!(stats.nodes, 0, "取消后不应有数据");
        }
    }
}

#[test]
fn dev03_persists_each_callsite_and_closes_unresolved_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("facts");
    std::fs::create_dir_all(&root).unwrap();
    write_repo_file(
        &root,
        "src/lib.rs",
        "fn target() {}\nfn caller() { target(); target(); missing(); }\n",
    );
    let db_path = tmp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM call_sites", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM evidence WHERE kind='call_site'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM relations WHERE kind='calls' AND target_key IS NULL AND resolution_state='unresolved'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM relation_evidence re JOIN relations r ON r.relation_id=re.relation_id WHERE r.target_key IS NULL", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        conn.query_row(
            "SELECT certainty FROM relations WHERE kind='calls' AND target_key IS NOT NULL LIMIT 1",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "inferred"
    );
    assert_eq!(conn.query_row("SELECT rule_id FROM relations WHERE kind='calls' AND source_key LIKE 'cs1:%' AND target_key IS NOT NULL LIMIT 1", [], |row| row.get::<_, String>(0)).unwrap(), "legacy:local");
    let facts_json: String = conn
        .query_row(
            "SELECT facts_json FROM file_facts WHERE rel_path='src/lib.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let facts: serde_json::Value = serde_json::from_str(&facts_json).unwrap();
    assert!(
        facts["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call["owner"].as_str() == Some("facts.src.lib.rs.caller"))
    );
}

#[test]
fn dev03_coverage_persists_excluded_and_parse_diagnostics_across_incremental() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("coverage");
    std::fs::create_dir_all(root.join("target")).unwrap();
    write_repo_file(&root, "src/good.rs", "fn good() {}\n");
    write_repo_file(&root, "src/partial.rs", "fn partial(\n");
    std::fs::write(root.join("src/binary.rs"), b"fn binary() {}\0").unwrap();
    std::fs::write(
        root.join("src/too_big.rs"),
        vec![b' '; PARSE_MAX_BYTES as usize + 1],
    )
    .unwrap();
    write_repo_file(&root, "README.md", "# unsupported\n");
    std::fs::write(root.join("ignored.png"), b"not indexed").unwrap();
    std::fs::write(root.join("target/generated.rs"), b"fn generated() {}\n").unwrap();

    let db_path = tmp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();

    let assert_snapshot = || {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let coverage_json: String = conn
            .query_row("SELECT value FROM meta WHERE key='coverage'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let coverage: CoverageSummary = serde_json::from_str(&coverage_json).unwrap();
        assert_eq!(coverage.files_parsed, 1);
        assert_eq!(coverage.files_partial, 1);
        assert_eq!(coverage.files_unreadable, 0);
        assert!(coverage.files_skipped >= 5, "{coverage:?}");
        assert_eq!(
            coverage.files_discovered,
            coverage.files_parsed
                + coverage.files_partial
                + coverage.files_skipped
                + coverage.files_unreadable
        );
        for code in [
            "excluded",
            "parse_partial",
            "binary",
            "budget_exceeded",
            "unsupported_language",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM diagnostics WHERE code=?1",
                    [code],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(count > 0, "缺少诊断 {code}");
        }
    };
    assert_snapshot();

    write_repo_file(&root, "src/good.rs", "fn good_changed() {}\n");
    assert!(matches!(
        run_incremental_if_stale(&root, &db_path, &mut no_cancel_options()).unwrap(),
        IncrementalOutcome::Updated(_)
    ));
    assert_snapshot();
}

#[test]
fn dev03_unreadable_coverage_and_failed_publish_are_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    let graph = GraphBuffer::new();
    let valid = facts::SnapshotFacts {
        files_discovered: 1,
        diagnostics: vec![facts::DiagnosticFact {
            rel_path: "unreadable.rs".into(),
            adapter: "source_reader".into(),
            code: "unreadable".into(),
            detail: "permission denied".into(),
            range_json: "{}".into(),
        }],
        ..Default::default()
    };
    let mut store = CodeIndexStore::open(&db_path).unwrap();
    store
        .replace_all_with_facts_at_generation(&graph, &[], &valid, &CodeIndexMeta::default(), 0)
        .unwrap();

    let invalid = facts::SnapshotFacts {
        files_discovered: 1,
        ..Default::default()
    };
    assert!(
        store
            .replace_all_with_facts_at_generation(
                &graph,
                &[],
                &invalid,
                &CodeIndexMeta::default(),
                1,
            )
            .unwrap_err()
            .to_string()
            .contains("覆盖文件计数不闭合")
    );
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM meta WHERE key='generation'", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "1"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM diagnostics WHERE code='unreadable'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let coverage_json: String = conn
        .query_row("SELECT value FROM meta WHERE key='coverage'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let coverage: CoverageSummary = serde_json::from_str(&coverage_json).unwrap();
    assert_eq!(coverage.files_discovered, 1);
    assert_eq!(coverage.files_unreadable, 1);
}

#[test]
#[ignore = "手动冒烟：对本机 khaslana 仓库建索引（cargo test --lib code_index::tests::smoke_index_khaslana -- --ignored）"]
fn smoke_index_khaslana() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("self.db");
    let started = std::time::Instant::now();
    let mut options = PipelineOptions::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Box::new(|p| println!("[{:?}] {}", p.phase, p.message)),
    );
    match run_index(repo_root, &db_path, true, &mut options).unwrap() {
        RunOutcome::Completed(stats) => {
            println!(
                "khaslana 索引完成：{} 文件 / {} 符号 / {} 边（调用 {}）/ 耗时 {:?}（参考工具对照：约 2951 节点 / 11760 边）",
                stats.files,
                stats.symbols,
                stats.edges,
                stats.calls,
                started.elapsed()
            );
            assert!(stats.files > 100);
            assert!(stats.symbols > 500);
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 回归测试：审查修复的三个缺陷（import_map 失效 / 孤儿 Module / 导入清洗）
// ---------------------------------------------------------------------------

/// 回归①：Rust `use crate::git::service` 必须经 import_map 命中
/// `src/git/service.rs` 的同名函数。修复前 crate 根段永不匹配 src 段，
/// 该策略在 Rust 项目上产出 0 条边（实测 khaslana 自索引 6781 条 CALLS
/// 边中 import_map 占 0）。
#[test]
fn import_map_resolves_rust_crate_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("importmap");
    std::fs::create_dir_all(&root).unwrap();
    let _ = git2::Repository::init(&root);
    write_repo_file(&root, "src/git/service.rs", "pub fn shared_helper() {}\n");
    write_repo_file(&root, "src/git/other.rs", "pub fn shared_helper() {}\n");
    write_repo_file(
        &root,
        "src/main.rs",
        "use crate::git::service;
fn caller() {
    shared_helper();
}
",
    );
    let db_path = tmp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let store = CodeIndexStore::open(&db_path).unwrap();
    let graph = store.load_graph().unwrap();
    let caller = graph
        .nodes
        .iter()
        .find(|n| n.name == "caller")
        .expect("caller");
    let call_edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.source == caller.id && e.etype == EdgeType::Calls)
        .collect();
    assert!(
        call_edges
            .iter()
            .any(|e| graph.get(e.target).file_path == "src/git/service.rs"
                && e.properties.contains("\"import_map\"")),
        "use crate::git::service 应经 import_map 命中 src/git/service.rs，实际边: {:?}",
        call_edges
            .iter()
            .map(|e| (
                graph.get(e.target).qualified_name.clone(),
                e.properties.clone()
            ))
            .collect::<Vec<_>>()
    );
}

/// 回归②：增量删除「引用某 Module 的最后一个导入」后，孤儿 Module 节点
/// 必须被清扫（Module 的 file_path 为空串，purge_files 清不到）。
#[test]
fn incremental_prunes_orphan_module_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("orphan");
    std::fs::create_dir_all(&root).unwrap();
    let _ = git2::Repository::init(&root);
    write_repo_file(
        &root,
        "a.rs",
        "use crate::git::service;
fn alpha() {}
",
    );
    let db_path = tmp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();

    // 重写 a.rs 去掉 use：mtime 变化触发增量。
    write_repo_file(&root, "a.rs", "fn alpha() {}\n");
    run_incremental_if_stale(&root, &db_path, &mut no_cancel_options()).unwrap();

    let store = CodeIndexStore::open(&db_path).unwrap();
    let graph = store.load_graph().unwrap();
    let orphan = graph
        .nodes
        .iter()
        .filter(|n| n.label == NodeLabel::Module && n.name.contains("service"))
        .count();
    assert_eq!(orphan, 0, "孤儿 Module 节点残留");
}

/// 回归③：JS 命名导入的 Module 名必须来自 source 字段，不混入
/// `{ alpha } from` 等噪声（修复前 Module 名是 `{ alpha } from "./lib`）。
#[test]
fn js_import_module_names_are_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("imp");
    std::fs::create_dir_all(&root).unwrap();
    let _ = git2::Repository::init(&root);
    std::fs::write(root.join("lib.js"), "export function alpha() {}\n").unwrap();
    std::fs::write(root.join("util.h"), "int util_fn(void);\n").unwrap();
    std::fs::write(
        root.join("app.js"),
        "import { alpha } from \"./lib\";\nfunction app() { alpha(); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("main.c"),
        "#include \"util.h\"\nint main() { return util_fn(); }\n",
    )
    .unwrap();
    let db_path = tmp.path().join("index.db");
    run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap();
    let store = CodeIndexStore::open(&db_path).unwrap();
    let graph = store.load_graph().unwrap();
    let modules: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| n.label == NodeLabel::Module)
        .map(|n| n.name.clone())
        .collect();
    // JS 相对导入剥 ./ 后为 lib；C 头文件导入为 util.h。
    assert!(
        modules.iter().any(|m| m == "lib"),
        "JS 导入应产出干净的模块名 lib，实际: {modules:?}"
    );
    assert!(
        modules.iter().any(|m| m == "util.h"),
        "C include 应产出 util.h，实际: {modules:?}"
    );
    assert!(
        modules
            .iter()
            .all(|m| !m.contains('{') && !m.contains('"') && !m.contains(" from ")),
        "模块名混入花括号/引号/from 噪声: {modules:?}"
    );
}

/// 回归①补充：策略分布统计（临时挂 --ignored 验证 import_map 真实生效）。
#[test]
#[ignore = "验证用：策略分布统计（cargo test --lib code_index::tests::strategy_distribution -- --ignored --nocapture）"]
fn strategy_distribution() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("self.db");
    let mut options = no_cancel_options();
    run_index(repo_root, &db_path, true, &mut options).unwrap();
    let store = CodeIndexStore::open(&db_path).unwrap();
    let graph = store.load_graph().unwrap();
    let mut strategy_count: std::collections::HashMap<String, usize> = Default::default();
    for e in &graph.edges {
        if e.etype == EdgeType::Calls {
            let s = serde_json::from_str::<serde_json::Value>(&e.properties)
                .ok()
                .and_then(|v| v.get("strategy").cloned())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            *strategy_count.entry(s).or_default() += 1;
        }
    }
    println!("CALLS 策略分布: {strategy_count:?}");
    assert!(
        strategy_distribution_helper(&strategy_count, "import_map"),
        "import_map 应在真实 Rust 仓库上生效，实际: {strategy_count:?}"
    );
}

fn strategy_distribution_helper(map: &std::collections::HashMap<String, usize>, key: &str) -> bool {
    map.get(key).is_some_and(|v| *v > 0)
}

// ---------------------------------------------------------------------------
// 查询 API（queries.rs）：trace / detail / overview / detect_changes
// ---------------------------------------------------------------------------

mod queries_tests {
    use super::*;
    use {DetailOutcome, TraceDirection, TraceOutcome};

    /// 构建一个带调用关系的临时仓库索引：main -> run_task -> helper，
    /// 以及两个同名 free_fn 分布在不同文件（歧义用例）。
    fn build_query_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("queryrepo");
        std::fs::create_dir_all(&root).unwrap();
        let _ = git2::Repository::init(&root);
        write_repo_file(&root, "src/main.rs", "fn main() {\n    run_task();\n}\n");
        write_repo_file(&root, "src/tasks.rs", "fn run_task() {\n    helper();\n}\n");
        write_repo_file(&root, "src/helper.rs", "fn helper() {}\n");
        write_repo_file(&root, "src/a/free.rs", "fn free_fn() {}\n");
        write_repo_file(&root, "src/b/free.rs", "fn free_fn() {}\n");
        // 提交基线：让 detect_changes 用例的「修改 helper.rs」成为唯一变更。
        {
            let repo = git2::Repository::open(&root).unwrap();
            crate::git::test_support::git_test_support::commit_all(&repo, "init");
        }
        let db_path = tmp.path().join("query.db");
        match run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap() {
            RunOutcome::Completed(_) => {}
            other => panic!("{other:?}"),
        }
        (tmp, db_path)
    }

    #[test]
    fn trace_calls_reports_both_directions_with_risk() {
        let (_tmp, db_path) = build_query_fixture();
        match trace_calls(&db_path, "run_task", TraceDirection::Both, 3, 100).unwrap() {
            TraceOutcome::Found(result) => {
                // 上游：main 调 run_task（hop1 CRITICAL）。
                assert!(
                    result
                        .callers
                        .iter()
                        .any(|h| h.name == "main" && h.risk == "CRITICAL")
                );
                // 下游：run_task 调 helper（hop1 CRITICAL）。
                assert!(result.callees.iter().any(|h| h.name == "helper"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn trace_calls_multi_hop_risk_decays() {
        let (_tmp, db_path) = build_query_fixture();
        // 从 helper 向上游追踪两跳：main 在 hop2，风险应为 HIGH。
        match trace_calls(&db_path, "helper", TraceDirection::Inbound, 2, 100).unwrap() {
            TraceOutcome::Found(result) => {
                let main = result
                    .callers
                    .iter()
                    .find(|h| h.name == "main")
                    .expect("main 应出现在两跳内");
                assert_eq!(main.hop, 2);
                assert_eq!(main.risk, "HIGH");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn symbol_detail_reports_ambiguous_for_same_name() {
        let (_tmp, db_path) = build_query_fixture();
        match symbol_detail(&db_path, None, "free_fn").unwrap() {
            DetailOutcome::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn symbol_detail_includes_source_snippet() {
        let (tmp, db_path) = build_query_fixture();
        let root = tmp.path().join("queryrepo");
        match symbol_detail(&db_path, Some(&root), "run_task").unwrap() {
            DetailOutcome::Found(detail) => {
                assert_eq!(detail.name, "run_task");
                let source = detail.source.expect("应读取到源码片段");
                assert!(source.lines.iter().any(|l| l.contains("helper();")));
                assert!(detail.callees.iter().any(|h| h.name == "helper"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn overview_lists_hotspots_and_dirs() {
        let (_tmp, db_path) = build_query_fixture();
        let overview = index_overview(&db_path).unwrap();
        assert!(overview.hotspots.iter().any(|h| h.name == "helper"));
        assert!(overview.top_dirs.iter().any(|(dir, _)| dir == "src"));
    }

    #[test]
    fn detect_changes_maps_worktree_to_impacted_symbols() {
        let (tmp, db_path) = build_query_fixture();
        let root = tmp.path().join("queryrepo");
        // 修改 helper.rs（工作区未提交）→ helper 符号受影响 → 上游 run_task。
        write_repo_file(
            &root,
            "src/helper.rs",
            "fn helper() {\n    println!(\"x\");\n}\n",
        );
        let report = impacted_symbols(&db_path, &root, 1).unwrap();
        assert!(report.changed_files.iter().any(|f| f == "src/helper.rs"));
        assert!(report.impacted_symbols.iter().any(|s| s.name == "helper"));
        // git 变更收集独立可用。
        let changed = changed_files_via_git(&root, None).unwrap();
        assert!(changed.iter().any(|f| f == "src/helper.rs"));
    }

    #[test]
    fn symbol_detail_on_shrunk_file_does_not_panic() {
        // 索引后把文件改短（行数少于已记录的 start_line）——源码片段读取必须
        // 钳制而非越界 panic（MCP 会话长驻时这是真实可达路径）。
        let (tmp, db_path) = build_query_fixture();
        let repo_root = tmp.path().join("queryrepo");
        // 先建索引。
        let mut options = no_cancel_options();
        let RunOutcome::Completed(_) = run_index(&repo_root, &db_path, true, &mut options).unwrap()
        else {
            panic!("应完成");
        };
        // 改短 helper.rs（原 1 行代码；清空后索引里的 start_line 超过文件行数）。
        std::fs::write(repo_root.join("src/helper.rs"), "").unwrap();

        let outcome = symbol_detail(&db_path, Some(&repo_root), "helper").unwrap();
        match outcome {
            DetailOutcome::Found(detail) => {
                // 片段被钳制为 None 或行区间合法，不 panic 即通过。
                if let Some(snippet) = detail.source {
                    assert!(!snippet.lines.is_empty() || !snippet.truncated);
                }
            }
            DetailOutcome::Ambiguous(_) => {}
            DetailOutcome::NotFound => {}
        }
    }

    #[test]
    fn queries_on_missing_index_do_not_create_ghost_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("not-built").join("index.db");
        let err = find_symbol_candidates(&db_path, "anything").unwrap_err();
        assert!(err.to_string().contains("代码索引不存在"), "{err}");
        // 只读打开不创建任何文件/目录。
        assert!(!db_path.exists());
    }

    #[test]
    fn search_symbols_filtered_reports_real_total() {
        let (root, db_path) = build_query_fixture();
        let _ = root;
        // fixture：free_fn 在 a/b 两个目录各一个（同名歧义），run_task/helper 唯一。
        let (_, total) = CodeIndexStore::open(&db_path)
            .unwrap()
            .search_symbols_filtered("free fn", None, 50)
            .unwrap();
        assert_eq!(total, 2, "total 应为过滤后真实总数而非本页条数");
        let (hits, total) = CodeIndexStore::open(&db_path)
            .unwrap()
            .search_symbols_filtered("free fn", Some("Function"), 1)
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(hits.len(), 1, "limit 截断不影响 total");
        let (_, total) = CodeIndexStore::open(&db_path)
            .unwrap()
            .search_symbols_filtered("free fn", Some("Class"), 50)
            .unwrap();
        assert_eq!(total, 0, "label 过滤在 SQL 内完成");
    }
}

// ---------------------------------------------------------------------------
// MCP 协议（mcp.rs::handle_message 纯函数式单测）
// ---------------------------------------------------------------------------

mod mcp_tests {
    use super::super::mcp::McpServer;
    use super::*;
    use serde_json::Value;

    fn server() -> (tempfile::TempDir, McpServer) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("mcprepo");
        std::fs::create_dir_all(&root).unwrap();
        let _ = git2::Repository::init(&root);
        write_repo_file(
            &root,
            "src/lib.rs",
            "fn entry_point() {\n    worker();\n}\n",
        );
        write_repo_file(&root, "src/worker.rs", "fn worker() {}\n");
        {
            let repo = git2::Repository::open(&root).unwrap();
            crate::git::test_support::git_test_support::commit_all(&repo, "init");
        }
        let db_path = tmp.path().join("mcp.db");
        match run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap() {
            RunOutcome::Completed(_) => {}
            other => panic!("{other:?}"),
        }
        let server = McpServer::for_test(&root, db_path);
        (tmp, server)
    }

    fn call_tool(server: &McpServer, name: &str, args: Value) -> Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": "req-1", "method": "tools/call",
            "params": { "name": name, "arguments": args }
        });
        let response = server
            .handle_message(&request.to_string())
            .expect("应有响应");
        let parsed: Value = serde_json::from_str(&response).unwrap();
        parsed["result"].clone()
    }

    #[test]
    fn initialize_negotiates_known_and_falls_back_to_latest() {
        let (_tmp, server) = server();
        // 已知版本回显。
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 7, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" }
        });
        let response: Value =
            serde_json::from_str(&server.handle_message(&request.to_string()).unwrap()).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(
            response["result"]["serverInfo"]["name"],
            "khaslana-code-index"
        );
        // 未知版本回最新。
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 8, "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        });
        let response: Value =
            serde_json::from_str(&server.handle_message(&request.to_string()).unwrap()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn notifications_silent_and_unknown_method_is_error() {
        let (_tmp, server) = server();
        assert!(
            server
                .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        let response: Value = serde_json::from_str(
            &server
                .handle_message(r#"{"jsonrpc":"2.0","id":"x","method":"resources/list"}"#)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(response["id"], "x");
        // 解析失败 → -32700。
        let response: Value =
            serde_json::from_str(&server.handle_message("not-json").unwrap()).unwrap();
        assert_eq!(response["error"]["code"], -32700);
    }

    #[test]
    fn tools_list_exposes_all_tools() {
        let (_tmp, server) = server();
        let response: Value = serde_json::from_str(
            &server
                .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
                .unwrap(),
        )
        .unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "list_projects",
            "search_symbols",
            "get_symbol_detail",
            "trace_path",
            "get_architecture",
            "detect_changes",
            "index_status",
            "refresh_index",
        ] {
            assert!(names.contains(&expected), "{names:?}");
        }
        // 多仓库模式：查询类工具的 schema 都带可选 repo 参数。
        let search = tools
            .iter()
            .find(|t| t["name"] == "search_symbols")
            .unwrap();
        assert!(search["inputSchema"]["properties"]["repo"].is_object());
    }

    #[test]
    fn tool_call_returns_text_and_structured_content() {
        let (_tmp, server) = server();
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker" }),
        );
        assert_eq!(result["isError"], Value::Null);
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "worker")
        );
        // structuredContent 双通道。
        assert!(result["structuredContent"]["results"].is_array());
    }

    #[test]
    fn tool_call_missing_argument_is_business_error() {
        let (_tmp, server) = server();
        let result = call_tool(&server, "search_symbols", serde_json::json!({}));
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("query"));
    }

    #[test]
    fn trace_path_tool_reports_risk() {
        let (_tmp, server) = server();
        let result = call_tool(
            &server,
            "trace_path",
            serde_json::json!({ "function_name": "worker", "direction": "inbound" }),
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["callers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| { c["name"] == "entry_point" && c["risk"] == "CRITICAL" })
        );
    }

    #[test]
    fn detect_changes_tool_lists_worktree_changes() {
        let (tmp, server) = server();
        let root = tmp.path().join("mcprepo");
        write_repo_file(&root, "src/worker.rs", "fn worker() {\n    1 + 1;\n}\n");
        let result = call_tool(
            &server,
            "detect_changes",
            serde_json::json!({ "scope": "symbols" }),
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["changed_files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "src/worker.rs")
        );
        assert!(
            payload["impacted_symbols"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "worker")
        );
    }

    // ------------------------------------------------------------------
    // 多仓库模式（khaslana mcp 无参数启动）
    // ------------------------------------------------------------------

    /// 建一个带 N 个已索引仓库的临时数据目录（code-index/<repo键>/index.db）。
    fn multi_server(repos: usize) -> (tempfile::TempDir, McpServer, Vec<std::path::PathBuf>) {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let mut roots = Vec::new();
        for i in 0..repos {
            let root = tmp.path().join(format!("repo{i}"));
            std::fs::create_dir_all(&root).unwrap();
            let _ = git2::Repository::init(&root);
            write_repo_file(
                &root,
                "src/lib.rs",
                &format!("fn entry_point_{i}() {{\n    worker_{i}();\n}}\n"),
            );
            write_repo_file(&root, "src/worker.rs", &format!("fn worker_{i}() {{}}\n"));
            {
                let repo = git2::Repository::open(&root).unwrap();
                crate::git::test_support::git_test_support::commit_all(&repo, "init");
            }
            // 与 GUI 侧 normalize_repo_path 一致：canonicalize（Windows 产生
            // \\?\ 前缀）+ repo_key 内部小写折叠；MCP context_from_root 按
            // canonicalize 后路径算键，两边必须同源。
            let canonical = std::fs::canonicalize(&root).unwrap();
            let key = crate::ai::review_store::repo_key(&canonical.to_string_lossy());
            let db_path = crate::code_index::open_index_db_path(&data_dir, &key).unwrap();
            match run_index(&root, &db_path, true, &mut no_cancel_options()).unwrap() {
                RunOutcome::Completed(_) => {}
                other => panic!("{other:?}"),
            }
            roots.push(root);
        }
        let server = McpServer::for_multi_test(data_dir);
        (tmp, server, roots)
    }

    #[test]
    fn list_projects_lists_all_indexed_repos_with_paths() {
        let (_tmp, server, roots) = multi_server(2);
        let result = call_tool(&server, "list_projects", serde_json::json!({}));
        assert_eq!(result["isError"], Value::Null);
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["total"], 2);
        let projects = payload["projects"].as_array().unwrap();
        for root in &roots {
            let canonical = std::fs::canonicalize(root).unwrap();
            let key = crate::ai::review_store::repo_key(&canonical.to_string_lossy());
            let entry = projects.iter().find(|p| p["repo"] == key.as_str()).unwrap();
            assert_eq!(entry["repo_path"], root.to_string_lossy().as_ref());
            assert!(entry["symbols"].as_u64().unwrap() > 0);
        }
    }

    #[test]
    fn multi_mode_without_repo_arg_auto_selects_single_project() {
        let (_tmp, server, roots) = multi_server(1);
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker_0" }),
        );
        assert_eq!(result["isError"], Value::Null);
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "worker_0")
        );
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn multi_mode_without_repo_arg_with_multiple_projects_lists_them() {
        let (_tmp, server, _roots) = multi_server(2);
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker_0" }),
        );
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(text.contains("repo 参数"));
        assert_eq!(payload["projects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn multi_mode_repo_arg_accepts_path_and_key() {
        let (_tmp, server, roots) = multi_server(2);
        // 按仓库绝对路径。
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker_1", "repo": roots[1].to_string_lossy() }),
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        let results = payload["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["name"] == "worker_1"));
        assert!(!results.iter().any(|r| r["name"] == "worker_0"));
        // 按 repo 键（list_projects 返回的哈希）。
        let canonical = std::fs::canonicalize(&roots[0]).unwrap();
        let key = crate::ai::review_store::repo_key(&canonical.to_string_lossy());
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker_0", "repo": key }),
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        let results = payload["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["name"] == "worker_0"));
        assert!(!results.iter().any(|r| r["name"] == "worker_1"));
    }

    #[test]
    fn multi_mode_hash_key_context_reads_repo_path_from_meta() {
        // repo 键解析的上下文根目录来自 meta repo_path：detect_changes /
        // refresh_index 等需要根目录的工具应照常工作。
        let (tmp, server, roots) = multi_server(2);
        let canonical = std::fs::canonicalize(&roots[0]).unwrap();
        let key = crate::ai::review_store::repo_key(&canonical.to_string_lossy());
        write_repo_file(
            &roots[0],
            "src/worker.rs",
            "fn worker_0() {\n    2 + 2;\n}\n",
        );
        let result = call_tool(
            &server,
            "detect_changes",
            serde_json::json!({ "scope": "files", "repo": key }),
        );
        assert_eq!(result["isError"], Value::Null);
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["changed_files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "src/worker.rs")
        );
        drop(tmp);
    }

    #[test]
    fn multi_mode_unknown_repo_arg_is_business_error() {
        let (_tmp, server, _roots) = multi_server(1);
        let result = call_tool(
            &server,
            "search_symbols",
            serde_json::json!({ "query": "worker_0", "repo": "D:/不存在的仓库" }),
        );
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("repo 参数"));
    }
}

// ---------------------------------------------------------------------------
// Java 语法探针（DEV-00）：锁定 tree-sitter-java 0.23.5 的实际节点名与字段名，
// 为后续 Java 语义提取（DEV-04/05）提供语法基线；升级语法 crate 版本时须重跑本组。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod java_grammar_probe {
    use tree_sitter::Parser;

    const JAVA_PROBE_SAMPLE: &str = r#"
package com.example.app;

import java.util.List;
import java.util.Map.Entry;
import static java.util.Collections.emptyList;
import static com.example.app.Constants.*;

public abstract class PaymentService implements AutoCloseable {
    private final List<String> log = new java.util.ArrayList<>();

    public PaymentService(int initial) { log.add(String.valueOf(initial)); }

    public void charge(int amount) { log(amount); }
    public void charge(String orderId) { log(orderId); }

    @Override
    public void close() { }

    @SuppressWarnings("unused")
    protected record Receipt(String id, int cents) { }

    public enum Status { OK, FAILED }

    private void log(Object msg) {
        Runnable r = () -> log(msg);
        this.log(msg);
        List<String> copy = List.copyOf(log);
        switch (msg) {
            case Integer i -> charge(i);
            default -> charge(String.valueOf(msg));
        }
    }

    interface Callback { void onDone(boolean ok); }
}
"#;

    fn probe_kinds_and_fields(source: &str) -> (Vec<String>, Vec<(String, Vec<String>)>) {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .expect("Java 语法加载失败");
        let tree = parser.parse(source, None).expect("解析器初始化失败");
        let root = tree.root_node();
        assert!(!root.has_error(), "探针样例不应产生解析错误");
        let mut kinds: Vec<String> = Vec::new();
        let mut fields: Vec<(String, Vec<String>)> = Vec::new();
        let mut cursor = root.walk();
        loop {
            let node = cursor.node();
            if node.is_named() {
                let kind = node.kind().to_string();
                if !kinds.contains(&kind) {
                    kinds.push(kind.clone());
                }
                if cursor.goto_first_child() {
                    let mut seen: Vec<String> = Vec::new();
                    loop {
                        if let Some(f) = cursor.field_name() {
                            let f = f.to_string();
                            if !seen.contains(&f) {
                                seen.push(f);
                            }
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                    cursor.goto_parent();
                    fields.push((kind, seen));
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return (kinds, fields);
                }
            }
        }
    }

    fn kind_field(kind: &str, fields: &[(String, Vec<String>)]) -> Vec<String> {
        // 同名节点可能出现多次（如多个 import_declaration），合并全部字段名。
        let mut merged: Vec<String> = Vec::new();
        for (k, fs) in fields {
            if k == kind {
                for f in fs {
                    if !merged.contains(f) {
                        merged.push(f.clone());
                    }
                }
            }
        }
        merged
    }

    #[test]
    fn java_probe_expected_node_kinds_exist() {
        let (kinds, _) = probe_kinds_and_fields(JAVA_PROBE_SAMPLE);
        for expected in [
            "program",
            "package_declaration",
            "import_declaration",
            "class_declaration",
            "interface_declaration",
            "record_declaration",
            "enum_declaration",
            "method_declaration",
            "constructor_declaration",
            "formal_parameters",
            "method_invocation",
            "object_creation_expression",
            "annotation",
            "marker_annotation",
            "lambda_expression",
            "switch_expression",
        ] {
            assert!(
                kinds.contains(&expected.to_string()),
                "缺少预期节点类型 {expected}；实际包含：{:?}",
                kinds
            );
        }
    }

    #[test]
    fn java_probe_declaration_and_invocation_fields() {
        let (_, fields) = probe_kinds_and_fields(JAVA_PROBE_SAMPLE);
        let method = kind_field("method_declaration", &fields);
        for expected in ["type", "name", "parameters", "body"] {
            assert!(
                method.contains(&expected.to_string()),
                "method_declaration 缺字段 {expected}；实际：{:?}",
                method
            );
        }
        let ctor = kind_field("constructor_declaration", &fields);
        for expected in ["name", "parameters", "body"] {
            assert!(
                ctor.contains(&expected.to_string()),
                "constructor_declaration 缺字段 {expected}；实际：{:?}",
                ctor
            );
        }
        let invocation = kind_field("method_invocation", &fields);
        for expected in ["object", "name", "arguments"] {
            assert!(
                invocation.contains(&expected.to_string()),
                "method_invocation 缺字段 {expected}（receiver 字段名为 object）；实际：{:?}",
                invocation
            );
        }
        let creation = kind_field("object_creation_expression", &fields);
        for expected in ["type", "arguments"] {
            assert!(
                creation.contains(&expected.to_string()),
                "object_creation_expression 缺字段 {expected}；实际：{:?}",
                creation
            );
        }
    }

    #[test]
    fn java_probe_import_forms() {
        // 通配导入是 import_declaration 下名为 asterisk 的 named 子节点（无字段名）；
        // static 导入没有独立节点类型。
        let (kinds, _) = probe_kinds_and_fields(JAVA_PROBE_SAMPLE);
        assert!(
            !kinds.contains(&"static_import_declaration".to_string()),
            "static 导入不应有独立节点类型"
        );
        assert!(
            kinds.contains(&"asterisk".to_string()),
            "通配导入应产出 asterisk named 子节点；实际包含：{:?}",
            kinds
        );
    }
}

// DEV-00 fixture manifest 守卫：只验证人工标注协议和文件闭合，不运行索引。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod code_understanding_fixture_manifest {
    use serde_json::Value;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    const ALLOWED_KINDS: &[&str] = &[
        "calls",
        "dispatch_candidates",
        "overrides",
        "inherits",
        "implements",
        "injects_candidate",
        "handles",
        "maps_to",
        "annotations",
        "statements",
        "selects_constructor",
    ];

    fn fixture_root(suite: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/tests/fixtures/code_understanding")
            .join(suite)
    }

    fn check_suite(suite: &str, prefix: &str) {
        let root = fixture_root(suite);
        let json_path = root.join("expected.json");
        let content = fs::read_to_string(&json_path).expect("fixture expected.json 应可读取");
        let document: Value =
            serde_json::from_str(&content).expect("fixture expected.json 应是合法 JSON");
        assert_eq!(document["suite"], suite);

        let ids = document["question_bank_ids"]
            .as_array()
            .expect("question_bank_ids 必须是数组");
        assert_eq!(ids.len(), 20, "每套题库必须声明 20 个稳定题号");
        let unique_ids: HashSet<&str> = ids.iter().filter_map(Value::as_str).collect();
        assert_eq!(unique_ids.len(), 20, "题号不得重复");
        assert!(unique_ids.iter().all(|id| id.starts_with(prefix)));
        let expected_ids: HashSet<String> = (1..=20).map(|n| format!("{prefix}{n:02}")).collect();
        let actual_ids: HashSet<String> = unique_ids.iter().map(|id| (*id).to_owned()).collect();
        assert_eq!(
            actual_ids, expected_ids,
            "题号必须覆盖 {prefix}01 到 {prefix}20"
        );

        let groups = document["groups"].as_array().expect("groups 必须是数组");
        assert_eq!(groups.len(), 12, "每套 fixture 必须有 12 个分组");
        let mut group_ids = HashSet::new();
        let mut referenced_question_ids = HashSet::new();
        let group_prefix = prefix.strip_suffix('Q').unwrap_or(prefix);
        let expected_group_ids: HashSet<String> =
            (1..=12).map(|n| format!("{group_prefix}{n:02}")).collect();
        let canonical_questions = document["question_bank"]
            .as_array()
            .expect("question_bank 必须是数组");
        assert_eq!(
            canonical_questions.len(),
            20,
            "question_bank 必须完整包含 20 题"
        );
        for question in canonical_questions {
            let id = question["id"]
                .as_str()
                .expect("canonical question.id 必须是字符串");
            assert!(
                expected_ids.contains(id),
                "canonical question.id 必须严格使用稳定题号: {id}"
            );
            let group = question["group"]
                .as_str()
                .expect("canonical question.group 必须是字符串");
            assert!(
                expected_group_ids.contains(group),
                "canonical question.group 必须引用已知分组: {group}"
            );
            assert!(
                !question["q"].as_str().unwrap_or("").trim().is_empty(),
                "canonical question.q 必须是非空字符串"
            );
            let allowed_points = question["allowed_points"]
                .as_array()
                .expect("canonical question.allowed_points 必须是数组");
            assert!(
                !allowed_points.is_empty(),
                "canonical question.allowed_points 不得为空"
            );
            assert!(
                allowed_points
                    .iter()
                    .all(|point| point.as_str().is_some_and(|value| !value.trim().is_empty())),
                "canonical question.allowed_points 项必须是非空字符串"
            );
        }
        let canonical_by_id: std::collections::HashMap<&str, &Value> = canonical_questions
            .iter()
            .map(|question| {
                (
                    question["id"].as_str().expect("canonical question.id 必填"),
                    question,
                )
            })
            .collect();
        assert_eq!(canonical_by_id.len(), 20, "canonical question.id 不得重复");
        let mut covered_files = HashSet::new();
        for group in groups {
            let id = group["id"].as_str().expect("group.id 必填");
            assert!(group_ids.insert(id), "group.id 不得重复: {id}");
            let group_files: HashSet<&str> = group["files"]
                .as_array()
                .expect("group.files 必须是数组")
                .iter()
                .map(|file| file.as_str().expect("group.files 项必须是字符串"))
                .collect();
            for file in group["files"].as_array().expect("group.files 必须是数组") {
                let rel = file.as_str().expect("group.files 项必须是字符串");
                assert!(
                    root.join(rel).is_file(),
                    "fixture 文件不存在: {suite}/{rel}"
                );
                assert!(
                    covered_files.insert(rel),
                    "同一 fixture 文件不得重复列入分组: {suite}/{rel}"
                );
            }
            for question_id in group["question_ids"]
                .as_array()
                .expect("group.question_ids 必须是数组")
            {
                let question_id = question_id.as_str().expect("question_ids 项必须是字符串");
                let canonical = canonical_by_id
                    .get(question_id)
                    .expect("group.question_ids 必须引用 canonical question");
                assert_eq!(canonical["group"], id, "question 必须引用所属 group");
                assert!(
                    referenced_question_ids.insert(question_id.to_owned()),
                    "question_ids 不得重复引用: {question_id}"
                );
            }
            for assertion in group["assertions"]
                .as_array()
                .expect("assertions 必须是数组")
            {
                let kind = assertion["kind"].as_str().expect("assertion.kind 必填");
                assert!(ALLOWED_KINDS.contains(&kind), "未知 assertion.kind: {kind}");
                let certainty = assertion["certainty"]
                    .as_str()
                    .expect("assertion.certainty 必填");
                assert!(
                    ["syntactic", "inferred", "unknown"].contains(&certainty),
                    "未知 certainty: {certainty}"
                );
                assert!(
                    assertion["call_site"].is_string(),
                    "legacy call_site 仍须保留可读定位"
                );
                let source = assertion["source"]
                    .as_object()
                    .expect("assertion.source 必填");
                assert!(
                    source.get("owner").is_some_and(Value::is_string),
                    "source.owner 必填"
                );
                let path = source["path"].as_str().expect("source.path 必填");
                assert!(
                    root.join(path).is_file(),
                    "source.path 文件不存在: {suite}/{path}"
                );
                assert!(
                    group_files.contains(path),
                    "source.path 必须属于当前 group.files: {suite}/{path}"
                );
                assert!(
                    source.get("callee").is_some_and(Value::is_string)
                        || source.get("expression").is_some_and(Value::is_string),
                    "source.callee 或 source.expression 至少一个必填"
                );
                let anchor = source["anchor"].as_str().expect("source.anchor 必填");
                assert!(!anchor.is_empty(), "source.anchor 不得为空");
                let occurrence = source["occurrence"]
                    .as_u64()
                    .expect("source.occurrence 必须是正整数");
                assert!(occurrence > 0, "occurrence 必须从 1 开始");
                let source_text =
                    fs::read_to_string(root.join(path)).expect("source.path 文件应可按 UTF-8 读取");
                let mut found_count = 0;
                let mut offset = 0;
                while let Some(found) = source_text[offset..].find(anchor) {
                    found_count += 1;
                    offset += found + anchor.len();
                }
                assert!(
                    found_count >= occurrence,
                    "source.anchor 在 {suite}/{path} 中出现次数不足: expected >= {occurrence}, actual {found_count}"
                );
                if certainty == "syntactic" {
                    assert!(
                        assertion["target"].is_string()
                            || kind == "statements"
                            || kind == "annotations",
                        "syntactic assertion 必须有 target 或语句/注解证据"
                    );
                }
                if certainty == "inferred" {
                    assert!(
                        assertion["rule"].is_string(),
                        "inferred assertion 必须带 rule"
                    );
                }
                if certainty == "unknown" {
                    assert!(
                        assertion["candidates"].is_array() || assertion["unresolved"].is_boolean(),
                        "unknown assertion 必须带 candidates 或 unresolved"
                    );
                }
            }
        }
        let canonical_id_set: HashSet<&str> = canonical_by_id.keys().copied().collect();
        let referenced_id_set: HashSet<&str> =
            referenced_question_ids.iter().map(String::as_str).collect();
        assert_eq!(
            referenced_id_set, canonical_id_set,
            "20 个 canonical 题目必须都被 group.question_ids 引用"
        );
        let mut disk_files = HashSet::new();
        fn collect_files(root: &Path, dir: &Path, out: &mut HashSet<String>) {
            for entry in fs::read_dir(dir).expect("fixture 子目录应可读取").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_files(root, &path, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .expect("fixture 路径应位于 suite 根内")
                        .to_string_lossy()
                        .replace('\\', "/");
                    if rel != "expected.json" {
                        out.insert(rel);
                    }
                }
            }
        }
        collect_files(&root, &root, &mut disk_files);
        let listed: HashSet<String> = covered_files
            .into_iter()
            .filter(|file| *file != "expected.json")
            .map(str::to_owned)
            .collect();
        assert_eq!(
            listed, disk_files,
            "实际 fixture 文件必须与 files 清单恰好一致"
        );
    }

    #[test]
    fn code_understanding_fixture_manifests_are_closed() {
        check_suite("java", "JQ");
        check_suite("spring", "SQ");
    }
}
