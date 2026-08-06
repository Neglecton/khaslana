use std::fs;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use git2::{BranchType, Oid, RepositoryInitOptions, RepositoryState};
use tempfile::TempDir;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    },
};

use super::*;
use crate::git::test_support::git_test_support as git_support;
use crate::types::SubmoduleRemoteSyncStatus;

#[cfg(windows)]
fn lock_directory_without_delete_share(path: &Path) -> HANDLE {
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // 不共享删除权限的目录句柄，稳定模拟 VS Code、终端或语言服务对该目录的占用。
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    handle
}

fn set_gitmodules_branch(root: &Path, submodule_path: &str, branch: &str) {
    let gitmodules_path = root.join(".gitmodules");
    let mut content = fs::read_to_string(&gitmodules_path).unwrap();
    let section = format!("[submodule \"{submodule_path}\"]");
    let start = content.find(&section).unwrap();
    let next_section = content[start + section.len()..]
        .find("\n[submodule ")
        .map(|offset| start + section.len() + offset)
        .unwrap_or(content.len());
    if content[start..next_section].contains("\n\tbranch = ") {
        let section_body = content[start..next_section].to_string();
        let replaced = section_body
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("branch = ") {
                    format!("\tbranch = {branch}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        content.replace_range(start..next_section, &replaced);
    } else {
        let insert_at = next_section;
        let prefix = if insert_at > 0 && content.as_bytes()[insert_at - 1] == b'\n' {
            ""
        } else {
            "\n"
        };
        content.insert_str(insert_at, &format!("{prefix}\tbranch = {branch}\n"));
    }
    fs::write(gitmodules_path, content).unwrap();
}

fn set_gitmodules_url(root: &Path, submodule_path: &str, url: &str) {
    let gitmodules_path = root.join(".gitmodules");
    let mut content = fs::read_to_string(&gitmodules_path).unwrap();
    let section = format!("[submodule \"{submodule_path}\"]");
    let start = content.find(&section).unwrap();
    let next_section = content[start + section.len()..]
        .find("\n[submodule ")
        .map(|offset| start + section.len() + offset)
        .unwrap_or(content.len());
    let section_body = content[start..next_section].to_string();
    let replaced = section_body
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("url = ") {
                format!("\turl = {url}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    content.replace_range(start..next_section, &replaced);
    fs::write(gitmodules_path, content).unwrap();
}

fn clone_repo_with_remote_feature() -> (TempDir, TempDir, std::path::PathBuf, Repository, GitService)
{
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), "README.md", "seed\n");
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();
    service
        .create_branch(&mut seed_repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut seed_repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(seed_dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&seed_repo, "feature");
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();

    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &RepoPath::new(&clone_path),
        )
        .unwrap();
    let clone_repo = Repository::open(&clone_path).unwrap();
    git_support::configure_user(&clone_repo);
    (remote_dir, clone_dir, clone_path, clone_repo, service)
}

fn create_bare_remote_with_seed(file_name: &str, body: &str) -> (TempDir, GitService) {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), file_name, body);
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();
    (remote_dir, service)
}

fn create_super_remote_with_submodule() -> (TempDir, TempDir, GitService) {
    let (sub_remote, service) = create_bare_remote_with_seed("sub.txt", "sub v1\n");
    let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");

    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("super-work");
    service
        .clone_repo(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    {
        let mut submodule = repo
            .submodule(
                &git_support::path_url(sub_remote.path()),
                Path::new("deps/sub"),
                true,
            )
            .unwrap();
        submodule.clone(None).unwrap();
        submodule.add_finalize().unwrap();
    }
    git_support::commit_all(&repo, "add submodule");
    service.push(&mut repo, &RemoteName::new("origin")).unwrap();

    (super_remote, sub_remote, service)
}

struct SuperRemoteWithSubmodules {
    super_remote: TempDir,
    sub_remotes: Vec<TempDir>,
    service: GitService,
}

fn create_super_remote_with_named_submodules(
    modules: &[(&str, &str, Option<&str>)],
) -> SuperRemoteWithSubmodules {
    let service = git_support::service();
    let mut sub_remotes = Vec::new();
    let mut sub_urls = Vec::new();
    for (_name, initial_body, _branch) in modules {
        let remote_dir = TempDir::new().unwrap();
        let mut bare_opts = RepositoryInitOptions::new();
        bare_opts.bare(true).initial_head("main");
        Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

        let (work_dir, mut repo, _seed_service) = git_support::init_repo();
        git_support::write_file(work_dir.path(), "sub.txt", initial_body);
        git_support::commit_all(&repo, "seed submodule");
        repo.remote("origin", &git_support::path_url(remote_dir.path()))
            .unwrap();
        service.push(&mut repo, &RemoteName::new("origin")).unwrap();
        sub_urls.push(git_support::path_url(remote_dir.path()));
        sub_remotes.push(remote_dir);
    }

    let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("super-work");
    service
        .clone_repo(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    for ((name, _initial_body, branch), url) in modules.iter().zip(sub_urls.iter()) {
        let path = format!("deps/{name}");
        {
            let mut submodule = repo.submodule(url, Path::new(&path), true).unwrap();
            submodule.clone(None).unwrap();
            submodule.add_finalize().unwrap();
        }
        if let Some(branch) = branch {
            set_gitmodules_branch(&work_path, &path, branch);
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(".gitmodules")).unwrap();
            index.write().unwrap();
        }
    }
    git_support::commit_all(&repo, "add submodules");
    service.push(&mut repo, &RemoteName::new("origin")).unwrap();

    SuperRemoteWithSubmodules {
        super_remote,
        sub_remotes,
        service,
    }
}

fn create_super_remote_with_two_submodules() -> SuperRemoteWithSubmodules {
    create_super_remote_with_named_submodules(&[
        ("one", "one v1\n", None),
        ("two", "two v1\n", None),
    ])
}

fn clone_super_repo(
    fixture: &SuperRemoteWithSubmodules,
    recursive_submodules: bool,
) -> (TempDir, std::path::PathBuf, Repository) {
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    fixture
        .service
        .clone_repo_with_options(
            &git_support::path_url(fixture.super_remote.path()),
            &RepoPath::new(&clone_path),
            CloneOptions {
                recursive_submodules,
            },
        )
        .unwrap();
    let repo = Repository::open(&clone_path).unwrap();
    git_support::configure_user(&repo);
    (clone_dir, clone_path, repo)
}

fn advance_submodule_remote(
    remote_dir: &Path,
    service: &GitService,
    body: &str,
    branch: &str,
) -> Oid {
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("sub-remote-work");
    service
        .clone_repo(
            &git_support::path_url(remote_dir),
            &RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    if branch != "main" {
        service
            .create_branch(&mut repo, &BranchName::new(branch))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new(branch))
            .unwrap();
    }
    git_support::write_file(&work_path, "sub.txt", body);
    let oid = git_support::commit_all(&repo, "advance submodule");
    service
        .push_branch(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new(branch),
            false,
        )
        .unwrap();
    oid
}

fn submodule_head_oid(root: &Path, path: &str) -> Oid {
    let repo = Repository::open(root.join(path)).unwrap();
    repo.head().unwrap().target().unwrap()
}

fn single_submodule_remote_status(
    service: &GitService,
    repo: &Repository,
) -> SubmoduleRemoteSyncStatus {
    let statuses = service.submodule_remote_sync_statuses(repo).unwrap();
    assert_eq!(statuses.len(), 1);
    statuses[0].1.clone()
}

struct NestedSubmoduleFixture {
    super_remote: TempDir,
    _middle_remote: TempDir,
    leaf_remote: TempDir,
    service: GitService,
}

fn create_nested_submodule_fixture() -> NestedSubmoduleFixture {
    let service = git_support::service();
    let (leaf_remote, _leaf_service) = create_bare_remote_with_seed("leaf.txt", "leaf v1\n");
    let (middle_remote, _middle_service) = create_bare_remote_with_seed("middle.txt", "middle\n");

    let middle_work_dir = TempDir::new().unwrap();
    let middle_work_path = middle_work_dir.path().join("middle-work");
    service
        .clone_repo(
            &git_support::path_url(middle_remote.path()),
            &RepoPath::new(&middle_work_path),
        )
        .unwrap();
    let mut middle_repo = Repository::open(&middle_work_path).unwrap();
    git_support::configure_user(&middle_repo);
    {
        let leaf_url = git_support::path_url(leaf_remote.path());
        let mut submodule = middle_repo
            .submodule(&leaf_url, Path::new("deps/leaf"), true)
            .unwrap();
        submodule.clone(None).unwrap();
        submodule.add_finalize().unwrap();
        set_gitmodules_url(&middle_work_path, "deps/leaf", &leaf_url);
        let mut index = middle_repo.index().unwrap();
        index.add_path(Path::new(".gitmodules")).unwrap();
        index.write().unwrap();
    }
    git_support::commit_all(&middle_repo, "add nested leaf");
    service
        .push(&mut middle_repo, &RemoteName::new("origin"))
        .unwrap();

    let (super_remote, _super_service) = create_bare_remote_with_seed("README.md", "super\n");
    let super_work_dir = TempDir::new().unwrap();
    let super_work_path = super_work_dir.path().join("super-work");
    service
        .clone_repo(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&super_work_path),
        )
        .unwrap();
    let mut super_repo = Repository::open(&super_work_path).unwrap();
    git_support::configure_user(&super_repo);
    {
        let middle_url = git_support::path_url(middle_remote.path());
        let mut submodule = super_repo
            .submodule(&middle_url, Path::new("deps/mid"), true)
            .unwrap();
        submodule.clone(None).unwrap();
        submodule.add_finalize().unwrap();
        set_gitmodules_url(&super_work_path, "deps/mid", &middle_url);
        let mut index = super_repo.index().unwrap();
        index.add_path(Path::new(".gitmodules")).unwrap();
        index.write().unwrap();
    }
    git_support::commit_all(&super_repo, "add middle submodule");
    service
        .push(&mut super_repo, &RemoteName::new("origin"))
        .unwrap();

    NestedSubmoduleFixture {
        super_remote,
        _middle_remote: middle_remote,
        leaf_remote,
        service,
    }
}

fn advance_remote_feature(remote_dir: &Path, service: &GitService) {
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("remote-work");
    service
        .clone_repo(
            &git_support::path_url(remote_dir),
            &RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();
    service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();
    git_support::write_file(&work_path, "feature.txt", "feature\nremote update\n");
    git_support::commit_all(&repo, "remote feature update");
    service.push(&mut repo, &RemoteName::new("origin")).unwrap();
}

fn advance_remote_main(remote_dir: &Path, service: &GitService, path: &str, body: &str) -> Oid {
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().join("remote-work");
    service
        .clone_repo(
            &git_support::path_url(remote_dir),
            &RepoPath::new(&work_path),
        )
        .unwrap();
    let mut repo = Repository::open(&work_path).unwrap();
    git_support::configure_user(&repo);
    git_support::write_file(&work_path, path, body);
    let oid = git_support::commit_all(&repo, "remote main update");
    service.push(&mut repo, &RemoteName::new("origin")).unwrap();
    oid
}

#[test]
fn branch_create_rename_delete() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "hello");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    assert!(repo.find_branch("feature", BranchType::Local).is_ok());

    service
        .rename_branch(
            &mut repo,
            &BranchName::new("feature"),
            &BranchName::new("topic"),
        )
        .unwrap();
    assert!(repo.find_branch("feature", BranchType::Local).is_err());
    assert!(repo.find_branch("topic", BranchType::Local).is_ok());

    service
        .delete_branch(&mut repo, &BranchName::new("topic"))
        .unwrap();
    assert!(repo.find_branch("topic", BranchType::Local).is_err());
}

