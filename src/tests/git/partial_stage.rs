use std::collections::HashSet;
use std::path::Path;

use git2::Repository;

use super::*;
use crate::git::test_support::git_test_support as git_support;

/// 读取 index 中指定路径的 blob 内容（部分暂存断言用）。
fn staged_blob_bytes(repo: &Repository, path: &str) -> Vec<u8> {
    let index = repo.index().unwrap();
    let entry = index.get_path(Path::new(path), 0).unwrap();
    let blob = repo.find_blob(entry.id).unwrap();
    blob.content().to_vec()
}

fn selection_of(items: &[(SelectionSide, u32)]) -> LineSelection {
    items
        .iter()
        .map(|&(side, lineno)| SelectedDiffLine { side, lineno })
        .collect::<HashSet<_>>()
}

/// 构造两个独立改动块的多行文件基线，返回 (仓库目录, 修改后内容)。
/// 基线 10 行；修改点：第 3 行改内容、第 6-7 行之间插入两行、第 9 行改内容。
#[allow(dead_code)]
fn partial_stage_baseline() -> (tempfile::TempDir, String) {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
    git_support::commit_all(&repo, "base");
    let modified = "1\n2\n3-mod\n4\n5\n6\n6a\n6b\n7\n8\n9-mod\n10\n";
    git_support::write_file(dir.path(), "a.txt", modified);
    // 触发一次快照避免未使用告警；基线仓库留给调用方。
    let _ = service.status_full(&repo).unwrap();
    (dir, modified.to_string())
}

#[test]
fn stage_lines_stages_single_added_line_pair() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n");
    git_support::commit_all(&repo, "base");
    // 在第 2 行后插入一行：唯一 '+' 行 new_lineno = 3。
    git_support::write_file(dir.path(), "a.txt", "1\n2\ninserted\n3\n4\n5\n");

    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Added, 3)]),
        )
        .unwrap();

    // index = 原内容 + 仅选中的插入行；工作区保持全部改动。
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        "1\n2\ninserted\n3\n4\n5\n",
    );
    // 工作区未动：全部改动仍显示，且已全部入暂存（index == 工作区）。
    git_support::assert_file_text(dir.path(), "a.txt", "1\n2\ninserted\n3\n4\n5\n");
    // 改动已全部入暂存：不存在未暂存变更（index 与 HEAD 的差异属于已暂存侧）。
    let changes = service.status_full(&repo).unwrap();
    assert!(
        changes.iter().all(|change| change.unstaged.is_none()),
        "不应有未暂存变更：{changes:?}"
    );
}

#[test]
fn stage_lines_partial_of_multiple_additions() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
    git_support::commit_all(&repo, "base");
    // 插入三行：new_lineno 3、4、5；只暂存第 3、5 两行。
    git_support::write_file(dir.path(), "a.txt", "1\n2\nx\ny\nz\n3\n4\n5\n6\n7\n8\n");

    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Added, 3), (SelectionSide::Added, 5)]),
        )
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        "1\n2\nx\nz\n3\n4\n5\n6\n7\n8\n"
    );
    // 工作区未动：仍包含全部三行插入。
    git_support::assert_file_text(dir.path(), "a.txt", "1\n2\nx\ny\nz\n3\n4\n5\n6\n7\n8\n");
}

#[test]
fn stage_lines_modification_pair_both_sides() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n");
    git_support::commit_all(&repo, "base");
    // 修改第 3 行：-3(旧 old_lineno=3) +3-mod(新 new_lineno=3)。
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3-mod\n4\n5\n");

    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Removed, 3), (SelectionSide::Added, 3)]),
        )
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        "1\n2\n3-mod\n4\n5\n"
    );
}

#[test]
fn stage_lines_whole_hunk_via_all_changes_selected() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n");
    git_support::commit_all(&repo, "base");
    git_support::write_file(dir.path(), "a.txt", "1-mod\n2\n3\n4\n5-mod\n");

    // 全选（两个修改对的四行）走整块快路径。
    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[
                (SelectionSide::Removed, 1),
                (SelectionSide::Added, 1),
                (SelectionSide::Removed, 5),
                (SelectionSide::Added, 5),
            ]),
        )
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        "1-mod\n2\n3\n4\n5-mod\n"
    );
}

