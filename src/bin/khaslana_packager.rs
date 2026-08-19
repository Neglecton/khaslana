// 本地一键打包器：`cargo setup` 别名驱动（见 .cargo/config.toml 的 [alias]）。
//
// 流程：release-perf 构建两个 exe（与官方发布同一 profile）-> 组装
// dist/package（与便携 zip 同一内容）-> 调 Inno Setup 编译安装器到 dist/。
// 版本号取编译期注入的 CARGO_PKG_VERSION，无需手填。
//
// 仅供本地出包；发布工作流仍显式分步执行（build/package/installer 各自
// 独立步骤），两边产物内容一致。纯 std 实现，避免给开发构建引入依赖。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if let Err(message) = run() {
        eprintln!("打包失败：{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // CARGO_MANIFEST_DIR 在编译期指向本仓库根，运行时不受工作目录影响。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");

    // 1. release-perf 构建。CARGO 环境变量由 cargo run 注入，指向当前
    //    使用的 cargo（对 rustup 多工具链友好），缺失时回退 PATH。
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(&cargo)
        .args([
            "build",
            "--profile",
            "release-perf",
            "--bin",
            "khaslana",
            "--bin",
            "khaslana_updater",
        ])
        .current_dir(&root)
        .status()
        .map_err(|err| format!("无法启动 cargo：{err}"))?;
    if !status.success() {
        return Err("release-perf 构建失败".into());
    }

    // 2. 组装 dist/package：先清旧目录，避免历史残留混入。
    let payload = root.join("dist").join("package");
    let _ = std::fs::remove_dir_all(&payload);
    std::fs::create_dir_all(&payload)
        .map_err(|err| format!("创建 {} 失败：{err}", payload.display()))?;
    let perf = root.join("target").join("release-perf");
    for name in ["khaslana.exe", "khaslana_updater.exe"] {
        copy_file(&perf.join(name), &payload.join(name))?;
    }
    for name in ["LICENSE", "README.md"] {
        let src = root.join(name);
        if src.exists() {
            copy_file(&src, &payload.join(name))?;
        }
    }

    // 3. 定位 ISCC 并编译安装器。
    let iscc = locate_iscc().ok_or_else(|| {
        "未找到 Inno Setup，请先安装：winget install JRSoftware.InnoSetup.7".to_string()
    })?;
    let iss = root.join("installer").join("khaslana.iss");
    let status = Command::new(&iscc)
        .arg(format!("/DAppVersion={version}"))
        .arg(&iss)
        .current_dir(&root)
        .status()
        .map_err(|err| format!("无法启动 ISCC（{}）：{err}", iscc.display()))?;
    if !status.success() {
        return Err("Inno Setup 编译失败".into());
    }

    let setup = root
        .join("dist")
        .join(format!("khaslana-setup-v{version}-windows-x86_64.exe"));
    let size_mb = std::fs::metadata(&setup).map(|m| m.len()).unwrap_or(0) / 1024 / 1024;
    println!("\n安装器已生成：{}（约 {size_mb} MB）", setup.display());
    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::copy(src, dst)
        .map_err(|err| format!("复制 {} 失败：{err}", src.display()))
        .map(|_| ())
}

/// 依次探测 Inno Setup 7（64 位默认路径）与旧版 6（x86 路径）。
fn locate_iscc() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files\Inno Setup 7\ISCC.exe",
        r"C:\Program Files (x86)\Inno Setup 7\ISCC.exe",
        r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}
