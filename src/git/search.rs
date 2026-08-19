// 代码搜索 Git 服务：在指定提交的文件树里按子串/正则逐行搜索，
// 供 AI 评审 agent 的 search_code 工具使用（近似符号查找）。
//
// 只读操作，不触碰 index/worktree。为控制大仓库成本，设扫描文件数与
// 单文件体积上限，超限即静默跳过；命中达到 max_results 立即停止。

use std::path::Path;

use git2::{ObjectType, Repository, Tree};

use crate::{
    GitService,
    types::{GitError, Result},
};

/// 一次搜索命中的行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSearchMatch {
    /// 相对仓库根的 git 风格路径。
    pub path: String,
    /// 一基行号。
    pub lineno: u32,
    /// 命中行内容（UTF-8 有损解码，按 SEARCH_LINE_SNIPPET_CHARS 截断）。
    pub line: String,
}

/// 最多扫描的文件数；超过后剩余文件静默跳过，避免超大仓库全树遍历。
const SEARCH_FILE_SCAN_LIMIT: usize = 1000;
/// 单文件字节上限；超过视为大文件跳过。
const SEARCH_BLOB_MAX_BYTES: u64 = 1024 * 1024;
/// 单条命中行的截断长度：压缩/生成产物等无 NUL 的超长单行（近 1MB）若
/// 整行入库，50 命中会产生明显内存峰值，定位价值在前缀就足够。
const SEARCH_LINE_SNIPPET_CHARS: usize = 200;

impl GitService {
    /// 在指定提交的树里逐行搜索 `query`。
    ///
    /// - `is_regex=false`：子串匹配（大小写敏感）；
    /// - `is_regex=true`：按 `regex` crate 语法编译，失败返回中文错误；
    /// - `path_prefix`：可选目录前缀（如 `src/`），限定搜索范围降噪；
    /// - 空白查询返回错误；二进制扩展名与超大 blob 跳过；
    /// - 命中累计到 `max_results` 即整体停止。
    pub fn search_code(
        &self,
        repo: &Repository,
        commit_oid: &str,
        query: &str,
        is_regex: bool,
        path_prefix: Option<&str>,
        max_results: usize,
    ) -> Result<Vec<CodeSearchMatch>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(GitError::Message("搜索内容不能为空".into()));
        }
        let matcher = if is_regex {
            Some(SearchMatcher::compile_regex(query)?)
        } else {
            None
        };
        let matcher = matcher.unwrap_or_else(|| SearchMatcher::substring(query));

        let commit = self.find_commit_by_oid(repo, commit_oid)?;
        let tree = commit.tree()?;

        let mut context = SearchContext {
            repo,
            matcher: &matcher,
            remaining: max_results,
            scanned: 0,
            matches: Vec::new(),
        };
        // 目录前缀限定：直接定位到前缀子树再遍历，前缀之外的整体剪掉。
        // 前缀不存在或指向文件时显式报错——静默返回空结果会让模型把
        // 「前缀写错」误读成「该标识符在仓库里不存在」。
        match normalize_path_prefix(path_prefix) {
            Some(prefix) => {
                let subtree = resolve_subtree(repo, &tree, &prefix)?;
                walk_tree(&mut context, &subtree, prefix);
            }
            None => walk_tree(&mut context, &tree, String::new()),
        }
        Ok(context.matches)
    }
}

/// 归一化目录前缀：去空白、去首尾斜杠、统一为 `a/b/` 形式；空返回 None。
fn normalize_path_prefix(prefix: Option<&str>) -> Option<String> {
    let trimmed = prefix?.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("{trimmed}/"))
}

/// 定位指定前缀的子树；路径不存在或不是目录返回中文错误（供模型自行
/// 修正前缀）。
fn resolve_subtree<'r>(
    repo: &'r Repository,
    root: &git2::Tree<'r>,
    prefix: &str,
) -> Result<git2::Tree<'r>> {
    let path = Path::new(prefix.trim_end_matches('/'));
    let entry = root.get_path(path).map_err(|_| {
        GitError::Message(format!("目录不存在：{prefix}（请检查 path_prefix 拼写）"))
    })?;
    let object = entry
        .to_object(repo)
        .map_err(|_| GitError::Message(format!("目录不存在：{prefix}")))?;
    object.into_tree().map_err(|_| {
        GitError::Message(format!(
            "{prefix} 是文件不是目录，请去掉文件名只保留目录前缀"
        ))
    })
}

