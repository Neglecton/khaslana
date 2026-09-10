// CU2 真实模型业务验收（开发文档 §7）。
//
// 分两层：
// 1. 离线测试（普通 `cargo test` 执行）：B02/B03 样例能建索引、能被六个工具导航；
//    不联网、不调用模型。
// 2. 实况测试（`#[ignore]`）：用**本机已配置的** AI 供应商真跑登录问答，把结果与
//    工具轨迹写入 docs/code-understanding/validation/live-runs/。执行方式：
//
//        cargo test --lib code_understanding_live -- --ignored --nocapture
//
// 真实模型不可用（未配置/未启用）时测试打印原因并跳过，不伪造通过；业务验收是否
// 完成以 live-runs 下的报告为准（开发文档 §7：mock 成功不能替代真实模型评估）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::code_understanding::{
    AnalysisResult, ChatTurnProvider, DataObjectCategory, SearchCodeArgs, SearchSymbolsArgs,
    UnderstandingAgentInput, UnderstandingAnswer, UnderstandingEvent, UnderstandingStep,
    UnderstandingTools, run_understanding_agent,
};
use crate::code_understanding::ReadFileArgs;
use crate::ai::{AiProviderSettings, ChatClient};
use crate::code_index::{PipelineOptions, RunOutcome, run_index};
use crate::storage::AppStorage;

fn copy_dir(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = target.join(&file_name);
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// 把样例复制到临时目录并建立索引。
fn prepared_fixture(case: &str) -> (TempDir, PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/code_understanding_business")
        .join(case);
    assert!(fixture.is_dir(), "缺少样例：{}", fixture.display());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(case.to_ascii_lowercase());
    copy_dir(&fixture, &root);
    git2::Repository::init(&root).unwrap();
    let db_path = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db_path, true, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("{case} 索引未完成：{other:?}"),
    }
    (temp, root, db_path)
}