#[test]
fn stage_lines_whole_hunk_from_second_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；第 5 行与第 35 行各改一处，间隔 29 行 >> 6 行上下文 -> 两个独立 hunk。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = (1..=40)
        .map(|i| {
            if i == 5 || i == 35 {
                format!("{i}-mod\n")
            } else {
                format!("{i}\n")
            }
        })
        .collect();
    git_support::write_file(dir.path(), "a.txt", &modified);

    // 只暂存第二个 hunk（35 -> 35-mod）：跳过第一个 hunk，patch 只含第二个块。
    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Removed, 35), (SelectionSide::Added, 35)]),
        )
        .unwrap();

    // index 只含第二处修改，第一处保持基线；工作区两处都在。
    let expect_staged: String = (1..=40)
        .map(|i| {
            if i == 35 {
                format!("{i}-mod\n")
            } else {
                format!("{i}\n")
            }
        })
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        expect_staged
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn stage_lines_second_hunk_after_insertion_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；hunk1 = 第 5 行后插入 3 行（净 +3），hunk2 = 第 40 行修改。
    // hunk2 的 hunk 头 new_start 相对 old_start 偏移 +3（受 hunk1 插入影响）。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = (1..=43)
        .map(|i| match i {
            5..=7 => format!("ins{i}\n"),
            43 => "40-mod\n".to_string(),
            i if i < 5 => format!("{i}\n"),
            _ => format!("{}\n", i - 3),
        })
        .collect();
    git_support::write_file(dir.path(), "a.txt", &modified);

    // 只暂存 hunk2：Removed old_lineno=40（index 侧），Added new_lineno=43（workdir 侧）。
    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Removed, 40), (SelectionSide::Added, 43)]),
        )
        .unwrap();

    // index = 基线 + 仅第 40 行修改。
    let expect_staged: String = (1..=40)
        .map(|i| {
            if i == 40 {
                "40-mod\n".to_string()
            } else {
                format!("{i}\n")
            }
        })
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        expect_staged
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn stage_lines_partial_after_dropped_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；hunk1 = 第 4 行后插入 3 行（被丢弃），hunk2 = 文件尾追加 N1/N2，
    // 只按行暂存 N1（Added new_lineno=44）。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = "1\n2\n3\n4\nA1\nA2\nA3\n".to_string()
        + &(5..=40).map(|i| format!("{i}\n")).collect::<String>()
        + "N1\nN2\n";
    git_support::write_file(dir.path(), "a.txt", &modified);

    service
        .stage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Added, 44)]),
        )
        .unwrap();

    // index = 基线 + 仅 N1；工作区保持全部改动。
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        format!("{base}N1\n")
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn unstage_lines_second_hunk_after_insertion_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；全部暂存后：hunk1 = 插入 3 行（保留），hunk2 = 旧第 40 行修改
    // （整块取消暂存）。反向整块快路径 + 前序块被丢弃。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = "1\n2\n3\n4\nA1\nA2\nA3\n".to_string()
        + &(5..=39).map(|i| format!("{i}\n")).collect::<String>()
        + "40-mod\n";
    git_support::write_file(dir.path(), "a.txt", &modified);
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.txt")).unwrap();
    index.write().unwrap();

    // 只取消暂存第二个块：staged diff 中 -40(old_lineno=40) +40-mod(new_lineno=43)。
    service
        .unstage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Removed, 40), (SelectionSide::Added, 43)]),
        )
        .unwrap();

    // index = 基线 + 仅插入的 3 行；工作区保持全部改动。
    let expect_index: String = "1\n2\n3\n4\nA1\nA2\nA3\n".to_string()
        + &(5..=40).map(|i| format!("{i}\n")).collect::<String>();
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        expect_index
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn unstage_lines_partial_after_insertion_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；全部暂存后：hunk1 = 插入 3 行（保留），hunk2 = 文件尾追加 N1/N2，
    // 只按行取消暂存 N2（Added new_lineno=45）。反向按行路径 + 前序块被丢弃。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = "1\n2\n3\n4\nA1\nA2\nA3\n".to_string()
        + &(5..=40).map(|i| format!("{i}\n")).collect::<String>()
        + "N1\nN2\n";
    git_support::write_file(dir.path(), "a.txt", &modified);
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.txt")).unwrap();
    index.write().unwrap();

    service
        .unstage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Added, 45)]),
        )
        .unwrap();

    // index = 基线 + 插入 3 行 + N1；工作区保持全部改动。
    let expect_index: String = "1\n2\n3\n4\nA1\nA2\nA3\n".to_string()
        + &(5..=40).map(|i| format!("{i}\n")).collect::<String>()
        + "N1\n";
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        expect_index
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn stage_lines_sequential_hunks() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 40 行基线；两处独立修改：第 5 行与第 35 行。
    let base: String = (1..=40).map(|i| format!("{i}\n")).collect();
    git_support::write_file(dir.path(), "a.txt", &base);
    git_support::commit_all(&repo, "base");
    let modified: String = (1..=40)
        .map(|i| {
            if i == 5 || i == 35 {
                format!("{i}-mod\n")
            } else {
                format!("{i}\n")
            }
        })
        .collect();
    git_support::write_file(dir.path(), "a.txt", &modified);

    // 模拟 UI：从 diff_for_path 拿当前 diff，按 hunk_index 构造整块选择。
    let hunk_selection = |repo: &Repository, service: &crate::git::GitService, hunk: usize| {
        let diff = service
            .diff_for_path(
                repo,
                Path::new("a.txt"),
                crate::types::DiffScope::Unstaged,
                false,
                crate::types::DiffEncodingChoice::Utf8,
            )
            .unwrap();
        let mut selection = LineSelection::new();
        for line in diff.lines.iter().filter(|line| line.hunk_index == hunk) {
            match line.kind {
                crate::types::DiffLineKind::Added => {
                    selection.insert(SelectedDiffLine {
                        side: SelectionSide::Added,
                        lineno: line.new_lineno.unwrap(),
                    });
                }
                crate::types::DiffLineKind::Removed => {
                    selection.insert(SelectedDiffLine {
                        side: SelectionSide::Removed,
                        lineno: line.old_lineno.unwrap(),
                    });
                }
                _ => {}
            }
        }
        assert!(!selection.is_empty(), "hunk {hunk} 应有可暂存行");
        selection
    };

    // 先暂存第一个块，再暂存第二个块（各自基于刷新后的 diff）。
    let first = hunk_selection(&repo, &service, 1);
    service
        .stage_lines(&mut repo, Path::new("a.txt"), &first)
        .unwrap();
    let second = hunk_selection(&repo, &service, 1);
    service
        .stage_lines(&mut repo, Path::new("a.txt"), &second)
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        modified
    );
    git_support::assert_file_text(dir.path(), "a.txt", &modified);
}

