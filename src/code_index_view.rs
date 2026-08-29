// 设置中心「代码索引」页：MCP 接入卡 + 多仓库列表（每仓库开关、状态徽标、
// 进度条与增量/重建/删除入口）。列表条目在打开设置页时构建（已打开 tabs +
// 最近仓库 + 索引偏好记录三路合并），随设置页内容区整体滚动；任务仍为全局
// 单任务（TaskKind::Index 池单线程 + code_index_task 守卫双保险），事件按
// repo_path 键控回传（索引中关闭仓库标签不影响完成落盘）。

use std::collections::HashSet;
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
    CodeIndexListEntry, CodeIndexTaskState, DialogState, FieldId, RepositoryView, UiEvent,
    normalize_repo_path, send_ui_event,
};
use khaslana::code_index::{
    IndexRunStats, PipelineOptions, RunOutcome, open_index_db_path, read_index_stats, run_index,
};

/// 仓库卡片状态徽标（互斥四态，设计见设计稿 code_index_settings.pen）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CodeIndexEntryStatus {
    /// 该仓库的索引任务正在运行。
    Running,
    /// 已启用且存在索引数据。
    Indexed,
    /// 已停用但索引数据保留（开关关闭不删数据）。
    DisabledWithData,
    /// 从未建立索引。
    NotIndexed,
}

impl CodeIndexEntryStatus {
    fn label(self) -> &'static str {
        match self {
            CodeIndexEntryStatus::Running => "索引中",
            CodeIndexEntryStatus::Indexed => "已索引",
            CodeIndexEntryStatus::DisabledWithData => "已停用",
            CodeIndexEntryStatus::NotIndexed => "未索引",
        }
    }
}

/// 卡片状态判定（纯函数）：运行中优先，其次按「有数据 + 开关」分派。
fn code_index_entry_status(running: bool, enabled: bool, has_stats: bool) -> CodeIndexEntryStatus {
    if running {
        CodeIndexEntryStatus::Running
    } else if has_stats {
        if enabled {
            CodeIndexEntryStatus::Indexed
        } else {
            CodeIndexEntryStatus::DisabledWithData
        }
    } else {
        CodeIndexEntryStatus::NotIndexed
    }
}

/// 仓库列表过滤（纯函数）：按名称或路径子串匹配，大小写不敏感，空串放行全部。
fn code_index_entry_matches_filter(entry: &CodeIndexListEntry, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(&needle) || entry.path.to_lowercase().contains(&needle)
}

/// 显示名：路径末段（保留原大小写）。
fn code_index_display_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|seg| !seg.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// 列表条目去重入列（已见 repo_key 跳过）。
fn push_code_index_entry(
    path_str: String,
    entries: &mut Vec<CodeIndexListEntry>,
    seen: &mut HashSet<String>,
) {
    let repo_key = normalize_repo_path(Path::new(&path_str));
    if seen.insert(repo_key.clone()) {
        entries.push(CodeIndexListEntry {
            name: code_index_display_name(&path_str),
            path: path_str,
            repo_key,
        });
    }
}

