use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::code_index::{PipelineOptions, RunOutcome, run_index};

fn answer(
    project_key: &str,
    question: &str,
    number: usize,
    completion: AnalysisCompletion,
) -> UnderstandingAnswer {
    UnderstandingAnswer {
        analysis: AnalysisResult {
            version: ANALYSIS_PROTOCOL_VERSION,
            context: AnalysisContext {
                project_key: project_key.to_string(),
                request_id: format!("request-{number}"),
                index_generation: 1,
                question: question.to_string(),
                scope: format!("scope-{number}"),
            },
            summary: format!("summary-{number}"),
            coverage_note: "测试范围".to_string(),
            findings: Vec::new(),
            callers: Vec::new(),
            callees: Vec::new(),
            data_accesses: Vec::new(),
            steps: Vec::new(),
            links: Vec::new(),
            unknowns: Vec::new(),
            completion,
        },
        steps: Vec::new(),
        reasoning: None,
        sources: Vec::new(),
    }
}

#[test]
fn session_keeps_five_answers_and_prompts_with_latest_three() {
    let mut session = UnderstandingSession::new("project", "tab-a").unwrap();
    for number in 1..=7 {
        let question = format!("question-{number}");
        let ticket = session.begin_request(&question).unwrap();
        assert!(
            session
                .finish_answer(
                    &ticket.key,
                    answer("project", &question, number, AnalysisCompletion::Complete),
                )
                .unwrap()
        );
    }

    assert_eq!(session.history().len(), UNDERSTANDING_SESSION_HISTORY_LIMIT);
    assert_eq!(session.history().front().unwrap().request.generation, 3);
    let ticket = session.begin_request("follow-up").unwrap();
    assert_eq!(
        ticket.prompt_context.history.len(),
        UNDERSTANDING_PROMPT_HISTORY_LIMIT
    );
    let text = ticket.prompt_context.history_text();
    assert!(!text.contains("question-4"));
    assert!(text.contains("question-5"));
    assert!(text.contains("question-6"));
    assert!(text.contains("question-7"));
    assert!(text.find("question-5") < text.find("question-7"));
}

#[test]
fn prompt_history_obeys_eight_k_character_budget() {
    let context = UnderstandingPromptContext {
        history: (1..=5)
            .map(|number| UnderstandingHistorySummary {
                question: format!("question-{number}"),
                summary: "长摘要".repeat(2_000),
                scope: "src/".to_string(),
                completion: AnalysisCompletion::Complete,
            })
            .collect(),
        selected_source: None,
    };
    let text = context.history_text();
    assert!(text.chars().count() <= UNDERSTANDING_PROMPT_HISTORY_MAX_CHARS);
    assert!(text.contains("question-5"), "应优先保留最新摘要");
    assert!(!text.contains("question-1"), "最多只考虑最近三次答案");
}

#[test]
fn cancel_waits_for_worker_exit_before_next_request() {
    let mut session = UnderstandingSession::new("project", "tab-a").unwrap();
    let ticket = session.begin_request("question").unwrap();
    assert!(!ticket.cancel.load(Ordering::Relaxed));

    assert_eq!(session.cancel_active(), Some(ticket.key.clone()));
    assert!(ticket.cancel.load(Ordering::Relaxed));
    assert_eq!(session.status(), UnderstandingSessionStatus::Cancelled);
    assert!(session.has_active_request());
    assert_eq!(
        session.begin_request("too early").unwrap_err().code,
        ErrorCode::IndexBusy
    );

    assert!(session.finish_cancelled(&ticket.key));
    assert!(!session.has_active_request());
    assert!(session.begin_request("next").is_ok());
}

#[test]
fn detached_request_rejects_late_events_and_answer() {
    let mut session = UnderstandingSession::new("project", "tab-a").unwrap();
    let ticket = session.begin_request("question").unwrap();
    let event = UnderstandingSession::tag_event(
        &ticket.key,
        UnderstandingEvent::Progress("working".to_string()),
    );
    assert!(session.accepts_event(&event));

    session.detach_active();
    assert_eq!(session.status(), UnderstandingSessionStatus::Stale);
    assert!(!session.accepts_event(&event));
    assert!(
        !session
            .finish_answer(
                &ticket.key,
                answer("project", "question", 1, AnalysisCompletion::Complete),
            )
            .unwrap()
    );
    assert!(session.history().is_empty());
    assert!(!session.has_active_request());
}

#[test]
fn request_route_rejects_other_session_and_generation() {
    let mut session = UnderstandingSession::new("project", "tab-a").unwrap();
    let ticket = session.begin_request("question").unwrap();
    for wrong in [
        UnderstandingRequestKey {
            project_key: "other".to_string(),
            session_key: "tab-a".to_string(),
            generation: ticket.key.generation,
        },
        UnderstandingRequestKey {
            project_key: "project".to_string(),
            session_key: "tab-b".to_string(),
            generation: ticket.key.generation,
        },
        UnderstandingRequestKey {
            project_key: "project".to_string(),
            session_key: "tab-a".to_string(),
            generation: ticket.key.generation + 1,
        },
    ] {
        let event = UnderstandingSession::tag_event(
            &wrong,
            UnderstandingEvent::Progress("late".to_string()),
        );
        assert!(!session.accepts_event(&event));
    }
}

fn copy_dir(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(from, to).unwrap();
        }
    }
}

#[test]
fn historical_source_is_revalidated_before_opening() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/code_understanding_business/B01");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("b01");
    copy_dir(&fixture, &root);
    git2::Repository::init(&root).unwrap();
    let db_path = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    assert!(matches!(
        run_index(&root, &db_path, true, &mut options).unwrap(),
        RunOutcome::Completed(_)
    ));

    let tools = UnderstandingTools::open(&root, &db_path).unwrap();
    let read = tools
        .read_file(
            "read-1",
            ReadFileArgs {
                path: "src/com/example/login/AuthService.java".to_string(),
                start_line: Some(1),
                end_line: Some(8),
            },
        )
        .unwrap()
        .data;
    let project_key = tools.context().project_key.clone();
    let mut session = UnderstandingSession::new(&project_key, "tab-a").unwrap();
    let ticket = session.begin_request("question").unwrap();
    let mut completed = answer(&project_key, "question", 1, AnalysisCompletion::Complete);
    completed.sources.push(read.source_ref.clone());
    session.finish_answer(&ticket.key, completed).unwrap();
    let entry = session.history().back().unwrap();
    assert_eq!(
        validate_history_source(&root, &db_path, entry, &read.source_id).unwrap(),
        read.source_ref
    );

    let changed = root.join("src/com/example/login/AuthService.java");
    std::fs::write(&changed, "changed\n").unwrap();
    let error = validate_history_source(&root, &db_path, entry, &read.source_id).unwrap_err();
    assert_eq!(error.code, ErrorCode::SourceChanged);
}