#[cfg(windows)]
#[test]
fn checkout_branch_keeps_vscode_locked_directory_and_switches() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "main\n");
    git_support::commit_all(&repo, "main");
    service
        .create_branch(&mut repo, &BranchName::new("with-directory"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("with-directory"))
        .unwrap();
    git_support::write_file(dir.path(), "opened-in-vscode/file.txt", "branch only\n");
    git_support::commit_all(&repo, "add directory");

    let opened_directory = dir.path().join("opened-in-vscode");
    let directory_handle = lock_directory_without_delete_share(&opened_directory);

    let checkout_result = service.checkout_branch(&mut repo, &BranchName::new("main"));
    unsafe {
        CloseHandle(directory_handle);
    }

    let snapshot = checkout_result.unwrap();
    assert_eq!(snapshot.head.as_deref(), Some("main"));
    assert!(!opened_directory.join("file.txt").exists());
    assert!(opened_directory.exists());
    assert!(opened_directory.read_dir().unwrap().next().is_none());
}

#[test]
fn clone_repo_with_recursive_submodules_checks_out_submodule() {
    let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");

    service
        .clone_repo_with_options(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&clone_path),
            CloneOptions {
                recursive_submodules: true,
            },
        )
        .unwrap();

    git_support::assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v1\n");
    let repo = Repository::open(&clone_path).unwrap();
    let submodules = service.submodules(&repo).unwrap();
    assert_eq!(submodules.len(), 1);
    assert!(submodules[0].status.is_ready());
}

#[test]
fn update_submodules_initializes_non_recursive_clone() {
    let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");

    let mut repo = Repository::open(
        service
            .clone_repo_with_options(
                &git_support::path_url(super_remote.path()),
                &RepoPath::new(&clone_path),
                CloneOptions {
                    recursive_submodules: false,
                },
            )
            .unwrap()
            .path,
    )
    .unwrap();
    assert!(!clone_path.join("deps/sub/sub.txt").exists());

    service.update_submodules(&mut repo).unwrap();

    git_support::assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v1\n");
    let submodules = service.submodules(&repo).unwrap();
    assert_eq!(submodules.len(), 1);
    assert!(submodules[0].status.is_ready());
}

#[test]
fn update_submodules_rejects_dirty_submodule_worktree() {
    let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    service
        .clone_repo_with_options(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&clone_path),
            CloneOptions {
                recursive_submodules: true,
            },
        )
        .unwrap();
    let mut repo = Repository::open(&clone_path).unwrap();
    git_support::write_file(&clone_path, "deps/sub/local.txt", "local\n");

    let err = service.update_submodules(&mut repo).unwrap_err();

    assert!(err.to_string().contains("子模块"));
    assert!(err.to_string().contains("本地改动"));
}

#[test]
fn open_fast_skips_submodule_scan_for_lazy_dialog_loading() {
    let (super_remote, _sub_remote, service) = create_super_remote_with_submodule();
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    service
        .clone_repo_with_options(
            &git_support::path_url(super_remote.path()),
            &RepoPath::new(&clone_path),
            CloneOptions {
                recursive_submodules: false,
            },
        )
        .unwrap();

    let snapshot = service.open_fast(&RepoPath::new(&clone_path)).unwrap();

    assert!(snapshot.changes.is_empty());
    let repo = Repository::open(&clone_path).unwrap();
    let submodules = service.submodules(&repo).unwrap();
    assert_eq!(submodules.len(), 1);
    assert_eq!(submodules[0].path, Path::new("deps/sub"));
}

#[test]
fn update_submodules_to_remote_latest_advances_workdir_without_parent_commit() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    let target = advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "sub v2\n",
        "main",
    );

    fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap();

    let target_string = target.to_string();
    assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), target);
    git_support::assert_file_text(&clone_path, "deps/sub/sub.txt", "sub v2\n");
    let submodules = fixture.service.submodules(&repo).unwrap();
    assert_eq!(submodules.len(), 1);
    assert_eq!(
        submodules[0].workdir_id.as_deref(),
        Some(target_string.as_str())
    );
    assert_ne!(submodules[0].index_id, submodules[0].workdir_id);
    assert_eq!(submodules[0].status.label(), "需更新");
    let changes = fixture.service.status(&repo).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| change.path == "deps/sub"
                && change.unstaged == Some(ChangeState::Modified))
    );
}

#[test]
fn update_submodules_to_remote_latest_uses_gitmodules_branch() {
    let fixture =
        create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("develop"))]);
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    let _main_target = advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "main v2\n",
        "main",
    );
    let develop_target = advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "develop v2\n",
        "develop",
    );

    fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap();

    assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), develop_target);
    git_support::assert_file_text(&clone_path, "deps/sub/sub.txt", "develop v2\n");
}

#[test]
fn update_submodules_to_remote_latest_branch_dot_uses_parent_branch() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    fixture
        .service
        .create_branch(&mut repo, &BranchName::new("release"))
        .unwrap();
    fixture
        .service
        .checkout_branch(&mut repo, &BranchName::new("release"))
        .unwrap();
    let target = advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "release v2\n",
        "release",
    );

    fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap();

    assert_eq!(submodule_head_oid(&clone_path, "deps/sub"), target);
    git_support::assert_file_text(&clone_path, "deps/sub/sub.txt", "release v2\n");
}

#[test]
fn update_submodules_to_remote_latest_branch_dot_rejects_detached_parent() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
    let (_clone_dir, _clone_path, mut repo) = clone_super_repo(&fixture, true);
    let head = repo.head().unwrap().target().unwrap();
    repo.set_head_detached(head).unwrap();
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "release v2\n",
        "release",
    );

    let err = fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap_err();

    assert!(err.to_string().contains("branch = ."));
    assert!(err.to_string().contains("父仓库当前不是本地分支"));
}

#[test]
fn update_single_submodule_to_remote_latest_only_updates_named_module() {
    let fixture = create_super_remote_with_two_submodules();
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    let target_one = advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "one v2\n",
        "main",
    );
    let original_two = submodule_head_oid(&clone_path, "deps/two");
    advance_submodule_remote(
        fixture.sub_remotes[1].path(),
        &fixture.service,
        "two v2\n",
        "main",
    );

    fixture
        .service
        .update_submodule_to_remote_latest(&mut repo, "deps/one")
        .unwrap();

    assert_eq!(submodule_head_oid(&clone_path, "deps/one"), target_one);
    assert_eq!(submodule_head_oid(&clone_path, "deps/two"), original_two);
    git_support::assert_file_text(&clone_path, "deps/one/sub.txt", "one v2\n");
    git_support::assert_file_text(&clone_path, "deps/two/sub.txt", "two v1\n");
}

#[test]
fn update_submodules_to_remote_latest_rejects_dirty_submodule() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "sub v2\n",
        "main",
    );
    git_support::write_file(&clone_path, "deps/sub/local.txt", "local\n");

    let err = fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap_err();

    assert!(err.to_string().contains("子模块"));
    assert!(err.to_string().contains("本地改动"));
}

#[test]
fn update_submodules_to_remote_latest_rejects_non_fast_forward() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, mut repo) = clone_super_repo(&fixture, true);
    let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
    git_support::configure_user(&subrepo);
    git_support::write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
    git_support::commit_all(&subrepo, "local submodule commit");
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "remote v2\n",
        "main",
    );

    let err = fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap_err();

    assert!(err.to_string().contains("不能快进到远端最新"));
}

#[test]
fn update_submodules_to_remote_latest_recurses_into_nested_submodules() {
    let fixture = create_nested_submodule_fixture();
    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    fixture
        .service
        .clone_repo_with_options(
            &git_support::path_url(fixture.super_remote.path()),
            &RepoPath::new(&clone_path),
            CloneOptions {
                recursive_submodules: true,
            },
        )
        .unwrap();
    let mut repo = Repository::open(&clone_path).unwrap();
    git_support::configure_user(&repo);
    let leaf_target = advance_submodule_remote(
        fixture.leaf_remote.path(),
        &fixture.service,
        "leaf v2\n",
        "main",
    );

    fixture
        .service
        .update_submodules_to_remote_latest(&mut repo)
        .unwrap();

    assert_eq!(
        submodule_head_oid(&clone_path, "deps/mid/deps/leaf"),
        leaf_target
    );
    git_support::assert_file_text(&clone_path, "deps/mid/deps/leaf/sub.txt", "leaf v2\n");
}

#[test]
fn submodule_remote_sync_status_reports_up_to_date() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::UpToDate
    );
}

#[test]
fn submodule_remote_sync_status_reports_behind() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "sub v2\n",
        "main",
    );

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Behind(1)
    );
}

#[test]
fn submodule_remote_sync_status_reports_ahead() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, true);
    let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
    git_support::configure_user(&subrepo);
    git_support::write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
    git_support::commit_all(&subrepo, "local submodule commit");

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Ahead(1)
    );
}

#[test]
fn submodule_remote_sync_status_reports_diverged() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, true);
    let subrepo = Repository::open(clone_path.join("deps/sub")).unwrap();
    git_support::configure_user(&subrepo);
    git_support::write_file(&clone_path, "deps/sub/sub.txt", "local v2\n");
    git_support::commit_all(&subrepo, "local submodule commit");
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "remote v2\n",
        "main",
    );

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Diverged {
            ahead: 1,
            behind: 1,
        }
    );
}

#[test]
fn submodule_remote_sync_status_uses_gitmodules_branch() {
    let fixture =
        create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("develop"))]);
    let (_clone_dir, _clone_path, repo) = clone_super_repo(&fixture, true);
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "develop v2\n",
        "develop",
    );
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "main v2\n",
        "main",
    );
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "main v3\n",
        "main",
    );

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Behind(1)
    );
}

#[test]
fn submodule_remote_sync_status_branch_dot_uses_parent_branch() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", Some("."))]);
    let (_clone_dir, _clone_path, mut repo) = clone_super_repo(&fixture, true);
    fixture
        .service
        .create_branch(&mut repo, &BranchName::new("release"))
        .unwrap();
    fixture
        .service
        .checkout_branch(&mut repo, &BranchName::new("release"))
        .unwrap();
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "release v2\n",
        "release",
    );
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "main v2\n",
        "main",
    );
    advance_submodule_remote(
        fixture.sub_remotes[0].path(),
        &fixture.service,
        "main v3\n",
        "main",
    );

    assert_eq!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Behind(1)
    );
}

