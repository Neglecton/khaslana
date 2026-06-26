use std::path::Path;
use std::process::Command;

/// 使用系统文件管理器打开目录，避免各处重复拼接平台命令。
pub(crate) fn open_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        Command::new("explorer.exe")
            .arg(windows_explorer_directory_arg(&path))
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_explorer_directory_arg(path: &Path) -> String {
    format!("/e,{}", windows_native_path(path))
}

#[cfg(target_os = "windows")]
fn windows_native_path(path: &Path) -> String {
    let mut path = path.to_string_lossy().into_owned();
    if let Some(stripped) = path.strip_prefix(r"\\?\UNC\") {
        path = format!(r"\\{}", stripped);
    } else if let Some(stripped) = path.strip_prefix(r"\\?\") {
        path = stripped.to_string();
    }

    // explorer.exe 会把正斜杠当作命令开关解析，转成 Windows 原生分隔符后再传入。
    path.replace('/', "\\")
}

#[cfg(test)]
#[path = "tests/system.rs"]
mod tests;
