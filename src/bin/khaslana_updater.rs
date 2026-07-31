//! Khaslana 自动更新器。
//!
//! 等待主进程退出后替换 exe 文件，并可选重启应用。
//!
//! 参数：
//!   --pid <PID>          主进程 PID，等待其退出
//!   --target-exe <PATH>  目标 khaslana.exe 路径
//!   --new-exe <PATH>     新 khaslana.exe 路径（staging 目录中）
//!   --new-updater <PATH> 新 khaslana_updater.exe 路径
//!   --backup-dir <PATH>  备份目录
//!   --restart            替换成功后重启应用

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    if let Err(err) = run() {
        eprintln!("更新失败：{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    let pid = arg_value(&args, "--pid").ok_or("缺少 --pid 参数")?;
    let target_exe =
        PathBuf::from(arg_value(&args, "--target-exe").ok_or("缺少 --target-exe 参数")?);
    let new_exe = PathBuf::from(arg_value(&args, "--new-exe").ok_or("缺少 --new-exe 参数")?);
    let new_updater =
        PathBuf::from(arg_value(&args, "--new-updater").ok_or("缺少 --new-updater 参数")?);
    let backup_dir =
        PathBuf::from(arg_value(&args, "--backup-dir").ok_or("缺少 --backup-dir 参数")?);
    let restart = args.iter().any(|a| a == "--restart");

    // 推断同目录下的 khaslana_updater.exe
    let target_updater = target_exe.with_file_name("khaslana_updater.exe");

    // 1. 等待父进程退出
    let pid: u32 = pid.parse()?;
    wait_for_process_exit(pid, 30)?;

    // 2. 创建备份目录
    fs::create_dir_all(&backup_dir)?;

    // 3. 备份当前 exe
    let backup_exe = backup_dir.join("khaslana.exe.bak");
    let backup_updater = backup_dir.join("khaslana_updater.exe.bak");

    if target_exe.exists() {
        fs::copy(&target_exe, &backup_exe)?;
    }
    if target_updater.exists() {
        fs::copy(&target_updater, &backup_updater)?;
    }

    // 4. 替换 exe 文件
    if let Err(e) = replace_file(&new_exe, &target_exe) {
        // 恢复备份
        let _ = fs::copy(&backup_exe, &target_exe);
        let _ = fs::copy(&backup_updater, &target_updater);
        return Err(format!("替换 khaslana.exe 失败：{e}").into());
    }

    if let Err(e) = replace_file(&new_updater, &target_updater) {
        // 恢复备份
        let _ = fs::copy(&backup_exe, &target_exe);
        let _ = fs::copy(&backup_updater, &target_updater);
        return Err(format!("替换 khaslana_updater.exe 失败：{e}").into());
    }

    // 5. 清理备份和 staging
    let _ = fs::remove_dir_all(&backup_dir);

    // 6. 可选重启
    if restart {
        Command::new(&target_exe).spawn()?;
    }

    Ok(())
}

/// 替换文件：先尝试直接 copy；失败时 rename-and-replace fallback。
///
/// Windows 上如果目标文件被短暂锁定，rename 旧文件后 copy 新文件可以成功。
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::copy(src, dst) {
        Ok(_) => Ok(()),
        Err(_) => {
            // fallback: rename old → .old, then copy new
            let old = dst.with_extension("old");
            let _ = fs::remove_file(&old);
            fs::rename(dst, &old)?;
            let result = fs::copy(src, dst);
            let _ = fs::remove_file(&old); // 清理旧文件
            result.map(|_| ())
        }
    }
}

/// 等待指定 PID 的进程退出。
///
/// 在 Windows 上通过 `tasklist` 命令检查进程是否仍存在。
/// 最多等待 `max_seconds` 秒。
fn wait_for_process_exit(pid: u32, max_seconds: u64) -> Result<(), Box<dyn std::error::Error>> {
    let start = std::time::Instant::now();
    loop {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // tasklist 输出中如果进程不存在，会显示 "INFO: No tasks are running..."
        // 如果存在，会显示进程名和 PID
        if !stdout.contains(&pid.to_string()) {
            return Ok(());
        }

        if start.elapsed().as_secs() >= max_seconds {
            return Err(format!("等待进程 {pid} 退出超时（{max_seconds}秒）").into());
        }

        thread::sleep(Duration::from_millis(500));
    }
}

/// 从命令行参数中提取指定名称的值。
fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
