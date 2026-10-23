use crate::repository_ssh_ui::{generation_form_valid, HostOperationRegistry};
use crate::RepoTabId;
use khaslana::ssh_host_trust::HostTrustOperationScope;

#[test]
fn answered_challenge_operation_remains_cancellable_by_its_tab() {
    let scope = HostTrustOperationScope::enter(None);
    let operation = scope.operation();
    let mut registry = HostOperationRegistry::default();
    registry.register(Some(RepoTabId(1)), operation.clone());
    // 授信响应出队不触碰独立登记表，任务仍活跃时必须可取消。
    registry.prune_inactive();
    assert!(operation.is_active());
    registry.cancel_tab(RepoTabId(1));
    assert!(!operation.is_active());
}

#[test]
fn closing_one_tab_does_not_cancel_another_tab_or_global_test() {
    let first_scope = HostTrustOperationScope::enter(None);
    let first = first_scope.operation();
    let second_scope = HostTrustOperationScope::enter(None);
    let second = second_scope.operation();
    let global_scope = HostTrustOperationScope::enter(None);
    let global = global_scope.operation();
    let mut registry = HostOperationRegistry::default();
    registry.register(Some(RepoTabId(1)), first.clone());
    registry.register(Some(RepoTabId(2)), second.clone());
    registry.register(None, global.clone());
    registry.cancel_tab(RepoTabId(1));
    assert!(!first.is_active());
    assert!(second.is_active());
    assert!(global.is_active());
}

#[test]
fn registration_deduplicates_operation_and_prunes_only_inactive_tokens() {
    let scope = HostTrustOperationScope::enter(None);
    let operation = scope.operation();
    let mut registry = HostOperationRegistry::default();
    registry.register(Some(RepoTabId(1)), operation.clone());
    registry.register(Some(RepoTabId(1)), operation.clone());
    assert_eq!(registry.entries.len(), 1);
    registry.prune_inactive();
    assert_eq!(registry.entries.len(), 1);
    drop(scope);
    registry.prune_inactive();
    assert!(registry.entries.is_empty());
}

#[test]
fn generation_requires_explicit_protection_and_matching_confirmation() {
    assert!(!generation_form_valid("work", "", "", false));
    assert!(!generation_form_valid("work", "secret", "different", false));
    assert!(!generation_form_valid(" ", "secret", "secret", false));
    assert!(generation_form_valid("work", "secret", "secret", false));
    assert!(generation_form_valid("work", "", "", true));
    assert!(!generation_form_valid("work", "secret", "secret", true));
}
