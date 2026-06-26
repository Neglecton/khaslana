use super::*;
use crate::git::test_support::git_test_support as git_support;
use crate::types::{BranchName, ChangeState, CommitMessage, DiffLineKind};
use git2::Oid;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn stage_and_commit(repo: &mut git2::Repository, svc: &GitService, message: &str) -> Oid {
    svc.stage_path(repo, Path::new(".")).unwrap();
    svc.commit(repo, &CommitMessage::new(message)).unwrap();
    repo.head().unwrap().target().unwrap()
}

// 构建测试仓库：main 分支有 src/lib.rs + README.md，feature 分支修改 lib.rs 并新增 new.rs。
fn build_repo() -> (TempDir, GitService) {
    let (dir, mut repo, svc) = git_support::init_repo();
    git_support::write_file(dir.path(), "src/types/mod.rs", "// types\n");
    git_support::write_file(dir.path(), "src/lib.rs", "pub fn a() -> i32 { 1 }\n");
    git_support::write_file(dir.path(), "README.md", "# main\n");
    stage_and_commit(&mut repo, &svc, "init");

    svc.create_branch_from(
        &mut repo,
        &BranchName::new("feature"),
        Some(&BranchName::new("main")),
        false,
    )
    .unwrap();
    svc.checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "src/lib.rs", "pub fn a() -> i32 { 2 }\n");
    git_support::write_file(dir.path(), "src/new.rs", "pub fn b() -> i32 { 3 }\n");
    stage_and_commit(&mut repo, &svc, "feature change");
    svc.checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    drop(repo);
    (dir, svc)
}

#[test]
fn resolve_browse_target_local_branch() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();

    let main = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();
    assert_eq!(main.display_name, "main");
    assert!(!main.commit_oid.is_empty());

    let feature = svc
        .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
        .unwrap();
    assert_ne!(main.commit_oid, feature.commit_oid);
}

#[test]
fn resolve_browse_target_tag() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let head_oid = repo.head().unwrap().target().unwrap();
    repo.reference("refs/tags/v1.0", head_oid, false, "test tag")
        .unwrap();

    let target = svc
        .resolve_browse_target(&repo, "v1.0", BrowseRefKind::Tag)
        .unwrap();
    assert_eq!(target.display_name, "v1.0");
    assert_eq!(target.commit_oid, head_oid.to_string());
}

#[test]
fn resolve_browse_target_missing_returns_error() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert!(
        svc.resolve_browse_target(&repo, "nonexistent", BrowseRefKind::LocalBranch)
            .is_err()
    );
    assert!(
        svc.resolve_browse_target(&repo, "ghost", BrowseRefKind::Tag)
            .is_err()
    );
}

#[test]
fn browse_tree_entries_root_directories_first() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();

    let entries = svc
        .browse_tree_entries(&repo, &target.commit_oid, None)
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "src");
    assert_eq!(entries[0].kind, BrowseEntryKind::Directory);
    assert_eq!(entries[1].name, "README.md");
    assert_eq!(entries[1].kind, BrowseEntryKind::File);
}

#[test]
fn browse_tree_entries_subdirectory() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();

    let entries = svc
        .browse_tree_entries(&repo, &target.commit_oid, Some(Path::new("src")))
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "types");
    assert_eq!(entries[0].kind, BrowseEntryKind::Directory);
    assert_eq!(entries[0].path, "src/types");
    assert_eq!(entries[1].name, "lib.rs");
    assert_eq!(entries[1].path, "src/lib.rs");
}

#[test]
fn browse_file_content_text_decodes() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();

    let content = svc
        .browse_file_content(
            &repo,
            &target.commit_oid,
            Path::new("src/lib.rs"),
            DiffEncodingChoice::Utf8,
        )
        .unwrap();
    assert!(!content.is_binary);
    assert_eq!(content.lines, vec!["pub fn a() -> i32 { 1 }"]);
}

