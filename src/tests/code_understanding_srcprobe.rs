// 临时确定性复现：模拟实况里的长序列工具调用，最后统一校验全部发放来源。
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::code_index::{PipelineOptions, RunOutcome, run_index};

fn copy(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        if from.is_dir() {
            copy(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

#[test]
#[ignore]
fn probe_long_session_source_invariants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("p");
    copy(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/tests/fixtures/code_understanding_business/B01"),
        &root,
    );
    git2::Repository::init(&root).unwrap();
    let db = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db, true, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("索引未完成：{other:?}"),
    }
    let tools = UnderstandingTools::open(&root, &db).unwrap();

    // 全部候选（模拟模型反复搜索、逐个 get_symbol）。
    let mut issued: Vec<String> = Vec::new();
    for query in [
        "login", "authenticate", "verify", "create", "record", "matches", "verify",
        "SessionService", "AuthService", "UserRepository", "LoginAttemptRepository",
        "findActiveByUsername", "incrementFailedAttempts", "markLoginSuccess", "executeUpdate",
    ] {
        let search = tools
            .search_symbols(
                "req",
                SearchSymbolsArgs {
                    query: query.to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        for candidate in &search.data.candidates {
            if let Ok(detail) = tools.get_symbol(
                "req-detail",
                GetSymbolArgs {
                    candidate_id: candidate.candidate_id.clone(),
                },
            ) && let Some(source) = &detail.data.source
            {
                issued.push(source.source_id.clone());
            }
        }
    }

    // 多种 read_file 范围（含重叠、超范围、单行）。
    let paths = [
        "src/com/example/login/UserRepository.java",
        "src/com/example/login/AuthService.java",
        "src/com/example/login/SessionService.java",
        "src/com/example/login/LoginController.java",
        "src/com/example/login/AdminLoginProbe.java",
        "src/com/example/login/PasswordVerifier.java",
        "src/com/example/login/PasswordHashLibrary.java",
        "src/com/example/login/LoginAttemptRepository.java",
        "src/com/example/login/LoginRejectedException.java",
    ];
    for path in paths {
        for (start, end) in [(1u32, 200u32), (1, 49), (37, 39), (10, 17), (1, 1), (2, 5)] {
            if let Ok(read) = tools.read_file(
                "req-read",
                ReadFileArgs {
                    path: path.to_string(),
                    start_line: Some(start),
                    end_line: Some(end),
                },
            ) {
                issued.push(read.data.source_id.clone());
            }
        }
    }

    println!("issued total={} unique={}", issued.len(), {
        let mut set = std::collections::BTreeSet::new();
        for id in &issued {
            set.insert(id.clone());
        }
        set.len()
    });

    // 统一在末尾校验：实况里失败正是发生在这一步。
    let mut failures = 0;
    for id in &issued {
        if let Err(error) = tools.validate_source(id) {
            failures += 1;
            println!("FAIL {id}: {error}");
        }
    }
    println!("validation failures: {failures}");
    for (key, computed) in tools.source_service().debug_keys() {
        if key != computed {
            println!("KEY MISMATCH stored={key} computed={computed}");
        }
    }
    assert_eq!(failures, 0, "发放的来源必须全部可通过校验");
}
