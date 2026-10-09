use super::*;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

fn options() -> PipelineOptions {
    PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}))
}

fn fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    for (path, source) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let db = temp.path().join("index.db");
    run_index(&root, &db, true, &mut options()).unwrap();
    (temp, root, db)
}

fn trace(db: &std::path::Path, name: &str, direction: TraceDirection) -> queries::TraceResult {
    let TraceOutcome::Found(result) = trace_calls(db, name, direction, 1, 100).unwrap() else { panic!("未找到 {name}"); };
    result
}

#[test]
fn java_receiver_is_preserved_and_unrelated_private_methods_have_no_callers() {
    let mut extractor = Extractor::new();
    let source = "class Client extends MissingBase { void run() { this.newResponseContext(); Integer.parseInt(\"1\"); text.substring(0); } }";
    let result = extractor.extract(LangId::Java, source.as_bytes()).unwrap().unwrap();
    let calls: Vec<_> = result.calls.iter().map(|call| call.callee_display.as_str()).collect();
    assert_eq!(calls, ["this.newResponseContext", "Integer.parseInt", "text.substring"]);
    let (_temp, _root, db) = fixture(&[
        ("java/PrivateOwner.java", "class PrivateOwner { private Object newResponseContext() { return null; } }"),
        ("java/Client.java", source),
    ]);
    assert_eq!(trace(&db, "newResponseContext", TraceDirection::Inbound).callers_total, 0);
}

#[test]
fn java_implicit_calls_stay_in_their_class_and_real_static_calls_are_preserved() {
    let (_temp, _root, db) = fixture(&[
        ("java/Tools.java", "class Tools { private void hidden() {} void own() { hidden(); } public static int parseInt(String s) { return Integer.parseInt(s); } public static String substring(String s) { return s.substring(0); } }"),
        ("java/Caller.java", "class Caller { void run() { hidden(); Tools.hidden(); Tools.parseInt(\"1\"); Integer.parseInt(\"1\"); text.substring(0); } }"),
    ]);
    let hidden = trace(&db, "hidden", TraceDirection::Inbound);
    assert_eq!(hidden.callers_total, 1);
    assert_eq!(hidden.callers[0].name, "own");
    let parse = trace(&db, "parseInt", TraceDirection::Inbound);
    assert_eq!(parse.callers_total, 1);
    assert_eq!(parse.callers[0].name, "run");
    assert_eq!(trace(&db, "substring", TraceDirection::Inbound).callers_total, 0);
}

#[test]
fn calls_never_cross_from_java_to_javascript_and_ts_can_import_js() {
    let (_temp, _root, db) = fixture(&[
        ("web/HashMap.js", "function HashMap() {} function info() {}"),
        ("java/Client.java", "import java.util.HashMap; class Client { void run() { new HashMap(); logger.info(\"hello\"); } }"),
        ("web/use.ts", "import { info } from './HashMap'; export function use() { info(); }"),
    ]);
    assert_eq!(trace(&db, "HashMap", TraceDirection::Inbound).callers_total, 0);
    let info = trace(&db, "info", TraceDirection::Inbound);
    assert_eq!(info.callers_total, 1);
    assert_eq!(info.callers[0].name, "use");
}

#[test]
fn file_names_and_language_statistics_use_the_actual_basename() {
    let (_temp, _root, db) = fixture(&[
        ("src/main/java/Sample.java", "class Sample {}"),
        ("web/main.js", "function run() {}"),
    ]);
    let overview = index_overview(&db).unwrap();
    assert_eq!(overview.languages, vec![("java".into(), 1), ("js".into(), 1)]);
    let store = CodeIndexStore::open(&db).unwrap();
    let name: String = store.conn.query_row("SELECT name FROM nodes WHERE label='File' AND file_path='src/main/java/Sample.java'", [], |row| row.get(0)).unwrap();
    assert_eq!(name, "Sample.java");
}

#[test]
fn top_level_test_callbacks_have_separate_owners() {
    let (_temp, _root, db) = fixture(&[
        ("src/policy.ts", "export function policy() {}"),
        ("tests/policy.spec.ts", "import { policy } from '../src/policy';\nit('first', () => { policy(); policy(); });\nit('second', () => { policy(); });\n"),
    ]);
    let callers = trace(&db, "policy", TraceDirection::Inbound);
    assert_eq!(callers.callers_total, 2);
    assert!(callers.callers.iter().all(|caller| caller.name.starts_with("callback@")));
    assert_ne!(callers.callers[0].qualified_name, callers.callers[1].qualified_name);
    let mut counts: Vec<_> = callers.callers.iter().map(|caller| {
        let sites = caller.call_sites.as_ref().expect("直接调用边应有位置证据");
        assert_eq!(sites.file_path, "tests/policy.spec.ts");
        assert_eq!(sites.lines.len(), sites.count);
        assert!(!sites.truncated);
        sites.count
    }).collect();
    counts.sort();
    assert_eq!(counts, [1, 2]);
}

