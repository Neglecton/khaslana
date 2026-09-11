use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tempfile::TempDir;

use super::*;
use crate::ai::review_store::repo_key;
use crate::code_index::{PipelineOptions, ProjectContext, RunOutcome, run_index};

fn write_file(root: &Path, relative_path: &str, content: &[u8]) {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn source_fixture() -> (TempDir, PathBuf, SourceService) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source-project");
    std::fs::create_dir_all(&root).unwrap();
    write_file(
        &root,
        "src/AuthService.java",
        b"class AuthService {\n  boolean login(String name) {\n    return name != null;\n  }\n}\n",
    );
    write_file(
        &root,
        "resources/mapper/UserMapper.xml",
        b"<mapper>\n<select id=\"findUser\">SELECT * FROM app_user WHERE name = #{name}</select>\n</mapper>\n",
    );
    write_file(&root, ".git/config", b"secret = should-not-be-readable\n");
    let canonical = std::fs::canonicalize(&root).unwrap();
    let context = ProjectContext {
        project_key: repo_key(&canonical.to_string_lossy()),
        canonical_root: canonical.to_string_lossy().into_owned(),
        index_db_path: temp.path().join("index.db").to_string_lossy().into_owned(),
    };
    let source = SourceService::open(context, 7).unwrap();
    (temp, root, source)
}

#[test]
fn source_reader_issues_and_revalidates_source_ids() {
    let (_temp, root, source) = source_fixture();
    let read = source
        .read_file("resources/mapper/UserMapper.xml", 2, 2)
        .unwrap();
    assert!(read.source_id.starts_with("sr1:"));
    assert_eq!(read.source_ref.generation, 7);
    assert_eq!(read.source_ref.start_line, 2);
    assert!(read.lines[0].text.contains("app_user"));
    assert_eq!(
        source.validate_source(&read.source_id).unwrap(),
        read.source_ref
    );

    write_file(
        &root,
        "resources/mapper/UserMapper.xml",
        b"<mapper>changed</mapper>\n",
    );
    let error = source.validate_source(&read.source_id).unwrap_err();
    assert_eq!(error.code, ErrorCode::SourceChanged);
}

#[test]
fn source_reader_rejects_paths_outside_allowlist() {
    let (_temp, _root, source) = source_fixture();
    let traversal = source.read_file("../outside.txt", 1, 1).unwrap_err();
    assert_eq!(traversal.code, ErrorCode::OutsideProject);

    let git_file = source.read_file(".git/config", 1, 1).unwrap_err();
    assert_eq!(git_file.code, ErrorCode::ExcludedPath);

    let forged = source
        .validate_source(&format!("sr1:{}", "0".repeat(64)))
        .unwrap_err();
    assert_eq!(forged.code, ErrorCode::AnswerInvalid);
}

#[test]
fn source_reader_never_follows_a_root_outside_link() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("link-project");
    std::fs::create_dir_all(&root).unwrap();
    let outside = temp.path().join("outside.java");
    std::fs::write(&outside, "class Outside {}\n").unwrap();
    let link = root.join("Outside.java");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&outside, &link).is_ok();
    if !linked {
        return;
    }
    let canonical = std::fs::canonicalize(&root).unwrap();
    let source = SourceService::open(
        ProjectContext {
            project_key: repo_key(&canonical.to_string_lossy()),
            canonical_root: canonical.to_string_lossy().into_owned(),
            index_db_path: temp.path().join("index.db").to_string_lossy().into_owned(),
        },
        1,
    )
    .unwrap();
    let error = source.read_file("Outside.java", 1, 1).unwrap_err();
    assert!(matches!(
        error.code,
        ErrorCode::ExcludedPath | ErrorCode::OutsideProject
    ));
}

#[test]
fn source_reader_enforces_line_character_binary_and_size_limits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("limits-project");
    std::fs::create_dir_all(&root).unwrap();
    let many_lines = (1..=250)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    write_file(&root, "src/many.txt", many_lines.as_bytes());
    write_file(&root, "src/binary.dat", b"text\0binary");
    write_file(
        &root,
        "src/large.txt",
        &vec![b'x'; SOURCE_FILE_MAX_BYTES as usize + 1],
    );
    let canonical = std::fs::canonicalize(&root).unwrap();
    let source = SourceService::open(
        ProjectContext {
            project_key: repo_key(&canonical.to_string_lossy()),
            canonical_root: canonical.to_string_lossy().into_owned(),
            index_db_path: temp.path().join("index.db").to_string_lossy().into_owned(),
        },
        1,
    )
    .unwrap();

    let read = source.read_file("src/many.txt", 1, 250).unwrap();
    assert_eq!(read.lines.len(), SOURCE_READ_MAX_LINES);
    assert!(read.truncated);
    assert!(read.truncation_reason.unwrap().contains("200"));

    let binary = source.read_file("src/binary.dat", 1, 1).unwrap_err();
    assert_eq!(binary.code, ErrorCode::EncodingUnsupported);
    let large = source.read_file("src/large.txt", 1, 1).unwrap_err();
    assert_eq!(large.code, ErrorCode::BudgetExceeded);
}

