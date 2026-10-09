// 设置中心「代码索引」页视图层纯函数测试（挂载于 src/code_index_view.rs）。

use super::*;

// ---------------------------------------------------------------------------
// 卡片状态判定
// ---------------------------------------------------------------------------

#[test]
fn entry_status_running_has_priority() {
    assert_eq!(
        code_index_entry_status(true, false, false),
        CodeIndexEntryStatus::Running
    );
    // 即使开关已关、数据也已删，只要任务还在跑就显示运行中。
    assert_eq!(
        code_index_entry_status(true, true, true),
        CodeIndexEntryStatus::Running
    );
}

#[test]
fn entry_status_dispatches_by_data_and_switch() {
    assert_eq!(
        code_index_entry_status(false, true, true),
        CodeIndexEntryStatus::Indexed
    );
    // 关闭开关不删数据 -> 已停用（保留数据，可重建/删除）。
    assert_eq!(
        code_index_entry_status(false, false, true),
        CodeIndexEntryStatus::DisabledWithData
    );
    assert_eq!(
        code_index_entry_status(false, true, false),
        CodeIndexEntryStatus::NotIndexed
    );
    assert_eq!(
        code_index_entry_status(false, false, false),
        CodeIndexEntryStatus::NotIndexed
    );
}

// ---------------------------------------------------------------------------
// 列表过滤
// ---------------------------------------------------------------------------

fn entry(name: &str, path: &str) -> CodeIndexListEntry {
    CodeIndexListEntry {
        repo_key: path.to_lowercase(),
        name: name.to_string(),
        path: path.to_string(),
    }
}

#[test]
fn filter_empty_matches_all() {
    let e = entry("khaslana", r"D:\workspace\khaslana");
    assert!(code_index_entry_matches_filter(&e, ""));
    assert!(code_index_entry_matches_filter(&e, "   "));
}

#[test]
fn filter_matches_name_or_path_case_insensitive() {
    let e = entry("khaslana", r"D:\workspace\CodeX-workspace\khaslana");
    assert!(code_index_entry_matches_filter(&e, "khasl"));
    assert!(code_index_entry_matches_filter(&e, "KHASL"));
    // 路径段命中（目录名）。
    assert!(code_index_entry_matches_filter(&e, "codex"));
    // 完全不相关。
    assert!(!code_index_entry_matches_filter(&e, "gpui"));
}

// ---------------------------------------------------------------------------
// 显示名推导
// ---------------------------------------------------------------------------

#[test]
fn display_name_takes_last_path_segment() {
    assert_eq!(
        code_index_display_name(r"D:\workspace\CodeX-workspace\khaslana"),
        "khaslana"
    );
    assert_eq!(
        code_index_display_name("/home/user/projects/gpui-ce"),
        "gpui-ce"
    );
    // 末尾分隔符不产生空段。
    assert_eq!(code_index_display_name(r"D:\repo\"), "repo");
    // 无分隔符整体返回。
    assert_eq!(code_index_display_name("repo"), "repo");
}

// ---------------------------------------------------------------------------
// 完成/失败事件的任务归属与提示分级
// ---------------------------------------------------------------------------

fn task(repo: &str, user_initiated: bool) -> CodeIndexTaskState {
    CodeIndexTaskState {
        task_id: 1,
        repo_path: repo.to_string(),
        cancel: Arc::new(AtomicBool::new(false)),
        user_initiated,
    }
}

#[test]
fn finished_event_of_auto_refresh_stays_silent() {
    // 自动增量刷新（仓库加载 / 工作区操作后）：在途但不主动，只更新状态。
    assert_eq!(
        code_index_task_ownership(Some(&task(r"d:\repo", false)), r"d:\repo", 1),
        (true, false)
    );
}

#[test]
fn finished_event_of_button_trigger_notifies() {
    // 设置页按钮 / 索引开关：主动触发，完成时弹提示。
    assert_eq!(
        code_index_task_ownership(Some(&task(r"d:\repo", true)), r"d:\repo", 1),
        (true, true)
    );
}

#[test]
fn finished_event_addressed_by_another_repo_is_ignored() {
    // 事件按仓库键寻址：别的仓库的在途任务不算这条事件的任务。
    assert_eq!(
        code_index_task_ownership(Some(&task(r"d:\other", true)), r"d:\repo", 1),
        (false, false)
    );
    // 任务已结束（迟到事件）：同样不动任务状态。
    assert_eq!(code_index_task_ownership(None, r"d:\repo", 1), (false, false));
}

#[test]
fn cancelled_task_events_do_not_own_a_restarted_task_in_the_same_repository() {
    let mut restarted = task(r"d:\repo", true);
    restarted.task_id = 2;
    assert_eq!(code_index_task_ownership(Some(&restarted), r"d:\repo", 1), (false, false));
    assert_eq!(code_index_task_ownership(Some(&restarted), r"d:\repo", 2), (true, true));
}

#[test]
fn index_panic_reports_failure_with_the_original_task_identity() {
    let event = code_index_task_event(7, r"d:\repo", || panic!("injected index panic"));
    let Some(UiEvent::CodeIndexFailed { task_id, repo_path, error }) = event else {
        panic!("索引 panic 必须返回带身份的失败事件");
    };
    assert_eq!(task_id, 7);
    assert_eq!(repo_path, r"d:\repo");
    assert!(error.contains("injected index panic"));
    let mut current = task(&repo_path, true);
    current.task_id = task_id;
    assert_eq!(code_index_task_ownership(Some(&current), &repo_path, task_id), (true, true));
    assert!(code_index_task_event(7, &repo_path, || Ok(RunOutcome::Cancelled)).is_none());
}

#[test]
fn late_stats_do_not_replace_newer_results_or_restore_deleted_index_cards() {
    let mut requests = HashMap::from([("repo".into(), 2), ("other".into(), 3)]);
    assert!(!take_code_index_stats_request(&mut requests, "repo", 1));
    assert_eq!(requests.get("repo"), Some(&2));
    assert!(take_code_index_stats_request(&mut requests, "repo", 2));
    assert!(!take_code_index_stats_request(&mut requests, "repo", 2));
    requests.remove("other");
    assert!(!take_code_index_stats_request(&mut requests, "other", 3));
}
