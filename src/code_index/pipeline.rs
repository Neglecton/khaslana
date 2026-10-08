//! 索引管线编排（参照 codebase-memory-mcp 的 `pipeline/pipeline.c` 与
//! `pipeline_incremental.c`）。
//!
//! 全量：discover → 结构 pass（Project/Branch/Folder/File）→ 提取 pass
//! （共用计算池动态分发，分批释放提取结果）→ 合并进图缓冲 →
//! 解析 pass（registry 策略链）→ 整库落盘。
//!
//! 增量路由与参考项目一致：已有库且 文件数 ≤ 已存哈希数 × 1.5 走增量，
//! 否则全量。增量 = mtime+size 三分类 → 入边快照 → 按文件清除 → 重解析变更
//! 文件 → 重解析受名字变化影响的调用文件 → 重链接快照边 → 按文件增量落盘。

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::discover::{DiscoveredFile, discover_files_cancellable};
use super::extract::{Extractor, FileExtractResult};
use super::graph::{
    EdgeType, GraphBuffer, NodeId, NodeLabel, calls_edge_properties,
    file_qualified_name, folder_qualified_name, lang_of_rel_path,
};
use super::resolve::Registry;
use super::store::{CodeIndexMeta, CodeIndexStore, FileHashRow};
use super::{IndexPhase, IndexProgress};
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
    pub(super) discovery_issues: Vec<super::coverage::FileCoverage>,
}

impl PipelineOptions {
    pub fn new(cancel: Arc<AtomicBool>, progress: Box<dyn FnMut(IndexProgress) + Send>) -> Self {
        Self { cancel, progress, discovery_issues: Vec::new() }
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
    let lock = super::jobs::database_lock(db_path);
    // Rayon 跨池等待可能执行其他排队任务；不能在重入任务里阻塞等待自己持有的库锁。
    let _guard = match lock.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Err(super::err("该仓库正在索引，请等待当前任务完成")),
    };
    if options.cancelled() { return Ok(RunOutcome::Cancelled); }
    let started = Instant::now();
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();
    let branch = detect_branch(repo_root);

    options.report(IndexPhase::Discover, 0, 0);
    let outcome = discover_files_cancellable(repo_root, Some(&options.cancel))?;
    options.discovery_issues = outcome.issues;
    let files = outcome.files;
    let total_files = files.len();
    if options.cancelled() {
        return Ok(RunOutcome::Cancelled);
    }

    let mut store = CodeIndexStore::open(db_path)?;

