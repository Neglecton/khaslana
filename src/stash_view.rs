use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::ui::theme::rgb;
use git2::Repository;
use gpui::{Context, IntoElement, ListSizingBehavior, Window, div, prelude::*, px, uniform_list};
use khaslana::{FileDiff, StashFileChange};

use crate::{
    CHANGE_ROW_HEIGHT, DialogState, DiffHeaderTarget, EncodingMenuTarget, FieldId, MainMode,
    RepositoryView, ResizeTarget, ScrollbarMode, UiEvent, change_state_badge, dialog_actions,
    menu_separator, perf_log, placeholder_row, scrollable_uniform_frame, send_ui_event,
    tasks::TaskKind,
    ui::{
        components::{command_group, list_row_surface, page_header},
        theme as ui_theme,
    },
};

#[derive(Clone, Debug, Default)]
pub(crate) struct StashPreviewState {
    pub(crate) stash_index: Option<usize>,
    pub(crate) stash_oid: Option<String>,
    pub(crate) stash_message: Option<String>,
    pub(crate) files: Vec<StashFileChange>,
    pub(crate) selected_file: Option<String>,
    pub(crate) diff: Option<Arc<FileDiff>>,
    /// 差异的语法高亮（仅全文模式计算；索引与 diff.lines 对齐）。
    pub(crate) diff_syntax: Option<Arc<khaslana::syntax::SyntaxSpans>>,
    pub(crate) loading_files: bool,
    pub(crate) loading_diff: bool,
    pub(crate) diff_headers_expanded: bool,
}

impl StashPreviewState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_showing(&self) -> bool {
        self.stash_oid.is_some()
    }
}

/// 贮藏文件列表的平面行视觉规则：默认使用基础 surface，选中时只提升
/// 选择底色，不再为每一行堆叠边框或卡片。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StashFileRowVisualRule {
    background: u32,
    text: u32,
    selected: bool,
}

const fn stash_file_row_visual_rule(selected: bool) -> StashFileRowVisualRule {
    StashFileRowVisualRule {
        background: if selected {
            ui_theme::STATE_SELECTION
        } else {
            ui_theme::SURFACE_BASE
        },
        text: ui_theme::CONTENT_PRIMARY,
        selected,
    }
}

#[cfg(test)]
#[path = "tests/stash_view.rs"]
mod tests;

impl RepositoryView {
    pub(crate) fn open_stash_dialog(&mut self) {
        if !self.ensure_no_merge_in_progress("创建贮藏") {
            return;
        }
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.stash_message.clear();
        self.stash_include_untracked = false;
        self.stash_keep_index = false;
        self.active_dialog = Some(DialogState::StashForm);
        self.last_error = None;
    }

    pub(crate) fn save_stash(&mut self) {
        if !self.ensure_no_merge_in_progress("创建贮藏") {
            return;
        }
        let message = self.stash_message.value.clone();
        let include_untracked = self.stash_include_untracked;
        let keep_index = self.stash_keep_index;
        self.close_dialog();
        self.with_repo_blocking("已贮藏当前修改", move |service, repo| {
            service.save_stash(repo, &message, include_untracked, keep_index)
        });
    }

    pub(crate) fn view_stash(&mut self, index: usize) {
        let Some(stash) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.stashes.iter().find(|stash| stash.index == index))
            .cloned()
        else {
            self.last_error = Some(format!("贮藏不存在：stash@{{{index}}}"));
            self.stash_context_menu = None;
            return;
        };

