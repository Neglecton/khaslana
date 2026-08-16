//! 按块/按行暂存服务：把未暂存/已暂存 diff 的部分改动应用到 index。
//!
//! 原理：以 `diff.print` 的**原始字节**（保真 CRLF / 非 UTF-8 / 无尾换行）重建
//! 部分 patch 文本，经 `Diff::from_buffer` + `repo.apply(ApplyLocation::Index)`
//! 应用——等价 `git apply --cached`，不触碰工作区（libgit2 的 Index 定位只写
//! index，无需 worktree_compat 包装）。
//!
//! - `stage_lines`（未暂存→暂存）：对 index→workdir diff 的部分改动前向应用；
//! - `unstage_lines`（取消暂存，等价 `git reset -p`）：对 HEAD→index diff 构造
//!   反向 patch（交换 +/- 前缀与 hunk 头两侧）。git2 0.21 的 `ApplyOptions`
//!   未暴露 reverse 标志，反向在文本层完成且为纯函数、可单测。
//!
//! 选择以行号为准（Added 行 new_lineno / Removed 行 old_lineno 唯一定位），
//! 服务端重生成 diff 为权威，与 UI 的上下文模式（紧凑/全文）无关。

use std::collections::HashSet;
use std::path::Path;

use git2::{ApplyLocation, DiffFormat, DiffOptions, Repository};

use super::GitService;
use crate::types::{GitError, RepositorySnapshot, Result};

/// 部分暂存的选择侧：Added 行以 new_lineno 定位，Removed 行以 old_lineno 定位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectionSide {
    Added,
    Removed,
}

/// 一条被选中的 diff 行（按行号唯一定位，跨上下文模式稳定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectedDiffLine {
    pub side: SelectionSide,
    pub lineno: u32,
}

/// 行选择集合。
pub type LineSelection = HashSet<SelectedDiffLine>;

/// 从 `diff.print` 收集的原始行（字节保真，含换行）。
#[derive(Clone, Debug)]
struct RawPatchLine {
    origin: char,
    content: Vec<u8>,
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
}

fn collect_raw_lines(diff: &git2::Diff<'_>) -> Result<Vec<RawPatchLine>> {
    let mut lines = Vec::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        lines.push(RawPatchLine {
            origin: line.origin(),
            content: line.content().to_vec(),
            old_lineno: line.old_lineno(),
            new_lineno: line.new_lineno(),
        });
        true
    })?;
    Ok(lines)
}

