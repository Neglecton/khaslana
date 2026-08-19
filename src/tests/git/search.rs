use super::*;
use crate::git::test_support::git_test_support as git_support;
use git2::ObjectType;

fn head_oid(repo: &git2::Repository) -> String {
    repo.head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

/// 构造测试仓库：嵌套目录 + 多行文本 + 二进制占位。
fn setup_repo() -> (tempfile::TempDir, git2::Repository, GitService) {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(
        dir.path(),
        "src/lib.rs",
        "fn alpha() {}\nfn beta() {}\n// alpha usage\n",
    );
    git_support::write_file(dir.path(), "src/nested/deep.py", "def alpha():\n    pass\n");
    git_support::write_file(dir.path(), "docs.md", "# alpha 说明\n正文没有关键词续行\n");
    // 二进制扩展名 + 内容包含关键词，验证被跳过。
    git_support::write_bytes(dir.path(), "asset.bin", b"alpha\x00binary");
    git_support::commit_all(&repo, "init");
    (dir, repo, service)
}

#[test]
fn search_code_substring_across_files_with_lineno() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    let matches = service
        .search_code(&repo, &oid, "alpha", false, None, 50)
        .unwrap();
    let summarized: Vec<(String, u32)> =
        matches.iter().map(|m| (m.path.clone(), m.lineno)).collect();
    assert_eq!(
        summarized,
        vec![
            ("docs.md".to_string(), 1),
            ("src/lib.rs".to_string(), 1),
            ("src/lib.rs".to_string(), 3),
            ("src/nested/deep.py".to_string(), 1),
        ]
    );
    // 命中行保留原文。
    assert_eq!(matches[0].line, "# alpha 说明");
}

#[test]
fn search_code_regex_match() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    let matches = service
        .search_code(&repo, &oid, r"fn \w+\(\)", true, None, 50)
        .unwrap();
    let paths: Vec<&str> = matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, vec!["src/lib.rs", "src/lib.rs"]);
    assert_eq!(matches[0].line, "fn alpha() {}");
    assert_eq!(matches[1].line, "fn beta() {}");
}

#[test]
fn search_code_regex_compile_error_is_chinese() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    let error = service
        .search_code(&repo, &oid, "(unclosed", true, None, 50)
        .unwrap_err()
        .to_string();
    assert!(error.contains("正则表达式无效"), "got: {error}");
}

#[test]
fn search_code_stops_at_max_results() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    let matches = service
        .search_code(&repo, &oid, "alpha", false, None, 2)
        .unwrap();
    assert_eq!(matches.len(), 2);
}

#[test]
fn search_code_rejects_blank_query() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    let error = service
        .search_code(&repo, &oid, "   ", false, None, 50)
        .unwrap_err()
        .to_string();
    assert_eq!(error, "搜索内容不能为空");
}

#[test]
fn search_code_skips_binary_blob_without_extension() {
    // 扩展名不在兜底名单里的二进制文件，靠 NUL 嗅探跳过。
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "text.txt", "alpha line\nplain\n");
    git_support::write_bytes(dir.path(), "noext", b"alpha\x00\x01\x02");
    git_support::commit_all(&repo, "init");
    let oid = head_oid(&repo);

    let matches = service
        .search_code(&repo, &oid, "alpha", false, None, 50)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, "text.txt");
}

#[test]
fn search_code_rejects_tree_oid_as_commit() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "only.txt", "hit\n");
    git_support::commit_all(&repo, "init");
    let tree_oid = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap()
        .id();
    assert_eq!(
        repo.find_object(tree_oid, None).unwrap().kind(),
        Some(ObjectType::Tree)
    );
    // 传 tree oid 应当在解析提交一步报错，而不是 panic。
    assert!(
        service
            .search_code(&repo, &tree_oid.to_string(), "hit", false, None, 50)
            .is_err()
    );
}

#[test]
fn search_code_path_prefix_limits_scope() {
    let (_dir, repo, service) = setup_repo();
    let oid = head_oid(&repo);
    // 限定 src/：docs.md 的命中被剪掉。
    let matches = service
        .search_code(&repo, &oid, "alpha", false, Some("src"), 50)
        .unwrap();
    assert!(matches.iter().all(|m| m.path.starts_with("src/")));
    assert_eq!(matches.len(), 3);

    // 前缀带斜杠/空白也能归一化。
    let matches = service
        .search_code(&repo, &oid, "alpha", false, Some(" /src/ "), 50)
        .unwrap();
    assert_eq!(matches.len(), 3);

    // 不存在的前缀：显式报错（新行为）——静默空结果会让模型误以为
    // 标识符不存在；坏前缀用例见 search_code_reports_bad_path_prefix_instead_of_empty。
    let err = service
        .search_code(&repo, &oid, "alpha", false, Some("nope"), 50)
        .unwrap_err();
    assert!(err.to_string().contains("目录不存在"), "got: {err}");
}

#[test]
fn search_code_truncates_long_hit_lines() {
    let (dir, repo, service) = git_support::init_repo();
    // 无 NUL 的超长单行（模拟压缩/生成产物），整行入库会有内存峰值。
    let long_line = format!("fn alpha() {{ {} }}", "x".repeat(20_000));
    git_support::write_file(dir.path(), "generated.rs", &long_line);
    git_support::commit_all(&repo, "init");
    let oid = head_oid(&repo);

    let matches = service
        .search_code(&repo, &oid, "alpha", false, None, 50)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert!(
        matches[0].line.chars().count() <= 201,
        "命中行应截断到约 200 字符"
    );
    assert!(matches[0].line.ends_with('…'));
}

#[test]
fn search_code_reports_bad_path_prefix_instead_of_empty() {
    let (_temp, repo, service) = setup_repo();
    let oid = head_oid(&repo);

    // 前缀不存在：显式报错而不是静默空结果（后者会让模型误以为标识符不存在）。
    let err = service
        .search_code(&repo, &oid, "alpha", false, Some("nope/"), 50)
        .unwrap_err();
    assert!(err.to_string().contains("目录不存在"), "got: {err}");

    // 前缀指向文件：提示去掉文件名只保留目录。
    let err = service
        .search_code(&repo, &oid, "alpha", false, Some("src/lib.rs"), 50)
        .unwrap_err();
    assert!(err.to_string().contains("文件不是目录"), "got: {err}");
}
