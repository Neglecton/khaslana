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

    /// 列出目标分支相对当前 HEAD 有差异的文件。
    ///
    /// 方向与 browse_file_diff 保持一致：old=HEAD、new=目标分支，因此 Added 表示
    /// 文件存在于目标分支但当前分支没有，Deleted 表示目标分支删除了当前分支文件。
    pub fn browse_compare_files(
        &self,
        repo: &Repository,
        commit_oid: &str,
    ) -> Result<Vec<BrowseCompareFile>> {
        let target_commit = self.find_commit_by_oid(repo, commit_oid)?;
        let target_tree = target_commit.tree()?;
        let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());

        let mut options = DiffOptions::new();
        let mut diff =
            repo.diff_tree_to_tree(head_tree.as_ref(), Some(&target_tree), Some(&mut options))?;
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

    /// 计算分支比较列表中某个文件的差异。
    ///
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
        let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());

        let mut options = DiffOptions::new();
        options.context_lines(super::diff_context_lines(full_context));
        // 重命名文件同时限定旧路径和新路径；普通文件只限定当前路径。
        if let Some(old_path) = old_path.filter(|old| *old != path) {
            options.pathspec(old_path).pathspec(path);
        } else {
            options.pathspec(path);
        }

        let diff =
            repo.diff_tree_to_tree(head_tree.as_ref(), Some(&target_tree), Some(&mut options))?;

        super::guard_full_file_size(&diff, full_context)?;
        self.file_diff_from_diff(diff, super::path_to_git(path), DiffScope::Staged, encoding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::PromptCredentialProvider;
    use crate::types::{BranchName, ChangeState, CommitMessage, DiffLineKind};
    use git2::{Oid, RepositoryInitOptions};
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn service() -> GitService {
        GitService::new(
            Arc::new(PromptCredentialProvider::memory_only(|_| Ok(None))),
            Arc::new(crate::git::NoopProgress),
        )
    }

    fn init_repo() -> (TempDir, git2::Repository, GitService) {
        let dir = TempDir::new().unwrap();
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = git2::Repository::init_opts(dir.path(), &options).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config
            .set_str("user.email", "test@example.invalid")
            .unwrap();
        (dir, repo, service())
    }

    fn write_file(root: &Path, path: &str, body: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, body).unwrap();
    }

    fn stage_and_commit(repo: &mut git2::Repository, svc: &GitService, message: &str) -> Oid {
        svc.stage_path(repo, Path::new(".")).unwrap();
        svc.commit(repo, &CommitMessage::new(message)).unwrap();
        repo.head().unwrap().target().unwrap()
    }

    // 构建测试仓库：main 分支有 src/lib.rs + README.md，feature 分支修改 lib.rs 并新增 new.rs。
    fn build_repo() -> (TempDir, GitService) {
        let (dir, mut repo, svc) = init_repo();
        write_file(dir.path(), "src/types/mod.rs", "// types\n");
        write_file(dir.path(), "src/lib.rs", "pub fn a() -> i32 { 1 }\n");
        write_file(dir.path(), "README.md", "# main\n");
        stage_and_commit(&mut repo, &svc, "init");

        svc.create_branch_from(
            &mut repo,
            &BranchName::new("feature"),
            Some(&BranchName::new("main")),
            false,
        )
        .unwrap();
        svc.checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        write_file(dir.path(), "src/lib.rs", "pub fn a() -> i32 { 2 }\n");
        write_file(dir.path(), "src/new.rs", "pub fn b() -> i32 { 3 }\n");
        stage_and_commit(&mut repo, &svc, "feature change");
        svc.checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        drop(repo);
        (dir, svc)
    }

    #[test]
    fn resolve_browse_target_local_branch() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();

        let main = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();
        assert_eq!(main.display_name, "main");
        assert!(!main.commit_oid.is_empty());

        let feature = svc
            .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
            .unwrap();
        assert_ne!(main.commit_oid, feature.commit_oid);
    }

    #[test]
    fn resolve_browse_target_tag() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let head_oid = repo.head().unwrap().target().unwrap();
        repo.reference("refs/tags/v1.0", head_oid, false, "test tag")
            .unwrap();

        let target = svc
            .resolve_browse_target(&repo, "v1.0", BrowseRefKind::Tag)
            .unwrap();
        assert_eq!(target.display_name, "v1.0");
        assert_eq!(target.commit_oid, head_oid.to_string());
    }

    #[test]
    fn resolve_browse_target_missing_returns_error() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        assert!(
            svc.resolve_browse_target(&repo, "nonexistent", BrowseRefKind::LocalBranch)
                .is_err()
        );
        assert!(
            svc.resolve_browse_target(&repo, "ghost", BrowseRefKind::Tag)
                .is_err()
        );
    }

    #[test]
    fn browse_tree_entries_root_directories_first() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();

        let entries = svc
            .browse_tree_entries(&repo, &target.commit_oid, None)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "src");
        assert_eq!(entries[0].kind, BrowseEntryKind::Directory);
        assert_eq!(entries[1].name, "README.md");
        assert_eq!(entries[1].kind, BrowseEntryKind::File);
    }

    #[test]
    fn browse_tree_entries_subdirectory() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();

        let entries = svc
            .browse_tree_entries(&repo, &target.commit_oid, Some(Path::new("src")))
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "types");
        assert_eq!(entries[0].kind, BrowseEntryKind::Directory);
        assert_eq!(entries[0].path, "src/types");
        assert_eq!(entries[1].name, "lib.rs");
        assert_eq!(entries[1].path, "src/lib.rs");
    }

    #[test]
    fn browse_file_content_text_decodes() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();

        let content = svc
            .browse_file_content(
                &repo,
                &target.commit_oid,
                Path::new("src/lib.rs"),
                DiffEncodingChoice::Utf8,
            )
            .unwrap();
        assert!(!content.is_binary);
        assert_eq!(content.lines, vec!["pub fn a() -> i32 { 1 }"]);
    }

    #[test]
    fn browse_file_content_binary_detected() {
        let (dir, svc) = build_repo();
        let repo_path = dir.path();
        let mut repo = git2::Repository::open(repo_path).unwrap();
        fs::write(repo_path.join("blob.bin"), [0u8, 1, 2, 0, 255, 0]).unwrap();
        svc.stage_path(&mut repo, Path::new("blob.bin")).unwrap();
        svc.commit(&mut repo, &CommitMessage::new("add binary"))
            .unwrap();
        drop(repo);

        let repo = git2::Repository::open(repo_path).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();
        let content = svc
            .browse_file_content(
                &repo,
                &target.commit_oid,
                Path::new("blob.bin"),
                DiffEncodingChoice::Utf8,
            )
            .unwrap();
        assert!(content.is_binary);
        assert!(content.lines.is_empty());
    }

    #[test]
    fn browse_file_diff_shows_changes() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();

        let feature = svc
            .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
            .unwrap();
        let diff = svc
            .browse_file_diff(
                &repo,
                &feature.commit_oid,
                Path::new("src/lib.rs"),
                false,
                DiffEncodingChoice::Utf8,
            )
            .unwrap();
        assert!(
            diff.lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Removed && l.content.contains("{ 1 }"))
        );
        assert!(
            diff.lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Added && l.content.contains("{ 2 }"))
        );
    }

    #[test]
    fn browse_file_diff_same_branch_empty() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();
        let diff = svc
            .browse_file_diff(
                &repo,
                &target.commit_oid,
                Path::new("README.md"),
                false,
                DiffEncodingChoice::Utf8,
            )
            .unwrap();
        assert!(diff.lines.is_empty());
    }

    #[test]
    fn browse_tree_entries_missing_path_errors() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();
        assert!(
            svc.browse_tree_entries(&repo, &target.commit_oid, Some(Path::new("nonexistent")))
                .is_err()
        );
    }

    #[test]
    fn browse_compare_files_only_changed_paths() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let feature = svc
            .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
            .unwrap();

        let files = svc
            .browse_compare_files(&repo, &feature.commit_oid)
            .unwrap();
        let paths = files
            .iter()
            .map(|file| (file.path.as_str(), file.status.clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                ("src/lib.rs", ChangeState::Modified),
                ("src/new.rs", ChangeState::Added),
            ]
        );
        assert!(!files.iter().any(|file| file.path == "README.md"));
    }

    #[test]
    fn browse_compare_files_empty_for_same_branch() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let main = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();

        let files = svc.browse_compare_files(&repo, &main.commit_oid).unwrap();

        assert!(files.is_empty());
    }

    #[test]
    fn browse_compare_files_reports_deleted_files() {
        let (dir, svc) = build_repo();
        let repo_path = dir.path();
        let mut repo = git2::Repository::open(repo_path).unwrap();
        svc.checkout_branch(&mut repo, &BranchName::new("feature"))
            .unwrap();
        fs::remove_file(repo_path.join("README.md")).unwrap();
        svc.stage_path(&mut repo, Path::new("README.md")).unwrap();
        svc.commit(&mut repo, &CommitMessage::new("delete readme"))
            .unwrap();
        svc.checkout_branch(&mut repo, &BranchName::new("main"))
            .unwrap();
        drop(repo);

        let repo = git2::Repository::open(repo_path).unwrap();
        let feature = svc
            .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
            .unwrap();
        let files = svc
            .browse_compare_files(&repo, &feature.commit_oid)
            .unwrap();

        let deleted = files
            .iter()
            .find(|file| file.path == "README.md")
            .expect("deleted file should be listed");
        assert_eq!(deleted.status, ChangeState::Deleted);
    }

    #[test]
    fn browse_compare_file_diff_keeps_head_to_target_direction() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let feature = svc
            .resolve_browse_target(&repo, "feature", BrowseRefKind::LocalBranch)
            .unwrap();

        let diff = svc
            .browse_file_diff_for_compare(
                &repo,
                &feature.commit_oid,
                Path::new("src/lib.rs"),
                None,
                false,
                DiffEncodingChoice::Utf8,
            )
            .unwrap();

        assert!(
            diff.lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Removed && line.content.contains("{ 1 }"))
        );
        assert!(
            diff.lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.content.contains("{ 2 }"))
        );
    }

    #[test]
    fn browse_file_content_missing_path_errors() {
        let (dir, svc) = build_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let target = svc
            .resolve_browse_target(&repo, "main", BrowseRefKind::LocalBranch)
            .unwrap();
        assert!(
            svc.browse_file_content(
                &repo,
                &target.commit_oid,
                Path::new("nope.rs"),
                DiffEncodingChoice::Utf8,
            )
            .is_err()
        );
    }
}
