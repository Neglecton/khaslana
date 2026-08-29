// 设置中心「代码索引」页：per-仓库开关、索引状态卡、重建/删除入口与
// 符号搜索验证卡。任务经 TaskKind::Index 池后台执行，事件按 repo_path
// 键控回传（索引中关闭仓库标签不影响完成落盘）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::{Context, Window, div, prelude::*, px};

use crate::tasks::TaskKind;
use crate::ui::{
    components::{dialog_actions, dialog_panel},
    theme::{self as ui_theme, rgb},
};
use crate::{
    CodeIndexTaskState, DialogState, FieldId, RepositoryView, UiEvent, normalize_repo_path,
    send_ui_event,
};
use khaslana::code_index::{
    IndexRunStats, PipelineOptions, RunOutcome, open_index_db_path, read_index_stats, run_index,
    search_symbols,
};

impl RepositoryView {
    // ------------------------------------------------------------------
    // 路径与偏好
    // ------------------------------------------------------------------

    /// 当前活动仓库的规范化路径（设置页/全局符号搜索面板作用对象）。
    pub(crate) fn active_repo_key(&self) -> Option<String> {
        let tab = self.active_tab()?;
        let path = tab.repo_path.as_ref()?;
        Some(normalize_repo_path(path))
    }

    fn active_repo_display_path(&self) -> Option<String> {
        let tab = self.active_tab()?;
        tab.repo_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
    }

    /// 某仓库的索引库文件路径（数据目录不可用时 None）。
    pub(crate) fn index_db_path(repo_key: &str) -> Option<PathBuf> {
        let data_dir = khaslana::storage::active_data_dir()?;
        open_index_db_path(&data_dir, &khaslana::ai::review_store::repo_key(repo_key)).ok()
    }

    // ------------------------------------------------------------------
    // 设置页渲染
    // ------------------------------------------------------------------