        self.close_popups();
        self.set_main_mode(MainMode::Stash);
        self.stash_preview = StashPreviewState {
            stash_index: Some(stash.index),
            stash_oid: Some(stash.oid.clone()),
            stash_message: Some(stash.message),
            loading_files: true,
            ..StashPreviewState::default()
        };
        self.status = "正在加载贮藏文件".to_string();
        self.load_stash_files(stash.oid);
    }

    pub(crate) fn open_drop_stash_confirm_dialog(&mut self, index: usize) {
        let Some(message) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.stashes.iter().find(|stash| stash.index == index))
            .map(|stash| stash.message.clone())
        else {
            self.last_error = Some(format!("贮藏不存在：stash@{{{index}}}"));
            self.stash_context_menu = None;
            return;
        };
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmDropStash { index, message });
        self.last_error = None;
    }

    pub(crate) fn open_pop_stash_confirm_dialog(&mut self, index: usize) {
        let Some(message) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.stashes.iter().find(|stash| stash.index == index))
            .map(|stash| stash.message.clone())
        else {
            self.last_error = Some(format!("贮藏不存在：stash@{{{index}}}"));
            self.stash_context_menu = None;
            return;
        };
        self.close_popups();
        self.active_dialog = Some(DialogState::ConfirmPopStash { index, message });
        self.last_error = None;
    }

    pub(crate) fn drop_stash(&mut self, index: usize) {
        self.close_dialog();
        self.stash_context_menu = None;
        self.with_repo("已删除贮藏", move |service, repo| {
            service.drop_stash(repo, index)
        });
    }

    pub(crate) fn prune_stash_preview(&mut self) {
        let Some(oid) = self.stash_preview.stash_oid.clone() else {
            return;
        };
        let still_exists = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.stashes.iter().any(|stash| stash.oid == oid));
        if !still_exists {
            self.stash_preview.clear();
            self.reset_uniform_scroll("stash-file-list");
            self.reset_uniform_scroll("stash-diff-scroll");
            if self.main_mode == MainMode::Stash {
                self.status = "当前贮藏已不存在".to_string();
            }
        }
    }

    pub(crate) fn toggle_stash_diff_headers(&mut self) {
        self.stash_preview.diff_headers_expanded = !self.stash_preview.diff_headers_expanded;
        self.reset_uniform_scroll("stash-diff-scroll");
    }

    pub(crate) fn load_stash_files(&mut self, stash_oid: String) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let files = service.stash_files(&repo, &stash_oid)?;
                perf_log(
                    "stash.files",
                    started,
                    format!("tab={} files={}", tab_id.0, files.len()),
                );
                Ok(UiEvent::StashFilesLoaded {
                    tab_id,
                    stash_oid,
                    files,
                    load_id,
                })
            })();

            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::HistoryLoadFailed {
                        tab_id,
                        error: err.to_string(),
                        load_id,
                    },
                ),
            }
        });
    }

    pub(crate) fn select_stash_file(&mut self, path: String, force_reload: bool) {
        let Some(stash_oid) = self.stash_preview.stash_oid.clone() else {
            return;
        };
        if !force_reload
            && self.stash_preview.selected_file.as_deref() == Some(path.as_str())
            && self.stash_preview.diff.is_some()
        {
            return;
        }

        self.stash_preview.selected_file = Some(path.clone());
        self.stash_preview.diff = None;
        self.stash_preview.diff_headers_expanded = false;
        self.stash_preview.loading_diff = true;
        self.reset_uniform_scroll("stash-diff-scroll");
        self.status = "正在加载贮藏差异".to_string();

        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let encoding = self.diff_encoding_choice_for_path(&repo_path);
        let full_context = self.full_file_view;
        let cache_key = self.diff_cache_key(
            crate::DiffCacheKind::Stash {
                stash_oid: stash_oid.clone(),
                path: path.clone(),
            },
            &repo_path,
        );
        if !force_reload && let Some(diff) = self.cached_diff(&cache_key) {
            self.stash_preview.loading_diff = false;
            self.stash_preview.diff_syntax = None;
            self.stash_preview.diff = Some(diff);
            self.stash_preview.diff_headers_expanded = false;
            self.status = "贮藏差异已加载".to_string();
            // 缓存命中不走事件落位，语法高亮在此手动调度
            self.schedule_syntax_highlight(crate::SyntaxSlot::StashDiff);
            return;
        }
        let service = self.service_for_tab(tab_id);
        let tx = self.tx.clone();
        let load_id = self.repository_load_id;

        self.tasks.spawn(TaskKind::Short, move || {
            let started = Instant::now();
            let result = (|| -> khaslana::Result<UiEvent> {
                let repo = Repository::open(repo_path)?;
                let diff = service.stash_file_diff(
                    &repo,
                    &stash_oid,
                    Path::new(&path),
                    full_context,
                    encoding,
                )?;
                perf_log(
                    "stash.diff",
                    started,
                    format!("tab={} lines={}", tab_id.0, diff.lines.len()),
                );
                Ok(UiEvent::StashDiffLoaded {
                    tab_id,
                    stash_oid,
                    path,
                    diff,
                    load_id,
                })
            })();

            match result {
                Ok(event) => send_ui_event(&tx, event),
                Err(err) => send_ui_event(
                    &tx,
                    UiEvent::HistoryLoadFailed {
                        tab_id,
                        error: err.to_string(),
                        load_id,
                    },
                ),
            }
        });
    }

    pub(crate) fn render_stash_preview_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(ui_theme::SURFACE_CANVAS))
            .child(self.render_stash_preview_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(self.render_stash_files(cx))
                    .child(self.render_column_splitter(ResizeTarget::HistoryFiles, cx))
                    .child(self.render_stash_diff(cx)),
            )
    }

    fn render_stash_preview_header(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let stash_label = self
            .stash_preview
            .stash_index
            .map(|index| format!("stash@{{{index}}}"))
            .unwrap_or_else(|| "未选择贮藏".to_string());
        let message = self
            .stash_preview
            .stash_message
            .clone()
            .unwrap_or_else(|| "请在左侧贮藏区右键选择“查看贮藏”".to_string());

        page_header("贮藏详情", Some("文件与差异预览")).child(
            command_group()
                .child(
                    div()
                        .max_w(px(280.0))
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(stash_label),
                )
                .child(
                    div()
                        .max_w(px(360.0))
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(ui_theme::TYPE_META))
                        .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                        .child(message),
                ),
        )
    }

    fn render_stash_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.stash_preview.files.len().max(1);
        let content_present = !self.stash_preview.files.is_empty();
        let handle = self.uniform_scroll_handle("stash-file-list");
        let list_handle = handle.clone();
        let content = div()
            .id("stash-file-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p(px(ui_theme::SPACE_2))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                uniform_list(
                    "stash-file-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                if this.stash_preview.files.is_empty() {
                                    return placeholder_row(if this.stash_preview.loading_files {
                                        "贮藏文件加载中..."
                                    } else if this.stash_preview.is_showing() {
                                        "该贮藏没有文件变更"
                                    } else {
                                        "请选择一个贮藏"
                                    })
                                    .into_any_element();
                                }
                                this.stash_preview
                                    .files
                                    .get(index)
                                    .cloned()
                                    .map(|file| this.stash_file_row(file, cx).into_any_element())
                                    .unwrap_or_else(|| placeholder_row("").into_any_element())
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.history_files_width))
            .min_w(px(self.history_files_width))
            .min_h(px(0.0))
            .h_full()
            .bg(rgb(ui_theme::SURFACE_BASE))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(px(ui_theme::ROW_HEIGHT_REGULAR))
                    .px(px(ui_theme::SPACE_3))
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("贮藏文件")
                    .child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                            .child(self.stash_preview.files.len().to_string()),
                    ),
            )
            .child(scrollable_uniform_frame(
                "stash-file-list",
                ScrollbarMode::Vertical,
                content,
                handle,
                content_present,
                cx,
            ))
    }

    fn stash_file_row(&self, file: StashFileChange, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.stash_preview.selected_file.as_deref() == Some(file.path.as_str());
        let path = file.path.clone();
        let path_label = file
            .old_path
            .as_ref()
            .filter(|old_path| old_path.as_str() != file.path.as_str())
            .map(|old_path| format!("{old_path} -> {}", file.path))
            .unwrap_or_else(|| file.path.clone());

        let visual = stash_file_row_visual_rule(selected);
        list_row_surface(format!("stash-file-{}", file.path), visual.selected)
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap(px(ui_theme::SPACE_1))
            .h(px(CHANGE_ROW_HEIGHT))
            .px(px(ui_theme::SPACE_2))
            .overflow_hidden()
            .bg(rgb(visual.background))
            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_stash_file(path.clone(), false);
                cx.notify();
            }))
            .child(change_state_badge(Some(&file.status)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(ui_theme::TYPE_BODY))
                    .text_color(rgb(visual.text))
                    // uniform_list 行禁 truncate（MinContent 测量坍缩后省略号
                    // 固化到绘制），硬裁剪替代。
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(path_label),
            )
    }

    fn render_stash_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 全文视图模式下标题前缀"全文："，提示当前展示整份文件
        let prefix = if self.full_file_view { "全文：" } else { "" };
        let title = self
            .stash_preview
            .selected_file
            .as_ref()
            .map(|path| format!("{prefix}贮藏差异：{path}"))
            .unwrap_or_else(|| "贮藏差异".to_string());
        let empty_message = if self.stash_preview.loading_diff {
            "贮藏差异加载中..."
        } else {
            "请选择一个贮藏文件查看差异"
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .min_w(px(0.0))
            .h_full()
            .child(self.diff_section_header(title, EncodingMenuTarget::Stash, cx))
            .child(self.render_virtual_diff(
                "stash-diff-scroll",
                self.stash_preview.diff.clone(),
                self.stash_preview.diff_headers_expanded,
                DiffHeaderTarget::Stash,
                empty_message.to_string(),
                cx,
            ))
            .child(self.render_encoding_dropdown(EncodingMenuTarget::Stash, cx))
    }

    pub(crate) fn render_stash_form_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("贮藏当前修改", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("创建贮藏后，当前工作区会回到干净状态，后续可从左侧贮藏区应用或弹出。"),
            )
            .child(self.input(FieldId::StashMessage, false, window, cx))
            .child(self.toggle_row(
                "stash-include-untracked",
                "包含未跟踪文件",
                self.stash_include_untracked,
                |this, _, _| this.stash_include_untracked = !this.stash_include_untracked,
                cx,
            ))
            .child(self.toggle_row(
                "stash-keep-index",
                "保留已暂存内容",
                self.stash_keep_index,
                |this, _, _| this.stash_keep_index = !this.stash_keep_index,
                cx,
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "创建贮藏",
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.save_stash(),
                        cx,
                    )),
            )
    }

    pub(crate) fn render_confirm_drop_stash_dialog(
        &self,
        index: usize,
        message: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除贮藏", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("确认删除 stash@{{{index}}}？")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(message),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                    .child("删除后无法从贮藏列表恢复。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        !self.busy,
                        move |this, _, _| this.drop_stash(index),
                        cx,
                    )),
            )
    }

    pub(crate) fn render_confirm_pop_stash_dialog(
        &self,
        index: usize,
        message: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("弹出贮藏", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("确认弹出 stash@{{{index}}}？")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(message),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("弹出会把贮藏的改动应用到工作区，成功后从贮藏列表移除该条目。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.button(
                        "确认弹出",
                        !self.busy,
                        move |this, _, _| this.pop_stash(index),
                        cx,
                    )),
            )
    }

    pub(crate) fn render_stash_context_menu_content(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_apply = !self.busy && !self.merge_in_progress();
        div()
            .child(crate::context_menu_item(
                "查看贮藏",
                !self.busy,
                move |this| this.view_stash(index),
                cx,
            ))
            .child(crate::context_menu_item(
                "应用贮藏",
                can_apply,
                move |this| this.apply_stash(index),
                cx,
            ))
            .child(crate::context_menu_item(
                "弹出贮藏",
                can_apply,
                move |this| this.open_pop_stash_confirm_dialog(index),
                cx,
            ))
            .child(menu_separator())
            .child(crate::context_menu_item(
                "删除贮藏...",
                !self.busy,
                move |this| this.open_drop_stash_confirm_dialog(index),
                cx,
            ))
    }
}
