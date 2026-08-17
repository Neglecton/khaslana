use crate::git::test_support::git_test_support as git_support;
use crate::types::{CommitRefKind, DiffEncodingChoice, HistoryScope};
use std::path::Path;

/// 构建测试仓库（main 分支三个提交）：
/// - c1：初始 a.txt（5 行）+ b.txt
/// - c2：只改 b.txt（不触及 a.txt）
/// - c3：只改 a.txt 的第 3 行
fn build_history_repo() -> (tempfile::TempDir, Vec<git2::Oid>) {
    let (dir, repo, _svc) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "a\nb\nc\nd\ne\n");
    git_support::write_file(dir.path(), "b.txt", "one\n");
    let c1 = git_support::commit_all(&repo, "init a and b");

    git_support::write_file(dir.path(), "b.txt", "one\ntwo\n");
    let c2 = git_support::commit_all(&repo, "touch b only");

    git_support::write_file(dir.path(), "a.txt", "a\nb\nC\nd\ne\n");
    let c3 = git_support::commit_all(&repo, "touch a line 3");

    (dir, vec![c1, c2, c3])
}

#[test]
fn file_history_returns_only_commits_touching_path() {
    let (dir, oids) = build_history_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (commits, _refs) = svc
        .file_history(&repo, HistoryScope::CurrentBranch, "a.txt", 0, 10, None)
        .unwrap();

    let oid_strings: Vec<String> = commits.iter().map(|c| c.oid.clone()).collect();
    // c2 只改了 b.txt，不应出现；顺序与 revwalk 一致（新→旧）
    assert_eq!(oid_strings, vec![oids[2].to_string(), oids[0].to_string()]);
    // 徽章照常填充（HEAD 标记在最新提交上）
    assert!(
        commits[0]
            .refs
            .iter()
            .any(|r| r.kind == CommitRefKind::Head)
    );
}

#[test]
fn file_history_pagination_on_filtered_stream() {
    let (dir, oids) = build_history_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (page1, _) = svc
        .file_history(&repo, HistoryScope::CurrentBranch, "a.txt", 0, 1, None)
        .unwrap();
    let (page2, _) = svc
        .file_history(&repo, HistoryScope::CurrentBranch, "a.txt", 1, 1, None)
        .unwrap();
    let (page3, _) = svc
        .file_history(&repo, HistoryScope::CurrentBranch, "a.txt", 2, 1, None)
        .unwrap();

    // 分页基于过滤后的流：无重复、无 c2、越界为空
    assert_eq!(page1.len(), 1);
    assert_eq!(page1[0].oid, oids[2].to_string());
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].oid, oids[0].to_string());
    assert!(page3.is_empty());
}

#[test]
fn file_history_all_refs_scope() {
    let (dir, oids) = build_history_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (commits, _refs) = svc
        .file_history(&repo, HistoryScope::AllRefs, "a.txt", 0, 10, None)
        .unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].oid, oids[2].to_string());
}

#[test]
fn file_history_untouched_path_is_empty() {
    let (dir, _oids) = build_history_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let (commits, _) = svc
        .file_history(
            &repo,
            HistoryScope::CurrentBranch,
            "not-exist.txt",
            0,
            10,
            None,
        )
        .unwrap();
    assert!(commits.is_empty());
}

/// 构建追溯测试仓库：两个提交分别改 a.txt 的不同行段。
fn build_blame_repo() -> (tempfile::TempDir, git2::Oid, git2::Oid) {
    let (dir, repo, _svc) = git_support::init_repo();
    git_support::write_file(dir.path(), "a.txt", "a\nb\nc\nd\ne\n");
    let first = git_support::commit_all(&repo, "first commit");

    git_support::write_file(dir.path(), "a.txt", "a\nb\nC\nd\ne\n");
    let second = git_support::commit_all(&repo, "second commit");
    (dir, first, second)
}

#[test]
fn blame_file_aligns_lines_and_groups_hunks() {
    let (dir, first, second) = build_blame_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let view = svc
        .blame_file(&repo, Path::new("a.txt"), DiffEncodingChoice::Auto)
        .unwrap();

    assert_eq!(view.path, "a.txt");
    assert_eq!(view.lines, vec!["a", "b", "C", "d", "e"]);
    // 行号与内容对齐：1 基起始 + 行数覆盖全部行
    assert_eq!(view.line_hunk.len(), view.lines.len());
    let covered: usize = view.hunks.iter().map(|h| h.line_count).sum();
    assert_eq!(covered, view.lines.len());

    // 行 -> 块索引反查正确（第 3 行属于第二个提交的块）
    let line3_hunk = &view.hunks[view.line_hunk[2]];
    assert_eq!(line3_hunk.commit.as_ref().unwrap().oid, second.to_string());
    assert_eq!(line3_hunk.start_line, 3);
    assert_eq!(line3_hunk.line_count, 1);

    // 其余行归属第一个提交
    for index in [0usize, 1, 3, 4] {
        let hunk = &view.hunks[view.line_hunk[index]];
        assert_eq!(hunk.commit.as_ref().unwrap().oid, first.to_string());
    }

    // 提交信息填充：作者与摘要来自测试签名与提交信息
    let info = line3_hunk.commit.as_ref().unwrap();
    assert_eq!(info.author, "Test User");
    assert_eq!(info.summary, "second commit");
    assert!(!info.short_oid.is_empty());
}