    // 增量路由（参考项目 try_incremental_or_delete_db 的判定式）。
    let existing_hashes = store.load_file_hashes()?;
    let incremental_eligible = !force_full
        && store.has_search_metadata()
        && store.discovery_issues()? == options.discovery_issues
        && !existing_hashes.is_empty()
        && total_files as f64 <= existing_hashes.len() as f64 * 1.5;
    let refs: Vec<&DiscoveredFile> = files.iter().collect();
    let inner = if incremental_eligible {
        run_incremental_inner(
            repo_root,
            &repo_name,
            &branch,
            &refs,
            &existing_hashes,
            &mut store,
            options,
        )?
    } else {
        run_full_inner(repo_root, &repo_name, &branch, &refs, &mut store, options)?
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
    store: &mut CodeIndexStore,
    options: &mut PipelineOptions,
) -> Result<InnerOutcome> {
    let mut graph = GraphBuffer::new();
    build_structure_pass(repo_name, branch, files, &mut graph);

    let parse_jobs: Vec<&DiscoveredFile> =
        files.iter().copied().filter(|f| is_parseable(f)).collect();
    let mut merger = GraphMerger::new(repo_name);
    run_extraction_pass(&parse_jobs, options, |parsed| merger.merge_parsed(&mut graph, parsed))?;
    if options.cancelled() { return Ok(InnerOutcome::Cancelled); }
    merger.resolve_pending(&mut graph, options)?;
    if options.cancelled() {
        return Ok(InnerOutcome::Cancelled);
    }

    drop(merger);
    options.report(IndexPhase::Write, 0, 0);
    // 全量图为全新构建，Module 恒有 IMPORTS 入边；清扫仅作防御（零成本）。
    graph.prune_orphan_modules();
    super::coverage::attach_discovery_issues(&mut graph, &options.discovery_issues);
    graph.release_build_indexes();
    if !write_store(
        store,
        repo_root,
        repo_name,
        branch,
        "full",
        &graph,
        files_hash_rows(files),
        &options.cancel,
    )? { return Ok(InnerOutcome::Cancelled); }
    Ok(InnerOutcome::Done)
}

fn files_hash_rows(files: &[&DiscoveredFile]) -> Vec<FileHashRow> {
    files
        .iter()
        .map(|f| FileHashRow {
            rel_path: f.rel_path.clone(),
            mtime_ns: f.mtime_ns,
            size: f.size,
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
    cancel: &AtomicBool,
) -> Result<bool> {
    let meta = index_meta(repo_root, repo_name, branch, mode);
    store.replace_all_cancellable(graph, &hashes, &meta, Some(cancel))
}

fn index_meta(repo_root: &Path, repo_name: &str, branch: &str, mode: &str) -> CodeIndexMeta {
    CodeIndexMeta {
        repo_name: repo_name.to_string(), repo_path: repo_root.to_string_lossy().to_string(),
        branch: branch.to_string(), indexed_at: now_millis(), duration_ms: 0, mode: mode.to_string(),
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
    store: &mut CodeIndexStore,
    options: &mut PipelineOptions,
) -> Result<InnerOutcome> {
    let old_by_path: HashMap<&str, &FileHashRow> = existing_hashes
        .iter()
        .map(|h| (h.rel_path.as_str(), h))
        .collect();
    let new_paths: HashSet<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();

    // 当前发现域以外的旧文件必须清除，避免忽略规则变更后仍返回旧符号。
    // 读取失败由覆盖记录单独披露，下次发现/解析继续重试。
    let mut changed: Vec<&DiscoveredFile> = Vec::new();
    let retry_files = store.retry_files()?;
    let mut unchanged_hashes: Vec<FileHashRow> = Vec::new();
    for file in files {
        match old_by_path.get(file.rel_path.as_str()) {
            Some(old) if old.mtime_ns == file.mtime_ns && old.size == file.size && !retry_files.contains(&file.rel_path) => {
                unchanged_hashes.push((*old).clone());
            }
            _ => changed.push(file),
        }
    }
    let mut deleted_paths: HashSet<String> = HashSet::new();
    for h in existing_hashes {
        if !new_paths.contains(h.rel_path.as_str()) {
            deleted_paths.insert(h.rel_path.clone());
        }
    }

    if changed.is_empty() && deleted_paths.is_empty() {
        return Ok(InnerOutcome::NoChange);
    }
    if options.cancelled() {
        return Ok(InnerOutcome::Cancelled);
    }

    options.report(IndexPhase::Parse, 0, changed.len());

    // 1. 只载入定义、结构和类型边，无关调用边继续保留在 SQLite。
    let mut graph = store.load_graph_without_calls()?;

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
        let src_survives = src_file.is_empty() || !purge_set.contains(src_file.as_ref());
        if src_survives && !tgt_file.is_empty() && purge_set.contains(tgt_file.as_ref()) && edge.etype != EdgeType::Calls {
            inbound_snapshot.push((
                graph.get(edge.source).qualified_name.to_string(),
                graph.get(edge.target).qualified_name.to_string(),
                edge.etype,
                edge.properties.to_string(),
            ));
        }
    }

    let mut changed_names: HashSet<String> = graph.nodes.iter().filter(|node| node.label.is_symbol() && purge_set.contains(node.file_path.as_ref()))
        .map(|node| node.name.clone()).collect();

    // 3. 按文件清除（级联删边 + 幸存节点 id 重排）。
    graph.purge_files(&purge_set);

    // 4. 补建结构后分批提取并合并，只保留一批 AST 提取结果。
    build_structure_pass(repo_name, branch, &changed, &mut graph);
    let mut merger = GraphMerger::new(repo_name);
    run_extraction_pass(&changed, options, |parsed| {
        changed_names.extend(parsed.iter().filter_map(|output| output.result.as_ref())
            .flat_map(|result| result.defs.iter().map(|def| def.name.clone())));
        merger.merge_parsed(&mut graph, parsed);
    })?;
    if options.cancelled() { return Ok(InnerOutcome::Cancelled); }
    let mut affected = purge_set.clone();
    affected.extend(store.callers_for_names(&changed_names)?);
    let surviving: HashSet<_> = affected.iter().filter(|path| !purge_set.contains(*path)).cloned().collect();
    store.load_call_records(&mut graph, &surviving)?;
    merger.resolve_paths = Some(affected.clone());
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

    // 8. 仅替换变更文件和受影响调用文件，数据库节点编号保持稳定。
    super::coverage::attach_discovery_issues(&mut graph, &options.discovery_issues);
    options.report(IndexPhase::Write, 0, 0);
    let mut hashes = unchanged_hashes;
    hashes.extend(files_hash_rows(&changed));
    hashes.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    drop(merger);
    graph.release_build_indexes();
    let meta = index_meta(repo_root, repo_name, branch, "incremental");
    if !store.update_files_cancellable(&graph, &hashes, &meta, &purge_set, &affected, &options.cancel)? { return Ok(InnerOutcome::Cancelled); }
    Ok(InnerOutcome::Done)
}

// ---------------------------------------------------------------------------
// 结构 pass（参照 pass_structure.c）
// ---------------------------------------------------------------------------

pub(super) fn build_structure_pass(
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
            serde_json::json!({ "line_count": 0, "coverage": initial_coverage(f) }).to_string(),
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

fn is_parseable(file: &DiscoveredFile) -> bool {
    file.size <= super::PARSE_MAX_BYTES && lang_of_rel_path(&file.rel_path).is_some()
}

// ---------------------------------------------------------------------------
// 提取 pass（并行，参照 pass_parallel.c 阶段 3A）
// ---------------------------------------------------------------------------

pub(super) struct ParseOutput {
    pub rel_path: String,
    pub result: Option<FileExtractResult>,
    pub line_count: usize,
    pub coverage: super::coverage::FileCoverage,
}

/// 共用计算池动态分发文件，先处理大文件，避免连续分块的长尾。
fn run_extraction_pass(
    jobs: &[&DiscoveredFile], options: &mut PipelineOptions, mut merge: impl FnMut(Vec<ParseOutput>),
) -> Result<()> {
    let total = jobs.len();
    options.report(IndexPhase::Parse, 0, total);
    let mut jobs = jobs.to_vec();
    jobs.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.rel_path.cmp(&b.rel_path)));
    let cancel = Arc::clone(&options.cancel);
    let mut done = 0;
    // 分批回报进度；每批的解析器和语法树在合并前释放。
    for batch in jobs.chunks(super::jobs::worker_count() * 8) {
        if options.cancelled() { break; }
        let mut parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::jobs::compute_pool().install(|| {
            batch.par_iter().map_init(Extractor::new, |extractor, file| {
                (!cancel.load(Ordering::Relaxed)).then(|| parse_one(file, extractor))
            }).filter_map(|output| output).collect::<Vec<_>>()
        }))).map_err(|_| super::err("索引解析线程异常，已保留旧索引"))?;
        done += parsed.len();
        parsed.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        merge(parsed);
        options.report(IndexPhase::Parse, done, total);
    }
    Ok(())
}

