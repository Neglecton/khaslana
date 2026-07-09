use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

use git2::Repository;
use serde::{Deserialize, Serialize};

use crate::{GitError, Result};

const IDEA_NOT_FOUND_MESSAGE: &str = "未找到 IntelliJ IDEA 命令，请确认 idea64 或 idea 已加入 PATH";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExternalMergeSettings {
    #[serde(default = "default_external_merge_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub auto_open_intellij: bool,
    #[serde(default)]
    pub intellij_path: String,
}

impl Default for ExternalMergeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_open_intellij: false,
            intellij_path: String::new(),
        }
    }
}

impl ExternalMergeSettings {
    pub fn normalized_intellij_path(&self) -> String {
        self.intellij_path.trim().to_string()
    }
}

fn default_external_merge_enabled() -> bool {
    true
}

pub fn resolve_intellij_idea_command() -> Result<PathBuf> {
    let env_path = std::env::var_os("KHASLANA_IDEA_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path_dirs = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    resolve_intellij_idea_command_from_env_and_path(env_path.as_deref(), &path_dirs)
}

pub fn resolve_intellij_idea_command_with_settings(
    settings: &ExternalMergeSettings,
) -> Result<PathBuf> {
    let env_path = std::env::var_os("KHASLANA_IDEA_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path_dirs = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    resolve_intellij_idea_command_for_settings(settings, env_path.as_deref(), &path_dirs)
}

pub fn run_intellij_idea_merge(repo: &Repository, path: &Path) -> Result<Vec<u8>> {
    let command = resolve_intellij_idea_command()?;
    run_intellij_idea_merge_with_command(repo, path, &command)
}

pub fn run_intellij_idea_merge_with_settings(
    repo: &Repository,
    path: &Path,
    settings: &ExternalMergeSettings,
) -> Result<Vec<u8>> {
    if !settings.enabled {
        return Err(GitError::Message("外部合并工具未启用".into()));
    }
    let command = resolve_intellij_idea_command_with_settings(settings)?;
    run_intellij_idea_merge_with_command(repo, path, &command)
}

pub(crate) fn resolve_intellij_idea_command_for_settings(
    settings: &ExternalMergeSettings,
    env_path: Option<&Path>,
    path_dirs: &[PathBuf],
) -> Result<PathBuf> {
    let configured = settings.normalized_intellij_path();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Ok(path);
        }
        return Err(GitError::Message(format!(
            "IntelliJ IDEA 命令不存在：{}",
            path.display()
        )));
    }
    resolve_intellij_idea_command_from_env_and_path(env_path, path_dirs)
}

pub(crate) fn resolve_intellij_idea_command_from_env_and_path(
    env_path: Option<&Path>,
    path_dirs: &[PathBuf],
) -> Result<PathBuf> {
    if let Some(path) = env_path {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(GitError::Message(format!(
            "IntelliJ IDEA 命令不存在：{}",
            path.display()
        )));
    }

    if let Some(command) = find_command_in_path("idea64", path_dirs) {
        return Ok(command);
    }
    if let Some(command) = find_command_in_path("idea", path_dirs) {
        return Ok(command);
    }
    if let Some(command) = find_common_intellij_idea_command() {
        return Ok(command);
    }

    Err(GitError::Message(IDEA_NOT_FOUND_MESSAGE.into()))
}

pub(crate) fn intellij_idea_merge_args(
    ours: &Path,
    theirs: &Path,
    base: &Path,
    result: &Path,
) -> Vec<OsString> {
    vec![
        OsString::from("merge"),
        ours.as_os_str().to_os_string(),
        theirs.as_os_str().to_os_string(),
        base.as_os_str().to_os_string(),
        result.as_os_str().to_os_string(),
    ]
}

