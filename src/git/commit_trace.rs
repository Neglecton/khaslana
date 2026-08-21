// 分支谱系追踪服务：供提交图谱页计算「分支动向高亮」的 OID 集合。
//
// `branch_commit_oids` 从分支 tip 出发收集可达提交（可选隐藏 HEAD 可达集，
// 得到「仅领先 HEAD」的增量动向），带数量上限与截断标记，避免超大仓库
// 一次遍历全历史撑爆内存。实现沿用 `branch_sync_status` 的 push/hide
// revwalk 模式，纯本地查询走 Short 任务池。

use git2::{BranchType, Repository, Sort};

use crate::{GitService, types::Result};

/// 单次谱系追踪的 OID 数量上限：图谱页高亮集合按此截断（够覆盖绝大多数
/// 分支的可视历史；截断时 UI 会提示「已高亮前 N 个提交」）。
pub const COMMIT_TRACE_OID_LIMIT: usize = 2000;

impl GitService {
    /// 收集分支的可达提交 OID 集合（图谱页高亮用）。
    ///
    /// 分支按「本地优先、远端兜底」解析：本地分支名直接命中，远端分支用
    /// 完整短名（如 `origin/feature`）按 Remote 类型命中。
    /// - `ahead_only = false`：分支全谱系（含祖先），高亮出该分支的完整主干路径；
    /// - `ahead_only = true`：仅分支领先 HEAD 的提交（push 分支 tip / hide HEAD），
    ///   看该分支相对当前 HEAD 的增量动向。
    ///
    /// 返回 `(oid 列表, 是否截断)`；分支不存在时返回中文错误。
    pub fn branch_commit_oids(
        &self,
        repo: &Repository,
        branch: &str,
        ahead_only: bool,
    ) -> Result<(Vec<String>, bool)> {
        let branch_ref = repo
            .find_branch(branch, BranchType::Local)
            .or_else(|_| repo.find_branch(branch, BranchType::Remote))
            .map_err(|_| crate::types::GitError::Message(format!("分支不存在：{branch}")))?;
        let Some(tip) = branch_ref.get().target() else {
            return Ok((Vec::new(), false));
        };
        drop(branch_ref);

        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        walk.push(tip)?;
        if ahead_only {
            // HEAD 不可解析（空仓库）时退化为全谱系，而非报错。
            if let Ok(head) = repo.head() {
                if let Some(head_oid) = head.target() {
                    walk.hide(head_oid)?;
                }
            }
        }

        let mut oids = Vec::new();
        let mut truncated = false;
        for oid in walk {
            if oids.len() >= COMMIT_TRACE_OID_LIMIT {
                truncated = true;
                break;
            }
            oids.push(oid?.to_string());
        }
        Ok((oids, truncated))
    }
}

#[cfg(test)]
#[path = "../tests/git/commit_trace.rs"]
mod tests;
