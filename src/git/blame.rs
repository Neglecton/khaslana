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

        // 工作区文件存在且非二进制时，先把换行风格对齐到 HEAD blob 再参与
        // blame_buffer（见 align_line_endings_to_blob 注释），避免 CRLF 工作区
        // 被裸字节 diff 判成整文件改动。
        let workdir_path = repo.workdir().map(|root| root.join(path));
        let workdir_bytes = match workdir_path {
            Some(file) if file.is_file() => fs::read(&file).ok(),
            _ => None,
        };
        let aligned_workdir: Option<Vec<u8>> = workdir_bytes
            .as_deref()
            .filter(|bytes| !bytes_sample_is_binary(bytes))
            .map(|raw| align_line_endings_to_blob(&head_content, raw));

        // 内容基准与 blame 来源：
        // - 工作区不可用 → HEAD blob + blob blame；
        // - 工作区与 HEAD 一致（含换行风格差异）→ HEAD blob + blob blame
        //   （干净文件不应因 CRLF 被判成全行「未提交」）；
        // - 工作区有真实改动 → 对齐内容 + blame_buffer（未提交行零 OID）；
        // - 工作区被清空 → 空内容、无块（git_blame_buffer 不接受空 buffer）。
        let buffer_blame;
        let blame_ref: Option<&git2::Blame<'_>>;
        let content_bytes: &[u8];
        match &aligned_workdir {
            None => {
                blame_ref = Some(&head_blame);
                content_bytes = &head_content;
            }
            Some(aligned) if aligned.is_empty() => {
                blame_ref = None;
                content_bytes = aligned;
            }
            Some(aligned) if aligned.as_slice() == head_content => {
                blame_ref = Some(&head_blame);
                content_bytes = &head_content;
            }
            Some(aligned) => {
                buffer_blame = head_blame.blame_buffer(aligned)?;
                blame_ref = Some(&buffer_blame);
                content_bytes = aligned;
            }
        }

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
        // 工作区文件被清空时无块可归属（blame_ref 为 None），空内容视图直接返回。
        if let Some(blame) = blame_ref {
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

/// 把工作区字节按 HEAD blob 的换行风格（含/不含 CR）对齐后返回。
///
/// `git_blame_buffer` 内部对 blob 与 buffer 做**裸字节 diff**，不经
/// core.autocrlf / gitattributes 过滤（libgit2-sys 也未暴露过滤器 API）；
/// Windows 仓库常见「工作区 CRLF、blob LF」，此时每一行在字节层都不同，
/// 会被判成整文件新增（全部零 OID → 全行「未提交」）。对齐后 diff 只
/// 反映真实内容差异。混合换行的 blob 以「是否含 CR」近似判断；特殊过滤器
///（working-tree-encoding、自定义 filter）不在处理范围。
fn align_line_endings_to_blob(blob: &[u8], workdir: &[u8]) -> Vec<u8> {
    let blob_crlf = blob.contains(&b'\r');
    let workdir_crlf = workdir.contains(&b'\r');
    if blob_crlf == workdir_crlf {
        return workdir.to_vec();
    }
    let mut aligned = Vec::with_capacity(workdir.len() + workdir.len() / 8);
    let mut index = 0;
    while index < workdir.len() {
        let byte = workdir[index];
        if !blob_crlf && byte == b'\r' && workdir.get(index + 1) == Some(&b'\n') {
            // blob 为 LF 风格：丢弃 \r\n 中的 \r（孤立 \r 保留）
            index += 1;
            continue;
        }
        if blob_crlf && byte == b'\n' && workdir[index.saturating_sub(1)] != b'\r' {
            // blob 为 CRLF 风格：补上缺失的 \r
            aligned.push(b'\r');
        }
        aligned.push(byte);
        index += 1;
    }
    aligned
}

/// 前 8KB 含 NUL 字节视为二进制（与工作区 diff/分支浏览嗅探口径一致）。
fn bytes_sample_is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8 * 1024)];
    sample.find_byte(0).is_some()
}

#[cfg(test)]
#[path = "../tests/git/blame.rs"]
mod tests;
