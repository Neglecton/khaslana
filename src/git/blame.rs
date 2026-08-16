// 文件历史（按路径过滤的提交列表）与文件追溯（blame）的 Git 服务。
//
// 「文件历史」只返回改动过指定路径的提交；「文件追溯」逐行归属到
// 最后修改它的提交。两者 v1 都基于当前路径，不追踪 rename/copy
// （等价 `git log <path>` 与不带 -M/-C 的 `git blame`）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use bstr::ByteSlice;
use git2::{BlameOptions, DiffOptions, Repository, Sort};

use super::HistoryRefsCache;
use super::is_empty_head_error;
use crate::{
    GitService,
    types::{
        BlameCommitInfo, BlameHunkInfo, BlameView, CommitInfo, DiffEncodingChoice, GitError,
        HistoryScope, Result,
    },
};

impl GitService {
    /// 按文件路径过滤的提交历史：只返回改动过该文件的提交。
    ///
    /// 性能特征：libgit2 的 revwalk 不支持 pathspec，需要全量迭代提交并对
    /// 每个提交做 first-parent tree-diff + 单 pathspec（树差异比较有剪枝，
    /// v1 可接受，超大仓库后续再下沉缓存）；分页 skip/take 作用在过滤后的
    /// OID 流上，与普通历史的 offset 语义一致。
    /// refs/徽章照常填充，`refs_cache` 复用逻辑与普通历史一致。
    pub fn file_history(
        &self,
        repo: &Repository,
        scope: HistoryScope,
        path: &str,
        offset: usize,
        limit: usize,
        refs_cache: Option<&HistoryRefsCache>,
    ) -> Result<(Vec<CommitInfo>, HistoryRefsCache)> {
        let refs_cache = match refs_cache {
            Some(cache) => cache.clone(),
            None => self.commit_graph_refs(repo)?,
        };
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        match scope {
            HistoryScope::CurrentBranch => {
                if let Err(err) = walk.push_head() {
                    if is_empty_head_error(&err) {
                        return Ok((Vec::new(), refs_cache));
                    }
                    return Err(err.into());
                }
            }
            HistoryScope::AllRefs => {
                if refs_cache.starts.is_empty() {
                    if let Err(err) = walk.push_head() {
                        if is_empty_head_error(&err) {
                            return Ok((Vec::new(), refs_cache));
                        }
                        return Err(err.into());
                    }
                } else {
                    for oid in &refs_cache.starts {
                        walk.push(*oid)?;
                    }
                }
            }
        }

        let path = Path::new(path);
        let mut touched = Vec::new();
        for oid in walk {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let parent_tree = if commit.parent_count() > 0 {
                Some(commit.parent(0)?.tree()?)
            } else {
                None
            };
            let tree = commit.tree()?;
            // 只判断该提交相对 first-parent 是否触及此路径，
            // 不加载内容（复用 commit_diff 的 pathspec 先例）。
            let mut options = DiffOptions::new();
            options.pathspec(path);
            let diff =
                repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
            if diff.deltas().next().is_some() {
                touched.push(oid);
            }
        }

        let commits = self.collect_commit_infos(
            repo,
            touched
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(std::result::Result::Ok),
            &refs_cache.refs_by_oid,
        )?;
        Ok((commits, refs_cache))
    }

    /// 计算文件追溯（blame）：把文件的每一行归属到最后修改它的提交。
    ///
    /// 基于 HEAD 版本；工作区文件存在且非二进制时，经 `blame_buffer` 把
    /// 未提交改动纳入结果（差异行不属于任何提交，`commit` 为 None）。
    /// v1 不支持对任意提交版本追溯（`BlameOptions::newest_commit` 留作后续）。
    pub fn blame_file(
        &self,
        repo: &Repository,
        path: &Path,
        encoding: DiffEncodingChoice,
    ) -> Result<BlameView> {
        // 守卫：HEAD 必须已包含该路径，未提交过的文件没有追溯信息。
        let head_tree = repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_tree().ok())
            .ok_or_else(|| GitError::Message("该文件尚未提交，暂无追溯信息".to_string()))?;
        let entry = head_tree
            .get_path(path)
            .map_err(|_| GitError::Message("该文件尚未提交，暂无追溯信息".to_string()))?;

