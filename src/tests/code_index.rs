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
