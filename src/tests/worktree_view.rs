use super::*;

#[test]
fn change_sections_compact_when_both_sides_are_empty() {
    assert_eq!(
        change_sections_layout(0, false, 0, false),
        ChangeSectionsLayout {
            show_staged: false,
            staged: ChangeSectionHeight::Compact,
            unstaged: ChangeSectionHeight::Compact,
        }
    );
}

#[test]
fn change_sections_share_remaining_height_when_both_sides_have_content() {
    assert_eq!(
        change_sections_layout(1, false, 20_000, false),
        ChangeSectionsLayout {
            show_staged: true,
            staged: ChangeSectionHeight::Fill,
            unstaged: ChangeSectionHeight::Fill,
        }
    );
}

#[test]
fn loading_section_keeps_space_while_empty_peer_stays_compact() {
    assert_eq!(
        change_sections_layout(0, true, 0, false),
        ChangeSectionsLayout {
            show_staged: true,
            staged: ChangeSectionHeight::Fill,
            unstaged: ChangeSectionHeight::Compact,
        }
    );
}

/// 重构计划 §5：无暂存内容时左列只显示未暂存列表，不渲染空的「已暂存变更」分区。
#[test]
fn staged_section_is_hidden_when_index_is_clean() {
    let layout = change_sections_layout(0, false, 12, false);
    assert!(!layout.show_staged);
    // 未暂存列表独占左列剩余高度。
    assert_eq!(layout.unstaged, ChangeSectionHeight::Fill);
}

/// 暂存区加载期间先占位，加载完成仍为空则整段收起。
#[test]
fn staged_section_appears_while_loading_then_hides_when_still_empty() {
    assert!(change_sections_layout(0, true, 3, false).show_staged);
    assert!(!change_sections_layout(0, false, 3, false).show_staged);
}

/// 暂存与未暂存同时为空时两个分区都不占空间（各自保持一行提示）。
#[test]
fn clean_worktree_keeps_both_sections_compact() {
    let layout = change_sections_layout(0, false, 0, false);
    assert!(!layout.show_staged);
    assert_eq!(layout.unstaged, ChangeSectionHeight::Compact);
}

/// 提交条次级动作的文案带当前分支名（最新 Pencil 稿第五版「提交到 dev_gpuikit」）。
#[test]
fn commit_branch_label_uses_current_head() {
    assert_eq!(
        commit_branch_button_label(Some("dev_gpuikit")),
        "提交到 dev_gpuikit"
    );
    // 恰好等于上限时不算超长，不带省略号。
    let exact = "a".repeat(COMMIT_BRANCH_LABEL_MAX_CHARS);
    assert_eq!(
        commit_branch_button_label(Some(&exact)),
        format!("提交到 {exact}")
    );
}

/// 长分支名按字符截断并加省略号：不能把蓝色主按钮挤出提交条，
/// 且必须按字符切（多字节分支名不能切坏 UTF-8）。
#[test]
fn commit_branch_label_truncates_long_names_by_chars() {
    let too_long = "feature/非常长的分支名称-abcdef";
    let label = commit_branch_button_label(Some(too_long));
    assert!(label.ends_with('…'));
    let shown = label.strip_prefix("提交到 ").unwrap();
    assert_eq!(shown.chars().count(), COMMIT_BRANCH_LABEL_MAX_CHARS + 1);
    assert!(too_long.starts_with(shown.trim_end_matches('…')));
}

/// HEAD 脱离分支或尚未打开仓库时退化为通用文案，不猜分支名。
#[test]
fn commit_branch_label_falls_back_without_head() {
    assert_eq!(commit_branch_button_label(None), "提交到当前分支");
    assert_eq!(commit_branch_button_label(Some("")), "提交到当前分支");
    assert_eq!(commit_branch_button_label(Some("   ")), "提交到当前分支");
}

/// 干净工作区与「一侧空」是两种空态（视觉规范 §2：分别设计文案）。
/// 未暂存列表空但暂存区有内容时，不能显示「工作区干净」——提交还没做完。
#[test]
fn unstaged_empty_text_separates_clean_worktree_from_one_side_empty() {
    // 两边都空：干净工作区的专属文案。
    assert_eq!(
        unstaged_empty_text(false, false),
        "工作区干净，没有待提交的改动"
    );
    // 暂存区有内容、未暂存空：流程没结束，走「已全部暂存」文案。
    assert_eq!(unstaged_empty_text(false, true), "未暂存变更已全部暂存");
    // 加载中优先于一切空态判断。
    assert_eq!(unstaged_empty_text(true, true), "修改区加载中...");
    assert_eq!(unstaged_empty_text(true, false), "修改区加载中...");
}

/// 计划 §5：禁用条件以真实业务守卫为准——普通提交从不因「暂存区非空」
/// 被禁用（amend 允许空暂存只改信息），只有「没有仓库 / 忙」才禁用。
#[test]
fn commit_action_enabled_ignores_staging_state() {
    // 空暂存的仓库也允许提交/修补（守卫里没有 staged_count 入参）。
    assert!(commit_action_enabled(true, false, false, false));
    assert!(!commit_action_enabled(false, false, false, false));
    assert!(!commit_action_enabled(true, true, false, false));
    // 合并中直接透传 merge_can_finish 的结论：忙/有冲突/信息为空时调用方
    // 会算出 false，这里必须跟着禁用。
    assert!(commit_action_enabled(true, false, true, true));
    assert!(!commit_action_enabled(true, false, true, false));
    // 合并中 busy → 调用方传入的 merge_can_finish 为 false（其内部查 busy）。
    assert!(!commit_action_enabled(true, true, true, false));
    // 合并中即便仓库空闲也不能绕过 merge 守卫（false 恒禁用）。
    assert!(!commit_action_enabled(true, true, true, false));
}

/// 推送入口还要求远端存在；合并中完全不提供推送（`merge_in_progress`
/// 时 `can_commit_and_push` 恒假，动作组换成「完成合并/中止合并」）。
#[test]
fn commit_and_push_requires_remote_and_hides_during_merge() {
    assert!(commit_and_push_enabled(true, false, false, true));
    assert!(!commit_and_push_enabled(true, false, false, false));
    assert!(!commit_and_push_enabled(false, false, false, true));
    assert!(!commit_and_push_enabled(true, true, false, true));
    assert!(!commit_and_push_enabled(true, false, true, true));
}
