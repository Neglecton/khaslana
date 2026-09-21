//! 设计 token → GPUI Kit 主题的单向映射。
//!
//! Kit 的组件外观取 `Theme.tokens`（由 `ThemeColor` 一比一派生），
//! 所以这里改的是 `ThemeColor` 并重建 `Theme`，而不是只改零散字段——
//! 否则按钮等组件仍会使用派生前的旧值。
//!
//! 只在本模块读写 Kit 主题；业务视图不直接改主题字段。
//! 后续接入真实应用时，这一层换成从用户主题偏好驱动即可。

use gpui_kit::component::{Theme, ThemeColor};
use gpui_kit::{App, px};

use crate::tokens;

/// 把「轻盈悬浮工作台」token 应用到 Kit 主题。
///
/// 深度层次（环境底 → 工作面板 → 控件 → 浮层）主要落在自绘的
/// `floating_panel` / `control_surface` 上；这里负责组件层共用的语义色，
/// 以免 Kit 组件与自绘面板出现两套颜色。
pub fn apply(cx: &mut App) {
    let mut colors: ThemeColor = Theme::global(cx).colors;

    // 文字与面
    colors.foreground = tokens::text_strong();
    colors.background = tokens::panel();
    colors.border = tokens::border();
    // `input` 同时是复选框未选中态的边框色，取描边档而不是输入框底色；
    // 自绘输入外壳（input_surface）不依赖这个 token。
    colors.input = tokens::border();
    colors.muted = tokens::hover_row();
    colors.muted_foreground = tokens::text_muted();
    colors.popover = tokens::panel();
    colors.popover_foreground = tokens::text_strong();
    colors.ring = tokens::focus_ring();
    colors.selection = tokens::selected_row();

    // 主色（强调色偏好后续从这里接管）
    colors.primary = tokens::primary();
    colors.primary_hover = tokens::primary_hover();
    colors.primary_active = tokens::primary_active();
    colors.primary_foreground = tokens::primary_foreground();
    colors.accent = tokens::hover_row();
    colors.accent_foreground = tokens::text_strong();

    // 主按钮：蓝色实面 + 白字
    colors.button_primary = tokens::primary();
    colors.button_primary_hover = tokens::primary_hover();
    colors.button_primary_active = tokens::primary_active();
    colors.button_primary_foreground = tokens::primary_foreground();

    // 普通按钮：白底轻薄实体 + 深色字（设计稿的工具栏与次级动作）
    colors.button = tokens::panel();
    colors.button_hover = tokens::hover_row();
    colors.button_active = tokens::control_quiet();
    colors.button_foreground = tokens::text_strong();
    colors.button_secondary = tokens::panel();
    colors.button_secondary_hover = tokens::hover_row();
    colors.button_secondary_active = tokens::control_quiet();
    colors.button_secondary_foreground = tokens::text_strong();
    colors.secondary = tokens::panel();
    colors.secondary_hover = tokens::hover_row();
    colors.secondary_active = tokens::control_quiet();
    colors.secondary_foreground = tokens::text_strong();

    // 列表与菜单行
    colors.list = tokens::panel();
    colors.list_hover = tokens::hover_row();
    colors.list_active = tokens::selected_row();
    colors.list_active_border = tokens::focus_ring();
    colors.list_head = tokens::input_surface();

    // 导航（侧栏）
    colors.sidebar = tokens::nav_surface();
    colors.sidebar_foreground = tokens::text_nav();
    colors.sidebar_accent = tokens::selected_nav();
    colors.sidebar_accent_foreground = tokens::nav_selected();
    colors.sidebar_border = tokens::border_soft();

    // 外壳
    colors.title_bar = tokens::toolbar_bg();
    colors.title_bar_border = tokens::border_soft();
    colors.status_bar = tokens::toolbar_bg();
    colors.status_bar_border = tokens::border_soft();
    colors.scrollbar = tokens::input_surface();
    colors.scrollbar_thumb = tokens::border();
    colors.scrollbar_thumb_hover = tokens::text_muted();

    // 语义状态：Git 增删不借用品牌色
    colors.success = tokens::add_text();
    colors.danger = tokens::del_text();
    colors.warning = tokens::badge_modified();

    // 重建整个主题：`Theme::from` 会按新 colors 派生 tokens
    let mut theme = Theme::from(&colors);
    theme.radius = px(tokens::RADIUS_CONTROL);
    theme.radius_lg = px(tokens::RADIUS_PANEL);
    theme.shadow = true;
    theme.font_size = px(tokens::FONT_BODY);
    theme.mono_font_size = px(tokens::FONT_CODE);
    // 画板不显示常驻滚动条，改为悬停出现。
    theme.scrollbar_mode = gpui_kit::component::scroll::ScrollbarMode::Hover;
    *Theme::global_mut(cx) = theme;
}
