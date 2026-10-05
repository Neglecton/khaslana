use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use git2::{FetchOptions, ObjectType, Repository};

use crate::{GitError, GitService, Result, parse_workflow_json5};

pub const CNB_TEMPLATE_REPO: &str = "https://cnb.cool/liuchenchen/work-studio.git";
/// 旧版批量同步留下的目录，只用于一次性导入，新的下载直接放到模板根目录。
pub const MANAGED_TEMPLATE_DIR: &str = "remote-cnb";
const CACHE_DIR: &str = ".remote-cnb-cache";
const MAX_TEMPLATES: usize = 100;
const MAX_TEMPLATE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RemoteTemplateEntry {
    pub file_name: String,
    pub display_name: String,
    pub commit: String,
    content: Vec<u8>,
}

/// 只读取远端目录与内容到缓存，不安装任何模板。
pub fn list_cnb_templates(
    service: &GitService,
    template_root: &Path,
) -> Result<Vec<RemoteTemplateEntry>> {
    list_templates_from_url(service, template_root, CNB_TEMPLATE_REPO)
}

fn list_templates_from_url(
    service: &GitService,
    template_root: &Path,
    source_url: &str,
) -> Result<Vec<RemoteTemplateEntry>> {
    fs::create_dir_all(template_root)?;
    let cache = template_root.join(CACHE_DIR);
    let repo = if cache.exists() {
        Repository::open_bare(&cache)?
    } else {
        Repository::init_bare(&cache)?
    };
    let mut remote = repo.remote_anonymous(source_url)?;
    let mut fetch = FetchOptions::new();
    fetch.remote_callbacks(service.remote_callbacks(None));
    service.apply_fetch_proxy(&mut fetch, Some(source_url))?;
    remote.fetch(
        &["+refs/heads/main:refs/remotes/origin/main"],
        Some(&mut fetch),
        None,
    )?;
    let commit = repo
        .find_reference("refs/remotes/origin/main")?
        .peel_to_commit()?;
    let tree = commit
        .tree()?
        .get_path(Path::new("work-studio-files"))?
        .to_object(&repo)?
        .peel_to_tree()?;

    let mut entries = Vec::new();
    let mut total_bytes = 0;
    for entry in tree.iter() {
        if entry.filemode() != 0o100644 {
            continue;
        }
        let Ok(name) = entry.name() else { continue };
        if !valid_template_name(name) {
            continue;
        }
        if entries.len() >= MAX_TEMPLATES {
            return Err(GitError::Message("远端工作流模板数量超过限制".into()));
        }
        let object = entry.to_object(&repo)?;
        if object.kind() != Some(ObjectType::Blob) {
            continue;
        }
        let blob = object.peel_to_blob()?;
        if blob.size() > MAX_TEMPLATE_BYTES {
            return Err(GitError::Message(format!("远端工作流模板过大：{name}")));
        }
        total_bytes += blob.size();
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(GitError::Message("远端工作流模板总大小超过限制".into()));
        }
        let content = std::str::from_utf8(blob.content())
            .map_err(|_| GitError::Message(format!("远端工作流模板不是 UTF-8：{name}")))?;
        let definition = parse_workflow_json5(content)?;
        entries.push(RemoteTemplateEntry {
            file_name: name.to_string(),
            display_name: definition.display_name(),
            commit: commit.id().to_string(),
            content: blob.content().to_vec(),
        });
    }
    if entries.is_empty() {
        return Err(GitError::Message("远端目录没有可用的工作流模板".into()));
    }
    entries.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
    Ok(entries)
}

/// 只安装用户选中的一项；同名文件存在时保留本地文件并返回错误。
pub fn download_cnb_template(
    template_root: &Path,
    template: &RemoteTemplateEntry,
) -> Result<PathBuf> {
    if !valid_template_name(&template.file_name) {
        return Err(GitError::Message("远端工作流模板文件名无效".into()));
    }
    let content = std::str::from_utf8(&template.content)
        .map_err(|_| GitError::Message("远端工作流模板不是 UTF-8".into()))?;
    parse_workflow_json5(content)?;
    fs::create_dir_all(template_root)?;
    let target = template_root.join(&template.file_name);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&target).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            GitError::Message(format!("本地已存在同名模板：{}", template.file_name))
        } else {
            err.into()
        }
    })?;
    if let Err(err) = file.write_all(&template.content).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&target);
        return Err(err.into());
    }
    Ok(target)
}

fn valid_template_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.ends_with(['.', ' '])
        && Path::new(name).extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("json5") || ext.eq_ignore_ascii_case("jsonc")
        })
}

#[cfg(test)]
#[path = "../tests/workflow_remote_templates.rs"]
mod tests;
