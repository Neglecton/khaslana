// 分支比较模式左侧差异文件列表 UI。
//
// 这里把 Git 服务层返回的差异文件扁平列表构造成目录嵌套文件树并渲染；
// 右侧内容/差异视图继续复用 browse_view.rs 中的浏览模式视图，避免重复实现
// diff 与全文渲染逻辑。

use std::collections::HashSet;
use std::path::Path;

use gpui::{
    Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, div, prelude::*, px,
    uniform_list,
};
use khaslana::{BrowseCompareFile, ChangeState};

use crate::ui::theme::rgb;
use crate::{
    CHANGE_ROW_HEIGHT, RepositoryView,
    ui::theme as ui_theme,
    ui_helpers::{
        ScrollbarMode, change_state_color, placeholder_row, scrollable_uniform_frame,
        section_header,
    },
};

/// 分支比较文件树中的一个可见行，目录和文件共用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompareTreeRow {
    /// 用于点击定位；目录填目录路径，文件填文件 path。
    pub path: String,
    /// 显示用的名字：目录就是目录名，文件就是文件名。
    pub name: String,
    pub depth: usize,
    pub kind: CompareTreeRowKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CompareTreeRowKind {
    /// 目录节点，expanded 表示当前是否展开。
    Directory { expanded: bool },
    /// 文件节点，保留原始差异文件信息以支持重命名展示和点击选中。
    File {
        status: ChangeState,
        old_path: Option<String>,
    },
}

/// 收集所有差异文件路径涉及的中间目录（git 风格相对路径）。
/// 渲染时默认全部展开；用户首次折叠时再把该集合固化为显式展开状态。
pub(crate) fn all_compare_dirs(files: &[BrowseCompareFile]) -> HashSet<String> {
    let mut dirs = HashSet::new();
    for file in files {
        collect_dirs(&file.path, &mut dirs);
        // 重命名文件可能涉及不同目录，旧路径所在目录也需要能正确嵌套展示，
        // 但显示名仍归属新路径所在目录，这里只按新路径构造目录树。
    }
    dirs
}

fn collect_dirs(path: &str, out: &mut HashSet<String>) {
    let mut acc = String::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if acc.is_empty() {
            acc = segment.to_string();
        } else {
            acc = format!("{acc}/{segment}");
        }
    }
    // acc 现在等于 path 本身；逐步去掉最后一段，得到所有中间目录。
    // 例如 "src/git/browse.rs" -> ["src", "src/git"]。
    while let Some(idx) = acc.rfind('/') {
        acc.truncate(idx);
        if !acc.is_empty() {
            out.insert(acc.clone());
        }
    }
}

/// 把差异文件扁平列表展平成带深度的可见行序列。
///
/// 目录排在文件之前，各自按名字排序；根目录本身不输出行。
/// `expanded` 为空时默认全部展开（首次加载）。
pub(crate) fn flatten_compare_files(
    files: &[BrowseCompareFile],
    expanded: &HashSet<String>,
) -> Vec<CompareTreeRow> {
    let default_all = expanded.is_empty();
    let mut rows = Vec::new();
    // 根 key 使用空字符串，与浏览模式 flatten_browse_tree 一致。
    flatten_dir("", 0, files, expanded, default_all, &mut rows);
    rows
}

fn flatten_dir(
    dir: &str,
    depth: usize,
    files: &[BrowseCompareFile],
    expanded: &HashSet<String>,
    default_all: bool,
    out: &mut Vec<CompareTreeRow>,
) {
    // 收集直接子项：子目录名 + 直接文件。
    let mut subdirs: HashSet<String> = HashSet::new();
    let mut direct_files: Vec<&BrowseCompareFile> = Vec::new();

    for file in files {
        // 只处理位于当前目录直接下层或更深层级的文件；不属于该目录的跳过，
        // 避免 strip_prefix 失败时把根级文件误算进子目录。
        let rest = if dir.is_empty() {
            file.path.as_str()
        } else {
            match file.path.strip_prefix(&format!("{dir}/")) {
                Some(rest) => rest,
                None => continue,
            }
        };
        if let Some(idx) = rest.find('/') {
            // 直接子目录
            let name = &rest[..idx];
            let full = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            };
            subdirs.insert(full);
        } else {
            direct_files.push(file);
        }
    }

    let mut subdirs: Vec<String> = subdirs.into_iter().collect();
    subdirs.sort();
    // 目录在前，文件在后。
    for subdir in subdirs {
        let name = subdir.rsplit('/').next().unwrap_or(&subdir).to_string();
        let is_expanded = default_all || expanded.contains(&subdir);
        out.push(CompareTreeRow {
            path: subdir.clone(),
            name,
            depth,
            kind: CompareTreeRowKind::Directory {
                expanded: is_expanded,
            },
        });
        if is_expanded {
            flatten_dir(&subdir, depth + 1, files, expanded, default_all, out);
        }
    }

    direct_files.sort_by(|a, b| {
        let an = a.path.rsplit('/').next().unwrap_or(&a.path);
        let bn = b.path.rsplit('/').next().unwrap_or(&b.path);
        an.cmp(bn)
    });
    for file in direct_files {
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_string();
        out.push(CompareTreeRow {
            path: file.path.clone(),
            name,
            depth,
            kind: CompareTreeRowKind::File {
                status: file.status.clone(),
                old_path: file.old_path.clone(),
            },
        });
    }
}

