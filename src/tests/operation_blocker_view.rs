use std::time::{Duration, Instant};

use super::{
    OPERATION_BLOCKER_VISIBLE_DELAY, OperationBlocker, should_render_operation_blocker,
    wrap_operation_message,
};

#[test]
fn operation_blocker_renders_only_when_modal_and_busy_and_delay_elapsed() {
    let now = Instant::now();

    // 无 blocker，即使忙碌也不渲染。
    assert!(!should_render_operation_blocker(
        OperationBlocker::None,
        true,
        Some(now),
        now,
    ));

    // Modal 但不忙碌，不渲染。
    assert!(!should_render_operation_blocker(
        OperationBlocker::Modal,
        false,
        Some(now),
        now,
    ));

    // Modal 且忙碌，但延迟未到，不渲染。
    let started = now;
    let before_delay = now + Duration::from_millis(100);
    assert!(before_delay > started);
    assert!(!should_render_operation_blocker(
        OperationBlocker::Modal,
        true,
        Some(started),
        before_delay,
    ));

    // Modal 且忙碌，延迟已到，渲染。
    let after_delay = started + OPERATION_BLOCKER_VISIBLE_DELAY;
    assert!(should_render_operation_blocker(
        OperationBlocker::Modal,
        true,
        Some(started),
        after_delay,
    ));

    // started 为 None（旧状态兼容），Modal 且忙碌时直接渲染，不等待延迟。
    assert!(should_render_operation_blocker(
        OperationBlocker::Modal,
        true,
        None,
        now,
    ));
}

#[test]
fn operation_blocker_message_wraps_remote_push_target() {
    let message = "正在推送 dev_wzf_20260609_引进新保单检视系统 到 origin/dev_wzf_20260609_引进新保单检视系统";

    assert_eq!(
        wrap_operation_message(message),
        "正在推送 dev_wzf_20260609_引进新保单检视系统 到\norigin/dev_wzf_20260609_引进新保单检视系统"
    );
}
