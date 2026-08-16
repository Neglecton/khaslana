use std::fs;
use std::path::Path;
use std::str;

use git2::build::CheckoutBuilder;
use git2::{ErrorCode, MergeFileOptions, Repository};

use super::{GitService, ensure_worktree_relative_path, path_to_git, remove_worktree_path};
use crate::{
    ConflictBlock, ConflictBlockStatus, ConflictDraftStatus, ConflictFileKind, ConflictFileView,
    ConflictResolutionSide, ExternalMergeSettings, GitError, OperationEvent, RepositorySnapshot,
    Result, external_merge,
};

impl GitService {
    pub fn conflict_file_view(&self, repo: &Repository, path: &Path) -> Result<ConflictFileView> {
        ensure_worktree_relative_path(path, "不能读取冲突详情")?;
        let git_path = path_to_git(path);

        match diff3_merge_text(repo, path)? {
            ConflictMergeText::Unsupported => Ok(ConflictFileView {
                path: git_path,
                kind: ConflictFileKind::Unsupported,
                draft: String::new(),
                ours_text: String::new(),
                theirs_text: String::new(),
                blocks: Vec::new(),
                draft_status: ConflictDraftStatus::Clean,
                fallback_reason: Some("该冲突缺少三方文本内容，请使用快捷解决按钮".into()),
            }),
            ConflictMergeText::Binary => Ok(ConflictFileView {
                path: git_path,
                kind: ConflictFileKind::Binary,
                draft: String::new(),
                ours_text: String::new(),
                theirs_text: String::new(),
                blocks: Vec::new(),
                draft_status: ConflictDraftStatus::Clean,
                fallback_reason: Some("该冲突文件不能使用文本合并编辑器".into()),
            }),
            ConflictMergeText::Text(merged_bytes) => {
                let merged_text = str::from_utf8(&merged_bytes).map_err(|_| {
                    GitError::Message("该冲突文件不是 UTF-8 文本，暂不能使用可视化编辑器".into())
                })?;
                let (draft, ours_text, theirs_text, blocks) =
                    parse_diff3_conflict_text(merged_text)?;

                Ok(ConflictFileView {
                    path: git_path,
                    kind: ConflictFileKind::Text,
                    draft,
                    ours_text,
                    theirs_text,
                    blocks,
                    draft_status: ConflictDraftStatus::Clean,
                    fallback_reason: None,
                })
            }
        }
    }

    /// 读取冲突文件的 diff3 原始文本（带 OURS/BASE/THEIRS 标记），
    /// 供 AI 合并建议构造 prompt 使用。
    pub fn conflict_diff3_text(&self, repo: &Repository, path: &Path) -> Result<String> {
        ensure_worktree_relative_path(path, "不能生成 AI 合并建议")?;
        match diff3_merge_text(repo, path)? {
            ConflictMergeText::Unsupported => Err(GitError::Message(
                "该冲突缺少三方文本内容，不支持 AI 合并建议".into(),
            )),
            ConflictMergeText::Binary => Err(GitError::Message(
                "二进制冲突文件不支持 AI 合并建议，请使用快捷解决按钮".into(),
            )),
            ConflictMergeText::Text(bytes) => {
                str::from_utf8(&bytes).map(str::to_string).map_err(|_| {
                    GitError::Message("该冲突文件不是 UTF-8 文本，暂不支持 AI 合并建议".into())
                })
            }
        }
    }

    pub fn apply_conflict_draft(
        &self,
        repo: &mut Repository,
        path: &Path,
        draft: &str,
    ) -> Result<RepositorySnapshot> {
        ensure_worktree_relative_path(path, "不能应用冲突草稿")?;
        self.progress
            .emit(OperationEvent::Started("正在应用冲突草稿".into()));
        conflict_for_path(&repo.index()?, path)?;
        write_conflict_draft(repo, path, draft)?;
        self.progress
            .emit(OperationEvent::Finished("冲突草稿已应用到工作区".into()));
        self.snapshot_after_operation(repo)
    }