#[test]
fn submodule_remote_sync_status_does_not_initialize_unchecked_out_submodule() {
    let fixture = create_super_remote_with_named_submodules(&[("sub", "sub v1\n", None)]);
    let (_clone_dir, clone_path, repo) = clone_super_repo(&fixture, false);
    assert!(!clone_path.join("deps/sub/sub.txt").exists());

    assert!(matches!(
        single_submodule_remote_status(&fixture.service, &repo),
        SubmoduleRemoteSyncStatus::Unavailable(_)
    ));
    assert!(!clone_path.join("deps/sub/sub.txt").exists());
}

#[test]
fn stage_unstage_and_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "src/lib.rs", "pub fn value() -> i32 { 1 }\n");

    service
        .stage_path(&mut repo, Path::new("src/lib.rs"))
        .unwrap();
    let changes = service.status(&repo).unwrap();
    assert_eq!(changes[0].staged, Some(ChangeState::Added));

    service
        .unstage_path(&mut repo, Path::new("src/lib.rs"))
        .unwrap();
    let changes = service.status(&repo).unwrap();
    assert_eq!(changes[0].unstaged, Some(ChangeState::Untracked));

    service
        .stage_path(&mut repo, Path::new("src/lib.rs"))
        .unwrap();
    git_support::write_file(dir.path(), "src/lib.rs", "pub fn value() -> i32 { 2 }\n");
    let changes = service.status(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "src/lib.rs")
        .unwrap();
    assert_eq!(change.staged, Some(ChangeState::Added));
    assert_eq!(change.unstaged, Some(ChangeState::Modified));
    service
        .stage_path(&mut repo, Path::new("src/lib.rs"))
        .unwrap();
    service
        .commit(&mut repo, &CommitMessage::new("add library"))
        .unwrap();
    assert!(service.status(&repo).unwrap().is_empty());
}

#[test]
fn batch_stage_and_unstage_paths() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "one.txt", "one\n");
    git_support::write_file(dir.path(), "two.txt", "two\n");

    let paths = [Path::new("one.txt"), Path::new("two.txt")];
    service.stage_paths(&mut repo, paths).unwrap();
    let changes = service.status(&repo).unwrap();
    assert!(
        changes.iter().any(|change| {
            change.path == "one.txt" && change.staged == Some(ChangeState::Added)
        })
    );
    assert!(
        changes.iter().any(|change| {
            change.path == "two.txt" && change.staged == Some(ChangeState::Added)
        })
    );

    service.unstage_paths(&mut repo, paths).unwrap();
    let changes = service.status(&repo).unwrap();
    assert!(changes.iter().any(|change| {
        change.path == "one.txt" && change.unstaged == Some(ChangeState::Untracked)
    }));
    assert!(changes.iter().any(|change| {
        change.path == "two.txt" && change.unstaged == Some(ChangeState::Untracked)
    }));
}

#[test]
fn unstage_tracked_change_after_commit_keeps_worktree_change() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "initial\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "file.txt", "changed\n");

    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();
    let changes = service.status(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(change.staged, Some(ChangeState::Modified));
    assert_eq!(change.unstaged, None);

    service
        .unstage_path(&mut repo, Path::new("file.txt"))
        .unwrap();
    let changes = service.status(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(change.staged, None);
    assert_eq!(change.unstaged, Some(ChangeState::Modified));
    git_support::assert_file_text(dir.path(), "file.txt", "changed\n");
}

#[test]
fn batch_stage_handles_deleted_files() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "keep.txt", "keep\n");
    git_support::write_file(dir.path(), "remove.txt", "remove\n");
    git_support::commit_all(&repo, "initial");

    fs::remove_file(dir.path().join("remove.txt")).unwrap();
    git_support::write_file(dir.path(), "keep.txt", "changed\n");

    let paths = [Path::new("keep.txt"), Path::new("remove.txt")];
    service.stage_paths(&mut repo, paths).unwrap();
    let changes = service.status(&repo).unwrap();
    assert!(changes.iter().any(|change| {
        change.path == "keep.txt" && change.staged == Some(ChangeState::Modified)
    }));
    assert!(changes.iter().any(|change| {
        change.path == "remove.txt" && change.staged == Some(ChangeState::Deleted)
    }));
}

#[test]
fn discard_unstaged_keeps_staged_change() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    git_support::write_file(dir.path(), "file.txt", "staged\n");
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();
    git_support::write_file(dir.path(), "file.txt", "worktree\n");

    service
        .discard_unstaged_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    git_support::assert_file_text(dir.path(), "file.txt", "staged\n");
    let changes = service.status_full(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(change.staged, Some(ChangeState::Modified));
    assert_eq!(change.unstaged, None);
}

#[test]
fn discard_all_removes_staged_and_unstaged_changes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    git_support::write_file(dir.path(), "file.txt", "staged\n");
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();
    git_support::write_file(dir.path(), "file.txt", "worktree\n");

    service
        .discard_all_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    git_support::assert_file_text(dir.path(), "file.txt", "base\n");
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_unstaged_removes_untracked_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "new.txt", "new\n");

    service
        .discard_unstaged_path(&mut repo, Path::new("new.txt"))
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_all_removes_staged_added_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "new.txt", "new\n");
    service.stage_path(&mut repo, Path::new("new.txt")).unwrap();

    service
        .discard_all_path(&mut repo, Path::new("new.txt"))
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_all_removes_staged_added_file_in_unborn_repo() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "new.txt", "new\n");
    service.stage_path(&mut repo, Path::new("new.txt")).unwrap();

    service
        .discard_all_path(&mut repo, Path::new("new.txt"))
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_unstaged_restores_deleted_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    fs::remove_file(dir.path().join("file.txt")).unwrap();

    service
        .discard_unstaged_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    git_support::assert_file_text(dir.path(), "file.txt", "base\n");
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_all_restores_staged_deleted_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    fs::remove_file(dir.path().join("file.txt")).unwrap();
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    service
        .discard_all_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    git_support::assert_file_text(dir.path(), "file.txt", "base\n");
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_unstaged_paths_handles_multiple_tracked_changes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "one.txt", "one\n");
    git_support::write_file(dir.path(), "two.txt", "two\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "one.txt", "one changed\n");
    git_support::write_file(dir.path(), "two.txt", "two changed\n");

    service
        .discard_unstaged_paths(&mut repo, [Path::new("one.txt"), Path::new("two.txt")])
        .unwrap();

    git_support::assert_file_text(dir.path(), "one.txt", "one\n");
    git_support::assert_file_text(dir.path(), "two.txt", "two\n");
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_unstaged_paths_removes_multiple_untracked_files() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "one.txt", "one\n");
    git_support::write_file(dir.path(), "two.txt", "two\n");

    service
        .discard_unstaged_paths(&mut repo, [Path::new("one.txt"), Path::new("two.txt")])
        .unwrap();

    assert!(!dir.path().join("one.txt").exists());
    assert!(!dir.path().join("two.txt").exists());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_all_paths_removes_multiple_staged_changes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "modify.txt", "base\n");
    git_support::write_file(dir.path(), "delete.txt", "delete\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "modify.txt", "changed\n");
    fs::remove_file(dir.path().join("delete.txt")).unwrap();
    git_support::write_file(dir.path(), "new.txt", "new\n");
    service
        .stage_paths(
            &mut repo,
            [
                Path::new("modify.txt"),
                Path::new("delete.txt"),
                Path::new("new.txt"),
            ],
        )
        .unwrap();

    service
        .discard_all_paths(
            &mut repo,
            [
                Path::new("modify.txt"),
                Path::new("delete.txt"),
                Path::new("new.txt"),
            ],
        )
        .unwrap();

    git_support::assert_file_text(dir.path(), "modify.txt", "base\n");
    git_support::assert_file_text(dir.path(), "delete.txt", "delete\n");
    assert!(!dir.path().join("new.txt").exists());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_paths_respect_staged_and_unstaged_scope() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "same.txt", "staged\n");
    service
        .stage_path(&mut repo, Path::new("same.txt"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "worktree\n");

    service
        .discard_unstaged_paths(&mut repo, [Path::new("same.txt")])
        .unwrap();

    git_support::assert_file_text(dir.path(), "same.txt", "staged\n");
    let changes = service.status_full(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "same.txt")
        .unwrap();
    assert_eq!(change.staged, Some(ChangeState::Modified));
    assert_eq!(change.unstaged, None);

    service
        .discard_all_paths(&mut repo, [Path::new("same.txt")])
        .unwrap();

    git_support::assert_file_text(dir.path(), "same.txt", "base\n");
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn discard_rejects_conflicted_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let _ = service.merge_branch(&mut repo, &BranchName::new("feature"));
    let err = service
        .discard_unstaged_path(&mut repo, Path::new("same.txt"))
        .unwrap_err();
    assert!(err.to_string().contains("存在冲突"));
}

#[test]
fn discard_paths_reject_conflicts_before_touching_other_files() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::write_file(dir.path(), "safe.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let _ = service.merge_branch(&mut repo, &BranchName::new("feature"));
    // 合并开始前必须保持工作区干净；进入冲突状态后再添加普通修改，验证批量回滚不会误触它。
    git_support::write_file(dir.path(), "safe.txt", "changed\n");
    let err = service
        .discard_unstaged_paths(&mut repo, [Path::new("safe.txt"), Path::new("same.txt")])
        .unwrap_err();

    assert!(err.to_string().contains("存在冲突"));
    git_support::assert_file_text(dir.path(), "safe.txt", "changed\n");
}

#[test]
fn status_fast_skips_untracked_but_keeps_tracked_and_staged_changes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "tracked.txt", "one\n");
    git_support::write_file(dir.path(), "staged.txt", "one\n");
    git_support::commit_all(&repo, "initial");

    git_support::write_file(dir.path(), "tracked.txt", "one\ntwo\n");
    git_support::write_file(dir.path(), "staged.txt", "one\ntwo\n");
    service
        .stage_path(&mut repo, Path::new("staged.txt"))
        .unwrap();
    git_support::write_file(dir.path(), "untracked.txt", "new\n");

    let fast = service.status_fast(&repo).unwrap();
    assert!(fast.iter().any(|change| {
        change.path == "tracked.txt" && change.unstaged == Some(ChangeState::Modified)
    }));
    assert!(fast.iter().any(|change| {
        change.path == "staged.txt" && change.staged == Some(ChangeState::Modified)
    }));
    assert!(!fast.iter().any(|change| change.path == "untracked.txt"));

    let full = service.status_full(&repo).unwrap();
    assert!(full.iter().any(|change| {
        change.path == "untracked.txt" && change.unstaged == Some(ChangeState::Untracked)
    }));
}

#[test]
fn metadata_snapshot_excludes_status_changes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "tracked.txt", "one\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "tracked.txt", "one\ntwo\n");
    git_support::write_file(dir.path(), "untracked.txt", "new\n");

    let metadata = service.snapshot_metadata(&mut repo).unwrap();
    assert_eq!(metadata.head.as_deref(), Some("main"));
    assert!(metadata.branches.iter().any(|branch| branch.name == "main"));
    assert!(metadata.changes.is_empty());

    let full = service.snapshot_details(&mut repo).unwrap();
    assert!(!full.changes.is_empty());
}