/// 解析 hunk 头 `@@ -a[,b] +c[,d] @@ ...`，返回 (old_start, old_count, new_start, new_count)。
/// count 缺省为 1（git 惯例）。
fn parse_hunk_header(content: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let text = std::str::from_utf8(content).ok()?;
    let rest = text.strip_prefix("@@ ")?.trim();
    let sides = rest.split(" @@").next()?;
    let mut side_iter = sides.split_whitespace();
    let old = side_iter.next()?.strip_prefix('-')?;
    let new = side_iter.next()?.strip_prefix('+')?;
    let parse = |token: &str| -> Option<(u32, u32)> {
        match token.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((token.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = parse(old)?;
    let (new_start, new_count) = parse(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// 格式化 hunk 头（count 为 1 时省略，为 0 时 start 归零）。
fn format_hunk_header(pre_start: u32, pre_count: u32, post_start: u32, post_count: u32) -> Vec<u8> {
    let side = |start: u32, count: u32| -> String {
        let start = if count == 0 { 0 } else { start };
        if count == 1 {
            format!("{start}")
        } else {
            format!("{start},{count}")
        }
    };
    format!(
        "@@ -{} +{} @@\n",
        side(pre_start, pre_count),
        side(post_start, post_count)
    )
    .into_bytes()
}

/// 把重算的 hunk 起始行号夹回 u32 范围（正常路径不会越界，纯防御）。
fn clamp_hunk_lineno(value: i64) -> u32 {
    value.clamp(0, u32::MAX as i64) as u32
}

/// 构造部分 patch 文本（字节保真）。
///
/// 规则（pre = patch 的 preimage 侧 = 应用基准；post = postimage 侧 = 目标内容）：
/// - forward（暂存）：pre=旧侧(index)、post=新侧(workdir)。
///   未选中的 `-` 行是 index 已有内容 -> 降级为上下文；未选中的 `+` 行不应
///   进入 index -> 整行丢弃。
/// - reverse（取消暂存）：两侧对调。未选中 `+`（index 内容）降级为上下文，
///   未选中 `-`（HEAD 独有、index 没有）整行丢弃。
/// - hunk 头 post 侧 start 必须重算：libgit2 的 apply 以 new_start 在「已被先前
///   输出块改写的目标内容」中精确定位（无偏移搜索），而源 diff 头中的 new_start
///   是完整后镜像坐标。重算式 = preimage 首行在目标初始内容中的行号 + 先前已输出
///   块的累计净行数变化；前序块全部整块保留时退化为原始 new_start。
/// - 整块全选走快路径（正文原样；反向交换 +/- 前缀；EOFNL 行
///   `"\ No newline..."` 内容不区分侧，跟随其宿主行自动换侧）。
/// - 含 EOFNL 标记的块拒绝按行部分选择（降级/丢弃会破坏无尾换行语义）。
///
/// 返回 `Ok(None)` 表示没有可应用的选中改动。
fn build_partial_patch(
    lines: &[RawPatchLine],
    selection: &LineSelection,
    reverse: bool,
) -> Result<Option<Vec<u8>>> {
    let selected = |origin: char, old_lineno: Option<u32>, new_lineno: Option<u32>| match origin {
        '+' => new_lineno.is_some_and(|lineno| {
            selection.contains(&SelectedDiffLine {
                side: SelectionSide::Added,
                lineno,
            })
        }),
        '-' => old_lineno.is_some_and(|lineno| {
            selection.contains(&SelectedDiffLine {
                side: SelectionSide::Removed,
                lineno,
            })
        }),
        _ => false,
    };

    let mut out: Vec<u8> = Vec::new();
    let mut has_change = false;

    let mut index = 0usize;
    // 文件头（'F'）原样透传。
    while index < lines.len() && lines[index].origin == 'F' {
        out.extend_from_slice(&lines[index].content);
        index += 1;
    }

    // libgit2 的 apply 用 hunk 头 new_start 在目标内容中精确定位本块（apply.c 的
    // apply_hunk + match_hunk，无偏移搜索），且多个块顺序应用时目标内容已被先前
    // 输出的块改写过。git 原始补丁头的 new_start 是「完整源 diff 后镜像」的坐标；
    // 丢弃或按行改写前面的块后，必须重算为「实际输出补丁的后镜像」坐标：
    //   new_start = preimage 首行在目标初始内容中的行号 + 先前已输出块的累计净行数变化。
    // 前序块全部整块保留时该值退化为原始 new_start；否则自动校正（例如丢弃了前面
    // 一个净增 3 行的块，后续块的 new_start 需要左移 3 行，否则定位错位 -> ApplyFail）。
    let mut emitted_delta: i64 = 0;

    while index < lines.len() {
        if lines[index].origin != 'H' {
            // 防御：Patch 输出中正文行不会先于 hunk 头出现。
            index += 1;
            continue;
        }
        let (old_start, old_count, new_start, new_count) =
            parse_hunk_header(&lines[index].content).unwrap_or((1, 0, 1, 0));
        index += 1;

        let body_start = index;
        while index < lines.len() && !matches!(lines[index].origin, 'H' | 'F') {
            index += 1;
        }
        let body = &lines[body_start..index];

        let change_count = body
            .iter()
            .filter(|line| line.origin == '+' || line.origin == '-')
            .count();
        let selected_count = body
            .iter()
            .filter(|line| line.origin == '+' || line.origin == '-')
            .filter(|line| selected(line.origin, line.old_lineno, line.new_lineno))
            .count();
        // 该块没有任何选中行：整块跳过，未选中的改动保持原状。
        if selected_count == 0 {
            continue;
        }

        // 整块全选：原样输出（反向交换 +/- 前缀与 hunk 头两侧；post 侧 start 按
        // 累计净行数变化重算，pre 侧保持源坐标即可——apply 定位只看 post 侧 start）。
        if selected_count == change_count {
            if reverse {
                // 反向头 = 原头两侧交换；正文交换 +/- 前缀。
                // 注意 line.content() 不含前缀字符，需按 origin 补写。
                // 反向时 preimage = 源 diff 新侧（index），首行行号 = new_start。
                let adjusted_post_start = clamp_hunk_lineno(new_start as i64 + emitted_delta);
                out.extend_from_slice(&format_hunk_header(
                    new_start,
                    new_count,
                    adjusted_post_start,
                    old_count,
                ));
                for line in body {
                    match line.origin {
                        '+' => {
                            out.push(b'-');
                            out.extend_from_slice(&line.content);
                        }
                        '-' => {
                            out.push(b'+');
                            out.extend_from_slice(&line.content);
                        }
                        // 上下文补空格前缀；EOFNL（"\ No newline..."）无前缀。
                        ' ' => {
                            out.push(b' ');
                            out.extend_from_slice(&line.content);
                        }
                        _ => out.extend_from_slice(&line.content),
                    }
                }
                emitted_delta += old_count as i64 - new_count as i64;
            } else {
                // 正向时 preimage = 源 diff 旧侧（index），首行行号 = old_start。
                let adjusted_post_start = clamp_hunk_lineno(old_start as i64 + emitted_delta);
                out.extend_from_slice(&format_hunk_header(
                    old_start,
                    old_count,
                    adjusted_post_start,
                    new_count,
                ));
                for line in body {
                    match line.origin {
                        ' ' | '+' | '-' => out.push(line.origin as u8),
                        _ => {}
                    }
                    out.extend_from_slice(&line.content);
                }
                emitted_delta += new_count as i64 - old_count as i64;
            }
            has_change = true;
            continue;
        }

        // 按行部分选择：含 EOFNL 标记的块拒绝（降级/丢弃会破坏无尾换行语义）。
        if body
            .iter()
            .any(|line| matches!(line.origin, '=' | '>' | '<'))
        {
            return Err(GitError::Message(
                "该改动块包含无尾换行文件的标记，暂不支持按行部分暂存，可整块操作".into(),
            ));
        }

        // 按行部分选择。pre 侧 = 应用基准（正向 index / 反向也 index，坐标不同源），
        // 每行携带 pre 侧显式行号；'+' 行不参与 pre 侧统计无需行号。
        // 反向时 pre 侧 = 源 diff 新侧（index），上下文行需用 new_lineno 定位。
        let mut pre_count = 0u32;
        let mut post_count = 0u32;
        let mut pre_first: Option<u32> = None;
        // 先解析每行（前缀 + pre 侧行号），再统一写出。
        let mut resolved: Vec<(u8, Option<u32>, &RawPatchLine)> = Vec::new();
        for line in body {
            let origin = line.origin;
            let sel = selected(origin, line.old_lineno, line.new_lineno);
            let entry: Option<(u8, Option<u32>)> = match (origin, sel) {
                (' ', _) => Some((
                    b' ',
                    if reverse {
                        line.new_lineno
                    } else {
                        line.old_lineno
                    },
                )),
                ('+', true) => {
                    if reverse {
                        // index 独有行被取消暂存：从 index 移除（index 侧行号权威）。
                        Some((b'-', line.new_lineno))
                    } else {
                        Some((b'+', None))
                    }
                }
                ('+', false) => {
                    if reverse {
                        // 留在 index：降级为上下文。
                        Some((b' ', line.new_lineno))
                    } else {
                        None // 不进入 index：整行丢弃。
                    }
                }
                ('-', true) => {
                    if reverse {
                        // HEAD 独有行恢复进 index。
                        Some((b'+', None))
                    } else {
                        Some((b'-', line.old_lineno))
                    }
                }
                ('-', false) => {
                    if reverse {
                        None // 保持不在 index：整行丢弃。
                    } else {
                        // index 已有内容保持：降级为上下文。
                        Some((b' ', line.old_lineno))
                    }
                }
                _ => None,
            };
            if let Some((prefix, pre_pos)) = entry {
                resolved.push((prefix, pre_pos, line));
            }
        }

        let mut body_out: Vec<u8> = Vec::new();
        for (prefix, pre_pos, line) in resolved {
            // 统计该侧出现的行（前缀 ' ' 与 '-' 在 preimage、' ' 与 '+' 在 postimage）。
            if prefix == b' ' || prefix == b'-' {
                pre_count += 1;
                if let Some(pos) = pre_pos {
                    pre_first.get_or_insert(pos);
                }
            }
            if prefix == b' ' || prefix == b'+' {
                post_count += 1;
            }
            body_out.push(prefix);
            body_out.extend_from_slice(&line.content);
        }

        // post 侧 start 按 preimage 实际落点重算：pre 侧首行行号（目标初始内容坐标）
        // + 先前已输出块的累计净行数变化。pre 侧行号缺失时退回该块 pre 侧 start。
        let pre_first_raw = pre_first.unwrap_or(if reverse { new_start } else { old_start });
        let adjusted_post_start = clamp_hunk_lineno(pre_first_raw as i64 + emitted_delta);
        out.extend_from_slice(&format_hunk_header(
            pre_first_raw,
            pre_count,
            adjusted_post_start,
            post_count,
        ));
        out.extend_from_slice(&body_out);
        emitted_delta += post_count as i64 - pre_count as i64;
        has_change = true;
    }

    if !has_change {
        return Ok(None);
    }
    Ok(Some(out))
}

impl GitService {
    /// 把未暂存 diff 中选中的行应用到暂存区（`git apply --cached` 语义）。
    pub fn stage_lines(
        &self,
        repo: &mut Repository,
        path: &Path,
        selection: &LineSelection,
    ) -> Result<RepositorySnapshot> {
        if selection.is_empty() {
            return Err(GitError::Message("没有选中的改动行".into()));
        }
        self.ensure_path_not_conflicted_with_action(repo, path, "暂存")?;
        self.progress.emit(crate::types::OperationEvent::Started(
            "正在暂存选中改动".into(),
        ));

        // 阶段一（块作用域内完成全部不可变借用）：重建 diff 并构造部分 patch 文本。
        let patch = {
            let mut options = DiffOptions::new();
            options.pathspec(path).context_lines(3);
            // 注意不带 include_untracked：未跟踪文件不支持部分暂存（无 index 基线）。
            let diff = repo.diff_index_to_workdir(None, Some(&mut options))?;
            self.prepare_partial_patch(&diff, selection, false, "暂存")?
        };

        // 阶段二：from_buffer 返回 Diff<'static>（不借 repo），apply 后快照。
        self.apply_partial_patch(repo, &patch, "暂存")?;

        self.progress.emit(crate::types::OperationEvent::Finished(
            "已暂存选中改动".into(),
        ));
        self.snapshot_after_operation(repo)
    }

    /// 把已暂存 diff 中选中的行移出暂存区（`git reset -p` 语义，反向 patch）。
    pub fn unstage_lines(
        &self,
        repo: &mut Repository,
        path: &Path,
        selection: &LineSelection,
    ) -> Result<RepositorySnapshot> {
        if selection.is_empty() {
            return Err(GitError::Message("没有选中的改动行".into()));
        }
        self.ensure_path_not_conflicted_with_action(repo, path, "取消暂存")?;
        self.progress.emit(crate::types::OperationEvent::Started(
            "正在取消暂存选中改动".into(),
        ));

        // 阶段一（块作用域内完成全部不可变借用）：重建 HEAD→index diff 并构造反向 patch。
        let patch = {
            let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
            let Some(head_tree) = head_tree else {
                return Err(GitError::Message(
                    "暂无提交历史，不能部分取消暂存，请使用整文件取消暂存".into(),
                ));
            };
            let mut options = DiffOptions::new();
            options.pathspec(path).context_lines(3);
            let diff = repo.diff_tree_to_index(Some(&head_tree), None, Some(&mut options))?;
            self.prepare_partial_patch(&diff, selection, true, "取消暂存")?
        };

        // 阶段二：反向 patch 应用到 index 后快照。
        self.apply_partial_patch(repo, &patch, "取消暂存")?;

        self.progress.emit(crate::types::OperationEvent::Finished(
            "已取消暂存选中改动".into(),
        ));
        self.snapshot_after_operation(repo)
    }

    /// 阶段一：守卫 + 构造部分 patch 文本（调用方以块作用域限制 diff/tree 借用）。
    fn prepare_partial_patch(
        &self,
        diff: &git2::Diff<'_>,
        selection: &LineSelection,
        reverse: bool,
        action: &str,
    ) -> Result<Vec<u8>> {
        // 守卫：整文件增/删/重命名/二进制不支持部分操作（无行级基线或需特殊头）。
        for delta in diff.deltas() {
            match delta.status() {
                git2::Delta::Added
                | git2::Delta::Deleted
                | git2::Delta::Renamed
                | git2::Delta::Untracked
                | git2::Delta::Unmodified => {
                    return Err(GitError::Message(format!(
                        "该文件为整文件新增/删除/重命名，暂不支持部分{action}，请使用整文件按钮"
                    )));
                }
                _ => {}
            }
        }
        let raw_lines = collect_raw_lines(diff)?;
        // 二进制守卫先于零-hunk 守卫：二进制 delta 只产出文件头 + 'B' 行，
        // 没有 hunk 头，顺序反了会给出误导文案。
        if raw_lines.iter().any(|line| line.origin == 'B') {
            return Err(GitError::Message(format!(
                "二进制文件不支持部分{action}，请使用整文件按钮"
            )));
        }
        if !raw_lines.iter().any(|line| line.origin == 'H') {
            return Err(GitError::Message(format!(
                "该文件没有可部分{action}的改动（可能是未跟踪文件或内容已变化），请刷新后使用整文件按钮"
            )));
        }
        let patch = build_partial_patch(&raw_lines, selection, reverse)?;
        let Some(patch) = patch else {
            return Err(GitError::Message(
                "所选改动与当前内容已一致，没有需要应用的变更".into(),
            ));
        };
        Ok(patch)
    }

    /// 阶段二：部分 patch 应用到 index（from_buffer 产出 Diff<'static>，不借 repo）。
    fn apply_partial_patch(&self, repo: &Repository, patch: &[u8], action: &str) -> Result<()> {
        let patch_diff = git2::Diff::from_buffer(patch)
            .map_err(|err| GitError::Message(format!("构造部分{action}补丁失败：{err}")))?;
        // apply 内部经 indexwriter 持有 index 锁：此前打开的 index 句柄
        //（冲突检查等）已在各自作用域释放。
        repo.apply(&patch_diff, ApplyLocation::Index, None)
            .map_err(|err| {
                GitError::Message(format!(
                    "所选改动无法应用到暂存区（内容可能已变化，请刷新后重试）：{err}"
                ))
            })?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/git/partial_stage.rs"]
mod tests;
