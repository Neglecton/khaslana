use super::*;
use khaslana::ChangeState;

fn file(path: &str, status: ChangeState) -> BrowseCompareFile {
    BrowseCompareFile {
        path: path.to_string(),
        old_path: None,
        status,
    }
}

fn renamed(path: &str, old_path: &str) -> BrowseCompareFile {
    BrowseCompareFile {
        path: path.to_string(),
        old_path: Some(old_path.to_string()),
        status: ChangeState::Renamed,
    }
}

#[test]
fn flatten_compare_files_builds_nested_tree() {
    let files = vec![
        file("src/a.rs", ChangeState::Modified),
        file("src/b.rs", ChangeState::Added),
        file("README.md", ChangeState::Modified),
    ];
    let expanded = all_compare_dirs(&files);

    let rows = flatten_compare_files(&files, &expanded);

    // src 目录 + 两个文件 + 根级 README
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].depth, 0);
    assert!(matches!(
        rows[0].kind,
        CompareTreeRowKind::Directory { expanded: true }
    ));
    assert_eq!(rows[0].name, "src");
    assert_eq!(rows[1].depth, 1);
    assert_eq!(rows[1].name, "a.rs");
    assert_eq!(rows[2].depth, 1);
    assert_eq!(rows[2].name, "b.rs");
    assert_eq!(rows[3].depth, 0);
    assert_eq!(rows[3].name, "README.md");
}

#[test]
fn flatten_compare_files_collapses_directory() {
    let files = vec![
        file("src/a.rs", ChangeState::Modified),
        file("src/b.rs", ChangeState::Added),
        file("README.md", ChangeState::Modified),
    ];
    // 显式空展开集合以外的集合：不含 src，应当折叠。
    let mut expanded = HashSet::new();
    expanded.insert(".".to_string());
    let rows = flatten_compare_files(&files, &expanded);

    // src 折叠 -> 不输出其下文件
    assert_eq!(rows.len(), 2);
    assert!(matches!(
        rows[0].kind,
        CompareTreeRowKind::Directory { expanded: false }
    ));
    assert_eq!(rows[1].name, "README.md");
}

#[test]
fn flatten_compare_files_orders_directories_before_files() {
    let files = vec![
        file("z_file.rs", ChangeState::Modified),
        file("abc_dir/x.rs", ChangeState::Added),
    ];
    let expanded = all_compare_dirs(&files);
    let rows = flatten_compare_files(&files, &expanded);

    // 第一行应是目录 abc_dir，而不是 z_file.rs
    assert!(matches!(rows[0].kind, CompareTreeRowKind::Directory { .. }));
    assert_eq!(rows[0].name, "abc_dir");
    // 根级文件排在后面
    assert_eq!(rows[2].name, "z_file.rs");
}

#[test]
fn flatten_compare_files_keeps_status_and_old_path() {
    let files = vec![renamed("src/new.rs", "src/old.rs")];
    let expanded = all_compare_dirs(&files);
    let rows = flatten_compare_files(&files, &expanded);

    // rows: [src(dir), new.rs(file)]
    assert_eq!(rows.len(), 2);
    match &rows[1].kind {
        CompareTreeRowKind::File { status, old_path } => {
            assert_eq!(*status, ChangeState::Renamed);
            assert_eq!(old_path.as_deref(), Some("src/old.rs"));
        }
        other => panic!("expected file row, got {other:?}"),
    }
}

#[test]
fn all_compare_dirs_collects_intermediate_dirs() {
    let files = vec![
        file("a/b/c.rs", ChangeState::Modified),
        file("x/y.rs", ChangeState::Added),
    ];
    let dirs = all_compare_dirs(&files);

    assert!(dirs.contains("a"));
    assert!(dirs.contains("a/b"));
    assert!(dirs.contains("x"));
    // 文件本身不应作为目录
    assert!(!dirs.contains("a/b/c.rs"));
    assert!(!dirs.contains("x/y.rs"));
}

#[test]
fn compare_file_leaf_display_plain() {
    assert_eq!(compare_file_leaf_display("main.rs", None), "main.rs");
}

#[test]
fn compare_file_leaf_display_rename() {
    assert_eq!(
        compare_file_leaf_display("new.rs", Some("src/old.rs")),
        "old.rs → new.rs"
    );
}

#[test]
fn compare_file_leaf_display_rename_same_basename() {
    // 同名不同目录：显示新文件名即可，避免重复
    assert_eq!(
        compare_file_leaf_display("main.rs", Some("sub/main.rs")),
        "main.rs"
    );
}

// 深前缀共享的差异文件：展平后可见行数（目录节点 + 文件叶子）必须大于文件数，
// 且每个文件叶子都应出现在展平行中。这固化了“uniform_list 行数不能用文件数”的不变量，
// 否则深层目录与文件叶子会因超出虚拟化列表行数而不渲染（文件树显示不全）。
#[test]
fn compare_visible_row_count_exceeds_file_count_for_nested_paths() {
    let files = vec![
        file("src/main/java/com/a/X.java", ChangeState::Modified),
        file("src/main/java/com/b/Y.java", ChangeState::Added),
        file("src/main/java/com/c/Z.java", ChangeState::Modified),
        file("src/main/java/com/d/W.java", ChangeState::Modified),
    ];
    let expanded = all_compare_dirs(&files);

    let rows = flatten_compare_files(&files, &expanded);
    let row_count = compare_visible_row_count(&files, &expanded);

    assert_eq!(row_count, rows.len());
    assert!(row_count > files.len());

    for file in &files {
        let present = rows.iter().any(|row| {
            row.path == file.path
                && matches!(row.kind, CompareTreeRowKind::File { .. })
        });
        assert!(present, "文件叶子未出现在展平行中: {}", file.path);
    }
}

#[test]
fn compare_visible_row_count_empty_is_one() {
    let expanded = HashSet::new();
    // 空列表返回 1，供占位行渲染。
    assert_eq!(compare_visible_row_count(&[], &expanded), 1);
}
