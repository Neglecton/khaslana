//! 从目标提交的 Git 对象构建独立索引，不 checkout，也不读取工作区源码。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use git2::{ObjectType, TreeWalkMode, TreeWalkResult};

use super::coverage::{FileCoverage, attach_discovery_issues};
use super::discover::{DiscoveredFile, excluded_name};
use super::graph::GraphBuffer;
use super::pipeline::{
    GraphMerger, ParseOutput, PipelineOptions, build_structure_pass, parse_content,
};
use super::store::{CodeIndexMeta, CodeIndexStore, open_read_only_if_exists};
use super::{Extractor, MAX_INDEX_FILES, PARSE_MAX_BYTES, err};
use crate::types::Result;

pub fn commit_index_path(data_dir: &Path, repo_path: &Path, commit: &str) -> Result<PathBuf> {
    let oid = git2::Oid::from_str(commit).map_err(|error| err(format!("评审提交无效：{error}")))?;
    let path = data_dir
        .join("code-index-snapshots")
        .join(crate::ai::review_store::repo_key(
            &repo_path.to_string_lossy(),
        ))
        .join(oid.to_string())
        .join("index.db");
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|error| err(format!("创建评审索引目录失败：{error}")))?;
    Ok(path)
}

pub fn ensure_commit_index(
    repo_path: &Path,
    commit: &str,
    db_path: &Path,
    cancelled: &AtomicBool,
) -> Result<bool> {
    if cancelled.load(Ordering::Relaxed) { return Ok(false); }
    // 已有快照的只读复用不必排队等待其他仓库的全量任务。
    if commit_index_ready(repo_path, commit, db_path)? { return Ok(!cancelled.load(Ordering::Relaxed)); }
    // 同提交并发评审共用建库锁；只串行化快照构建，不占用 Git 工作区锁。
    let lock = super::jobs::database_lock(db_path);
    super::jobs::index_task_pool().install(|| {
        let _guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Err(err("该提交的评审索引正在构建，请稍后重试")),
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let _process_guard = super::jobs::database_process_lock(db_path)?;
        if commit_index_ready(repo_path, commit, db_path)? { return Ok(!cancelled.load(Ordering::Relaxed)); }
        build_commit_index(repo_path, commit, db_path, cancelled)
    })
}

fn commit_index_ready(repo_path: &Path, commit: &str, db_path: &Path) -> Result<bool> {
    if let Some(store) = open_read_only_if_exists(db_path)? {
        let indexed: String = store
        .conn
        .query_row("SELECT value FROM meta WHERE key='branch'", [], |row| {
            row.get(0)
        })
        .unwrap_or_default();
        let source: String = store
        .conn
        .query_row("SELECT value FROM meta WHERE key='repo_path'", [], |row| {
            row.get(0)
        })
        .unwrap_or_default();
        let mode: String = store
        .conn
        .query_row("SELECT value FROM meta WHERE key='mode'", [], |row| {
            row.get(0)
        })
        .unwrap_or_default();
        if indexed == commit
        && source == repo_path.to_string_lossy()
        && mode == "commit_snapshot"
        && store.has_search_metadata()
        && store.retry_files()?.is_empty()
        {
        return Ok(true);
        }
    }
    Ok(false)
}

fn build_commit_index(
    repo_path: &Path,
    commit: &str,
    db_path: &Path,
    cancelled: &AtomicBool,
) -> Result<bool> {
    let repo = git2::Repository::open(repo_path)?;
    let oid = git2::Oid::from_str(commit).map_err(|error| err(format!("评审提交无效：{error}")))?;
    let tree = repo.find_commit(oid)?.tree()?;
    let mut files = Vec::new();
    let mut parsed = Vec::new();
    let mut issues = Vec::new();
    let mut extractor = Extractor::new();
    let mut limit_hit = false;
    let walked = tree.walk(TreeWalkMode::PreOrder, |prefix, entry| {
        if cancelled.load(Ordering::Relaxed) {
            return TreeWalkResult::Abort;
        }
        let Ok(name) = entry.name() else {
            issues.push(FileCoverage::new(prefix, "excluded", "非 UTF-8 路径"));
            return TreeWalkResult::Skip;
        };
        let path = format!("{prefix}{name}");
        let directory = entry.kind() == Some(ObjectType::Tree);
        if excluded_name(name, directory)
            || entry.filemode() == 0o120000
            || entry.kind() == Some(ObjectType::Commit)
        {
            issues.push(FileCoverage::new(
                &path,
                "excluded",
                "目录或产物过滤、符号链接、子模块",
            ));
            return if directory {
                TreeWalkResult::Skip
            } else {
                TreeWalkResult::Ok
            };
        }
        if entry.kind() != Some(ObjectType::Blob) {
            return TreeWalkResult::Ok;
        }
        if files.len() >= MAX_INDEX_FILES {
            limit_hit = true;
            return TreeWalkResult::Abort;
        }
        let size = match repo.odb().and_then(|odb| odb.read_header(entry.id())) {
            Ok((size, _)) => size as u64,
            Err(error) => {
                parsed.push(ParseOutput {
                    rel_path: path.clone(),
                    result: None,
                    line_count: 0,
                    coverage: FileCoverage::new(&path, "read_failed", error.to_string()),
                });
                files.push(DiscoveredFile {
                    rel_path: path,
                    abs_path: PathBuf::new(),
                    size: 0,
                    mtime_ns: 0,
                });
                return TreeWalkResult::Ok;
            }
        };
        let file = DiscoveredFile {
            rel_path: path.clone(),
            abs_path: PathBuf::new(),
            size,
            mtime_ns: 0,
        };
        if size <= PARSE_MAX_BYTES && super::graph::lang_of_rel_path(&path).is_some() {
            match repo.find_blob(entry.id()) {
                Ok(blob) => parsed.push(parse_content(&file, blob.content(), &mut extractor)),
                Err(error) => parsed.push(ParseOutput {
                    rel_path: path.clone(),
                    result: None,
                    line_count: 0,
                    coverage: FileCoverage::new(&path, "read_failed", error.to_string()),
                }),
            }
        }
        files.push(file);
        TreeWalkResult::Ok
    });
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if limit_hit {
        return Err(err(format!("评审索引文件数超过 {MAX_INDEX_FILES} 个")));
    }
    walked?;
    let project = repo_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let mut graph = GraphBuffer::new();
    let file_refs = files.iter().collect::<Vec<_>>();
    build_structure_pass(project, commit, &file_refs, &mut graph);
    let mut merger = GraphMerger::new(project);
    merger.merge_parsed(&mut graph, parsed);
    let mut options = PipelineOptions::new(
        std::sync::Arc::new(AtomicBool::new(false)),
        Box::new(|_| {}),
    );
    merger.resolve_pending_cancellable(&mut graph, &mut options, Some(cancelled))?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    attach_discovery_issues(&mut graph, &issues);
    drop(merger);
    graph.release_build_indexes();
    let mut store = CodeIndexStore::open(db_path)?;
    if !store.replace_all_cancellable(
        &graph,
        &[],
        &CodeIndexMeta {
            repo_name: project.into(),
            repo_path: repo_path.to_string_lossy().into_owned(),
            branch: commit.into(),
            mode: "commit_snapshot".into(),
            ..Default::default()
        },
        Some(cancelled),
    )? {
        return Ok(false);
    }
    Ok(true)
}
