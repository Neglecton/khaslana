use std::sync::Arc;

use crate::ui::theme::rgb;
use gpui::{
    ClickEvent, Context, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px, uniform_list,
};
use khaslana::{BranchInfo, BranchKind, BranchName, RemoteInfo, StashInfo, TagInfo};

use crate::{
    BRANCH_MENU_HEIGHT, BRANCH_MENU_WIDTH, BranchContextMenu, FieldId, REMOTE_MENU_HEIGHT,
    REMOTE_MENU_WIDTH, RemoteContextMenu, RepositoryView, STASH_MENU_HEIGHT, STASH_MENU_WIDTH,
    ScrollbarMode, SidebarSection, SidebarSectionState, StashContextMenu, TAG_MENU_HEIGHT,
    TAG_MENU_WIDTH, TagContextMenu, clamped_menu_position, context_menu_item,
    context_menu_item_with_context, menu_separator, placeholder_row, scrollable_uniform_frame,
    ui::{
        components::{glass_menu, sync_badge, tooltip_text},
        icons::{ToolbarIcon, toolbar_icon, toolbar_icon_rotated},
        theme as ui_theme,
    },
};

#[cfg(test)]
fn sidebar_branch_matches_query(branch: &BranchInfo, query: &str) -> bool {
    sidebar_branch_matches_normalized_query(branch, &query.trim().to_lowercase())
}

/// Git 引用名通常为 ASCII；常见路径用字节窗口做无分配匹配，只有非 ASCII
/// 名称才回退到 Unicode 小写化，避免大分支列表每次过滤产生逐项临时 String。
fn sidebar_text_contains_normalized_query(value: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if value.is_ascii() && query.is_ascii() {
        value
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()))
    } else {
        value.to_lowercase().contains(query)
    }
}

/// 分支名/upstream 与已归一化查询词的匹配（提交图谱页分支下拉复用同一规则）。
pub(crate) fn sidebar_branch_matches_normalized_query(branch: &BranchInfo, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    sidebar_text_contains_normalized_query(&branch.name, query)
        || branch
            .upstream
            .as_deref()
            .is_some_and(|upstream| sidebar_text_contains_normalized_query(upstream, query))
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
const SIDEBAR_TAG_CREATE_ID: &str = "sidebar-tag-create";
// uniform_list 要求各模型项等高；搜索框保持 28px 最小高度，并把上下内边距
// 收紧到各 4px，使筛选输入框完整落入 36px 高密度导航槽位。
const SIDEBAR_BRANCH_FILTER_INPUT_MIN_HEIGHT: f32 = 28.0;
const SIDEBAR_BRANCH_FILTER_VERTICAL_PADDING: f32 = 4.0;
const SIDEBAR_NAV_ITEM_HEIGHT: f32 =
    SIDEBAR_BRANCH_FILTER_INPUT_MIN_HEIGHT + SIDEBAR_BRANCH_FILTER_VERTICAL_PADDING * 2.0;

/// Context Navigator 的行背景、悬停和命中区域统一占满虚拟列表槽位宽度，
/// 不随分支名、远端名等内容长度收缩。
fn sidebar_full_width_row() -> gpui::Div {
    div().w_full()
}

/// 钉在滚动区外的固定行（分组标题/搜索框），与列表条目共用 36px 槽位保持视觉节奏。
fn sidebar_pinned_row(element: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .flex()
        .flex_none()
        .w_full()
        .h(px(SIDEBAR_NAV_ITEM_HEIGHT))
        .min_h(px(SIDEBAR_NAV_ITEM_HEIGHT))
        .child(element)
        .into_any_element()
}

/// 分组搜索框打开时返回其输入字段；标题与搜索框都钉在分组列表之外。
fn sidebar_section_search_field(
    section: SidebarSection,
    local_open: bool,
    remote_open: bool,
) -> Option<FieldId> {
    match section {
        SidebarSection::LocalBranches if local_open => Some(FieldId::SidebarLocalBranchSearch),
        SidebarSection::RemoteBranches if remote_open => Some(FieldId::SidebarRemoteBranchSearch),
        _ => None,
    }
}

fn sidebar_branch_search_button_id(section: SidebarSection) -> &'static str {
    // 图标按钮不显示文字，必须用分组专属 id，避免本地/远端搜索入口点击命中冲突。
    match section {
        SidebarSection::LocalBranches => "sidebar-local-branch-search-toggle",
        SidebarSection::RemoteBranches => "sidebar-remote-branch-search-toggle",
        _ => "sidebar-branch-search-toggle",
    }
}