#[test]
fn browse_file_content_binary_detected() {
    let (dir, svc) = build_repo();
    let repo_path = dir.path();
    let mut repo = git2::Repository::open(repo_path).unwrap();
    fs::write(repo_path.join("blob.bin"), [0u8, 1, 2, 0, 255, 0]).unwrap();
    svc.stage_path(&mut repo, Path::new("blob.bin")).unwrap();
    svc.commit(&mut repo, &CommitMessage::new("add binary"))
        .unwrap();
    drop(repo);

    let repo = git2::Repository::open(repo_path).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();
    let content = svc
        .browse_file_content(
            &repo,
            &target.commit_oid,
            Path::new("blob.bin"),
            DiffEncodingChoice::Utf8,
        )
        .unwrap();
    assert!(content.is_binary);
    assert!(content.lines.is_empty());
}

#[test]
fn browse_file_diff_shows_changes() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();

    let feature = svc
        .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
        .unwrap();
    let diff = svc
        .browse_file_diff(
            &repo,
            &feature.commit_oid,
            Path::new("src/lib.rs"),
            false,
            DiffEncodingChoice::Utf8,
        )
        .unwrap();
    assert!(
        diff.lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Removed && l.content.contains("{ 1 }"))
    );
    assert!(
        diff.lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Added && l.content.contains("{ 2 }"))
    );
}

#[test]
fn browse_file_diff_same_branch_empty() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();
    let diff = svc
        .browse_file_diff(
            &repo,
            &target.commit_oid,
            Path::new("README.md"),
            false,
            DiffEncodingChoice::Utf8,
        )
        .unwrap();
    assert!(diff.lines.is_empty());
}

#[test]
fn browse_tree_entries_missing_path_errors() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();
    assert!(
        svc.browse_tree_entries(&repo, &target.commit_oid, Some(Path::new("nonexistent")))
            .is_err()
    );
}

#[test]
fn browse_compare_files_only_changed_paths() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let feature = svc
        .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
        .unwrap();

    let files = svc
        .browse_compare_files(&repo, &feature.commit_oid)
        .unwrap();
    let paths = files
        .iter()
        .map(|file| (file.path.as_str(), file.status.clone()))
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            ("src/lib.rs", ChangeState::Modified),
            ("src/new.rs", ChangeState::Added),
        ]
    );
    assert!(!files.iter().any(|file| file.path == "README.md"));
}

#[test]
fn browse_compare_files_empty_for_same_branch() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let main = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();

    let files = svc.browse_compare_files(&repo, &main.commit_oid).unwrap();

    assert!(files.is_empty());
}

#[test]
fn browse_compare_files_reports_deleted_files() {
    let (dir, svc) = build_repo();
    let repo_path = dir.path();
    let mut repo = git2::Repository::open(repo_path).unwrap();
    svc.checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    fs::remove_file(repo_path.join("README.md")).unwrap();
    svc.stage_path(&mut repo, Path::new("README.md")).unwrap();
    svc.commit(&mut repo, &CommitMessage::new("delete readme"))
        .unwrap();
    svc.checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    drop(repo);

    let repo = git2::Repository::open(repo_path).unwrap();
    let feature = svc
        .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
        .unwrap();
    let files = svc
        .browse_compare_files(&repo, &feature.commit_oid)
        .unwrap();

    let deleted = files
        .iter()
        .find(|file| file.path == "README.md")
        .expect("deleted file should be listed");
    assert_eq!(deleted.status, ChangeState::Deleted);
}

#[test]
fn browse_compare_file_diff_keeps_head_to_target_direction() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let feature = svc
        .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
        .unwrap();

    let diff = svc
        .browse_file_diff_for_compare(
            &repo,
            &feature.commit_oid,
            Path::new("src/lib.rs"),
            None,
            false,
            DiffEncodingChoice::Utf8,
        )
        .unwrap();

    assert!(
        diff.lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Removed && line.content.contains("{ 1 }"))
    );
    assert!(
        diff.lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.content.contains("{ 2 }"))
    );
}

#[test]
fn browse_file_content_missing_path_errors() {
    let (dir, svc) = build_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let target = svc
        .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
        .unwrap();
    assert!(
        svc.browse_file_content(
            &repo,
            &target.commit_oid,
            Path::new("nope.rs"),
            DiffEncodingChoice::Utf8,
        )
        .is_err()
    );
}