#[test]
fn repeated_calls_keep_exact_counts_bounded_lines_and_survive_incremental_reload() {
    let source = format!("fn target() {{}}\nfn caller() {{\n{}\n}}", "target();\n".repeat(120));
    let (_temp, root, db) = fixture(&[("src/lib.rs", &source), ("src/other.rs", "fn other() {}")]);
    let check = || {
        let result = trace(&db, "target", TraceDirection::Inbound);
        assert_eq!(result.callers_total, 1);
        let sites = result.callers[0].call_sites.as_ref().unwrap();
        assert_eq!(sites.count, 120);
        assert_eq!(sites.lines, (3..103).collect::<Vec<_>>());
        assert!(sites.truncated);
    };
    check();
    // 新增同名定义让未修改调用文件重解，随后移除定义，验证持久化行号没有丢失。
    std::fs::write(root.join("src/other.rs"), "fn target() {}\n").unwrap();
    run_index(&root, &db, false, &mut options()).unwrap();
    std::fs::write(root.join("src/other.rs"), "fn other_again() {}\n").unwrap();
    run_index(&root, &db, false, &mut options()).unwrap();
    check();
}

#[test]
fn coverage_separates_absent_repository_targets_from_uncertain_candidates() {
    let (_temp, _root, db) = fixture(&[
        ("src/main.ts", "function info() {}\nfunction run() { console.info('x'); Promise.resolve(); }"),
    ]);
    let coverage = check_coverage(&db, None, &["src/main.ts".into()], &[], 0, 10).unwrap();
    assert_eq!(coverage["results"][0]["call_sites"], 2);
    assert_eq!(coverage["results"][0]["unresolved_calls"], 2);
    assert_eq!(coverage["results"][0]["unresolved_no_candidate"], 1);
    assert_eq!(coverage["results"][0]["unresolved_with_candidates"], 1);
    let stats = read_index_stats(&db).unwrap().unwrap();
    assert_eq!(stats.coverage["unresolved_no_candidate"], 1);
    assert_eq!(stats.coverage["unresolved_with_candidates"], 1);
}

#[test]
fn java_only_explicit_static_imports_allow_bare_cross_class_calls() {
    let (_temp, _root, db) = fixture(&[
        ("src/pkg/Tools.java", "package pkg; public class Tools { public static void target() {} public static void another() {} }"),
        ("src/pkg/Normal.java", "package pkg; import pkg.Tools; class Normal { void bad() { target(); } }"),
        ("src/pkg/Specific.java", "package pkg; import static pkg.Tools.target; class Specific { void good() { target(); another(); } }"),
        ("src/pkg/Wild.java", "package pkg; import static pkg.Tools.*; class Wild { void goodAll() { target(); another(); } }"),
    ]);
    let target = trace(&db, "target", TraceDirection::Inbound);
    let names: Vec<_> = target.callers.iter().map(|caller| caller.name.as_str()).collect();
    assert_eq!(names, ["good", "goodAll"]);
    let another = trace(&db, "another", TraceDirection::Inbound);
    assert_eq!(another.callers_total, 1);
    assert_eq!(another.callers[0].name, "goodAll");
}

#[test]
fn version_five_pollution_is_not_served_and_is_rebuilt_once_without_file_edits() {
    let (_temp, root, db) = fixture(&[
        ("web/map.js", "function HashMap() {}"),
        ("src/Use.java", "class Use { void run() { new HashMap(); } }"),
    ]);
    let store = CodeIndexStore::open(&db).unwrap();
    store.conn.execute("INSERT INTO edges(source_id,target_id,type,properties) SELECT s.id,t.id,'CALLS','{}' FROM nodes s,nodes t WHERE s.name='run' AND t.name='HashMap'", []).unwrap();
    store.conn.execute("UPDATE meta SET value='5' WHERE key='search_content_version'", []).unwrap();
    assert!(read_index_stats(&db).unwrap().unwrap().needs_rebuild);
    assert!(trace_calls(&db, "HashMap", TraceDirection::Inbound, 1, 10).unwrap_err().to_string().contains("需重建"));
    let generation = index_generation(&db).unwrap();
    run_index(&root, &db, false, &mut options()).unwrap();
    assert_ne!(index_generation(&db).unwrap(), generation);
    assert!(!read_index_stats(&db).unwrap().unwrap().needs_rebuild);
    assert_eq!(trace(&db, "HashMap", TraceDirection::Inbound).callers_total, 0);
    assert!(matches!(run_index(&root, &db, false, &mut options()).unwrap(), RunOutcome::Unchanged));
}

#[test]
fn call_record_codec_keeps_lines_and_reads_the_previous_format() {
    let record = calls::FileCalls { imports: vec!["pkg.Tools".into()], calls: vec![calls::StoredCall {
        source_qn: "p.caller".into(), callee_display: "Tools.target".into(), name: "target".into(), scope: "Caller".into(), line: 37,
    }] };
    let bytes = record.encode().unwrap();
    assert_eq!(calls::FileCalls::decode(&bytes).unwrap().calls[0].line, 37);
    let mut old = bytes;
    old[..6].copy_from_slice(b"KCALL1");
    old.truncate(old.len() - 4);
    let decoded = calls::FileCalls::decode(&old).unwrap();
    assert_eq!(decoded.calls[0].callee_display.as_ref(), "Tools.target");
    assert_eq!(decoded.calls[0].line, 0);
}

#[test]
fn ambiguous_local_java_overloads_do_not_fall_through_to_imported_methods() {
    let (_temp, _root, db) = fixture(&[
        ("src/pkg/Remote.java", "package pkg; class Remote { public static void target() {} }"),
        ("src/pkg/Local.java", "package pkg; import static pkg.Remote.target; class Local { void target(int a) {} void target(String a) {} void run() { target(1); } }"),
    ]);
    let remote = search_symbols(&db, "target", 10).unwrap().into_iter().find(|hit| hit.file_path.ends_with("Remote.java")).unwrap();
    assert_eq!(trace(&db, &remote.qualified_name, TraceDirection::Inbound).callers_total, 0);
}
