use super::*;

use crate::ai::review::AiReviewStep;

fn record(repo_path: &str, created_at: u64, content: &str) -> AiReviewRecord {
    AiReviewRecord {
        id: String::new(),
        repo_path: repo_path.to_string(),
        target_display_name: "feature/x".into(),
        target_commit_oid: "0123456789abcdef".into(),
        model: "test-model".into(),
        created_at_millis: created_at,
        duration_secs: 12,
        file_count: 3,
        result: AiReviewResult {
            content: content.to_string(),
            reasoning: None,
            steps: vec![AiReviewStep::Reasoning {
                text: "先看改动".into(),
            }],
        },
    }
}

#[test]
fn save_and_list_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    save_review_record(dir.path(), record("D:/repo/a", 1000, "结论 A")).unwrap();
    save_review_record(dir.path(), record("D:/repo/a", 2000, "结论 B")).unwrap();

    let records = list_review_records(dir.path(), "D:/repo/a", 10).unwrap();
    assert_eq!(records.len(), 2);
    // 倒序：最新在前。
    assert_eq!(records[0].result.content, "结论 B");
    assert_eq!(records[0].id, "2000");
    assert_eq!(records[0].model, "test-model");
    // 完整轨迹随记录往返。
    assert_eq!(records[1].result.steps.len(), 1);

    // 不同仓库互相隔离；路径斜杠差异归一到同一 key。
    let other = list_review_records(dir.path(), "D:/repo/b", 10).unwrap();
    assert!(other.is_empty());
    let same = list_review_records(dir.path(), "D:/repo/a/", 10).unwrap();
    assert_eq!(same.len(), 2);
}

#[test]
fn same_millisecond_records_get_suffixed_ids() {
    let dir = tempfile::tempdir().unwrap();
    let first = save_review_record(dir.path(), record("r", 5000, "1")).unwrap();
    let second = save_review_record(dir.path(), record("r", 5000, "2")).unwrap();
    assert_eq!(first, "5000");
    assert_eq!(second, "5000-2");

    // 倒序时同毫秒后创建的（-2 后缀）在前。
    let records = list_review_records(dir.path(), "r", 10).unwrap();
    assert_eq!(records[0].id, "5000-2");
    assert_eq!(records[1].id, "5000");
}

#[test]
fn prune_keeps_newest_max_stored_records() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..(MAX_STORED_RECORDS + 5) {
        save_review_record(
            dir.path(),
            record("r", 1000 + index as u64, &format!("v{index}")),
        )
        .unwrap();
    }
    let records = list_review_records(dir.path(), "r", 100).unwrap();
    assert_eq!(records.len(), MAX_STORED_RECORDS);
    // 保留的是最新的（时间戳最大的）。
    assert_eq!(
        records[0].id,
        (1000 + MAX_STORED_RECORDS as u64 + 4).to_string()
    );
    assert_eq!(
        records[0].result.content,
        format!("v{}", MAX_STORED_RECORDS + 4)
    );
}

#[test]
fn list_skips_corrupted_files() {
    let dir = tempfile::tempdir().unwrap();
    save_review_record(dir.path(), record("r", 1000, "好的")).unwrap();
    let repo_dir = dir.path().join("ai-reviews").join(repo_key("r"));
    fs::write(repo_dir.join("9999.json"), "{broken json").unwrap();

    let records = list_review_records(dir.path(), "r", 10).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].result.content, "好的");
}

#[test]
fn repo_key_is_stable_and_normalized() {
    // 同一路径（含尾部斜杠差异）同 key；不同路径不同 key。
    assert_eq!(repo_key("D:/repo/a"), repo_key("D:/repo/a/"));
    assert_ne!(repo_key("D:/repo/a"), repo_key("D:/repo/b"));
    // FNV-1a 已知向量："a" → 0xe40c292c。
    assert_eq!(repo_key("a"), "e40c292c");
}
#[test]
fn repo_key_folds_case_for_windows_paths() {
    // Windows 路径大小写不敏感：同一仓库不同大小写写法必须落到同一记录目录，
    // 否则盘符/目录大小写变化就会让旧记录「失联」。
    assert_eq!(repo_key("D:/Repo/Project"), repo_key("d:/repo/project"));
    assert_eq!(repo_key("D:/Repo/"), repo_key("d:/REPO"));
    // 折叠键与旧键（未折叠）对纯小写路径一致，对含大写路径不同。
    assert_eq!(repo_key("d:/repo"), legacy_repo_key("d:/repo"));
    assert_ne!(repo_key("D:/Repo"), legacy_repo_key("D:/Repo"));
}

#[test]
fn list_merges_legacy_unfolded_key_dir() {
    let dir = tempfile::tempdir().unwrap();
    let repo_path = "D:/MyRepo";
    // 新键目录（大小写折叠后）正常写入一条。
    save_review_record(dir.path(), record(repo_path, 2000, "新记录")).unwrap();
    // 旧键目录（折叠规则上线前的键）手工放一条旧记录，模拟升级前落盘。
    let legacy_dir = dir
        .path()
        .join("ai-reviews")
        .join(legacy_repo_key(repo_path));
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy = record(repo_path, 1000, "旧记录");
    std::fs::write(
        legacy_dir.join("1000.json"),
        serde_json::to_string(&legacy).unwrap(),
    )
    .unwrap();

    let records = list_review_records(dir.path(), repo_path, 20).unwrap();
    assert_eq!(records.len(), 2);
    // 按时间倒序，旧键目录的记录仍可见（兼容读取，不再写入）。
    assert_eq!(records[0].result.content, "新记录");
    assert_eq!(records[1].result.content, "旧记录");
}