/// 单行匹配器：子串或预编译正则。
enum SearchMatcher {
    Substring { needle: String },
    Regex { regex: regex::Regex },
}

impl SearchMatcher {
    fn substring(needle: &str) -> Self {
        Self::Substring {
            needle: needle.to_string(),
        }
    }

    fn compile_regex(pattern: &str) -> Result<Self> {
        regex::Regex::new(pattern)
            .map(|regex| Self::Regex { regex })
            .map_err(|err| GitError::Message(format!("正则表达式无效：{err}")))
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Substring { needle } => line.contains(needle.as_str()),
            Self::Regex { regex } => regex.is_match(line),
        }
    }
}

/// 一次搜索的可变状态（扫描计数、命中累积）。
struct SearchContext<'a> {
    repo: &'a Repository,
    matcher: &'a SearchMatcher,
    /// 距离命中上限的剩余名额；为 0 时停止一切扫描。
    remaining: usize,
    /// 已扫描文件数，用于上限保护。
    scanned: usize,
    matches: Vec<CodeSearchMatch>,
}

/// 递归遍历树（显式调用栈深度受目录层级限制，直接递归即可）。
///
/// 目录优先深入、条目按树内顺序访问；一旦命中数打满立即剪枝返回。
/// 扫描数达上限同样剪枝——否则只是跳过 blob 读取，树遍历本身与每棵
/// 子树的 `to_object` 加载仍会走完全仓库。
fn walk_tree(context: &mut SearchContext<'_>, tree: &Tree<'_>, prefix: String) {
    for entry in tree.iter() {
        if context.remaining == 0 || context.scanned >= SEARCH_FILE_SCAN_LIMIT {
            return;
        }
        let name = entry.name().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                if let Ok(child) = entry.to_object(context.repo)
                    && let Ok(child) = child.into_tree()
                {
                    walk_tree(context, &child, path);
                }
            }
            // 子模块不搜索。
            Some(ObjectType::Commit) => {}
            _ => search_blob(context, &path, entry.id()),
        }
    }
}

/// 搜索单个文件 blob：守卫（扩展名/体积/扫描数）通过后逐行匹配。
///
/// 扫描计数只统计真正付出了 IO 的文件：二进制扩展名检查是纯字符串判断、
/// 无任何 IO，先于计数（否则大量二进制/生成产物会把 1000 个名额吃光，
/// 有效扫描面明显缩水）；体积预检与 NUL 嗅探有真实对象读取，仍计数。
fn search_blob(context: &mut SearchContext<'_>, path: &str, blob_id: git2::Oid) {
    if context.scanned >= SEARCH_FILE_SCAN_LIMIT {
        return;
    }
    if super::path_has_binary_extension(path) {
        return;
    }
    context.scanned += 1;
    // 先读对象头取体积，超大文件不整体加载。
    if let Ok(odb) = context.repo.odb()
        && let Ok((size, _)) = odb.read_header(blob_id)
        && size as u64 > SEARCH_BLOB_MAX_BYTES
    {
        return;
    }
    let Ok(blob) = context.repo.find_blob(blob_id) else {
        return;
    };
    // 内容嗅探：前 8KB 含 NUL 视为二进制跳过（扩展名兜底之外的保险）。
    let content = blob.content();
    if content[..content.len().min(8 * 1024)].contains(&0) {
        return;
    }

    // 搜索面向 AI 评审的代码定位，UTF-8 有损解码即可；非 UTF-8 文件的
    // 命中行可能出现替换符，不影响定位价值。
    let text = String::from_utf8_lossy(content);
    for (index, line) in text.lines().enumerate() {
        if context.remaining == 0 {
            return;
        }
        if context.matcher.is_match(line) {
            // 超长行入库前截断（定位价值在前缀；整行入库在无 NUL 的
            // 压缩/生成产物场景会产生显著内存峰值）。
            let line: String = if line.chars().count() > SEARCH_LINE_SNIPPET_CHARS {
                let truncated: String = line.chars().take(SEARCH_LINE_SNIPPET_CHARS).collect();
                format!("{truncated}…")
            } else {
                line.to_string()
            };
            context.matches.push(CodeSearchMatch {
                path: path.to_string(),
                lineno: index as u32 + 1,
                line,
            });
            context.remaining -= 1;
        }
    }
}

#[cfg(test)]
#[path = "../tests/git/search.rs"]
mod tests;