    pub fn apply_conflict_draft_and_resolve(
        &self,
        repo: &mut Repository,
        path: &Path,
        draft: &str,
    ) -> Result<RepositorySnapshot> {
        ensure_worktree_relative_path(path, "不能应用并解决冲突")?;
        self.progress
            .emit(OperationEvent::Started("正在应用结果并标记冲突解决".into()));
        conflict_for_path(&repo.index()?, path)?;
        write_conflict_draft(repo, path, draft)?;
        let snapshot = self.mark_conflict_resolved_inner(repo, path)?;
        self.progress
            .emit(OperationEvent::Finished("冲突结果已应用并标记解决".into()));
        Ok(snapshot)
    }

    pub fn resolve_conflict_with_intellij_idea(
        &self,
        repo: &mut Repository,
        path: &Path,
    ) -> Result<RepositorySnapshot> {
        self.resolve_conflict_with_intellij_idea_settings(
            repo,
            path,
            &ExternalMergeSettings::default(),
        )
    }

    pub fn resolve_conflict_with_intellij_idea_settings(
        &self,
        repo: &mut Repository,
        path: &Path,
        settings: &ExternalMergeSettings,
    ) -> Result<RepositorySnapshot> {
        ensure_worktree_relative_path(path, "不能使用 IntelliJ IDEA 解决冲突")?;
        self.progress.emit(OperationEvent::Started(
            "正在等待 IntelliJ IDEA 合并完成".into(),
        ));
        conflict_for_path(&repo.index()?, path)?;
        let result = external_merge::run_intellij_idea_merge_with_settings(repo, path, settings)?;
        write_conflict_result_bytes(repo, path, &result)?;
        let snapshot = self.mark_conflict_resolved_inner(repo, path)?;
        self.progress.emit(OperationEvent::Finished(
            "IntelliJ IDEA 合并结果已应用".into(),
        ));
        Ok(snapshot)
    }

    pub fn resolve_conflict_with_side(
        &self,
        repo: &mut Repository,
        path: &Path,
        side: ConflictResolutionSide,
    ) -> Result<RepositorySnapshot> {
        ensure_worktree_relative_path(path, "不能解决冲突")?;
        let label = match side {
            ConflictResolutionSide::Ours => "当前版本",
            ConflictResolutionSide::Theirs => "传入版本",
        };
        self.progress
            .emit(OperationEvent::Started(format!("正在使用{label}解决冲突")));

        let mut index = repo.index()?;
        let conflict = conflict_for_path(&index, path)?;
        let selected_entry = match side {
            ConflictResolutionSide::Ours => conflict.our.as_ref(),
            ConflictResolutionSide::Theirs => conflict.their.as_ref(),
        };

        if selected_entry.is_some() {
            let mut checkout = CheckoutBuilder::new();
            checkout
                .force()
                .path(path)
                .disable_pathspec_match(true)
                .update_index(false);
            match side {
                ConflictResolutionSide::Ours => {
                    checkout.use_ours(true);
                }
                ConflictResolutionSide::Theirs => {
                    checkout.use_theirs(true);
                }
            }
            super::worktree_compat::checkout_index_preserving_locked_directories(
                repo,
                Some(&mut index),
                &mut checkout,
            )?;
            drop(index);
            let snapshot = self.mark_conflict_resolved_inner(repo, path)?;
            self.progress
                .emit(OperationEvent::Finished(format!("已使用{label}解决冲突")));
            return Ok(snapshot);
        }

        remove_worktree_path(repo, path)?;
        index.conflict_remove(path)?;
        let _ = index.remove_path(path);
        index.write()?;
        drop(index);
        self.progress
            .emit(OperationEvent::Finished(format!("已使用{label}解决冲突")));
        self.snapshot_after_operation(repo)
    }

