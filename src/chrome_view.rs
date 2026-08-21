//! Focus Workbench 的应用壳层。
//!
//! 该模块只组织 titlebar 与 Context Navigator（模式按钮 + 仓库引用分组），
//! 不触碰各主页面内部布局。每次渲染都从 GPUI `Window::viewport_size()` 读取当前视口
//! 宽度，再交给纯函数决定壳层的信息密度；窗口创建时另有 `WindowOptions::window_min_size`
//! 保护原生控制区。

use std::sync::Arc;

use gpui::{
    Context, CursorStyle, FocusHandle, IntoElement, KeyDownEvent, MouseButton, Stateful, Window,
    WindowControlArea, div, prelude::*, px,
};

use crate::{
    MainMode, RemoteBranchOperationKind, RepositoryView, WINDOW_CONTROLS_WIDTH,
    ui::{
        components::{command_group, focusable_icon_button, tooltip_text},
        icons::{ToolbarIcon, toolbar_icon, toolbar_icon_with_size},
        theme::{self, rgb},
    },
};

/// 原生控制区固定在标题栏最右侧，窗口不能缩到把它们挤出视口。
pub(crate) const MIN_WINDOW_WIDTH: f32 = 860.0;
pub(crate) const MIN_WINDOW_HEIGHT: f32 = 520.0;
pub(crate) const STATUS_BAR_HEIGHT: f32 = 24.0;
pub(crate) const NARROW_LAYOUT_WIDTH: f32 = 1120.0;
pub(crate) const COMFORTABLE_LAYOUT_WIDTH: f32 = 1440.0;