/// 统一导航器只按声明顺序排列 section；分组标题钉在滚动区外常驻，
/// 折叠只影响本 section 的条目列表是否渲染。
fn sidebar_section_is_visible(section: SidebarSection, has_stashes: bool) -> bool {
    !matches!(section, SidebarSection::Stashes) || has_stashes
}

/// 钉住标题布局中，分组只在可见且展开时渲染条目列表；标题本身常驻不滚动。
fn sidebar_section_should_render_rows(
    section: SidebarSection,
    state: SidebarSectionState,
    has_stashes: bool,
) -> bool {
    sidebar_section_is_visible(section, has_stashes) && state.is_expanded(section)
}

fn sidebar_remote_manage_enabled(repo_available: bool, busy: bool) -> bool {
    repo_available && !busy
}

fn sidebar_remote_manage_disabled_reason(repo_available: bool, busy: bool) -> Option<&'static str> {
    if sidebar_remote_manage_enabled(repo_available, busy) {
        None
    } else if busy {
        Some("当前操作进行中，请稍候")
    } else {
        Some("请先打开仓库")
    }
}

/// 分组条目列表的轻量可见项。这里故意只保存快照数组下标和静态分组信息，不能保存
/// `AnyElement`：`uniform_list` 会在可视范围内才把该模型转换成实际行元素。
/// 分组标题与搜索框钉在各分组列表之外，不属于任何滚动模型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarNavItem {
    Branch(usize),
    Remote(usize),
    Tag(usize),
    Stash(usize),
    EmptyLocalBranches,
    EmptyRemoteBranches,
    LoadingRemotes,
    LoadingRemoteBranches,
}

/// 每个分组独立滚动区的固定 id；`uniform_scroll_handle` 按 tab 键控，
/// 切换仓库后各分组滚动位置自然保留。
pub(crate) fn sidebar_section_scroll_id(section: SidebarSection) -> &'static str {
    match section {
        SidebarSection::LocalBranches => "sidebar-section-local-branches",
        SidebarSection::Remotes => "sidebar-section-remotes",
        SidebarSection::RemoteBranches => "sidebar-section-remote-branches",
        SidebarSection::Tags => "sidebar-section-tags",
        SidebarSection::Stashes => "sidebar-section-stashes",
    }
}

/// 分组列表高度策略：条目少的分组按内容定高（≤上限行数），条目多的分组平分
/// 剩余空间。定高盒允许被压缩，空间不足时内部列表自行滚动兜底，
/// 保证分组标题永不被推出视口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarSectionHeight {
    Fill,
    Content(usize),
}

const SIDEBAR_SECTION_CONTENT_ROW_LIMIT: usize = 8;

fn sidebar_section_height(entry_count: usize) -> SidebarSectionHeight {
    if entry_count > SIDEBAR_SECTION_CONTENT_ROW_LIMIT {
        SidebarSectionHeight::Fill
    } else {
        SidebarSectionHeight::Content(entry_count)
    }
}

/// 分支分组条目的公共过滤：只产生匹配分支的快照下标，不复制 `BranchInfo`。
fn sidebar_branch_indices(branches: &[BranchInfo], kind: BranchKind, query: &str) -> Vec<usize> {
    let normalized_query = query.trim().to_lowercase();
    branches
        .iter()
        .enumerate()
        .filter(|(_, branch)| branch.kind == kind)
        .filter(|(_, branch)| sidebar_branch_matches_normalized_query(branch, &normalized_query))
        .map(|(index, _)| index)
        .collect()
}

/// 本地分支分组条目；搜索激活且无命中时返回单个占位项。
pub(crate) fn sidebar_local_branch_entries(
    branches: &[BranchInfo],
    query: &str,
) -> Vec<SidebarNavItem> {
    let mut items: Vec<SidebarNavItem> = sidebar_branch_indices(branches, BranchKind::Local, query)
        .into_iter()
        .map(SidebarNavItem::Branch)
        .collect();
    if items.is_empty() && !query.trim().is_empty() {
        items.push(SidebarNavItem::EmptyLocalBranches);
    }
    items
}

