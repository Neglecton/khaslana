use crate::ui::theme::rgb;
use gpui::{
    ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, Window, div, prelude::*, px,
};
use khaslana::{BranchInfo, BranchKind, BranchName, RemoteInfo, StashInfo, TagInfo};

use crate::{
    BRANCH_MENU_HEIGHT, BRANCH_MENU_WIDTH, BranchContextMenu, FieldId, REMOTE_MENU_HEIGHT,
    REMOTE_MENU_WIDTH, RemoteContextMenu, RepositoryView, STASH_MENU_HEIGHT, STASH_MENU_WIDTH,
    SidebarSection, StashContextMenu, TAG_MENU_HEIGHT, TAG_MENU_WIDTH, TagContextMenu,
    clamped_menu_position, context_menu_item, context_menu_item_with_context, menu_separator,
    nav_list, placeholder_row,
    ui::{
        components::{glass_menu, sync_badge, tooltip_text},
        icons::{ToolbarIcon, toolbar_icon, toolbar_icon_rotated},
        theme as ui_theme,
    },
};

fn filter_sidebar_branches(
    branches: &[BranchInfo],
    kind: BranchKind,
    query: &str,
) -> Vec<BranchInfo> {
    branches
        .iter()
        .filter(|branch| branch.kind == kind)
        .filter(|branch| sidebar_branch_matches_query(branch, query))
        .cloned()
        .collect()
}

fn sidebar_branch_matches_query(branch: &BranchInfo, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let query = query.to_lowercase();
    branch.name.to_lowercase().contains(&query)
        || branch
            .upstream
            .as_deref()
            .is_some_and(|upstream| upstream.to_lowercase().contains(&query))
}

/// 从 upstream 全名中去掉 refs/remotes/ 前缀，返回简短显示名。
/// 例如 "refs/remotes/origin/main" → "origin/main"
fn strip_remote_prefix(upstream: &str) -> String {
    upstream
        .strip_prefix("refs/remotes/")
        .unwrap_or(upstream)
        .to_string()
}

const SIDEBAR_LOCAL_BRANCH_CREATE_ID: &str = "sidebar-local-branch-create";

fn sidebar_branch_search_button_id(section: SidebarSection) -> &'static str {
    // 图标按钮不显示文字，必须用分组专属 id，避免本地/远端搜索入口点击命中冲突。
    match section {
        SidebarSection::LocalBranches => "sidebar-local-branch-search-toggle",
        SidebarSection::RemoteBranches => "sidebar-remote-branch-search-toggle",
        _ => "sidebar-branch-search-toggle",
    }
}

