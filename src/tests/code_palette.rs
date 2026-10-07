use super::*;
use khaslana::{BranchInfo, TagInfo};

fn branch(name: &str, kind: BranchKind) -> BranchInfo {
    BranchInfo {
        name: name.into(),
        kind,
        is_head: false,
        upstream: None,
        ahead: None,
        behind: None,
    }
}

#[test]
fn search_finds_settings_and_function_aliases_without_a_repository_or_index() {
    let entries = function_entries(None, false, None, true);
    let proxy = filter_entries(entries.clone(), "  PROXY  ");
    assert_eq!(proxy.len(), 1);
    assert_eq!(
        proxy[0].target,
        SearchTarget::Settings(SettingsCategory::Proxy)
    );
    assert_eq!(
        filter_entries(entries.clone(), "拣选")[0].target,
        SearchTarget::Feature(RepoTabId(0), "拣选提交", MainMode::History)
    );
    assert_eq!(
        filter_entries(entries, "MCP")[0].target,
        SearchTarget::AiSettings(AiSettingsTab::Mcp)
    );
}

#[test]
fn search_preserves_all_branches_and_disambiguates_repository_and_ref_kind() {
    let mut entries = Vec::new();
    let snapshot = RepositorySnapshot {
        branches: (0..120)
            .map(|index| branch(&format!("feature/{index}"), BranchKind::Local))
            .chain([branch("feature/1", BranchKind::Remote)])
            .collect(),
        tags: vec![TagInfo {
            name: "feature/1".into(),
        }],
        ..Default::default()
    };
    append_repository_entries(&mut entries, RepoTabId(1), "仓库甲", &snapshot);
    append_repository_entries(&mut entries, RepoTabId(2), "仓库乙", &snapshot);
    assert_eq!(filter_entries(entries.clone(), "feature").len(), 244);
    let results = filter_entries(entries, "feature/1 仓库乙 远端");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].target,
        SearchTarget::Branch(RepoTabId(2), "feature/1".into(), BranchKind::Remote)
    );
}

#[test]
fn exact_name_precedes_prefix_and_keyword_matches() {
    let entries = vec![
        SearchEntry::new(
            "设置",
            "仓库 main",
            "设置",
            "",
            SearchTarget::OpenRepository,
        ),
        SearchEntry::new(
            "main-feature",
            "",
            "本地分支",
            "",
            SearchTarget::CloneRepository,
        ),
        SearchEntry::new(
            "main",
            "",
            "本地分支",
            "",
            SearchTarget::Repository(RepoTabId(1)),
        ),
    ];
    let results = filter_entries(entries, "main");
    assert_eq!(
        results
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["main", "main-feature", "设置"]
    );
}

#[test]
fn keyboard_navigation_skips_disabled_results_and_handles_empty_list() {
    let entries = function_entries(None, true, None, false);
    let available: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.disabled_reason.is_none())
        .map(|(index, _)| index)
        .collect();
    for (position, index) in available.iter().enumerate() {
        assert_eq!(
            next_selection(&entries, Some(*index), true),
            available
                .get((position + 1).min(available.len() - 1))
                .copied()
        );
    }
    assert_eq!(next_selection(&[], None, true), None);
    assert_eq!(
        next_selection(&filter_entries(entries, "创建分支"), None, true),
        None
    );
}

#[test]
fn settings_remain_available_while_git_operations_are_busy() {
    let snapshot = RepositorySnapshot::default();
    let entries = function_entries(Some(RepoTabId(1)), true, Some(&snapshot), true);
    assert!(
        filter_entries(entries.clone(), "外观")[0]
            .disabled_reason
            .is_none()
    );
    assert!(
        filter_entries(entries.clone(), "工作区")[0]
            .disabled_reason
            .is_none()
    );
    assert_eq!(
        filter_entries(entries, "创建标签")[0].disabled_reason,
        Some("当前操作进行中，请稍候")
    );
}