#[test]
fn merge_success() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let snapshot = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    assert!(dir.path().join("feature.txt").exists());
    assert!(service.conflicts(&repo).unwrap().is_empty());
    assert!(!snapshot.merge_in_progress);
    assert_eq!(repo.state(), RepositoryState::Clean);
    assert_eq!(repo.head().unwrap().peel_to_commit().unwrap().parent_count(), 2);
}

#[test]
fn up_to_date_merge_does_not_create_commit() {
    let (_dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(_dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let original_head = repo.head().unwrap().target().unwrap();

    let snapshot = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();

    assert_eq!(repo.head().unwrap().target(), Some(original_head));
    assert!(!snapshot.merge_in_progress);
    assert_eq!(repo.state(), RepositoryState::Clean);
}

#[test]
fn fast_forward_merge_keeps_index_clean() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    let snapshot = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();

    assert!(dir.path().join("feature.txt").exists());
    assert!(snapshot.changes.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
    assert!(!snapshot.merge_in_progress);
    assert_eq!(repo.state(), RepositoryState::Clean);
    assert_eq!(repo.head().unwrap().peel_to_commit().unwrap().parent_count(), 1);
}

#[test]
fn merge_conflict_detection() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let err = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    assert!(matches!(err, GitError::Conflicts(paths) if paths == vec!["same.txt"]));
    assert_eq!(repo.state(), RepositoryState::Merge);
    let snapshot = service.snapshot_after_operation(&mut repo).unwrap();
    assert!(snapshot.merge_in_progress);
    assert!(snapshot.merge_message.is_some());
}

#[test]
fn conflicted_merge_can_finish_as_two_parent_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    let unresolved = service.finish_merge(&mut repo, &CommitMessage::new("finish merge"));
    assert!(matches!(unresolved, Err(GitError::Conflicts(_))));
    let empty = service.finish_merge(&mut repo, &CommitMessage::new("  "));
    assert!(matches!(empty, Err(GitError::EmptyCommitMessage)));

    service
        .resolve_conflict_with_side(
            &mut repo,
            Path::new("same.txt"),
            crate::ConflictResolutionSide::Ours,
        )
        .unwrap();
    let snapshot = service
        .finish_merge(&mut repo, &CommitMessage::new("finish merge"))
        .unwrap();

    assert!(!snapshot.merge_in_progress);
    assert!(snapshot.merge_message.is_none());
    assert_eq!(repo.state(), RepositoryState::Clean);
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.parent_count(), 2);
    assert_eq!(commit.message().unwrap(), "finish merge");
    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
}

#[test]
fn reopened_merge_can_restore_message_and_finish() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    drop(repo);

    let mut reopened = Repository::open(dir.path()).unwrap();
    let restored = service.snapshot_after_operation(&mut reopened).unwrap();
    assert!(restored.merge_in_progress);
    assert!(restored.merge_message.is_some());
    service
        .resolve_conflict_with_side(
            &mut reopened,
            Path::new("same.txt"),
            crate::ConflictResolutionSide::Theirs,
        )
        .unwrap();
    service
        .finish_merge(&mut reopened, &CommitMessage::new("finish after reopen"))
        .unwrap();

    assert_eq!(reopened.state(), RepositoryState::Clean);
    assert_eq!(
        reopened.head().unwrap().peel_to_commit().unwrap().parent_count(),
        2
    );
    git_support::assert_file_text(dir.path(), "same.txt", "feature\n");
}

#[test]
fn abort_merge_restores_head_index_and_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");
    let original_head = repo.head().unwrap().target().unwrap();

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap_err();
    let reopened = Repository::open(dir.path()).unwrap();
    assert_eq!(reopened.state(), RepositoryState::Merge);
    drop(reopened);

    let snapshot = service.abort_merge(&mut repo).unwrap();
    assert_eq!(repo.head().unwrap().target(), Some(original_head));
    assert_eq!(repo.state(), RepositoryState::Clean);
    assert!(!snapshot.merge_in_progress);
    assert!(service.status_full(&repo).unwrap().is_empty());
    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
}

#[cfg(windows)]
#[test]
fn conflicted_merge_and_abort_keep_vscode_locked_directory_compatible() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "opened/obsolete.txt", "obsolete\n");
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    fs::remove_file(dir.path().join("opened/obsolete.txt")).unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let opened_directory = dir.path().join("opened");
    let directory_handle = lock_directory_without_delete_share(&opened_directory);
    let merge_result = service.merge_branch(&mut repo, &BranchName::new("feature"));
    assert!(matches!(merge_result, Err(GitError::Conflicts(_))));
    assert!(opened_directory.exists());
    assert!(!opened_directory.join("obsolete.txt").exists());

    let abort_result = service.abort_merge(&mut repo);
    unsafe {
        CloseHandle(directory_handle);
    }
    abort_result.unwrap();
    git_support::assert_file_text(dir.path(), "opened/obsolete.txt", "obsolete\n");
    git_support::assert_file_text(dir.path(), "same.txt", "main\n");
    assert_eq!(repo.state(), RepositoryState::Clean);
}

#[test]
fn merge_rejects_dirty_worktree_without_moving_head() {
    for prepare_dirty in ["unstaged", "staged", "untracked"] {
        let (dir, mut repo, service) = git_support::init_repo();
        git_support::write_file(dir.path(), "base.txt", "base\n");
        git_support::commit_all(&repo, "initial");
        service
            .create_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        service
            .checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        git_support::write_file(dir.path(), "feature.txt", "feature\n");
        git_support::commit_all(&repo, "feature");
        service
            .checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        let original_head = repo.head().unwrap().target().unwrap();

        match prepare_dirty {
            "unstaged" => git_support::write_file(dir.path(), "base.txt", "dirty\n"),
            "staged" => {
                git_support::write_file(dir.path(), "base.txt", "dirty\n");
                service
                    .stage_path(&mut repo, Path::new("base.txt"))
                    .unwrap();
            }
            "untracked" => git_support::write_file(dir.path(), "local.txt", "dirty\n"),
            _ => unreachable!(),
        }

        let err = service
            .merge_branch(&mut repo, &BranchName::new("feature"))
            .unwrap_err();
        assert!(err.to_string().contains("请先提交或贮藏"));
        assert_eq!(repo.head().unwrap().target(), Some(original_head));
        assert_eq!(repo.state(), RepositoryState::Clean);
    }
}

#[test]
fn finish_and_abort_merge_reject_clean_repository() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    let finish = service.finish_merge(&mut repo, &CommitMessage::new("merge"));
    let finish_error = finish.unwrap_err().to_string();
    assert!(
        finish_error.contains("没有正在进行的合并"),
        "{finish_error}"
    );
    let abort = service.abort_merge(&mut repo);
    let abort_error = abort.unwrap_err().to_string();
    assert!(abort_error.contains("没有正在进行的合并"), "{abort_error}");
}

#[test]
fn clone_fetch_push_against_local_bare_remote() {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), "README.md", "seed\n");
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();

    let clone_dir = TempDir::new().unwrap();
    let clone_path = clone_dir.path().join("clone");
    let snapshot = service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &RepoPath::new(&clone_path),
        )
        .unwrap();
    assert_eq!(snapshot.head.as_deref(), Some("main"));

    let mut clone_repo = Repository::open(&clone_path).unwrap();
    git_support::configure_user(&clone_repo);
    git_support::write_file(&clone_path, "clone.txt", "clone\n");
    git_support::commit_all(&clone_repo, "clone");
    service
        .push(&mut clone_repo, &RemoteName::new("origin"))
        .unwrap();

    let other_dir = TempDir::new().unwrap();
    let other_path = other_dir.path().join("other");
    service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &RepoPath::new(&other_path),
        )
        .unwrap();
    assert!(other_path.join("clone.txt").exists());
}

#[test]
fn test_proxy_connects_to_current_remote_refs() {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), "README.md", "seed\n");
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();

    service
        .test_proxy(&seed_repo, &RemoteName::new("origin"))
        .unwrap();
}

#[test]
fn commit_and_push_pushes_new_commit() {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), "README.md", "seed\n");
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();

    git_support::write_file(seed_dir.path(), "next.txt", "next\n");
    service
        .stage_path(&mut seed_repo, Path::new("next.txt"))
        .unwrap();
    let snapshot = service
        .commit_and_push(
            &mut seed_repo,
            &CommitMessage::new("next"),
            &RemoteName::new("origin"),
        )
        .unwrap()
        .unwrap();

    assert!(snapshot.changes.is_empty());
    let status = service
        .branch_sync_status(&seed_repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();
    assert_eq!(status.ahead, 0);
    assert_eq!(status.behind, 0);

    let other_dir = TempDir::new().unwrap();
    let other_path = other_dir.path().join("other");
    service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &RepoPath::new(&other_path),
        )
        .unwrap();
    assert!(other_path.join("next.txt").exists());
}

