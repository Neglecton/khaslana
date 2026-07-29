use std::{ops::Range, path::Path, sync::Arc};

use crate::ui::theme::rgb;
use gpui::{
    Context, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior, MouseButton,
    MouseDownEvent, Window, div, prelude::*, px, uniform_list,
};
use khaslana::{
    ConflictBlock, ConflictBlockResolution, ConflictBlockStatus, ConflictFileKind,
    ConflictFileView, ConflictResolutionSide, RepositorySnapshot,
};

use crate::{
    MainMode, RepositoryView,
    ui::{components::app_panel, theme as ui_theme},
    ui_helpers::{ScrollbarMode, scrollable_uniform_frame},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictDocumentPane {
    Ours,
    Result,
    Theirs,
}

pub(crate) fn conflict_status_message(label: &str, count: usize) -> String {
    let operation = label.strip_suffix("完成").unwrap_or("操作");
    format!("{operation}产生冲突，请在左侧“冲突”区域解决（{count} 个文件）")
}

fn conflict_paths(snapshot: Option<&RepositorySnapshot>) -> Vec<String> {
    snapshot
        .map(|snapshot| snapshot.conflicts.clone())
        .unwrap_or_default()
}

fn conflict_document_line_owners(
    content: &str,
    pane: ConflictDocumentPane,
    view: &ConflictFileView,
    line_count: usize,
) -> Vec<Option<usize>> {
    let mut owners = vec![None; line_count.max(1)];
    for (index, block) in view.blocks.iter().enumerate() {
        let (start, end) = conflict_document_byte_range(block, pane);
        let range = conflict_byte_range_to_lines(content, start, end);
        for line_index in range {
            if let Some(owner) = owners.get_mut(line_index) {
                *owner = Some(index);
            }
        }
    }
    owners
}

fn conflict_document_byte_range(
    block: &ConflictBlock,
    pane: ConflictDocumentPane,
) -> (usize, usize) {
    match pane {
        ConflictDocumentPane::Ours => (block.ours_start, block.ours_end),
        ConflictDocumentPane::Result => (block.start, block.end),
        ConflictDocumentPane::Theirs => (block.theirs_start, block.theirs_end),
    }
}

fn conflict_byte_range_to_lines(content: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let start_line = content[..start.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let mut end_line = content[..end.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    if end_line == start_line {
        end_line += 1;
    }
    start_line..end_line
}

#[derive(Clone, Debug)]
struct ConflictDocumentLineModel {
    content: Arc<str>,
    ranges: Arc<[Range<usize>]>,
    owners: Arc<[Option<usize>]>,
}

#[derive(Clone, Debug)]
struct ConflictPlainLineModel {
    content: Arc<str>,
    ranges: Arc<[Range<usize>]>,
}

impl ConflictPlainLineModel {
    fn new(content: &str) -> Self {
        Self {
            content: Arc::from(content),
            ranges: Arc::from(conflict_document_line_ranges(content)),
        }
    }

    fn line_count(&self) -> usize {
        self.ranges.len().max(1)
    }

    fn line_text(&self, index: usize) -> &str {
        let Some(range) = self.ranges.get(index) else {
            return "";
        };
        &self.content[range.clone()]
    }
}

impl ConflictDocumentLineModel {
    fn new(content: &str, pane: ConflictDocumentPane, view: &ConflictFileView) -> Self {
        let ranges = conflict_document_line_ranges(content);
        let owners = conflict_document_line_owners(content, pane, view, ranges.len());
        Self {
            content: Arc::from(content),
            ranges: Arc::from(ranges),
            owners: Arc::from(owners),
        }
    }

    fn line_count(&self) -> usize {
        self.ranges.len().max(1)
    }

    fn line_text(&self, index: usize) -> &str {
        let Some(range) = self.ranges.get(index) else {
            return "";
        };
        &self.content[range.clone()]
    }

    fn owner_at(&self, index: usize) -> Option<usize> {
        self.owners.get(index).copied().flatten()
    }
}

fn conflict_document_line_ranges(content: &str) -> Vec<Range<usize>> {
    if content.is_empty() {
        return vec![0..0];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            ranges.push(start..index);
            start = index + ch.len_utf8();
        }
    }
    ranges.push(start..content.len());
    ranges
}

fn conflict_line_colors(
    pane: ConflictDocumentPane,
    block: &ConflictBlock,
    active: bool,
) -> (u32, u32) {
    match block.status {
        ConflictBlockStatus::Ignored => {
            if active {
                (ui_theme::ACCENT, ui_theme::MUTED_FOREGROUND)
            } else {
                (ui_theme::CARD, ui_theme::MUTED_FOREGROUND)
            }
        }
        ConflictBlockStatus::Resolved(_) => match pane {
            ConflictDocumentPane::Ours | ConflictDocumentPane::Theirs => {
                if active {
                    (ui_theme::COLOR_ERROR, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
            ConflictDocumentPane::Result => {
                if active {
                    (ui_theme::ACCENT, ui_theme::PRIMARY)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
        },
        ConflictBlockStatus::Unresolved => match pane {
            ConflictDocumentPane::Ours | ConflictDocumentPane::Theirs => {
                if active {
                    (ui_theme::COLOR_ERROR, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::COLOR_WARNING, ui_theme::FOREGROUND)
                }
            }
            ConflictDocumentPane::Result => {
                if active {
                    (ui_theme::COLOR_WARNING, ui_theme::COLOR_WARNING_FOREGROUND)
                } else {
                    (ui_theme::CARD, ui_theme::FOREGROUND)
                }
            }
        },
    }
}

impl RepositoryView {
    pub(crate) fn render_conflict_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts = conflict_paths(self.snapshot.as_ref());
        let conflict_rows = conflicts
            .iter()
            .cloned()
            .map(|path| self.conflict_row(path, cx).into_any_element())
            .collect::<Vec<_>>();
        let has_conflicts = !conflict_rows.is_empty();
        let conflict_count = conflicts.len();

        div().when(has_conflicts, |this| {
            this.child(self.render_conflict_summary(conflicts.len()))
                .child(self.render_change_section(
                    "冲突",
                    "conflict-list",
                    "",
                    false,
                    conflict_rows,
                    true,
                    conflict_count,
                    false,
                    Vec::new(),
                    cx,
                ))
                .child(div().flex_none().h(px(1.0)).bg(rgb(ui_theme::BORDER)))
        })
    }

    pub(crate) fn render_conflict_workbench(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        app_panel()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(self.render_conflict_file_rail(cx))
            .child(self.render_column_splitter(crate::ResizeTarget::Changes, cx))
            .child(self.render_conflict_detail(window, cx))
    }

    fn render_conflict_file_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts = conflict_paths(self.snapshot.as_ref());
        let selected_path = self.conflict_workbench.selected_path.as_deref();
        let file_rows = conflicts
            .iter()
            .cloned()
            .map(|path| {
                self.conflict_file_row(path.clone(), selected_path == Some(path.as_str()), cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        app_panel()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.changes_width))
            .min_w(px(self.changes_width))
            .h_full()
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::COLOR_WARNING))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                    .child(format!("存在 {} 个冲突文件", conflicts.len())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .children(file_rows),
            )
    }

    fn conflict_file_row(
        &self,
        path: String,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = self.conflict_workbench.files.get(&path);
        let badge = match view.map(|view| view.kind) {
            Some(ConflictFileKind::Text) => "文本",
            Some(ConflictFileKind::Binary) => "二进制",
            Some(ConflictFileKind::Unsupported) => "回退",
            None => "加载中",
        };
        let unresolved = view
            .map(ConflictFileView::unresolved_block_count)
            .unwrap_or_default();
        let dirty = view
            .map(|view| view.draft_status)
            .is_some_and(|status| matches!(status, khaslana::ConflictDraftStatus::Dirty));
        let applied = view
            .map(|view| view.draft_status)
            .is_some_and(|status| matches!(status, khaslana::ConflictDraftStatus::Applied));
        let path_for_select = path.clone();

        crate::ui::components::list_row_surface(format!("conflict-workbench-{path}"), selected)
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                    window.focus(&this.conflict_editor.focus);
                    this.main_mode = MainMode::Conflict;
                    this.select_conflict_file(path_for_select.clone());
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(path),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_1()
                            .py(px(2.0))
                            .rounded_sm()
                            .bg(rgb(ui_theme::ACCENT))
                            .text_size(px(10.0))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(badge),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("未处理 {unresolved}"))
                    .when(dirty, |this| this.child("草稿已修改"))
                    .when(applied, |this| this.child("已应用")),
            )
    }

    fn render_conflict_detail(&self, _window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_path = self.conflict_workbench.selected_path.clone();
        let selected_view = selected_path
            .as_ref()
            .and_then(|path| self.conflict_workbench.files.get(path));
        let title = selected_path
            .clone()
            .unwrap_or_else(|| "请选择一个冲突文件".to_string());

        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(self.render_conflict_header(title, selected_view, cx))
            .child(match selected_view {
                Some(view) if view.kind == ConflictFileKind::Text => self
                    .render_text_conflict_detail(view, cx)
                    .into_any_element(),
                Some(view) => self
                    .render_fallback_conflict_detail(view, cx)
                    .into_any_element(),
                None => div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("请选择一个冲突文件")
                    .into_any_element(),
            })
    }

    fn render_conflict_header(
        &self,
        title: String,
        view: Option<&ConflictFileView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let block_count = view.map(|view| view.blocks.len()).unwrap_or_default();
        let selected_block = self
            .conflict_workbench
            .selected_block
            .min(block_count.saturating_sub(1));
        let progress = if block_count == 0 {
            "无文本冲突块".to_string()
        } else {
            format!(
                "块 {}/{}，未处理 {}",
                selected_block + 1,
                block_count,
                view.map(ConflictFileView::unresolved_block_count)
                    .unwrap_or_default()
            )
        };
        let unresolved = view
            .map(ConflictFileView::unresolved_block_count)
            .unwrap_or_default();
        let ignored = view
            .map(ConflictFileView::ignored_block_count)
            .unwrap_or_default();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(progress),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_wrap()
                    .child(self.button(
                        "上一个块",
                        block_count > 0,
                        |this, _, _| this.step_conflict_block(-1),
                        cx,
                    ))
                    .child(self.button(
                        "下一个块",
                        block_count > 0,
                        |this, _, _| this.step_conflict_block(1),
                        cx,
                    ))
                    .when(unresolved > 0, |this| {
                        this.child(self.conflict_count_badge(
                            format!("未处理 {unresolved}"),
                            ui_theme::COLOR_WARNING,
                            ui_theme::COLOR_WARNING_FOREGROUND,
                        ))
                    })
                    .when(ignored > 0, |this| {
                        this.child(self.conflict_count_badge(
                            format!("已忽略 {ignored}"),
                            ui_theme::ACCENT,
                            ui_theme::PRIMARY,
                        ))
                    })
                    .child(self.button(
                        "忽略该块",
                        block_count > 0 && !self.busy,
                        |this, _, _| this.ignore_selected_conflict_block(),
                        cx,
                    ))
                    .child(self.button(
                        if self.conflict_workbench.show_base {
                            "隐藏 Base"
                        } else {
                            "显示 Base"
                        },
                        block_count > 0,
                        |this, _, _| {
                            this.conflict_workbench.show_base = !this.conflict_workbench.show_base
                        },
                        cx,
                    ))
                    .child(self.button(
                        "应用到工作区",
                        view.is_some_and(|view| view.kind == ConflictFileKind::Text) && !self.busy,
                        |this, _, _| this.apply_selected_conflict_draft(false),
                        cx,
                    ))
                    .child(self.button(
                        if self.external_merge_settings.enabled {
                            "用 IntelliJ IDEA 解决"
                        } else {
                            "配置 IDEA 并解决"
                        },
                        view.is_some() && !self.busy,
                        |this, _, _| this.resolve_selected_conflict_with_intellij_idea(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "应用并标记已解决",
                        view.is_some_and(|view| view.kind == ConflictFileKind::Text) && !self.busy,
                        |this, _, _| this.apply_selected_conflict_draft(true),
                        cx,
                    )),
            )
    }

    fn render_text_conflict_detail(
        &self,
        view: &ConflictFileView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected_block = self
            .conflict_workbench
            .selected_block
            .min(view.blocks.len().saturating_sub(1));
        let block = view.blocks.get(selected_block);
        let warning =
            (view.has_manual_blocks() || view.requires_resolution_confirmation()).then(|| {
                format!(
                    "仍有 {} 个代码块未处理；直接解决时会先弹出确认。",
                    view.unresolved_block_count()
                )
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .when_some(warning, |this, warning| {
                this.child(
                    div()
                        .mx_3()
                        .mt_3()
                        .px_3()
                        .py_2()
                        .rounded_sm()
                        .bg(rgb(ui_theme::COLOR_WARNING))
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .child(warning),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .gap_2()
                    .p_3()
                    .child(self.render_conflict_document_pane(
                        "当前版本",
                        "conflict-ours-scroll",
                        crate::CONFLICT_OURS_SCROLL_HANDLE_ID,
                        &view.ours_text,
                        ConflictDocumentPane::Ours,
                        view,
                        selected_block,
                        vec![
                            self.button(
                                "接受当前",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::Ours,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                            self.button(
                                "接受两边（当前在前）",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::BothOursFirst,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                        ],
                        cx,
                    ))
                    .child(self.render_conflict_document_pane(
                        "结果区",
                        "conflict-result-scroll",
                        crate::CONFLICT_RESULT_SCROLL_HANDLE_ID,
                        &view.draft,
                        ConflictDocumentPane::Result,
                        view,
                        selected_block,
                        Vec::new(),
                        cx,
                    ))
                    .child(self.render_conflict_document_pane(
                        "传入版本",
                        "conflict-theirs-scroll",
                        crate::CONFLICT_THEIRS_SCROLL_HANDLE_ID,
                        &view.theirs_text,
                        ConflictDocumentPane::Theirs,
                        view,
                        selected_block,
                        vec![
                            self.button(
                                "接受传入",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::Theirs,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                            self.button(
                                "接受两边（传入在前）",
                                !self.busy,
                                |this, _, _| {
                                    this.apply_selected_conflict_resolution(
                                        ConflictBlockResolution::BothTheirsFirst,
                                    )
                                },
                                cx,
                            )
                            .into_any_element(),
                        ],
                        cx,
                    )),
            )
            .when(
                self.conflict_workbench.show_base
                    && block.is_some_and(|block| block.base.as_ref().is_some()),
                |this| {
                    this.child(
                        app_panel()
                            .flex_none()
                            .mx_3()
                            .mb_3()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(rgb(ui_theme::BORDER))
                                    .bg(rgb(ui_theme::CARD))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(rgb(ui_theme::FOREGROUND))
                                            .child("Base"),
                                    )
                                    .when_some(block, |this, block| {
                                        this.child(self.conflict_block_status_badge(
                                            block.status,
                                            block.has_manual_edits,
                                        ))
                                    }),
                            )
                            .child(self.render_conflict_plain_text(
                                "conflict-base-scroll",
                                block.and_then(|block| block.base.as_deref()).unwrap_or(""),
                                cx,
                            )),
                    )
                },
            )
    }

    fn render_fallback_conflict_detail(
        &self,
        view: &ConflictFileView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return div().into_any_element();
        };
        let path_for_ours = path.clone();
        let path_for_theirs = path.clone();
        let path_for_mark = path.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .gap_3()
            .p_4()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .bg(rgb(ui_theme::COLOR_WARNING))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                    .child(
                        view.fallback_reason
                            .clone()
                            .unwrap_or_else(|| "该冲突暂不支持可视化文本编辑".into()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.button(
                        "当前版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_ours.clone(),
                                ConflictResolutionSide::Ours,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "传入版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_theirs.clone(),
                                ConflictResolutionSide::Theirs,
                            )
                        },
                        cx,
                    ))
                    .child(self.primary_button(
                        "标记解决",
                        !self.busy,
                        move |this, _, _| this.mark_conflict_resolved(path_for_mark.clone()),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_conflict_document_pane(
        &self,
        title: &'static str,
        scroll_id: &'static str,
        handle_id: &'static str,
        content: &str,
        pane: ConflictDocumentPane,
        view: &ConflictFileView,
        selected_block: usize,
        actions: Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_block = view.blocks.get(selected_block);
        let has_actions = !actions.is_empty();
        app_panel()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(title),
                    )
                    .when_some(active_block, |this, block| {
                        this.child(
                            self.conflict_block_status_badge(block.status, block.has_manual_edits),
                        )
                    }),
            )
            .when(has_actions, move |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .children(actions),
                )
            })
            .child(self.render_conflict_document_text(
                scroll_id,
                handle_id,
                content,
                pane,
                view,
                selected_block,
                cx,
            ))
    }

    fn render_conflict_document_text(
        &self,
        scroll_id: &'static str,
        handle_id: &'static str,
        content: &str,
        pane: ConflictDocumentPane,
        view: &ConflictFileView,
        selected_block: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = self.uniform_scroll_handle(handle_id);
        let list_handle = handle.clone();
        let model = Arc::new(ConflictDocumentLineModel::new(content, pane, view));
        let row_count = model.line_count();
        let model_for_list = model.clone();
        let blocks = Arc::<[ConflictBlock]>::from(view.blocks.clone());
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |_this, range: Range<usize>, _window, _cx| {
                        range
                            .map(|line_index| {
                                let owner = model_for_list.owner_at(line_index);
                                let block = owner.and_then(|index| blocks.get(index));
                                let active = owner == Some(selected_block);
                                let (bg, fg) = block
                                    .map(|block| conflict_line_colors(pane, block, active))
                                    .unwrap_or((ui_theme::CARD, ui_theme::FOREGROUND));
                                let line = model_for_list.line_text(line_index);
                                div()
                                    .min_h(px(18.0))
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(bg))
                                    .text_color(rgb(fg))
                                    .child(if line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        line.to_string()
                                    })
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();
        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Vertical,
            content,
            handle,
            true,
            cx,
        )
    }

    fn render_conflict_plain_text(
        &self,
        scroll_id: &'static str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = handle.clone();
        let model = Arc::new(ConflictPlainLineModel::new(content));
        let row_count = model.line_count();
        let model_for_list = model.clone();
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |_this, range: Range<usize>, _window, _cx| {
                        range
                            .map(|line_index| {
                                let line = model_for_list.line_text(line_index);
                                div()
                                    .min_h(px(18.0))
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(if line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        line.to_string()
                                    })
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();
        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Vertical,
            content,
            handle,
            true,
            cx,
        )
    }

    fn conflict_count_badge(&self, label: String, bg: u32, fg: u32) -> impl IntoElement {
        div()
            .flex_none()
            .px_2()
            .py(px(2.0))
            .rounded_sm()
            .bg(rgb(bg))
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(fg))
            .child(label)
    }

    fn conflict_block_status_badge(
        &self,
        status: ConflictBlockStatus,
        has_manual_edits: bool,
    ) -> impl IntoElement {
        let (label, bg, fg) = match status {
            ConflictBlockStatus::Ignored => {
                ("已忽略", ui_theme::ACCENT, ui_theme::MUTED_FOREGROUND)
            }
            ConflictBlockStatus::Resolved(_) => ("已处理", ui_theme::ACCENT, ui_theme::PRIMARY),
            ConflictBlockStatus::Unresolved if has_manual_edits => (
                "手工修改",
                ui_theme::COLOR_WARNING,
                ui_theme::COLOR_WARNING_FOREGROUND,
            ),
            ConflictBlockStatus::Unresolved => (
                "未处理",
                ui_theme::COLOR_WARNING,
                ui_theme::COLOR_WARNING_FOREGROUND,
            ),
        };
        div()
            .flex_none()
            .px_2()
            .py(px(2.0))
            .rounded_sm()
            .bg(rgb(bg))
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(fg))
            .child(label)
    }

    fn resolve_conflict_with_side(&mut self, path: String, side: ConflictResolutionSide) {
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");
        let label = match side {
            ConflictResolutionSide::Ours => "已使用当前版本解决冲突",
            ConflictResolutionSide::Theirs => "已使用传入版本解决冲突",
        };
        self.with_repo(label, move |service, repo| {
            service.resolve_conflict_with_side(repo, Path::new(&path), side)
        });
    }

    fn mark_conflict_resolved(&mut self, path: String) {
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");
        self.with_repo("冲突已标记为解决", move |service, repo| {
            service.mark_conflict_resolved(repo, Path::new(&path))
        });
    }

    fn resolve_selected_conflict_with_intellij_idea(&mut self) {
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        self.resolve_conflict_with_intellij_idea_path(path);
    }

    fn resolve_conflict_with_intellij_idea_path(&mut self, path: String) -> bool {
        self.request_external_merge_for_path(path)
    }

    pub(crate) fn maybe_auto_open_external_merge_for_selected_conflict(&mut self) {
        if self.busy
            || !self.external_merge_settings.enabled
            || !self.external_merge_settings.auto_open_intellij
        {
            return;
        }
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            return;
        };
        if !self.conflict_workbench.files.contains_key(&path)
            || self
                .conflict_workbench
                .external_merge_auto_opened
                .contains(&path)
        {
            return;
        }
        if self.resolve_conflict_with_intellij_idea_path(path.clone()) {
            self.conflict_workbench
                .mark_external_merge_auto_opened(path);
        }
    }

    fn render_conflict_summary(&self, count: usize) -> impl IntoElement {
        div()
            .flex_none()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::COLOR_WARNING))
            .text_size(px(12.0))
            .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
            .child(format!("存在 {count} 个冲突文件"))
    }

    fn conflict_row(&self, path: String, cx: &mut Context<Self>) -> impl IntoElement {
        let path_for_switch = path.clone();
        let path_for_ours = path.clone();
        let path_for_theirs = path.clone();
        let path_for_mark = path.clone();

        div()
            .id(format!("conflict-{path}"))
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::COLOR_WARNING))
            .bg(rgb(ui_theme::COLOR_WARNING))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.0))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::COLOR_WARNING)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            window.focus(&this.conflict_editor.focus);
                            this.main_mode = MainMode::Conflict;
                            this.select_conflict_file(path_for_switch.clone());
                            this.change_context_menu = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(24.0))
                            .text_size(px(11.0))
                            .font_family("monospace")
                            .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                            .child("!"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.button(
                        "当前版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_ours.clone(),
                                ConflictResolutionSide::Ours,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "传入版本",
                        !self.busy,
                        move |this, _, _| {
                            this.resolve_conflict_with_side(
                                path_for_theirs.clone(),
                                ConflictResolutionSide::Theirs,
                            )
                        },
                        cx,
                    ))
                    .child(self.button(
                        "标记解决",
                        !self.busy,
                        move |this, _, _| this.mark_conflict_resolved(path_for_mark.clone()),
                        cx,
                    )),
            )
    }
}

#[cfg(test)]
#[path = "../tests/conflicts.rs"]
mod tests;