fn expected_cases() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/code_understanding_business/expected.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn case_question(case: &str) -> String {
    let expected = expected_cases();
    expected["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == case)
        .and_then(|entry| entry["question"].as_str())
        .expect("expected.json 应有该样例的问题")
        .to_string()
}

// ── 离线：样例可索引、可被工具导航（不联网） ────────────────────────────────

#[test]
fn b02_mybatis_fixture_indexes_and_exposes_mapper_xml() {
    let (_temp, root, db_path) = prepared_fixture("B02");
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();

    // 核心方法与入口是符号。
    let service = tools
        .search_symbols(
            "req",
            SearchSymbolsArgs {
                query: "AuthService".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        service
            .data
            .candidates
            .iter()
            .any(|candidate| candidate.relative_path.ends_with("AuthService.java")),
        "应找到 AuthService：{:?}",
        service.data
    );

    // Mapper XML 没有符号节点，但 search_code / read_file 必须能取到。
    let xml = tools
        .search_code(
            "req-xml",
            SearchCodeArgs {
                query: "app_user".to_string(),
                regex: false,
                path_prefix: Some("src/main/resources".to_string()),
                suffix: Some(".xml".to_string()),
                limit: Some(20),
            },
        )
        .unwrap();
    assert!(
        xml.data.matches.iter().any(|hit| hit.relative_path.ends_with("UserMapper.xml")),
        "应能在 XML 中搜到 app_user：{:?}",
        xml.data
    );
    let read = tools
        .read_file(
            "req-read",
            ReadFileArgs {
                path: "src/main/resources/mapper/LoginLogMapper.xml".to_string(),
                start_line: Some(1),
                end_line: Some(10),
            },
        )
        .unwrap();
    assert!(
        read.data.lines.iter().any(|line| line.text.contains("INSERT INTO login_log")),
        "应读到 login_log 插入语句：{:?}",
        read.data
    );
}

#[test]
fn b03_jpa_fixture_exposes_entity_mapping_and_repository() {
    let (_temp, root, db_path) = prepared_fixture("B03");
    let tools = UnderstandingTools::open(&root, &db_path).unwrap();

    // @Table(name = "app_user") 的映射证据必须可读。
    let entity = tools
        .read_file(
            "req-entity",
            ReadFileArgs {
                path: "src/main/java/com/example/crm/entity/UserEntity.java".to_string(),
                start_line: Some(1),
                end_line: Some(40),
            },
        )
        .unwrap();
    assert!(
        entity.data.lines.iter().any(|line| line.text.contains("@Table(name = \"app_user\")")),
        "应读到 @Table 映射：{:?}",
        entity.data
    );

    // LoginAudit 没有 @Table：工具只应如实读出它，不能替模型推断表名。
    let audit = tools
        .read_file(
            "req-audit",
            ReadFileArgs {
                path: "src/main/java/com/example/crm/entity/LoginAudit.java".to_string(),
                start_line: Some(1),
                end_line: Some(30),
            },
        )
        .unwrap();
    assert!(
        audit.data.lines.iter().all(|line| !line.text.contains("@Table")),
        "样例要求 LoginAudit 无显式表映射：{:?}",
        audit.data
    );

    let repository = tools
        .search_code(
            "req-repo",
            SearchCodeArgs {
                query: "findByUsernameAndEnabledTrue".to_string(),
                regex: false,
                path_prefix: None,
                suffix: Some(".java".to_string()),
                limit: Some(20),
            },
        )
        .unwrap();
    assert!(
        repository
            .data
            .matches
            .iter()
            .any(|hit| hit.relative_path.ends_with("UserRepository.java")),
        "应找到派生查询方法：{:?}",
        repository.data
    );
}

#[test]
fn expected_json_covers_three_main_samples() {
    let expected = expected_cases();
    let ids: Vec<&str> = expected["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    // B01～B03 主样例就位；B04/B05 边界变体仍是 T3 剩余项。
    assert_eq!(ids, vec!["B01", "B02", "B03"]);
}

// ── 实况：真实模型跑登录问答并留报告 ───────────────────────────────────────

/// 读取本机 AI 配置；未配置/未启用时返回原因。
///
/// 真实模型验收不应修改用户保存的设置（本机存档里可能是已下线的模型名）。
/// 允许用环境变量临时覆盖，沿用用户配置里的 Base URL / API Key：
/// - `KHASLANA_LIVE_MODEL`：覆盖模型名；
/// - `KHASLANA_LIVE_BASE_URL`：覆盖 Base URL（可选）。
fn live_settings() -> Result<(AiProviderSettings, Option<String>), String> {
    let storage = AppStorage::open_default().map_err(|error| format!("无法打开配置库：{error}"))?;
    let mut settings = storage
        .load_ai_provider_settings()
        .map_err(|error| format!("无法读取 AI 配置：{error}"))?;
    if let Ok(model) = std::env::var("KHASLANA_LIVE_MODEL")
        && !model.trim().is_empty()
    {
        settings.model = model.trim().to_string();
    }
    if let Ok(base_url) = std::env::var("KHASLANA_LIVE_BASE_URL")
        && !base_url.trim().is_empty()
    {
        settings.base_url = base_url.trim().to_string();
    }
    if !settings.is_usable() {
        return Err(format!(
            "AI 供应商未配置或未启用（enabled={}, model={:?}）",
            settings.enabled, settings.model
        ));
    }
    let proxy = storage
        .load_proxy_settings()
        .ok()
        .and_then(|proxy| proxy.proxy_url_for_target(&settings.normalized_base_url()));
    Ok((settings, proxy))
}

fn describe_steps(steps: &[UnderstandingStep]) -> Vec<serde_json::Value> {
    steps
        .iter()
        .map(|step| match step {
            UnderstandingStep::Reasoning { text } => serde_json::json!({
                "kind": "reasoning",
                "chars": text.chars().count()
            }),
            UnderstandingStep::Message { text } => serde_json::json!({
                "kind": "message",
                "text": text
            }),
            UnderstandingStep::ToolCall {
                name,
                args_summary,
                result_excerpt,
                error,
            } => serde_json::json!({
                "kind": "tool_call",
                "name": name,
                "args": args_summary,
                "error": error,
                "result_head": result_excerpt.chars().take(400).collect::<String>()
            }),
        })
        .collect()
}

/// 执行一次实况问答；未配置模型时返回 `Ok(None)`（跳过，不算通过）。
fn run_live(
    case: &str,
    question: &str,
    run_index_number: usize,
) -> Result<Option<(UnderstandingAnswer, Duration)>, String> {
    let (settings, proxy) = match live_settings() {
        Ok(value) => value,
        Err(reason) => {
            eprintln!("跳过 {case} run{run_index_number}：{reason}");
            return Ok(None);
        }
    };
    eprintln!(
        "运行 {case} run{run_index_number}：model={} base_url={}",
        settings.model, settings.base_url
    );
    let (_temp, root, db_path) = prepared_fixture(case);
    let client = ChatClient::new(settings.clone(), proxy);
    let provider = ChatTurnProvider::new(&client);
    let input = UnderstandingAgentInput {
        repo_root: root.clone(),
        index_db_path: db_path.clone(),
        request_id: format!("live-{case}-{run_index_number}"),
        question: question.to_string(),
    };
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let mut last_progress = String::new();
    let outcome = run_understanding_agent(&input, &provider, &cancel, &mut |event| match event {
        UnderstandingEvent::Progress(text) => {
            if text != last_progress {
                eprintln!("  [{case}] {text}");
                last_progress = text;
            }
        }
        UnderstandingEvent::Step(UnderstandingStep::ToolCall {
            name,
            args_summary,
            result_excerpt,
            error,
        }) => {
            eprintln!("  [{case}] 工具 {name} {args_summary}{}", if error { "（失败）" } else { "" });
            // 诊断用：把工具结果里出现的全部 source_id 打出来，便于核对模型
            // 引用的来源是否真的发放过（真实模型可能抄错长哈希）。
            if std::env::var_os("KHASLANA_LIVE_VERBOSE").is_some() {
                let ids: Vec<&str> = result_excerpt
                    .match_indices("sr1:")
                    .map(|(index, _)| {
                        let rest = &result_excerpt[index..];
                        let end = rest
                            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == ':'))
                            .unwrap_or(rest.len());
                        &rest[..end]
                    })
                    .collect();
                eprintln!("      sources: {}", ids.join(", "));
            }
        }
        _ => {}
    });
    let elapsed = started.elapsed();
    let answer = outcome.map_err(|error| format!("{case} run{run_index_number} 失败：{error}"))?;
    let Some(answer) = answer else {
        return Err(format!("{case} run{run_index_number} 被取消"));
    };
    Ok(Some((answer, elapsed)))
}

/// 把一次实况结果写成报告文件（含答案与工具轨迹）。
fn write_report(
    case: &str,
    run_index_number: usize,
    question: &str,
    model: &str,
    elapsed: Duration,
    answer: &UnderstandingAnswer,
) {
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/code-understanding/validation/live-runs");
    std::fs::create_dir_all(&out_dir).unwrap();
    let report = serde_json::json!({
        "case": case,
        "run": run_index_number,
        "question": question,
        "model": model,
        "elapsed_ms": elapsed.as_millis() as u64,
        "analysis": &answer.analysis,
        "steps": describe_steps(&answer.steps),
    });
    let path = out_dir.join(format!("{case}-run{run_index_number}.json"));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&report).expect("报告必须可序列化"),
    )
    .unwrap();
    eprintln!("  报告已写入 {}", path.display());
}

