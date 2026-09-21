//! 工作区原生样板（对照 Pencil 画板 `Sgcwi` / `a4JtW` / `GuA1a`）。
//!
//! 目的：在真实 GPUI Kit 运行时里验证「轻盈悬浮工作台」能否原生还原，
//! 并暴露 Kit 组件与自绘面板的接缝问题。演示数据是静态的，不连接 Git。

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable, Icon};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::tokens;

/// 差异行的三种形态。
#[derive(Clone, Copy, PartialEq)]
enum DiffKind {
    Context,
    Added,
    Removed,
}

/// 文件列表行。
struct FileRow {
    status: &'static str,
    status_color: Hsla,
    path: &'static str,
    additions: u32,
    deletions: u32,
}

/// 演示数据：未暂存（对齐画板 `Sgcwi`）。
fn unstaged_files() -> Vec<FileRow> {
    vec![
        FileRow {
            status: "M",
            status_color: tokens::badge_modified(),
            path: "ui/components.rs",
            additions: 28,
            deletions: 12,
        },
        FileRow {
            status: "M",
            status_color: tokens::badge_modified(),
            path: "ui/main.rs",
            additions: 42,
            deletions: 18,
        },
        FileRow {
            status: "A",
            status_color: tokens::badge_added(),
            path: "git/service.rs",
            additions: 52,
            deletions: 0,
        },
        FileRow {
            status: "M",
            status_color: tokens::badge_modified(),
            path: "Cargo.toml",
            additions: 6,
            deletions: 26,
        },
    ]
}

/// 演示数据：已暂存（对齐画板 `a4JtW`）。
fn staged_files() -> Vec<FileRow> {
    vec![
        FileRow {
            status: "A",
            status_color: tokens::badge_added(),
            path: "git/service.rs",
            additions: 52,
            deletions: 0,
        },
        FileRow {
            status: "M",
            status_color: tokens::badge_modified(),
            path: "Cargo.toml",
            additions: 6,
            deletions: 26,
        },
    ]
}

/// 差异内容：与画板 `Sgcwi` 同一段，行号与增删分布一致。
fn diff_lines() -> Vec<(DiffKind, &'static str, &'static str)> {
    vec![
        (DiffKind::Context, "118", "    pub fn render(&self) -> impl IntoView {"),
        (DiffKind::Context, "119", "        let surface = Surface::new()"),
        (DiffKind::Removed, "120", "−           .border_1()"),
        (DiffKind::Added, "121", "+           .rounded(PANEL_RADIUS)"),
        (DiffKind::Added, "122", "+           .shadow(PANEL_SHADOW)"),
        (DiffKind::Context, "123", "            .background(theme.panel);"),
        (DiffKind::Context, "124", " "),
        (DiffKind::Context, "125", "        surface.child("),
        (DiffKind::Context, "126", "            div()"),
        (DiffKind::Context, "127", "                .flex()"),
        (DiffKind::Removed, "128", "−               .gap_0()"),
        (DiffKind::Added, "129", "+               .gap_3()"),
        (DiffKind::Context, "130", "                .child(file_list)"),
        (DiffKind::Context, "131", "                .child(diff_view)"),
        (DiffKind::Context, "132", "        )"),
        (DiffKind::Context, "133", "    }"),
        (DiffKind::Context, "134", "}"),
    ]
}

const HUNK_HEADER: &str = "@@ -118,7 +118,10 @@ impl RepositoryView {";
/// 代码与行号用等宽字体；当前跟随 Kit 主题的等宽族（Windows 为 Consolas）。
const MONO: &str = "Consolas";

pub struct WorkbenchView {
    /// 演示开关：有暂存 → 双列表；无暂存 → 单列表。
    has_staged: bool,
    /// 演示忙碌态：提交按钮 loading。
    committing: bool,
    /// 宽窗也允许手动收起导航；与窄窗紧凑布局断点分离。
    navigator_collapsed: bool,
    search: Entity<InputState>,
    commit_message: Entity<TextareaState>,
}

