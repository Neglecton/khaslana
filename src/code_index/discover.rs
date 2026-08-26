//! 文件发现与过滤（参照 codebase-memory-mcp 的 `discover.c`）。
//!
//! 遍历用 `ignore` crate：完整实现嵌套 .gitignore、`.git/info/exclude`、全局
//! excludesfile 语义（与参考项目自研的 gitignore 匹配器行为一致）。在此之上叠加
//! 参考项目的 ALWAYS_SKIP_DIRS / 后缀黑名单——这些目录即使不在 gitignore 里
//! （未提交的构建产物等）也必须剪枝。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;

use super::{MAX_INDEX_FILES, err};
use crate::types::Result;

/// 单条发现的文件记录。
#[derive(Clone, Debug)]
pub struct DiscoveredFile {
    /// 相对仓库根的正斜杠路径（`src/git.rs`），与 file_hashes/节点 file_path 一致。
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub size: u64,
    /// Unix 纳秒时间戳（增量对比键；i64 可表示到 2262 年）。
    pub mtime_ns: u64,
}

#[derive(Debug, Default)]
pub struct DiscoverOutcome {
    pub files: Vec<DiscoveredFile>,
    /// 被排除的文件总数（目录剪枝 + 后缀黑名单合计，仅统计口径展示）。
    pub excluded_count: usize,
}

/// 永远跳过的目录名（basename 匹配，任意深度，大小写不敏感）。取参考项目
/// ALWAYS_SKIP_DIRS 中对桌面代码仓库有意义的子集 + VCS 元数据。
const ALWAYS_SKIP_DIRS: &[&str] = &[
    // VCS 与 IDE
    ".git",
    ".hg",
    ".svn",
    ".jj",
    ".sl",
    ".idea",
    ".vscode",
    ".fleet",
    // Python
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".ipynb_checkpoints",
    // JavaScript
    "node_modules",
    "bower_components",
    ".next",
    ".nuxt",
    ".turbo",
    ".parcel-cache",
    ".svelte-kit",
    ".astro",
    ".vite",
    ".webpack",
    // 构建产物
    "dist",
    "build",
    "out",
    "obj",
    "target",
    "Debug",
    "Release",
    "bazel-bin",
    "bazel-out",
    "bazel-testlogs",
    "Pods",
    "DerivedData",
    ".gradle",
    ".stack-work",
    "zig-cache",
    "zig-out",
    ".terraform",
    // 依赖副本（参考项目显式跳过 vendor/vendored）
    "vendor",
    "vendored",
    // 其他
    ".cache",
    ".claude",
    ".codebase-memory",
    ".khaslana",
    "coverage",
];

/// 永远忽略的后缀：二进制、媒体、字体、数据库、压缩包与生成产物。
const ALWAYS_IGNORED_SUFFIXES: &[&str] = &[
    // 编译产物
    ".pyc", ".pyo", ".o", ".obj", ".so", ".dylib", ".dll", ".exe", ".lib", ".a", ".class", ".jar",
    ".war", ".ko", ".rlib", ".rmeta", ".pdb", ".res", // 图片 / 字体 / 媒体
    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".avif", ".icns", ".ttf", ".otf",
    ".woff", ".woff2", ".eot", ".mp3", ".mp4", ".avi", ".mov", ".mkv", ".wav", ".flac", ".ogg",
    ".webm", // 数据库 / 二进制文档
    ".db", ".sqlite", ".sqlite3", ".mdb", ".wasm", ".pdf", ".doc", ".docx", ".xls", ".xlsx",
    ".ppt", ".pptx", // 压缩包
    ".zip", ".gz", ".bz2", ".xz", ".7z", ".rar", ".tar", ".tgz", ".zst",
    // 生成物与临时文件
    ".min.js", ".min.css", ".map", ".snap", ".bak", ".tmp", ".swp", ".orig", ".rej", ".mo",
];

/// 忽略的配置类文件全名（锁文件/构建配置噪声大、无符号价值；对齐参考项目
/// IGNORED_JSON_FILES 思路）。
const IGNORED_FILENAMES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "poetry.lock",
    "pipfile.lock",
    "flake.lock",
    "gemfile.lock",
    "packages.lock.json",
    "project.assets.json",
];

fn is_ignored_suffix(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ALWAYS_IGNORED_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

fn is_ignored_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    IGNORED_FILENAMES.iter().any(|f| lower == *f)
}

fn file_mtime_ns(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 收集仓库工作区中参与索引的文件列表。结果按 rel_path 排序保证确定性
/// （增量分类与测试断言都依赖稳定顺序）。
///
/// 目录在遍历时整棵剪枝：跳过表命中，或目录内含 `.git` 文件（子模块 /
/// linked worktree 标记——子模块有自己的索引域，避免重复索引）。
pub fn discover_files(repo_root: &Path) -> Result<DiscoverOutcome> {
    let mut outcome = DiscoverOutcome::default();
    let root = repo_root.to_path_buf();
    let pruned_dirs = Arc::new(AtomicUsize::new(0));
    let pruned_dirs_in_filter = Arc::clone(&pruned_dirs);

    let walker = WalkBuilder::new(repo_root)
        .hidden(true) // 跳过隐藏项；.github 等隐藏目录损失可接受，换来 .git 等零成本剪枝
        .filter_entry(move |entry| {
            let path = entry.path();
            if path == root.as_path() {
                return true;
            }
            let Some(file_type) = entry.file_type() else {
                return true;
            };
            if !file_type.is_dir() {
                return true;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let prune = ALWAYS_SKIP_DIRS
                .iter()
                .any(|d| name.eq_ignore_ascii_case(d))
                // 子模块 / linked worktree：`.git` 是文件而非目录。
                || path.join(".git").is_file();
            if prune {
                pruned_dirs_in_filter.fetch_add(1, Ordering::Relaxed);
            }
            !prune
        })
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let abs_path = entry.path().to_path_buf();
        let Some(rel) = abs_path
            .strip_prefix(repo_root)
            .ok()
            .and_then(|p| p.to_str())
        else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        // Windows 下统一正斜杠相对路径，与 diff 视图/file_hashes 口径一致。
        let rel_path = rel.replace('\\', "/");
        let name = abs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if is_ignored_suffix(&name) || is_ignored_filename(&name) || name.starts_with(".git") {
            outcome.excluded_count += 1;
            continue;
        }

        let Ok(meta) = std::fs::metadata(&abs_path) else {
            continue;
        };

        outcome.files.push(DiscoveredFile {
            rel_path,
            abs_path,
            size: meta.len(),
            mtime_ns: file_mtime_ns(&meta),
        });

        if outcome.files.len() > MAX_INDEX_FILES {
            return Err(err(format!(
                "仓库文件数超过索引上限 {} 个，已终止索引",
                MAX_INDEX_FILES
            )));
        }
    }

    outcome.excluded_count += pruned_dirs.load(Ordering::Relaxed);
    outcome.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(outcome)
}