/// 根壳中间区使用确定高度，避免页面最小高度把状态栏和导航器底部推出视口。
pub(crate) fn shell_content_height(viewport_height: f32) -> f32 {
    (viewport_height - theme::TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT).max(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutBand {
    Narrow,
    Standard,
    Comfortable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShellLayoutPolicy {
    pub(crate) band: LayoutBand,
    pub(crate) show_context_navigator: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextNavigatorPresentation {
    Hidden,
    Docked,
    Overlay,
}

/// 宽度策略只描述信息优先级：窄窗口覆盖 Context Navigator，
/// 绝不挤压 titlebar 的原生命中区域；「贮藏」「子模块」常驻内联，不再收纳进 overflow。
pub(crate) const fn shell_layout_policy(width: f32) -> ShellLayoutPolicy {
    if width < NARROW_LAYOUT_WIDTH {
        ShellLayoutPolicy {
            band: LayoutBand::Narrow,
            show_context_navigator: false,
        }
    } else if width < COMFORTABLE_LAYOUT_WIDTH {
        ShellLayoutPolicy {
            band: LayoutBand::Standard,
            show_context_navigator: true,
        }
    } else {
        ShellLayoutPolicy {
            band: LayoutBand::Comfortable,
            show_context_navigator: true,
        }
    }
}

/// 为壳层命令提供可操作的禁用说明，避免 disabled 控件只有灰色视觉却没有原因。
fn chrome_action_disabled_reason(
    label: &'static str,
    repo_open: bool,
    remote_open: bool,
    busy: bool,
    merge_in_progress: bool,
) -> Option<&'static str> {
    if busy {
        return Some("当前操作进行中，请稍候");
    }
    if label == "设置" {
        return None;
    }
    if !repo_open {
        return Some("请先打开仓库");
    }

    if matches!(label, "获取" | "拉取" | "推送") && !remote_open {
        return Some("当前仓库没有可用远端");
    }
    if matches!(label, "拉取" | "推送") && merge_in_progress {
        return Some("合并进行中，完成或中止合并后再操作");
    }
    None
}

/// 只有主工作台页面承载仓库上下文；专用模式保留完整画布，不显示无意义的展开入口。
pub(crate) const fn context_navigator_supported_mode(mode: MainMode) -> bool {
    matches!(
        mode,
        MainMode::Worktree | MainMode::History | MainMode::Workflow
    )
}

/// Navigator 的呈现由真实窗口宽度、模式偏好和窄窗临时覆盖态共同决定。
/// `Hidden` 表示收起为窄条（展开箭头 + 模式图标），并非消失。
pub(crate) const fn context_navigator_presentation(
    policy: ShellLayoutPolicy,
    mode: MainMode,
    dock_requested: bool,
    overlay_requested: bool,
) -> ContextNavigatorPresentation {
    if !context_navigator_supported_mode(mode) {
        ContextNavigatorPresentation::Hidden
    } else if policy.show_context_navigator && dock_requested {
        ContextNavigatorPresentation::Docked
    } else if !policy.show_context_navigator && overlay_requested {
        ContextNavigatorPresentation::Overlay
    } else {
        ContextNavigatorPresentation::Hidden
    }
}

impl RepositoryView {
    pub(crate) fn shell_layout_policy(&self, window: &Window) -> ShellLayoutPolicy {
        shell_layout_policy(window.viewport_size().width.into())
    }

    pub(crate) fn context_navigator_presentation(
        &self,
        window: &Window,
    ) -> ContextNavigatorPresentation {
        context_navigator_presentation(
            self.shell_layout_policy(window),
            self.main_mode,
            self.context_navigator_preferences
                .is_visible(self.main_mode),
            self.context_navigator_overlay_open,
        )
    }

    pub(crate) fn render_chrome_titlebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let policy = self.shell_layout_policy(window);
        let repo_open = self.repo_path.is_some();
        let remote_open = !self.loading.remote() && self.current_remote().is_some();
        let merge_in_progress = self.merge_in_progress();
        let behind_count = self
            .branch_sync_status
            .as_ref()
            .map(|status| status.behind)
            .unwrap_or(0);
        let ahead_count = self
            .branch_sync_status
            .as_ref()
            .map(|status| status.ahead)
            .unwrap_or(0);
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_deref())
            .unwrap_or("未打开仓库")
            .to_string();
        let branch_tooltip = branch.clone();

        // 左侧内容可收缩，overflow 与原生控制区保持固定宽度并留在正常 flex 流中，
        // 避免 GPUI 绝对定位子元素在自绘标题栏中丢失绘制或命中区域。
        let left_content = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .items_center()
            .child(self.render_chrome_brand())
            .child(self.render_repo_switcher_button(cx))
            .when(policy.band != LayoutBand::Narrow, |this| {
                this.child(
                    div()
                        .flex()
                        // 可收缩：空间紧张时分支名先于拖拽区收窄截断，短名只占实际内容宽。
                        .min_w(px(0.0))
                        .items_center()
                        .gap(px(theme::SPACE_1))
                        .ml(px(theme::SPACE_2))
                        .child(
                            div()
                                // 240px 上限在标准档（≥1120px）最坏布局下仍放得下；
                                // 超长分支名截断后悬浮显示完整名称。
                                .max_w(px(240.0))
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE_1))
                                .px(px(theme::SPACE_2))
                                .text_size(px(theme::TYPE_BODY))
                                .text_color(rgb(theme::CONTENT_SECONDARY))
                                .child(toolbar_icon(
                                    ToolbarIcon::ChevronRight,
                                    theme::CONTENT_TERTIARY,
                                ))
                                .child(
                                    div()
                                        .id("titlebar-branch-name")
                                        .min_w(px(0.0))
                                        .truncate()
                                        .tooltip(move |_window, cx| {
                                            tooltip_text(branch_tooltip.clone(), cx)
                                        })
                                        .child(branch),
                                ),
                        ),
                )
            })
            .child(
                command_group()
                    .ml(px(theme::SPACE_2))
                    .child(self.chrome_command_button(
                        "刷新",
                        ToolbarIcon::Refresh,
                        None,
                        repo_open && !self.busy,
                        &self.chrome_refresh_focus,
                        |this, _window, _cx| this.refresh(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "获取",
                        ToolbarIcon::Fetch,
                        None,
                        repo_open && remote_open && !self.busy,
                        &self.chrome_fetch_focus,
                        |this, _window, _cx| this.fetch(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "拉取",
                        ToolbarIcon::Pull,
                        (behind_count > 0).then(|| format!("↓{behind_count}")),
                        repo_open && remote_open && !self.busy && !merge_in_progress,
                        &self.chrome_pull_focus,
                        |this, _window, _cx| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Pull)
                        },
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "推送",
                        ToolbarIcon::Push,
                        (ahead_count > 0).then(|| format!("↑{ahead_count}")),
                        repo_open && remote_open && !self.busy && !merge_in_progress,
                        &self.chrome_push_focus,
                        |this, _window, _cx| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Push)
                        },
                        cx,
                    ))
                    // 「贮藏」「子模块」常驻内联：壳层不再做响应式收纳，任何窗口宽度都可直达。
                    .child(self.chrome_command_button(
                        "贮藏",
                        ToolbarIcon::Stash,
                        None,
                        repo_open && !self.busy && !merge_in_progress,
                        &self.chrome_stash_focus,
                        |this, _window, _cx| this.open_stash_dialog(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "子模块",
                        ToolbarIcon::Submodule,
                        None,
                        repo_open && !self.busy,
                        &self.chrome_submodule_focus,
                        |this, _window, _cx| this.open_submodule_manager(),
                        cx,
                    )),
            )
            .child(self.render_chrome_drag_area());

        div()
            .id("focus-workbench-titlebar")
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .h(px(theme::TITLEBAR_HEIGHT))
            .pl(px(theme::SPACE_3))
            .border_b_1()
            .border_color(rgb(theme::BORDER_MUTED))
            .bg(rgb(theme::TITLEBAR_SURFACE))
            .child(left_content)
            // 「设置」固定在原生窗口控制区左侧：任何窗口宽度都紧邻最小化按钮，
            // 不被中间拖拽区挤走（设置中心唯一常驻入口，快捷键 Ctrl+, 等效）。
            .child(
                self.chrome_command_button(
                    "设置",
                    ToolbarIcon::Settings,
                    None,
                    true,
                    &self.chrome_settings_focus,
                    |this, window, _cx| {
                        this.open_settings_center();
                        window.focus(&this.settings_center_focus);
                    },
                    cx,
                )
                .mr(px(theme::SPACE_2)),
            )
            .child(self.render_chrome_window_controls(window))
    }

    /// Context Navigator 的模式按钮条目（收起窄条图标与展开态文字按钮共用同一来源，
    /// 保证两态顺序一致）。设置入口在 titlebar，不在此列；有冲突时在「工作区」后
    /// 追加「冲突处理」条件条目。
    fn navigator_mode_entries(
        &self,
    ) -> Vec<(&'static str, ToolbarIcon, &'static str, bool, MainMode)> {
        let has_conflicts = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.conflicts.is_empty());
        let mut entries = vec![(
            "nav-worktree",
            ToolbarIcon::Worktree,
            "工作区",
            self.main_mode == MainMode::Worktree,
            MainMode::Worktree,
        )];
        if has_conflicts {
            entries.push((
                "nav-conflict",
                ToolbarIcon::Ai,
                "冲突处理",
                self.main_mode == MainMode::Conflict,
                MainMode::Conflict,
            ));
        }
        entries.push((
            "nav-history",
            ToolbarIcon::History,
            "提交记录",
            self.main_mode == MainMode::History,
            MainMode::History,
        ));
        entries.push((
            "nav-workflow",
            ToolbarIcon::Workflow,
            "工作流",
            self.main_mode == MainMode::Workflow,
            MainMode::Workflow,
        ));
        entries
    }

    /// 模式按钮的稳定焦点句柄（收起窄条与展开态共用，保证 Tab 顺序稳定）。
    fn navigator_mode_focus_handle(&self, id: &str) -> FocusHandle {
        match id {
            "nav-worktree" => self.nav_worktree_focus.clone(),
            "nav-conflict" => self.nav_conflict_focus.clone(),
            "nav-history" => self.nav_history_focus.clone(),
            _ => self.nav_workflow_focus.clone(),
        }
    }

    /// Context Navigator 收起态窄条：全高 48px 竖条，顶部展开箭头 + 下方模式图标
    /// （32px 方块、图标中心 x=24）。专用页面（冲突/贮藏/浏览/追溯）不显示分组
    /// 列表，但窄条仍常驻——模式图标是这些页面返回工作台/历史的唯一入口。
    pub(crate) fn render_navigator_collapsed_strip(
        &self,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 专用页面没有可展开的导航区，箭头禁用并说明原因（窄窗覆盖逻辑同样不可达）。
        let toggle_enabled = context_navigator_supported_mode(self.main_mode);
        let strip = div()
            .id("navigator-collapsed-strip")
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .h_full()
            .min_h(px(0.0))
            .w(px(theme::NAVIGATOR_COLLAPSED_WIDTH))
            .py(px(theme::SPACE_2))
            .border_r_1()
            .border_color(rgb(theme::BORDER_MUTED))
            .bg(rgb(theme::RAIL_SURFACE))
            .child(self.render_context_navigator_toggle(overlay, toggle_enabled, cx))
            // 箭头与模式图标之间留出间隔，避免两排图标贴在一起
            .child(div().h(px(theme::SPACE_2)));
        self.navigator_mode_entries().into_iter().fold(
            strip,
            |strip, (id, icon, _label, active, mode)| {
                strip.child(self.navigator_mode_button(id, icon, active, mode, cx))
            },
        )
    }

    /// 展开态模式按钮：图标与文字是**同一个**按钮--悬停、按下、选中反馈整行同步。
    /// 左内边距让图标中心落在 x=24，与收起窄条图标位置一致（两态切换图标零位移）。
    /// 仅鼠标交互，键盘 Tab 顺序保持在收起窄条的图标按钮上。
    fn navigator_expanded_mode_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        label: &'static str,
        active: bool,
        mode: MainMode,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        div()
            .id(format!("navigator-mode-{id}"))
            .relative()
            .flex_none()
            .w_full()
            .h(px(theme::CONTROL_HEIGHT_REGULAR))
            .mb(px(theme::SPACE_1))
            .flex()
            .items_center()
            // 图标槽 16px：pl(16) + 槽中心 8 -> 图标中心 x=24（与收起窄条一致）
            .pl(px(16.0))
            .gap(px(theme::SPACE_2))
            .rounded(px(theme::RADIUS_XS))
            .cursor_pointer()
            .text_size(px(theme::TYPE_BODY))
            .bg(if active {
                rgb(theme::PRIMARY_SUBTLE)
            } else {
                rgb(theme::SURFACE_BASE)
            })
            .text_color(rgb(if active {
                theme::PRIMARY
            } else {
                theme::CONTENT_SECONDARY
            }))
            .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
            .when(active, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(theme::SPACE_2))
                        .bottom(px(theme::SPACE_2))
                        .w(px(2.0))
                        .rounded_full()
                        .bg(rgb(theme::PRIMARY)),
                )
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // 点击不抢焦点：避免按 Ctrl/Alt 等修饰键时显示键盘导航焦点环。
                this.set_main_mode(mode);
                cx.notify();
            }))
            .child(toolbar_icon(
                icon,
                if active {
                    theme::PRIMARY
                } else {
                    theme::CONTENT_SECONDARY
                },
            ))
            .child(label)
    }

    /// `enabled` 为 false 时按钮禁用（专用页面没有可展开的导航区）。
    pub(crate) fn render_context_navigator_toggle(
        &self,
        overlay: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let visible = if overlay {
            self.context_navigator_overlay_open
        } else {
            self.context_navigator_preferences
                .is_visible(self.main_mode)
        };
        // 箭头表达点击后的移动方向：展开时指左（收起导航），收起时指右（展开导航）。
        let toggle_icon = if visible {
            ToolbarIcon::ChevronLeft
        } else {
            ToolbarIcon::ChevronRight
        };
        focusable_icon_button(
            "context-navigator-toggle".into(),
            toggle_icon,
            if visible {
                "收起上下文导航"
            } else {
                "展开上下文导航"
            },
            enabled,
            &self.context_navigator_toggle_focus,
            move |this, window, _cx| {
                if overlay {
                    let opening = !this.context_navigator_overlay_open;
                    this.context_navigator_overlay_open = opening;
                    if opening {
                        window.focus(&this.context_navigator_focus);
                    }
                } else {
                    let mode = this.main_mode;
                    this.context_navigator_preferences.toggle(mode);
                }
            },
            cx,
        )
        .when(visible, |this| this.bg(rgb(theme::PRIMARY_SUBTLE)))
    }

    pub(crate) fn render_context_navigator(
        &self,
        window: &Window,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 模式按钮区：位于「上下文导航」标题与本地分支分组之间，
        // 图标 + 文字整行按钮，点击切换主页面。
        let mode_buttons = self.navigator_mode_entries().into_iter().fold(
            div()
                .id("navigator-mode-buttons")
                .flex()
                .flex_none()
                .flex_col()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_2))
                .border_b_1()
                .border_color(rgb(theme::BORDER_MUTED)),
            |buttons, (id, icon, label, active, mode)| {
                buttons
                    .child(self.navigator_expanded_mode_button(id, icon, label, active, mode, cx))
            },
        );
        div()
            .id("context-navigator")
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.sidebar_width))
            .h_full()
            .min_h(px(0.0))
            .bg(rgb(theme::SURFACE_BASE))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(px(theme::SPACE_3))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER_MUTED))
                    .text_size(px(theme::TYPE_META))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::CONTENT_SECONDARY))
                    .child("上下文导航")
                    .child(self.render_context_navigator_toggle(overlay, true, cx)),
            )
            .child(mode_buttons)
            .child(self.render_sidebar(window, cx))
    }

    pub(crate) fn render_context_navigator_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(0.0))
            // 覆盖层左缘贴收起窄条右缘（窄条恒定 48px 常驻在最左列）。
            .left(px(theme::NAVIGATOR_COLLAPSED_WIDTH))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .bg(crate::ui::theme::rgba(theme::DIALOG_OVERLAY))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    this.context_navigator_overlay_open = false;
                    window.focus(&this.context_navigator_toggle_focus);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_none()
                    .h_full()
                    // 原生 GPUI 仅支持 tab group，不伪造临时焦点句柄；将 Tab 顺序限制在导航区。
                    .tab_group()
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.render_context_navigator(window, true, cx)),
            )
    }

    fn render_chrome_brand(&self) -> impl IntoElement {
        div()
            .id("titlebar-brand")
            .flex()
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(theme::SPACE_2))
            .cursor(CursorStyle::Arrow)
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex_none()
                    .size(px(26.0))
                    .rounded(px(theme::RADIUS_SM))
                    .bg(rgb(theme::PRIMARY))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(theme::PRIMARY_FOREGROUND))
                    .child("K"),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::CONTENT_PRIMARY))
                    .child("Khaslana"),
            )
    }

    fn render_chrome_drag_area(&self) -> impl IntoElement {
        div()
            .id("titlebar-drag-area")
            .flex_1()
            .min_w(px(24.0))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
    }

    fn render_chrome_window_controls(&self, window: &Window) -> gpui::Div {
        let maximize_icon = if window.is_maximized() {
            ToolbarIcon::Restore
        } else {
            ToolbarIcon::Maximize
        };
        div()
            .flex()
            .flex_none()
            .w(px(WINDOW_CONTROLS_WIDTH))
            .h_full()
            .child(self.chrome_window_control_button(
                "window-minimize",
                ToolbarIcon::Minus,
                false,
                WindowControlArea::Min,
            ))
            .child(self.chrome_window_control_button(
                "window-maximize",
                maximize_icon,
                false,
                WindowControlArea::Max,
            ))
            .child(self.chrome_window_control_button(
                "window-close",
                ToolbarIcon::Close,
                true,
                WindowControlArea::Close,
            ))
    }

    fn chrome_window_control_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        danger: bool,
        area: WindowControlArea,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .w(px(44.0))
            .h_full()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .window_control_area(area)
            .hover(move |this| {
                this.bg(rgb(if danger {
                    theme::FEEDBACK_ERROR_BG
                } else {
                    theme::STATE_HOVER
                }))
            })
            .child(toolbar_icon_with_size(
                icon,
                theme::CONTENT_PRIMARY,
                12.0,
                16.0,
            ))
    }

    fn chrome_command_button(
        &self,
        label: &'static str,
        icon_kind: ToolbarIcon,
        sync_label: Option<String>,
        enabled: bool,
        focus: &FocusHandle,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let on_click = Arc::new(on_click);
        let keyboard_click = Arc::clone(&on_click);
        let focus_for_click = focus.clone();
        let disabled_reason = if enabled {
            None
        } else {
            chrome_action_disabled_reason(
                label,
                self.repo_path.is_some(),
                !self.loading.remote() && self.current_remote().is_some(),
                self.busy,
                self.merge_in_progress(),
            )
        };
        div()
            .id(format!("chrome-command-{label}"))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(theme::SPACE_1))
            .min_h(px(theme::CONTROL_HEIGHT_REGULAR))
            .px(px(theme::SPACE_2))
            .rounded(px(theme::RADIUS_XS))
            .track_focus(focus)
            .tab_index(if enabled { 0 } else { -1 })
            .tab_stop(enabled)
            .text_color(rgb(if enabled {
                theme::CONTENT_PRIMARY
            } else {
                theme::CONTENT_TERTIARY
            }))
            .text_size(px(theme::TYPE_BODY))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                    .active(|this| this.opacity(0.8))
            })
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.5))
            .when_some(disabled_reason, |this, reason| {
                this.tooltip(move |_window, cx| crate::ui::components::tooltip_text(reason, cx))
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    keyboard_click(this, window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    window.focus(&focus_for_click);
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(
                icon_kind,
                if enabled {
                    theme::CONTENT_SECONDARY
                } else {
                    theme::CONTENT_TERTIARY
                },
            ))
            .child(label)
            .when_some(sync_label, |this, label| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_META))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::PRIMARY))
                        .child(label),
                )
            })
    }

    /// 收起窄条的模式图标按钮：32px 方块，支持 Tab 焦点与 Enter/Space 激活。
    fn navigator_mode_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        active: bool,
        mode: MainMode,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let focus = self.navigator_mode_focus_handle(id);
        div()
            .id(id)
            .relative()
            .flex_none()
            .size(px(theme::CONTROL_HEIGHT_REGULAR))
            .mb(px(theme::SPACE_1))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .track_focus(&focus)
            .tab_index(0)
            .bg(if active {
                rgb(theme::PRIMARY_SUBTLE)
            } else {
                rgb(theme::RAIL_SURFACE)
            })
            .text_color(rgb(if active {
                theme::PRIMARY
            } else {
                theme::CONTENT_SECONDARY
            }))
            .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.set_main_mode(mode);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // 点击不抢焦点：抢焦点后按 Ctrl/Alt 等修饰键会让图标显示键盘
                // 导航焦点环，视觉上像图标样式突变；Tab 键盘导航不受影响。
                this.set_main_mode(mode);
                cx.notify();
            }))
            .child(toolbar_icon(
                icon,
                if active {
                    theme::PRIMARY
                } else {
                    theme::CONTENT_SECONDARY
                },
            ))
            .when(active, |this| {
                this.child(
                    div()
                        .absolute()
                        // 按钮在 48px 窄条内居中，指示条贴窄条左缘。
                        .left(px(-theme::SPACE_2))
                        .top(px(theme::SPACE_2))
                        .bottom(px(theme::SPACE_2))
                        .w(px(2.0))
                        .rounded_full()
                        .bg(rgb(theme::PRIMARY)),
                )
            })
    }
}

#[cfg(test)]
#[path = "tests/chrome_view.rs"]
mod tests;