#[test]
fn commit_and_push_keeps_local_commit_when_push_fails() {
    let remote_dir = TempDir::new().unwrap();
    let mut bare_opts = RepositoryInitOptions::new();
    bare_opts.bare(true).initial_head("main");
    Repository::init_opts(remote_dir.path(), &bare_opts).unwrap();

    let (seed_dir, mut seed_repo, service) = git_support::init_repo();
    git_support::write_file(seed_dir.path(), "README.md", "seed\n");
    git_support::commit_all(&seed_repo, "seed");
    seed_repo
        .remote("origin", &git_support::path_url(remote_dir.path()))
        .unwrap();
    service
        .push(&mut seed_repo, &RemoteName::new("origin"))
        .unwrap();
    fs::remove_dir_all(remote_dir.path()).unwrap();

    git_support::write_file(seed_dir.path(), "local.txt", "local\n");
    service
        .stage_path(&mut seed_repo, Path::new("local.txt"))
        .unwrap();
    let result = service
        .commit_and_push(
            &mut seed_repo,
            &CommitMessage::new("local only"),
            &RemoteName::new("origin"),
        )
        .unwrap();

    let Err((snapshot, err)) = result else {
        panic!("push should fail");
    };
    assert!(err.to_string().contains("Git 错误"));
    assert!(snapshot.changes.is_empty());
    assert_eq!(
        seed_repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .summary()
            .unwrap(),
        Some("local only")
    );
    let status = service
        .branch_sync_status(&seed_repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();
    assert_eq!(status.ahead, 1);
    assert_eq!(status.behind, 0);
}

#[test]
fn open_fast_lists_only_local_branches() {
    let (_remote_dir, _clone_dir, clone_path, mut repo, service) = clone_repo_with_remote_feature();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    let fast = service.open_fast(&RepoPath::new(&clone_path)).unwrap();
    assert!(
        fast.branches
            .iter()
            .all(|branch| branch.kind == BranchKind::Local)
    );
    assert!(fast.branches.iter().any(|branch| branch.name == "main"));
    assert!(fast.remotes.is_empty());
    assert!(fast.changes.is_empty());
    assert!(fast.tags.is_empty());
    assert!(fast.stashes.is_empty());

    let details = service.snapshot_details(&mut repo).unwrap();
    assert!(details.remotes.iter().any(|remote| remote.name == "origin"));
    assert!(
        details
            .branches
            .iter()
            .any(|branch| { branch.kind == BranchKind::Remote && branch.name == "origin/feature" })
    );
}

#[test]
fn branch_sync_status_reports_local_ahead_commits() {
    let (_remote_dir, clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
    git_support::write_file(
        clone_dir.path().join("clone").as_path(),
        "local.txt",
        "local\n",
    );
    let oid = git_support::commit_all(&repo, "local ahead");

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();

    assert_eq!(status.branch, "main");
    assert_eq!(status.ahead, 1);
    assert_eq!(status.behind, 0);
    assert_eq!(status.unpushed_oids, vec![oid.to_string()]);
    assert!(!status.unpushed_oids_truncated);
}

#[test]
fn branch_sync_status_caps_unpushed_oid_list() {
    let (_remote_dir, clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
    for index in 0..(BRANCH_SYNC_UNPUSHED_OID_LIMIT + 1) {
        git_support::write_file(
            clone_dir.path().join("clone").as_path(),
            "local.txt",
            &format!("local {index}\n"),
        );
        git_support::commit_all(&repo, &format!("local ahead {index}"));
    }

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();

    assert_eq!(status.ahead, BRANCH_SYNC_UNPUSHED_OID_LIMIT + 1);
    assert_eq!(status.behind, 0);
    assert_eq!(status.unpushed_oids.len(), BRANCH_SYNC_UNPUSHED_OID_LIMIT);
    assert!(status.unpushed_oids_truncated);
}

#[test]
fn branch_sync_status_reports_remote_behind_commits() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    advance_remote_main(remote_dir.path(), &service, "remote.txt", "remote\n");
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();

    assert_eq!(status.ahead, 0);
    assert_eq!(status.behind, 1);
    assert!(status.unpushed_oids.is_empty());
    assert!(!status.unpushed_oids_truncated);
}

#[test]
fn local_branch_metadata_reports_ahead_and_behind_counts() {
    let (remote_dir, clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    git_support::write_file(
        clone_dir.path().join("clone").as_path(),
        "local.txt",
        "local\n",
    );
    git_support::commit_all(&repo, "local ahead");
    advance_remote_main(remote_dir.path(), &service, "remote.txt", "remote\n");
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    let main = service
        .local_branches(&repo)
        .unwrap()
        .into_iter()
        .find(|branch| branch.name == "main")
        .unwrap();

    assert_eq!(main.ahead, Some(1));
    assert_eq!(main.behind, Some(1));
}

#[test]
fn branch_sync_status_reports_diverged_branch() {
    let (remote_dir, clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    git_support::write_file(
        clone_dir.path().join("clone").as_path(),
        "local.txt",
        "local\n",
    );
    let local_oid = git_support::commit_all(&repo, "local ahead");
    advance_remote_main(remote_dir.path(), &service, "remote.txt", "remote\n");
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();

    assert_eq!(status.ahead, 1);
    assert_eq!(status.behind, 1);
    assert_eq!(status.unpushed_oids, vec![local_oid.to_string()]);
    assert!(!status.unpushed_oids_truncated);
}

#[test]
fn branch_sync_status_falls_back_to_remote_tracking_name() {
    let (_remote_dir, _clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
    let mut branch = repo.find_branch("main", BranchType::Local).unwrap();
    branch.set_upstream(None).unwrap();

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap()
        .unwrap();

    assert_eq!(status.branch, "main");
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!(status.ahead, 0);
    assert_eq!(status.behind, 0);
}

#[test]
fn fetch_prunes_deleted_remote_branch_and_clears_sync_status() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();

    let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
    remote_repo
        .find_reference("refs/heads/feature")
        .unwrap()
        .delete()
        .unwrap();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    assert!(
        repo.find_branch("origin/feature", BranchType::Remote)
            .is_err()
    );
    assert!(
        service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn refresh_with_remote_prunes_deleted_current_upstream() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();

    let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
    remote_repo
        .find_reference("refs/heads/feature")
        .unwrap()
        .delete()
        .unwrap();
    let snapshot = service
        .refresh(&mut repo, Some(&RemoteName::new("origin")))
        .unwrap();

    assert!(
        !snapshot
            .branches
            .iter()
            .any(|branch| { branch.kind == BranchKind::Remote && branch.name == "origin/feature" })
    );
    assert!(
        service
            .branch_sync_status(&repo, &RemoteName::new("origin"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn branch_sync_status_is_unknown_for_detached_head() {
    let (_remote_dir, _clone_dir, _clone_path, repo, service) = clone_repo_with_remote_feature();
    let oid = repo.head().unwrap().target().unwrap();
    repo.set_head_detached(oid).unwrap();

    let status = service
        .branch_sync_status(&repo, &RemoteName::new("origin"))
        .unwrap();

    assert!(status.is_none());
}

#[test]
fn add_remote_returns_name_and_url() {
    let (dir, mut repo, service) = git_support::init_repo();
    let remote_dir = TempDir::new().unwrap();
    let snapshot = service
        .add_remote(
            &mut repo,
            &RemoteName::new("upstream"),
            &git_support::path_url(remote_dir.path()),
        )
        .unwrap();

    let remote = snapshot
        .remotes
        .iter()
        .find(|remote| remote.name == "upstream")
        .unwrap();
    assert_eq!(remote.url, git_support::path_url(remote_dir.path()));
    assert!(dir.path().join(".git").exists());
}

#[test]
fn update_remote_renames_and_updates_fetch_and_push_url() {
    let (_dir, mut repo, service) = git_support::init_repo();
    let old_dir = TempDir::new().unwrap();
    let new_dir = TempDir::new().unwrap();
    service
        .add_remote(
            &mut repo,
            &RemoteName::new("origin"),
            &git_support::path_url(old_dir.path()),
        )
        .unwrap();

    let snapshot = service
        .update_remote(
            &mut repo,
            &RemoteName::new("origin"),
            &RemoteName::new("upstream"),
            &git_support::path_url(new_dir.path()),
        )
        .unwrap();

    assert!(snapshot.remotes.iter().any(|remote| {
        remote.name == "upstream" && remote.url == git_support::path_url(new_dir.path())
    }));
    assert!(
        snapshot
            .remotes
            .iter()
            .all(|remote| remote.name != "origin")
    );
    let remote = repo.find_remote("upstream").unwrap();
    assert_eq!(remote.url().unwrap(), git_support::path_url(new_dir.path()));
    assert_eq!(
        remote.pushurl().unwrap(),
        Some(git_support::path_url(new_dir.path()).as_str())
    );
}

#[test]
fn delete_remote_removes_it_from_snapshot() {
    let (_dir, mut repo, service) = git_support::init_repo();
    let remote_dir = TempDir::new().unwrap();
    service
        .add_remote(
            &mut repo,
            &RemoteName::new("origin"),
            &git_support::path_url(remote_dir.path()),
        )
        .unwrap();

    let snapshot = service
        .delete_remote(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    assert!(
        snapshot
            .remotes
            .iter()
            .all(|remote| remote.name != "origin")
    );
    assert!(repo.find_remote("origin").is_err());
}

#[test]
fn remote_validation_rejects_empty_url_and_duplicate_name() {
    let (_dir, mut repo, service) = git_support::init_repo();
    let remote_dir = TempDir::new().unwrap();

    assert!(
        service
            .add_remote(&mut repo, &RemoteName::new("origin"), "")
            .unwrap_err()
            .to_string()
            .contains("远端地址不能为空")
    );

    service
        .add_remote(
            &mut repo,
            &RemoteName::new("origin"),
            &git_support::path_url(remote_dir.path()),
        )
        .unwrap();
    assert!(
        service
            .add_remote(
                &mut repo,
                &RemoteName::new("origin"),
                &git_support::path_url(remote_dir.path()),
            )
            .unwrap_err()
            .to_string()
            .contains("远端名称已存在")
    );
}

#[test]
fn checkout_remote_branch_creates_tracks_and_switches() {
    let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
        clone_repo_with_remote_feature();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();

    let snapshot = service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();

    assert_eq!(snapshot.head.as_deref(), Some("feature"));
    assert!(repo.find_branch("feature", BranchType::Local).is_ok());
    let branch = repo.find_branch("feature", BranchType::Local).unwrap();
    let upstream = branch.upstream().unwrap();
    assert_eq!(upstream.name().unwrap(), Some("origin/feature"));
    assert!(repo.workdir().unwrap().join("feature.txt").exists());
    assert!(snapshot.changes.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn checkout_remote_branch_reuses_existing_local_branch() {
    let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
        clone_repo_with_remote_feature();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();
    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();

    let snapshot = service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();

    assert_eq!(snapshot.head.as_deref(), Some("feature"));
    let branch = repo.find_branch("feature", BranchType::Local).unwrap();
    let upstream = branch.upstream().unwrap();
    assert_eq!(upstream.name().unwrap(), Some("origin/feature"));
    assert!(snapshot.changes.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn set_branch_upstream_tracks_existing_remote_branch() {
    let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
        clone_repo_with_remote_feature();
    service
        .create_branch(&mut repo, &BranchName::new("topic"))
        .unwrap();

    let snapshot = service
        .set_branch_upstream(
            &mut repo,
            &BranchName::new("topic"),
            &RemoteName::new("origin"),
            &BranchName::new("feature"),
        )
        .unwrap();

    let branch = repo.find_branch("topic", BranchType::Local).unwrap();
    assert_eq!(
        branch.upstream().unwrap().name().unwrap(),
        Some("origin/feature")
    );
    assert!(snapshot.branches.iter().any(|branch| {
        branch.kind == BranchKind::Local
            && branch.name == "topic"
            && branch.upstream.as_deref() == Some("origin/feature")
    }));
}

#[test]
fn set_branch_upstream_rejects_missing_remote_branch() {
    let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
        clone_repo_with_remote_feature();

    let err = service
        .set_branch_upstream(
            &mut repo,
            &BranchName::new("main"),
            &RemoteName::new("origin"),
            &BranchName::new("missing"),
        )
        .unwrap_err();

    assert!(err.to_string().contains("远端分支不存在"));
}

#[test]
fn delete_remote_branch_removes_remote_ref_and_tracking_branch() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();

    let snapshot = service
        .delete_remote_branch(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new("feature"),
        )
        .unwrap();

    let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
    assert!(remote_repo.find_reference("refs/heads/feature").is_err());
    assert!(
        repo.find_branch("origin/feature", BranchType::Remote)
            .is_err()
    );
    assert!(
        !snapshot
            .branches
            .iter()
            .any(|branch| { branch.kind == BranchKind::Remote && branch.name == "origin/feature" })
    );
}

#[test]
fn fast_forward_pull_keeps_index_clean_after_branch_switch() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();
    service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();

    advance_remote_feature(remote_dir.path(), &service);

    let snapshot = service.pull(&mut repo, &RemoteName::new("origin")).unwrap();

    assert_eq!(snapshot.head.as_deref(), Some("feature"));
    assert!(
        fs::read_to_string(repo.workdir().unwrap().join("feature.txt"))
            .unwrap()
            .contains("remote update")
    );
    assert!(snapshot.changes.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[cfg(windows)]
#[test]
fn fast_forward_pull_keeps_vscode_locked_directory_and_updates() {
    let (remote_dir, _clone_dir, clone_path, mut repo, service) = clone_repo_with_remote_feature();
    git_support::write_file(&clone_path, "opened-in-vscode/file.txt", "tracked\n");
    git_support::commit_all(&repo, "add opened directory");
    service.push(&mut repo, &RemoteName::new("origin")).unwrap();

    let remote_work_dir = TempDir::new().unwrap();
    let remote_work_path = remote_work_dir.path().join("remote-work");
    service
        .clone_repo(
            &git_support::path_url(remote_dir.path()),
            &RepoPath::new(&remote_work_path),
        )
        .unwrap();
    let mut remote_repo = Repository::open(&remote_work_path).unwrap();
    git_support::configure_user(&remote_repo);
    fs::remove_file(remote_work_path.join("opened-in-vscode/file.txt")).unwrap();
    let mut remote_index = remote_repo.index().unwrap();
    remote_index
        .remove_path(Path::new("opened-in-vscode/file.txt"))
        .unwrap();
    remote_index.write().unwrap();
    drop(remote_index);
    git_support::commit_all(&remote_repo, "remove opened directory");
    service
        .push(&mut remote_repo, &RemoteName::new("origin"))
        .unwrap();

    let opened_directory = clone_path.join("opened-in-vscode");
    let directory_handle = lock_directory_without_delete_share(&opened_directory);
    let pull_result = service.pull(&mut repo, &RemoteName::new("origin"));
    unsafe {
        CloseHandle(directory_handle);
    }

    let snapshot = pull_result.unwrap();
    assert_eq!(snapshot.head.as_deref(), Some("main"));
    assert!(!opened_directory.join("file.txt").exists());
    assert!(opened_directory.exists());
    assert!(opened_directory.read_dir().unwrap().next().is_none());
    assert!(snapshot.changes.is_empty());
}

#[test]
fn pull_branch_fetches_and_merges_selected_remote_branch() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    advance_remote_feature(remote_dir.path(), &service);

    let snapshot = service
        .pull_branch(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new("feature"),
        )
        .unwrap();

    assert_eq!(snapshot.head.as_deref(), Some("main"));
    assert!(
        fs::read_to_string(repo.workdir().unwrap().join("feature.txt"))
            .unwrap()
            .contains("remote update")
    );
    assert!(snapshot.changes.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn pull_branch_reports_missing_remote_branch_in_chinese() {
    let (_remote_dir, _clone_dir, _clone_path, mut repo, service) =
        clone_repo_with_remote_feature();

    let err = service
        .pull_branch(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new("missing"),
        )
        .unwrap_err();

    assert!(err.to_string().contains("远端分支不存在"));
}

#[test]
fn pull_local_branch_fast_forwards_selected_non_current_branch() {
    let (remote_dir, _clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    service
        .fetch(&mut repo, &RemoteName::new("origin"))
        .unwrap();
    service
        .checkout_remote_branch(&mut repo, &BranchName::new("origin/feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    advance_remote_feature(remote_dir.path(), &service);

    let snapshot = service
        .pull_local_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();

    assert_eq!(snapshot.head.as_deref(), Some("main"));
    let local_oid = repo
        .find_branch("feature", BranchType::Local)
        .unwrap()
        .get()
        .target();
    let remote_oid = repo
        .find_branch("origin/feature", BranchType::Remote)
        .unwrap()
        .get()
        .target();
    assert_eq!(local_oid, remote_oid);
}

#[test]
fn push_branch_to_creates_different_remote_branch_and_sets_upstream() {
    let (remote_dir, clone_dir, _clone_path, mut repo, service) = clone_repo_with_remote_feature();
    git_support::write_file(
        clone_dir.path().join("clone").as_path(),
        "local.txt",
        "local\n",
    );
    git_support::commit_all(&repo, "local branch content");

    let snapshot = service
        .push_branch_to(
            &mut repo,
            &RemoteName::new("origin"),
            &BranchName::new("main"),
            &BranchName::new("published/main"),
            true,
        )
        .unwrap();

    assert!(snapshot.changes.is_empty());
    let branch = repo.find_branch("main", BranchType::Local).unwrap();
    assert_eq!(
        branch.upstream().unwrap().name().unwrap(),
        Some("origin/published/main")
    );
    let remote_repo = Repository::open_bare(remote_dir.path()).unwrap();
    assert!(
        remote_repo
            .find_reference("refs/heads/published/main")
            .is_ok()
    );
}

#[test]
fn credential_provider_is_called_when_required() {
    struct CountingProvider(Arc<std::sync::atomic::AtomicUsize>);

    impl CredentialProvider for CountingProvider {
        fn credential_for(
            &self,
            _request: CredentialRequest,
        ) -> Result<Option<crate::credentials::GitCredential>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
    }

    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = GitService::new(
        Arc::new(CountingProvider(count.clone())),
        Arc::new(NoopProgress),
    );
    let result = credential_for_remote(
        None,
        service.credential_provider.as_ref(),
        "https://example.invalid/repo.git",
        None,
        CredentialType::USER_PASS_PLAINTEXT,
        None,
    );
    assert!(result.is_err());
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn credential_provider_credential_is_used_before_external_fallbacks() {
    struct StaticProvider;

    impl CredentialProvider for StaticProvider {
        fn credential_for(
            &self,
            request: CredentialRequest,
        ) -> Result<Option<crate::credentials::GitCredential>> {
            Ok(Some(crate::credentials::GitCredential::UserPass {
                username: request.username_from_url.unwrap_or_else(|| "git".into()),
                secret: "token".into(),
                display_name: None,
                save_to_keyring: false,
                scope: crate::credentials::CredentialScope::RemoteUrl,
            }))
        }
    }

    let result = credential_for_remote(
        None,
        &StaticProvider,
        "https://example.invalid/repo.git",
        Some("alice"),
        CredentialType::USER_PASS_PLAINTEXT | CredentialType::DEFAULT,
        None,
    );

    assert!(result.is_ok());
}

#[test]
fn diff_for_staged_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "file.txt", "one\ntwo\n");
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    let diff = service
        .diff_for_path(
            &repo,
            Path::new("file.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    let added = diff
        .lines
        .iter()
        .find(|line| line.kind == DiffLineKind::Added && line.content.contains("two"))
        .unwrap();
    assert_eq!(added.old_lineno, None);
    assert_eq!(added.new_lineno, Some(2));
}

#[test]
fn diff_uses_three_context_lines() {
    let (dir, mut repo, service) = git_support::init_repo();
    let original = (1..=12)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    git_support::write_file(dir.path(), "file.txt", &original);
    git_support::commit_all(&repo, "initial");

    let modified = (1..=12)
        .map(|line| {
            if line == 8 {
                "line 8 changed\n".to_string()
            } else {
                format!("line {line}\n")
            }
        })
        .collect::<String>();
    git_support::write_file(dir.path(), "file.txt", &modified);
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    let diff = service
        .diff_for_path(
            &repo,
            Path::new("file.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    let body = diff
        .lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Header)
        .map(|line| line.content.as_str())
        .collect::<Vec<_>>();

    assert!(body.iter().any(|line| line.contains("line 5")));
    assert!(body.iter().any(|line| line.contains("line 11")));
    assert!(!body.iter().any(|line| line.contains("line 4")));
    assert!(!body.iter().any(|line| line.contains("line 12")));
}

#[test]
fn diff_full_context_includes_entire_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    let original = (1..=12)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    git_support::write_file(dir.path(), "file.txt", &original);
    git_support::commit_all(&repo, "initial");

    let modified = (1..=12)
        .map(|line| {
            if line == 8 {
                "line 8 changed\n".to_string()
            } else {
                format!("line {line}\n")
            }
        })
        .collect::<String>();
    git_support::write_file(dir.path(), "file.txt", &modified);
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();

    // 全文上下文应包含远离改动的整段未改行（紧凑 3 行上下文会排除它们）。
    let diff = service
        .diff_for_path(
            &repo,
            Path::new("file.txt"),
            DiffScope::Staged,
            true,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    let body: Vec<&str> = diff
        .lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Header)
        .map(|line| line.content.as_str())
        .collect();

    // "line 01" 这种独占前缀的断言避免与 "line 1X" 子串混淆。
    assert!(body.iter().any(|line| *line == "line 1"));
    assert!(body.iter().any(|line| *line == "line 12"));
    // 改动行依旧按 Added/Removed 高亮。
    assert!(
        diff.lines
            .iter()
            .any(|line| { line.kind == DiffLineKind::Removed && line.content == "line 8" })
    );
    assert!(
        diff.lines
            .iter()
            .any(|line| { line.kind == DiffLineKind::Added && line.content == "line 8 changed" })
    );
}

#[test]
fn diff_full_context_rejects_oversized_file() {
    let (dir, mut repo, service) = git_support::init_repo();
    // 生成一个超过全文阈值的文件并提交，再做一处改动后暂存。
    let big = "a".repeat((FULL_FILE_MAX_BYTES + 1024) as usize) + "\n";
    git_support::write_file(dir.path(), "big.txt", &big);
    git_support::commit_all(&repo, "initial");
    let mut modified = big.clone();
    modified.push_str("tail change\n");
    git_support::write_file(dir.path(), "big.txt", &modified);
    service.stage_path(&mut repo, Path::new("big.txt")).unwrap();

    // 全文视图应被字节预检拦截。
    let err = service
        .diff_for_path(
            &repo,
            Path::new("big.txt"),
            DiffScope::Staged,
            true,
            DiffEncodingChoice::Auto,
        )
        .unwrap_err();
    assert!(err.to_string().contains(FULL_FILE_TOO_LARGE_MESSAGE));

    // 紧凑差异在该文件上仍可正常生成。
    let compact = service
        .diff_for_path(
            &repo,
            Path::new("big.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    assert!(
        compact
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.content.contains("tail change"))
    );
}

#[test]
fn diff_auto_detects_gb18030_text() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_bytes(dir.path(), "cn.txt", b"hello\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_bytes(dir.path(), "cn.txt", &[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
    service.stage_path(&mut repo, Path::new("cn.txt")).unwrap();

    let diff = service
        .diff_for_path(
            &repo,
            Path::new("cn.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();

    assert_eq!(diff.encoding.requested, DiffEncodingChoice::Auto);
    assert_eq!(diff.encoding.resolved, DiffEncodingChoice::Gb18030);
    assert!(
        diff.lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.content.contains("你好"))
    );
}

#[test]
fn diff_auto_detection_uses_bounded_sample_for_large_diff() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_bytes(dir.path(), "large-cn.txt", b"seed\n");
    git_support::commit_all(&repo, "initial");
    let mut body = Vec::new();
    for _ in 0..(DIFF_ENCODING_SAMPLE_LIMIT / 4) {
        body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
    }
    body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3, b'\n']);
    git_support::write_bytes(dir.path(), "large-cn.txt", &body);
    service
        .stage_path(&mut repo, Path::new("large-cn.txt"))
        .unwrap();

    let diff = service
        .diff_for_path(
            &repo,
            Path::new("large-cn.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();

    assert_eq!(diff.encoding.resolved, DiffEncodingChoice::Gb18030);
    assert!(diff.lines.iter().any(|line| line.content.contains("你好")));
}

#[test]
fn diff_manual_big5_decodes_text() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_bytes(dir.path(), "big5.txt", b"hello\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_bytes(dir.path(), "big5.txt", &[0xa7, 0x41, 0xa6, 0x6e, b'\n']);
    service
        .stage_path(&mut repo, Path::new("big5.txt"))
        .unwrap();

    let utf8_diff = service
        .diff_for_path(
            &repo,
            Path::new("big5.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Utf8,
        )
        .unwrap();
    assert!(utf8_diff.encoding.lossy);

    let big5_diff = service
        .diff_for_path(
            &repo,
            Path::new("big5.txt"),
            DiffScope::Staged,
            false,
            DiffEncodingChoice::Big5,
        )
        .unwrap();

    assert_eq!(big5_diff.encoding.resolved, DiffEncodingChoice::Big5);
    assert!(
        big5_diff
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.content.contains("你好"))
    );
}

#[test]
fn commit_history_pages_and_commit_diff() {
    let (dir, repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "root.txt", "root\n");
    let root_oid = git_support::commit_all(&repo, "root commit");
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "add file");
    git_support::write_file(dir.path(), "file.txt", "one\ntwo\n");
    git_support::commit_all(&repo, "modify file");

    let first_page = service
        .commit_history(&repo, HistoryScope::CurrentBranch, 0, 2)
        .unwrap();
    assert_eq!(first_page.len(), 2);
    assert_eq!(first_page[0].summary, "modify file");
    assert_eq!(first_page[1].summary, "add file");

    let second_page = service
        .commit_history(&repo, HistoryScope::CurrentBranch, 2, 2)
        .unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].summary, "root commit");
    assert_ne!(first_page[1].oid, second_page[0].oid);

    let files = service.commit_files(&repo, &first_page[0].oid).unwrap();
    assert!(
        files
            .iter()
            .any(|file| { file.path == "file.txt" && file.status == ChangeState::Modified })
    );
    let diff = service
        .commit_file_diff(
            &repo,
            &first_page[0].oid,
            Path::new("file.txt"),
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    assert!(diff.lines.iter().any(|line| {
        line.kind == DiffLineKind::Added
            && line.content.contains("two")
            && line.new_lineno == Some(2)
    }));

    let root_files = service.commit_files(&repo, &root_oid.to_string()).unwrap();
    assert!(
        root_files
            .iter()
            .any(|file| { file.path == "root.txt" && file.status == ChangeState::Added })
    );
    let root_diff = service
        .commit_file_diff(
            &repo,
            &root_oid.to_string(),
            Path::new("root.txt"),
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    assert!(
        root_diff
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.content.contains("root"))
    );
}

#[test]
fn commit_history_scope_current_branch_excludes_other_branch_commits() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "base");
    service
        .create_branch(&mut repo, &BranchName::new("side"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("side"))
        .unwrap();
    git_support::write_file(dir.path(), "side.txt", "side\n");
    git_support::commit_all(&repo, "side only");
    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();

    let current = service
        .commit_history(&repo, HistoryScope::CurrentBranch, 0, 20)
        .unwrap();
    let all = service
        .commit_history(&repo, HistoryScope::AllRefs, 0, 20)
        .unwrap();

    assert!(!current.iter().any(|commit| commit.summary == "side only"));
    assert!(all.iter().any(|commit| commit.summary == "side only"));
}

#[test]
fn commit_history_with_refs_reuses_reference_cache_for_pagination() {
    let (dir, repo, service) = git_support::init_repo();
    for index in 0..5 {
        git_support::write_file(dir.path(), "file.txt", &format!("value {index}\n"));
        git_support::commit_all(&repo, &format!("commit {index}"));
    }
    let head_oid = repo.head().unwrap().target().unwrap();
    repo.tag_lightweight(
        "v-head",
        repo.find_commit(head_oid).unwrap().as_object(),
        false,
    )
    .unwrap();

    let (first_page, refs_cache) = service
        .commit_history_with_refs(&repo, HistoryScope::CurrentBranch, 0, 2, None)
        .unwrap();
    let (second_page, reused_cache) = service
        .commit_history_with_refs(&repo, HistoryScope::CurrentBranch, 2, 2, Some(&refs_cache))
        .unwrap();
    let baseline_second_page = service
        .commit_history(&repo, HistoryScope::CurrentBranch, 2, 2)
        .unwrap();

    assert_eq!(first_page.len(), 2);
    assert_eq!(second_page, baseline_second_page);
    assert_eq!(reused_cache.starts, refs_cache.starts);
    assert_eq!(reused_cache.refs_by_oid, refs_cache.refs_by_oid);
}

#[test]
fn commit_graph_lists_all_branch_reachable_commits_and_refs() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "root.txt", "root\n");
    let root_oid = git_support::commit_all(&repo, "root commit");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    let feature_oid = git_support::commit_all(&repo, "feature commit");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    let main_oid = git_support::commit_all(&repo, "main commit");

    repo.reference("refs/remotes/origin/feature", feature_oid, true, "test")
        .unwrap();
    let feature_commit = repo.find_commit(feature_oid).unwrap();
    repo.tag_lightweight("v-feature", feature_commit.as_object(), false)
        .unwrap();
    drop(feature_commit);

    let commits = service.commit_graph(&repo, 0, 20).unwrap();
    let summaries = commits
        .iter()
        .map(|commit| commit.summary.as_str())
        .collect::<Vec<_>>();
    assert!(summaries.contains(&"main commit"));
    assert!(summaries.contains(&"feature commit"));
    assert!(summaries.contains(&"root commit"));

    let feature = commits
        .iter()
        .find(|commit| commit.oid == feature_oid.to_string())
        .unwrap();
    assert!(feature.parents.contains(&root_oid.to_string()));
    assert!(feature.refs.iter().any(|reference| {
        reference.kind == CommitRefKind::LocalBranch && reference.name == "feature"
    }));
    assert!(feature.refs.iter().any(|reference| {
        reference.kind == CommitRefKind::RemoteBranch && reference.name == "origin/feature"
    }));
    assert!(feature.refs.iter().any(|reference| {
        reference.kind == CommitRefKind::Tag && reference.name == "v-feature"
    }));

    let main = commits
        .iter()
        .find(|commit| commit.oid == main_oid.to_string())
        .unwrap();
    assert!(
        main.refs
            .iter()
            .any(|reference| reference.kind == CommitRefKind::Head)
    );
}

#[test]
fn commit_graph_records_merge_parents() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    let feature_oid = git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    let main_oid = git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();

    let commits = service.commit_graph(&repo, 0, 20).unwrap();
    let merge = commits
        .iter()
        .find(|commit| commit.oid == merge_oid.to_string())
        .unwrap();
    assert_eq!(merge.parents.len(), 2);
    assert!(merge.parents.contains(&main_oid.to_string()));
    assert!(merge.parents.contains(&feature_oid.to_string()));
}

#[test]
fn commit_graph_paginates_without_duplicates() {
    let (dir, repo, service) = git_support::init_repo();
    for index in 0..5 {
        git_support::write_file(dir.path(), "file.txt", &format!("{index}\n"));
        git_support::commit_all(&repo, &format!("commit {index}"));
    }

    let first_page = service.commit_graph(&repo, 0, 3).unwrap();
    let second_page = service.commit_graph(&repo, 3, 3).unwrap();
    assert_eq!(first_page.len(), 3);
    assert_eq!(second_page.len(), 2);
    let first_oids = first_page
        .iter()
        .map(|commit| commit.oid.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for commit in second_page {
        assert!(!first_oids.contains(commit.oid.as_str()));
    }
}

#[test]
fn commit_graph_empty_repo_returns_empty() {
    let (_dir, repo, service) = git_support::init_repo();
    let commits = service.commit_graph(&repo, 0, 20).unwrap();
    assert!(commits.is_empty());
}

#[test]
fn reset_to_commit_soft_keeps_changes_staged() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    git_support::commit_all(&repo, "two");

    service
        .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Soft)
        .unwrap();

    assert_eq!(repo.head().unwrap().target(), Some(first_oid));
    let changes = service.status_full(&repo).unwrap();
    let file = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(file.staged, Some(ChangeState::Modified));
    assert_eq!(file.unstaged, None);
}

#[test]
fn reset_to_commit_mixed_keeps_changes_unstaged() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    git_support::commit_all(&repo, "two");

    service
        .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Mixed)
        .unwrap();

    assert_eq!(repo.head().unwrap().target(), Some(first_oid));
    let changes = service.status_full(&repo).unwrap();
    let file = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(file.staged, None);
    assert_eq!(file.unstaged, Some(ChangeState::Modified));
}

#[test]
fn reset_to_commit_hard_updates_head_and_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    git_support::commit_all(&repo, "two");

    service
        .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Hard)
        .unwrap();

    assert_eq!(repo.head().unwrap().target(), Some(first_oid));
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\n"
    );
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn reset_to_commit_rejects_detached_head() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    repo.set_head_detached(first_oid).unwrap();

    let error = service
        .reset_to_commit(&mut repo, &first_oid.to_string(), ResetMode::Mixed)
        .unwrap_err()
        .to_string();
    assert!(error.contains("detached HEAD"));
}

#[test]
fn uncommit_to_staged_soft_resets_head_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    let second_oid = git_support::commit_all(&repo, "two");

    service
        .uncommit_to_staged(&mut repo, &second_oid.to_string())
        .unwrap();

    assert_eq!(repo.head().unwrap().target(), Some(first_oid));
    let changes = service.status_full(&repo).unwrap();
    let file = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(file.staged, Some(ChangeState::Modified));
    assert_eq!(file.unstaged, None);
    git_support::assert_file_text(dir.path(), "file.txt", "two\n");
}

#[test]
fn uncommit_to_staged_rejects_non_head_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    let second_oid = git_support::commit_all(&repo, "two");

    let error = service
        .uncommit_to_staged(&mut repo, &first_oid.to_string())
        .unwrap_err()
        .to_string();

    assert!(error.contains("只能将当前最新提交还原到暂存区"));
    assert_eq!(repo.head().unwrap().target(), Some(second_oid));
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn uncommit_to_staged_rejects_initial_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let first_oid = git_support::commit_all(&repo, "one");

    let error = service
        .uncommit_to_staged(&mut repo, &first_oid.to_string())
        .unwrap_err()
        .to_string();

    assert!(error.contains("初始提交暂不支持还原到暂存区"));
    assert_eq!(repo.head().unwrap().target(), Some(first_oid));
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn uncommit_to_staged_rejects_merge_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "shared.txt", "base\n");
    git_support::commit_all(&repo, "base");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();

    let error = service
        .uncommit_to_staged(&mut repo, &merge_oid.to_string())
        .unwrap_err()
        .to_string();

    assert!(error.contains("合并提交暂不支持还原到暂存区"));
    assert_eq!(repo.head().unwrap().target(), Some(merge_oid));
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn revert_commit_creates_new_commit_and_restores_content() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    let second_oid = git_support::commit_all(&repo, "two");

    service
        .revert_commit(&mut repo, &second_oid.to_string())
        .unwrap();

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_ne!(head.id(), second_oid);
    assert!(head.summary().unwrap().unwrap().contains("Revert"));
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\n"
    );
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn revert_commit_rejects_dirty_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "one");
    git_support::write_file(dir.path(), "file.txt", "two\n");
    let second_oid = git_support::commit_all(&repo, "two");
    git_support::write_file(dir.path(), "scratch.txt", "dirty\n");

    let error = service
        .revert_commit(&mut repo, &second_oid.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("工作区修改"));
}

#[test]
fn revert_commit_rejects_merge_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();

    let error = service
        .revert_commit(&mut repo, &merge_oid.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("合并提交"));
}

#[test]
fn revert_merge_commit_with_first_parent_creates_revert_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");

    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();

    let snapshot = service
        .revert_merge_commit(&mut repo, &merge_oid.to_string())
        .unwrap();

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_ne!(head.id(), merge_oid);
    assert!(head.summary().unwrap().unwrap().contains("Revert"));
    assert!(!dir.path().join("feature.txt").exists());
    git_support::assert_file_text(dir.path(), "main.txt", "main\n");
    assert!(snapshot.conflicts.is_empty());
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn revert_merge_commit_rejects_non_merge_commit() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    let oid = git_support::commit_all(&repo, "one");

    let error = service
        .revert_merge_commit(&mut repo, &oid.to_string())
        .unwrap_err()
        .to_string();

    assert!(error.contains("不是合并提交"));
    assert_eq!(repo.head().unwrap().target(), Some(oid));
    assert!(service.status_full(&repo).unwrap().is_empty());
}