fn parse_one(file: &DiscoveredFile, extractor: &mut Extractor) -> ParseOutput {
    if !is_parseable(file) {
        return ParseOutput { rel_path: file.rel_path.clone(), result: None, line_count: 0,
            coverage: initial_coverage(file) };
    }
    match std::fs::read(&file.abs_path) {
        Ok(bytes) => parse_content(file, &bytes, extractor),
        Err(error) => ParseOutput { rel_path: file.rel_path.clone(), result: None, line_count: 0,
            coverage: super::coverage::FileCoverage::new(&file.rel_path, "read_failed", error.to_string()) },
    }
}

pub(super) fn parse_content(file: &DiscoveredFile, bytes: &[u8], extractor: &mut Extractor) -> ParseOutput {
    let mut output = ParseOutput {
        rel_path: file.rel_path.clone(),
        result: None,
        line_count: 0,
        coverage: initial_coverage(file),
    };
    let Some(lang) = lang_of_rel_path(&file.rel_path) else {
        return output;
    };
    // 二进制嗅探：前 8KB 出现 NUL 视为二进制（与项目内其他嗅探口径一致）。
    let sniff_len = bytes.len().min(8192);
    if bytes[..sniff_len].contains(&0) {
        output.coverage.status = "excluded".into();
        output.coverage.reason = "二进制文件".into();
        return output;
    }
    output.line_count = byte_line_count(&bytes);
    match extractor.extract(lang, bytes) {
        Ok(Some(result)) => {
            output.coverage.status = if result.error_ranges.is_empty() { "indexed" } else { "partial" }.into();
            output.coverage.error_ranges = result.error_ranges.clone();
            output.coverage.error_ranges_truncated = result.error_ranges_truncated;
            output.coverage.call_sites = result.calls.len();
            output.result = Some(result);
        }
        Ok(None) => { output.coverage.status = "parse_failed".into(); output.coverage.reason = "解析器未返回语法树".into(); }
        Err(error) => { output.coverage.status = "parse_failed".into(); output.coverage.reason = error.to_string(); }
    }
    output
}

