use super::*;

fn entry(path: &str, kind: BrowseEntryKind) -> BrowseEntry {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    BrowseEntry {
        path: path.to_string(),
        name,
        kind,
        size: 0,
    }
}

fn make_entries(dir: &str, items: &[(&str, BrowseEntryKind)]) -> (PathBuf, Vec<BrowseEntry>) {
    let key = if dir.is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(dir)
    };
    let entries = items
        .iter()
        .map(|(name, kind)| {
            let path = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            };
            entry(&path, *kind)
        })
        .collect();
    (key, entries)
}

#[test]
fn flatten_browse_tree_no_expansion() {
    let mut entries_by_dir = HashMap::new();
    let (root_key, root_entries) = make_entries(
        "",
        &[
            ("src", BrowseEntryKind::Directory),
            ("README.md", BrowseEntryKind::File),
        ],
    );
    entries_by_dir.insert(root_key, root_entries);

    let rows = flatten_browse_tree(&entries_by_dir, &HashSet::new());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].entry.name, "src");
    assert_eq!(rows[0].depth, 0);
    assert_eq!(rows[1].entry.name, "README.md");
    assert_eq!(rows[1].depth, 0);
}

#[test]
fn flatten_browse_tree_with_expansion() {
    let mut entries_by_dir = HashMap::new();
    let (root_key, root_entries) = make_entries(
        "",
        &[
            ("src", BrowseEntryKind::Directory),
            ("README.md", BrowseEntryKind::File),
        ],
    );
    entries_by_dir.insert(root_key, root_entries);

    let (src_key, src_entries) = make_entries(
        "src",
        &[
            ("lib.rs", BrowseEntryKind::File),
            ("types", BrowseEntryKind::Directory),
        ],
    );
    entries_by_dir.insert(src_key, src_entries);

    let mut expanded = HashSet::new();
    expanded.insert(PathBuf::from("src"));

    let rows = flatten_browse_tree(&entries_by_dir, &expanded);
    // src, src/lib.rs, src/types, README.md
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].entry.name, "src");
    assert_eq!(rows[0].depth, 0);
    assert_eq!(rows[1].entry.name, "lib.rs");
    assert_eq!(rows[1].depth, 1);
    assert_eq!(rows[2].entry.name, "types");
    assert_eq!(rows[2].depth, 1);
    assert_eq!(rows[3].entry.name, "README.md");
    assert_eq!(rows[3].depth, 0);
}

#[test]
fn flatten_browse_tree_nested_expansion() {
    let mut entries_by_dir = HashMap::new();
    let (root_key, root_entries) = make_entries("", &[("a", BrowseEntryKind::Directory)]);
    entries_by_dir.insert(root_key, root_entries);

    let (a_key, a_entries) = make_entries(
        "a",
        &[
            ("b", BrowseEntryKind::Directory),
            ("f.txt", BrowseEntryKind::File),
        ],
    );
    entries_by_dir.insert(a_key, a_entries);

    let (ab_key, ab_entries) = make_entries("a/b", &[("c.txt", BrowseEntryKind::File)]);
    entries_by_dir.insert(ab_key, ab_entries);

    let mut expanded = HashSet::new();
    expanded.insert(PathBuf::from("a"));
    expanded.insert(PathBuf::from("a/b"));

    let rows = flatten_browse_tree(&entries_by_dir, &expanded);
    // a, a/b, a/b/c.txt, a/f.txt
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].depth, 0); // a
    assert_eq!(rows[1].depth, 1); // a/b
    assert_eq!(rows[2].depth, 2); // a/b/c.txt
    assert_eq!(rows[3].depth, 1); // a/f.txt
}

#[test]
fn browse_tree_render_snapshot_keeps_row_count_and_indices_stable() {
    let mut entries_by_dir = HashMap::new();
    let (root_key, root_entries) = make_entries(
        "",
        &[
            ("src", BrowseEntryKind::Directory),
            ("README.md", BrowseEntryKind::File),
        ],
    );
    entries_by_dir.insert(root_key, root_entries);
    let (src_key, src_entries) = make_entries("src", &[("lib.rs", BrowseEntryKind::File)]);
    entries_by_dir.insert(src_key, src_entries);

    let mut expanded = HashSet::new();
    expanded.insert(PathBuf::from("src"));
    let rows_snapshot = std::sync::Arc::new(flatten_browse_tree(&entries_by_dir, &expanded));
    let row_count = rows_snapshot.len().max(1);

    entries_by_dir.get_mut(&PathBuf::new()).unwrap().clear();
    expanded.clear();

    assert_eq!(row_count, rows_snapshot.len());
    assert_eq!(rows_snapshot[0].entry.path, "src");
    assert_eq!(rows_snapshot[1].entry.path, "src/lib.rs");
    assert_eq!(rows_snapshot[2].entry.path, "README.md");
}

#[test]
fn flatten_browse_tree_collapsed_not_loaded() {
    let mut entries_by_dir = HashMap::new();
    let (root_key, root_entries) = make_entries("", &[("src", BrowseEntryKind::Directory)]);
    entries_by_dir.insert(root_key, root_entries);
    // src 子树未加载

    let rows = flatten_browse_tree(&entries_by_dir, &HashSet::new());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry.name, "src");
}

#[test]
fn browse_canvas_mode_label_keeps_compare_content_distinct_from_diff() {
    assert_eq!(
        browse_canvas_mode_label(false, BrowseViewMode::Content),
        "文件内容"
    );
    assert_eq!(
        browse_canvas_mode_label(true, BrowseViewMode::Content),
        "目标分支全文"
    );
    assert_eq!(
        browse_canvas_mode_label(false, BrowseViewMode::Diff),
        "与当前分支差异"
    );
    assert_eq!(
        browse_canvas_mode_label(true, BrowseViewMode::Diff),
        "与当前分支差异"
    );
}

#[test]
fn browse_tree_indent_uses_focus_workbench_spacing_scale() {
    assert_eq!(browse_tree_indent(0), 0.0);
    assert_eq!(browse_tree_indent(1), ui_theme::SPACE_3);
    assert_eq!(browse_tree_indent(3), ui_theme::SPACE_3 * 3.0);
}
