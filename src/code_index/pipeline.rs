//! 索引管线编排（参照 codebase-memory-mcp 的 `pipeline/pipeline.c` 与
//! `pipeline_incremental.c`）。
//!
//! 全量：discover → 结构 pass（Project/Branch/Folder/File）→ 提取 pass
//! （并行，worker 各持独立 Extractor，连续分块免锁保序）→ 合并进图缓冲 →
//! 解析 pass（registry 策略链）→ 整库落盘。
//!
//! 增量路由与参考项目一致：已有库且 文件数 ≤ 已存哈希数 × 1.5 走增量，
//! 否则全量。增量 = mtime+size 三分类 → 入边快照 → 按文件清除 → 生成单批文件
//! 事实快照（图只合并变更文件）→ 重链接快照边 → 整库重写。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::discover::{DiscoveredFile, discover_files};
use super::extract::{Extractor, FileExtractResult};
use super::facts::{CallFact, DiagnosticFact, FileFacts, SnapshotFacts};
use super::graph::{
    EdgeType, GraphBuffer, NodeId, NodeLabel, calls_edge_properties, file_properties,
    file_qualified_name, folder_qualified_name, lang_of_rel_path,
};
use super::identity::content_fingerprint;
use super::java::{JavaProjectModel, JavaSourceScope};
use super::resolve::Registry;
use super::store::{CodeIndexMeta, CodeIndexStore, FileHashRow};
use super::{IndexPhase, IndexProgress, err};
use crate::types::Result;

#[derive(Clone, Debug, Default)]
pub struct IndexRunStats {
    pub files: usize,
    pub symbols: usize,
    pub edges: usize,
    pub calls: usize,
    pub duration_ms: u64,
}

pub struct PipelineOptions {
    /// 取消标志：在文件边界与阶段边界检查；取消后不落盘（增量保留旧库）。
    pub cancel: Arc<AtomicBool>,
    /// 进度回调（节流由管线内部保证：阶段切换或每 50 文件）。
    pub progress: Box<dyn FnMut(IndexProgress) + Send>,
}

impl PipelineOptions {
    pub fn new(cancel: Arc<AtomicBool>, progress: Box<dyn FnMut(IndexProgress) + Send>) -> Self {
        Self { cancel, progress }
    }