#[test]
fn revert_merge_commit_rejects_dirty_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");
    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();
    git_support::write_file(dir.path(), "scratch.txt", "dirty\n");

    let error = service
        .revert_merge_commit(&mut repo, &merge_oid.to_string())
        .unwrap_err()
        .to_string();

    assert!(error.contains("工作区修改"));
    assert_eq!(repo.head().unwrap().target(), Some(merge_oid));
}

#[test]
fn revert_merge_commit_reports_conflicts() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "same.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "same.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");
    service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    let merge_oid = repo.head().unwrap().target().unwrap();

    git_support::write_file(dir.path(), "same.txt", "main after merge\n");
    git_support::commit_all(&repo, "main after merge");

    let err = service
        .revert_merge_commit(&mut repo, &merge_oid.to_string())
        .unwrap_err();

    match err {
        GitError::Conflicts(paths) => assert_eq!(paths, vec!["same.txt"]),
        other => panic!("unexpected error: {other:?}"),
    }
    let snapshot = service.snapshot_after_operation(&mut repo).unwrap();
    assert_eq!(snapshot.conflicts, vec!["same.txt"]);
}

#[test]
fn merge_commit_files_use_first_parent_diff() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    service
        .create_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    service
        .checkout_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    git_support::write_file(dir.path(), "feature.txt", "feature\n");
    git_support::commit_all(&repo, "feature");

    service
        .checkout_branch(&mut repo, &BranchName::new("main"))
        .unwrap();
    git_support::write_file(dir.path(), "main.txt", "main\n");
    git_support::commit_all(&repo, "main");

    let snapshot = service
        .merge_branch(&mut repo, &BranchName::new("feature"))
        .unwrap();
    assert_eq!(snapshot.head.as_deref(), Some("main"));

    let merge_oid = repo.head().unwrap().target().unwrap().to_string();
    let commit = repo
        .find_commit(git2::Oid::from_str(&merge_oid).unwrap())
        .unwrap();
    assert_eq!(commit.parent_count(), 2);

    let files = service.commit_files(&repo, &merge_oid).unwrap();
    assert!(
        files
            .iter()
            .any(|file| { file.path == "feature.txt" && file.status == ChangeState::Added })
    );
    assert!(!files.iter().any(|file| file.path == "main.txt"));
}

