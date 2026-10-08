//! GUI 后台子进程共用的 Windows 控制台策略。

pub(crate) fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW：后台检查和 stdio 不弹终端。
    }
    #[cfg(not(windows))]
    let _ = command;
}
