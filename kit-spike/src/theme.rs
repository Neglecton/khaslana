//! 设计 token → GPUI Kit 主题的单向映射，含深浅两套。
//!
//! Kit 的组件外观取 `Theme.tokens`（由 `ThemeColor` 一比一派生），
//! 所以这里改的是 `ThemeColor` 并重建 `Theme`，而不是只改零散字段——
//! 否则按钮等组件仍会使用派生前的旧值（M1 实测）。
//!
//! 只在本模块读写 Kit 主题；业务视图不直接改主题字段。
//! 后续接入真实应用时，这一层换成从用户主题偏好驱动即可。

use gpui_kit::component::{Theme, ThemeColor};
use gpui_kit::{App, Hsla, px, rgba};

use crate::tokens;

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// 把「轻盈悬浮工作台」token 应用到 Kit 主题（浅色）。
pub fn apply(cx: &mut App) {
    apply_variant(cx, false);
}

/// 按深浅变体重建整套主题。
///
/// 切换深浅必须走这条完整路径：只改 `Theme.colors` 不会刷新按旧色派生的 `tokens`。
pub fn apply_variant(cx: &mut App, dark: bool) {
    let mut colors: ThemeColor = Theme::global(cx).colors;

    if dark {
        // 深色：深蓝灰环境底、稍亮面板、轻轮廓，避免反相成霓虹。
        colors.foreground = c(0xE6ECF7_FF);
        colors.background = c(0x1E222C_FF);
        colors.border = c(0x323A4A_FF);
        colors.input = c(0x323A4A_FF);
        colors.muted = c(0x28303E_FF);
        colors.muted_foreground = c(0x93A0B8_FF);
        colors.popover = c(0x252B37_FF);
        colors.popover_foreground = c(0xE6ECF7_FF);
        colors.ring = c(0x5C86D8_FF);
        colors.selection = c(0x2B3B57_FF);

        colors.button = c(0x2A3140_FF);
        colors.button_hover = c(0x333B4C_FF);
        colors.button_active = c(0x222835_FF);
        colors.button_foreground = c(0xE6ECF7_FF);
        colors.button_secondary = c(0x2A3140_FF);
        colors.button_secondary_hover = c(0x333B4C_FF);
        colors.button_secondary_active = c(0x222835_FF);
        colors.button_secondary_foreground = c(0xE6ECF7_FF);
        colors.secondary = c(0x2A3140_FF);
        colors.secondary_hover = c(0x333B4C_FF);
        colors.secondary_active = c(0x222835_FF);
        colors.secondary_foreground = c(0xE6ECF7_FF);

        colors.list = c(0x1E222C_FF);
        colors.list_hover = c(0x28303E_FF);
        colors.list_active = c(0x2B3B57_FF);
        colors.list_active_border = c(0x5C86D8_FF);
        colors.list_head = c(0x252B37_FF);

        colors.sidebar = c(0x1B1F29_FF);
        colors.sidebar_foreground = c(0xAEBBD2_FF);
        colors.sidebar_accent = c(0x2B3B57_FF);
        colors.sidebar_accent_foreground = c(0x8FB4F5_FF);
        colors.sidebar_border = c(0x2A3140_FF);

        colors.title_bar = c(0x1B1F29_FF);
        colors.title_bar_border = c(0x2A3140_FF);
        colors.status_bar = c(0x1B1F29_FF);
        colors.status_bar_border = c(0x2A3140_FF);
        colors.scrollbar = c(0x28303E_FF);
        colors.scrollbar_thumb = c(0x3A4356_FF);
        colors.scrollbar_thumb_hover = c(0x4C576E_FF);

        colors.accent = c(0x2B3B57_FF);
        colors.accent_foreground = c(0xE6ECF7_FF);
    } else {
        // 浅色：与画板一致（见 visual-spec §3）。
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

        colors.accent = tokens::hover_row();
        colors.accent_foreground = tokens::text_strong();
    }

    // 主色与语义色两套共用（强调色偏好后续从这里接管）
    colors.primary = tokens::primary();
    colors.primary_hover = tokens::primary_hover();
    colors.primary_active = tokens::primary_active();
    colors.primary_foreground = tokens::primary_foreground();
    colors.button_primary = tokens::primary();
    colors.button_primary_hover = tokens::primary_hover();
    colors.button_primary_active = tokens::primary_active();
    colors.button_primary_foreground = tokens::primary_foreground();
    colors.success = tokens::add_text();
    colors.danger = tokens::del_text();
    colors.warning = tokens::badge_modified();

    // 重建整个主题：`Theme::from` 会按新 colors 派生 tokens
    let mut theme = Theme::from(&colors);
    theme.radius = px(tokens::RADIUS_CONTROL);
    theme.radius_lg = px(tokens::RADIUS_PANEL);
    theme.shadow = !dark;
    theme.font_size = px(tokens::FONT_BODY);
    theme.mono_font_size = px(tokens::FONT_CODE);
    // 画板不显示常驻滚动条，改为悬停出现。
    theme.scrollbar_mode = gpui_kit::component::scroll::ScrollbarMode::Hover;
    *Theme::global_mut(cx) = theme;
}
