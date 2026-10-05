use std::fs;

use tempfile::TempDir;

use super::*;
use crate::git::test_support::git_test_support as git_support;

const FIRST: &str = "{ version: 1, name: '初版', steps: [{ op: 'ensureClean' }] }";
const SECOND: &str = "{ version: 1, name: '新版', steps: [{ op: 'ensureClean' }] }";

#[test]
fn listing_does_not_install_and_downloads_only_selected_template() {
    let (source_dir, source, service) = git_support::init_repo();
    git_support::write_file(source_dir.path(), "work-studio-files/first.json5", FIRST);
    git_support::write_file(source_dir.path(), "work-studio-files/second.json5", SECOND);
    git_support::commit_all(&source, "templates");
    let target = TempDir::new().unwrap();
    let url = git_support::path_url(source_dir.path());

    let list = list_templates_from_url(&service, target.path(), &url).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].display_name, "初版");
    assert!(!target.path().join("first.json5").exists());
    assert!(!target.path().join("second.json5").exists());

    let installed = download_cnb_template(target.path(), &list[0]).unwrap();
    assert_eq!(fs::read_to_string(installed).unwrap(), FIRST);
    assert!(!target.path().join("second.json5").exists());
}

#[test]
fn download_keeps_existing_local_template() {
    let (source_dir, source, service) = git_support::init_repo();
    git_support::write_file(source_dir.path(), "work-studio-files/demo.json5", SECOND);
    git_support::commit_all(&source, "remote template");
    let target = TempDir::new().unwrap();
    let url = git_support::path_url(source_dir.path());
    let list = list_templates_from_url(&service, target.path(), &url).unwrap();

    fs::write(target.path().join("demo.json5"), FIRST).unwrap();
    assert!(download_cnb_template(target.path(), &list[0]).is_err());
    assert_eq!(fs::read_to_string(target.path().join("demo.json5")).unwrap(), FIRST);
}

#[test]
fn invalid_remote_template_does_not_appear_in_catalog() {
    let (source_dir, source, service) = git_support::init_repo();
    git_support::write_file(source_dir.path(), "work-studio-files/demo.json5", "{broken");
    git_support::commit_all(&source, "invalid template");
    let target = TempDir::new().unwrap();
    let url = git_support::path_url(source_dir.path());

    assert!(list_templates_from_url(&service, target.path(), &url).is_err());
    assert!(!target.path().join("demo.json5").exists());
}
