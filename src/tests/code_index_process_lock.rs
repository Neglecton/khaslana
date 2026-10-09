use super::*;
use crate::git::test_support::git_test_support::{init_repo, write_file};
use crate::code_index::{PipelineOptions, RunOutcome, run_index, index_generation, search_symbols};

fn options() -> PipelineOptions {
    PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}))
}

#[test]
fn independent_writer_child() {
    let Some(root) = std::env::var_os("KHASLANA_INDEX_TEST_ROOT") else { return; };
    let db = std::path::PathBuf::from(std::env::var_os("KHASLANA_INDEX_TEST_DB").unwrap());
    let blocked = std::env::var("KHASLANA_INDEX_TEST_BLOCKED").unwrap() == "1";
    let result = run_index(std::path::Path::new(&root), &db, false, &mut options());
    if blocked {
        assert!(result.unwrap_err().to_string().contains("其他进程"));
    } else {
        assert!(matches!(result.unwrap(), RunOutcome::Completed(_)));
    }
}

fn run_child(root: &std::path::Path, db: &std::path::Path, blocked: bool) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "code_index::jobs::process_lock_tests::independent_writer_child", "--quiet"])
        .env("KHASLANA_INDEX_TEST_ROOT", root).env("KHASLANA_INDEX_TEST_DB", db)
        .env("KHASLANA_INDEX_TEST_BLOCKED", if blocked { "1" } else { "0" })
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "跨进程索引验证失败：{status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            panic!("索引写锁必须非阻塞返回");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn independent_process_cannot_overwrite_a_locked_index_and_can_retry_after_unlock() {
    let (dir, _repo, _service) = init_repo();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("index.db");
    write_file(dir.path(), "lib.rs", "fn original() {}\n");
    run_index(dir.path(), &db, true, &mut options()).unwrap();
    let generation = index_generation(&db).unwrap();
    write_file(dir.path(), "lib.rs", "fn replacement() {}\n");
    let guard = database_process_lock(&db).unwrap();
    run_child(dir.path(), &db, true);
    assert_eq!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "original", 10).unwrap().len(), 1, "写锁不阻塞已有索引查询");
    drop(guard);
    run_child(dir.path(), &db, false);
    assert_ne!(index_generation(&db).unwrap(), generation);
    assert_eq!(search_symbols(&db, "replacement", 10).unwrap().len(), 1);
}
