//! 本地完成历史持久化的单元测试（CU2-T5）。
//!
//! 覆盖：保存与回读往返、每仓库隔离、坏文件跳过、版本不符跳过、
//! 数量裁剪（只保留最近 30 条）、同毫秒唯一 id、来源往返校验。

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::code_understanding::AnalysisEvidenceState;
use crate::code_understanding::agent::UnderstandingStep;
use crate::code_understanding::analysis::{
    AnalysisCompletion, AnalysisContext, AnalysisFinding, AnalysisResult,
};

fn temp_base(tag: &str) -> std::path::PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "khaslana-cu-history-test-{}-{}-{tag}",
        std::process::id(),
        millis
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_analysis(question: &str) -> AnalysisResult {
    AnalysisResult {
        version: 1,
        context: AnalysisContext {
            project_key: "D:/demo".to_string(),
            request_id: "cu-1".to_string(),
            index_generation: 7,
            question: question.to_string(),
            scope: "已读取 src/login.rs:1-80".to_string(),
        },
        summary: "登录由 LoginService 完成".to_string(),
        coverage_note: String::new(),
        findings: vec![AnalysisFinding {
            text: "先查用户再校验密码".to_string(),
            state: AnalysisEvidenceState::Observed,
            condition: None,
            source_ids: vec!["sr1:test".to_string()],
        }],
        callers: Vec::new(),
        callees: Vec::new(),
        data_accesses: Vec::new(),
        steps: Vec::new(),
        links: Vec::new(),
        unknowns: vec!["动态表名未确认".to_string()],
        completion: AnalysisCompletion::Complete,
    }
}

fn sample_record(
    project_key: &str,
    question: &str,
    finished_at_millis: u64,
) -> UnderstandingHistoryRecord {
    UnderstandingHistoryRecord {
        format_version: UNDERSTANDING_HISTORY_FORMAT_VERSION,
        project_key: project_key.to_string(),
        session_key: "primary".to_string(),
        generation: 1,
        question: question.to_string(),
        completion: AnalysisCompletion::Complete,
        analysis: sample_analysis(question),
        steps: vec![UnderstandingStep::ToolCall {
            name: "read_file".to_string(),
            args_summary: "read_file src/login.rs:1-80".to_string(),
            result_excerpt: "public Result login(...)".to_string(),
            error: false,
        }],
        reasoning: Some("需要先读入口".to_string()),
        sources: vec![UnderstandingSourceRecord {
            project_key: project_key.to_string(),
            generation: 7,
            relative_path: "src/login.rs".to_string(),
            content_sha256: "a".repeat(64),
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
        }],
        model: "test-model".to_string(),
        started_at_millis: 0,
        finished_at_millis,
        duration_secs: 3,
        index_generation: 7,
    }
}

#[test]
fn save_and_list_roundtrip() {
    let base = temp_base("roundtrip");
    let record = sample_record("D:/demo", "登录怎么实现？", 1000);
    let id = save_understanding_history_record(&base, record.clone()).unwrap();
    assert_eq!(id, "1000");

    let records = list_understanding_history_records(&base, "D:/demo", 20).unwrap();
    assert_eq!(records.len(), 1);
    let loaded = &records[0];
    assert_eq!(loaded.question, "登录怎么实现？");
    assert_eq!(loaded.analysis.summary, record.analysis.summary);
    assert_eq!(loaded.steps, record.steps);
    assert_eq!(loaded.sources.len(), 1);
    // 来源可重建为结构有效的 SourceRef
    let source_ref = loaded.sources[0].to_source_ref().unwrap();
    assert_eq!(source_ref.relative_path, "src/login.rs");
    assert_eq!(source_ref.start_line, 1);
    fs::remove_dir_all(&base).ok();
}

#[test]
fn repos_are_isolated() {
    let base = temp_base("isolated");
    save_understanding_history_record(&base, sample_record("D:/a", "问题A", 100)).unwrap();
    save_understanding_history_record(&base, sample_record("D:/b", "问题B", 101)).unwrap();

    assert_eq!(
        list_understanding_history_records(&base, "D:/a", 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_understanding_history_records(&base, "D:/b", 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_understanding_history_records(&base, "D:/c", 20)
            .unwrap()
            .len(),
        0
    );
    // 大小写不敏感：同一仓库（Windows 路径）读得到同一目录
    assert_eq!(
        list_understanding_history_records(&base, "d:/A", 20)
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(&base).ok();
}

#[test]
fn corrupt_and_version_mismatch_skipped() {
    let base = temp_base("corrupt");
    let dir = base
        .join("code-understanding")
        .join(crate::ai::review_store::repo_key("D:/demo"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("500.json"), "{not json").unwrap();
    let mut bad_version = sample_record("D:/demo", "问题", 501);
    bad_version.format_version = 999;
    fs::write(
        dir.join("501.json"),
        serde_json::to_string(&bad_version).unwrap(),
    )
    .unwrap();
    // .tmp 文件不参与列表
    fs::write(dir.join("502.json.tmp"), "partial").unwrap();

    let records = list_understanding_history_records(&base, "D:/demo", 20).unwrap();
    assert!(records.is_empty(), "坏记录与版本不符记录都应被跳过");
    fs::remove_dir_all(&base).ok();
}

#[test]
fn prunes_to_max_records() {
    let base = temp_base("prune");
    for millis in 1..=UNDERSTANDING_HISTORY_MAX_RECORDS as u64 + 5 {
        save_understanding_history_record(&base, sample_record("D:/demo", "问题", millis)).unwrap();
    }
    let records = list_understanding_history_records(&base, "D:/demo", 1000).unwrap();
    assert_eq!(records.len(), UNDERSTANDING_HISTORY_MAX_RECORDS);
    // 保留的是最近的：最旧的 1..=5 被删除
    assert!(records.iter().all(|record| record.finished_at_millis > 5));
    fs::remove_dir_all(&base).ok();
}

#[test]
fn same_millis_gets_suffix() {
    let base = temp_base("suffix");
    save_understanding_history_record(&base, sample_record("D:/demo", "第一条", 777)).unwrap();
    let second =
        save_understanding_history_record(&base, sample_record("D:/demo", "第二条", 777)).unwrap();
    assert_eq!(second, "777-2");
    let records = list_understanding_history_records(&base, "D:/demo", 20).unwrap();
    assert_eq!(records.len(), 2);
    fs::remove_dir_all(&base).ok();
}

#[test]
fn invalid_source_record_rejected() {
    let base = temp_base("invalid-source");
    let mut record = sample_record("D:/demo", "问题", 900);
    record.sources[0].start_line = 0; // 行号必须 ≥ 1
    save_understanding_history_record(&base, record).unwrap();
    let records = list_understanding_history_records(&base, "D:/demo", 20).unwrap();
    assert_eq!(records.len(), 1);
    // 读取不报错，但来源重建（to_source_ref）必须失败，防止伪造引用
    assert!(records[0].sources[0].to_source_ref().is_err());
    fs::remove_dir_all(&base).ok();
}