impl WorkbenchView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索文件、符号或命令"));
        let commit_message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("简要描述这次修改…")
                .default_value("优化工作区面板布局")
        });
        Self {
            has_staged: true,
            committing: false,
            navigator_collapsed: false,
            search,
            commit_message,
        }
    }

    // ------------------------------------------------------------ 顶部工具栏

    fn render_toolbar(&self, cx: &mut Context<Self>, compact: bool) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(60.))
            .pl(px(24.))
            .pr(px(12.))
            .flex()
            .items_center()
            .rounded_tl(px(24.))
            .rounded_tr(px(24.))
            .bg(tokens::toolbar_bg())
            .child(
                div()
                    .flex_none()
                    .text_size(px(21.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(tokens::text_heading())
                    .child("Khaslana"),
            )
            .child(
                // 仓库上下文：白底浅实体 + 接触阴影
                div()
                    .flex_none()
                    .ml(px(16.))
                    .w(px(201.))
                    .h(px(tokens::H_CONTROL))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(tokens::RADIUS_CONTROL))
                    .bg(tokens::panel())
                    .shadow(tokens::control_shadow())
                    .child(
                        Icon::new(IconName::FolderGit2)
                            .size(px(16.))
                            .text_color(tokens::text_nav()),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::FONT_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens::text_strong())
                            .whitespace_nowrap()
                            .child("khaslana"),
                    )
                    .child(div().flex_1())
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(16.))
                            .text_color(tokens::text_nav()),
                    ),
            )
            .child(
                // 全局搜索：浅面内凹，右侧显示快捷键
                div()
                    .flex_1()
                    .ml(px(16.))
                    .max_w(px(490.))
                    .min_w(px(140.))
                    .h(px(tokens::H_CONTROL))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .rounded(px(tokens::RADIUS_CONTROL))
                    .bg(tokens::input_surface())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(18.))
                            .text_color(tokens::text_muted()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Input::new(&self.search).appearance(false).cleanable(false)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_placeholder())
                            .child("Ctrl P"),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(12.))
                    .flex()
                    .gap(px(10.))
                    .child(self.toolbar_command(cx, "refresh", IconName::RefreshCw, "刷新", compact))
                    .child(self.toolbar_command(cx, "fetch", IconName::RotateCw, "获取", compact))
                    .child(self.toolbar_command(cx, "pull", IconName::ArrowDown, "拉取", compact))
                    .child(self.toolbar_command(cx, "push", IconName::ArrowUp, "推送", compact)),
            )
            .child(div().flex_1())
            .child(
                // 窗口控制：32×32，间距 2，距右 12
                div()
                    .flex_none()
                    .flex()
                    .gap(px(2.))
                    .child(window_control(IconName::Minus, "minimize"))
                    .child(window_control(IconName::Square, "maximize"))
                    .child(window_control(IconName::X, "close")),
            )
    }

    fn toolbar_command(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        compact: bool,
    ) -> impl IntoElement {
        // 窄窗保留全部命令：按钮退化为图标 + tooltip，不把主操作挤出窗口。
        let button = Button::new(id)
            .secondary()
            .h(px(tokens::H_CONTROL))
            .border_1()
            .border_color(tokens::border_soft())
            .shadow(tokens::control_shadow())
            .icon(Icon::new(icon).size(px(16.)).text_color(tokens::text_nav()))
            .tooltip(label)
            .on_click(cx.listener(|_, _, _, _| {}));
        if compact { button } else { button.label(label) }
    }

    // ---------------------------------------------------------------- 导航区

    fn render_navigator(
        &self,
        cx: &mut Context<Self>,
        window_compact: bool,
        navigator_collapsed: bool,
    ) -> impl IntoElement {
        if window_compact || navigator_collapsed {
            return self.render_navigator_strip(cx).into_any_element();
        }
        div()
            .flex_none()
            .w(px(204.))
            .rounded(px(tokens::RADIUS_NAV))
            .bg(tokens::nav_surface())
            .shadow(tokens::panel_shadow())
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(12.))
            .gap(px(6.))
            .child(self.nav_mode_entry(cx, "worktree", IconName::House, "工作区", true))
            .child(self.nav_mode_entry(cx, "history", IconName::Clock, "提交历史", false))
            .child(self.nav_mode_entry(cx, "graph", IconName::GitFork, "提交图谱", false))
            .child(self.nav_mode_entry(cx, "workflow", IconName::Workflow, "工作流", false))
            .child(
                div()
                    .mt(px(12.))
                    .mb(px(4.))
                    .px(px(14.))
                    .text_size(px(tokens::FONT_META))
                    .text_color(tokens::text_muted())
                    .child("本地分支"),
            )
            .child(self.nav_ref_entry(cx, "branch-dev", IconName::GitBranch, "dev_gpuikit", true))
            .child(self.nav_ref_entry(cx, "branch-master", IconName::GitBranch, "master", false))
            .child(self.nav_ref_entry(cx, "remotes", IconName::Cloud, "远端", false))
            .child(self.nav_ref_entry(cx, "tags", IconName::Tag, "标签", false))
            .child(self.nav_ref_entry(cx, "stash", IconName::Archive, "贮藏", false))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.nav_icon_button(cx, IconName::Settings, "nav-settings", "设置", false))
                    .child(self.nav_icon_button(cx, IconName::PanelLeftClose, "nav-collapse", "收起导航", true)),
            )
            .into_any_element()
    }

    /// 窄窗折叠态：48px 图标栏，模式与设置入口仍可达。
    fn render_navigator_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .w(px(48.))
            .rounded(px(14.))
            .bg(tokens::nav_surface())
            .shadow(tokens::panel_shadow())
            .overflow_hidden()
            .flex()
            .flex_col()
            .items_center()
            .py(px(12.))
            .gap(px(6.))
            .child(strip_mode_entry(cx, "worktree", IconName::House, true))
            .child(strip_mode_entry(cx, "history", IconName::Clock, false))
            .child(strip_mode_entry(cx, "graph", IconName::GitFork, false))
            .child(strip_mode_entry(cx, "workflow", IconName::Workflow, false))
            .child(div().flex_1())
            .child(self.nav_icon_button(cx, IconName::Settings, "nav-settings-strip", "设置", false))
            .child(self.nav_icon_button(cx, IconName::PanelLeft, "nav-expand", "展开导航", true))
    }

    fn nav_icon_button(
        &self,
        cx: &mut Context<Self>,
        icon: IconName,
        id: &'static str,
        tooltip: &'static str,
        toggle: bool,
    ) -> impl IntoElement {
        Button::new(id)
            .ghost()
            .w(px(tokens::H_CONTROL))
            .h(px(tokens::H_CONTROL))
            .icon(Icon::new(icon).size(px(18.)).text_color(tokens::text_muted()))
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                if toggle {
                    this.navigator_collapsed = !this.navigator_collapsed;
                    cx.notify();
                }
            }))
    }

    fn nav_mode_entry(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        selected: bool,
    ) -> impl IntoElement {
        let (bg, fg, weight) = if selected {
            (
                tokens::selected_nav(),
                tokens::nav_selected(),
                FontWeight::SEMIBOLD,
            )
        } else {
            (
                Hsla::transparent_black(),
                tokens::text_nav(),
                FontWeight::NORMAL,
            )
        };
        div()
            .id(format!("nav-mode-{id}"))
            .h(px(42.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(tokens::RADIUS_CONTROL))
            .bg(bg)
            .cursor_pointer()
            .hover(|d| d.bg(tokens::hover_row()))
            .child(Icon::new(icon).size(px(18.)).text_color(fg))
            .child(
                div()
                    .text_size(px(tokens::FONT_PANEL_TITLE))
                    .font_weight(weight)
                    .text_color(fg)
                    .whitespace_nowrap()
                    .child(label),
            )
            .on_click(cx.listener(|_, _, _, _| {}))
    }

    fn nav_ref_entry(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        selected: bool,
    ) -> impl IntoElement {
        let (bg, fg) = if selected {
            (tokens::selected_branch(), tokens::branch_selected())
        } else {
            (Hsla::transparent_black(), tokens::text_nav())
        };
        div()
            .id(format!("nav-ref-{id}"))
            .h(px(tokens::H_CONTROL))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(tokens::RADIUS_ROW))
            .bg(bg)
            .cursor_pointer()
            .hover(|d| d.bg(tokens::hover_row()))
            .child(
                Icon::new(icon)
                    .size(px(16.))
                    .text_color(if selected {
                        tokens::primary()
                    } else {
                        tokens::text_muted()
                    }),
            )
            .child(
                div()
                    .text_size(px(tokens::FONT_BODY))
                    .text_color(fg)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .on_click(cx.listener(|_, _, _, _| {}))
    }

    // ---------------------------------------------------------------- 主工作台

    fn render_page_header(&self, cx: &mut Context<Self>, compact: bool) -> impl IntoElement {
        let files = 4;
        div()
            .flex_none()
            .h(px(58.))
            .flex()
            .items_end()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(if compact {
                                20.
                            } else {
                                tokens::FONT_PAGE_TITLE
                            }))
                            .font_weight(FontWeight::BOLD)
                            .text_color(tokens::text_heading())
                            .child("工作区"),
                    )
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .text_size(px(tokens::FONT_BODY))
                                .text_color(tokens::text_subtle())
                                .child("查看变更、编写提交信息并进行代码审查"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(tokens::FONT_BODY))
                            .text_color(tokens::text_secondary())
                            .child(format!("{files} 个文件变更")),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::FONT_META))
                            .text_color(tokens::add_text())
                            .child("+128"),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::FONT_META))
                            .text_color(tokens::del_text())
                            .child("−56"),
                    )
                    .child(self.render_demo_switch(cx)),
            )
    }

    /// 文件列表面板；标题固定，行列表内部滚动。
    fn render_file_panel(&self, cx: &mut Context<Self>, staged: bool) -> impl IntoElement {
        let files = if staged {
            staged_files()
        } else if self.has_staged {
            unstaged_files()
                .into_iter()
                .filter(|file| file.path == "ui/components.rs" || file.path == "ui/main.rs")
                .collect()
        } else {
            unstaged_files()
        };
        let count = files.len();
        let title = if staged {
            "已暂存文件"
        } else if self.has_staged {
            "未暂存文件"
        } else {
            "变更文件"
        };
        let subtitle = if staged {
            "将包含在本次提交中".to_string()
        } else if self.has_staged {
            "工作区变更".to_string()
        } else {
            format!("未暂存  {count}")
        };
        let action = if staged { "全部取消暂存" } else { "全部暂存" };
        let mut rows = Vec::new();
        for (ix, file) in files.iter().enumerate() {
            rows.push(self.render_file_row(cx, file, ix, staged, !staged && ix == 0));
        }

        div()
            .id(if staged { "file-panel-staged" } else { "file-panel-unstaged" })
            .flex_1()
            .min_h(px(0.))
            .rounded(px(tokens::RADIUS_PANEL))
            .bg(tokens::panel_veil())
            .shadow(tokens::panel_shadow())
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(12.))
            .child(
                div()
                    .flex_none()
                    .h(px(32.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(tokens::FONT_PANEL_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tokens::text_strong())
                            .child(format!("{title}  {count}")),
                    )
                    .when(!staged, |d| {
                        d.child(
                            Icon::new(IconName::ListFilter)
                                .size(px(18.))
                                .text_color(tokens::text_muted()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(25.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(tokens::FONT_META))
                            .text_color(tokens::text_muted())
                            .child(subtitle),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::FONT_META))
                            .text_color(tokens::primary())
                            .cursor_pointer()
                            .child(action),
                    ),
            )
            .child(
                div()
                    .id(if staged { "file-rows-staged" } else { "file-rows-unstaged" })
                    .flex_1()
                    .min_h(px(0.))
                    .mt(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .pt(px(8.))
                    .px(px(4.))
                    .text_size(px(tokens::FONT_MICRO))
                    .text_color(tokens::text_subtle())
                    .child(if staged {
                        format!("{count} 个文件已就绪")
                    } else {
                        format!("{count} 个文件等待暂存")
                    }),
            )
    }

    fn render_file_row(
        &self,
        cx: &mut Context<Self>,
        file: &FileRow,
        ix: usize,
        staged: bool,
        selected: bool,
    ) -> AnyElement {
        div()
            .id((if staged { "file-row-staged" } else { "file-row-unstaged" }, ix))
            .flex_none()
            .h(px(tokens::H_FILE_ROW))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(tokens::RADIUS_ROW))
            .when(selected, |d| d.bg(tokens::selected_row()))
            .cursor_pointer()
            .hover(|d| d.bg(tokens::hover_row()))
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .text_size(px(tokens::FONT_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(file.status_color)
                    .child(file.status),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(tokens::FONT_META))
                    .text_color(tokens::text_strong())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(file.path),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(tokens::text_stats())
                    .child(if file.deletions == 0 {
                        format!("+{}", file.additions)
                    } else if file.additions == 0 {
                        format!("−{}", file.deletions)
                    } else {
                        format!("+{} −{}", file.additions, file.deletions)
                    }),
            )
            .on_click(cx.listener(|_, _, _, _| {}))
            .into_any_element()
    }

    // ---------------------------------------------------------------- 差异面板

    fn render_diff_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h(px(0.))
            .rounded(px(tokens::RADIUS_PANEL))
            .bg(tokens::panel())
            .shadow(tokens::panel_shadow())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .h(px(56.))
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                Icon::new(IconName::FileCode)
                                    .size(px(18.))
                                    .text_color(tokens::primary()),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::FONT_BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tokens::text_strong())
                                    .child("src/ui/components.rs"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                Button::new("diff-unified")
                                    .secondary()
                                    .h(px(tokens::H_CONTROL))
                                    .label("统一")
                                    .on_click(cx.listener(|_, _, _, _| {})),
                            )
                            .child(
                                Button::new("diff-more")
                                    .ghost()
                                    .h(px(tokens::H_CONTROL))
                                    .w(px(tokens::H_CONTROL))
                                    .icon(
                                        Icon::new(IconName::Ellipsis)
                                            .size(px(18.))
                                            .text_color(tokens::text_muted()),
                                    )
                                    .on_click(cx.listener(|_, _, _, _| {})),
                            ),
                    ),
            )
            .child(
                // hunk 头：与代码行同高，整行浅底
                div()
                    .flex_none()
                    .h(px(tokens::H_DIFF_LINE))
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .bg(tokens::diff_hunk_bg())
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_hunk())
                            .child(HUNK_HEADER),
                    ),
            )
            .child(
                div()
                    .id("diff-lines")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(diff_lines().into_iter().map(|(kind, no, text)| {
                        let (bg, fg) = match kind {
                            DiffKind::Context => (tokens::panel(), tokens::text_body()),
                            DiffKind::Added => (tokens::diff_add_bg(), tokens::diff_add_fg()),
                            DiffKind::Removed => (tokens::diff_del_bg(), tokens::diff_del_fg()),
                        };
                        div()
                            .flex_none()
                            .h(px(tokens::H_DIFF_LINE))
                            .pl(px(18.))
                            .flex()
                            .items_center()
                            .bg(bg)
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(44.))
                                    .pr(px(16.))
                                    .text_right()
                                    .font_family(MONO)
                                    .text_size(px(tokens::FONT_MICRO))
                                    .text_color(tokens::text_line_no())
                                    .child(no),
                            )
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(px(tokens::FONT_CODE))
                                    .text_color(fg)
                                    .whitespace_nowrap()
                                    .child(text),
                            )
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(28.))
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_subtle())
                            .child("UTF-8  ·  Rust"),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::FONT_MICRO))
                            .text_color(tokens::text_stats())
                            .child("+28  −12"),
                    ),
            )
    }

    // ---------------------------------------------------------------- 提交区

    fn render_commit_panel(&self, cx: &mut Context<Self>, compact: bool) -> impl IntoElement {
        let staged_count = if self.has_staged {
            staged_files().len()
        } else {
            0
        };
        let can_commit = staged_count > 0 && !self.committing;

        div()
            .flex_none()
            .h(px(if compact { 156. } else { 190. }))
            .rounded(px(tokens::RADIUS_PANEL))
            .bg(tokens::panel_veil())
            .shadow(tokens::panel_shadow())
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(16.))
            .gap(px(14.))
            .child(
                div()
                    .flex_none()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(tokens::FONT_PANEL_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tokens::text_strong())
                            .child("提交信息"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(
                                Icon::new(IconName::Sparkles)
                                    .size(px(15.))
                                    .text_color(tokens::primary()),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::FONT_META))
                                    .text_color(tokens::primary())
                                    .child("AI 生成"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(72.))
                    .rounded(px(tokens::RADIUS_CONTROL))
                    .bg(tokens::input_surface())
                    .px(px(12.))
                    .py(px(10.))
                    .child(Textarea::new(&self.commit_message).appearance(false)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Checkbox::new("amend")
                            .label("修补上一次提交")
                            .text_size(px(tokens::FONT_META))
                            .text_color(tokens::text_subtle()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .when(!compact, |d| {
                                d.child(
                                    div()
                                        .text_size(px(tokens::FONT_META))
                                        .text_color(tokens::text_subtle())
                                        .child(if staged_count > 0 {
                                            format!("已暂存 {staged_count} 个文件")
                                        } else {
                                            "暂存区为空".to_string()
                                        }),
                                )
                            })
                            .child(
                                Button::new("commit")
                                    .secondary()
                                    .h(px(tokens::H_CONTROL))
                                    .icon(Icon::new(IconName::Check).size(px(16.)))
                                    .label("提交到 dev_gpuikit")
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|_, _, _, _| {})),
                            )
                            .child(
                                Button::new("commit-push")
                                    .primary()
                                    .h(px(tokens::H_CONTROL))
                                    .icon(Icon::new(IconName::ArrowUp).size(px(16.)))
                                    .label("提交并推送")
                                    .shadow(tokens::primary_shadow())
                                    .loading(self.committing)
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.committing = true;
                                        cx.notify();
                                        cx.spawn(async move |this, cx| {
                                            cx.background_executor()
                                                .timer(Duration::from_millis(1200))
                                                .await;
                                            let _ = this.update(cx, |this, cx| {
                                                this.committing = false;
                                                cx.notify();
                                            });
                                        })
                                        .detach();
                                    })),
                            ),
                    ),
            )
    }

    // ------------------------------------------------------------------ 状态栏

    fn render_status_bar(&self) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(14.))
            .px(px(24.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(tokens::text_status())
                    .child("就绪  ·  khaslana"),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(tokens::text_status())
                    .child(if self.has_staged {
                        "有暂存 · 同时展示两个列表"
                    } else {
                        "无暂存 · 仅展示未暂存列表"
                    }),
            )
    }

    /// 样板自带的演示开关：切换画板 `Sgcwi` / `a4JtW` 两种状态。
    fn render_demo_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let option = move |id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>| {
            div()
                .id(format!("demo-switch-{id}"))
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .when(on, |d| d.bg(tokens::primary()))
                .cursor_pointer()
                .text_size(px(tokens::FONT_MICRO))
                .text_color(if on {
                    tokens::primary_foreground()
                } else {
                    tokens::text_muted()
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.has_staged = id == "staged";
                    cx.notify();
                }))
        };

        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .p(px(3.))
            .rounded(px(tokens::RADIUS_CONTROL))
            .bg(tokens::input_surface())
            .child(option("unstaged", "无暂存", !self.has_staged, cx))
            .child(option("staged", "有暂存", self.has_staged, cx))
    }
}

