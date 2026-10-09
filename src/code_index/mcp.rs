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

#[path = "mcp_transport.rs"]
mod transport;

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

#[cfg(test)]
#[path = "../tests/code_index_mcp_protocol.rs"]
mod protocol_tests;

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
        let repo = git2::Repository::open(&repo_root)
            .map_err(|e| super::err(format!("{} 不是 Git 仓库：{e}", repo_root.display())))?;
        let repo_root = repo.workdir().ok_or_else(|| super::err("裸仓库没有工作区，不能建立工作区代码索引"))?
            .canonicalize().map_err(|e| super::err(format!("仓库工作区不可用：{e}")))?;
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
                // 路由只需要仓库路径；不能每次 search/trace 都先 COUNT 全库节点和边。
                use rusqlite::OptionalExtension;
                let store = super::store::open_read_only_if_exists(&db_path)
                    .map_err(|error| json!({ "error": error.to_string() }))?
                    .ok_or_else(|| json!({ "error": "索引库不可用或版本不兼容", "repo": key }))?;
                let root: Option<String> = store.conn.query_row("SELECT value FROM meta WHERE key='repo_path'", [], |row| row.get(0))
                    .optional().map_err(|error| json!({ "error": format!("读取仓库路径失败：{error}") }))?;
                return Ok(RepoContext { repo_root: root.filter(|root| !root.is_empty()).map(PathBuf::from),
                    db_path, repo_key: key });
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
        entries.sort_by(|a, b| b.1.indexed_at.cmp(&a.1.indexed_at).then(a.0.cmp(&b.0)));
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
        let guarded = matches!(name, "search_symbols" | "search_graph" | "get_symbol_detail" | "get_code_snippet" | "trace_path" | "get_architecture" | "check_index_coverage" | "detect_changes");
        let generation = if guarded {
            match super::store::index_generation(&ctx.db_path) {
                Ok(generation) => generation,
                Err(error) => return text_result(&json!({ "error": error.to_string() }), true),
            }
        } else { None };
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
                    let current = match super::store::index_generation(&ctx.db_path) {
                        Ok(generation) => generation,
                        Err(error) => return text_result(&json!({ "error": error.to_string() }), true),
                    };
                    if generation != current {
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
                value["coverage_note"] = json!("total 统计指定深度内去重的节点，不是调用次数；hop=1 的 call_sites 给出直接边的调用次数和最多 100 个行号。启发式调用图仍可能缺少动态调用或类型信息。");
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
                if stats.needs_rebuild { value["hint"] = json!("索引需升级重建；等待后台刷新结束，或调用 refresh_index，再确认 needs_rebuild=false"); }
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
        let db_path = ctx.db_path.clone();
        let scheduled = self.coordinator.schedule(ctx.repo_key.clone(), true, move |cancel| {
            Self::run_index_inner(&repo_root, &db_path, force_full, cancel, |message| {
                eprintln!("[khaslana-mcp] {message}");
            }).map_err(|error| error.to_string())
        });
        if !scheduled && (force_full || !self.indexing(&ctx.repo_key)) {
            return Err(json!({ "error": "刷新未安排：同仓库任务仍在运行或调度器已关闭",
                "repo": ctx.repo_key, "scheduled": false, "refresh": self.coordinator.status(&ctx.repo_key),
                "hint": "先用 index_status 等待当前任务结束，再重新调用 refresh_index" }));
        }
        // stdio 串行分发不能等分钟级建库；小任务保留直接返回结果的兼容行为。
        if !self.coordinator.wait_timeout(&ctx.repo_key, std::time::Duration::from_millis(100)) {
            return Ok(json!({ "status": "indexing", "indexing": true, "repo": ctx.repo_key,
                "scheduled": scheduled, "refresh": self.coordinator.status(&ctx.repo_key),
                "hint": "索引仍在后台处理，请用 index_status 查询完成或失败状态；无需重复 refresh_index" }));
        }
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
                return Some(jsonrpc_error(&Value::Null, -32700, "Parse error"));
            }
        };
        let valid_id = parsed.get("id").is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
        if !parsed.is_object() || parsed["jsonrpc"] != "2.0" || !parsed["method"].is_string() || !valid_id {
            return Some(jsonrpc_error(&Value::Null, -32600, "Invalid Request"));
        }
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
        if parsed.get("params").is_some_and(|params| !params.is_object()) {
            return Some(jsonrpc_error(&id, -32602, "params 必须是对象"));
        }

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
                        "instructions": "Select repo via list_projects; omit only for a single repo. Single-repo servers ignore repo overrides; ambiguous selection lists choices. Search definitions, then pass returned qualified_name to get_code_snippet or trace_path; resolve ambiguity via suggestions, never guess symbols from tool names. When has_more, keep filters and use offset=next_offset plus generation; on stale generation restart at offset=0 without generation. Check evidence paths with check_index_coverage; read source for partial, stale or missing coverage. Tree-sitter and heuristic calls are incomplete: empty results do not prove absence; clean coverage does not prove completeness. Queries trigger background incremental checks; index_status.refresh reports queue, errors and backoff. After refresh_index returns indexing, poll index_status for completion/failure, do not refresh repeatedly.",
                    }),
                ))
            }
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
                let Some(definition) = tool_definitions().iter().find(|tool| tool["name"] == name) else {
                    return Some(jsonrpc_error(&id, -32602, &format!("未知工具 {name}")));
                };
                if let Err(error) = validate_arguments(&definition["inputSchema"], &arguments, "arguments") {
                    return Some(jsonrpc_error(&id, -32602, &error));
                }
                Some(jsonrpc_result(&id, &self.call_tool(name, &arguments)))
            }
            _ => Some(jsonrpc_error(&id, -32601, "Method not found")),
        }
    }
}