impl RepositoryView {
    pub(crate) fn render_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.snapshot.as_ref();
        let branches = snapshot
            .map(|snapshot| snapshot.branches.clone())
            .unwrap_or_default();
        let local_search = if self.sidebar_local_branch_search_open {
            self.sidebar_local_branch_search.value.trim()
        } else {
            ""
        };
        let remote_branch_search = if self.sidebar_remote_branch_search_open {
            self.sidebar_remote_branch_search.value.trim()
        } else {
            ""
        };
        let local_placeholder = if local_search.is_empty() {
            None
        } else {
            Some("没有匹配的本地分支")
        };
        let remote_branch_placeholder = if self.loading.remote() {
            Some("远端分支加载中...")
        } else if remote_branch_search.is_empty() {
            None
        } else {
            Some("没有匹配的远端分支")
        };
        let local_rows = filter_sidebar_branches(&branches, BranchKind::Local, local_search)
            .into_iter()
            .map(|branch| self.branch_row(branch, cx).into_any_element())
            .collect::<Vec<_>>();
        let remote_branch_rows =
            filter_sidebar_branches(&branches, BranchKind::Remote, remote_branch_search)
                .into_iter()
                .map(|branch| self.branch_row(branch, cx).into_any_element())
                .collect::<Vec<_>>();
        let remote_rows = snapshot
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|remote| self.remote_row(remote, cx).into_any_element())
            .collect::<Vec<_>>();
        let tag_rows = snapshot
            .map(|snapshot| snapshot.tags.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|tag| self.tag_row(tag, cx).into_any_element())
            .collect::<Vec<_>>();
        let stash_rows = snapshot
            .map(|snapshot| snapshot.stashes.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|stash| self.stash_row(stash, cx).into_any_element())
            .collect::<Vec<_>>();
        let local_branch_filter = if self.sidebar_local_branch_search_open {
            Some(
                self.sidebar_branch_search_input(FieldId::SidebarLocalBranchSearch, window, cx)
                    .into_any_element(),
            )
        } else {
            None
        };
        let remote_branch_filter = if self.sidebar_remote_branch_search_open {
            Some(
                self.sidebar_branch_search_input(FieldId::SidebarRemoteBranchSearch, window, cx)
                    .into_any_element(),
            )
        } else {
            None
        };
        let local_branch_action = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .child(self.sidebar_create_branch_button(cx))
            .child(self.sidebar_branch_search_button(
                SidebarSection::LocalBranches,
                self.sidebar_local_branch_search_open,
                cx,
            ))
            .into_any_element();
        let remote_branch_action = self.sidebar_branch_search_button(
            SidebarSection::RemoteBranches,
            self.sidebar_remote_branch_search_open,
            cx,
        );

        // 设计图：远端区域右侧「管理」药丸按钮
        // cornerRadius $--radius-pill, stroke $--sidebar-border, padding [2,8]
        let manage_pill = {
            let enabled = self.repo_path.is_some() && !self.busy;
            div()
                .id("sidebar-remote-manage")
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(ui_theme::RADIUS_PILL))
                .border_1()
                .border_color(rgb(ui_theme::SIDEBAR_BORDER))
                .bg(rgb(ui_theme::SIDEBAR))
                .text_size(px(10.0))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_color(rgb(ui_theme::SIDEBAR_FOREGROUND))
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                })
                .when(!enabled, |this| this.opacity(0.62))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.open_remote_manager();
                    cx.notify();
                }))
                .child("管理")
                .into_any_element()
        };

        // 设计图：标签区域右侧计数 badge
        let tag_count = tag_rows.len();
        let tag_action = if tag_count > 0 {
            Some(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(tag_count.to_string())
                    .into_any_element(),
            )
        } else {
            None
        };

        // 设计图：贮藏区域右侧计数 badge
        let stash_count = stash_rows.len();
        let stash_action = if stash_count > 0 {
            Some(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(stash_count.to_string())
                    .into_any_element(),
            )
        } else {
            None
        };

        // 设计图：分割线 — 1px $--sidebar-border, 左右 padding 16px
        let sidebar_divider = || {
            div()
                .flex_none()
                .px(px(16.0))
                .py(px(8.0))
                .child(div().w_full().h(px(1.0)).bg(rgb(ui_theme::SIDEBAR_BORDER)))
        };

        let mut sidebar = div()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.sidebar_width))
            .min_w(px(self.sidebar_width))
            .h_full()
            // 扁平纯色背景，与工具栏保持一致。
            .bg(rgb(ui_theme::SIDEBAR))
            .pt(px(12.0))
            .child(self.render_nav_section(
                "本地分支",
                "local-branch-list",
                SidebarSection::LocalBranches,
                local_rows,
                local_placeholder,
                local_branch_filter,
                3.0,
                Some(local_branch_action),
                cx,
            ))
            .child(sidebar_divider())
            .child(self.render_nav_section(
                "远端",
                "remote-list",
                SidebarSection::Remotes,
                remote_rows,
                self.loading.remote().then_some("远端加载中..."),
                None,
                2.0,
                Some(manage_pill),
                cx,
            ))
            .child(sidebar_divider())
            .child(self.render_nav_section(
                "远端分支",
                "remote-branch-list",
                SidebarSection::RemoteBranches,
                remote_branch_rows,
                remote_branch_placeholder,
                remote_branch_filter,
                3.0,
                Some(remote_branch_action),
                cx,
            ));

        if !tag_rows.is_empty() {
            sidebar = sidebar
                .child(sidebar_divider())
                .child(self.render_nav_section(
                    "标签",
                    "tag-list",
                    SidebarSection::Tags,
                    tag_rows,
                    None,
                    None,
                    2.0,
                    tag_action,
                    cx,
                ));
        }
        if !stash_rows.is_empty() {
            sidebar = sidebar
                .child(sidebar_divider())
                .child(self.render_nav_section(
                    "贮藏",
                    "stash-list",
                    SidebarSection::Stashes,
                    stash_rows,
                    None,
                    None,
                    2.0,
                    stash_action,
                    cx,
                ));
        }

        sidebar
    }

    fn render_nav_section(
        &self,
        title: &'static str,
        id: &'static str,
        section: SidebarSection,
        rows: Vec<gpui::AnyElement>,
        placeholder: Option<&'static str>,
        filter: Option<gpui::AnyElement>,
        weight: f32,
        action: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.sidebar_sections.is_expanded(section);
        let header = self.nav_section_header(title, section, expanded, action, cx);
        let rows = if rows.is_empty() {
            placeholder
                .map(|text| placeholder_row(text).into_any_element())
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            rows
        };
        div()
            .flex()
            .flex_col()
            .when(expanded, |this| this.flex_1().min_h(px(96.0)))
            .when(!expanded, |this| this.flex_none())
            // 设计图：区域间用分割线隔开，不用 border-top
            // 分割线由 render_sidebar 中的 sidebar_divider() 单独渲染
            .when(expanded, |this| {
                let mut this = this;
                this.style().flex_grow = Some(weight);
                this
            })
            .child(header)
            .when_some(filter.filter(|_| expanded), |this, filter| {
                this.child(filter)
            })
            .when(expanded, |this| this.child(nav_list(self, id, rows, cx)))
    }

    fn sidebar_branch_search_input(
        &self,
        field: FieldId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_none()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::SIDEBAR))
            // 复用统一输入框，确保侧边栏搜索也支持现有 IME、选区和光标逻辑。
            .child(self.input(field, true, window, cx))
    }

    fn sidebar_create_branch_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.sidebar_header_icon_button(
            SIDEBAR_LOCAL_BRANCH_CREATE_ID,
            ToolbarIcon::Plus,
            false,
            self.repo_path.is_some() && !self.busy,
            |this, _, _| this.open_create_branch_dialog(),
            cx,
        )
    }

    fn sidebar_branch_search_button(
        &self,
        section: SidebarSection,
        open: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let icon = if open {
            ToolbarIcon::Close
        } else {
            ToolbarIcon::Search
        };
        self.sidebar_header_icon_button(
            sidebar_branch_search_button_id(section),
            icon,
            open,
            self.repo_path.is_some(),
            move |this, window, _| this.toggle_sidebar_branch_search(section, window),
            cx,
        )
    }

    fn sidebar_header_icon_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        active: bool,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 设计图：20×20 圆角方块，$--radius-xs，无描边
        // icon 14px，$--sidebar-foreground 色
        let icon_color = if !enabled {
            ui_theme::MUTED_FOREGROUND
        } else if active {
            ui_theme::PRIMARY
        } else {
            ui_theme::SIDEBAR_FOREGROUND
        };
        div()
            .id(id)
            .flex_none()
            .size(px(20.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(if active {
                ui_theme::ACCENT
            } else {
                ui_theme::SIDEBAR
            }))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.62))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(icon, icon_color))
            .into_any_element()
    }

    fn toggle_sidebar_branch_search(&mut self, section: SidebarSection, window: &mut Window) {
        self.close_popups();
        match section {
            SidebarSection::LocalBranches => {
                self.sidebar_local_branch_search_open = !self.sidebar_local_branch_search_open;
                if self.sidebar_local_branch_search_open {
                    // 搜索按钮同时负责展开分组，避免输入框打开后被折叠状态隐藏。
                    if !self.sidebar_sections.is_expanded(section) {
                        self.sidebar_sections.toggle(section);
                    }
                    window.focus(&self.sidebar_local_branch_search.focus);
                } else {
                    self.sidebar_local_branch_search.clear();
                }
            }
            SidebarSection::RemoteBranches => {
                self.sidebar_remote_branch_search_open = !self.sidebar_remote_branch_search_open;
                if self.sidebar_remote_branch_search_open {
                    // 搜索按钮同时负责展开分组，避免输入框打开后被折叠状态隐藏。
                    if !self.sidebar_sections.is_expanded(section) {
                        self.sidebar_sections.toggle(section);
                    }
                    window.focus(&self.sidebar_remote_branch_search.focus);
                } else {
                    self.sidebar_remote_branch_search.clear();
                }
            }
            _ => {}
        }
    }

    fn nav_section_header(
        &self,
        title: &'static str,
        section: SidebarSection,
        expanded: bool,
        action: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 设计图：区域标题 Funnel Sans 风格
        // fontSize 11, fontWeight 600, letterSpacing 0.5, $--sidebar-foreground 色
        let is_collapsible = matches!(
            section,
            SidebarSection::Remotes
                | SidebarSection::Tags
                | SidebarSection::Stashes
                | SidebarSection::RemoteBranches
        );

        let title_el = div()
            .min_w(px(0.0))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(ui_theme::SIDEBAR_FOREGROUND))
            .truncate()
            .child(title);

        // 折叠区域折叠状态：chevron-right + 标题 + action(计数badge/管理药丸)
        if is_collapsible && !expanded {
            let chevron = div()
                .id(format!("sidebar-section-toggle-{title}"))
                .flex_none()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_sidebar_section(section);
                    cx.notify();
                }))
                .child(toolbar_icon(
                    ToolbarIcon::ChevronRight,
                    ui_theme::SIDEBAR_FOREGROUND,
                ))
                .into_any_element();

            return div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(16.0))
                .py(px(8.0))
                .child(chevron)
                .child(title_el)
                .when_some(action, |this, action| this.child(action));
        }

        // 可折叠区域展开状态：chevron-down + 标题 + 右侧操作按钮
        if is_collapsible && expanded {
            let chevron = div()
                .id(format!("sidebar-section-toggle-{title}"))
                .flex_none()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_sidebar_section(section);
                    cx.notify();
                }))
                .child(toolbar_icon_rotated(
                    ToolbarIcon::ChevronRight,
                    ui_theme::SIDEBAR_FOREGROUND,
                    90.0,
                ))
                .into_any_element();

            let actions = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.0))
                .when_some(action, |this, action| this.child(action));

            return div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px(px(16.0))
                .py(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .child(chevron)
                        .child(title_el),
                )
                .child(actions);
        }

        // 非折叠区域（本地分支）：标题 + 右侧操作按钮（space_between 布局）
        let actions = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .when_some(action, |this, action| this.child(action));

        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(16.0))
            .py(px(8.0))
            .child(title_el)
            .child(actions)
    }

    fn remote_row(&self, remote: RemoteInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.current_remote().as_deref() == Some(remote.name.as_str());
        let name = remote.name.clone();
        let right_click_name = remote.name.clone();

        // 设计图：globe icon 14px + 名称 fontWeight 500, padding [6,16,6,24]
        let name_color = if selected {
            ui_theme::PRIMARY
        } else {
            ui_theme::SIDEBAR_ACCENT_FOREGROUND
        };

        div()
            .id(format!("remote-{}", remote.name))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(if selected {
                rgb(ui_theme::SIDEBAR_ACCENT)
            } else {
                rgb(ui_theme::SIDEBAR)
            })
            .hover(move |this| {
                if selected {
                    this.bg(rgb(ui_theme::SIDEBAR_ACCENT))
                } else {
                    this.bg(rgb(ui_theme::ACCENT))
                }
            })
            .cursor_pointer()
            // globe icon 14px, $--sidebar-foreground 色
            .child(toolbar_icon(
                ToolbarIcon::Globe,
                ui_theme::SIDEBAR_FOREGROUND,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(name_color))
                    .truncate()
                    .child(remote.name),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.selected_remote = Some(name.clone());
                this.close_popups();
                if let Some((tab_id, path, remote, load_id, request_id)) =
                    this.prepare_branch_sync_status_request()
                {
                    this.load_branch_sync_status_for_tab(tab_id, path, remote, load_id, request_id);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.selected_remote = Some(right_click_name.clone());
                    this.active_dialog = None;
                    this.branch_context_menu = None;
                    this.change_context_menu = None;
                    this.credential_context_menu = None;
                    this.tag_context_menu = None;
                    this.stash_context_menu = None;
                    this.commit_context_menu = None;
                    this.encoding_menu_target = None;
                    let (x, y) =
                        clamped_menu_position(event, window, REMOTE_MENU_WIDTH, REMOTE_MENU_HEIGHT);
                    this.remote_context_menu = Some(RemoteContextMenu {
                        remote: right_click_name.clone(),
                        x,
                        y,
                    });
                    cx.notify();
                }),
            )
    }

    pub(crate) fn render_remote_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.remote_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(REMOTE_MENU_WIDTH))
            .child(context_menu_item(
                "刷新",
                !self.busy,
                {
                    let remote = menu.remote.clone();
                    move |this| this.refresh_remote(remote.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    fn tag_row(&self, tag: TagInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let name = tag.name.clone();
        let right_click_name = tag.name.clone();

        // 设计图：与分支行一致的样式，$--sidebar-accent-foreground 色
        div()
            .id(format!("tag-{}", tag.name))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(rgb(ui_theme::SIDEBAR))
            .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::SIDEBAR_ACCENT_FOREGROUND))
                    .truncate()
                    .child(name),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.branch_context_menu = None;
                    this.change_context_menu = None;
                    this.stash_context_menu = None;
                    this.commit_context_menu = None;
                    this.encoding_menu_target = None;
                    this.active_dialog = None;
                    let (x, y) =
                        clamped_menu_position(event, window, TAG_MENU_WIDTH, TAG_MENU_HEIGHT);
                    this.tag_context_menu = Some(TagContextMenu {
                        tag: right_click_name.clone(),
                        x,
                        y,
                    });
                    cx.notify();
                }),
            )
    }

    fn stash_row(&self, stash: StashInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let index = stash.index;
        let label = format!("stash@{{{}}} {}", stash.index, stash.message);

        // 设计图：与分支行一致的样式
        div()
            .id(format!("stash-{}", stash.index))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(rgb(ui_theme::SIDEBAR))
            .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::SIDEBAR_ACCENT_FOREGROUND))
                    .truncate()
                    .child(label),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.branch_context_menu = None;
                    this.change_context_menu = None;
                    this.tag_context_menu = None;
                    this.commit_context_menu = None;
                    this.encoding_menu_target = None;
                    this.active_dialog = None;
                    let (x, y) =
                        clamped_menu_position(event, window, STASH_MENU_WIDTH, STASH_MENU_HEIGHT);
                    this.stash_context_menu = Some(StashContextMenu { index, x, y });
                    cx.notify();
                }),
            )
    }

    fn branch_row(&self, branch: BranchInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let is_local = branch.kind == BranchKind::Local;
        let is_current = branch.is_head;
        let name = branch.name.clone();
        let click_name = branch.name.clone();
        let click_kind = branch.kind.clone();
        let click_is_head = branch.is_head;
        let right_click_name = branch.name.clone();
        let right_click_kind = branch.kind.clone();
        let right_click_is_head = branch.is_head;
        let selected = self.selected_branch.as_deref() == Some(&branch.name);
        let upstream = branch.upstream.clone();
        let ahead = branch.ahead.unwrap_or(0);
        let behind = branch.behind.unwrap_or(0);

        // 设计图：
        // 活跃分支：bg $--sidebar-accent, 左侧 HEAD 圆点 6×6 $--primary,
        //   名称 fontWeight 600 $--sidebar-primary-foreground, upstream 小字 $--muted-foreground
        // 非活跃分支：bg 透明, 左侧空 24px, 名称 normal weight $--sidebar-accent-foreground
        // 远端分支：名称 $--muted-foreground
        let row_bg = if is_current {
            ui_theme::SIDEBAR_ACCENT
        } else if selected {
            ui_theme::SIDEBAR_ACCENT
        } else {
            ui_theme::SIDEBAR
        };

        let leading = if is_current {
            // HEAD 指示圆点 6×6, $--primary
            div()
                .flex_none()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(3.0))
                .bg(rgb(ui_theme::PRIMARY))
                .into_any_element()
        } else {
            // 空白占位，对齐 HEAD 行（24px = 16px left padding + 6px dot + 8px gap）
            div().flex_none().w(px(6.0)).into_any_element()
        };

        // 分支名颜色：选中不再变蓝，仅当前分支用深色加粗
        let name_color = if is_current {
            ui_theme::SIDEBAR_PRIMARY_FOREGROUND
        } else if is_local {
            ui_theme::SIDEBAR_ACCENT_FOREGROUND
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        let name_weight = if is_current {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        };

        let name_el = div()
            .flex_1()
            .min_w(px(120.0))
            .text_size(px(12.0))
            .font_weight(name_weight)
            .text_color(rgb(name_color))
            .truncate()
            .child(branch.name.clone());

        // upstream 改为 hover tooltip，不再内联显示，避免遮挡分支名
        let row = div()
            .id(format!("branch-{}", branch.name))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(2.0))
            .pl(px(22.0))
            .bg(rgb(row_bg))
            .hover(move |this| {
                if is_current {
                    this.bg(rgb(ui_theme::SIDEBAR_ACCENT))
                } else {
                    this.bg(rgb(ui_theme::ACCENT))
                }
            })
            .cursor_pointer()
            .child(leading)
            .child(name_el)
            .when(ahead > 0, |this| this.child(sync_badge("↑", ahead)))
            .when(behind > 0, |this| this.child(sync_badge("↓", behind)));

        // 本地分支悬停时显示 upstream 和同步数量，便于确认推送/拉取目标。
        let row = if let Some(up) = upstream.filter(|_| is_local) {
            let upstream_label = format!(
                "→{} · 待推送 {} · 待拉取 {}",
                strip_remote_prefix(&up),
                ahead,
                behind
            );
            row.tooltip(move |_window, cx| tooltip_text(upstream_label.clone(), cx))
        } else {
            row
        };

        row.on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.selected_branch = Some(name.clone());
            this.branch_context_menu = None;
            this.change_context_menu = None;
            this.commit_context_menu = None;
            this.encoding_menu_target = None;
            if event.standard_click() && event.click_count() >= 2 && !this.busy {
                match click_kind {
                    BranchKind::Local if !click_is_head => this.checkout(click_name.clone()),
                    BranchKind::Remote if !this.has_local_branch_for_remote(&click_name) => {
                        this.checkout_remote_branch(click_name.clone())
                    }
                    _ => {}
                }
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.selected_branch = Some(right_click_name.clone());
                this.active_dialog = None;
                let (x, y) =
                    clamped_menu_position(event, window, BRANCH_MENU_WIDTH, BRANCH_MENU_HEIGHT);
                this.branch_context_menu = Some(BranchContextMenu {
                    branch: right_click_name.clone(),
                    kind: right_click_kind.clone(),
                    is_head: right_click_is_head,
                    x,
                    y,
                });
                this.tag_context_menu = None;
                this.stash_context_menu = None;
                this.change_context_menu = None;
                this.commit_context_menu = None;
                this.encoding_menu_target = None;
                cx.notify();
            }),
        )
    }

    pub(crate) fn render_branch_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.branch_context_menu.clone() else {
            return div().into_any_element();
        };
        let is_local = menu.kind == BranchKind::Local;
        let can_pull_local = is_local
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.branches.iter().any(|branch| {
                    branch.kind == BranchKind::Local
                        && branch.name == menu.branch
                        && branch.upstream.is_some()
                })
            });

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(BRANCH_MENU_WIDTH))
            .when(!is_local, |this| {
                let branch = menu.branch.clone();
                this.child(context_menu_item_with_context(
                    "复制名称",
                    !self.busy,
                    {
                        let branch = branch.clone();
                        move |this, cx| this.copy_branch_name(branch.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item_with_context(
                    "复制 checkout 命令",
                    !self.busy,
                    {
                        let branch = branch.clone();
                        move |this, cx| this.copy_remote_checkout_command(branch.clone(), cx)
                    },
                    cx,
                ))
                .child(menu_separator())
            })
            .child(context_menu_item(
                "切换到此分支",
                is_local && !menu.is_head && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.checkout(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "拉取此分支更新",
                can_pull_local && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.pull_local_branch_update(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "合并到当前分支",
                !menu.is_head && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.merge_branch(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "变基到当前分支",
                !menu.is_head && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.rebase_branch(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "拉取到本地并切换",
                !is_local && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.checkout_remote_branch(branch.clone())
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                "设置/修改 upstream...",
                is_local && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.open_set_branch_upstream_dialog(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "重命名...",
                is_local && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.open_rename_branch_dialog(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除分支",
                is_local && !menu.is_head && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.delete_branch(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除远端分支",
                !is_local && !self.busy,
                {
                    let branch = menu.branch.clone();
                    move |this| this.open_delete_remote_branch_confirm(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "浏览此分支",
                !self.busy,
                {
                    let branch = menu.branch.clone();
                    let kind = menu.kind.clone();
                    move |this| this.open_browse_branch(branch.clone(), kind.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "与当前分支比较",
                !menu.is_head && !self.busy,
                {
                    let branch = menu.branch.clone();
                    let kind = menu.kind.clone();
                    move |this| this.open_compare_branch(branch.clone(), kind.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    fn pull_local_branch_update(&mut self, branch: String) {
        self.branch_context_menu = None;
        self.with_repo_blocking("分支拉取完成", move |service, repo| {
            service.pull_local_branch(repo, &BranchName::new(branch))
        });
    }
}

#[cfg(test)]
#[path = "tests/sidebar_view.rs"]
mod tests;
