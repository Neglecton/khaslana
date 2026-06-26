/// Git 测试共享辅助模块。
///
/// 提供跨文件复用的 fixture 构建函数（`init_repo`、`service`、`commit_all` 等），
/// 供 `git.rs`、`git/browse.rs`、`git/conflicts.rs`、`git/rebase.rs`、`workflow.rs`
/// 等模块的单元测试引用，消除各 `mod tests` 中的重复定义。
#[cfg(test)]
pub(crate) mod git_test_support {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use git2::{IndexAddOption, Oid, Repository, RepositoryInitOptions};
    use tempfile::TempDir;

    use super::super::{GitService, parents, signature};
    use crate::credentials::PromptCredentialProvider;
    use crate::git::NoopProgress;

    /// 创建使用内存凭据提供者的 GitService。
    pub fn service() -> GitService {
        GitService::new(
            Arc::new(PromptCredentialProvider::memory_only(|_| Ok(None))),
            Arc::new(NoopProgress),
        )
    }

    /// 初始化临时 Git 仓库，设置 main 为初始分支并配置测试用户。
    pub fn init_repo() -> (TempDir, Repository, GitService) {
        let dir = TempDir::new().unwrap();
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = Repository::init_opts(dir.path(), &options).unwrap();
        configure_user(&repo);
        (dir, repo, service())
    }

    /// 为仓库设置测试用的 user.name 和 user.email。
    pub fn configure_user(repo: &Repository) {
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config
            .set_str("user.email", "test@example.invalid")
            .unwrap();
    }

    /// 向工作区写入文本文件，自动创建中间目录。
    pub fn write_file(root: &Path, path: &str, body: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, body).unwrap();
    }

    /// 向工作区写入二进制文件，自动创建中间目录。
    pub fn write_bytes(root: &Path, path: &str, body: &[u8]) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, body).unwrap();
    }

    /// 断言工作区文件内容（自动将 CRLF 归一化为 LF）。
    pub fn assert_file_text(root: &Path, path: &str, expected: &str) {
        let actual = fs::read_to_string(root.join(path)).unwrap();
        assert_eq!(actual.replace("\r\n", "\n"), expected);
    }

    /// 暂存全部变更并提交，使用仓库签名和父提交。
    pub fn commit_all(repo: &Repository, message: &str) -> Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = signature(repo).unwrap();
        let ps = parents(repo).unwrap();
        let parent_refs = ps.iter().collect::<Vec<_>>();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap()
    }

    /// 将本地路径转换为 file:// URL（平台感知）。
    pub fn path_url(path: &Path) -> String {
        let normalized = path.display().to_string().replace('\\', "/");
        if cfg!(windows) {
            format!("file:///{normalized}")
        } else {
            format!("file://{normalized}")
        }
    }
}