#[test]
fn unstage_lines_reverts_selected_staged_change() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n2\n3\n4\n5\n");
    git_support::commit_all(&repo, "base");
    // 暂存两个独立修改：第 2 行、第 4 行。
    git_support::write_file(dir.path(), "a.txt", "1\n2-mod\n3\n4-mod\n5\n");
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.txt")).unwrap();
    index.write().unwrap();

    // 取消暂存第 4 行的修改：staged diff 中 -4(old_lineno=4) +4-mod(new_lineno=4)。
    service
        .unstage_lines(
            &mut repo,
            Path::new("a.txt"),
            &selection_of(&[(SelectionSide::Removed, 4), (SelectionSide::Added, 4)]),
        )
        .unwrap();

    // index 回退第 4 行，保留第 2 行的暂存修改。
    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "a.txt")),
        "1\n2-mod\n3\n4\n5\n"
    );
    // 工作区保持全部修改（含 4-mod）。
    git_support::assert_file_text(dir.path(), "a.txt", "1\n2-mod\n3\n4-mod\n5\n");
}

#[test]
fn stage_lines_preserves_crlf_bytes() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 显式关闭 autocrlf：Windows 全局配置常为 true，会让 libgit2 在 diff/apply
    // 层把 CRLF 归一化为 LF，破坏字节保真断言的确定性。
    repo.config()
        .unwrap()
        .set_str("core.autocrlf", "false")
        .unwrap();
    // 基线与修改均用 CRLF（write_file 写入 \r\n）。
    git_support::write_file(dir.path(), "crlf.txt", "a\r\nb\r\nc\r\n");
    git_support::commit_all(&repo, "base");
    git_support::write_file(dir.path(), "crlf.txt", "a\r\nb\r\ninserted\r\nc\r\n");

    service
        .stage_lines(
            &mut repo,
            Path::new("crlf.txt"),
            &selection_of(&[(SelectionSide::Added, 3)]),
        )
        .unwrap();

    let staged = staged_blob_bytes(&repo, "crlf.txt");
    let crlf: &[u8] = b"\r\n";
    let staged_has_crlf = staged.windows(2).any(|w| w == crlf);
    assert_eq!(
        String::from_utf8_lossy(&staged),
        "a\r\nb\r\ninserted\r\nc\r\n"
    );
    assert!(
        staged_has_crlf,
        "暂存内容必须保留 CRLF 字节，实际：{:?}",
        staged
    );
}

