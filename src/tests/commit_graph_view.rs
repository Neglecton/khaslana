use super::*;

fn test_commit(oid: &str, parents: &[&str]) -> CommitInfo {
    CommitInfo {
        oid: oid.to_string(),
        short_oid: oid.to_string(),
        summary: oid.to_string(),
        message: oid.to_string(),
        author: "测试作者".to_string(),
        author_email: Some("test@example.invalid".to_string()),
        committer: "测试作者".to_string(),
        committer_email: Some("test@example.invalid".to_string()),
        time: 0,
        parents: parents.iter().map(|parent| (*parent).to_string()).collect(),
        refs: Vec::new(),
    }
}

#[test]
fn unmerged_branch_tips_do_not_connect_from_top() {
    let commits = vec![
        test_commit("feature-tip", &["base"]),
        test_commit("main-tip", &["base"]),
        test_commit("base", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    assert!(!rows[0].connected_from_top);
    assert!(!rows[1].connected_from_top);
    assert!(rows[2].connected_from_top);
}

// 分叉的两个分支 tip 汇合到同一父提交时，后到的 tip 并入父提交已有泳道，
// 自身泳道释放——否则父提交行之后会残留幽灵竖线贯穿到列表末尾。
#[test]
fn fork_rejoining_parent_releases_lane() {
    let commits = vec![
        test_commit("main-tip", &["base"]),
        test_commit("feature-tip", &["base"]),
        test_commit("base", &["root"]),
        test_commit("root", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    // feature-tip 行并入 base 所在泳道 0，自身泳道在行内仍可见（画圆点）。
    assert!(rows[1].lanes.contains(&1));
    assert_eq!(rows[1].connectors, vec![0]);
    // base 行及之后：幽灵泳道不应残留。
    assert_eq!(rows[2].lanes, vec![0]);
    assert_eq!(rows[3].lanes, vec![0]);
}

// 合并提交的第二父提交尚未分页加载时，其泳道不应被剪掉：引入行画斜线但不画悬空顶部竖线，
// 下一行该泳道作为贯穿竖线接续，保证线条连续。
#[test]
fn unloaded_parent_lane_stays_continuous() {
    let commits = vec![
        test_commit("merge", &["base", "missing"]),
        test_commit("base", &[]),
    ];

    let rows = commit_graph_rows(&commits);

    assert!(rows[0].connectors.contains(&1));
    assert!(!rows[0].lanes.contains(&1));
    assert!(rows[1].lanes.contains(&1));
}

// 可见泳道上限随列宽增长，过窄时回退到 0。
#[test]
fn graph_max_lane_scales_with_width() {
    assert_eq!(graph_max_lane(20.0), 0);
    assert_eq!(graph_max_lane(64.0), 3);
    assert_eq!(graph_max_lane(96.0), 5);
    assert_eq!(graph_max_lane(480.0), 32);
}

// 淡化判定：高亮激活时谱系外的行淡化（合并与否不影响；谱系内的合并提交正是
// 分支吸收其他线索的位置，保持正常）；未激活高亮时仅按开关淡化合并提交。
#[test]
fn graph_row_dimmed_combines_trace_and_merge_rules() {
    // 无高亮：默认不淡化任何行
    assert!(!graph_row_dimmed(None, false, false));
    assert!(!graph_row_dimmed(None, true, false));
    // 无高亮 + 开关开启：合并提交淡化
    assert!(!graph_row_dimmed(None, false, true));
    assert!(graph_row_dimmed(None, true, true));
    // 高亮激活：谱系外一律淡化，谱系内一律正常（开关不再额外起作用）
    assert!(graph_row_dimmed(Some(false), false, false));
    assert!(graph_row_dimmed(Some(false), true, false));
    assert!(!graph_row_dimmed(Some(true), true, false));
    assert!(!graph_row_dimmed(Some(true), true, true));
}

// 搜索过滤：子串命中摘要/作者/短 SHA（大小写不敏感），空查询返回全量索引。
#[test]
fn filter_graph_commits_matches_summary_author_and_short_oid() {
    let mut commit = test_commit("abcd1234", &[]);
    commit.summary = "修复登录超时".to_string();
    commit.author = "Alice".to_string();
    let other = test_commit("ffff0000", &[]);

    let commits = vec![commit, other];

    // 空查询 = 全量
    assert_eq!(filter_graph_commits(&commits, ""), vec![0, 1]);
    assert_eq!(filter_graph_commits(&commits, "   "), vec![0, 1]);
    // 摘要命中（中文子串）
    assert_eq!(filter_graph_commits(&commits, "登录"), vec![0]);
    // 作者命中（大小写不敏感）
    assert_eq!(filter_graph_commits(&commits, "alice"), vec![0]);
    // 短 SHA 命中
    assert_eq!(filter_graph_commits(&commits, "ABCD"), vec![0]);
    // 无命中
    assert!(filter_graph_commits(&commits, "不存在").is_empty());
}

// 行高与主历史页一致：泳道跨行连续依赖统一 48px 行高。
#[test]
fn commit_graph_row_height_matches_history_rows() {
    assert_eq!(COMMIT_GRAPH_ROW_HEIGHT, 48.0);
    assert!(GRAPH_REF_LABEL_CAP > 1);
}