/// 同帧多卡渲染的交互元素必须唯一 ElementId：`app_button` 等内部以 label
/// 作为元素 id，卡片列里多个同 label 按钮会共享元素状态（第一张卡可点、
/// 其余点击丢失）。用仓库键 + 动作包一层有 id 的容器隔离状态路径。
fn code_index_interactive_host(repo_key: &str, action: &str, inner: impl IntoElement) -> gpui::AnyElement {
    div()
        .id(format!("code-index-{action}-{}", repo_key))
        .child(inner)
        .into_any_element()
}

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
        let repo_display = self
            .active_tab()
            .and_then(|tab| tab.repo_path.as_ref())
            .map(|p| p.to_string_lossy().to_string());
        let filter = self.code_index_filter.value.clone();
        let entries: Vec<&CodeIndexListEntry> = self
            .code_index_list_entries
            .iter()
            .filter(|entry| code_index_entry_matches_filter(entry, &filter))
            .collect();
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
            .gap_3()
            // 页头。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("代码索引"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(17.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("为每个仓库建立本机代码知识图谱（tree-sitter 解析 → 符号与调用关系 → SQLite），供全局符号搜索（Ctrl+P）与 MCP 工具使用。仓库列表在页面底部，可分别启用、重建或删除索引。"),
                    ),
            )
            // MCP 服务器接入卡：把索引暴露给外部 AI 工具。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child("MCP 服务器"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("多仓库模式：一条配置服务所有已索引仓库"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("把代码索引以 MCP 工具暴露给外部 AI 工具（Claude Code / Cursor / ZCode 等）。命令：khaslana mcp（多仓库模式，工具按 repo 参数选择目标；也可写 khaslana mcp <仓库路径> 固定单仓库）；启动后首次使用仓库时自动建立或增量刷新索引。"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
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
                    ),
            )
            // 仓库列表区（页面底部）：标题 + 过滤框 + 卡片列。
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
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(format!("仓库（{}）", entries.len())),
                            )
                            .child(
                                div().w(px(220.0)).child(
                                    self.input(FieldId::CodeIndexFilter, true, window, cx),
                                ),
                            ),
                    )
                    .children(entries.iter().map(|entry| {
                        self.render_code_index_repo_card(entry, cx)
                    }))
                    .when(entries.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(match repo_display {
                                    Some(_) => "没有匹配的仓库。".to_string(),
                                    None => "当前没有打开的仓库。先在仓库切换下拉中打开一个仓库，再回到此页启用索引。".to_string(),
                                }),
                        )
                    }),
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

    /// 单张仓库卡片：头行（头像 + 名称 + 状态徽标 + 滑动开关）、路径、
    /// 状态行（进度条 / 统计 / 说明）与操作按钮行。
    fn render_code_index_repo_card(
        &self,
        entry: &CodeIndexListEntry,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let running = self
            .code_index_task
            .as_ref()
            .is_some_and(|task| task.repo_path == entry.repo_key);
        let enabled = self.code_index_enabled_cache.contains(&entry.repo_key);
        let stats = self.code_index_stats.get(&entry.repo_key);
        let status = code_index_entry_status(running, enabled, stats.is_some());

        // 开关：点击切换该仓库偏好；开启且无在途任务时立即全量建索引。
        let toggle_repo = entry.repo_key.clone();
        let toggle_enabled = enabled;

        // 全局单任务守卫：任一任务运行时，其它仓库（及本仓库非取消操作）禁用。
        let actions_free = self.code_index_task.is_none();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .w_full()
            .p_3()
            .rounded(px(ui_theme::RADIUS_XS))
            .border_1()
            .border_color(rgb(if running {
                ui_theme::PRIMARY
            } else {
                ui_theme::BORDER_MUTED
            }));
        // 头行。
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .child(crate::repo_avatar(&entry.name))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(entry.name.clone()),
                )
                .child(code_index_status_pill(status))
                .child(div().flex_1())
                .child(
                    div()
                        .id(format!("code-index-toggle-{}", entry.repo_key))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.set_code_index_enabled_for(&toggle_repo, !toggle_enabled, cx);
                        }))
                        .child(code_index_switch(enabled)),
                ),
        );
        // 路径行。
        card = card.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(entry.path.clone()),
        );
        // 状态行。
        card = card.child(match (status, stats) {
            (CodeIndexEntryStatus::Running, _) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(code_index_progress_bar(
                    self.code_index_progress_done,
                    self.code_index_progress_total,
                ))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(if self.code_index_progress_message.is_empty() {
                            "正在索引…".to_string()
                        } else {
                            self.code_index_progress_message.clone()
                        }),
                )
                .into_any_element(),
            (_, Some(s)) => {
                let text = match status {
                    CodeIndexEntryStatus::DisabledWithData => {
                        format!("索引已停用，数据保留 · {}", code_index_stats_line(s))
                    }
                    _ => code_index_stats_line(s),
                };
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(text)
                    .into_any_element()
            }
            (_, None) => {
                let text = if enabled {
                    "尚未建立索引 · 即将自动建立全量索引".to_string()
                } else {
                    "未建立索引 · 开启开关后自动建立全量索引".to_string()
                };
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(text)
                    .into_any_element()
            }
        });
        // 按钮行（按状态给出可用动作）。Fn 闭包可被多次调用，捕获的仓库键
        // 在闭包体内克隆后传入；每个按钮包一层唯一 id 容器隔离元素状态
        //（app_button 以 label 作元素 id，多卡同帧渲染会冲突）。
        let repo_increment = entry.repo_key.clone();
        let repo_rebuild = entry.repo_key.clone();
        let repo_delete = entry.repo_key.clone();
        let display_delete = entry.name.clone();
        let repo_rebuild_disabled = entry.repo_key.clone();
        let repo_delete_2 = entry.repo_key.clone();
        let display_delete_2 = entry.name.clone();
        let buttons: Vec<gpui::AnyElement> = match status {
            CodeIndexEntryStatus::Running => vec![
                code_index_interactive_host(
                    &entry.repo_key,
                    "cancel",
                    self.button("取消", true, |this, _, cx| {
                        this.cancel_code_index(cx);
                    }, cx),
                ),
                code_index_interactive_host(
                    &entry.repo_key,
                    "increment-disabled",
                    self.button("增量更新", false, |_this, _, _| {}, cx),
                ),
                code_index_interactive_host(
                    &entry.repo_key,
                    "rebuild-disabled",
                    self.button("重建索引", false, |_this, _, _| {}, cx),
                ),
            ],
            CodeIndexEntryStatus::Indexed => vec![
                code_index_interactive_host(
                    &entry.repo_key,
                    "increment",
                    self.button("增量更新", actions_free, move |this, _, cx| {
                        this.start_code_index_task_for_repo(&repo_increment, false, cx);
                    }, cx),
                ),
                code_index_interactive_host(
                    &entry.repo_key,
                    "rebuild",
                    self.button("重建索引", actions_free, move |this, _, cx| {
                        this.start_code_index_task_for_repo(&repo_rebuild, true, cx);
                    }, cx),
                ),
                code_index_interactive_host(
                    &entry.repo_key,
                    "delete",
                    self.danger_button("删除索引数据", true, move |this, _, _| {
                        this.request_delete_code_index(display_delete.clone(), repo_delete.clone());
                    }, cx),
                ),
            ],
            CodeIndexEntryStatus::DisabledWithData => vec![
                code_index_interactive_host(
                    &entry.repo_key,
                    "rebuild",
                    self.button("重建索引", actions_free, move |this, _, cx| {
                        this.start_code_index_task_for_repo(&repo_rebuild_disabled, true, cx);
                    }, cx),
                ),
                code_index_interactive_host(
                    &entry.repo_key,
                    "delete",
                    self.danger_button("删除索引数据", true, move |this, _, _| {
                        this.request_delete_code_index(display_delete_2.clone(), repo_delete_2.clone());
                    }, cx),
                ),
            ],
            CodeIndexEntryStatus::NotIndexed => vec![],
        };
        if !buttons.is_empty() {
            let mut row = div().flex().items_center().gap_1p5();
            for button in buttons {
                row = row.child(button);
            }
            if status == CodeIndexEntryStatus::Running {
                row = row.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child("同一时间只能有一个索引任务"),
                );
            }
            card = card.child(row);
        }
        card.into_any_element()
    }

    // ------------------------------------------------------------------
    // 动作
    // ------------------------------------------------------------------

    /// 开关切换（任意仓库）：写偏好表；开启即发起全量索引，关闭保留已建数据。
    pub(crate) fn set_code_index_enabled_for(
        &mut self,
        repo_key: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        match self.storage.set_code_index_enabled(repo_key, enabled) {
            Ok(()) => {}
            Err(err) => {
                self.notify_error(format!("保存索引设置失败：{err}"), cx);
                return;
            }
        }
        if enabled {
            self.code_index_enabled_cache.insert(repo_key.to_string());
            if self.code_index_task.is_none() {
                self.spawn_code_index_task_for(repo_key, true, cx);
            } else {
                self.status = "已有索引任务在进行中，请稍后手动增量更新".into();
            }
        } else {
            self.code_index_enabled_cache.remove(repo_key);
            self.status = "代码索引已关闭（已有索引数据保留）".into();
        }
        cx.notify();
    }

    /// 设置页列表按钮入口（显式仓库键；全局单任务守卫）。
    pub(crate) fn start_code_index_task_for_repo(
        &mut self,
        repo_key: &str,
        force_full: bool,
        cx: &mut Context<Self>,
    ) {
        if self.code_index_task.is_some() {
            self.notify_error("已有索引任务在进行中", cx);
            return;
        }
        self.spawn_code_index_task_for(repo_key, force_full, cx);
    }

    /// 「删除索引数据」按钮：打开按仓库键寻址的确认弹窗。
    fn request_delete_code_index(&mut self, display_name: String, repo_key: String) {
        self.active_dialog = Some(DialogState::ConfirmDeleteCodeIndex {
            repo_key,
            display_name,
        });
    }

    /// 发起索引任务（按显式仓库键构造；自动触发可能来自非活动 tab 的事件）。
    fn spawn_code_index_task_for(
        &mut self,
        repo_key: &str,
        force_full: bool,
        cx: &mut Context<Self>,
    ) {
        if self.code_index_task.is_some() {
            return;
        }
        let Some(db_path) = Self::index_db_path(repo_key) else {
            self.status = "代码索引不可用：数据目录无法创建".into();
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.code_index_task = Some(CodeIndexTaskState {
            repo_path: repo_key.to_string(),
            cancel: Arc::clone(&cancel),
        });
        self.code_index_progress_message = "准备索引…".to_string();
        self.code_index_progress_done = 0;
        self.code_index_progress_total = 0;

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
            self.code_index_progress_done = 0;
            self.code_index_progress_total = 0;
            self.status = "代码索引已取消".into();
        }
    }

    pub(crate) fn confirm_delete_code_index_now(
        &mut self,
        repo_key: &str,
        cx: &mut Context<Self>,
    ) {
        self.active_dialog = None;
        let Some(db_path) = Self::index_db_path(repo_key) else {
            return;
        };
        let dir = db_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| db_path.clone());
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                self.code_index_stats.remove(repo_key);
                self.notify_success("索引数据已删除", cx);
            }
            Err(err) => {
                // 目录不存在视为已删除。
                if dir.exists() {
                    self.notify_error(format!("删除索引数据失败：{err}"), cx);
                    return;
                }
                self.code_index_stats.remove(repo_key);
                self.notify_success("索引数据已删除", cx);
            }
        }
        cx.notify();
    }

    /// 设置页打开/刷新：同步读偏好缓存，构建多仓库列表（已打开 tabs +
    /// 最近仓库 + 偏好记录三路合并去重），并对全部仓库后台读库刷新统计。
    pub(crate) fn refresh_code_index_stats(&mut self) {
        // 偏好缓存同步读主库（单行小表）。
        let mut pref_keys: Vec<String> = Vec::new();
        if let Ok(prefs) = self.storage.load_code_index_preferences() {
            self.code_index_enabled_cache = prefs
                .repositories
                .iter()
                .filter(|(_, enabled)| **enabled)
                .map(|(repo, _)| repo.clone())
                .collect();
            pref_keys = prefs.repositories.into_keys().collect();
        } else {
            self.last_error = Some("读取代码索引设置失败".into());
        }

        let mut entries: Vec<CodeIndexListEntry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // 1) 当前打开的仓库（按 tab 顺序）。
        for tab in &self.tabs {
            if let Some(path) = &tab.repo_path {
                push_code_index_entry(
                    path.to_string_lossy().to_string(),
                    &mut entries,
                    &mut seen,
                );
            }
        }
        // 2) 最近打开的仓库（按最近时间倒序，主库已排序）。
        if let Ok(recents) = self.storage.load_recent_repos() {
            for (path, _) in recents {
                push_code_index_entry(
                    path.to_string_lossy().to_string(),
                    &mut entries,
                    &mut seen,
                );
            }
        }
        // 3) 索引偏好中剩余的仓库（老记录不在最近列表里也可见，可清理）。
        for repo_key in pref_keys {
            if !seen.contains(&repo_key) {
                seen.insert(repo_key.clone());
                entries.push(CodeIndexListEntry {
                    name: code_index_display_name(&repo_key),
                    path: repo_key.clone(),
                    repo_key,
                });
            }
        }

        self.code_index_list_entries = entries;
        // 过滤框重置：列表按最新仓库集合重新浏览。
        self.code_index_filter.set_value(String::new());

        let stats_keys: Vec<String> = self
            .code_index_list_entries
            .iter()
            .map(|entry| entry.repo_key.clone())
            .collect();
        for repo_key in stats_keys {
            self.request_code_index_stats(&repo_key);
        }
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

    // ------------------------------------------------------------------
    // 事件处理（main.rs 的 handle_ui_event 分发到这里）
    // ------------------------------------------------------------------

    pub(crate) fn handle_code_index_progress(
        &mut self,
        repo_path: String,
        message: String,
        done: usize,
        total: usize,
    ) {
        if self
            .code_index_task
            .as_ref()
            .is_some_and(|t| t.repo_path == repo_path)
        {
            self.code_index_progress_message = message.clone();
            self.code_index_progress_done = done;
            self.code_index_progress_total = total;
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
            self.code_index_progress_done = 0;
            self.code_index_progress_total = 0;
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
            self.code_index_progress_done = 0;
            self.code_index_progress_total = 0;
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
}

// ----------------------------------------------------------------------
// 渲染纯函数与小部件
// ----------------------------------------------------------------------

/// 滑动开关（设计稿样式）：34×19 圆角胶囊 + 15px 圆形滑块，开启时滑块
/// 右置、胶囊主题色；区别于复选框样式的 `toggle_box`。
fn code_index_switch(enabled: bool) -> impl IntoElement {
    div()
        .w(px(34.0))
        .h(px(19.0))
        .rounded_full()
        .bg(rgb(if enabled {
            ui_theme::PRIMARY
        } else {
            ui_theme::BORDER
        }))
        .px(px(2.0))
        .py(px(2.0))
        .flex()
        .items_center()
        .when(enabled, |this| this.justify_end())
        .when(!enabled, |this| this.justify_start())
        .child(div().size(px(15.0)).rounded_full().bg(rgb(ui_theme::CARD)))
}

/// 状态徽标 pill（圆点 + 文字）。
fn code_index_status_pill(status: CodeIndexEntryStatus) -> gpui::AnyElement {
    let (bg, text) = match status {
        CodeIndexEntryStatus::Running => (ui_theme::PRIMARY_SUBTLE, ui_theme::PRIMARY),
        CodeIndexEntryStatus::Indexed => {
            (ui_theme::COLOR_SUCCESS, ui_theme::COLOR_SUCCESS_FOREGROUND)
        }
        CodeIndexEntryStatus::DisabledWithData | CodeIndexEntryStatus::NotIndexed => {
            (ui_theme::TILE, ui_theme::CONTENT_SECONDARY)
        }
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(px(10.5))
        .child(div().size(px(5.0)).rounded_full().bg(rgb(text)))
        .child(status.label())
        .into_any_element()
}

/// 细进度条：按 done/total 比例填充（relative 百分比宽度，无需像素级容器宽度）。
fn code_index_progress_bar(done: usize, total: usize) -> gpui::AnyElement {
    let fraction = if total == 0 {
        0.0
    } else {
        done.min(total) as f32 / total as f32
    };
    div()
        .h(px(4.0))
        .w_full()
        .rounded(px(ui_theme::RADIUS_XS))
        .overflow_hidden()
        .bg(rgb(ui_theme::SURFACE_SUNKEN))
        .child(
            div()
                .w(gpui::relative(fraction))
                .h_full()
                .bg(rgb(ui_theme::PRIMARY)),
        )
        .into_any_element()
}

/// 统计行文案（已索引 / 已停用共用，前缀由调用方拼接）。
fn code_index_stats_line(stats: &khaslana::code_index::IndexStats) -> String {
    format!(
        "{} 文件 · {} 符号 · {} 关系（调用 {}）· {:.1} MB · {} · 分支 {}",
        stats.files,
        stats.symbols,
        stats.edges,
        stats.calls,
        stats.db_bytes as f64 / (1024.0 * 1024.0),
        format_indexed_at(stats.indexed_at),
        if stats.branch.is_empty() { "-" } else { &stats.branch },
    )
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

impl RepositoryView {
    /// 「删除索引数据」确认弹窗（按仓库键寻址，文案指明目标仓库）。
    pub(crate) fn render_code_index_delete_confirm(
        &self,
        repo_key: &str,
        display_name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let repo = repo_key.to_string();
        dialog_panel("删除索引数据")
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除 {display_name} 的代码索引数据？")),
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
                        move |this, _, cx| {
                            this.confirm_delete_code_index_now(&repo, cx);
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

#[cfg(test)]
#[path = "tests/code_index_view.rs"]
mod tests;