#[test]
fn snapshot_lists_tags_and_stashes() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "initial");

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.tag_lightweight("v1.0.0", head.as_object(), false)
        .unwrap();
    drop(head);

    git_support::write_file(dir.path(), "scratch.txt", "stash me\n");
    let signature = signature(&repo).unwrap();
    repo.stash_save(
        &signature,
        "work in progress",
        Some(git2::StashFlags::INCLUDE_UNTRACKED),
    )
    .unwrap();

    let snapshot = service.snapshot(&mut repo).unwrap();
    assert!(snapshot.tags.iter().any(|tag| tag.name == "v1.0.0"));
    assert_eq!(snapshot.stashes.len(), 1);
    assert_eq!(snapshot.stashes[0].index, 0);
    assert!(snapshot.stashes[0].message.contains("work in progress"));
}

#[test]
fn checkout_tag_detaches_head_and_updates_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "one\n");
    git_support::commit_all(&repo, "one");
    let first = repo.head().unwrap().peel_to_commit().unwrap();
    repo.tag_lightweight("v1", first.as_object(), false)
        .unwrap();
    drop(first);

    git_support::write_file(dir.path(), "file.txt", "two\n");
    git_support::commit_all(&repo, "two");

    service
        .checkout_tag(&mut repo, &TagName::new("v1"))
        .unwrap();

    assert!(repo.head_detached().unwrap());
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\n"
    );
}