/// 远端分支分组条目；远端列表加载中且尚无任何远端分支时显示加载占位。
pub(crate) fn sidebar_remote_branch_entries(
    branches: &[BranchInfo],
    query: &str,
    remote_loading: bool,
) -> Vec<SidebarNavItem> {
    if remote_loading
        && branches
            .iter()
            .all(|branch| branch.kind != BranchKind::Remote)
    {
        return vec![SidebarNavItem::LoadingRemoteBranches];
    }
    let mut items: Vec<SidebarNavItem> =
        sidebar_branch_indices(branches, BranchKind::Remote, query)
            .into_iter()
            .map(SidebarNavItem::Branch)
            .collect();
    if items.is_empty() && !query.trim().is_empty() {
        items.push(SidebarNavItem::EmptyRemoteBranches);
    }
    items
}

/// 远端分组条目；刷新远端期间显示加载占位。
pub(crate) fn sidebar_remote_entries(
    remote_count: usize,
    remote_loading: bool,
) -> Vec<SidebarNavItem> {
    if remote_loading {
        vec![SidebarNavItem::LoadingRemotes]
    } else {
        (0..remote_count).map(SidebarNavItem::Remote).collect()
    }
}

pub(crate) fn sidebar_tag_entries(tag_count: usize) -> Vec<SidebarNavItem> {
    (0..tag_count).map(SidebarNavItem::Tag).collect()
}

pub(crate) fn sidebar_stash_entries(stash_count: usize) -> Vec<SidebarNavItem> {
    (0..stash_count).map(SidebarNavItem::Stash).collect()
}

impl RepositoryView {
    pub(crate) fn render_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.snapshot.as_ref();
        // 引用快照分支列表，不做整表 clone：大仓库远端分支上千条，
        // 每帧深拷贝再过滤会放大任何触发重绘的操作。
        let branches = snapshot
            .map(|snapshot| snapshot.branches.as_slice())
            .unwrap_or(&[]);
        let local_query = if self.sidebar_local_branch_search_open {
            self.sidebar_local_branch_search.value.trim()
        } else {
            ""
        };
        let remote_branch_query = if self.sidebar_remote_branch_search_open {
            self.sidebar_remote_branch_search.value.trim()
        } else {
            ""
        };
        let (remote_count, tag_count, stash_count) = snapshot
            .map(|snapshot| {
                (
                    snapshot.remotes.len(),
                    snapshot.tags.len(),
                    snapshot.stashes.len(),
                )
            })
            .unwrap_or_default();
        let remote_loading = self.loading.remote();
        let has_stashes = stash_count > 0;

