// 分支比较模式左侧差异文件列表 UI。
//
// 这里仅负责把 Git 服务层返回的差异文件扁平列表渲染出来；右侧内容/差异视图
// 继续复用 browse_view.rs 中的浏览模式视图，避免重复实现 diff 与全文渲染逻辑。

use std::path::Path;

use gpui::{
    Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, div, prelude::*, px,
    rgb, uniform_list,
};
use khaslana::BrowseCompareFile;

use crate::{
    CHANGE_ROW_HEIGHT, RepositoryView,
    ui::theme as ui_theme,
    ui_helpers::{
        ScrollbarMode, change_state_color, placeholder_row, scrollable_uniform_frame,
        section_header,
    },
};

pub(crate) fn browse_compare_file_display(file: &BrowseCompareFile) -> String {
    match file.old_path.as_deref() {
        Some(old_path) if old_path != file.path => format!("{old_path} → {}", file.path),
        _ => file.path.clone(),
    }
}

impl RepositoryView {
    /// 渲染分支比较模式左侧的差异文件列表。
    pub(crate) fn render_browse_compare_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_display = self
            .browse
            .target
            .as_ref()
            .map(|target| target.display_name.clone())
            .unwrap_or_else(|| "加载中...".to_string());
        let short_oid = self
            .browse
            .target
            .as_ref()
            .map(|target| {
                target
                    .commit_oid
                    .get(..7)
                    .unwrap_or(&target.commit_oid)
                    .to_string()
            })
            .unwrap_or_default();
        let file_count = self.browse.compare_files.len();
        let row_count = file_count.max(1);
        let has_target = self.browse.target.is_some();
        let content_present = file_count > 0;
        let handle = self.uniform_scroll_handle("browse-compare-scroll");
        let list_handle = handle.clone();

        let content = div()
            .id("browse-compare-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .bg(rgb(ui_theme::PANEL_BG))
            .child(
                uniform_list(
                    "browse-compare-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                if this.browse.compare_files.is_empty() {
                                    return placeholder_row(if !has_target {
                                        "正在解析引用..."
                                    } else if this.browse.compare_loading {
                                        "正在加载分支差异..."
                                    } else {
                                        "该分支与当前分支没有差异"
                                    })
                                    .into_any_element();
                                }
                                this.browse
                                    .compare_files
                                    .get(index)
                                    .cloned()
                                    .map(|file| {
                                        this.browse_compare_file_row(file, cx).into_any_element()
                                    })
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
            .w(px(self.browse_tree_width))
            .min_w(px(self.browse_tree_width))
            .min_h(px(0.0))
            .h_full()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::HEADER_BG))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::ACCENT_STRONG))
                                    .truncate()
                                    .child(format!("比较：{target_display}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .font_family("Consolas, monospace")
                                    .text_color(rgb(ui_theme::TEXT_FAINT))
                                    .child(short_oid),
                            ),
                    )
                    .child(self.button("关闭", !self.busy, |this, _, _| this.close_browse(), cx)),
            )
            .child(section_header(format!("差异文件 · {file_count}")))
            .child(scrollable_uniform_frame(
                "browse-compare-scroll",
                ScrollbarMode::Vertical,
                content,
                handle,
                content_present,
                cx,
            ))
    }

    fn browse_compare_file_row(
        &self,
        file: BrowseCompareFile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self
            .browse
            .selected_file
            .as_deref()
            .map(|selected| selected == Path::new(&file.path))
            .unwrap_or(false);
        let status_label = file.status.label();
        let status_color = change_state_color(&file.status);
        let display_path = browse_compare_file_display(&file);
        let file_for_click = file.clone();

        div()
            .id(format!("browse-compare-file:{}", file.path))
            .flex()
            .items_center()
            .gap_2()
            .h(px(CHANGE_ROW_HEIGHT))
            .px_2()
            .rounded_sm()
            .border_1()
            .border_color(if selected {
                rgb(ui_theme::ROW_SELECTED_BORDER)
            } else {
                rgb(ui_theme::BORDER_MUTED)
            })
            .bg(if selected {
                rgb(ui_theme::ROW_SELECTED)
            } else {
                rgb(ui_theme::SURFACE)
            })
            .hover(|this| this.bg(rgb(ui_theme::ROW_HOVER)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    this.select_browse_compare_file(file_for_click.clone());
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(22.0))
                    .py_0p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(status_color))
                    .text_size(px(10.0))
                    .font_family("Consolas, monospace")
                    .text_color(rgb(status_color))
                    .text_align(gpui::TextAlign::Center)
                    .child(status_label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::TEXT))
                    .truncate()
                    .child(display_path),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use khaslana::ChangeState;

    #[test]
    fn browse_compare_file_display_plain_path() {
        let file = BrowseCompareFile {
            path: "src/main.rs".to_string(),
            old_path: None,
            status: ChangeState::Modified,
        };

        assert_eq!(browse_compare_file_display(&file), "src/main.rs");
    }

    #[test]
    fn browse_compare_file_display_rename_path() {
        let file = BrowseCompareFile {
            path: "src/new.rs".to_string(),
            old_path: Some("src/old.rs".to_string()),
            status: ChangeState::Renamed,
        };

        assert_eq!(
            browse_compare_file_display(&file),
            "src/old.rs → src/new.rs"
        );
    }
}
