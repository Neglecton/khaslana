//! 内嵌 MCP stdio 服务器（协议行为逐条对照 codebase-memory-mcp 的
//! `src/mcp/mcp.c`：换行分隔 JSON 帧 + Content-Length 兼容、单行紧凑 JSON
//! text 载荷 + structuredContent 双通道、业务错误 isError + 自纠错 hint、
//! 字符串/数字 id 原样回显、单线程串行处理）。
//!
//! 两种启动形态（对齐参考项目「单服务器 + 项目注册表」的多仓库语义）：
//! - `khaslana mcp <仓库路径>`：单仓库模式，启动即校验并后台保障索引，
//!   工具调用固定查该仓库（per-仓库各挂一条的高级用法，repo 参数被忽略）。
//! - `khaslana mcp`：多仓库模式（推荐，MCP 配置与仓库零耦合），启动时
//!   不绑定仓库；工具调用经可选 `repo` 参数解析目标（仓库绝对路径，或
//!   `list_projects` 返回的 repo 键），未传时若仅一个已索引仓库自动选中、
//!   多个则报错列出清单让模型自纠错；首次触达的仓库自动后台建索引。
//!
//! 入口 `run(repo_path)`：进入 stdin 消息循环直到 EOF。stdout 只走协议，
//! 进度/日志一律 stderr。
//!
//! 由 `main()` 顶部的 `khaslana mcp [仓库路径]` 子命令分支调用（先于 GUI
//! 启动），供 Claude Code / Cursor / ZCode 等 AI 工具作为 MCP 服务器挂载，
//! 查询本机已索引仓库的代码知识图谱。

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::queries::{self, DetailOutcome, TraceDirection, TraceOutcome};
use super::store::{IndexStats, read_index_stats};
use super::search::{SearchOptions, search_symbols_with_options};
use crate::types::Result;

/// 服务器支持的 MCP 协议版本（协商：客户端声明版本在列表内则回显，否则回最新）。
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "khaslana-code-index";
/// 单帧字节上限（防恶意/损坏输入撑爆内存；对齐参考项目 10MB）。
const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
/// trace_path / 详情查询的默认 BFS 节点上限（对齐参考项目 MCP_BFS_LIMIT）。
const DEFAULT_MAX_NODES: usize = 100;

/// 单个仓库的查询上下文：根目录（按 repo 键解析且 meta 缺路径时可能未知）
/// 与索引库路径。
#[derive(Clone)]
struct RepoContext {
    repo_root: Option<PathBuf>,
    db_path: PathBuf,
    repo_key: String,
}

/// 服务器状态。`fixed` 为单仓库模式的固定仓库；多仓库模式为 None，按每次
/// 工具调用的 repo 参数解析。
pub struct McpServer {
    fixed: Option<RepoContext>,
    data_dir: PathBuf,
    coordinator: super::jobs::IndexCoordinator,
    auto_refresh: bool,

}

#[cfg(test)]
#[path = "../tests/code_index_mcp_jobs.rs"]
mod jobs_tests;

impl Drop for McpServer {
    fn drop(&mut self) { self.coordinator.stop(); }
}

impl McpServer {
    /// 初始化。`Some(repo_path)` 为单仓库模式：校验仓库 → 索引保障（无库
    /// 自动全量建、有库增量检查，后台线程防 initialize 超时）；`None` 为
    /// 多仓库模式：只解析数据目录，仓库在工具调用时按 repo 参数解析。
    /// 失败返回 Err（调用方写 stderr 后退出）。
    pub fn new(repo_path: Option<&Path>) -> Result<Self> {
        let data_dir = crate::storage::active_data_dir()
            .ok_or_else(|| super::err("数据目录不可用，无法定位索引库"))?;
        let mut server = Self {
            fixed: None,
            data_dir,
            coordinator: super::jobs::IndexCoordinator::default(),
            auto_refresh: true,
        };
        if let Some(repo_path) = repo_path {
            let ctx = Self::context_from_root(&server.data_dir, repo_path)?;
            server.ensure_index_background(&ctx);
            server.fixed = Some(ctx);
        }
        Ok(server)
    }

    /// 测试专用：单仓库模式（跳过仓库校验与索引保障）。
    #[cfg(test)]
    pub(crate) fn for_test(repo_root: &Path, db_path: PathBuf) -> Self {
        Self {
            fixed: Some(RepoContext {
                repo_root: Some(repo_root.to_path_buf()),
                db_path,
                repo_key: String::new(),
            }),
            data_dir: std::env::temp_dir(),
            coordinator: super::jobs::IndexCoordinator::default(),
            auto_refresh: false,
        }
    }

    /// 测试专用：多仓库模式（不绑定仓库），data_dir 指向临时数据目录。
    #[cfg(test)]
    pub(crate) fn for_multi_test(data_dir: PathBuf) -> Self {
        Self {
            fixed: None,
            data_dir,
            coordinator: super::jobs::IndexCoordinator::default(),
            auto_refresh: false,
        }
    }

    /// 由仓库根目录构造查询上下文（canonicalize + git2 校验 + repo 键定位库）。
    fn context_from_root(data_dir: &Path, repo_path: &Path) -> Result<RepoContext> {
        let repo_root = repo_path
            .canonicalize()
            .map_err(|e| super::err(format!("仓库路径不可用 {}: {e}", repo_path.display())))?;
        git2::Repository::open(&repo_root)
            .map_err(|e| super::err(format!("{} 不是 Git 仓库：{e}", repo_root.display())))?;
        let repo_key = crate::ai::review_store::repo_key(&repo_root.to_string_lossy());
        let db_path = super::open_index_db_path(data_dir, &repo_key)?;
        Ok(RepoContext {
            repo_root: Some(repo_root),
            db_path,
            repo_key,
        })
    }