    pub(crate) fn render_code_index_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let repo_display = self.active_repo_display_path();
        let repo_key = self.active_repo_key();
        let enabled = repo_key
            .as_ref()
            .is_some_and(|key| self.code_index_enabled_cache.contains(key));
        let stats = repo_key
            .as_ref()
            .and_then(|key| self.code_index_stats.get(key));
        let running_on_active = repo_key.as_ref().is_some_and(|key| {
            self.code_index_task
                .as_ref()
                .is_some_and(|task| &task.repo_path == key)
        });
        // 配置片段为多仓库形态（零仓库耦合，一条配置服务所有已索引仓库）。
        // 单仓库高级用法可在 args 末尾追加 "<仓库路径>"。
        let mcp_config = {
            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
                .unwrap_or_else(|_| "khaslana.exe".to_string());
            format!(
                r#"{{"mcpServers":{{"khaslana-code-index":{{"command":"{exe}","args":["mcp"]}}}}}}"#
            )
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            // 当前仓库卡：路径 + 开关。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(match repo_display {
                                Some(path) => format!("当前仓库：{path}"),
                                None => "当前没有打开的仓库。先在仓库切换下拉中打开一个仓库，再回到此页启用索引。".to_string(),
                            }),
                    )
                    .when_some(repo_key.clone(), |this, _| {
                        this.child(self.toggle_row(
                            "code-index-enabled",
                            "为此仓库启用代码索引",
                            enabled,
                            |this, _, cx| {
                                let next = !self_enabled(this);
                                this.set_code_index_enabled_for_active(next, cx);
                            },
                            cx,
                        ))
                    }),
            )
            // 状态卡。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("索引状态"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(code_index_status_text(
                                running_on_active,
                                &self.code_index_progress_message,
                                stats,
                            )),
                    )
                    .when(running_on_active, |this| {
                        this.child(self.button("取消索引", true, |this, _, cx| {
                            this.cancel_code_index(cx);
                        }, cx))
                    })
                    .when(!running_on_active && stats.is_some(), |this| {
                        this.child(
                            dialog_actions()
                                .child(self.button(
                                    "增量更新",
                                    true,
                                    |this, _, cx| {
                                        this.start_code_index_task_from_settings(false, cx);
                                    },
                                    cx,
                                ))
                                .child(self.button(
                                    "重建索引",
                                    true,
                                    |this, _, cx| {
                                        this.start_code_index_task_from_settings(true, cx);
                                    },
                                    cx,
                                ))
                                .child(self.button(
                                    "删除索引数据",
                                    true,
                                    |this, _, _| {
                                        if this.active_repo_key().is_some() {
                                            this.active_dialog =
                                                Some(DialogState::ConfirmDeleteCodeIndex);
                                        }
                                    },
                                    cx,
                                )),
                        )
                    }),
            )
            // 符号搜索验证卡。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("符号搜索"),
                    )
                    .when_some(repo_key.clone(), |this, _| {
                        this.child(self.input(FieldId::CodeIndexSearch, false, window, cx))
                            .child(self.button("搜索", !self.code_index_searching, |this, _, cx| {
                                this.run_code_index_search(cx);
                            }, cx))
                    })
                    .when_some(self.code_index_search_hits.clone(), |this, hits| {
                        this.child(render_search_hits(&hits))
                    }),
            )
            // MCP 服务器接入卡：把索引暴露给外部 AI 工具。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("MCP 服务器"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("把代码索引以 MCP 工具暴露给外部 AI 工具（Claude Code / Cursor / ZCode 等）。命令：khaslana mcp（多仓库模式，一条配置服务所有已索引仓库，工具按 repo 参数选择目标；也可写 khaslana mcp <仓库路径> 固定单仓库）；启动后首次使用仓库时自动建立或增量刷新索引。以下配置可直接粘贴到 AI 工具的 MCP 设置中："),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::SURFACE_SUNKEN))
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(mcp_config.clone()),
                    )
                    .child(self.button(
                        "复制 MCP 配置",
                        true,
                        move |this, _, cx| {
                            let config = mcp_config.clone();
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(config));
                            this.notify_success("已复制 MCP 配置", cx);
                        },
                        cx,
                    )),
            )
            // 说明文案。
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!(
                        "索引基于 tree-sitter 解析仓库工作区（遵循 .gitignore），提取文件、符号、调用与导入关系存入本机 SQLite 库；{}。支持符号提取的语言：Rust、Python、JavaScript、TypeScript/TSX、Go、Java、C、C++、C#、PHP、Kotlin；其余文本文件仅登记为文件节点。",
                        "数据位于应用数据目录 code-index/ 下，按仓库哈希隔离",
                    )),
            )
    }

    // ------------------------------------------------------------------
    // 动作
    // ------------------------------------------------------------------

    /// 开关切换：写偏好表；开启即发起全量索引，关闭保留已建数据。
    pub(crate) fn set_code_index_enabled_for_active(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        match self.storage.set_code_index_enabled(&repo_key, enabled) {
            Ok(()) => {}
            Err(err) => {
                self.notify_error(format!("保存索引设置失败：{err}"), cx);
                return;
            }
        }
        if enabled {
            self.code_index_enabled_cache.insert(repo_key);
            self.start_code_index_task(true, cx);
        } else {
            self.code_index_enabled_cache.remove(&repo_key);
            self.status = "代码索引已关闭（已有索引数据保留）".into();
        }
        cx.notify();
    }

    /// 设置页按钮入口（活动仓库）。
    pub(crate) fn start_code_index_task_from_settings(
        &mut self,
        force_full: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        self.spawn_code_index_task_for(&repo_key, force_full, cx);
    }

    /// 发起索引任务（活动仓库；全局单任务守卫）。
    pub(crate) fn start_code_index_task(&mut self, force_full: bool, cx: &mut Context<Self>) {
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        if self.code_index_task.is_some() {
            self.notify_error("已有索引任务在进行中", cx);
            return;
        }
        self.spawn_code_index_task_for(&repo_key, force_full, cx);
    }

    /// 核心任务构造：按显式仓库键（自动触发可能来自非活动 tab 的事件）。
    fn spawn_code_index_task_for(
        &mut self,
        repo_key: &str,
        force_full: bool,
        cx: &mut Context<Self>,
    ) {
        if self.code_index_task.is_some() {
            return;
        }
        let Some(db_path) = Self::index_db_path(&repo_key) else {
            self.status = "代码索引不可用：数据目录无法创建".into();
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.code_index_task = Some(CodeIndexTaskState {
            repo_path: repo_key.to_string(),
            cancel: Arc::clone(&cancel),
        });
        self.code_index_progress_message = "准备索引…".to_string();

        let tx = self.tx.clone();
        let task_repo = repo_key.to_string();
        let fail_repo = task_repo.clone();
        self.tasks.spawn(TaskKind::Index, move || {
            let progress_tx = tx.clone();
            let event_repo = task_repo.clone();
            let mut options = PipelineOptions::new(
                Arc::clone(&cancel),
                Box::new(move |progress| {
                    send_ui_event(
                        &progress_tx,
                        UiEvent::CodeIndexProgress {
                            repo_path: event_repo.clone(),
                            message: progress.message,
                            done: progress.done,
                            total: progress.total,
                        },
                    );
                }),
            );
            let outcome = run_index(Path::new(&task_repo), &db_path, force_full, &mut options);
            match outcome {
                Ok(RunOutcome::Completed(stats)) => {
                    send_ui_event(
                        &tx,
                        UiEvent::CodeIndexFinished {
                            repo_path: task_repo,
                            stats: Some(stats),
                        },
                    );
                }
                Ok(RunOutcome::Unchanged) => {
                    send_ui_event(
                        &tx,
                        UiEvent::CodeIndexFinished {
                            repo_path: task_repo,
                            stats: None,
                        },
                    );
                }
                Ok(RunOutcome::Cancelled) => {
                    // 取消由 UI 侧置位时同步复位，无需事件。
                }
                Err(error) => {
                    send_ui_event(
                        &tx,
                        UiEvent::CodeIndexFailed {
                            repo_path: fail_repo,
                            error: error.to_string(),
                        },
                    );
                }
            }
        });
        cx.notify();
    }

    /// 自动触发（仓库加载完成后 / 工作区操作后）：仅在偏好启用且无在途任务时
    /// 发起增量检查；否则静默跳过。
    pub(crate) fn maybe_auto_code_index_refresh(
        &mut self,
        repo_path: Option<&Path>,
        cx: &mut Context<Self>,
    ) {
        if self.code_index_task.is_some() {
            return;
        }
        let Some(path) = repo_path else { return };
        let repo_key = normalize_repo_path(path);
        if !self.code_index_enabled_cache.contains(&repo_key) {
            return;
        }
        // 自动触发来自后台事件，目标仓库可能不是活动 tab——按显式键构造。
        self.spawn_code_index_task_for(&repo_key, false, cx);
    }

    pub(crate) fn cancel_code_index(&mut self, _cx: &mut Context<Self>) {
        if let Some(task) = self.code_index_task.take() {
            task.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.code_index_progress_message.clear();
            self.status = "代码索引已取消".into();
        }
    }

    pub(crate) fn confirm_delete_code_index_now(&mut self, cx: &mut Context<Self>) {
        self.active_dialog = None;
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        let Some(db_path) = Self::index_db_path(&repo_key) else {
            return;
        };
        let dir = db_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| db_path.clone());
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                self.code_index_stats.remove(&repo_key);
                self.code_index_search_hits = None;
                self.notify_success("索引数据已删除", cx);
            }
            Err(err) => {
                // 目录不存在视为已删除。
                if dir.exists() {
                    self.notify_error(format!("删除索引数据失败：{err}"), cx);
                    return;
                }
                self.code_index_stats.remove(&repo_key);
                self.notify_success("索引数据已删除", cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn refresh_code_index_stats(&mut self) {
        // 偏好缓存同步读主库（单行小表）。
        if let Ok(prefs) = self.storage.load_code_index_preferences() {
            self.code_index_enabled_cache = prefs
                .repositories
                .into_iter()
                .filter(|(_, enabled)| *enabled)
                .map(|(repo, _)| repo)
                .collect();
        } else {
            self.last_error = Some("读取代码索引设置失败".into());
        }
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        self.request_code_index_stats(&repo_key);
    }

    fn request_code_index_stats(&mut self, repo_key: &str) {
        let Some(db_path) = Self::index_db_path(repo_key) else {
            return;
        };
        let tx = self.tx.clone();
        let repo = repo_key.to_string();
        self.tasks.spawn(TaskKind::Short, move || {
            let stats = read_index_stats(&db_path).ok().flatten();
            send_ui_event(
                &tx,
                UiEvent::CodeIndexStatsLoaded {
                    repo_path: repo,
                    stats,
                },
            );
        });
    }

    pub(crate) fn run_code_index_search(&mut self, cx: &mut Context<Self>) {
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        let query = self.code_index_search_input.value.trim().to_string();
        if query.is_empty() {
            self.code_index_search_hits = None;
            cx.notify();
            return;
        }
        let Some(db_path) = Self::index_db_path(&repo_key) else {
            return;
        };
        self.code_index_searching = true;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let hits = search_symbols(&db_path, &query, 50).unwrap_or_default();
            send_ui_event(&tx, UiEvent::CodeIndexSearchFinished { hits });
        });
        cx.notify();
    }

    // ------------------------------------------------------------------
    // 事件处理（main.rs 的 handle_ui_event 分发到这里）
    // ------------------------------------------------------------------

    pub(crate) fn handle_code_index_progress(
        &mut self,
        repo_path: String,
        message: String,
        _done: usize,
        _total: usize,
    ) {
        if self
            .code_index_task
            .as_ref()
            .is_some_and(|t| t.repo_path == repo_path)
        {
            self.code_index_progress_message = message.clone();
        }
        // 进度同时反映到当前活动仓库的状态栏。
        if self.active_repo_key().as_deref() == Some(repo_path.as_str()) {
            self.status = format!("代码索引：{message}");
        }
    }

    pub(crate) fn handle_code_index_finished(
        &mut self,
        repo_path: String,
        stats: Option<IndexRunStats>,
        cx: &mut Context<Self>,
    ) {
        let was_tracked = self
            .code_index_task
            .as_ref()
            .is_some_and(|t| t.repo_path == repo_path);
        if was_tracked {
            self.code_index_task = None;
            self.code_index_progress_message.clear();
        }
        match stats {
            Some(stats) => {
                self.code_index_stats
                    .insert(repo_path.clone(), to_cached_stats(&stats));
                if self.active_repo_key().as_deref() == Some(repo_path.as_str()) {
                    self.notify_success(
                        format!(
                            "代码索引完成：{} 文件 · {} 符号 · {} 关系",
                            stats.files, stats.symbols, stats.edges
                        ),
                        cx,
                    );
                }
                // 完成即后台重读库统计：to_cached_stats 无 db_bytes/时间/分支
                // （这些只在库里），先粗填避免空窗，读库回来覆盖为精确值。
                self.request_code_index_stats(&repo_path);
            }
            None => {
                if self.active_repo_key().as_deref() == Some(repo_path.as_str()) {
                    self.status = "代码索引：内容无变化".into();
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn handle_code_index_failed(
        &mut self,
        repo_path: String,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .code_index_task
            .as_ref()
            .is_some_and(|t| t.repo_path == repo_path)
        {
            self.code_index_task = None;
            self.code_index_progress_message.clear();
        }
        tracing::warn!(target: "khaslana::code_index", "索引失败 {repo_path}: {error}");
        if self.active_repo_key().as_deref() == Some(repo_path.as_str()) {
            self.notify_error(format!("代码索引失败:{error}"), cx);
        }
        cx.notify();
    }

    pub(crate) fn handle_code_index_stats_loaded(
        &mut self,
        repo_path: String,
        stats: Option<khaslana::code_index::IndexStats>,
    ) {
        match stats {
            Some(stats) => {
                self.code_index_stats.insert(repo_path, stats);
            }
            None => {
                self.code_index_stats.remove(&repo_path);
            }
        }
    }

    pub(crate) fn handle_code_index_search_finished(
        &mut self,
        hits: Vec<khaslana::code_index::SearchHit>,
        cx: &mut Context<Self>,
    ) {
        self.code_index_searching = false;
        self.code_index_search_hits = Some(hits);
        cx.notify();
    }
}

/// toggle_row 闭包内拿不到外部变量，借 this 读当前开关态。
fn self_enabled(this: &RepositoryView) -> bool {
    this.active_repo_key()
        .is_some_and(|key| this.code_index_enabled_cache.contains(&key))
}

/// 状态卡文案。
fn code_index_status_text(
    running: bool,
    progress_message: &str,
    stats: Option<&khaslana::code_index::IndexStats>,
) -> String {
    if running {
        return format!("索引中：{progress_message}");
    }
    match stats {
        None => "未索引。开启开关后将自动建立全量索引。".to_string(),
        Some(s) => format!(
            "已索引 · {} 文件 / {} 符号 / {} 关系（调用 {}）· 分支 {} · {} · 库大小 {:.1} MB",
            s.files,
            s.symbols,
            s.edges,
            s.calls,
            if s.branch.is_empty() { "-" } else { &s.branch },
            format_indexed_at(s.indexed_at),
            s.db_bytes as f64 / (1024.0 * 1024.0),
        ),
    }
}

fn format_indexed_at(millis: u64) -> String {
    use chrono::TimeZone;
    if millis == 0 {
        return "-".to_string();
    }
    chrono::Local
        .timestamp_millis_opt(millis as i64)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn render_search_hits(hits: &[khaslana::code_index::SearchHit]) -> impl IntoElement {
    let rows: Vec<gpui::AnyElement> = hits
        .iter()
        .take(50)
        .map(|hit| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                .child(
                    div()
                        .px_1()
                        .rounded(px(ui_theme::RADIUS_XS))
                        .bg(rgb(ui_theme::SURFACE_SUNKEN))
                        .text_color(rgb(ui_theme::PRIMARY))
                        .child(hit.label.clone()),
                )
                .child(
                    div()
                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                        .child(hit.name.clone()),
                )
                .child(format!("{}:{}", hit.file_path, hit.start_line))
                .into_any_element()
        })
        .collect();
    div().flex().flex_col().gap_1().children(rows)
}

impl RepositoryView {
    /// 「删除索引数据」确认弹窗。
    pub(crate) fn render_code_index_delete_confirm(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        dialog_panel("删除索引数据")
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("确认删除当前仓库的代码索引数据？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_ERROR_FOREGROUND))
                    .child("删除后需要重新建立全量索引才能恢复搜索能力；不影响仓库本身。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        true,
                        |this, _, cx| {
                            this.confirm_delete_code_index_now(cx);
                        },
                        cx,
                    )),
            )
    }
}

/// IndexRunStats -> 统计缓存条目（db_bytes 由 read_index_stats 单独补齐）。
fn to_cached_stats(stats: &IndexRunStats) -> khaslana::code_index::IndexStats {
    khaslana::code_index::IndexStats {
        files: stats.files,
        symbols: stats.symbols,
        edges: stats.edges,
        calls: stats.calls,
        duration_ms: stats.duration_ms,
        ..Default::default()
    }
}