    pub fn mark_conflict_resolved(
        &self,
        repo: &mut Repository,
        path: &Path,
    ) -> Result<RepositorySnapshot> {
        ensure_worktree_relative_path(path, "不能标记冲突已解决")?;
        self.progress
            .emit(OperationEvent::Started("正在标记冲突已解决".into()));
        let snapshot = self.mark_conflict_resolved_inner(repo, path)?;
        self.progress
            .emit(OperationEvent::Finished("冲突已标记为解决".into()));
        Ok(snapshot)
    }

    fn mark_conflict_resolved_inner(
        &self,
        repo: &mut Repository,
        path: &Path,
    ) -> Result<RepositorySnapshot> {
        let mut index = repo.index()?;
        conflict_for_path(&index, path)?;

        let workdir = repo
            .workdir()
            .ok_or_else(|| GitError::Message("裸仓库没有工作区，不能标记冲突已解决".into()))?;
        let full_path = workdir.join(path);
        match fs::symlink_metadata(&full_path) {
            Ok(metadata) => {
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    return Err(GitError::Message(
                        "冲突路径是文件夹，不能标记为已解决".into(),
                    ));
                }
                index.conflict_remove(path)?;
                index.add_path(path)?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                index.conflict_remove(path)?;
                let _ = index.remove_path(path);
            }
            Err(err) => return Err(GitError::Io(err)),
        }
        index.write()?;
        drop(index);
        self.snapshot_after_operation(repo)
    }
}

fn conflict_for_path(index: &git2::Index, path: &Path) -> Result<git2::IndexConflict> {
    index.conflict_get(path).map_err(|err| {
        if err.code() == ErrorCode::NotFound {
            GitError::Message(format!("该文件不存在冲突：{}", path_to_git(path)))
        } else {
            GitError::Git(err)
        }
    })
}

/// `diff3_merge_text` 的加载结果：三方文本可用 / 缺三方条目 / 二进制。
enum ConflictMergeText {
    /// 带 OURS/BASE/THEIRS 标记的合并原始字节（UTF-8 判定留给调用方，
    /// 便于按场景给出不同文案）。
    Text(Vec<u8>),
    Unsupported,
    Binary,
}

/// 守卫并生成冲突文件的 diff3 合并文本：
/// 三方 index 条目齐全且均为文本 blob 时，返回带
/// `<<<<<<< OURS / ||||||| BASE / ======= / >>>>>>> THEIRS` 标记的合并文本。
fn diff3_merge_text(repo: &Repository, path: &Path) -> Result<ConflictMergeText> {
    let index = repo.index()?;
    let conflict = conflict_for_path(&index, path)?;

    let (Some(ancestor), Some(ours), Some(theirs)) = (
        conflict.ancestor.as_ref(),
        conflict.our.as_ref(),
        conflict.their.as_ref(),
    ) else {
        return Ok(ConflictMergeText::Unsupported);
    };

    if [ancestor, ours, theirs]
        .into_iter()
        .any(|entry| entry.mode == 0 || blob_is_binary(repo, entry).unwrap_or(true))
    {
        return Ok(ConflictMergeText::Binary);
    }

    let mut options = MergeFileOptions::new();
    options
        .style_diff3(true)
        .ancestor_label("BASE")
        .our_label("OURS")
        .their_label("THEIRS");
    let merged = repo.merge_file_from_index(ancestor, ours, theirs, Some(&mut options))?;
    Ok(ConflictMergeText::Text(merged.content().to_vec()))
}

fn write_conflict_draft(repo: &Repository, path: &Path, draft: &str) -> Result<()> {
    write_conflict_result_bytes(repo, path, draft.as_bytes())
}

fn write_conflict_result_bytes(repo: &Repository, path: &Path, result: &[u8]) -> Result<()> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::Message("裸仓库没有工作区，不能写入冲突结果".into()))?;
    let full_path = workdir.join(path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(full_path, result)?;
    Ok(())
}