#[test]
fn stash_apply_keeps_entry_and_pop_removes_it() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    git_support::write_file(dir.path(), "file.txt", "applied\n");
    let sig = signature(&repo).unwrap();
    repo.stash_save(&sig, "change file", None).unwrap();
    assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

    service.apply_stash(&mut repo, 0).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "applied\n"
    );
    assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

    let (pop_dir, mut pop_repo, pop_service) = git_support::init_repo();
    git_support::write_file(pop_dir.path(), "file.txt", "base\n");
    git_support::commit_all(&pop_repo, "initial");
    git_support::write_file(pop_dir.path(), "file.txt", "popped\n");
    let sig = signature(&pop_repo).unwrap();
    pop_repo.stash_save(&sig, "pop file", None).unwrap();

    pop_service.pop_stash(&mut pop_repo, 0).unwrap();
    assert_eq!(
        fs::read_to_string(pop_dir.path().join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "popped\n"
    );
    assert!(pop_service.stashes(&mut pop_repo).unwrap().is_empty());
}

#[cfg(windows)]
#[test]
fn apply_stash_keeps_vscode_locked_directory_and_applies_deletion() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "opened-in-vscode/tracked.txt", "tracked\n");
    git_support::commit_all(&repo, "initial");
    fs::remove_file(dir.path().join("opened-in-vscode/tracked.txt")).unwrap();
    service
        .save_stash(&mut repo, "delete tracked file", false, false)
        .unwrap();

    let opened_directory = dir.path().join("opened-in-vscode");
    let directory_handle = lock_directory_without_delete_share(&opened_directory);
    let apply_result = service.apply_stash(&mut repo, 0);
    unsafe {
        CloseHandle(directory_handle);
    }

    let snapshot = apply_result.unwrap();
    assert!(!opened_directory.join("tracked.txt").exists());
    assert!(opened_directory.exists());
    assert!(opened_directory.read_dir().unwrap().next().is_none());
    assert_eq!(snapshot.stashes.len(), 1);
}

#[cfg(windows)]
#[test]
fn save_stash_keeps_vscode_locked_directory_and_cleans_worktree() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "README.md", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "opened-in-vscode/staged.txt", "staged\n");
    let mut index = repo.index().unwrap();
    index
        .add_path(Path::new("opened-in-vscode/staged.txt"))
        .unwrap();
    index.write().unwrap();
    drop(index);

    let opened_directory = dir.path().join("opened-in-vscode");
    let directory_handle = lock_directory_without_delete_share(&opened_directory);
    let stash_result = service.save_stash(&mut repo, "locked directory", false, false);
    unsafe {
        CloseHandle(directory_handle);
    }

    let snapshot = stash_result.unwrap();
    assert!(!opened_directory.join("staged.txt").exists());
    assert!(opened_directory.exists());
    assert!(opened_directory.read_dir().unwrap().next().is_none());
    assert!(snapshot.changes.is_empty());
    assert_eq!(snapshot.stashes.len(), 1);
}

#[test]
fn save_stash_stashes_tracked_changes_and_lists_diff_files() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");

    git_support::write_file(dir.path(), "file.txt", "changed\n");
    let snapshot = service
        .save_stash(&mut repo, "tracked work", false, false)
        .unwrap();

    assert!(snapshot.changes.is_empty());
    git_support::assert_file_text(dir.path(), "file.txt", "base\n");
    assert_eq!(snapshot.stashes.len(), 1);
    assert!(snapshot.stashes[0].message.contains("tracked work"));

    let files = service
        .stash_files(&repo, &snapshot.stashes[0].oid)
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "file.txt");
    assert_eq!(files[0].status, ChangeState::Modified);

    let diff = service
        .stash_file_diff(
            &repo,
            &snapshot.stashes[0].oid,
            Path::new("file.txt"),
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    assert!(
        diff.lines
            .iter()
            .any(|line| line.content.contains("changed"))
    );
}

#[test]
fn save_stash_include_untracked_records_untracked_files() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "base.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "new.txt", "new\n");

    let err = service
        .save_stash(&mut repo, "skip untracked", false, false)
        .unwrap_err();
    assert!(err.to_string().contains("包含未跟踪文件"));
    assert!(dir.path().join("new.txt").exists());

    let snapshot = service
        .save_stash(&mut repo, "with untracked", true, false)
        .unwrap();
    assert!(!dir.path().join("new.txt").exists());
    let files = service
        .stash_files(&repo, &snapshot.stashes[0].oid)
        .unwrap();
    assert!(
        files
            .iter()
            .any(|file| file.path == "new.txt" && file.status == ChangeState::Untracked)
    );

    let diff = service
        .stash_file_diff(
            &repo,
            &snapshot.stashes[0].oid,
            Path::new("new.txt"),
            false,
            DiffEncodingChoice::Auto,
        )
        .unwrap();
    assert!(diff.lines.iter().any(|line| line.content.contains("new")));
}

#[test]
fn save_stash_keep_index_preserves_staged_content() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "file.txt", "staged\n");
    service
        .stage_path(&mut repo, Path::new("file.txt"))
        .unwrap();
    git_support::write_file(dir.path(), "file.txt", "worktree\n");

    service
        .save_stash(&mut repo, "keep staged", false, true)
        .unwrap();

    git_support::assert_file_text(dir.path(), "file.txt", "staged\n");
    let changes = service.status_full(&repo).unwrap();
    let change = changes
        .iter()
        .find(|change| change.path == "file.txt")
        .unwrap();
    assert_eq!(change.staged, Some(ChangeState::Modified));
    assert_eq!(change.unstaged, None);
}

#[test]
fn drop_stash_removes_entry() {
    let (dir, mut repo, service) = git_support::init_repo();
    git_support::write_file(dir.path(), "file.txt", "base\n");
    git_support::commit_all(&repo, "initial");
    git_support::write_file(dir.path(), "file.txt", "changed\n");

    service
        .save_stash(&mut repo, "to drop", false, false)
        .unwrap();
    assert_eq!(service.stashes(&mut repo).unwrap().len(), 1);

    let snapshot = service.drop_stash(&mut repo, 0).unwrap();
    assert!(snapshot.stashes.is_empty());
    assert!(service.stashes(&mut repo).unwrap().is_empty());
}