    /// 仓库键格式校验（repo_key = FNV-1a 32 位小写十六进制 8 字符）。
    fn is_repo_key(spec: &str) -> bool {
        spec.len() == 8 && spec.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// 解析 repo 参数：优先按仓库路径（存在但非 Git 仓库显式报错，不静默）；
    /// 路径不存在时回退按 8 位 repo 键在数据目录中定位索引库。
    fn context_from_spec(&self, spec: &str) -> std::result::Result<RepoContext, Value> {
        let path = Path::new(spec);
        if path.exists() {
            return Self::context_from_root(&self.data_dir, path)
                .map_err(|e| json!({ "error": e.to_string() }));
        }
        if Self::is_repo_key(spec) {
            let key = spec.to_ascii_lowercase();
            let db_path = self.data_dir.join("code-index").join(&key).join("index.db");
            if db_path.is_file() {
                let stats = read_index_stats(&db_path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                return Ok(Self::context_from_entry(&self.data_dir, &key, &stats));
            }
        }
        Err(json!({
            "error": format!("repo 参数 {spec} 既不是存在的仓库路径，也没有对应的索引库"),
            "hint": "传仓库绝对路径，或先调 list_projects 查看可用仓库",
        }))
    }

    /// 由 repo 键 + 已读统计构造上下文：根目录取 meta repo_path（旧库可能
    /// 为空 → None，需要根目录的工具会给出明确错误，下次索引落盘补写）。
    fn context_from_entry(data_dir: &Path, repo_key: &str, stats: &IndexStats) -> RepoContext {
        RepoContext {
            repo_root: if stats.repo_path.is_empty() {
                None
            } else {
                Some(PathBuf::from(&stats.repo_path))
            },
            db_path: data_dir.join("code-index").join(repo_key).join("index.db"),
            repo_key: repo_key.to_string(),
        }
    }

    /// 枚举数据目录下全部已索引仓库（code-index/<键>/index.db 且统计可读），
    /// 按最近索引时间倒序。
    fn list_project_entries(data_dir: &Path) -> Vec<(String, IndexStats)> {
        let mut entries = Vec::new();
        let Ok(dirs) = std::fs::read_dir(data_dir.join("code-index")) else {
            return entries;
        };
        for dir in dirs.flatten() {
            let db_path = dir.path().join("index.db");
            if !db_path.is_file() {
                continue;
            }
            let Some(stats) = read_index_stats(&db_path).ok().flatten() else {
                continue;
            };
            entries.push((dir.file_name().to_string_lossy().to_string(), stats));
        }
        entries.sort_by(|a, b| b.1.indexed_at.cmp(&a.1.indexed_at));
        entries
    }

    /// 解析工具调用的目标仓库：单仓库模式固定返回（repo 参数被忽略）；
    /// 多仓库模式按 repo 参数解析，未传时仅一个已索引仓库自动选中、多个
    /// 报错列出清单让模型自纠错。
    fn resolve_context(&self, repo_arg: Option<&str>) -> std::result::Result<RepoContext, Value> {
        if let Some(fixed) = &self.fixed {
            return Ok(fixed.clone());
        }
        let Some(spec) = repo_arg else {
            let entries = Self::list_project_entries(&self.data_dir);
            return match entries.len() {
                0 => Err(json!({
                    "error": "还没有任何已索引仓库",
                    "hint": "调用 refresh_index 并传 repo 参数（仓库绝对路径）建立索引",
                })),
                1 => Ok(Self::context_from_entry(
                    &self.data_dir,
                    &entries[0].0,
                    &entries[0].1,
                )),
                _ => {
                    let projects: Vec<Value> = entries
                        .iter()
                        .map(|(key, stats)| project_entry_json(key, stats))
                        .collect();
                    Err(json!({
                        "error": format!("未指定 repo 参数且本机已有 {} 个已索引仓库", projects.len()),
                        "projects": projects,
                        "hint": "传 repo 参数（仓库绝对路径，或上方任一条目的 repo 键）；也可先调 list_projects 查看",
                    }))
                }
            };
        };
        let ctx = self.context_from_spec(spec)?;
        Ok(ctx)
    }

    /// 查询触发后台索引检查：按仓库排队、成功检查节流、失败指数退避。
    /// 无库自动全量建、有库增量检查。大仓库全量
    /// 建索引可达分钟级，走专用任务池防 MCP 客户端 initialize/工具超时；建立
    /// 期间查询得到「索引尚未建立」错误 + hint，属预期行为。
    fn ensure_index_background(&self, ctx: &RepoContext) {
        if !self.auto_refresh { return; }
        let Some(repo_root) = ctx.repo_root.clone() else { return; };
        let db_path = ctx.db_path.clone();
        let repo_key = ctx.repo_key.clone();
        self.coordinator.schedule(repo_key.clone(), false, move |cancel| {
            let result = Self::run_index_inner(&repo_root, &db_path, false, cancel, |message| {
                eprintln!("[khaslana-mcp] {message}");
            });
            if let Err(error) = &result { eprintln!("[khaslana-mcp] 后台索引失败（{repo_key}）：{error}"); }
            result.map_err(|error| error.to_string())
        });
    }

    fn indexing(&self, key: &str) -> bool {
        self.coordinator.status(key)["active"].as_bool().unwrap_or(false)
    }

    fn run_index_inner(
        repo_root: &Path,
        db_path: &Path,
        force_full: bool,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        progress: fn(String),
    ) -> Result<()> {
        let mut options = super::PipelineOptions::new(
            cancel,
            Box::new(move |p| progress(format!("{} {}/{}", p.phase.display(), p.done, p.total))),
        );
        match super::run_index(repo_root, db_path, force_full, &mut options)? {
            super::RunOutcome::Completed(stats) => progress(format!(
                "索引完成：{} 文件 / {} 符号 / {} 边",
                stats.files, stats.symbols, stats.edges
            )),
            super::RunOutcome::Unchanged => progress("索引无变化".to_string()),
            super::RunOutcome::Cancelled => return Err(super::err("索引已取消，保留原有索引")),
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 工具分发
    // ------------------------------------------------------------------

    /// tools/call 分发：返回 MCP content 信封（业务错误 isError:true）。
    /// list_projects 不依赖仓库；其余工具先按 repo 参数解析目标仓库。
    fn call_tool(&self, name: &str, arguments: &Value) -> Value {
        if !matches!(name, "list_projects" | "search_symbols" | "search_graph" | "get_symbol_detail" | "get_code_snippet" | "trace_path" | "get_architecture" | "detect_changes" | "index_status" | "check_index_coverage" | "refresh_index") {
            return text_result(&json!({ "error": format!("未知工具 {name}"), "hint": "可用工具见 tools/list" }), true);
        }
        if name == "list_projects" {
            return match self.tool_list_projects() {
                Ok(value) => text_result(&value, false),
                Err(value) => text_result(&value, true),
            };
        }
        let repo_arg = Self::arg_str(arguments, "repo");
        let ctx = match self.resolve_context(repo_arg) {
            Ok(ctx) => ctx,
            Err(value) => return text_result(&value, true),
        };
        if name != "refresh_index" { self.ensure_index_background(&ctx); }
        let guarded = matches!(name, "search_symbols" | "search_graph" | "get_symbol_detail" | "get_code_snippet" | "trace_path" | "get_architecture" | "check_index_coverage");
        let generation = if guarded { super::store::index_generation(&ctx.db_path).ok().flatten() } else { None };
        if guarded && Self::arg_str(arguments, "generation").is_some_and(|expected| generation.as_deref() != Some(expected)) {
            return text_result(&json!({ "error": "索引已刷新，旧分页代际失效", "hint": "移除 generation 并从 offset=0 重新查询" }), true);
        }
        let result = match name {
            "search_symbols" | "search_graph" => self.tool_search_symbols(&ctx, arguments, name == "search_graph"),
            "get_symbol_detail" => self.tool_symbol_detail(&ctx, arguments),
            "get_code_snippet" => self.tool_code_snippet(&ctx, arguments),
            "trace_path" => self.tool_trace_path(&ctx, arguments),
            "get_architecture" => self.tool_architecture(&ctx),
            "detect_changes" => self.tool_detect_changes(&ctx, arguments),
            "index_status" => self.tool_index_status(&ctx),
            "check_index_coverage" => self.tool_coverage(&ctx, arguments),
            "refresh_index" => self.tool_refresh_index(&ctx, arguments),
            other => {
                return text_result(
                    &json!({
                        "error": format!("未知工具 {other}"),
                        "hint": "可用工具见 tools/list",
                    }),
                    true,
                );
            }
        };
        match result {
            Ok(mut value) => {
                if guarded {
                    if generation != super::store::index_generation(&ctx.db_path).ok().flatten() {
                        return text_result(&json!({ "error": "查询期间索引已更新", "hint": "从 offset=0 重新查询" }), true);
                    }
                    value["generation"] = json!(generation);
                }
                text_result(&value, false)
            },
            Err(mut value) => {
                // 后台索引建立期间查询会命中「索引尚未建立」——补自纠错 hint
                // （仅当正在建的就是当前仓库，多仓库模式不误伤其他仓库报错）。
                if self.indexing(&ctx.repo_key) {
                    value["hint"] =
                        json!("索引正在后台建立/刷新，稍候重试；可用 index_status 观察");
                }
                text_result(&value, true)
            }
        }
    }

    fn arg_str<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
        arguments.get(key).and_then(|v| v.as_str())
    }

    fn arg_usize(arguments: &Value, key: &str) -> Option<usize> {
        arguments
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
    }

    fn missing_arg_error(key: &str) -> Value {
        json!({ "error": format!("缺少必填参数 {key}"), "hint": "参考 tools/list 中该工具的 inputSchema" })
    }

    fn tool_search_symbols(
        &self,
        ctx: &RepoContext,
        args: &Value,
        structural: bool,
    ) -> std::result::Result<Value, Value> {
        let query = Self::arg_str(args, "query");
        if query.is_none() && (!structural || ["name_pattern", "qn_pattern", "file_pattern", "label"]
            .iter().all(|key| Self::arg_str(args, key).is_none())) {
            return Err(Self::missing_arg_error("query 或 name_pattern / qn_pattern / file_pattern / label"));
        }
        let limit = Self::arg_usize(args, "limit").unwrap_or(20).clamp(1, 200);
        let offset = Self::arg_usize(args, "offset").unwrap_or(0);
        let options = SearchOptions {
            query, label: Self::arg_str(args, "label"),
            file_pattern: Self::arg_str(args, "file_pattern"),
            name_pattern: Self::arg_str(args, "name_pattern"),
            qn_pattern: Self::arg_str(args, "qn_pattern"), limit, offset,
        };
        let (all, total) = search_symbols_with_options(&ctx.db_path, &options)
            .map_err(|e| json!({ "error": e.to_string() }))?;
        let count = all.len();
        let next_offset = offset.saturating_add(count);
        let has_more = next_offset < total;
        let mut result = json!({ "repo": ctx.repo_key, "total": total, "results": all,
            "offset": offset, "returned": count, "has_more": has_more });
        if has_more { result["next_offset"] = json!(next_offset); }
        if count == 0 {
            result["hint"] = if total > 0 {
                json!("offset 已超过结果总数，从 offset=0 重新查询")
            } else {
                json!("无命中。缩短 query 或用 search_graph 的 name_pattern 正则、file_pattern 路径 glob 定位；怀疑过期时调用 refresh_index。索引是尽力解析，查不到不能证明代码不存在。")
            };
        }
        Ok(result)
    }

    fn tool_code_snippet(&self, ctx: &RepoContext, args: &Value) -> std::result::Result<Value, Value> {
        let Some(name) = Self::arg_str(args, "qualified_name") else {
            return Err(Self::missing_arg_error("qualified_name"));
        };
        let mut result = self.tool_symbol_detail(ctx, &json!({ "name": name }))?;
        if let Some(object) = result.as_object_mut() {
            object.remove("callers");
            object.remove("callees");
            if !object.contains_key("source") && !object.contains_key("suggestions") {
                object.insert("source_note".to_string(), json!("当前无法读取源码文件。确认 repo 为存在的仓库绝对路径；文件已移动或索引过期时先刷新。"));
            }
        }
        Ok(result)
    }

    fn tool_symbol_detail(
        &self,
        ctx: &RepoContext,
        args: &Value,
    ) -> std::result::Result<Value, Value> {
        let Some(name) = Self::arg_str(args, "name") else {
            return Err(Self::missing_arg_error("name"));
        };
        let outcome = queries::symbol_detail(&ctx.db_path, ctx.repo_root.as_deref(), name)
            .map_err(|e| json!({ "error": e.to_string() }))?;
        match outcome {
            DetailOutcome::Found(detail) => Ok(serde_json::to_value(&*detail)
                .map_err(|e| json!({ "error": format!("序列化详情失败：{e}") }))?),
            DetailOutcome::Ambiguous(candidates) => Ok(json!({
                "status": "ambiguous",
                "message": format!("有 {} 个同名定义，请用 qualified_name 精确查询", candidates.len()),
                "suggestions": candidates,
            })),
            DetailOutcome::NotFound => Err(json!({
                "error": "symbol not found",
                "name": name,
                "hint": "先用 search_symbols 模糊检索确认名称",
            })),
        }
    }

    fn tool_trace_path(
        &self,
        ctx: &RepoContext,
        args: &Value,
    ) -> std::result::Result<Value, Value> {
        let Some(function_name) = Self::arg_str(args, "function_name") else {
            return Err(Self::missing_arg_error("function_name"));
        };
        let direction = TraceDirection::parse(Self::arg_str(args, "direction").unwrap_or("both"));
        let depth = Self::arg_usize(args, "depth").unwrap_or(3).clamp(1, 8) as u32;
        let limit = Self::arg_usize(args, "limit").unwrap_or(DEFAULT_MAX_NODES).clamp(1, 200);
        let offset = Self::arg_usize(args, "offset").unwrap_or(0);
        let risk_labels = args
            .get("risk_labels")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let outcome = queries::trace_calls_page(
            &ctx.db_path,
            function_name,
            direction,
            depth,
            limit,
            offset,
        )
        .map_err(|e| json!({ "error": e.to_string() }))?;
        match outcome {
            TraceOutcome::Found(mut result) => {
                if !risk_labels {
                    for hop in result.callers.iter_mut().chain(result.callees.iter_mut()) {
                        hop.risk = "";
                    }
                }
                let mut value = serde_json::to_value(&result)
                    .map_err(|e| json!({ "error": format!("序列化结果失败：{e}") }))?;
                if result.has_more { value["next_offset"] = json!(offset.saturating_add(limit)); }
                value["coverage_note"] = json!("调用边来自 tree-sitter 与启发式解析，可能缺少动态调用或类型信息；total 只统计当前索引中指定深度内的可达节点。");
                Ok(value)
            }
            TraceOutcome::Ambiguous(candidates) => Ok(json!({
                "status": "ambiguous",
                "message": format!("有 {} 个同名可调用定义，请用 qualified_name 精确查询", candidates.len()),
                "suggestions": candidates,
            })),
            TraceOutcome::NotFound => Err(json!({
                "error": "function not found",
                "function_name": function_name,
                "hint": "先用 search_symbols 模糊检索确认名称",
            })),
        }
    }

    fn tool_architecture(&self, ctx: &RepoContext) -> std::result::Result<Value, Value> {
        let overview =
            queries::index_overview(&ctx.db_path).map_err(|e| json!({ "error": e.to_string() }))?;
        serde_json::to_value(&overview).map_err(|e| json!({ "error": format!("序列化失败：{e}") }))
    }

    fn tool_detect_changes(
        &self,
        ctx: &RepoContext,
        args: &Value,
    ) -> std::result::Result<Value, Value> {
        let Some(repo_root) = ctx.repo_root.as_ref() else {
            return Err(json!({
                "error": "该索引库缺少仓库路径元数据，无法读取工作区状态",
                "hint": "改传 repo 参数为仓库绝对路径；或先 refresh_index（同样传绝对路径）补写元数据",
            }));
        };
        let scope = Self::arg_str(args, "scope").unwrap_or("symbols");
        let base_branch = Self::arg_str(args, "base_branch");
        let changed_files = queries::changed_files_via_git(repo_root, base_branch)
            .map_err(|e| json!({ "error": e.to_string() }))?;
        let mut result = match scope {
            "files" => json!({
                "changed_files": changed_files,
                "changed_count": changed_files.len(),
            }),
            _ => {
                // symbols：文件 + 受影响符号 + 上游调用方展开。
                let report = queries::impacted_symbols_for_files(&ctx.db_path, &changed_files, 1)
                    .map_err(|e| json!({ "error": e.to_string() }))?;
                serde_json::to_value(&report)
                    .map_err(|e| json!({ "error": format!("序列化失败：{e}") }))?
            }
        };
        if changed_files.is_empty() {
            result["hint"] = json!("工作区与基线之间没有检测到变更文件");
        }
        Ok(result)
    }

    fn tool_index_status(&self, ctx: &RepoContext) -> std::result::Result<Value, Value> {
        let indexing = self.indexing(&ctx.repo_key);
        let stats =
            read_index_stats(&ctx.db_path).map_err(|e| json!({ "error": e.to_string(), "status": "error",
                "repo": ctx.repo_key, "indexing": indexing, "refresh": self.coordinator.status(&ctx.repo_key) }))?;
        match stats {
            Some(stats) => {
                let mut value = stats_to_json(&stats);
                value["repo"] = json!(ctx.repo_key);
                value["indexing"] = json!(indexing);
                value["refresh"] = self.coordinator.status(&ctx.repo_key);
                if indexing { value["status"] = json!("indexing"); }
                value["coverage_note"] = json!("索引基于 tree-sitter 和启发式调用解析；统计正常不等于所有定义和关系均已覆盖。");
                Ok(value)
            }
            None => Ok(json!({
                "status": if indexing { "indexing" } else { "empty" },
                "indexing": indexing,
                "repo": ctx.repo_key,
                "refresh": self.coordinator.status(&ctx.repo_key),
                "hint": "索引为空。调用 refresh_index 建立全量索引（多仓库模式需传 repo 参数）",
            })),
        }
    }

    fn tool_list_projects(&self) -> std::result::Result<Value, Value> {
        let entries = Self::list_project_entries(&self.data_dir);
        let projects: Vec<Value> = entries
            .iter()
            .map(|(key, stats)| project_entry_json(key, stats))
            .collect();
        let count = projects.len();
        let mut result = json!({ "total": count, "projects": projects });
        if count == 0 {
            result["hint"] = json!(
                "还没有任何已索引仓库：调用 refresh_index 并传 repo 参数（仓库绝对路径）建立索引"
            );
        } else {
            result["hint"] = json!(
                "其余工具传 repo 参数（仓库绝对路径或这里的 repo 键）选择目标仓库；本机仅一个仓库时可不传"
            );
        }
        Ok(result)
    }

    fn tool_coverage(&self, ctx: &RepoContext, args: &Value) -> std::result::Result<Value, Value> {
        let strings = |key: &str| -> std::result::Result<Vec<String>, Value> {
            let Some(value) = args.get(key) else { return Ok(Vec::new()); };
            let Some(items) = value.as_array() else { return Err(json!({ "error": format!("{key} 必须是字符串数组") })); };
            items.iter().map(|value| value.as_str().map(str::to_string)
                .ok_or_else(|| json!({ "error": format!("{key} 必须是字符串数组") }))).collect()
        };
        super::coverage::check_coverage(&ctx.db_path, ctx.repo_root.as_deref(), &strings("paths")?, &strings("scopes")?,
            Self::arg_usize(args, "offset").unwrap_or(0), Self::arg_usize(args, "limit").unwrap_or(50).clamp(1, 200))
            .map_err(|error| json!({ "error": error.to_string() }))
    }

    fn tool_refresh_index(
        &self,
        ctx: &RepoContext,
        args: &Value,
    ) -> std::result::Result<Value, Value> {
        let mode = Self::arg_str(args, "mode").unwrap_or("incremental");
        let force_full = match mode {
            "incremental" => false,
            "full" => true,
            other => {
                return Err(
                    json!({ "error": format!("未知 mode {other}，可选 incremental | full") }),
                );
            }
        };
        let Some(repo_root) = ctx.repo_root.clone() else {
            return Err(json!({
                "error": "该索引库缺少仓库路径元数据，无法定位仓库根目录",
                "hint": "改传 repo 参数为仓库绝对路径再刷新",
            }));
        };
        self.coordinator.wait(&ctx.repo_key);
        let db_path = ctx.db_path.clone();
        if !self.coordinator.schedule(ctx.repo_key.clone(), true, move |cancel| {
            Self::run_index_inner(&repo_root, &db_path, force_full, cancel, |message| {
                eprintln!("[khaslana-mcp] {message}");
            }).map_err(|error| error.to_string())
        }) {
            return Err(json!({ "error": "同仓库索引任务仍在运行或调度器已关闭" }));
        }
        self.coordinator.wait(&ctx.repo_key);
        if let Some(error) = self.coordinator.status(&ctx.repo_key)["last_error"].as_str() {
            return Err(json!({ "error": error }));
        }

        let stats = read_index_stats(&ctx.db_path)
            .map_err(|e| json!({ "error": e.to_string() }))?
            .ok_or_else(|| json!({ "error": "索引完成后统计仍为空" }))?;
        let mut value = stats_to_json(&stats);
        value["status"] = json!("ready");
        value["repo"] = json!(ctx.repo_key);
        Ok(value)
    }

    // ------------------------------------------------------------------
    // 协议消息处理
    // ------------------------------------------------------------------

    /// 处理一条 JSON-RPC 消息；通知类返回 None（不产生响应）。
    pub fn handle_message(&self, line: &str) -> Option<String> {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                return Some(jsonrpc_error(&json!(0), -32700, "Parse error"));
            }
        };
        // 无 id 的消息按 JSON-RPC 通知处理：不产生任何响应。
        let Some(id) = parsed.get("id").cloned() else {
            return None;
        };
        let Some(method) = parsed.get("method").and_then(|m| m.as_str()) else {
            return Some(jsonrpc_error(
                &id,
                -32600,
                "Invalid Request: missing method",
            ));
        };

        match method {
            "initialize" => {
                let requested = parsed
                    .pointer("/params/protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                    requested
                } else {
                    SUPPORTED_PROTOCOL_VERSIONS[0]
                };
                Some(jsonrpc_result(
                    &id,
                    &json!({
                        "protocolVersion": negotiated,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                        "instructions": "先用 list_projects 选择 repo，再用 search_graph/search_symbols 找定义；query 支持名称、签名和文档关键词，name_pattern 是正则，file_pattern 是仓库相对路径 glob。has_more=true 时带 next_offset 和 generation 继续翻页，保留其他参数；代际失效从 offset=0 重查。取得 qualified_name 后用 get_code_snippet 读源码、trace_path 查调用关系；同名歧义按 suggestions 重查。对证据文件调用 check_index_coverage，部分解析、过期或未索引时回读源码。查询按需触发后台增量检查，可用 index_status.refresh 查看排队/失败/退避状态；refresh_index 可立即刷新。tree-sitter 和启发式调用解析不能证明代码不存在。",
                    }),
                ))
            }
            m if m.starts_with("notifications/") => None,
            "ping" => Some(jsonrpc_result(&id, &json!({}))),
            "tools/list" => Some(jsonrpc_result(&id, &json!({ "tools": tool_definitions() }))),
            "tools/call" => {
                let name = parsed
                    .pointer("/params/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = parsed
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Some(jsonrpc_result(&id, &self.call_tool(name, &arguments)))
            }
            _ => Some(jsonrpc_error(&id, -32601, "Method not found")),
        }
    }
}

/// 读取索引统计为 JSON（index_status / refresh_index / list_projects 共用）。
fn stats_to_json(stats: &IndexStats) -> Value {
    json!({
        "status": "ready",
        "generation": stats.generation,
        "coverage": stats.coverage,
        "nodes": stats.nodes,
        "edges": stats.edges,
        "files": stats.files,
        "symbols": stats.symbols,
        "calls": stats.calls,
        "branch": stats.branch,
        "mode": stats.mode,
        "indexed_at": stats.indexed_at,
        "duration_ms": stats.duration_ms,
        "db_bytes": stats.db_bytes,
        "repo_path": stats.repo_path,
    })
}

/// list_projects / 多仓库错误提示共用的条目 JSON（统计 + repo 键）。
fn project_entry_json(repo_key: &str, stats: &IndexStats) -> Value {
    let mut value = stats_to_json(stats);
    value["repo"] = json!(repo_key);
    value
}

/// MCP content 信封：text 单行紧凑 JSON；非错误且可解析为对象时附
/// structuredContent（对齐参考项目 cbm_mcp_text_result 的双通道）。
fn text_result(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    let mut result = json!({ "content": [ { "type": "text", "text": text } ] });
    if is_error {
        result["isError"] = json!(true);
    } else if value.is_object() {
        result["structuredContent"] = value.clone();
    }
    result
}

fn jsonrpc_result(id: &Value, result: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn jsonrpc_error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

// ---------------------------------------------------------------------------
// 工具定义（name/title/description/inputSchema，中文描述与评审 agent 工具一致）
// ---------------------------------------------------------------------------

fn tool_definitions() -> Vec<Value> {
    let output_schema = json!({ "type": "object", "additionalProperties": true });
    // 多仓库模式共用的可选 repo 参数说明。
    let repo_prop = json!({
        "type": "string",
        "description": "可选：目标仓库（仓库绝对路径，或 list_projects 返回的 repo 键）。单仓库挂载、或本机仅一个已索引仓库时可省略；多个仓库且未传时会报错并列出可选清单"
    });
    let with_repo = |mut schema: Value| {
        schema["properties"]["repo"] = repo_prop.clone();
        schema
    };
    let mut definitions = vec![
        json!({
            "name": "list_projects",
            "title": "项目清单",
            "description": "列出本机全部已索引仓库（repo 键、仓库路径、文件/符号/边统计、最近索引时间）。多仓库模式下先用它了解可选目标；其余工具的 repo 参数可传仓库绝对路径或这里的 repo 键。",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "search_symbols",
            "title": "符号搜索",
            "description": "查找函数/类型/方法定义的首选工具。搜索名称、路径、声明签名和文档注释，支持 camelCase/snake_case 拆词；精确名称优先，多词完全无命中时自动放宽。默认只返回定义符号，可按标签、路径 glob、名称正则过滤。返回位置、签名、文档摘要与 total/has_more/next_offset；has_more 时保持参数并带 offset=next_offset 翻页。拿到 qualified_name 后可用 get_code_snippet / trace_path。索引是尽力解析，空结果不能证明代码不存在。",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索词，支持多词与驼峰拆分" },
                    "label": { "type": "string", "description": "可选：按节点标签过滤（Function/Method/Class/Struct/Interface/Enum/Trait/Type/Field）" },
                    "file_pattern": { "type": "string", "description": "仓库相对路径 glob，如 src/git/*；支持 *、?、[...]，不是正则" },
                    "name_pattern": { "type": "string", "description": "名称正则，如 ^(push|pull).*；与其他条件同时过滤" },
                    "qn_pattern": { "type": "string", "description": "qualified_name 正则，用于限定模块或类型作用域" },
                    "offset": { "type": "integer", "minimum": 0, "description": "跳过的结果数，默认 0；翻页传上次 next_offset" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "返回条数上限，默认 20，最大 200" }
                },
                "required": ["query"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_symbol_detail",
            "title": "符号详情",
            "description": "查某符号的定义详情：精确位置、直接调用方/被调用方、定义处源码片段（钳 200 行）。回答『这个函数在哪个文件、长什么样』用它。同名歧义时返回 status=ambiguous + suggestions 数组——改用其中的 qualified_name 重查，不要拿 MCP 工具名或猜测名当符号名。",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "符号名或完整 qualified_name" }
                },
                "required": ["name"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "trace_path",
            "title": "调用链追踪",
            "description": "沿 CALLS 边追踪调用方和被调用方，每跳带风险分级。function_name 优先传搜索所得 qualified_name；direction 默认 both，depth 默认 3、最大 8。callers_total/callees_total 是当前索引中该深度内的可达总数，两个方向分别分页（limit 默认 100，最大 200）；has_more 时保持参数并传 offset=next_offset。启发式解析可能漏掉动态调用，结果不是完整语义分析。",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "function_name": { "type": "string", "description": "函数/方法名" },
                    "direction": { "type": "string", "enum": ["inbound", "outbound", "both"], "description": "默认 both" },
                    "depth": { "type": "integer", "description": "BFS 层数，默认 3，最大 8" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "每个方向的返回条数，默认 100" },
                    "offset": { "type": "integer", "minimum": 0, "description": "每个方向跳过的节点数，默认 0" },
                    "risk_labels": { "type": "boolean", "description": "是否附带风险分级，默认 true" }
                },
                "required": ["function_name"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_architecture",
            "title": "架构概览",
            "description": "接手陌生仓库或开始代码审查前，先调用一次建立全局认知：节点/边总量与类型分布、语言构成、顶层目录符号密度、fan-in 调用热点 Top 10。一次调用替代数十次逐文件浏览。",
            "inputSchema": with_repo(json!({ "type": "object", "properties": {} })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "detect_changes",
            "title": "变更影响分析",
            "description": "提交或代码审查前评估未提交改动的影响面：工作区三态变更（未暂存+已暂存+未跟踪；可选 base_branch 走三点 diff）→ 受影响符号 → 上游调用方（带风险分级）。scope=files 只给文件清单，默认 symbols 给全量。注意：底层实现函数是 impacted_symbols / changed_files_via_git，不要拿本工具名当符号名去 search_symbols。",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["files", "symbols"], "description": "files=仅变更文件列表；symbols=文件+受影响符号+调用方（默认）" },
                    "base_branch": { "type": "string", "description": "可选：基线分支名（三点 diff，已提交+未提交一起算）" }
                }
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "index_status",
            "title": "索引状态",
            "description": "查询当前索引的状态与统计（文件/符号/边/分支/最近索引时间/库大小）。首次使用或怀疑索引过期时先调用确认；空索引时按返回的 hint 调 refresh_index。",
            "inputSchema": with_repo(json!({ "type": "object", "properties": {} })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "refresh_index",
            "title": "刷新索引",
            "description": "同步重建索引：incremental（默认，mtime+size 增量，通常秒级）或 full（全量重建）。长时间编辑后 search/trace 结果可疑时先刷新；工作区大量增删文件后也建议调用。对尚未索引的仓库调用即建立索引（多仓库模式需传 repo 参数为仓库绝对路径）。",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["incremental", "full"], "description": "默认 incremental" }
                }
            })),
            "outputSchema": output_schema,
        }),
    ];
    let mut search_graph = definitions[1].clone();
    search_graph["name"] = json!("search_graph");
    search_graph["title"] = json!("结构与关键词搜索");
    search_graph["description"] = json!("定位代码结构的首选工具。query 按名称、签名和文档关键词检索；也可省略 query，仅用 name_pattern/qn_pattern 正则、file_pattern 路径 glob 或 label 浏览定义。条件同时生效，返回位置、签名和摘要。total 是过滤后的真实总数；has_more=true 时保持条件并传 offset=next_offset 继续翻页。精确源码用 get_code_snippet，调用关系用 trace_path。空结果不是代码不存在的证明。");
    search_graph["inputSchema"].as_object_mut().unwrap().remove("required");
    search_graph["inputSchema"]["anyOf"] = json!([
        { "required": ["query"] }, { "required": ["name_pattern"] },
        { "required": ["qn_pattern"] }, { "required": ["file_pattern"] }, { "required": ["label"] }
    ]);
    let mut snippet = definitions[2].clone();
    snippet["name"] = json!("get_code_snippet");
    snippet["title"] = json!("定义源码");
    snippet["description"] = json!("先从 search_graph/search_symbols 取得 qualified_name，再精确读取该定义的源码（最多 200 行，truncated 表示截断）、签名和文档。返回的 source.lines 是源码行数组，source.start_line 是首行行号。该工具不搜索名称；同名歧义时按 suggestions 中的 qualified_name 重查。调用关系另用 trace_path。");
    let name_schema = snippet["inputSchema"]["properties"]["name"].clone();
    snippet["inputSchema"]["properties"].as_object_mut().unwrap().remove("name");
    snippet["inputSchema"]["properties"]["qualified_name"] = name_schema;
    snippet["inputSchema"]["required"] = json!(["qualified_name"]);
    definitions.extend([search_graph, snippet]);
    definitions.push(json!({
        "name": "check_index_coverage", "title": "索引覆盖检查",
        "description": "确认搜索或调用结果之前，检查已涉及文件的覆盖与新鲜度。paths 是精确相对路径数组；scopes 是目录前缀数组（. 表示仓库），至少传一个。返回 indexed/partial/read_failed/parse_failed/excluded/unsupported/not_indexed/unknown、语法错误范围、未解析调用数及 metadata_matches/metadata_changed。失败或部分覆盖需直接回读源码；状态正常也不证明语义完整。按 offset/next_offset 翻页。",
        "inputSchema": with_repo(json!({ "type": "object", "properties": {
            "paths": { "type": "array", "items": { "type": "string" } },
            "scopes": { "type": "array", "items": { "type": "string" } },
            "offset": { "type": "integer", "minimum": 0 },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
        }, "anyOf": [{ "required": ["paths"] }, { "required": ["scopes"] }] })),
        "outputSchema": output_schema
    }));
    for definition in &mut definitions {
        let read_only = definition["name"] != "refresh_index";
        definition["annotations"] = json!({ "readOnlyHint": read_only, "destructiveHint": false,
            "idempotentHint": true, "openWorldHint": false });
        if matches!(definition["name"].as_str(), Some("search_symbols" | "search_graph" | "trace_path" | "get_code_snippet" | "get_symbol_detail" | "get_architecture" | "check_index_coverage")) {
            definition["inputSchema"]["properties"]["generation"] = json!({ "type": "string", "description": "可选：沿用上一页返回的 generation，刷新后会明确报代际失效，需从 offset=0 重查" });
        }
    }
    definitions
}

