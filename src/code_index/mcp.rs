//! 内嵌 MCP stdio 服务器（协议行为逐条对照 codebase-memory-mcp 的
//! `src/mcp/mcp.c`：换行分隔 JSON 帧 + Content-Length 兼容、单行紧凑 JSON
//! text 载荷 + structuredContent 双通道、业务错误 isError + 自纠错 hint、
//! 字符串/数字 id 原样回显、单线程串行处理）。
//!
//! 入口 `run(repo_path)`：校验仓库 → 索引保障（无库自动全量建、有库增量
//! 检查，对齐参考项目 auto_index——MCP 配置本身就是显式的 per-仓库意图）
//! → 进入 stdin 消息循环。stdout 只走协议，进度/日志一律 stderr。
//!
//! 由 `main()` 顶部的 `khaslana mcp <仓库路径>` 子命令分支调用（先于
//! GUI 启动），供 Claude Code / Cursor / ZCode 等 AI 工具作为 MCP 服务器
//! 挂载，查询本仓库的代码索引。

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::queries::{self, DetailOutcome, TraceDirection, TraceOutcome};
use super::store::{IndexStats, read_index_stats, search_symbols};
use crate::types::Result;

/// 服务器支持的 MCP 协议版本（协商：客户端声明版本在列表内则回显，否则回最新）。
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "khaslana-code-index";
/// 单帧字节上限（防恶意/损坏输入撑爆内存；对齐参考项目 10MB）。
const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
/// trace_path / 详情查询的默认 BFS 节点上限（对齐参考项目 MCP_BFS_LIMIT）。
const DEFAULT_MAX_NODES: usize = 100;

/// 服务器状态：一个会话只服务一个仓库。
pub struct McpServer {
    repo_root: PathBuf,
    db_path: PathBuf,
}

impl McpServer {
    /// 初始化：校验仓库 → 索引保障。失败返回 Err（调用方写 stderr 后退出）。
    pub fn new(repo_path: &Path) -> Result<Self> {
        let repo_root = repo_path
            .canonicalize()
            .map_err(|e| super::err(format!("仓库路径不可用 {}: {e}", repo_path.display())))?;
        git2::Repository::open(&repo_root)
            .map_err(|e| super::err(format!("{} 不是 Git 仓库：{e}", repo_root.display())))?;
        let repo_key = crate::ai::review_store::repo_key(&repo_root.to_string_lossy());
        let data_dir = crate::storage::active_data_dir()
            .ok_or_else(|| super::err("数据目录不可用，无法定位索引库"))?;
        let db_path = super::open_index_db_path(&data_dir, &repo_key)?;

        let server = Self { repo_root, db_path };
        server.ensure_index()?;
        Ok(server)
    }

