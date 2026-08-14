// 分支浏览模式的 Git 服务：只读地解析分支/标签引用、遍历文件树、读取文件内容、
// 计算与当前 HEAD 的差异。所有操作均不修改 index/worktree。

use std::path::Path;

use bstr::ByteSlice;
use git2::{DiffFindOptions, DiffOptions, ObjectType, Repository};

use crate::{
    GitService,
    types::{
        BrowseCompareFile, BrowseEntry, BrowseEntryKind, BrowseFileContent, BrowseTarget,
        DiffEncodingChoice, DiffEncodingInfo, DiffScope, FileDiff, GitError, Result,
    },
};

/// 浏览目标引用的种类，决定 `resolve_browse_target` 的解析路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseRefKind {
    /// 本地分支。
    LocalBranch,
    /// 远端分支（名称形如 origin/feature）。
    RemoteBranch,
    /// 标签。
    Tag,
}

impl GitService {
    /// 解析浏览目标引用，返回显示名与 peel 到 commit 的 tip OID。
    ///
    /// 分支复用现有 find_branch_reference（本地/远端），标签使用 revparse_single。
    /// 引用不存在时返回中文错误。
    pub fn resolve_browse_target(
        &self,
        repo: &Repository,
        name: &str,
        kind: BrowseRefKind,
    ) -> Result<BrowseTarget> {
        let commit = match kind {
            BrowseRefKind::LocalBranch | BrowseRefKind::RemoteBranch => {
                let reference = self.find_branch_reference(repo, name)?;
                let peeled = reference
                    .peel(ObjectType::Commit)
                    .map_err(|err| GitError::Message(format!("无法解析引用 {name}: {err}")))?;
                peeled
                    .into_commit()
                    .map_err(|_| GitError::Message(format!("{name} 不是提交")))?
            }
            BrowseRefKind::Tag => {
                let object = repo
                    .revparse_single(&format!("refs/tags/{name}"))
                    .map_err(|_| GitError::Message(format!("标签 {name} 不存在")))?;
                object
                    .peel_to_commit()
                    .map_err(|_| GitError::Message(format!("标签 {name} 不指向提交")))?
            }
        };
        let commit_oid = commit.id().to_string();
        Ok(BrowseTarget {
            display_name: name.to_string(),
            commit_oid,
        })
    }