        // 分组标题钉在滚动区外常驻：条目再多也只滚动条目列表本身，
        // 后续分组标题依次固定显示在区域下方。每个展开分组各自一个虚拟列表
        //（索引模型，可视回调才建行），20,000 条远端分支不会预建 20,000 行。
        let sections = [
            SidebarSection::LocalBranches,
            SidebarSection::Remotes,
            SidebarSection::RemoteBranches,
            SidebarSection::Tags,
            SidebarSection::Stashes,
        ];
        let mut children: Vec<gpui::AnyElement> = Vec::new();
        for section in sections {
            if !sidebar_section_is_visible(section, has_stashes) {
                continue;
            }
            children.push(sidebar_pinned_row(
                self.nav_section_header(section, cx).into_any_element(),
            ));
            if !sidebar_section_should_render_rows(section, self.sidebar_sections, has_stashes) {
                continue;
            }
            if let Some(field) = sidebar_section_search_field(
                section,
                self.sidebar_local_branch_search_open,
                self.sidebar_remote_branch_search_open,
            ) {
                // 搜索框钉在标题下方，不随条目列表滚动。
                children.push(sidebar_pinned_row(
                    self.sidebar_branch_search_input(field, window, cx)
                        .into_any_element(),
                ));
            }
            let entries = Arc::new(match section {
                SidebarSection::LocalBranches => {
                    sidebar_local_branch_entries(branches, local_query)
                }
                SidebarSection::Remotes => sidebar_remote_entries(remote_count, remote_loading),
                SidebarSection::RemoteBranches => {
                    sidebar_remote_branch_entries(branches, remote_branch_query, remote_loading)
                }
                SidebarSection::Tags => sidebar_tag_entries(tag_count),
                SidebarSection::Stashes => sidebar_stash_entries(stash_count),
            });
            if entries.is_empty() {
                // 空分组（无过滤词）与折叠态视觉一致：只显示标题行。
                continue;
            }
            children.push(
                self.sidebar_section_list(section, entries, cx)
                    .into_any_element(),
            );
        }

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            // 纯鼠标区域：不再承载 R/B/T/S/M 字母快捷键与键盘焦点
            //（键盘白名单见 AGENTS.md §8；分组折叠均由鼠标点击完成）。
            .overflow_hidden()
            .bg(rgb(ui_theme::SURFACE_BASE))
            .pt(px(8.0))
            .pb(px(12.0))
            .children(children)
    }

    /// 单个分组的条目虚拟列表。条目少的分组按内容定高（Content），条目多的
    /// 分组平分剩余空间（Fill）；定高盒不加 flex_none，被压缩时列表内部
    /// 滚动兜底，分组标题永不被推出视口。
    fn sidebar_section_list(
        &self,
        section: SidebarSection,
        entries: Arc<Vec<SidebarNavItem>>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let scroll_id = sidebar_section_scroll_id(section);
        let scroll_handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = scroll_handle.clone();
        let item_count = entries.len();
        let entries_for_rows = Arc::clone(&entries);
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(
                uniform_list(
                    scroll_id,
                    item_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        range
                            .map(|index| {
                                entries_for_rows
                                    .get(index)
                                    .copied()
                                    .map(|item| {
                                        this.render_sidebar_navigation_item(item, window, cx)
                                    })
                                    .unwrap_or_else(|| placeholder_row("").into_any_element())
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .flex_1()
                .min_h(px(0.0)),
            );
        let frame = scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Vertical,
            content.into_any_element(),
            scroll_handle,
            !entries.is_empty(),
            cx,
        );
        match sidebar_section_height(item_count) {
            SidebarSectionHeight::Fill => frame.into_any_element(),
            SidebarSectionHeight::Content(rows) => div()
                .flex()
                .flex_col()
                .h(px(rows as f32 * SIDEBAR_NAV_ITEM_HEIGHT))
                .min_h(px(0.0))
                .child(frame)
                .into_any_element(),
        }
    }

    /// 只为 `uniform_list` 当前请求的可见 range 创建实际元素；分组标题与
    /// 搜索框钉在列表外，不经此分发。
    fn render_sidebar_navigation_item(
        &self,
        item: SidebarNavItem,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let element = match item {
            SidebarNavItem::Branch(index) => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.branches.get(index))
                .cloned()
                .map(|branch| self.branch_row(branch, cx).into_any_element())
                .unwrap_or_else(|| placeholder_row("").into_any_element()),
            SidebarNavItem::Remote(index) => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.remotes.get(index))
                .cloned()
                .map(|remote| self.remote_row(remote, cx).into_any_element())
                .unwrap_or_else(|| placeholder_row("").into_any_element()),
            SidebarNavItem::Tag(index) => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.tags.get(index))
                .cloned()
                .map(|tag| self.tag_row(tag, cx).into_any_element())
                .unwrap_or_else(|| placeholder_row("").into_any_element()),
            SidebarNavItem::Stash(index) => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.stashes.get(index))
                .cloned()
                .map(|stash| self.stash_row(stash, cx).into_any_element())
                .unwrap_or_else(|| placeholder_row("").into_any_element()),
            SidebarNavItem::EmptyLocalBranches => {
                placeholder_row("没有匹配的本地分支").into_any_element()
            }
            SidebarNavItem::EmptyRemoteBranches => {
                placeholder_row("没有匹配的远端分支").into_any_element()
            }
            SidebarNavItem::LoadingRemotes => placeholder_row("远端加载中...").into_any_element(),
            SidebarNavItem::LoadingRemoteBranches => {
                placeholder_row("远端分支加载中...").into_any_element()
            }
        };
        // uniform_list 以第一个 item 测量全表高度，所有导航元素必须共享固定槽位。
        div()
            .flex()
            .flex_none()
            .w_full()
            .h(px(SIDEBAR_NAV_ITEM_HEIGHT))
            .min_h(px(SIDEBAR_NAV_ITEM_HEIGHT))
            .child(element)
            .into_any_element()
    }

    fn sidebar_branch_search_input(
        &self,
        field: FieldId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        sidebar_full_width_row()
            .flex_none()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::SURFACE_BASE))
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
            ui_theme::CONTENT_SECONDARY
        };
        let on_click = Arc::new(on_click);
        div()
            .id(id)
            .flex_none()
            .size(px(20.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(if active {
                ui_theme::STATE_HOVER
            } else {
                ui_theme::SURFACE_BASE
            }))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.62))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.stop_propagation();
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
        section: SidebarSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 全部分组可折叠；本地分支是唯一默认展开的分组（SidebarSectionState::default）。
        let (title, is_collapsible) = match section {
            SidebarSection::LocalBranches => ("本地分支", true),
            SidebarSection::Remotes => ("远端", true),
            SidebarSection::RemoteBranches => ("远端分支", true),
            SidebarSection::Tags => ("标签", true),
            SidebarSection::Stashes => ("贮藏", true),
        };
        let expanded = self.sidebar_sections.is_expanded(section);
        let enabled = self.repo_path.is_some() && !self.busy;
        let title_el = div()
            .min_w(px(0.0))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
            .truncate()
            .child(title);
        let action = self.sidebar_section_action(section, enabled, cx);

        let toggle = if is_collapsible {
            let icon = if expanded {
                toolbar_icon_rotated(ToolbarIcon::ChevronRight, ui_theme::CONTENT_SECONDARY, 90.0)
                    .into_any_element()
            } else {
                toolbar_icon(ToolbarIcon::ChevronRight, ui_theme::CONTENT_SECONDARY)
                    .into_any_element()
            };
            // 使用 Context Navigator 已有的稳定焦点句柄，并在侧边栏容器的键盘路径中
            // 提供 Enter/Space 以及快捷键；不在 render 期间创建临时 FocusHandle。
            Some(
                div()
                    .id(format!("sidebar-section-toggle-{title}"))
                    .flex_none()
                    .child(icon)
                    .into_any_element(),
            )
        } else {
            None
        };

        sidebar_full_width_row()
            .id(format!("sidebar-section-header-{title}"))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(8.0))
            .when(is_collapsible, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_sidebar_section(section);
                        cx.stop_propagation();
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w(px(0.0))
                    .when_some(toggle, |this, toggle| this.child(toggle))
                    .child(title_el),
            )
            .when_some(action, |this, action| this.child(action))
    }

    fn sidebar_section_action(
        &self,
        section: SidebarSection,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        match section {
            SidebarSection::LocalBranches => Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.sidebar_create_branch_button(cx))
                    .child(self.sidebar_branch_search_button(
                        section,
                        self.sidebar_local_branch_search_open,
                        cx,
                    ))
                    .into_any_element(),
            ),
            SidebarSection::Remotes => {
                let enabled = sidebar_remote_manage_enabled(self.repo_path.is_some(), self.busy);
                let disabled_reason =
                    sidebar_remote_manage_disabled_reason(self.repo_path.is_some(), self.busy);
                Some(
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
                        .border_color(rgb(ui_theme::BORDER_MUTED))
                        .bg(rgb(ui_theme::SURFACE_BASE))
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(rgb(if enabled {
                            ui_theme::CONTENT_SECONDARY
                        } else {
                            ui_theme::CONTENT_TERTIARY
                        }))
                        .when(enabled, |this| {
                            this.cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                        })
                        .when(!enabled, |this| this.cursor_not_allowed().opacity(0.62))
                        .when_some(disabled_reason, |this, reason| {
                            this.tooltip(move |_window, cx| tooltip_text(reason, cx))
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            if enabled {
                                this.open_remote_manager();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child("管理")
                        .into_any_element(),
                )
            }
            SidebarSection::RemoteBranches => Some(self.sidebar_branch_search_button(
                section,
                self.sidebar_remote_branch_search_open,
                cx,
            )),
            SidebarSection::Tags => {
                let tag_count = self
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.tags.len());
                Some(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(self.sidebar_header_icon_button(
                            SIDEBAR_TAG_CREATE_ID,
                            ToolbarIcon::Plus,
                            false,
                            enabled,
                            |this, _, _| this.open_tag_form_dialog(None, String::new()),
                            cx,
                        ))
                        .when(tag_count > 0, |this| {
                            this.child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child(tag_count.to_string()),
                            )
                        })
                        .into_any_element(),
                )
            }
            SidebarSection::Stashes => self.snapshot.as_ref().and_then(|snapshot| {
                let count = snapshot.stashes.len();
                (count > 0).then(|| {
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(count.to_string())
                        .into_any_element()
                })
            }),
        }
    }

    fn remote_row(&self, remote: RemoteInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.current_remote().as_deref() == Some(remote.name.as_str());
        let name = remote.name.clone();
        let right_click_name = remote.name.clone();

        // 设计图：globe icon 14px + 名称 fontWeight 500, padding [6,16,6,24]
        let name_color = if selected {
            ui_theme::PRIMARY
        } else {
            ui_theme::CONTENT_PRIMARY
        };

        sidebar_full_width_row()
            .id(format!("remote-{}", remote.name))
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::SURFACE_BASE)
            })
            .when(selected, |this| {
                this.border_l_2().border_color(rgb(ui_theme::PRIMARY))
            })
            .hover(move |this| {
                if selected {
                    this.bg(rgb(ui_theme::PRIMARY_SUBTLE))
                } else {
                    this.bg(rgb(ui_theme::STATE_HOVER))
                }
            })
            .cursor_pointer()
            // globe icon 14px, $--sidebar-foreground 色
            .child(toolbar_icon(
                ToolbarIcon::Globe,
                ui_theme::CONTENT_SECONDARY,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(name_color))
                    .overflow_hidden()
                    .whitespace_nowrap()
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
        sidebar_full_width_row()
            .id(format!("tag-{}", tag.name))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .overflow_hidden()
                    .whitespace_nowrap()
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
        // 左键条目直接查看贮藏（右键菜单的「查看贮藏」保留同一路径）。
        let click_index = stash.index;
        let label = format!("stash@{{{}}} {}", stash.index, stash.message);

        // 设计图：与分支行一致的样式
        sidebar_full_width_row()
            .id(format!("stash-{}", stash.index))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .pl(px(24.0))
            .py(px(4.0))
            .bg(rgb(ui_theme::SURFACE_BASE))
            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.busy {
                    this.view_stash(click_index);
                }
                cx.notify();
            }))
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
            ui_theme::PRIMARY_SUBTLE
        } else if selected {
            ui_theme::PRIMARY_SUBTLE
        } else {
            ui_theme::SURFACE_BASE
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
            ui_theme::CONTENT_PRIMARY
        } else if is_local {
            ui_theme::CONTENT_PRIMARY
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
            // 各分组列表的第 0 项是数据行：uniform_list 以 MinContent 测量第 0 项
            // 宽度，truncate 的省略号会被坍缩宽度固化，必须用硬裁剪。
            .overflow_hidden()
            .whitespace_nowrap()
            .child(branch.name.clone());

        // upstream 改为 hover tooltip，不再内联显示，避免遮挡分支名
        let row = sidebar_full_width_row()
            .id(format!("branch-{}", branch.name))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(2.0))
            .pl(px(22.0))
            .bg(rgb(row_bg))
            .when(selected, |this| {
                this.border_l_2().border_color(rgb(ui_theme::PRIMARY))
            })
            .hover(move |this| {
                if is_current {
                    this.bg(rgb(ui_theme::PRIMARY_SUBTLE))
                } else {
                    this.bg(rgb(ui_theme::STATE_HOVER))
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
        let merge_in_progress = self.merge_in_progress();
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
                is_local && !menu.is_head && !self.busy && !merge_in_progress,
                {
                    let branch = menu.branch.clone();
                    move |this| this.checkout(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "拉取此分支更新",
                can_pull_local && !self.busy && !merge_in_progress,
                {
                    let branch = menu.branch.clone();
                    move |this| this.pull_local_branch_update(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "合并到当前分支",
                !menu.is_head && !self.busy && !merge_in_progress,
                {
                    let branch = menu.branch.clone();
                    move |this| this.merge_branch(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "变基到当前分支",
                !menu.is_head && !self.busy && !merge_in_progress,
                {
                    let branch = menu.branch.clone();
                    move |this| this.rebase_branch(branch.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "拉取到本地并切换",
                !is_local && !self.busy && !merge_in_progress,
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
        if !self.ensure_no_merge_in_progress("拉取分支") {
            return;
        }
        self.branch_context_menu = None;
        self.with_repo_blocking("分支拉取完成", move |service, repo| {
            service.pull_local_branch(repo, &BranchName::new(branch))
        });
    }
}

#[cfg(test)]
#[path = "tests/sidebar_view.rs"]
mod tests;