/// 文件名级别的显示文本；重命名展示为 `old_basename → new_basename`。
pub(crate) fn compare_file_leaf_display(name: &str, old_path: Option<&str>) -> String {
    match old_path {
        Some(old) => {
            let old_basename = old.rsplit('/').next().unwrap_or(old);
            if old_basename == name {
                name.to_string()
            } else {
                format!("{old_basename} → {name}")
            }
        }
        None => name.to_string(),
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
            // 右侧留出与滚动条（8px 厚 + 2px 边距）等宽的内边距，避免行白框右端被滚动条压住。
            .pl_2()
            .py_2()
            .pr(px(10.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    "browse-compare-list",
                    file_count.max(1),
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        if this.browse.compare_files.is_empty() {
                            return range
                                .map(|_| {
                                    placeholder_row(if !has_target {
                                        "正在解析引用..."
                                    } else if this.browse.compare_loading {
                                        "正在加载分支差异..."
                                    } else {
                                        "该分支与当前分支没有差异"
                                    })
                                    .into_any_element()
                                })
                                .collect::<Vec<_>>();
                        }
                        // 展开集合：空表示默认全部展开。
                        let expanded = if this.browse.compare_expanded.is_empty() {
                            all_compare_dirs(&this.browse.compare_files)
                        } else {
                            this.browse.compare_expanded.clone()
                        };
                        let rows = flatten_compare_files(&this.browse.compare_files, &expanded);
                        range
                            .map(move |index| {
                                rows.get(index)
                                    .cloned()
                                    .map(|row| {
                                        this.browse_compare_tree_row(row, cx).into_any_element()
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
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
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
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .truncate()
                                    .child(format!("比较：{target_display}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .font_family("Consolas, monospace")
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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

    /// 渲染文件树的一行（目录或文件）。
    fn browse_compare_tree_row(
        &self,
        row: CompareTreeRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let indent = px(12.0 * row.depth as f32);
        let path_for_click = row.path.clone();

        match row.kind {
            CompareTreeRowKind::Directory { expanded } => {
                let caret = if expanded { "▼" } else { "▶" };
                let icon = if expanded { "📂" } else { "📁" };
                div()
                    .id(format!("browse-compare-dir:{}", row.path))
                    .flex()
                    .flex_none()
                    .w_full()
                    .min_w(px(0.0))
                    .items_center()
                    .gap_1()
                    .h(px(CHANGE_ROW_HEIGHT))
                    .pl(indent)
                    .pr(px(8.0))
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .overflow_hidden()
                    .bg(rgb(ui_theme::CARD))
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                            this.toggle_compare_dir(path_for_click.clone());
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(14.0))
                            .text_size(px(10.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(caret),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.0))
                            .text_size(px(13.0))
                            .child(icon),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(row.name),
                    )
            }
            CompareTreeRowKind::File { status, old_path } => {
                let selected = self
                    .browse
                    .selected_file
                    .as_deref()
                    .map(|selected| selected == Path::new(&row.path))
                    .unwrap_or(false);
                let status_label = status.label();
                let status_color = change_state_color(&status);
                let display = compare_file_leaf_display(&row.name, old_path.as_deref());
                let file_for_click = BrowseCompareFile {
                    path: row.path.clone(),
                    old_path,
                    status,
                };

                div()
                    .id(format!("browse-compare-file:{}", row.path))
                    .flex()
                    .flex_none()
                    .w_full()
                    .min_w(px(0.0))
                    .items_center()
                    .gap_2()
                    .h(px(CHANGE_ROW_HEIGHT))
                    .pl(indent)
                    .pr(px(8.0))
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .bg(if selected {
                        rgb(ui_theme::ACCENT)
                    } else {
                        rgb(ui_theme::CARD)
                    })
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
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
                            .flex_none()
                            .w(px(18.0))
                            .text_size(px(13.0))
                            .child("📄"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .truncate()
                            .child(display),
                    )
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/browse_compare_view.rs"]
mod tests;