#[test]
fn blame_file_marks_uncommitted_lines_with_none() {
    let (dir, first, second) = build_blame_repo();
    // 工作区再改第 1 行（未暂存）：blame_buffer 路径
    git_support::write_file(dir.path(), "a.txt", "AA\nb\nC\nd\ne\n");
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let view = svc
        .blame_file(&repo, Path::new("a.txt"), DiffEncodingChoice::Auto)
        .unwrap();

    // 行数组来自工作区内容
    assert_eq!(view.lines, vec!["AA", "b", "C", "d", "e"]);
    // 第 1 行未提交：commit 为 None
    let uncommitted = &view.hunks[view.line_hunk[0]];
    assert!(uncommitted.commit.is_none());
    assert_eq!(uncommitted.start_line, 1);
    // 已提交行的归属不受影响
    let line3 = &view.hunks[view.line_hunk[2]];
    assert_eq!(line3.commit.as_ref().unwrap().oid, second.to_string());
    let line5 = &view.hunks[view.line_hunk[4]];
    assert_eq!(line5.commit.as_ref().unwrap().oid, first.to_string());
}

#[test]
fn blame_file_missing_in_head_returns_error() {
    let (dir, _first, _second) = build_blame_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let err = svc
        .blame_file(&repo, Path::new("nope.txt"), DiffEncodingChoice::Auto)
        .unwrap_err();
    assert!(err.to_string().contains("该文件尚未提交"));
}

#[test]
fn blame_file_rejects_binary() {
    let (dir, repo, _svc) = git_support::init_repo();
    git_support::write_bytes(dir.path(), "bin.dat", &[0x00, 0x01, 0x02, 0x00]);
    git_support::commit_all(&repo, "add binary");
    drop(repo);

    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();
    let err = svc
        .blame_file(&repo, Path::new("bin.dat"), DiffEncodingChoice::Auto)
        .unwrap_err();
    assert!(err.to_string().contains("二进制文件不支持追溯"));
}

// ===== 换行风格回归（Windows core.autocrlf 场景）=====

/// 构建换行风格测试仓库：core.autocrlf=true（工作区 CRLF、blob LF）。
fn build_crlf_repo() -> (tempfile::TempDir, git2::Oid) {
    let (dir, repo, _svc) = git_support::init_repo();
    // 模拟 Windows 常见配置：提交时 CRLF 归一化为 LF，工作区保持 CRLF
    repo.config()
        .unwrap()
        .set_str("core.autocrlf", "true")
        .unwrap();
    git_support::write_file(dir.path(), "a.txt", "a\r\nb\r\nc\r\n");
    let commit = git_support::commit_all(&repo, "crlf commit");
    drop(repo);
    (dir, commit)
}

// 工作区与 HEAD 内容一致（仅换行风格不同）时，不应把整文件判成未提交。
#[test]
fn blame_file_crlf_clean_workdir_all_attributed() {
    let (dir, commit) = build_crlf_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let view = svc
        .blame_file(&repo, Path::new("a.txt"), DiffEncodingChoice::Auto)
        .unwrap();

    // 三行全部归属提交，没有任何「未提交」行
    assert_eq!(view.lines, vec!["a", "b", "c"]);
    for hunk in &view.hunks {
        assert!(
            hunk.commit.is_some(),
            "干净工作区（CRLF/LF 差异）不应出现未提交行"
        );
        assert_eq!(hunk.commit.as_ref().unwrap().oid, commit.to_string());
    }
}

// CRLF 工作区 + 真实改动：改动行「未提交」，未改行仍归属提交。
#[test]
fn blame_file_crlf_partial_edit_attributed() {
    let (dir, commit) = build_crlf_repo();
    // 工作区改第 2 行（未暂存，保持 CRLF）
    git_support::write_file(dir.path(), "a.txt", "a\r\nB\r\nc\r\n");
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();

    let view = svc
        .blame_file(&repo, Path::new("a.txt"), DiffEncodingChoice::Auto)
        .unwrap();

    assert_eq!(view.lines, vec!["a", "B", "c"]);
    let edited = &view.hunks[view.line_hunk[1]];
    assert!(edited.commit.is_none(), "改动行应为未提交");
    for index in [0usize, 2] {
        let hunk = &view.hunks[view.line_hunk[index]];
        assert_eq!(
            hunk.commit.as_ref().unwrap().oid,
            commit.to_string(),
            "未改行应归属提交（不被换行差异吞掉）"
        );
    }
}

// blob 本身是 CRLF（未开 autocrlf）且工作区被清空：空内容、无块，不报错
//（git_blame_buffer 断言 buffer 非空，需绕过）。
#[test]
fn blame_file_emptied_workdir_returns_empty_view() {
    let (dir, commit) = build_crlf_repo();
    git_support::write_file(dir.path(), "a.txt", "");
    let repo = git2::Repository::open(dir.path()).unwrap();
    let svc = git_support::service();
    let _ = commit;

    let view = svc
        .blame_file(&repo, Path::new("a.txt"), DiffEncodingChoice::Auto)
        .unwrap();
    assert!(view.lines.is_empty());
    assert!(view.hunks.is_empty());
}