    fn report(&mut self, phase: IndexPhase, done: usize, total: usize) {
        let message = match phase {
            IndexPhase::Discover => format!("发现 {total} 个文件"),
            _ => format!("{} {}/{}", phase.display(), done, total),
        };
        (self.progress)(IndexProgress {
            phase,
            done,
            total,
            message,
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub enum RunOutcome {
    Completed(IndexRunStats),
    /// 增量检查后无任何变化，未写库。
    Unchanged,
    Cancelled,
}

/// 增量专用结果：无变化时零写入快速返回。
#[derive(Debug)]
pub enum IncrementalOutcome {
    NoChange,
    Updated(IndexRunStats),
}

/// 索引入口（对齐参考项目 `cbm_pipeline_run` 的内部增量路由）：根据库的现状
/// 自动选择全量或增量。`force_full` 为 true 时跳过增量判断。
pub fn run_index(
    repo_root: &Path,
    db_path: &Path,
    force_full: bool,
    options: &mut PipelineOptions,
) -> Result<RunOutcome> {
    let started = Instant::now();
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();
    let branch = detect_branch(repo_root);

    options.report(IndexPhase::Discover, 0, 0);
    let outcome = discover_files(repo_root)?;
    let excluded_count = outcome.excluded_count;
    let files = outcome.files;
    let total_files = files.len();
    if options.cancelled() {
        return Ok(RunOutcome::Cancelled);
    }

    let refs: Vec<&DiscoveredFile> = files.iter().collect();
    let mut store = match CodeIndexStore::open(db_path) {
        Ok(store) => store,
        Err(error) if force_full && error.to_string().contains("NeedsRebuild") => {
            // 旧库只读探测成功后，先在内存完成全部提取；取消/失败均不会触碰 v2。
            let Some((mut graph, parsed)) = build_full_graph(&repo_name, &branch, &refs, options)?
            else {
                return Ok(RunOutcome::Cancelled);
            };
            graph.prune_orphan_modules();
            let meta = index_meta(repo_root, &repo_name, &branch, "rebuild");
            let hashes = files_hash_rows(&parsed);
            let project_model = java_project_model(&parsed);
            let facts = snapshot_from_parsed(
                &repo_name,
                &parsed,
                &project_model,
                excluded_count,
                now_millis(),
            );
            CodeIndexStore::rebuild_incompatible_with_facts(
                db_path, &graph, &hashes, &facts, &meta,
            )?;
            let store = CodeIndexStore::open(db_path)?;
            let stats = store.read_stats()?.unwrap_or_default();
            return Ok(RunOutcome::Completed(IndexRunStats {
                files: stats.files,
                symbols: stats.symbols,
                edges: stats.edges,
                calls: stats.calls,
                duration_ms: started.elapsed().as_millis() as u64,
            }));
        }
        Err(error) => return Err(error),
    };

    // 增量路由（参考项目 try_incremental_or_delete_db 的判定式）。
    let existing_hashes = store.load_file_hashes()?;
    let incremental_eligible = !force_full
        && !existing_hashes.is_empty()
        && total_files as f64 <= existing_hashes.len() as f64 * 1.5;
    let inner = if incremental_eligible {
        run_incremental_inner(
            repo_root,
            &repo_name,
            &branch,
            &refs,
            &existing_hashes,
            excluded_count,
            &mut store,
            options,
        )?
    } else {
        run_full_inner(
            repo_root,
            &repo_name,
            &branch,
            &refs,
            excluded_count,
            &mut store,
            options,
        )?
    };

    match inner {
        InnerOutcome::Cancelled => Ok(RunOutcome::Cancelled),
        InnerOutcome::NoChange => Ok(RunOutcome::Unchanged),
        InnerOutcome::Done => {
            let Some(stats) = store.read_stats()? else {
                return Ok(RunOutcome::Completed(IndexRunStats {
                    files: total_files,
                    duration_ms: started.elapsed().as_millis() as u64,
                    ..Default::default()
                }));
            };
            Ok(RunOutcome::Completed(IndexRunStats {
                files: stats.files,
                symbols: stats.symbols,
                edges: stats.edges,
                calls: stats.calls,
                duration_ms: started.elapsed().as_millis() as u64,
            }))
        }
    }
}

/// 增量便捷入口（仓库打开后的自动刷新）：无变化零写入。
pub fn run_incremental_if_stale(
    repo_root: &Path,
    db_path: &Path,
    options: &mut PipelineOptions,
) -> Result<IncrementalOutcome> {
    match run_index(repo_root, db_path, false, options)? {
        RunOutcome::Completed(stats) => Ok(IncrementalOutcome::Updated(stats)),
        RunOutcome::Unchanged | RunOutcome::Cancelled => Ok(IncrementalOutcome::NoChange),
    }
}

enum InnerOutcome {
    Done,
    NoChange,
    Cancelled,
}

fn detect_branch(repo_root: &Path) -> String {
    git2::Repository::open(repo_root)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|head| head.shorthand().ok().map(str::to_string))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// 全量
// ---------------------------------------------------------------------------

fn run_full_inner(
    repo_root: &Path,
    repo_name: &str,
    branch: &str,
    files: &[&DiscoveredFile],
    excluded_count: usize,
    store: &mut CodeIndexStore,
    options: &mut PipelineOptions,
) -> Result<InnerOutcome> {
    let baseline_generation = store.generation()?;
    let Some((mut graph, parsed)) = build_full_graph(repo_name, branch, files, options)? else {
        return Ok(InnerOutcome::Cancelled);
    };

    options.report(IndexPhase::Write, 0, 0);
    // 全量图为全新构建，Module 恒有 IMPORTS 入边；清扫仅作防御（零成本）。
    graph.prune_orphan_modules();
    write_store(
        store,
        repo_root,
        repo_name,
        branch,
        "full",
        &graph,
        files_hash_rows(&parsed),
        snapshot_from_parsed(
            repo_name,
            &parsed,
            &java_project_model(&parsed),
            excluded_count,
            now_millis(),
        ),
        baseline_generation,
    )?;
    Ok(InnerOutcome::Done)
}

fn build_full_graph(
    repo_name: &str,
    branch: &str,
    files: &[&DiscoveredFile],
    options: &mut PipelineOptions,
) -> Result<Option<(GraphBuffer, Vec<ParseOutput>)>> {
    let mut graph = GraphBuffer::new();
    build_structure_pass(repo_name, branch, files, &mut graph);
    let parsed = run_extraction_pass(files, options)?;
    if options.cancelled() {
        return Ok(None);
    }
    let mut merger = GraphMerger::new(repo_name, java_project_model(&parsed));
    merger.merge_parsed(&mut graph, &parsed);
    merger.resolve_pending(&mut graph, options)?;
    if options.cancelled() {
        return Ok(None);
    }
    Ok(Some((graph, parsed)))
}

fn files_hash_rows(parsed: &[ParseOutput]) -> Vec<FileHashRow> {
    parsed
        .iter()
        .filter_map(|output| {
            Some(FileHashRow {
                rel_path: output.rel_path.clone(),
                sha256: output.sha256.clone()?,
                mtime_ns: output.mtime_ns,
                size: output.size,
            })
        })
        .collect()
}

fn write_store(
    store: &mut CodeIndexStore,
    repo_root: &Path,
    repo_name: &str,
    branch: &str,
    mode: &str,
    graph: &GraphBuffer,
    hashes: Vec<FileHashRow>,
    facts: SnapshotFacts,
    baseline_generation: u64,
) -> Result<()> {
    let meta = index_meta(repo_root, repo_name, branch, mode);
    store.replace_all_with_facts_at_generation(graph, &hashes, &facts, &meta, baseline_generation)
}

fn index_meta(repo_root: &Path, repo_name: &str, branch: &str, mode: &str) -> CodeIndexMeta {
    CodeIndexMeta {
        repo_name: repo_name.to_string(),
        repo_path: repo_root.to_string_lossy().to_string(),
        branch: branch.to_string(),
        indexed_at: now_millis(),
        duration_ms: 0,
        mode: mode.to_string(),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 增量
// ---------------------------------------------------------------------------

fn run_incremental_inner(
    repo_root: &Path,
    repo_name: &str,
    branch: &str,
    files: &[&DiscoveredFile],
    existing_hashes: &[FileHashRow],
    excluded_count: usize,
    store: &mut CodeIndexStore,
    options: &mut PipelineOptions,
) -> Result<InnerOutcome> {
    let baseline_generation = store.generation()?;
    let old_by_path: HashMap<&str, &FileHashRow> = existing_hashes
        .iter()
        .map(|h| (h.rel_path.as_str(), h))
        .collect();
    let new_paths: HashSet<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();

    // 三分类（对齐参考项目 pipeline_incremental.c）：changed = 新增或
    // mtime/size 变化；unchanged 原样保留；deleted-vs-mode-skipped 里盘上
    // 仍在的（权限/过滤规则变化导致本次未发现）保守保留其哈希行。
    let mut changed: Vec<&DiscoveredFile> = Vec::new();
    let mut unchanged_hashes: Vec<FileHashRow> = Vec::new();
    for file in files {
        match old_by_path.get(file.rel_path.as_str()) {
            Some(old) if old.mtime_ns == file.mtime_ns && old.size == file.size => {
                unchanged_hashes.push((*old).clone());
            }
            _ => changed.push(file),
        }
    }
    let mut deleted_paths: HashSet<String> = HashSet::new();
    for h in existing_hashes {
        if !new_paths.contains(h.rel_path.as_str()) {
            if repo_root.join(&h.rel_path).exists() {
                unchanged_hashes.push(h.clone());
            } else {
                deleted_paths.insert(h.rel_path.clone());
            }
        }
    }

    if changed.is_empty() && deleted_paths.is_empty() {
        return Ok(InnerOutcome::NoChange);
    }
    if options.cancelled() {
        return Ok(InnerOutcome::Cancelled);
    }

    options.report(IndexPhase::Parse, 0, changed.len());

    // 1. 整图载入 RAM。
    let mut graph = store.load_graph()?;

    // 2. 入边快照：target 在待清除文件、source 在幸存节点的跨文件边
    //    （级联删除会连带清掉这些边，先按 QN 键控捕获，重解析后恢复）。
    let purge_set: HashSet<String> = changed
        .iter()
        .map(|f| f.rel_path.clone())
        .chain(deleted_paths.iter().cloned())
        .collect();
    let mut inbound_snapshot: Vec<(String, String, EdgeType, String)> = Vec::new();
    for edge in &graph.edges {
        let src_file = &graph.get(edge.source).file_path;
        let tgt_file = &graph.get(edge.target).file_path;
        let src_survives = src_file.is_empty() || !purge_set.contains(src_file);
        if src_survives && !tgt_file.is_empty() && purge_set.contains(tgt_file) {
            inbound_snapshot.push((
                graph.get(edge.source).qualified_name.clone(),
                graph.get(edge.target).qualified_name.clone(),
                edge.etype,
                edge.properties.clone(),
            ));
        }
    }

    // 3. 按文件清除（级联删边 + 幸存节点 id 重排）。
    graph.purge_files(&purge_set);

    // 4. 全部发现文件只读、只解析一次，形成同一代际的不可变事实快照；
    //    兼容图只合并变更子集，避免重复创建未变符号。
    let all_parsed = run_extraction_pass(files, options)?;
    let changed_paths: HashSet<&str> = changed.iter().map(|file| file.rel_path.as_str()).collect();
    let parsed: Vec<ParseOutput> = all_parsed
        .iter()
        .filter(|output| changed_paths.contains(output.rel_path.as_str()))
        .cloned()
        .collect();
    if options.cancelled() {
        return Ok(InnerOutcome::Cancelled);
    }

    // 5. 结构补建（新文件的 Folder/File 节点可能不存在；upsert 幂等）。
    build_structure_pass(repo_name, branch, &changed, &mut graph);

    let project_model = java_project_model(&all_parsed);
    let mut merger = GraphMerger::new(repo_name, project_model.clone());
    merger.merge_parsed(&mut graph, &parsed);
    merger.resolve_pending(&mut graph, options)?;
    if options.cancelled() {
        return Ok(InnerOutcome::Cancelled);
    }

    // 6. 重链接快照入边（add_edge 三元组去重，幂等）。
    for (src_qn, tgt_qn, etype, props) in &inbound_snapshot {
        if let (Some(src), Some(tgt)) = (graph.find_by_qn(src_qn), graph.find_by_qn(tgt_qn)) {
            graph.add_edge(src, tgt, *etype, props.clone());
        }
    }

    // 7. 清扫孤儿 Module：删除/改写导入语句后不再被任何文件 IMPORTS 的
    //    模块（Module 无 file_path，purge_files 清不到；参考项目靠整图
    //    重建播种 registry 天然无此残留）。
    graph.prune_orphan_modules();

    // 8. 整库重写。
    options.report(IndexPhase::Write, 0, 0);
    let hashes = files_hash_rows(&all_parsed);
    write_store(
        store,
        repo_root,
        repo_name,
        branch,
        "incremental",
        &graph,
        hashes,
        snapshot_from_parsed(
            repo_name,
            &all_parsed,
            &project_model,
            excluded_count,
            now_millis(),
        ),
        baseline_generation,
    )?;
    Ok(InnerOutcome::Done)
}

// ---------------------------------------------------------------------------
// 结构 pass（参照 pass_structure.c）
// ---------------------------------------------------------------------------

fn build_structure_pass(
    project: &str,
    branch: &str,
    files: &[&DiscoveredFile],
    graph: &mut GraphBuffer,
) {
    let project_id = graph.upsert_node(
        NodeLabel::Project,
        project,
        project.to_string(),
        "",
        0,
        0,
        "{}".to_string(),
    );
    let branch_id = graph.upsert_node(
        NodeLabel::Branch,
        branch,
        format!("{project}.branch.{branch}"),
        "",
        0,
        0,
        "{}".to_string(),
    );
    graph.add_edge(project_id, branch_id, EdgeType::HasBranch, "{}".to_string());

    // 目录链去重并按路径排序，保证 Folder 创建顺序父先于子且确定。
    let mut dirs: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let dir = parent_dir(&f.rel_path);
            (!dir.is_empty()).then_some(dir)
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    let mut dir_ids: HashMap<String, NodeId> = HashMap::new();
    for dir in dirs {
        let qn = folder_qualified_name(project, &dir);
        let name = dir.rsplit('/').next_back().unwrap_or(&dir).to_string();
        let id = graph.upsert_node(
            NodeLabel::Folder,
            name,
            qn.clone(),
            "",
            0,
            0,
            "{}".to_string(),
        );
        let parent_qn = match dir.rsplit_once('/') {
            Some((parent, _)) => folder_qualified_name(project, parent),
            None => project.to_string(),
        };
        let parent_id = graph.find_by_qn(&parent_qn).unwrap_or(project_id);
        graph.add_edge(parent_id, id, EdgeType::ContainsFolder, "{}".to_string());
        dir_ids.insert(dir, id);
    }

    for f in files {
        let qn = file_qualified_name(project, &f.rel_path);
        let name = f
            .rel_path
            .rsplit('/')
            .next_back()
            .unwrap_or(&f.rel_path)
            .to_string();
        let file_id = graph.upsert_node(
            NodeLabel::File,
            name,
            qn,
            f.rel_path.clone(),
            0,
            0,
            "{}".to_string(),
        );
        let dir = parent_dir(&f.rel_path);
        let parent_id = if dir.is_empty() {
            project_id
        } else {
            graph
                .find_by_qn(&folder_qualified_name(project, &dir))
                .unwrap_or(project_id)
        };
        graph.add_edge(parent_id, file_id, EdgeType::ContainsFile, "{}".to_string());
    }
}

fn parent_dir(rel_path: &str) -> String {
    match rel_path.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 提取 pass（并行，参照 pass_parallel.c 阶段 3A）
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ParseOutput {
    rel_path: String,
    result: Option<FileExtractResult>,
    line_count: usize,
    sha256: Option<String>,
    mtime_ns: u64,
    size: u64,
    parse_status: &'static str,
    diagnostic: Option<DiagnosticFact>,
    project_text: Option<String>,
}

/// 并行提取：worker 数 = 可用核数-1 钳到 [1,6]，每个 worker 独立持有
/// Extractor/Parser 实例，处理一段连续分块（免锁、输出按块序拼接保序）。
/// 取消在文件边界生效，取消后返回已完成的分块由调用方丢弃。
fn run_extraction_pass(
    jobs: &[&DiscoveredFile],
    options: &mut PipelineOptions,
) -> Result<Vec<ParseOutput>> {
    let total = jobs.len();
    options.report(IndexPhase::Parse, 0, total);
    if total == 0 {
        return Ok(Vec::new());
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .saturating_sub(1)
        .clamp(1, 6);
    let chunk_size = total.div_ceil(workers);
    let cancel = Arc::clone(&options.cancel);

    let chunks: Result<Vec<Vec<ParseOutput>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk_size.max(1))
            .map(|chunk| {
                let cancel = Arc::clone(&cancel);
                scope.spawn(move || {
                    let mut extractor = Extractor::new();
                    let mut out = Vec::with_capacity(chunk.len());
                    for file in chunk {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        out.push(parse_one(file, &mut extractor));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| err("代码索引解析线程异常退出，已放弃本次发布"))
            })
            .collect()
    });
    let chunks = chunks?;

    let done: usize = chunks.iter().map(Vec::len).sum();
    let mut outputs: Vec<ParseOutput> = chunks.into_iter().flatten().collect();
    outputs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    options.report(IndexPhase::Parse, done.min(total), total);
    Ok(outputs)
}

fn parse_one(file: &DiscoveredFile, extractor: &mut Extractor) -> ParseOutput {
    let mut output = ParseOutput {
        rel_path: file.rel_path.clone(),
        result: None,
        line_count: 0,
        sha256: None,
        mtime_ns: file.mtime_ns,
        size: file.size,
        parse_status: "error",
        diagnostic: None,
        project_text: None,
    };
    let bytes = match std::fs::read(&file.abs_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            output.diagnostic = Some(DiagnosticFact {
                rel_path: file.rel_path.clone(),
                adapter: "source_reader".into(),
                code: "unreadable".into(),
                detail: error.to_string(),
                range_json: "{}".into(),
            });
            return output;
        }
    };
    output.size = bytes.len() as u64;
    output.mtime_ns = std::fs::metadata(&file.abs_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(file.mtime_ns);
    output.sha256 = Some(content_fingerprint(&bytes));
    if is_java_project_file(&file.rel_path) {
        output.project_text = std::str::from_utf8(&bytes).ok().map(str::to_string);
    }
    let Some(lang) = lang_of_rel_path(&file.rel_path) else {
        if output.project_text.is_some() {
            output.parse_status = "ok";
            return output;
        }
        output.parse_status = "skipped";
        output.diagnostic = Some(DiagnosticFact {
            rel_path: file.rel_path.clone(),
            adapter: "generic".into(),
            code: "unsupported_language".into(),
            detail: "没有对应 tree-sitter 适配器".into(),
            range_json: "{}".into(),
        });
        return output;
    };
    if output.size > super::PARSE_MAX_BYTES {
        output.parse_status = "skipped";
        output.diagnostic = Some(DiagnosticFact {
            rel_path: file.rel_path.clone(),
            adapter: "generic".into(),
            code: "budget_exceeded".into(),
            detail: format!("文件超过 {} 字节解析上限", super::PARSE_MAX_BYTES),
            range_json: "{}".into(),
        });
        return output;
    }
    // 二进制嗅探：前 8KB 出现 NUL 视为二进制（与项目内其他嗅探口径一致）。
    let sniff_len = bytes.len().min(8192);
    if bytes[..sniff_len].contains(&0) {
        output.parse_status = "skipped";
        output.diagnostic = Some(DiagnosticFact {
            rel_path: file.rel_path.clone(),
            adapter: "source_reader".into(),
            code: "binary".into(),
            detail: "前 8KB 含 NUL".into(),
            range_json: "{}".into(),
        });
        return output;
    }
    output.line_count = byte_line_count(&bytes);
    match extractor.extract(lang, &bytes) {
        Ok(Some(result)) => {
            if result.has_error {
                output.parse_status = "partial";
                output.diagnostic = Some(DiagnosticFact {
                    rel_path: file.rel_path.clone(),
                    adapter: "generic".into(),
                    code: "parse_partial".into(),
                    detail: "tree-sitter 语法树含 ERROR 节点，已保留可提取事实".into(),
                    range_json: "{}".into(),
                });
            } else {
                output.parse_status = "ok";
            }
            output.result = Some(result);
        }
        Ok(None) => {
            output.diagnostic = Some(DiagnosticFact {
                rel_path: file.rel_path.clone(),
                adapter: "generic".into(),
                code: "parse_error".into(),
                detail: "tree-sitter 未返回语法树".into(),
                range_json: "{}".into(),
            })
        }
        Err(error) => {
            output.diagnostic = Some(DiagnosticFact {
                rel_path: file.rel_path.clone(),
                adapter: "generic".into(),
                code: "parse_error".into(),
                detail: error.to_string(),
                range_json: "{}".into(),
            })
        }
    }
    output
}

/// 事实快照独立于兼容图构建。即使某调用无法解析也逐点落盘；全量和增量均
/// 直接消费本轮唯一的 ParseOutput 批次，不再二次读盘或解析。
fn snapshot_from_parsed(
    project: &str,
    parsed: &[ParseOutput],
    project_model: &JavaProjectModel,
    excluded_count: usize,
    extracted_at: u64,
) -> SnapshotFacts {
    let mut snapshot = SnapshotFacts {
        files_discovered: parsed.len() as u64 + excluded_count as u64,
        excluded_count: excluded_count as u64,
        ..Default::default()
    };
    if excluded_count > 0 {
        snapshot.diagnostics.push(DiagnosticFact {
            rel_path: String::new(),
            adapter: "discover".into(),
            code: "excluded".into(),
            detail: format!("发现阶段排除 {excluded_count} 项"),
            range_json: "{}".into(),
        });
    }
    for output in parsed {
        if let Some(diagnostic) = &output.diagnostic {
            snapshot.diagnostics.push(diagnostic.clone());
        }
        let Some(sha256) = output.sha256.clone() else {
            continue;
        };
        let (definitions, imports, calls) = if let Some(result) = &output.result {
            let definitions = result.defs.iter().map(|def| serde_json::json!({"name":def.name,"kind":def.label.as_str(),"scope":def.scope,"signature":def.signature,"start_byte":def.start_byte,"end_byte":def.end_byte,"start_line":def.start_line,"end_line":def.end_line,"java":def.java})).collect();
            let imports = result
                .imports
                .iter()
                .map(|import| import.module.clone())
                .collect();
            let calls = result
                .calls
                .iter()
                .map(|call| {
                    let owner_qn = call.owner.as_ref().map(|owner| {
                        let mut qn = file_qualified_name(project, &output.rel_path);
                        if !owner.class_chain.is_empty() {
                            qn.push('.');
                            qn.push_str(&owner.class_chain.join("."));
                        }
                        qn.push('.');
                        qn.push_str(&owner.fn_name);
                        qn.push_str(&owner.signature);
                        qn
                    });
                    snapshot.calls.push(CallFact {
                        owner_qn: owner_qn.clone(),
                        rel_path: output.rel_path.clone(),
                        sha256: sha256.clone(),
                        start_byte: call.start_byte,
                        end_byte: call.end_byte,
                        start_line: call.line,
                        end_line: call.end_line,
                        receiver_expr: call.receiver_expr.clone(),
                        callee_expr: call.callee_display.clone(),
                        argument_exprs: call.argument_exprs.clone(),
                        resolution: "unresolved".into(),
                        target_qn: None,
                        detail: "generic resolver 未证明目标".into(),
                    });
                    serde_json::json!({"owner":owner_qn,"callee":call.callee_display,"start_byte":call.start_byte,"end_byte":call.end_byte,"start_line":call.line,"end_line":call.end_line,"receiver":call.receiver_expr,"arguments":call.argument_exprs})
                })
                .collect();
            (definitions, imports, calls)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        snapshot.files.push(FileFacts::from_json(
            output.rel_path.clone(),
            sha256,
            output.parse_status,
            serde_json::json!({
                "definitions":definitions,
                "imports":imports,
                "calls":calls,
                "java":result_java_json(output, project_model),
            }),
            extracted_at,
        ));
    }
    for diagnostic in &project_model.diagnostics {
        snapshot.diagnostics.push(DiagnosticFact {
            rel_path: diagnostic.rel_path.clone(),
            adapter: "java-project".into(),
            code: diagnostic.code.clone(),
            detail: diagnostic.detail.clone(),
            range_json: "{}".into(),
        });
    }
    snapshot
}

fn java_project_model(parsed: &[ParseOutput]) -> JavaProjectModel {
    JavaProjectModel::from_sources(parsed.iter().filter_map(|output| {
        output
            .project_text
            .as_deref()
            .map(|text| (output.rel_path.as_str(), text))
    }))
}

fn is_java_project_file(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    matches!(
        name.to_ascii_lowercase().as_str(),
        "pom.xml" | "settings.gradle" | "settings.gradle.kts" | "build.gradle" | "build.gradle.kts"
    )
}

fn result_java_json(output: &ParseOutput, project_model: &JavaProjectModel) -> serde_json::Value {
    let Some(result) = output.result.as_ref() else {
        return serde_json::Value::Null;
    };
    let Some(java) = result.java.as_ref() else {
        return serde_json::Value::Null;
    };
    let scope = project_model.scope_for(&output.rel_path);
    serde_json::json!({"scope":scope,"facts":java})
}

fn definition_properties(
    def: &super::extract::SymbolDef,
    source_scope: &JavaSourceScope,
    package: &str,
) -> String {
    let Some(java) = def.java.as_ref() else {
        return "{}".to_string();
    };
    serde_json::to_string(&serde_json::json!({
        "java": {
            "module_key": source_scope.module_key,
            "source_set": source_scope.source_set,
            "source_root": source_scope.source_root,
            "package": package,
            "symbol": java,
        }
    }))
    .expect("Java 节点属性可序列化")
}

fn byte_line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    bytes.iter().filter(|&&b| b == b'\n').count() + usize::from(*bytes.last().unwrap() != b'\n')
}

// ---------------------------------------------------------------------------
// 合并与解析（参照 registry 构建 + calls pass）
// ---------------------------------------------------------------------------

/// 待解析调用点：source 已定位（函数优先、文件兜底），目标待 registry 解析。
struct PendingCall {
    source: NodeId,
    source_file: String,
    imports: Vec<String>,
    callee_display: String,
    name: String,
    qualifier: Option<String>,
}

/// 定义键：scope 链 \u{1} 名字。方法归属与调用来源定位共用。
fn def_key(rel_path: &str, scope: &[String], name: &str, signature: &str) -> String {
    let mut key = rel_path.to_string();
    key.push('\u{1}');
    key.push_str(&scope.join("\u{1}"));
    key.push('\u{1}');
    key.push_str(name);
    key.push('\u{1}');
    key.push_str(signature);
    key
}

struct GraphMerger {
    project: String,
    def_index: HashMap<String, NodeId>,
    pending_calls: Vec<PendingCall>,
    pending_types: Vec<(NodeId, super::extract::TypeRef)>,
    java_project: JavaProjectModel,
}

impl GraphMerger {
    fn new(project: &str, java_project: JavaProjectModel) -> Self {
        Self {
            project: project.to_string(),
            def_index: HashMap::new(),
            pending_calls: Vec::new(),
            pending_types: Vec::new(),
            java_project,
        }
    }

    fn merge_parsed(&mut self, graph: &mut GraphBuffer, parsed: &[ParseOutput]) {
        let mut line_updates: Vec<(NodeId, usize)> = Vec::new();

        for output in parsed {
            let Some(result) = output.result.as_ref() else {
                continue;
            };
            let rel_path = output.rel_path.as_str();
            let file_qn = file_qualified_name(&self.project, rel_path);
            let Some(file_id) = graph.find_by_qn(&file_qn) else {
                continue;
            };
            line_updates.push((file_id, output.line_count));

            // 导入 → Module 节点 + IMPORTS 边。
            let import_modules: Vec<String> = result
                .imports
                .iter()
                .filter(|i| !i.module.is_empty())
                .map(|i| i.module.clone())
                .collect();
            for module in &import_modules {
                let module_qn = format!("{}.mod.{module}", self.project);
                let module_id = graph.upsert_node(
                    NodeLabel::Module,
                    module.clone(),
                    module_qn,
                    "",
                    0,
                    0,
                    "{}".to_string(),
                );
                graph.add_edge(file_id, module_id, EdgeType::Imports, "{}".to_string());
            }

            // 定义 → 符号节点 + DEFINES / DEFINES_METHOD 边。
            for def in &result.defs {
                let qn = format!(
                    "{file_qn}{}.{}",
                    if def.scope.is_empty() {
                        String::new()
                    } else {
                        format!(".{}", def.scope.join("."))
                    },
                    format!("{}{}", def.name, def.signature)
                );
                let properties = definition_properties(
                    def,
                    &self.java_project.scope_for(rel_path),
                    result
                        .java
                        .as_ref()
                        .map(|java| java.package.as_str())
                        .unwrap_or(""),
                );
                let node_id = graph.add_symbol(
                    def.label,
                    def.name.clone(),
                    qn,
                    rel_path.to_string(),
                    def.start_line,
                    def.end_line,
                    properties,
                );
                self.def_index.insert(
                    def_key(rel_path, &def.scope, &def.name, &def.signature),
                    node_id,
                );
                // 方法/字段挂到容器符号；顶层定义挂到文件。
                match def.scope.split_last() {
                    Some((container, parents)) => {
                        if let Some(&cid) = self
                            .def_index
                            .get(&def_key(rel_path, parents, container, ""))
                        {
                            graph.add_edge(cid, node_id, EdgeType::DefinesMethod, "{}".to_string());
                            continue;
                        }
                        graph.add_edge(file_id, node_id, EdgeType::Defines, "{}".to_string());
                    }
                    None => {
                        graph.add_edge(file_id, node_id, EdgeType::Defines, "{}".to_string());
                    }
                }
            }

            // 类型继承引用：挂在同文件的第一个容器符号上。
            for tr in &result.type_refs {
                let host = result.defs.iter().find(|d| {
                    matches!(
                        d.label,
                        NodeLabel::Class
                            | NodeLabel::Struct
                            | NodeLabel::Interface
                            | NodeLabel::Trait
                    )
                });
                let Some(host) = host else { continue };
                if let Some(&host_id) =
                    self.def_index
                        .get(&def_key(rel_path, &host.scope, &host.name, &host.signature))
                {
                    self.pending_types.push((host_id, tr.clone()));
                }
            }

            // 调用点：归属函数优先定位，找不到退化为文件级调用。
            for call in &result.calls {
                let source = call
                    .owner
                    .as_ref()
                    .and_then(|o| {
                        self.def_index.get(&def_key(
                            rel_path,
                            &o.class_chain,
                            &o.fn_name,
                            &o.signature,
                        ))
                    })
                    .copied()
                    .unwrap_or(file_id);
                self.pending_calls.push(PendingCall {
                    source,
                    source_file: rel_path.to_string(),
                    imports: import_modules.clone(),
                    callee_display: call.callee_display.clone(),
                    name: call.name.clone(),
                    qualifier: qualifier_segment(&call.callee_display),
                });
            }
        }

        // File 节点行数属性批量回填（id == 下标不变量）。
        for (id, line_count) in line_updates {
            graph.nodes[id as usize].properties = file_properties(line_count);
        }
    }

    fn resolve_pending(
        &mut self,
        graph: &mut GraphBuffer,
        options: &mut PipelineOptions,
    ) -> Result<()> {
        options.report(IndexPhase::Resolve, 0, 0);
        let registry = Registry::build(graph);
        let calls = std::mem::take(&mut self.pending_calls);
        let total = calls.len();
        for (done, call) in calls.into_iter().enumerate() {
            if done % 2000 == 0 {
                options.report(IndexPhase::Resolve, done, total);
                if options.cancelled() {
                    return Ok(());
                }
            }
            if let Some(target) = registry.resolve_call(
                &call.name,
                &call.source_file,
                &call.imports,
                call.qualifier.as_deref(),
            ) {
                graph.add_edge(
                    call.source,
                    target.id,
                    EdgeType::Calls,
                    calls_edge_properties(&call.callee_display, target.confidence, target.strategy),
                );
            }
        }
        for (host_id, tr) in std::mem::take(&mut self.pending_types) {
            if let Some(target) = registry.resolve_type(&tr.name) {
                let etype = if tr.inherits {
                    EdgeType::Inherits
                } else {
                    EdgeType::Implements
                };
                graph.add_edge(host_id, target, etype, "{}".to_string());
            }
        }
        options.report(IndexPhase::Resolve, total, total);
        Ok(())
    }
}

/// 限定表达式的倒数第二段（`GitService::open` → GitService；
/// 单段调用返回 None）。
fn qualifier_segment(callee_display: &str) -> Option<String> {
    let segs: Vec<&str> = callee_display
        .split(['.', ':', '>'])
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() >= 2 {
        Some(segs[segs.len() - 2].to_string())
    } else {
        None
    }
}
