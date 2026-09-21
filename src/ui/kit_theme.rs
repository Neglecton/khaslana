//! Khaslana 语义 token → GPUI Kit 主题的单向映射。
//!
//! Kit 的组件外观读 `Theme.tokens`，而它由 `ThemeColor` **一比一派生**：
//! 写入 `ThemeColor` 后必须整体重建 `Theme`，只改零散字段不会刷新已派生的
//! tokens（M1 在隔离样板上实测过）。这里只做映射，不改写 Khaslana 自己的
//! 语义色板——业务视图继续用 `ui::theme`，两边由同一组 token 驱动。

use gpui::{App, Hsla, px, rgb};
use gpui_kit::component::Theme;
use gpui_kit::component::scroll::ScrollbarMode;

use super::theme::{self as ui_theme, AccentPalette, ThemeVariant};

/// 按当前变体与强调色重建 Kit 全局主题。
///
/// 调用点应与 `ui_theme::set_active_variant` / `set_active_accent` 同步，
/// 保证自绘视图与 Kit 组件看到同一套颜色。
pub(crate) fn apply(cx: &mut App, variant: ThemeVariant, accent: &AccentPalette) {
    let color = |token: u32| -> Hsla {
        rgb(ui_theme::resolve_color_for_variant(token, variant)).into()
    };
    let accent_color = |pair: (u32, u32)| -> Hsla {
        match variant {
            ThemeVariant::Light => rgb(pair.0).into(),
            ThemeVariant::Dark => rgb(pair.1).into(),
        }
    };

    let mut colors = Theme::global(cx).colors;

    // 文字与面
    colors.foreground = color(ui_theme::CONTENT_PRIMARY);
    // 基础面取工作面板（Kit 的输入、卡片默认底色都由它派生）。
    colors.background = color(ui_theme::WB_PANEL);
    colors.border = color(ui_theme::BORDER_MUTED);
    // `input` 同时是复选框未选中态的边框色，取描边档而不是输入框底色。
    colors.input = color(ui_theme::BORDER_STRONG);
    // `muted` 是各类弱背景（hint、ghost hover）的公共来源，跟工作台的悬停档一致。
    colors.muted = color(ui_theme::WB_ROW_HOVER);
    colors.muted_foreground = color(ui_theme::CONTENT_TERTIARY);
    colors.popover = color(ui_theme::SURFACE_OVERLAY);
    colors.popover_foreground = color(ui_theme::CONTENT_PRIMARY);
    colors.ring = accent_color(accent.focused_border);
    colors.selection = accent_color(accent.selection);

    // 主色族跟随用户强调色偏好
    colors.primary = accent_color(accent.primary);
    colors.primary_hover = accent_color(accent.primary);
    colors.primary_active = accent_color(accent.primary);
    colors.primary_foreground = accent_color(accent.foreground);
    colors.accent = color(ui_theme::STATE_SELECTION);
    colors.accent_foreground = color(ui_theme::CONTENT_PRIMARY);

    // 主按钮：强调色实面
    colors.button_primary = accent_color(accent.primary);
    colors.button_primary_hover = accent_color(accent.primary);
    colors.button_primary_active = accent_color(accent.primary);
    colors.button_primary_foreground = accent_color(accent.foreground);

    // 普通按钮：抬升面 + 悬停/按下态
    colors.button = color(ui_theme::SURFACE_RAISED);
    colors.button_hover = color(ui_theme::STATE_HOVER);
    colors.button_active = color(ui_theme::SURFACE_SUNKEN);
    colors.button_foreground = color(ui_theme::CONTENT_PRIMARY);
    colors.button_secondary = color(ui_theme::SURFACE_RAISED);
    colors.button_secondary_hover = color(ui_theme::STATE_HOVER);
    colors.button_secondary_active = color(ui_theme::SURFACE_SUNKEN);
    colors.button_secondary_foreground = color(ui_theme::CONTENT_PRIMARY);
    colors.secondary = color(ui_theme::SURFACE_RAISED);
    colors.secondary_hover = color(ui_theme::STATE_HOVER);
    colors.secondary_active = color(ui_theme::SURFACE_SUNKEN);
    colors.secondary_foreground = color(ui_theme::CONTENT_PRIMARY);

    // 列表与菜单行
    colors.list = color(ui_theme::WB_PANEL);
    colors.list_hover = color(ui_theme::WB_ROW_HOVER);
    colors.list_active = color(ui_theme::STATE_SELECTION);
    colors.list_active_border = accent_color(accent.focused_border);
    colors.list_head = color(ui_theme::SURFACE_SUNKEN);

    // 侧栏（Context Navigator）
    colors.sidebar = color(ui_theme::WB_NAV);
    colors.sidebar_foreground = color(ui_theme::CONTENT_SECONDARY);
    colors.sidebar_accent = color(ui_theme::STATE_SELECTION);
    colors.sidebar_accent_foreground = accent_color(accent.primary);
    colors.sidebar_border = color(ui_theme::BORDER_MUTED);

    // 外壳
    colors.title_bar = color(ui_theme::WB_TOOLBAR);
    colors.title_bar_border = color(ui_theme::WB_TOOLBAR);
    colors.status_bar = color(ui_theme::WB_ENV);
    colors.status_bar_border = color(ui_theme::BORDER_MUTED);
    colors.scrollbar = color(ui_theme::SCROLLBAR_TRACK);
    colors.scrollbar_thumb = color(ui_theme::SCROLLBAR_THUMB);
    colors.scrollbar_thumb_hover = color(ui_theme::SCROLLBAR_THUMB_ACTIVE);

    // 语义状态：Git 增删与反馈不借用品牌色
    colors.success = color(ui_theme::FEEDBACK_SUCCESS_TEXT);
    colors.danger = color(ui_theme::FEEDBACK_ERROR_TEXT);
    colors.warning = color(ui_theme::FEEDBACK_WARNING_TEXT);

    // 开关（Kit Switch）：未选中轨道取描边档、滑块固定白（深浅主题通用，
    // 与自绘开关的历史配色一致）；选中态轨道由 Kit 用 `primary` 表达。
    colors.switch = color(ui_theme::BORDER);
    colors.switch_thumb = color(ui_theme::WHITE);

    let mut theme = Theme::from(&colors);
    theme.radius = px(ui_theme::RADIUS_SM);
    theme.radius_lg = px(ui_theme::RADIUS_MD);
    theme.shadow = true;
    theme.font_size = px(ui_theme::TYPE_BODY);
    theme.mono_font_size = px(ui_theme::TYPE_BODY);
    // 画板与主界面都按「悬停出现」显示滚动条，避免常驻占宽。
    theme.scrollbar_mode = ScrollbarMode::Hover;
    *Theme::global_mut(cx) = theme;
}