pub(crate) fn run_intellij_idea_merge_with_command(
    repo: &Repository,
    path: &Path,
    command: &Path,
) -> Result<Vec<u8>> {
    ensure_worktree_relative_path(path, "不能使用 IntelliJ IDEA 解决冲突")?;

    let index = repo.index()?;
    let conflict = index.conflict_get(path).map_err(|err| {
        if err.code() == git2::ErrorCode::NotFound {
            GitError::Message(format!(
                "该文件不存在冲突：{}",
                path.components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            ))
        } else {
            GitError::Git(err)
        }
    })?;
    let ancestor = read_conflict_blob(repo, conflict.ancestor.as_ref())?;
    let ours = read_conflict_blob(repo, conflict.our.as_ref())?;
    let theirs = read_conflict_blob(repo, conflict.their.as_ref())?;

    let merge_dir = create_merge_temp_dir()?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("conflict");
    let base_path = merge_dir.join(format!("base-{file_name}"));
    let ours_path = merge_dir.join(format!("ours-{file_name}"));
    let theirs_path = merge_dir.join(format!("theirs-{file_name}"));
    let result_path = merge_dir.join(format!("result-{file_name}"));

    fs::write(&base_path, ancestor)?;
    fs::write(&ours_path, ours)?;
    fs::write(&theirs_path, theirs)?;

    let args = intellij_idea_merge_args(&ours_path, &theirs_path, &base_path, &result_path);
    let status = run_command(command, &args)
        .map_err(|err| GitError::Message(format!("无法启动外部合并工具：{err}")))?;
    if !status.success() {
        return Err(GitError::Message(format!(
            "IntelliJ IDEA 合并工具退出失败：{status}"
        )));
    }

    fs::read(&result_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            GitError::Message("IntelliJ IDEA 合并未生成结果文件".into())
        } else {
            GitError::Io(err)
        }
    })
}

fn read_conflict_blob(repo: &Repository, entry: Option<&git2::IndexEntry>) -> Result<Vec<u8>> {
    let Some(entry) = entry else {
        return Err(GitError::Message(
            "该冲突缺少 BASE/OURS/THEIRS，暂不能用 IntelliJ IDEA 三方合并".into(),
        ));
    };
    if entry.mode == 0 || entry.id.is_zero() {
        return Err(GitError::Message(
            "该冲突缺少 BASE/OURS/THEIRS，暂不能用 IntelliJ IDEA 三方合并".into(),
        ));
    }
    Ok(repo.find_blob(entry.id)?.content().to_vec())
}

fn ensure_worktree_relative_path(path: &Path, action: &str) -> Result<()> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Ok(());
    }

    Err(GitError::Message(format!("文件路径无效，{action}")))
}

fn create_merge_temp_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir()
        .join("khaslana-merge")
        .join(format!("{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn find_command_in_path(command: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    candidate_command_names(command)
        .into_iter()
        .find_map(|name| {
            path_dirs
                .iter()
                .map(|dir| dir.join(&name))
                .find(|path| path.is_file())
        })
}

fn candidate_command_names(command: &str) -> Vec<OsString> {
    #[cfg(windows)]
    {
        ["exe", "bat", "cmd", ""]
            .into_iter()
            .map(|ext| {
                if ext.is_empty() {
                    OsString::from(command)
                } else {
                    OsString::from(format!("{command}.{ext}"))
                }
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![OsString::from(command)]
    }
}

fn find_common_intellij_idea_command() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut roots = Vec::new();
        for key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(key) {
                roots.push(PathBuf::from(root).join("JetBrains"));
            }
        }
        for root in roots {
            if let Some(path) = find_idea64_under(&root, 3) {
                return Some(path);
            }
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let toolbox = PathBuf::from(local_app_data)
                .join("JetBrains")
                .join("Toolbox")
                .join("apps");
            return find_idea64_under(&toolbox, 8);
        }
        None
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn find_idea64_under(root: &Path, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 || !root.is_dir() {
        return None;
    }
    let direct = root.join("bin").join("idea64.exe");
    if direct.is_file() {
        return Some(direct);
    }
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = find_idea64_under(&path, max_depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn run_command(command: &Path, args: &[OsString]) -> std::io::Result<ExitStatus> {
    #[cfg(windows)]
    {
        if command
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
        {
            return Command::new("cmd")
                .arg("/C")
                .arg("call")
                .arg(command)
                .args(args)
                .status();
        }
    }

    Command::new(command).args(args).status()
}

#[cfg(test)]
#[path = "tests/external_merge.rs"]
mod tests;