#[test]
fn source_search_and_tree_include_non_indexed_xml() {
    let (_temp, _root, source) = source_fixture();
    let result = source
        .search_code("app_user", false, Some("resources"), Some(".xml"), 20)
        .unwrap();
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 2);
    assert_eq!(
        result.matches[0].relative_path,
        "resources/mapper/UserMapper.xml"
    );

    let tree = source.get_file_tree(Some("resources"), 4, 200).unwrap();
    assert!(
        tree.entries
            .iter()
            .any(|entry| entry.relative_path == "resources/mapper/UserMapper.xml")
    );
    assert!(
        !tree
            .entries
            .iter()
            .any(|entry| entry.relative_path.contains(".git"))
    );
}

fn indexed_fixture() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("indexed-project");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    write_file(
        &root,
        "src/main.rs",
        b"fn main() { login(); }\nfn login() { check_password(); }\nfn check_password() {}\n",
    );
    write_file(
        &root,
        "resources/UserMapper.xml",
        b"<select>SELECT * FROM app_user</select>\n",
    );
    let db_path = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db_path, true, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("索引未完成：{other:?}"),
    }
    (temp, root, db_path)
}

/// 含一个类节点的索引样例：验证非可调用候选的 trace 报错。
fn class_fixture() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("class-project");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    write_file(
        &root,
        "src/session.rs",
        b"pub struct Session {
    pub id: u64,
}

impl Session {
    pub fn create(id: u64) -> Session {
        Session { id }
    }
}
",
    );
    let db_path = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db_path, true, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("索引未完成：{other:?}"),
    }
    (temp, root, db_path)
}

#[test]
fn six_tool_session_uses_registered_candidates_and_safe_sources() {
    let (_temp, root, db_path) = indexed_fixture();
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    let search = tools
        .search_symbols(
            "req-search",
            SearchSymbolsArgs {
                query: "login".to_string(),
                path_prefix: Some("src".to_string()),
                language: Some("rust".to_string()),
                limit: Some(20),
            },
        )
        .unwrap();
    assert_eq!(search.request_id, "req-search");
    assert_eq!(search.data.candidates.len(), 1, "{search:?}");
    let candidate_id = search.data.candidates[0].candidate_id.clone();

    let detail = tools
        .get_symbol(
            "req-detail",
            GetSymbolArgs {
                candidate_id: candidate_id.clone(),
            },
        )
        .unwrap();
    assert_eq!(detail.data.name, "login");
    let source = detail.data.source.expect("定义应可通过安全 reader 读取");
    assert!(source.lines.iter().any(|line| line.text.contains("login")));
    tools
        .source_service()
        .validate_source(&source.source_id)
        .unwrap();

    let trace = tools
        .trace_calls(
            "req-trace",
            TraceCallsArgs {
                candidate_id,
                direction: CallTraceDirection::Both,
                depth: Some(1),
                limit: Some(20),
            },
        )
        .unwrap();
    assert!(trace.data.callers.iter().any(|hop| hop.name == "main"));
    assert!(
        trace
            .data
            .callees
            .iter()
            .any(|hop| hop.name == "check_password")
    );
    assert!(trace.data.coverage_note.contains("无边不表示"));

    let code = tools
        .search_code(
            "req-code",
            SearchCodeArgs {
                query: "app_user".to_string(),
                regex: false,
                path_prefix: Some("resources".to_string()),
                suffix: Some(".xml".to_string()),
                limit: Some(20),
            },
        )
        .unwrap();
    assert_eq!(code.data.matches.len(), 1);

    let read = tools
        .read_file(
            "req-read",
            ReadFileArgs {
                path: "resources/UserMapper.xml".to_string(),
                start_line: Some(1),
                end_line: Some(1),
            },
        )
        .unwrap();
    assert!(read.data.lines[0].text.contains("app_user"));

    let tree = tools
        .get_file_tree(
            "req-tree",
            GetFileTreeArgs {
                path: None,
                depth: Some(2),
                limit: Some(200),
            },
        )
        .unwrap();
    assert!(
        tree.data
            .entries
            .iter()
            .any(|entry| entry.relative_path == "src/main.rs")
    );

    let fake = tools
        .get_symbol(
            "req-fake",
            GetSymbolArgs {
                candidate_id: format!("sc1:{}", "0".repeat(64)),
            },
        )
        .unwrap_err();
    assert_eq!(fake.code, ErrorCode::AmbiguousSymbol);

    let schemas = tool_schemas();
    assert_eq!(schemas.len(), 6);
    assert_eq!(
        schemas.iter().map(|schema| schema.name).collect::<Vec<_>>(),
        vec![
            "search_symbols",
            "get_symbol",
            "trace_calls",
            "search_code",
            "read_file",
            "get_file_tree"
        ]
    );
}

