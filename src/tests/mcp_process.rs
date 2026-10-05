use super::*;

#[cfg(windows)]
#[test]
fn windows_command_resolves_cmd_without_selecting_ps1() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("npx.cmd"), "@echo off\r\n").unwrap();
    std::fs::write(directory.path().join("npx.ps1"), "throw 'wrong entry'").unwrap();
    assert_eq!(resolve_windows_command("npx", directory.path().as_os_str()),
        Some(directory.path().join("npx.cmd")));
    assert_eq!(resolve_windows_command("npx.cmd", directory.path().as_os_str()),
        Some(directory.path().join("npx.cmd")));
    assert!(resolve_windows_command("missing", directory.path().as_os_str()).is_none());
}

#[cfg(windows)]
#[test]
fn windows_command_prefers_executable_and_supports_explicit_path_with_spaces() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path().join("node with spaces");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("runner.cmd"), "").unwrap();
    std::fs::write(directory.join("runner.exe"), "").unwrap();
    assert_eq!(resolve_windows_command("runner", directory.as_os_str()),
        Some(directory.join("runner.exe")));
    let script = directory.join("runner.cmd");
    assert_eq!(resolve_windows_command(&script.to_string_lossy(), std::ffi::OsStr::new("")),
        Some(script));
}

#[test]
fn command_rejects_empty_or_nul_program() {
    assert!(command(" ", &[]).is_err());
    assert!(command("bad\0name", &[]).is_err());
}