#[test]
fn stage_lines_no_trailing_newline_whole_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "nonl.txt", "one\ntwo");
    git_support::commit_all(&repo, "base");
    git_support::write_file(dir.path(), "nonl.txt", "one\ntwo\nthree");

    // 全选（修改涉及 -two/+two/+three 三行，含 EOFNL 标记）走整块原样快路径。
    service
        .stage_lines(
            &mut repo,
            Path::new("nonl.txt"),
            &selection_of(&[
                (SelectionSide::Removed, 2),
                (SelectionSide::Added, 2),
                (SelectionSide::Added, 3),
            ]),
        )
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&staged_blob_bytes(&repo, "nonl.txt")),
        "one\ntwo\nthree"
    );
}

#[test]
fn stage_lines_rejects_partial_selection_in_eofnl_hunk() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "nonl.txt", "one\ntwo");
    git_support::commit_all(&repo, "base");
    // 末行无换行时插入两行：块内含 EOFNL 标记。
    git_support::write_file(dir.path(), "nonl.txt", "one\ntwo\nx\ny");

    let error = service
        .stage_lines(
            &mut repo,
            Path::new("nonl.txt"),
            &selection_of(&[(SelectionSide::Added, 4)]),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("无尾换行"), "实际错误：{error}");
}

#[test]
fn stage_lines_rejects_untracked_and_deleted_and_binary() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "base");

    // 未跟踪文件：重建 diff 无内容 → 明确报错。
    git_support::write_file(dir.path(), "new.txt", "untracked\n");
    let error = service
        .stage_lines(
            &mut repo,
            Path::new("new.txt"),
            &selection_of(&[(SelectionSide::Added, 1)]),
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("没有可部分") || error.contains("未跟踪"),
        "实际错误：{error}"
    );

    // 删除文件：delta Deleted 守卫。
    std::fs::remove_file(dir.path().join("base.txt")).unwrap();
    let error = service
        .stage_lines(
            &mut repo,
            Path::new("base.txt"),
            &selection_of(&[(SelectionSide::Removed, 1)]),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("整文件"), "实际错误：{error}");

    // 二进制文件：'B' 行守卫。
    let (dir2, mut repo2, service2) = git_support::init_repo();
    git_support::write_bytes(dir2.path(), "bin.dat", b"\x00\x01\x02");
    git_support::commit_all(&repo2, "base");
    git_support::write_bytes(dir2.path(), "bin.dat", b"\x00\x01\x02\x03\x04");
    let error = service2
        .stage_lines(
            &mut repo2,
            Path::new("bin.dat"),
            &selection_of(&[(SelectionSide::Added, 1)]),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("二进制"), "实际错误：{error}");
}