#[test]
fn indexed_symbol_source_is_not_mixed_with_changed_worktree_content() {
    let (_temp, root, db_path) = indexed_fixture();
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    let search = tools
        .search_symbols(
            "req-search",
            SearchSymbolsArgs {
                query: "login".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    let candidate_id = search.data.candidates[0].candidate_id.clone();
    write_file(
        &root,
        "src/main.rs",
        b"fn main() { login(); }\n\nfn login() { changed(); }\nfn changed() {}\n",
    );
    let detail = tools
        .get_symbol("req-detail", GetSymbolArgs { candidate_id })
        .unwrap();
    assert!(detail.data.source.is_none());
    assert!(
        detail
            .data
            .source_unavailable_reason
            .unwrap()
            .contains("SourceChanged")
    );
}

#[test]
fn tool_session_rejects_a_new_index_generation() {
    let (_temp, root, db_path) = indexed_fixture();
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    write_file(
        &root,
        "src/extra.rs",
        b"fn added_after_session_started() {}\n",
    );
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db_path, false, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("增量索引未完成：{other:?}"),
    }
    let error = tools
        .get_file_tree(
            "req-tree",
            GetFileTreeArgs {
                path: None,
                depth: None,
                limit: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::GenerationMismatch);
}

#[test]
fn trace_on_a_non_callable_candidate_explains_the_real_cause() {
    // trace_calls 只解析函数/方法；类/字段候选返回 NotFound 时不能报成
    // 「索引代际变化」——实况验收里模型因此误以为索引失效而放弃调用链。
    assert!(super::tools::is_callable_label("Method"));
    assert!(super::tools::is_callable_label("Function"));
    assert!(!super::tools::is_callable_label("Class"));
    assert!(!super::tools::is_callable_label("Field"));

    let (_temp, root, db_path) = class_fixture();
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    let search = tools
        .search_symbols(
            "req-class",
            SearchSymbolsArgs {
                query: "Session".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    let class = search
        .data
        .candidates
        .iter()
        .find(|candidate| matches!(candidate.label.as_str(), "Struct" | "Class"))
        .expect("Session 应为结构体/类候选");
    let error = tools
        .trace_calls(
            "req-trace-class",
            TraceCallsArgs {
                candidate_id: class.candidate_id.clone(),
                direction: CallTraceDirection::Inbound,
                depth: None,
                limit: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::CursorExpired);
    assert!(
        error.message.contains("不是可调用符号"),
        "应指明真实原因而不是索引失效：{}",
        error.message
    );
    assert!(
        !error.message.contains("索引"),
        "不得误导成索引问题：{}",
        error.message
    );
}

#[test]
fn source_ids_are_short_and_accept_a_unique_prefix() {
    // 真实模型无法可靠转录长十六进制串（实测抄错长度）。来源 ID 因此缩短到
    // 8 位，并允许按唯一前缀兜底匹配，但仍然拒绝未发放过的 ID。
    let (_temp, _root, source) = source_fixture();
    let read = source
        .read_file("resources/mapper/UserMapper.xml", 2, 2)
        .unwrap();
    let id = &read.source_id;
    assert!(id.starts_with("sr1:"));
    let digest = &id[4..];
    assert_eq!(digest.len(), 8, "来源 ID 摘要应为 8 位：{id}");

    // 完整 ID 与足够长的唯一前缀都能解析到同一条来源。
    assert_eq!(source.validate_source(id).unwrap(), read.source_ref);
    let prefix = &id[..id.len() - 1];
    assert_eq!(
        source.validate_source(prefix).unwrap(),
        read.source_ref,
        "唯一前缀应可解析"
    );

    // 太短的前缀与伪造 ID 仍被拒绝（不能凭猜测套到别的来源）。
    assert!(source.validate_source("sr1:0").is_err());
    assert!(source.validate_source("sr1:ffffffff").is_err());
}

#[test]
fn candidate_ids_are_short_and_accept_a_unique_prefix() {
    // 与来源 ID 同一类问题：长候选 ID 会被模型抄错。候选 ID 因此也改为短序号，
    // 并允许唯一前缀匹配，但仍拒绝未注册过的 ID。
    let (_temp, root, db_path) = indexed_fixture();
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    let search = tools
        .search_symbols(
            "req-search",
            SearchSymbolsArgs {
                query: "login".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    let candidate = &search.data.candidates[0];
    let id = &candidate.candidate_id;
    assert!(id.starts_with("sc1:"));
    assert_eq!(id[4..].len(), 1, "首个候选应为一号短 ID：{id}");

    // 完整 ID 与唯一前缀都能解析到同一符号。
    let full = tools
        .get_symbol(
            "req-full",
            GetSymbolArgs {
                candidate_id: id.clone(),
            },
        )
        .unwrap();
    assert_eq!(full.data.name, "login");

    // 多个候选时前缀必须唯一：'1' 只对应 sc1:1。
    let unknown = tools
        .get_symbol(
            "req-unknown",
            GetSymbolArgs {
                candidate_id: "sc1:99".to_string(),
            },
        )
        .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::AmbiguousSymbol);
    assert!(unknown.message.contains("不是本次会话搜索结果"));
}
