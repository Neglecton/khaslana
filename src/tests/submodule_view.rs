use super::*;

fn module_with_status(status: SubmoduleState) -> SubmoduleInfo {
    SubmoduleInfo {
        name: "deps/sub".to_string(),
        path: "deps/sub".into(),
        url: None,
        branch: None,
        head_id: None,
        index_id: None,
        workdir_id: None,
        status,
    }
}

fn ready_status() -> SubmoduleState {
    SubmoduleState {
        initialized: true,
        checked_out: true,
        head_matches_index: true,
        workdir_modified: false,
        workdir_untracked: false,
    }
}

#[test]
fn submodule_status_display_prioritizes_local_worktree_problems() {
    let module = module_with_status(SubmoduleState {
        workdir_modified: true,
        ..ready_status()
    });

    assert_eq!(
        submodule_status_display(&module, Some(&SubmoduleRemoteSyncStatus::Behind(3))),
        ("有改动".to_string(), SubmoduleStatusTone::Warning)
    );
}

#[test]
fn submodule_status_display_maps_remote_ahead_behind() {
    let module = module_with_status(ready_status());

    assert_eq!(
        submodule_status_display(&module, Some(&SubmoduleRemoteSyncStatus::UpToDate)),
        ("远端同步".to_string(), SubmoduleStatusTone::Ready)
    );
    assert_eq!(
        submodule_status_display(&module, Some(&SubmoduleRemoteSyncStatus::Behind(2))),
        ("落后 2".to_string(), SubmoduleStatusTone::Info)
    );
    assert_eq!(
        submodule_status_display(&module, Some(&SubmoduleRemoteSyncStatus::Ahead(1))),
        ("超前 1".to_string(), SubmoduleStatusTone::Warning)
    );
    assert_eq!(
        submodule_status_display(
            &module,
            Some(&SubmoduleRemoteSyncStatus::Diverged {
                ahead: 1,
                behind: 2,
            }),
        ),
        ("分叉 1/2".to_string(), SubmoduleStatusTone::Danger)
    );
}

#[test]
fn submodule_status_display_keeps_local_label_before_remote_check_finishes() {
    let module = module_with_status(SubmoduleState {
        head_matches_index: false,
        ..ready_status()
    });

    assert_eq!(
        submodule_status_display(&module, Some(&SubmoduleRemoteSyncStatus::Checking)),
        ("需更新".to_string(), SubmoduleStatusTone::Warning)
    );
    assert_eq!(
        submodule_status_display(&module, None),
        ("需更新".to_string(), SubmoduleStatusTone::Warning)
    );
}