fn blob_is_binary(repo: &Repository, entry: &git2::IndexEntry) -> Result<bool> {
    // 先读 ODB 对象头取体积：超过全文视图阈值的 blob 直接按二进制处理
    // （无法用文本合并编辑器），避免大对象为嗅探 NUL 先整体 inflate 到堆；
    // 小对象才加载内容并只检查前 8KB。
    if let Ok(odb) = repo.odb()
        && let Ok((size, _)) = odb.read_header(entry.id)
        && size as u64 > super::FULL_FILE_MAX_BYTES
    {
        return Ok(true);
    }
    let blob = repo.find_blob(entry.id)?;
    let sample = &blob.content()[..blob.content().len().min(8 * 1024)];
    Ok(sample.contains(&0))
}

/// 去掉行尾 `\n` / `\r\n`，得到标记行比较用的纯文本。
fn trim_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// 判断是否为 libgit2 生成的冲突分隔标记行。
///
/// 标记是整行的固定文本（`<<<<<<< OURS` 等），必须整行精确匹配而不是
/// `starts_with`——正常内容行完全可能以 `=======` 开头（RST/Markdown
/// 标题下划线、分隔线），前缀匹配会把它误判为块边界，导致三栏解析错位。
fn is_marker_line(line: &str, marker: &str) -> bool {
    trim_line_ending(line) == marker
}

fn parse_diff3_conflict_text(
    content: &str,
) -> Result<(String, String, String, Vec<ConflictBlock>)> {
    let lines = split_lines_preserve_endings(content);
    let mut index = 0;
    let mut draft = String::new();
    let mut ours_text = String::new();
    let mut theirs_text = String::new();
    let mut blocks = Vec::new();

    while index < lines.len() {
        let line = lines[index];
        if !is_marker_line(line, "<<<<<<< OURS") {
            draft.push_str(line);
            ours_text.push_str(line);
            theirs_text.push_str(line);
            index += 1;
            continue;
        }

        index += 1;
        let ours_start = index;
        while index < lines.len() && !is_marker_line(lines[index], "||||||| BASE") {
            index += 1;
        }
        if index >= lines.len() {
            return Err(GitError::Message("冲突文本缺少 BASE 分隔标记".into()));
        }
        let ours = lines[ours_start..index].concat();

        index += 1;
        let base_start = index;
        while index < lines.len() && !is_marker_line(lines[index], "=======") {
            index += 1;
        }
        if index >= lines.len() {
            return Err(GitError::Message("冲突文本缺少中间分隔标记".into()));
        }
        let base = lines[base_start..index].concat();

        index += 1;
        let theirs_start = index;
        while index < lines.len() && !is_marker_line(lines[index], ">>>>>>> THEIRS") {
            index += 1;
        }
        if index >= lines.len() {
            return Err(GitError::Message("冲突文本缺少 THEIRS 结束标记".into()));
        }
        let theirs = lines[theirs_start..index].concat();
        index += 1;

        let start = draft.len();
        let ours_start = ours_text.len();
        let theirs_start = theirs_text.len();
        draft.push_str(&ours);
        ours_text.push_str(&ours);
        theirs_text.push_str(&theirs);
        let end = draft.len();
        blocks.push(ConflictBlock {
            base: Some(base),
            ours,
            theirs,
            start,
            end,
            ours_start,
            ours_end: ours_text.len(),
            theirs_start,
            theirs_end: theirs_text.len(),
            status: ConflictBlockStatus::Unresolved,
            has_manual_edits: false,
        });
    }

    Ok((draft, ours_text, theirs_text, blocks))
}

fn split_lines_preserve_endings(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut start = 0;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            lines.push(&content[start..index + 1]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

#[cfg(test)]
#[path = "../tests/git/conflicts.rs"]
mod tests;
