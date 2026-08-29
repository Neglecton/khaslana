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