impl Render for WorkbenchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 窄窗断点：860×520 最小窗下命令必须仍可达（视觉规范 §5）。
        let compact = window.viewport_size().width < px(1120.);
        div()
            .size_full()
            .rounded(px(24.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(tokens::env_bg())
            .text_size(px(tokens::FONT_BODY))
            .text_color(tokens::text_body())
            .child(self.render_toolbar(cx, compact))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(24.))
                    .pt(px(24.))
                    .pb(px(20.))
                    .flex()
                    .gap(px(20.))
                    .child(self.render_navigator(cx, compact, self.navigator_collapsed))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(tokens::GAP_PANEL))
                            .child(self.render_page_header(cx, compact))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .flex()
                                    .gap(px(tokens::GAP_PANEL))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(if compact { 240. } else { 340. }))
                                            .flex()
                                            .flex_col()
                                            .gap(px(tokens::GAP_PANEL))
                                            .when(self.has_staged, |d| {
                                                d.child(self.render_file_panel(cx, true))
                                                    .child(self.render_file_panel(cx, false))
                                            })
                                            .when(!self.has_staged, |d| {
                                                d.child(self.render_file_panel(cx, false))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .flex()
                                            .flex_col()
                                            .gap(px(tokens::GAP_PANEL))
                                            .child(self.render_diff_panel(cx))
                                            .child(self.render_commit_panel(cx, compact)),
                                    ),
                            ),
                    ),
            )
            .child(self.render_status_bar())
            .child(div().flex_none().h(px(24.)))
    }
}

/// 窄窗图标栏的模式按钮：36×36，选中态为浅底圆角。
fn strip_mode_entry(
    cx: &mut Context<WorkbenchView>,
    id: &'static str,
    icon: IconName,
    selected: bool,
) -> impl IntoElement {
    let fg = if selected {
        tokens::nav_selected()
    } else {
        tokens::text_nav()
    };
    div()
        .id(format!("strip-mode-{id}"))
        .w(px(36.))
        .h(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(tokens::RADIUS_CONTROL))
        .when(selected, |d| d.bg(tokens::selected_nav()))
        .cursor_pointer()
        .hover(|d| d.bg(tokens::hover_row()))
        .child(Icon::new(icon).size(px(18.)).text_color(fg))
        .on_click(cx.listener(|_, _, _, _| {}))
}

/// 窗口控制按钮：32×32，hover 浮出浅底。
fn window_control(icon: IconName, kind: &'static str) -> impl IntoElement {
    div()
        .id(format!("window-control-{kind}"))
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|d| d.bg(tokens::hover_row()))
        .child(
            Icon::new(icon)
                .size(px(14.))
                .text_color(tokens::text_nav()),
        )
}