#[test]
fn stage_lines_empty_selection_errors() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "1\n");
    git_support::commit_all(&repo, "base");
    git_support::write_file(dir.path(), "a.txt", "1\n2\n");

    let error = service
        .stage_lines(&mut repo, Path::new("a.txt"), &LineSelection::new())
        .unwrap_err()
        .to_string();
    assert!(error.contains("没有选中"), "实际错误：{error}");
}

// ── build_partial_patch 纯函数单测 ────────────────────────────────

#[test]
fn build_patch_recalculates_new_start_after_dropped_hunk() {
    // hunk1 净 +2（整块丢弃）、hunk2 原头 @@ -10,3 +12,3 @@：
    // 输出补丁的 hunk2 头应重算为 @@ -10,3 +10,3 @@（new_start = old_start + 已输出块累计净行数 0），
    // 否则 libgit2 apply 会按 new_start=12 定位到错误位置。
    let lines = vec![
        raw('F', b"diff --git a/x b/x\n", None, None),
        raw('H', b"@@ -1,4 +1,6 @@\n", None, None),
        raw(' ', b"1\n", Some(1), Some(1)),
        raw('-', b"2\n", Some(2), None),
        raw('+', b"a\n", None, Some(2)),
        raw('+', b"b\n", None, Some(3)),
        raw(' ', b"3\n", Some(3), Some(4)),
        raw(' ', b"4\n", Some(4), Some(5)),
        raw('H', b"@@ -10,3 +12,3 @@\n", None, None),
        raw(' ', b"9\n", Some(9), Some(11)),
        raw('-', b"10\n", Some(10), None),
        raw('+', b"10m\n", None, Some(12)),
        raw(' ', b"11\n", Some(11), Some(13)),
    ];
    let selection = selection_of(&[(SelectionSide::Removed, 10), (SelectionSide::Added, 12)]);
    let patch = build_partial_patch(&lines, &selection, false)
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&patch).into_owned();
    assert!(text.contains("@@ -10,3 +10,3 @@"), "实际：{text}");
    // 被丢弃的 hunk1 不应出现在输出中。
    assert!(!text.contains("@@ -1,"), "实际：{text}");
}

fn raw(origin: char, content: &[u8], old: Option<u32>, new: Option<u32>) -> RawPatchLine {
    RawPatchLine {
        origin,
        content: content.to_vec(),
        old_lineno: old,
        new_lineno: new,
    }
}

#[test]
fn build_patch_forward_downgrades_and_drops() {
    // @@ -1,3 +1,4 @@：ctx1, -2(未选), +new(未选), +sel(选中), ctx3
    let lines = vec![
        raw('F', b"diff --git a/x b/x\n", None, None),
        raw('H', b"@@ -1,3 +1,4 @@\n", None, None),
        raw(' ', b"1\n", Some(1), Some(1)),
        raw('-', b"2\n", Some(2), None),
        raw('+', b"new\n", None, Some(3)),
        raw('+', b"sel\n", None, Some(4)),
        raw(' ', b"3\n", Some(3), Some(5)),
    ];
    let patch = build_partial_patch(&lines, &selection_of(&[(SelectionSide::Added, 4)]), false)
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&patch).into_owned();
    // 未选 '-'(2) 降级为上下文；未选 '+'(new) 丢弃；头重算：
    // preimage = ctx(1, 2, 3) 共 3 行；postimage = ctx + sel 共 4 行。
    assert!(text.contains("@@ -1,3 +1,4 @@"), "实际：{text}");
    assert!(text.contains(" 2\n"), "未选删除行应降级为上下文：{text}");
    assert!(!text.contains("+new\n"), "未选新增行应被丢弃：{text}");
    assert!(text.contains("+sel\n"), "选中行保留：{text}");
}

