use super::state::{AiConnectionTestState, ConnectionTestOutcome};
use khaslana::AiProviderSettings;

fn settings() -> AiProviderSettings {
    AiProviderSettings {
        enabled: true,
        base_url: "https://example.test/v1".into(),
        model: "test-model".into(),
        ..Default::default()
    }
}

#[test]
fn stale_completion_does_not_finish_new_test() {
    let mut state = AiConnectionTestState::default();
    let first = state.begin(settings());
    let second = state.begin(settings());
    assert!(!state.finish(first, Ok("旧结果".into())));
    assert!(state.is_running());
    assert!(state.finish(second, Ok("新结果".into())));
    assert!(matches!(state.outcome_for(&settings()), ConnectionTestOutcome::Passed(message) if message == "新结果"));
}

#[test]
fn changed_connection_never_displays_old_success() {
    let mut state = AiConnectionTestState::default();
    let original = settings();
    let request = state.begin(original.clone());
    assert!(state.finish(request, Ok("通过".into())));
    for field in ["address", "key", "model"] {
        let mut edited = original.clone();
        match field {
            "address" => edited.base_url = "https://other.test/v1".into(),
            "key" => edited.api_key = "new-secret".into(),
            _ => edited.model = "other-model".into(),
        }
        assert!(matches!(state.outcome_for(&edited), ConnectionTestOutcome::Idle));
    }
    assert!(matches!(state.outcome_for(&original), ConnectionTestOutcome::Passed(_)));
}

#[test]
fn editing_during_test_preserves_pending_request_and_scopes_failure() {
    let mut state = AiConnectionTestState::default();
    let original = settings();
    let request = state.begin(original.clone());
    let mut edited = original.clone();
    edited.model = "edited-model".into();
    assert!(matches!(state.outcome_for(&edited), ConnectionTestOutcome::Idle));
    assert!(state.is_running());
    assert!(state.finish(request, Err("服务不可用".into())));
    assert!(!state.is_running());
    assert!(matches!(state.outcome_for(&edited), ConnectionTestOutcome::Idle));
    assert!(matches!(state.outcome_for(&original), ConnectionTestOutcome::Failed(error) if error == "服务不可用"));
}

#[test]
fn duplicate_completion_cannot_overwrite_result() {
    let mut state = AiConnectionTestState::default();
    let request = state.begin(settings());
    assert!(state.finish(request, Err("连接失败".into())));
    assert!(!state.finish(request, Ok("迟到结果".into())));
    assert!(matches!(state.outcome_for(&settings()), ConnectionTestOutcome::Failed(_)));
}

#[test]
fn global_reset_invalidates_in_flight_completion() {
    let mut state = AiConnectionTestState::default();
    let request = state.begin(settings());
    state.interrupt("后台任务异常".into());
    assert!(!state.is_running());
    assert!(!state.finish(request, Ok("迟到结果".into())));
    assert!(matches!(state.outcome_for(&settings()), ConnectionTestOutcome::Failed(_)));
    let next = state.begin(settings());
    assert!(!state.finish(request, Err("旧失败".into())));
    assert!(state.is_running());
    assert!(state.finish(next, Ok("通过".into())));
}
