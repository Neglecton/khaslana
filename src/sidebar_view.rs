use gpui::{
    ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, Window, div, linear_color_stop,
    linear_gradient, prelude::*, px, rgb, rgba,
};
use khaslana::{BranchInfo, BranchKind, RemoteInfo, StashInfo, TagInfo};

use crate::{
    BRANCH_MENU_HEIGHT, BRANCH_MENU_WIDTH, BranchContextMenu, FieldId, NAV_ROW_HEIGHT,
    REMOTE_MENU_HEIGHT, REMOTE_MENU_WIDTH, RemoteContextMenu, RepositoryView, STASH_MENU_HEIGHT,
    STASH_MENU_WIDTH, SidebarSection, StashContextMenu, TAG_MENU_HEIGHT, TAG_MENU_WIDTH,
    TagContextMenu, clamped_menu_position, context_menu_item, context_menu_item_with_context,
    menu_separator, nav_list, nav_row, placeholder_row,
    ui::{
        components::glass_menu,
        icons::{ToolbarIcon, toolbar_icon},
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

        let mut sidebar = div()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.sidebar_width))
            .min_w(px(self.sidebar_width))
            .h_full()
            .border_r_1()
            .border_color(rgba(ui_theme::GLASS_BORDER))
            // 使用 GPUI 原生线性渐变，避免上下色块模拟造成明显分界线。
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(ui_theme::SIDEBAR_GRADIENT_TOP), 0.0),
                linear_color_stop(rgba(ui_theme::SIDEBAR_GRADIENT_BOTTOM), 1.0),
            ))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .bg(rgba(ui_theme::SIDEBAR_GRADIENT_SOFTEN)),
            )
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
            .child(
                self.render_nav_section(
                    "远端",
                    "remote-list",
                    SidebarSection::Remotes,
                    remote_rows,
                    self.loading.remote().then_some("远端加载中..."),
                    None,
                    2.0,
                    Some(
                        self.button(
                            "管理",
                            self.repo_path.is_some() && !self.busy,
                            |this, _, _| this.open_remote_manager(),
                            cx,
                        )
                        .into_any_element(),
                    ),
                    cx,
                ),
            )
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
            sidebar = sidebar.child(self.render_nav_section(
                "标签",
                "tag-list",
                SidebarSection::Tags,
                tag_rows,
                None,
                None,
                2.0,
                None,
                cx,
            ));
        }
        if !stash_rows.is_empty() {
            sidebar = sidebar.child(self.render_nav_section(
                "贮藏",
                "stash-list",
                SidebarSection::Stashes,
                stash_rows,
                None,
                None,
                2.0,
                None,
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
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
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
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgba(ui_theme::GLASS_BG))
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
        let icon_color = if !enabled {
            ui_theme::TEXT_FAINT
        } else if active {
            ui_theme::ACCENT_STRONG
        } else {
            ui_theme::TEXT_MUTED
        };
        div()
            .id(id)
            .flex_none()
            .size(px(24.0))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(rgb(if active {
                ui_theme::ROW_SELECTED_BORDER
            } else {
                ui_theme::BORDER
            }))
            .bg(rgb(if active {
                ui_theme::ACCENT_SOFT
            } else {
                ui_theme::SURFACE
            }))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::ROW_HOVER)))
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
        let toggle_label = if expanded { "∧" } else { "∨" };
        let toggle = div()
            .id(format!("sidebar-section-toggle-{title}"))
            .flex_none()
            .size(px(24.0))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::SURFACE))
            .text_size(px(13.0))
            .text_color(rgb(ui_theme::TEXT_MUTED))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::ROW_HOVER)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_sidebar_section(section);
                cx.notify();
            }))
            .child(toggle_label)
            .into_any_element();

        let actions = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .when_some(action, |this, action| this.child(action))
            .child(toggle);

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
            .bg(rgb(ui_theme::HEADER_BG))
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::TEXT))
                    .truncate()
                    .child(title),
            )
            .child(actions)
    }

    fn remote_row(&self, remote: RemoteInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.current_remote().as_deref() == Some(remote.name.as_str());
        let name = remote.name.clone();
        let right_click_name = remote.name.clone();

        nav_row(format!("remote-{}", remote.name), false, selected)
            .hover(move |this| {
                if selected {
                    this.bg(rgb(ui_theme::ACCENT_SOFT))
                } else {
                    this.bg(rgb(ui_theme::ROW_HOVER))
                }
            })
            .child(
                div()
                    .flex_none()
                    .w(px(3.0))
                    .h(px(18.0))
                    .rounded_sm()
                    .bg(if selected {
                        rgb(ui_theme::ACCENT)
                    } else {
                        rgb(ui_theme::BORDER)
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(if selected {
                        rgb(ui_theme::ACCENT_STRONG)
                    } else {
                        rgb(ui_theme::TEXT)
                    })
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

        nav_row(format!("tag-{}", tag.name), false, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::TEXT_MUTED))
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

        nav_row(format!("stash-{}", stash.index), false, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::TEXT_MUTED))
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
        let marker = if branch.is_head { "* " } else { "" };
        let selected = self.selected_branch.as_deref() == Some(&branch.name);
        let label = match branch.kind {
            BranchKind::Local => format!("{marker}{}", branch.name),
            BranchKind::Remote => format!("  {}", branch.name),
        };
        let row_bg = if is_current {
            ui_theme::ROW_SELECTED
        } else if selected {
            ui_theme::ROW_SELECTED
        } else {
            ui_theme::SURFACE
        };
        let row_border = if is_current {
            ui_theme::ROW_SELECTED_BORDER
        } else if selected {
            ui_theme::ROW_SELECTED_BORDER
        } else {
            ui_theme::BORDER
        };
        let marker_bg = if is_current {
            ui_theme::ACCENT
        } else if selected {
            ui_theme::ROW_SELECTED_BORDER
        } else {
            ui_theme::BORDER_MUTED
        };

        div()
            .id(format!("branch-{}", branch.name))
            .flex()
            .h(px(NAV_ROW_HEIGHT))
            .min_h(px(NAV_ROW_HEIGHT))
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(row_bg))
            .border_1()
            .border_color(rgb(row_border))
            .hover(move |this| {
                if is_current {
                    this.bg(rgb(ui_theme::ACCENT_SOFT))
                } else {
                    this.bg(rgb(ui_theme::ROW_HOVER))
                }
            })
            .child(
                div()
                    .flex_none()
                    .w(px(3.0))
                    .h(px(18.0))
                    .rounded_sm()
                    .bg(rgb(marker_bg)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(if is_current {
                        rgb(ui_theme::ACCENT_STRONG)
                    } else if is_local {
                        rgb(ui_theme::TEXT)
                    } else {
                        rgb(ui_theme::TEXT_MUTED)
                    })
                    .truncate()
                    .child(label),
            )
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
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
                    let kind = menu.kind;
                    move |this| this.open_browse_branch(branch.clone(), kind.clone())
                },
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, kind: BranchKind, upstream: Option<&str>) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            kind,
            is_head: false,
            upstream: upstream.map(str::to_string),
        }
    }

    fn branch_names(branches: Vec<BranchInfo>) -> Vec<String> {
        branches.into_iter().map(|branch| branch.name).collect()
    }

    #[test]
    fn sidebar_branch_search_empty_query_returns_only_requested_kind() {
        let branches = vec![
            branch("main", BranchKind::Local, None),
            branch("feature/a", BranchKind::Local, None),
            branch("origin/main", BranchKind::Remote, None),
        ];

        assert_eq!(
            branch_names(filter_sidebar_branches(&branches, BranchKind::Local, "")),
            vec!["main", "feature/a"]
        );
        assert_eq!(
            branch_names(filter_sidebar_branches(&branches, BranchKind::Remote, "")),
            vec!["origin/main"]
        );
    }

    #[test]
    fn sidebar_branch_search_is_case_insensitive() {
        let branches = vec![
            branch("Feature/Login", BranchKind::Local, None),
            branch("bugfix/logout", BranchKind::Local, None),
        ];

        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Local,
                "feature",
            )),
            vec!["Feature/Login"]
        );
    }

    #[test]
    fn sidebar_branch_search_keeps_local_and_remote_groups_separate() {
        let branches = vec![
            branch("feature/a", BranchKind::Local, None),
            branch("origin/feature/a", BranchKind::Remote, None),
        ];

        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Local,
                "feature",
            )),
            vec!["feature/a"]
        );
        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Remote,
                "feature",
            )),
            vec!["origin/feature/a"]
        );
    }

    #[test]
    fn sidebar_remote_branch_search_matches_full_or_partial_name() {
        let branches = vec![
            branch("origin/feature/a", BranchKind::Remote, None),
            branch("upstream/release", BranchKind::Remote, None),
        ];

        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Remote,
                "origin/feature",
            )),
            vec!["origin/feature/a"]
        );
        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Remote,
                "release",
            )),
            vec!["upstream/release"]
        );
    }

    #[test]
    fn sidebar_branch_action_button_ids_keep_actions_distinct() {
        assert_ne!(
            sidebar_branch_search_button_id(SidebarSection::LocalBranches),
            sidebar_branch_search_button_id(SidebarSection::RemoteBranches),
        );
        assert_ne!(
            SIDEBAR_LOCAL_BRANCH_CREATE_ID,
            sidebar_branch_search_button_id(SidebarSection::LocalBranches),
        );
    }

    #[test]
    fn sidebar_local_branch_search_matches_upstream() {
        let branches = vec![
            branch("main", BranchKind::Local, Some("origin/trunk")),
            branch("feature/a", BranchKind::Local, Some("origin/feature/a")),
        ];

        assert_eq!(
            branch_names(filter_sidebar_branches(
                &branches,
                BranchKind::Local,
                "origin/trunk",
            )),
            vec!["main"]
        );
    }
}