/// 读取索引统计为 JSON（index_status / refresh_index / list_projects 共用）。
fn stats_to_json(stats: &IndexStats) -> Value {
    json!({
        "status": if stats.needs_rebuild { "rebuild_required" } else { "ready" },
        "generation": stats.generation,
        "needs_rebuild": stats.needs_rebuild,
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

/// 只校验本服务工具清单使用的 schema 子集，避免文档与实际参数规则各维护一套。
fn validate_arguments(schema: &Value, value: &Value, path: &str) -> std::result::Result<(), String> {
    let valid_type = match schema["type"].as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("boolean") => value.is_boolean(),
        _ => true,
    };
    if !valid_type { return Err(format!("{path} 类型无效，应为 {}", schema["type"])); }
    if let Some(values) = schema["enum"].as_array() {
        if !values.contains(value) { return Err(format!("{path} 不在允许的取值中")); }
    }
    if let Some(minimum) = schema["minimum"].as_i64() {
        if value.as_i64().is_some_and(|number| number < minimum) {
            return Err(format!("{path} 不得小于 {minimum}"));
        }
    }
    if let Some(maximum) = schema["maximum"].as_u64() {
        if value.as_u64().is_some_and(|number| number > maximum) {
            return Err(format!("{path} 不得大于 {maximum}"));
        }
    }
    if let Some(required) = schema["required"].as_array() {
        for key in required.iter().filter_map(Value::as_str) {
            if value.get(key).is_none() { return Err(format!("缺少必填参数 {path}.{key}")); }
        }
    }
    if let Some(choices) = schema["anyOf"].as_array() {
        if !choices.iter().any(|choice| validate_arguments(choice, value, path).is_ok()) {
            return Err(format!("{path} 缺少必需的查询条件，请参考 tools/list"));
        }
    }
    if let Some(properties) = schema["properties"].as_object() {
        for (key, property) in properties {
            if let Some(value) = value.get(key) { validate_arguments(property, value, &format!("{path}.{key}"))?; }
        }
    }
    if let Some(items) = value.as_array() {
        for (index, item) in items.iter().enumerate() {
            validate_arguments(&schema["items"], item, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 工具定义：中文展示标题，紧凑英文供模型读取；默认值由 schema 表达。
// ---------------------------------------------------------------------------

fn tool_definitions() -> &'static [Value] {
    static DEFINITIONS: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    DEFINITIONS.get_or_init(build_tool_definitions)
}

fn build_tool_definitions() -> Vec<Value> {
    let output_schema = json!({ "type": "object", "additionalProperties": true });
    // 多仓库模式共用的可选 repo 参数说明。
    let repo_prop = json!({
        "type": "string",
        "description": "Absolute worktree path or list_projects repo key; omit for a single repo."
    });
    let with_repo = |mut schema: Value| {
        schema["properties"]["repo"] = repo_prop.clone();
        schema
    };
    let mut definitions = vec![
        json!({
            "name": "list_projects",
            "title": "项目清单",
            "description": "List local indexed repos: keys, paths, file/symbol/edge counts and index times. Select a repo for subsequent tools.",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "search_symbols",
            "title": "符号搜索",
            "description": "Keyword search over definition names, paths, signatures and docs. Exact names rank first; relax all-words matching only if empty. Filters AND together. Returns locations, signatures, doc excerpts and total/has_more/next_offset; use qualified_name for snippet/trace. No match is not proof of absence.",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Names/paths/signatures/docs; splits camelCase/snake_case." },
                    "label": { "type": "string", "description": "Node label: Function/Method/Class/Struct/Interface/Enum/Trait/Type/Field." },
                    "file_pattern": { "type": "string", "description": "Repo-relative glob (* ? [...]), e.g. src/git/*; not regex." },
                    "name_pattern": { "type": "string", "description": "Name regex, e.g. ^(push|pull).*" },
                    "qn_pattern": { "type": "string", "description": "Qualified-name regex for module/type scope." },
                    "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Use next_offset with unchanged filters and generation." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20 }
                },
                "required": ["query"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_symbol_detail",
            "title": "符号详情",
            "description": "Definition location, direct callers/callees and source (max 200 lines). On status=ambiguous, retry a suggestions qualified_name. Use names from search, not tool names.",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Symbol name or exact qualified_name from search." }
                },
                "required": ["name"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "trace_path",
            "title": "调用链追踪",
            "description": "Traverse CALLS with risk labels. Totals count distinct reachable nodes within depth, not calls; directions page independently. hop=1 call_sites has count/file_path/lines (max 100, truncated). Heuristic links may miss calls.",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "function_name": { "type": "string", "description": "Function/method name; prefer qualified_name from search." },
                    "direction": { "type": "string", "enum": ["inbound", "outbound", "both"], "default": "both", "description": "inbound=callers; outbound=callees." },
                    "depth": { "type": "integer", "minimum": 1, "maximum": 8, "default": 3, "description": "BFS depth." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 100, "description": "Nodes per direction." },
                    "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Skip per direction; use next_offset with unchanged args and generation." },
                    "risk_labels": { "type": "boolean", "default": true }
                },
                "required": ["function_name"]
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_architecture",
            "title": "架构概览",
            "description": "Repo overview: node/edge totals and types, languages, top-level folder symbol density and top 10 fan-in hotspots.",
            "inputSchema": with_repo(json!({ "type": "object", "properties": {} })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "detect_changes",
            "title": "变更影响分析",
            "description": "Impact of unstaged, staged and untracked changes: files, affected symbols and upstream callers with risk labels. Optional base_branch includes committed changes via three-dot diff.",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["files", "symbols"], "default": "symbols", "description": "files=paths only; symbols=files, symbols and callers." },
                    "base_branch": { "type": "string", "description": "Three-dot diff base; includes working-tree changes." }
                }
            })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "index_status",
            "title": "索引状态",
            "description": "Index stats and this process's refresh queue/errors/backoff. If empty or needs_rebuild, use refresh_index; old relations are blocked until rebuilt.",
            "inputSchema": with_repo(json!({ "type": "object", "properties": {} })),
            "outputSchema": output_schema,
        }),
        json!({
            "name": "refresh_index",
            "title": "刷新索引",
            "description": "Refresh index; first build needs an absolute repo path. Returns ready or indexing: poll index_status, do not repeat refresh. An active local job is reused for incremental; full is rejected until it finishes.",
            "inputSchema": with_repo(json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["incremental", "full"], "default": "incremental", "description": "incremental=mtime+size check; full=rebuild." }
                }
            })),
            "outputSchema": output_schema,
        }),
    ];
    let mut search_graph = definitions[1].clone();
    search_graph["name"] = json!("search_graph");
    search_graph["title"] = json!("结构与关键词搜索");
    search_graph["description"] = json!("Find definitions by keywords and/or structural filters (AND). Without query, supply any regex/glob/label filter. Returns locations, signatures, doc excerpts, filtered total/has_more/next_offset. Use qualified_name for snippet/trace. No match is not proof of absence.");
    search_graph["inputSchema"].as_object_mut().unwrap().remove("required");
    search_graph["inputSchema"]["anyOf"] = json!([
        { "required": ["query"] }, { "required": ["name_pattern"] },
        { "required": ["qn_pattern"] }, { "required": ["file_pattern"] }, { "required": ["label"] }
    ]);
    let mut snippet = definitions[2].clone();
    snippet["name"] = json!("get_code_snippet");
    snippet["title"] = json!("定义源码");
    snippet["description"] = json!("Read a definition found by search_graph/search_symbols, not a search tool. Returns signature, docs and source.lines/start_line (max 200; source.truncated). On ambiguity, retry a suggestions qualified_name. For calls use trace_path.");
    let name_schema = snippet["inputSchema"]["properties"]["name"].clone();
    snippet["inputSchema"]["properties"].as_object_mut().unwrap().remove("name");
    snippet["inputSchema"]["properties"]["qualified_name"] = name_schema;
    snippet["inputSchema"]["required"] = json!(["qualified_name"]);
    definitions.extend([search_graph, snippet]);
    definitions.push(json!({
        "name": "check_index_coverage", "title": "索引覆盖检查",
        "description": "Check evidence files: coverage status, parse gaps, unresolved calls and metadata freshness. unresolved_no_candidate=no same-language repo definition; unresolved_with_candidates=target uncertain. Neither is a parse error rate. Read source for gaps/stale/missing coverage; clean status is no proof of completeness. Supply paths or scopes.",
        "inputSchema": with_repo(json!({ "type": "object", "properties": {
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Exact repo-relative file paths." },
            "scopes": { "type": "array", "items": { "type": "string" }, "description": "Directory prefixes; '.' covers the repo." },
            "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Use next_offset with unchanged args and generation." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
        }, "anyOf": [{ "required": ["paths"] }, { "required": ["scopes"] }] })),
        "outputSchema": output_schema
    }));
    for definition in &mut definitions {
        let read_only = definition["name"] != "refresh_index";
        definition["annotations"] = json!({ "readOnlyHint": read_only, "destructiveHint": false,
            "idempotentHint": true, "openWorldHint": false });
        if definition["inputSchema"]["properties"].get("offset").is_some() {
            definition["inputSchema"]["properties"]["offset"]["maximum"] = json!(i64::MAX);
        }
        if matches!(definition["name"].as_str(), Some("search_symbols" | "search_graph" | "trace_path" | "get_code_snippet" | "get_symbol_detail" | "get_architecture" | "check_index_coverage" | "detect_changes")) {
            definition["inputSchema"]["properties"]["generation"] = json!({ "type": "string", "description": "Reuse result generation; if stale, omit and restart at offset=0." });
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
    let stdout = std::io::stdout();
    match transport::serve(&server, &mut stdin.lock(), &mut stdout.lock()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[khaslana-mcp] 连接结束：{error}");
            1
        }
    }
}