fn initial_coverage(file: &DiscoveredFile) -> super::coverage::FileCoverage {
    let (status, reason) = if file.size > super::PARSE_MAX_BYTES { ("excluded", "超过单文件解析上限") }
        else if lang_of_rel_path(&file.rel_path).is_none() { ("unsupported", "当前语言不支持符号解析") }
        else { ("indexed", "") };
    super::coverage::FileCoverage::new(&file.rel_path, status, reason)
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

/// 定义键：scope 链 \u{1} 名字。方法归属与调用来源定位共用。
fn def_key(scope: &[String], name: &str) -> String {
    let mut key = scope.join("\u{1}");
    key.push('\u{1}');
    key.push_str(name);
    key
}

pub(super) struct GraphMerger {
    project: String,
    def_index: HashMap<String, NodeId>,
    resolve_paths: Option<HashSet<String>>,
    pending_types: Vec<(NodeId, super::extract::TypeRef)>,
}

impl GraphMerger {
    pub(super) fn new(project: &str) -> Self {
        Self {
            project: project.to_string(),
            def_index: HashMap::new(),
            resolve_paths: None,
            pending_types: Vec::new(),
        }
    }

    pub(super) fn merge_parsed(&mut self, graph: &mut GraphBuffer, parsed: Vec<ParseOutput>) {

        for output in parsed {
            // 同名函数的归属只在当前文件内查找，不能复用上一文件的定义键。
            self.def_index.clear();
            let rel_path = output.rel_path.as_str();
            let file_qn = file_qualified_name(&self.project, rel_path);
            let Some(file_id) = graph.find_by_qn(&file_qn) else {
                continue;
            };
            graph.nodes[file_id as usize].properties = serde_json::json!({
                "line_count": output.line_count, "coverage": output.coverage }).to_string().into();
            let Some(result) = output.result else { continue; };

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
                    def.name
                );
                let node_id = graph.add_symbol(
                    def.label,
                    def.name.clone(),
                    qn,
                    rel_path.to_string(),
                    def.start_line,
                    def.end_line,
                    serde_json::json!({ "signature": def.signature, "docstring": def.docstring, "scope": def.scope }).to_string(),
                );
                self.def_index
                    .insert(def_key(&def.scope, &def.name), node_id);
                // 方法/字段挂到容器符号；顶层定义挂到文件。
                match def.scope.split_last() {
                    Some((container, parents)) => {
                        if let Some(&cid) = self.def_index.get(&def_key(parents, container)) {
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
                if let Some(&host_id) = self.def_index.get(&def_key(&host.scope, &host.name)) {
                    self.pending_types.push((host_id, tr.clone()));
                }
            }

            // 调用点采用类型化文件记录，导入表与路径只保留一份。
            let mut strings = HashMap::<String, Arc<str>>::new();
            let mut intern = |value: String| -> Arc<str> {
                Arc::clone(strings.entry(value.clone()).or_insert_with(|| Arc::from(value)))
            };
            let calls = result.calls.into_iter().map(|call| {
                let source = call.owner.as_ref()
                    .and_then(|owner| self.def_index.get(&def_key(&owner.class_chain, &owner.fn_name)))
                    .copied().unwrap_or(file_id);
                super::calls::StoredCall {
                    source_qn: Arc::clone(&graph.get(source).qualified_name),
                    callee_display: intern(call.callee_display), name: intern(call.name),
                    scope: intern(call.owner.map(|owner| owner.class_chain.join("\u{1}")).unwrap_or_default()),
                }
            }).collect();
            graph.call_records.insert(Arc::clone(&graph.get(file_id).file_path), super::calls::FileCalls { imports: import_modules, calls });

        }

    }

    pub(super) fn resolve_pending(
        &mut self,
        graph: &mut GraphBuffer,
        options: &mut PipelineOptions,
    ) -> Result<()> {
        self.resolve_pending_cancellable(graph, options, None)
    }

    pub(super) fn resolve_pending_cancellable(
        &mut self, graph: &mut GraphBuffer, options: &mut PipelineOptions, cancelled: Option<&AtomicBool>,
    ) -> Result<()> {
        options.report(IndexPhase::Resolve, 0, 0);
        // 名字变化只重算相关调用文件；缓存包含完整表达式和作用域，也缓存未解析结果。
        let paths: Vec<_> = graph.call_records.keys().filter(|path|
            self.resolve_paths.as_ref().is_none_or(|paths| paths.contains(path.as_ref()))).cloned().collect();
        let mut paths = paths;
        paths.sort();
        graph.remove_call_edges_for(self.resolve_paths.as_ref());
        let registry = Registry::build(graph);
        let total = paths.iter().map(|path| graph.call_records[path].calls.len()).sum();
        let mut done = 0;
        for batch in paths.chunks(super::jobs::worker_count() * 2) {
            if options.cancelled() || cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) { return Ok(()); }
            let cancel = &options.cancel;
            let results: Vec<_> = super::jobs::compute_pool().install(|| batch.par_iter().map(|path| {
                let records = &graph.call_records[path];
                let imports = registry.imported_files(&records.imports);
                let mut memo = HashMap::<(Arc<str>, Arc<str>, Arc<str>), Option<super::resolve::ResolvedTarget>>::new();
                let mut edges = Vec::new();
                let mut keys = HashSet::new();
                let mut unresolved = 0;
                for (index, call) in records.calls.iter().enumerate() {
                    if index % 512 == 0 && (cancel.load(Ordering::Relaxed)
                        || cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed))) { break; }
                    let key = (Arc::clone(&call.name), Arc::clone(&call.callee_display), Arc::clone(&call.scope));
                    let resolved = if let Some(target) = memo.get(&key) { target.clone() } else {
                        let scope: Vec<_> = call.scope.split('\u{1}').filter(|part| !part.is_empty()).map(str::to_string).collect();
                        let qualifier = qualifier_segment(&call.callee_display);
                        let target = registry.resolve_prepared(&call.name, path, &imports, qualifier.as_deref(), &scope);
                        // 有界缓存防御生成代码中的大量不同表达式。
                        if memo.len() < 16384 { memo.insert(key, target.clone()); }
                        target
                    };
                    if let Some(target) = resolved {
                        if let Some(source) = graph.find_by_qn(&call.source_qn) {
                            if source != target.id && keys.insert((source, target.id)) {
                                edges.push((source, target.id, calls_edge_properties(&call.callee_display, target.confidence, target.strategy)));
                            }
                        }
                    } else { unresolved += 1; }
                }
                (Arc::clone(path), edges, unresolved, records.calls.len())
            }).collect());
            for (path, edges, unresolved, count) in results {
                for (source, target, properties) in edges { graph.add_edge(source, target, EdgeType::Calls, properties); }
                if let Some(file_id) = graph.find_by_qn(&file_qualified_name(&self.project, &path)) {
                    let mut properties: serde_json::Value = serde_json::from_str(&graph.nodes[file_id as usize].properties).unwrap_or_default();
                    properties["coverage"]["unresolved_calls"] = serde_json::json!(unresolved);
                    graph.nodes[file_id as usize].properties = properties.to_string().into();
                }
                done += count;
            }
            options.report(IndexPhase::Resolve, done, total);
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

/// 限定表达式的完整前缀（`crate::git::open` → crate.git；
/// 单段调用返回 None）。
fn qualifier_segment(callee_display: &str) -> Option<String> {
    let segs: Vec<&str> = callee_display
        .split(['.', ':', '>'])
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() >= 2 {
        Some(segs[..segs.len() - 1].join("."))
    } else {
        None
    }
}
