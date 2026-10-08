//! MCP 测试与工作流共用的进程启动器；参数始终按独立参数传递。

use std::path::PathBuf;
#[cfg(windows)]
use std::path::Path;
use crate::{GitError, Result};

#[cfg(windows)]
fn resolve_windows_command(command: &str, search_path: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = Path::new(command);
    let explicit = path.is_absolute() || path.components().count() > 1;
    let directories: Vec<PathBuf> = if explicit {
        vec![PathBuf::new()]
    } else {
        std::env::split_paths(search_path).filter(|dir| !dir.as_os_str().is_empty()).collect()
    };
    for directory in directories {
        let candidate = directory.join(path);
        if candidate.extension().is_some() {
            if candidate.is_file() { return Some(candidate); }
        } else {
            // 不选择 PowerShell 脚本：GUI 不经过 PowerShell，npm 的 stdio 入口是 .cmd。
            for extension in ["exe", "com", "cmd", "bat"] {
                let candidate = candidate.with_extension(extension);
                if candidate.is_file() { return Some(candidate); }
            }
        }
    }
    None
}

pub(super) fn command(command: &str, args: &[String]) -> Result<tokio::process::Command> {
    let command = command.trim();
    if command.is_empty() || command.contains('\0') {
        return Err(GitError::Message("请填写有效的 MCP 启动命令".into()));
    }
    #[cfg(windows)]
    let program = resolve_windows_command(command, &std::env::var_os("PATH").unwrap_or_default())
        .ok_or_else(|| GitError::Message(format!(
            "找不到 MCP 启动命令 {command}；请确认已安装并加入 PATH，或填写完整路径。使用 npx 需安装 Node.js（含 npm），安装后重启 Khaslana。")))?;
    #[cfg(not(windows))]
    let program = PathBuf::from(command);

    let mut process = std::process::Command::new(&program);
    // npm 标准入口直接交给同目录的 Node，避免批处理的额外 shell 与参数再解析。
    #[cfg(windows)]
    if program.file_stem().and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("npx") || name.eq_ignore_ascii_case("npm"))
        && program.extension().and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bat")) {
        let directory = program.parent().unwrap_or_else(|| Path::new("."));
        let node = directory.join("node.exe");
        let cli = directory.join("node_modules/npm/bin")
            .join(format!("{}-cli.js", program.file_stem().unwrap().to_string_lossy().to_ascii_lowercase()));
        if node.is_file() && cli.is_file() {
            process = std::process::Command::new(node);
            process.arg(cli);
        }
    }
    process.args(args);
    // 与其它 MCP 客户端隔离 npm 下载缓存，仍尊重用户显式指定的缓存目录。
    if program.file_stem().and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("npx"))
        && std::env::var_os("npm_config_cache").is_none()
        && let Some(data_dir) = crate::storage::active_data_dir() {
        process.env("npm_config_cache", data_dir.join("workflow-npm-cache"));
    }
    crate::process::hide_console(&mut process);
    let mut process = tokio::process::Command::from(process);
    process.kill_on_drop(true);
    Ok(process)
}

#[cfg(test)]
#[path = "../tests/mcp_process.rs"]
mod tests;
