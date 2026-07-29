use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::ui::theme::rgb;
use git2::Repository;
use gpui::{Context, IntoElement, ListSizingBehavior, Window, div, prelude::*, px, uniform_list};
use khaslana::{FileDiff, StashFileChange};

use crate::{
    CHANGE_ROW_HEIGHT, DialogState, DiffHeaderTarget, EncodingMenuTarget, FieldId, MainMode,
    RepositoryView, ResizeTarget, ScrollbarMode, UiEvent, change_state_color, dialog_actions,
    menu_separator, perf_log, placeholder_row, scrollable_uniform_frame, section_header,
    send_ui_event, tasks::TaskKind, ui::theme as ui_theme,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct StashPreviewState {
    pub(crate) stash_index: Option<usize>,
    pub(crate) stash_oid: Option<String>,
    pub(crate) stash_message: Option<String>,
    pub(crate) files: Vec<StashFileChange>,
    pub(crate) selected_file: Option<String>,
    pub(crate) diff: Option<Arc<FileDiff>>,
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

impl RepositoryView {
    pub(crate) fn open_stash_dialog(&mut self) {
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
            self.stash_preview.diff = Some(diff);
            self.stash_preview.diff_headers_expanded = false;
            self.status = "贮藏差异已加载".to_string();
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
            .bg(rgb(ui_theme::CARD))
            .child(self.render_stash_preview_header())
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

    fn render_stash_preview_header(&self) -> impl IntoElement {
        let title = self
            .stash_preview
            .stash_index
            .map(|index| format!("贮藏详情：stash@{{{index}}}"))
            .unwrap_or_else(|| "贮藏详情".to_string());
        let message = self
            .stash_preview
            .stash_message
            .clone()
            .unwrap_or_else(|| "请在左侧贮藏区右键选择“查看贮藏”".to_string());

        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .truncate()
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
            .p_2()
            .bg(rgb(ui_theme::CARD))
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
            .child(section_header("贮藏文件"))
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
        let state = file.status.label();
        let state_color = change_state_color(&file.status);

        div()
            .id(format!("stash-file-{}", file.path))
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_1()
            .h(px(CHANGE_ROW_HEIGHT))
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .bg(if selected {
                rgb(ui_theme::ACCENT)
            } else {
                rgb(ui_theme::CARD)
            })
            .border_1()
            .border_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::BORDER)
            })
            .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_stash_file(path.clone(), false);
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(24.0))
                    .text_size(px(11.0))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(state_color))
                    .child(state),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
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
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除 stash@{{{index}}}？")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(message),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_ERROR_FOREGROUND))
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

    pub(crate) fn render_stash_context_menu_content(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .child(crate::context_menu_item(
                "查看贮藏",
                !self.busy,
                move |this| this.view_stash(index),
                cx,
            ))
            .child(crate::context_menu_item(
                "应用贮藏",
                !self.busy,
                move |this| this.apply_stash(index),
                cx,
            ))
            .child(crate::context_menu_item(
                "弹出贮藏",
                !self.busy,
                move |this| this.pop_stash(index),
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