        // 大文件保护：先读 ODB 对象头取体积（不加载内容），超大文件直接报错。
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
        let head_content = blob.content().to_vec();
        if bytes_sample_is_binary(&head_content) {
            return Err(GitError::Message("二进制文件不支持追溯".to_string()));
        }

        // 默认不追踪 rename/copy（等价 git blame 不带 -M/-C）。
        let head_blame = repo.blame_file(path, Some(&mut BlameOptions::new()))?;

        // 工作区文件存在且非二进制时改用其内容做 blame_buffer，
        // 行数组与追溯块保持同一坐标系；否则直接使用 HEAD blob 的行。
        let workdir_path = repo.workdir().map(|root| root.join(path));
        let workdir_bytes = match workdir_path {
            Some(file) if file.is_file() => fs::read(&file).ok(),
            _ => None,
        };
        let use_workdir = workdir_bytes
            .as_deref()
            .is_some_and(|bytes| !bytes_sample_is_binary(bytes));

        let buffer_blame;
        let blame = if use_workdir {
            buffer_blame = head_blame.blame_buffer(workdir_bytes.as_deref().unwrap())?;
            &buffer_blame
        } else {
            &head_blame
        };
        let content_bytes: &[u8] = if use_workdir {
            workdir_bytes.as_deref().unwrap()
        } else {
            &head_content
        };

        // 编码：有限字节样本选编码，尊重用户手动选择，整体解码后按行切分
        //（与分支浏览内容视图同一套口径）。
        let sample_len = content_bytes.len().min(super::DIFF_ENCODING_SAMPLE_LIMIT);
        let (resolved_encoding, encoding_impl) =
            super::resolve_diff_encoding(encoding, &content_bytes[..sample_len]);
        let (decoded, _used, _had_errors) = encoding_impl.decode(content_bytes);
        let lines: Vec<String> = decoded.lines().map(str::to_string).collect();

        // libgit2 会为每个 hunk 填充 final 签名与 summary（blame_buffer 的
        // 未提交行除外：零 OID、无签名），无需再逐提交查库。
        let mut commit_cache: HashMap<String, Option<BlameCommitInfo>> = HashMap::new();
        let mut hunks = Vec::new();
        let mut line_hunk = vec![0usize; lines.len()];
        for hunk in blame.iter() {
            let hunk_index = hunks.len();
            let start = hunk.final_start_line();
            let count = hunk.lines_in_hunk();
            let final_oid = hunk.final_commit_id();
            let commit_info = if final_oid.is_zero() {
                // blame_buffer 中工作区差异行不属于任何提交
                None
            } else {
                let oid_string = final_oid.to_string();
                commit_cache
                    .entry(oid_string.clone())
                    .or_insert_with(|| {
                        let signature = hunk.final_signature();
                        Some(BlameCommitInfo {
                            short_oid: oid_string.chars().take(8).collect(),
                            author: signature
                                .as_ref()
                                .and_then(|sig| sig.name().ok())
                                .unwrap_or("?")
                                .to_string(),
                            time: signature
                                .as_ref()
                                .map(|sig| sig.when().seconds())
                                .unwrap_or_default(),
                            summary: hunk
                                .summary_bytes()
                                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                                .unwrap_or_else(|| "(无提交信息)".to_string()),
                            oid: oid_string,
                        })
                    })
                    .clone()
            };
            // 行号 1 基 -> 行索引 0 基填充所属块；防御性钳制，
            // 内容与 blame 使用同一坐标系，正常不会越界。
            let first_index = start.saturating_sub(1);
            let last_index = first_index.saturating_add(count).min(line_hunk.len());
            for line_index in first_index..last_index {
                line_hunk[line_index] = hunk_index;
            }
            hunks.push(BlameHunkInfo {
                commit: commit_info,
                start_line: start,
                line_count: count,
            });
        }

        Ok(BlameView {
            path: super::path_to_git(path),
            lines,
            hunks,
            line_hunk,
            encoding: resolved_encoding,
        })
    }
}

/// 前 8KB 含 NUL 字节视为二进制（与工作区 diff/分支浏览嗅探口径一致）。
fn bytes_sample_is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8 * 1024)];
    sample.find_byte(0).is_some()
}

#[cfg(test)]
#[path = "../tests/git/blame.rs"]
mod tests;