    /// 列出指定提交某个目录下的直接子条目。
    ///
    /// prefix 为 None 时返回仓库根目录；为 Some("src") 时返回 src/ 的直接子项。
    /// 结果按「目录在前、文件在后，各自按名称排序」排列。
    pub fn browse_tree_entries(
        &self,
        repo: &Repository,
        commit_oid: &str,
        prefix: Option<&Path>,
    ) -> Result<Vec<BrowseEntry>> {
        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let root_tree = commit.tree()?;
        // 根据前缀定位子树
        let target_tree = match prefix {
            Some(path) if !path.as_os_str().is_empty() => {
                let entry = root_tree
                    .get_path(path)
                    .map_err(|_| GitError::Message(format!("路径不存在: {}", path.display())))?;
                let object = entry.to_object(repo)?;
                object
                    .into_tree()
                    .map_err(|_| GitError::Message("该路径不是目录".to_string()))?
            }
            _ => root_tree,
        };

        let mut entries = Vec::with_capacity(target_tree.len());
        for entry in target_tree.iter() {
            let name = entry.name().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let kind = match entry.kind() {
                Some(ObjectType::Tree) => BrowseEntryKind::Directory,
                Some(ObjectType::Commit) => BrowseEntryKind::Submodule,
                _ => BrowseEntryKind::File,
            };
            // 构造完整 git 风格相对路径
            let path = match prefix {
                Some(base) if !base.as_os_str().is_empty() => {
                    let mut full = super::path_to_git(base);
                    full.push('/');
                    full.push_str(&name);
                    full
                }
                _ => name.clone(),
            };
            // 文件取 blob 字节数；目录/子模块填 0
            let size = if kind == BrowseEntryKind::File {
                entry
                    .to_object(repo)
                    .ok()
                    .and_then(|object| object.into_blob().ok())
                    .map(|blob| blob.size() as u64)
                    .unwrap_or(0)
            } else {
                0
            };
            entries.push(BrowseEntry {
                path,
                name,
                kind,
                size,
            });
        }
        // 目录在前、文件在后；各自按名称排序
        entries.sort_by(|a, b| {
            let a_dir = matches!(a.kind, BrowseEntryKind::Directory);
            let b_dir = matches!(b.kind, BrowseEntryKind::Directory);
            b_dir.cmp(&a_dir).then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// 读取指定提交中某文件的只读内容。
    ///
    /// 二进制检测取前若干 KB 是否含 NUL 字节；文本按选定/检测编码解码后按行切分。
    /// 字节量超过 FULL_FILE_MAX_BYTES 直接返回错误，UI 据此提示。
    pub fn browse_file_content(
        &self,
        repo: &Repository,
        commit_oid: &str,
        path: &Path,
        encoding: DiffEncodingChoice,
    ) -> Result<BrowseFileContent> {
        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let tree = commit.tree()?;
        let entry = tree
            .get_path(path)
            .map_err(|_| GitError::Message(format!("文件不存在: {}", path.display())))?;

        // 大文件保护：先读 ODB 对象头取体积（不加载内容），超大文件直接报错，
        // 避免几百 MB 的 blob 为了一次判断先整体 inflate 到堆。
        if let Ok(odb) = repo.odb()
            && let Ok((size, _)) = odb.read_header(entry.id())
            && size as u64 > super::FULL_FILE_MAX_BYTES
        {
            return Err(GitError::Message(
                super::FULL_FILE_TOO_LARGE_MESSAGE.to_string(),
            ));
        }

        let blob = entry
            .to_object(repo)?
            .into_blob()
            .map_err(|_| GitError::Message("该路径不是文件".to_string()))?;
        let content = blob.content();

        // 大文件保护：超过阈值直接报错
        if content.len() as u64 > super::FULL_FILE_MAX_BYTES {
            return Err(GitError::Message(
                super::FULL_FILE_TOO_LARGE_MESSAGE.to_string(),
            ));
        }

        // 二进制检测：前 8KB 含 NUL 字节视为二进制
        let sample_for_binary = &content[..content.len().min(8 * 1024)];
        let is_binary = sample_for_binary.find_byte(0).is_some();

        if is_binary {
            return Ok(BrowseFileContent {
                path: super::path_to_git(path),
                is_binary: true,
                encoding: DiffEncodingInfo {
                    requested: encoding,
                    resolved: DiffEncodingChoice::Utf8,
                    lossy: false,
                },
                lines: Vec::new(),
            });
        }

        // 文本：有限字节样本选编码，整体解码后按行切分
        let sample_len = content.len().min(super::DIFF_ENCODING_SAMPLE_LIMIT);
        let (resolved_encoding, encoding_impl) =
            super::resolve_diff_encoding(encoding, &content[..sample_len]);
        let (decoded, _used, had_errors) = encoding_impl.decode(content);
        let lines = decoded
            .lines()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        Ok(BrowseFileContent {
            path: super::path_to_git(path),
            is_binary: false,
            encoding: DiffEncodingInfo {
                requested: encoding,
                resolved: resolved_encoding,
                lossy: had_errors,
            },
            lines,
        })
    }

    /// 列出目标分支领先当前 HEAD 的提交所改动的文件（三点比较）。
    ///
    /// 以 merge_base(HEAD, 目标) 的树作为 old、目标分支树作为 new，即 `merge_base..target`，
    /// 因此只展示选定分支领先当前分支的提交，当前分支独有的改动不会进入列表。
    /// Added 表示目标分支相对共同祖先新增的文件，Deleted 表示目标分支相对共同祖先删除的文件。
    /// 若无法计算共同祖先（无 HEAD 或无关历史），降级为 HEAD 树，避免报错。
    pub fn browse_compare_files(
        &self,
        repo: &Repository,
        commit_oid: &str,
    ) -> Result<Vec<BrowseCompareFile>> {
        let target_commit = self.find_commit_by_oid(repo, commit_oid)?;
        let target_tree = target_commit.tree()?;
        let base_tree = compare_base_tree(repo, &target_commit);

        let mut options = DiffOptions::new();
        let mut diff =
            repo.diff_tree_to_tree(base_tree.as_ref(), Some(&target_tree), Some(&mut options))?;
        // 分支比较列表启用基础重命名识别，便于左侧用 R 展示重命名而非一增一删。
        let mut find_options = DiffFindOptions::new();
        find_options.renames(true).rename_threshold(50);
        diff.find_similar(Some(&mut find_options))?;

        let mut files = Vec::new();
        for delta in diff.deltas() {
            let status = super::change_state_from_delta(delta.status());
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(super::path_to_git);
            let Some(path) = path else {
                continue;
            };
            let old_path = delta.old_file().path().map(super::path_to_git);
            files.push(BrowseCompareFile {
                path,
                old_path,
                status,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// 计算目标分支文件与当前 HEAD 之间的差异。
    ///
    /// 方向锁定为 old=HEAD、new=目标分支，差异里高亮的是被浏览分支相对当前分支的内容。
    /// 复用 guard_full_file_size + file_diff_from_diff。
    pub fn browse_file_diff(
        &self,
        repo: &Repository,
        commit_oid: &str,
        path: &Path,
        full_context: bool,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        self.browse_file_diff_for_compare(repo, commit_oid, path, None, full_context, encoding)
    }

    /// 计算分支比较列表中某个文件的差异（三点比较，与 browse_compare_files 同向）。
    ///
    /// old 侧取 merge_base(HEAD, 目标) 的树，仅展示目标分支领先当前分支引入的改动；
    /// 对重命名文件同时传入旧路径和新路径，避免 pathspec 只命中新路径时丢失重命名信息。
    pub fn browse_file_diff_for_compare(
        &self,
        repo: &Repository,
        commit_oid: &str,
        path: &Path,
        old_path: Option<&Path>,
        full_context: bool,
        encoding: DiffEncodingChoice,
    ) -> Result<FileDiff> {
        let target_commit = self.find_commit_by_oid(repo, commit_oid)?;
        let target_tree = target_commit.tree()?;
        let base_tree = compare_base_tree(repo, &target_commit);

        let mut options = DiffOptions::new();
        options.context_lines(super::diff_context_lines(full_context));
        // 重命名文件同时限定旧路径和新路径；普通文件只限定当前路径。
        if let Some(old_path) = old_path.filter(|old| *old != path) {
            options.pathspec(old_path).pathspec(path);
        } else {
            options.pathspec(path);
        }

        let diff =
            repo.diff_tree_to_tree(base_tree.as_ref(), Some(&target_tree), Some(&mut options))?;

        super::guard_full_file_size(repo, &diff, full_context)?;
        self.file_diff_from_diff(
            repo,
            diff,
            super::path_to_git(path),
            DiffScope::Staged,
            encoding,
        )
    }
}

/// 计算分支比较的 old 侧树：merge_base(HEAD, 目标) 的树，实现三点比较。
///
/// 无法计算共同祖先时（无 HEAD 或无关历史）降级为 HEAD 树，避免报错。
fn compare_base_tree<'r>(
    repo: &'r Repository,
    target_commit: &git2::Commit<'r>,
) -> Option<git2::Tree<'r>> {
    let head_commit = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    head_commit
        .as_ref()
        .and_then(|head| {
            repo.merge_base(head.id(), target_commit.id())
                .ok()
                .and_then(|oid| repo.find_commit(oid).ok())
                .and_then(|base| base.tree().ok())
        })
        .or_else(|| head_commit.as_ref().and_then(|head| head.tree().ok()))
}

#[cfg(test)]
#[path = "../tests/git/browse.rs"]
mod tests;