#[test]
fn build_patch_reverse_swaps_sides() {
    // staged diff：@@ -1,2 +1,2 @@ ctx1, -old(选), +new(选), ctx2 → 反向全选。
    let lines = vec![
        raw('F', b"diff --git a/x b/x\n", None, None),
        raw('H', b"@@ -1,2 +1,2 @@\n", None, None),
        raw(' ', b"1\n", Some(1), Some(1)),
        raw('-', b"old\n", Some(2), None),
        raw('+', b"new\n", None, Some(2)),
        raw(' ', b"2\n", Some(3), Some(3)),
    ];
    let patch = build_partial_patch(
        &lines,
        &selection_of(&[(SelectionSide::Removed, 2), (SelectionSide::Added, 2)]),
        true,
    )
    .unwrap()
    .unwrap();
    let text = String::from_utf8_lossy(&patch).into_owned();
    assert!(
        text.contains("@@ -1,2 +1,2 @@"),
        "两侧同计数交换后不变：{text}"
    );
    assert!(text.contains("+old\n"), "反向后旧内容成为新增：{text}");
    assert!(text.contains("-new\n"), "反向后 index 内容成为删除：{text}");
}

#[test]
fn build_patch_reverse_partial_keeps_index_side_as_context() {
    // staged：ctx1, -old(未选), +new(未选), +sel(选), ctx2 → 仅取消暂存 sel。
    let lines = vec![
        raw('H', b"@@ -1,3 +1,4 @@\n", None, None),
        raw(' ', b"1\n", Some(1), Some(1)),
        raw('-', b"old\n", Some(2), None),
        raw('+', b"new\n", None, Some(3)),
        raw('+', b"sel\n", None, Some(4)),
        raw(' ', b"2\n", Some(3), Some(5)),
    ];
    let patch = build_partial_patch(&lines, &selection_of(&[(SelectionSide::Added, 4)]), true)
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&patch).into_owned();
    // preimage(index) = ctx1, new(上下文化), ctx2 → 3 行；postimage 含 -sel → 2 行。
    // 反向：pre 侧行号来自 new_lineno（1,3,5→? ctx1=1, new=3, ctx2=5）。
    assert!(
        text.contains(" new\n"),
        "未选 index 内容降级为上下文：{text}"
    );
    assert!(text.contains("-sel\n"), "选中 index 内容反向为删除：{text}");
    assert!(!text.contains("+old\n"), "未选 HEAD 内容应被丢弃：{text}");
    assert!(
        !text.contains(" old\n"),
        "未选 HEAD 内容不应作为上下文：{text}"
    );
}

#[test]
fn build_patch_returns_none_when_nothing_selected() {
    let lines = vec![
        raw('F', b"diff --git a/x b/x\n", None, None),
        raw('H', b"@@ -1,2 +1,2 @@\n", None, None),
        raw(' ', b"1\n", Some(1), Some(1)),
        raw('+', b"x\n", None, Some(2)),
        raw(' ', b" 2\n", Some(2), Some(3)),
    ];
    let patch = build_partial_patch(&lines, &LineSelection::new(), false).unwrap();
    assert!(patch.is_none());
}

#[test]
fn parse_and_format_hunk_header_round_trip() {
    assert_eq!(
        parse_hunk_header(b"@@ -12,3 +13,4 @@ fn main() {\n"),
        Some((12, 3, 13, 4))
    );
    assert_eq!(parse_hunk_header(b"@@ -5 +6 @@\n"), Some((5, 1, 6, 1)));
    assert_eq!(parse_hunk_header(b"@@ -0,0 +1,3 @@\n"), Some((0, 0, 1, 3)));
    let header = String::from_utf8_lossy(&format_hunk_header(12, 3, 13, 4)).into_owned();
    assert_eq!(header, "@@ -12,3 +13,4 @@\n");
    let header = String::from_utf8_lossy(&format_hunk_header(5, 1, 6, 1)).into_owned();
    assert_eq!(header, "@@ -5 +6 @@\n");
    let header = String::from_utf8_lossy(&format_hunk_header(0, 0, 1, 3)).into_owned();
    assert_eq!(header, "@@ -0,0 +1,3 @@\n");
}