    /// 测试专用：跳过仓库校验与索引保障，直接以给定路径构造。
    #[cfg(test)]
    pub(crate) fn for_test(repo_root: &Path, db_path: PathBuf) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            db_path,
        }
    }

    /// 索引保障：无库（或空库，上次取消/中断）自动全量建，有库增量检查
    /// （无变化零写入）。进度一律写 stderr。
    fn ensure_index(&self) -> Result<()> {
        let existing = read_index_stats(&self.db_path).ok().flatten();
        let needs_full = existing.as_ref().is_none_or(|s| s.nodes == 0);
        eprintln!(
            "[khaslana-mcp] 索引{}，启动{}…",
            if needs_full {
                "不存在或为空，将全量建立"
            } else {
                "已存在"
            },
            if needs_full {
                "全量建"
            } else {
                "增量检查"
            },
        );
        self.run_index_inner(needs_full, |message| eprintln!("[khaslana-mcp] {message}"))
    }

    fn run_index_inner(&self, force_full: bool, progress: fn(String)) -> Result<()> {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut options = super::PipelineOptions::new(
            cancel,
            Box::new(move |p| progress(format!("{} {}/{}", p.phase.display(), p.done, p.total))),
        );
        match super::run_index(&self.repo_root, &self.db_path, force_full, &mut options)? {
            super::RunOutcome::Completed(stats) => progress(format!(
                "索引完成：{} 文件 / {} 符号 / {} 边",
                stats.files, stats.symbols, stats.edges
            )),
            super::RunOutcome::Unchanged => progress("索引无变化".to_string()),
            super::RunOutcome::Cancelled => progress("索引已取消".to_string()),
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 工具分发
    // ------------------------------------------------------------------

    /// tools/call 分发：返回 MCP content 信封（业务错误 isError:true）。
    fn call_tool(&self, name: &str, arguments: &Value) -> Value {
        let result = match name {
            "search_symbols" => self.tool_search_symbols(arguments),
            "get_symbol_detail" => self.tool_symbol_detail(arguments),
            "trace_path" => self.tool_trace_path(arguments),
            "get_architecture" => self.tool_architecture(),
            "detect_changes" => self.tool_detect_changes(arguments),
            "index_status" => self.tool_index_status(),
            "refresh_index" => self.tool_refresh_index(arguments),
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
            Ok(value) => text_result(&value, false),
            Err(value) => text_result(&value, true),
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

    fn tool_search_symbols(&self, args: &Value) -> std::result::Result<Value, Value> {
        let Some(query) = Self::arg_str(args, "query") else {
            return Err(Self::missing_arg_error("query"));
        };
        let label = Self::arg_str(args, "label");
        let limit = Self::arg_usize(args, "limit").unwrap_or(20).clamp(1, 200);
        let all = search_symbols(&self.db_path, query, limit * 2).unwrap_or_default();
        let total_all = all.len();
        let hits: Vec<Value> = all
            .into_iter()
            .filter(|hit| label.is_none_or(|l| hit.label.eq_ignore_ascii_case(l)))
            .take(limit)
            .map(|hit| {
                json!({
                    "name": hit.name,
                    "label": hit.label,
                    "qualified_name": hit.qualified_name,
                    "file_path": hit.file_path,
                    "start_line": hit.start_line,
                })
            })
            .collect();
        let count = hits.len();
        let mut result = json!({ "total": count, "results": hits, "has_more": total_all > count });
        if count == 0 {
            result["hint"] = json!(
                "无命中。改用更短的词（如 push 代替 pushBranch）；FTS5 按 camelCase 拆分匹配。"
            );
        }
        Ok(result)
    }

    fn tool_symbol_detail(&self, args: &Value) -> std::result::Result<Value, Value> {
        let Some(name) = Self::arg_str(args, "name") else {
            return Err(Self::missing_arg_error("name"));
        };
        let outcome = queries::symbol_detail(&self.db_path, Some(&self.repo_root), name)
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

    fn tool_trace_path(&self, args: &Value) -> std::result::Result<Value, Value> {
        let Some(function_name) = Self::arg_str(args, "function_name") else {
            return Err(Self::missing_arg_error("function_name"));
        };
        let direction = TraceDirection::parse(Self::arg_str(args, "direction").unwrap_or("both"));
        let depth = Self::arg_usize(args, "depth").unwrap_or(3).clamp(1, 8) as u32;
        let risk_labels = args
            .get("risk_labels")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let outcome = queries::trace_calls(
            &self.db_path,
            function_name,
            direction,
            depth,
            DEFAULT_MAX_NODES,
        )
        .map_err(|e| json!({ "error": e.to_string() }))?;
        match outcome {
            TraceOutcome::Found(mut result) => {
                if !risk_labels {
                    for hop in result.callers.iter_mut().chain(result.callees.iter_mut()) {
                        hop.risk = "";
                    }
                }
                Ok(serde_json::to_value(&result)
                    .map_err(|e| json!({ "error": format!("序列化结果失败：{e}") }))?)
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

    fn tool_architecture(&self) -> std::result::Result<Value, Value> {
        let overview = queries::index_overview(&self.db_path)
            .map_err(|e| json!({ "error": e.to_string() }))?;
        serde_json::to_value(&overview).map_err(|e| json!({ "error": format!("序列化失败：{e}") }))
    }

    fn tool_detect_changes(&self, args: &Value) -> std::result::Result<Value, Value> {
        let scope = Self::arg_str(args, "scope").unwrap_or("symbols");
        let base_branch = Self::arg_str(args, "base_branch");
        let changed_files = queries::changed_files_via_git(&self.repo_root, base_branch)
            .map_err(|e| json!({ "error": e.to_string() }))?;
        let mut result = match scope {
            "files" => json!({
                "changed_files": changed_files,
                "changed_count": changed_files.len(),
            }),
            _ => {
                // symbols：文件 + 受影响符号 + 上游调用方展开。
                let report = queries::impacted_symbols_for_files(&self.db_path, &changed_files, 1)
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

    fn tool_index_status(&self) -> std::result::Result<Value, Value> {
        let stats =
            read_index_stats(&self.db_path).map_err(|e| json!({ "error": e.to_string() }))?;
        match stats {
            Some(stats) => Ok(stats_to_json(&stats)),
            None => Ok(json!({
                "status": "empty",
                "hint": "索引为空。调用 refresh_index {\"mode\":\"full\"} 建立全量索引",
            })),
        }
    }

    fn tool_refresh_index(&self, args: &Value) -> std::result::Result<Value, Value> {
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
        self.run_index_inner(force_full, |message| eprintln!("[khaslana-mcp] {message}"))
            .map_err(|e| json!({ "error": e.to_string() }))?;
        let stats = read_index_stats(&self.db_path)
            .map_err(|e| json!({ "error": e.to_string() }))?
            .ok_or_else(|| json!({ "error": "索引完成后统计仍为空" }))?;
        let mut value = stats_to_json(&stats);
        value["status"] = json!("ready");
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
        // 通知（无 id）不响应；请求缺 id 时按 JSON-RPC 惯例以 null 回应。
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

/// 读取索引统计为 JSON（index_status / refresh_index 共用）。
fn stats_to_json(stats: &IndexStats) -> Value {
    json!({
        "status": "ready",
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
    })
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
    vec![
        json!({
            "name": "search_symbols",
            "title": "符号搜索",
            "description": "按名称模糊检索代码索引中的符号（FTS5，camelCase/snake_case 拆分感知：'push branch' 可命中 pushBranch）。返回 name/label/qualified_name/file_path/start_line。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索词，支持多词与驼峰拆分" },
                    "label": { "type": "string", "description": "可选：按节点标签过滤（Function/Method/Class/Struct/Interface/Enum/Trait/Type/Field）" },
                    "limit": { "type": "integer", "description": "返回条数上限，默认 20" }
                },
                "required": ["query"]
            },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_symbol_detail",
            "title": "符号详情",
            "description": "按名称或 qualified_name 精确查符号：定义位置、直接调用方/被调用方、定义处源码片段。同名歧义时返回 suggestions，请改用 qualified_name 重查。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "符号名或完整 qualified_name" }
                },
                "required": ["name"]
            },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "trace_path",
            "title": "调用链追踪",
            "description": "沿 CALLS 边做 BFS 追踪调用链。inbound=谁调用它（向上游），outbound=它调用谁（向下游），both=双向。每跳带 risk 风险分级（hop 1 最高的 CRITICAL，随距离衰减）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "function_name": { "type": "string", "description": "函数/方法名" },
                    "direction": { "type": "string", "enum": ["inbound", "outbound", "both"], "description": "默认 both" },
                    "depth": { "type": "integer", "description": "BFS 层数，默认 3，最大 8" },
                    "risk_labels": { "type": "boolean", "description": "是否附带风险分级，默认 true" }
                },
                "required": ["function_name"]
            },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "get_architecture",
            "title": "架构概览",
            "description": "索引总览：节点/边总量与类型分布、语言分布、顶层目录符号密度、fan-in 调用热点 Top 10。适合在探索一个陌生仓库时先调用。",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "detect_changes",
            "title": "变更影响分析",
            "description": "收集工作区变更（未暂存+已暂存+未跟踪；可选 base_branch 三点 diff）并映射到受影响符号及其上游调用方（带风险分级）。提交代码审查前用它评估影响面。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["files", "symbols"], "description": "files=仅变更文件列表；symbols=文件+受影响符号+调用方（默认）" },
                    "base_branch": { "type": "string", "description": "可选：基线分支名（三点 diff，已提交+未提交一起算）" }
                }
            },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "index_status",
            "title": "索引状态",
            "description": "当前仓库索引的统计（文件/符号/边/分支/最近索引时间/库大小）。空索引时返回 hint 引导 refresh_index。",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": output_schema,
        }),
        json!({
            "name": "refresh_index",
            "title": "刷新索引",
            "description": "同步重建索引：incremental（默认，mtime+size 增量）或 full（全量重建）。工作区大量改动后可调用以保鲜。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["incremental", "full"], "description": "默认 incremental" }
                }
            },
            "outputSchema": output_schema,
        }),
    ]
}

// ---------------------------------------------------------------------------
// stdio 主循环
// ---------------------------------------------------------------------------

/// MCP 服务器入口：进入 stdin 消息循环直到 EOF。返回进程退出码。
pub fn run(repo_path: &Path) -> i32 {
    let server = match McpServer::new(repo_path) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("[khaslana-mcp] 启动失败：{error}");
            return 2;
        }
    };
    eprintln!(
        "[khaslana-mcp] {} 就绪（{} 个工具）",
        SERVER_NAME,
        tool_definitions().len()
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut reader = stdin.lock();
    let mut pending_content_length: Option<usize> = None;

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
                Ok(_) => raw.trim().to_string(),
            }
        };

        if line.is_empty() {
            continue;
        }
        // 兼容 LSP 风格 Content-Length 帧：读头部直到空行，取长度后整帧读入。
        if pending_content_length.is_none()
            && line.to_ascii_lowercase().starts_with("content-length:")
        {
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
        if let Some(response) = server.handle_message(&line) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
    0
}