// ---------------------------------------------------------------------------
// stdio 主循环
// ---------------------------------------------------------------------------

/// MCP 服务器入口：进入 stdin 消息循环直到 EOF。`Some(repo_path)` 为单仓库
/// 模式，`None` 为多仓库模式（见模块文档）。返回进程退出码。
pub fn run(repo_path: Option<&Path>) -> i32 {
    let server = match McpServer::new(repo_path) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("[khaslana-mcp] 启动失败：{error}");
            return 2;
        }
    };
    eprintln!(
        "[khaslana-mcp] {} 就绪（{}，{} 个工具）",
        SERVER_NAME,
        if repo_path.is_some() {
            "单仓库模式"
        } else {
            "多仓库模式"
        },
        tool_definitions().len()
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut reader = stdin.lock();
    let mut pending_content_length: Option<usize> = None;
    // 响应帧格式跟随请求：客户端以 Content-Length 帧发来时按同格式回
    // （对齐参考项目）；换行分隔则回换行。
    let mut content_length_mode = false;

    loop {
        let line = if let Some(length) = pending_content_length.take() {
            // Content-Length 帧模式：读满 length 字节。
            let mut frame = vec![0u8; length];
            if reader.read_exact(&mut frame).is_err() {
                break;
            }
            String::from_utf8_lossy(&frame).trim().to_string()
        } else {
            let mut raw = String::new();
            match reader.read_line(&mut raw) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    content_length_mode = false;
                    raw.trim().to_string()
                }
            }
        };

        if line.is_empty() {
            continue;
        }
        // 兼容 LSP 风格 Content-Length 帧：读头部直到空行，取长度后整帧读入。
        if !content_length_mode && line.to_ascii_lowercase().starts_with("content-length:") {
            content_length_mode = true;
            let length = line
                .split(':')
                .next_back()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            // 消费剩余头部行（直到空行）。
            let mut header_line = String::new();
            loop {
                header_line.clear();
                match reader.read_line(&mut header_line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if header_line.trim().is_empty() {
                            break;
                        }
                    }
                }
            }
            if length > MAX_FRAME_BYTES {
                eprintln!("[khaslana-mcp] 帧超过 {MAX_FRAME_BYTES} 字节上限，已丢弃");
                pending_content_length = None;
                continue;
            }
            pending_content_length = Some(length);
            continue;
        }

        if line.len() > MAX_FRAME_BYTES {
            eprintln!("[khaslana-mcp] 消息超过 {MAX_FRAME_BYTES} 字节上限，已丢弃");
            continue;
        }
        // 请求帧可能一帧一对象（换行分隔约定）；逐行处理。
        // panic 隔离：单条消息内的意外 panic（索引/文件状态异常等）降级为
        // 该请求的 -32603 Internal error，不杀死服务器进程。
        let request_id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned());
        let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            server.handle_message(&line)
        })) {
            Ok(response) => response,
            Err(payload) => {
                let message = if let Some(m) = payload.downcast_ref::<&str>() {
                    (*m).to_string()
                } else if let Some(m) = payload.downcast_ref::<String>() {
                    m.clone()
                } else {
                    "未知原因".to_string()
                };
                eprintln!("[khaslana-mcp] 消息处理 panic：{message}");
                Some(jsonrpc_error(
                    &request_id.unwrap_or(json!(null)),
                    -32603,
                    "Internal error: 工具调用异常，请重试或调用 refresh_index 后再试",
                ))
            }
        };
        if let Some(response) = response {
            if content_length_mode {
                let _ = write!(
                    stdout,
                    "Content-Length: {}\r\n\r\n{response}",
                    response.len()
                );
            } else {
                let _ = writeln!(stdout, "{response}");
            }
            let _ = stdout.flush();
        }
    }
    0
}