/// 渲染答案的人工复核摘要（真实模型判断不受测试脚本约束，这里只呈现）。
fn print_answer(case: &str, answer: &AnalysisResult) {
    eprintln!("── {case} 答案摘要 ──");
    eprintln!("summary: {}", answer.summary);
    eprintln!("completion: {:?}", answer.completion);
    eprintln!("scope: {}", answer.context.scope);
    eprintln!("coverage: {}", answer.coverage_note);
    eprintln!("findings:");
    for finding in &answer.findings {
        eprintln!(
            "  - [{:?}{}] {}（来源 {}）",
            finding.state,
            finding
                .condition
                .as_deref()
                .map(|condition| format!(" · {condition}"))
                .unwrap_or_default(),
            finding.text,
            finding.source_ids.len()
        );
    }
    eprintln!("callers:");
    for caller in &answer.callers {
        eprintln!("  - {:?} {}", caller.kind, caller.name);
    }
    eprintln!("callees:");
    for callee in &answer.callees {
        eprintln!("  - {:?} {}", callee.kind, callee.name);
    }
    eprintln!("data_accesses:");
    for access in &answer.data_accesses {
        eprintln!(
            "  - {}{} {:?} state={:?} conditions={:?}",
            access.object,
            if access.category == DataObjectCategory::DbObject {
                String::new()
            } else {
                format!(" [{:?}]", access.category)
            },
            access.operations,
            access.state,
            access.conditions
        );
    }
    eprintln!("steps: {}", answer.steps.len());
    eprintln!("unknowns:");
    for unknown in &answer.unknowns {
        eprintln!("  - {unknown}");
    }
}

/// 一次实况运行 + 报告 + 摘要输出；返回答案供人工判读。
fn live_once(case: &str, run: usize) -> Option<UnderstandingAnswer> {
    let question = case_question(case);
    let settings = match live_settings() {
        Ok((settings, _)) => settings,
        Err(reason) => {
            eprintln!("跳过 {case} run{run}：{reason}");
            return None;
        }
    };
    let (answer, elapsed) = match run_live(case, &question, run) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(error) => panic!("{error}"),
    };
    print_answer(case, &answer.analysis);
    write_report(case, run, &question, &settings.model, elapsed, &answer);
    Some(answer)
}

// 每个主样例的主问题跑两遍（开发文档 §7：至少两遍、保留两遍结果，不能只展示最好一次）。
// 因此每问两个独立 `#[ignore]` 测试，便于分批执行与单独重试。

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b01_login_run1() {
    live_once("B01", 1);
}

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b01_login_run2() {
    live_once("B01", 2);
}

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b02_mybatis_login_run1() {
    live_once("B02", 1);
}

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b02_mybatis_login_run2() {
    live_once("B02", 2);
}

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b03_jpa_login_run1() {
    live_once("B03", 1);
}

#[test]
#[ignore = "实况：调用本机已配置的 AI 供应商"]
fn live_b03_jpa_login_run2() {
    live_once("B03", 2);
}
