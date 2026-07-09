use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

fn touch_executable(path: &Path) {
    fs::write(path, "").unwrap();
}

fn candidate_path(dir: &Path, command: &str) -> PathBuf {
    dir.join(candidate_command_names(command).remove(0))
}

#[test]
fn idea_env_path_wins_over_path_commands() {
    let temp = TempDir::new().unwrap();
    let env_tool = temp.path().join("custom-idea");
    let path_tool = candidate_path(temp.path(), "idea64");
    touch_executable(&env_tool);
    touch_executable(&path_tool);

    let resolved =
        resolve_intellij_idea_command_from_env_and_path(Some(&env_tool), &[temp.path().into()])
            .unwrap();

    assert_eq!(resolved, env_tool);
}

#[test]
fn idea64_wins_over_idea_from_path() {
    let temp = TempDir::new().unwrap();
    let idea64 = candidate_path(temp.path(), "idea64");
    let idea = candidate_path(temp.path(), "idea");
    touch_executable(&idea64);
    touch_executable(&idea);

    let resolved =
        resolve_intellij_idea_command_from_env_and_path(None, &[temp.path().into()]).unwrap();

    assert_eq!(resolved, idea64);
}

#[test]
fn intellij_merge_args_keep_official_order() {
    let args = intellij_idea_merge_args(
        Path::new("ours.txt"),
        Path::new("theirs.txt"),
        Path::new("base.txt"),
        Path::new("result.txt"),
    );

    assert_eq!(
        args,
        vec![
            OsString::from("merge"),
            OsString::from("ours.txt"),
            OsString::from("theirs.txt"),
            OsString::from("base.txt"),
            OsString::from("result.txt"),
        ]
    );
}

#[test]
fn invalid_worktree_relative_path_is_rejected_before_merge() {
    let (_dir, repo, _service) = crate::git::test_support::git_test_support::init_repo();
    let error = run_intellij_idea_merge_with_command(
        &repo,
        Path::new("../outside.txt"),
        &PathBuf::from("idea64.exe"),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("文件路径无效"));
}
