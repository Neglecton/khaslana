use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{Rgba, WindowAppearance, rgb as gpui_rgb, rgba as gpui_rgba};
use khaslana::ThemeMode;

// Khaslana 运行时主题色。业务视图继续传递 u32 语义 token，最终在 rgb/rgba 入口解析，
// 这样主题切换不会把色板状态散落到每个 view 和自绘组件中。
const THEME_TOKEN_PREFIX: u32 = 0xCAFE_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ThemeVariant {
    #[default]
    Light,
    Dark,
}

impl ThemeVariant {
    pub(crate) const fn window_appearance(self) -> WindowAppearance {
        match self {
            Self::Light => WindowAppearance::Light,
            Self::Dark => WindowAppearance::Dark,
        }
    }
}

static ACTIVE_THEME_VARIANT: AtomicU8 = AtomicU8::new(ThemeVariant::Light as u8);

pub(crate) fn set_active_variant(variant: ThemeVariant) {
    ACTIVE_THEME_VARIANT.store(variant as u8, Ordering::Relaxed);
}

pub(crate) fn active_variant() -> ThemeVariant {
    match ACTIVE_THEME_VARIANT.load(Ordering::Relaxed) {
        value if value == ThemeVariant::Dark as u8 => ThemeVariant::Dark,
        _ => ThemeVariant::Light,
    }
}

pub(crate) fn variant_for_mode(
    mode: ThemeMode,
    window_appearance: WindowAppearance,
) -> ThemeVariant {
    match mode {
        ThemeMode::Light => ThemeVariant::Light,
        ThemeMode::Dark => ThemeVariant::Dark,
        ThemeMode::System => match window_appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeVariant::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeVariant::Light,
        },
    }
}

macro_rules! theme_tokens {
    ($( $id:literal: $name:ident => $light:expr, $dark:expr; )+) => {
        $(
            pub(crate) const $name: u32 = THEME_TOKEN_PREFIX | $id;
        )+

        pub(crate) fn resolve_color_for_variant(color: u32, variant: ThemeVariant) -> u32 {
            match color {
                $(
                    $name => match variant {
                        ThemeVariant::Light => $light,
                        ThemeVariant::Dark => $dark,
                    },
                )+
                _ => color,
            }
        }
    };
}

theme_tokens! {
    // ── 基础色板 ──────────────────────────────────────────
    1: BACKGROUND => 0xffffff, 0x111318;
    2: FOREGROUND => 0x2A2933, 0xE8E8EC;
    3: CARD => 0xffffff, 0x181A20;
    4: CARD_FOREGROUND => 0x2A2933, 0xE8E8EC;
    5: PRIMARY => 0x5749F4, 0x9A92FF;
    6: PRIMARY_FOREGROUND => 0xffffff, 0x111318;
    7: PRIMARY_SUBTLE => 0xE8E6FB, 0x302D55;
    8: SECONDARY => 0xD9D9DB, 0x343740;
    9: SECONDARY_FOREGROUND => 0x2A2933, 0xE8E8EC;
    10: MUTED_FOREGROUND => 0x616167, 0xA8A8B0;
    11: ACCENT => 0xF5F5F5, 0x24262D;
    12: ACCENT_FOREGROUND => 0x2A2933, 0xE8E8EC;
    13: DESTRUCTIVE => 0xCC3314, 0xFF7863;
    14: DESTRUCTIVE_FOREGROUND => 0xffffff, 0x1D0905;
    15: BORDER => 0xC5C5CB, 0x3E414B;
    16: INPUT => 0xC5C5CB, 0x3E414B;
    17: WHITE => 0xffffff, 0xffffff;
    18: POPOVER => 0xffffff, 0x1C1E25;
    19: POPOVER_FOREGROUND => 0x2A2933, 0xE8E8EC;
    20: TILE => 0xF5F5F5, 0x24262D;

    // ── 侧边栏 ────────────────────────────────────────────
    21: SIDEBAR => 0xffffff, 0x14161B;
    22: SIDEBAR_FOREGROUND => 0x939399, 0xA8A8B0;
    23: SIDEBAR_ACCENT => 0xF5F5F5, 0x24262D;
    24: SIDEBAR_ACCENT_FOREGROUND => 0x2A2933, 0xE8E8EC;
    25: SIDEBAR_PRIMARY_FOREGROUND => 0x2A2933, 0xE8E8EC;
    26: SIDEBAR_BORDER => 0xD9D9DB, 0x343740;

    // ── Git 状态色 ─────────────────────────────────────────
    27: GIT_ADDED => 0x2DA44E, 0x56D364;
    28: GIT_MODIFIED => 0x0066FF, 0x58A6FF;
    29: GIT_REMOVED => 0xCF222E, 0xFF7B72;
    30: GIT_UNTRACKED => 0x8B949E, 0xA8A8B0;

    // ── Diff 配色 ──────────────────────────────────────────
    31: DIFF_ADDED_BG => 0xE6F4EA, 0x183A27;
    32: DIFF_ADDED_TEXT => 0x1A7F37, 0x7EE787;
    33: DIFF_REMOVED_BG => 0xFFEBE9, 0x3D1F22;
    34: DIFF_REMOVED_TEXT => 0xCF222E, 0xFFA198;
    35: DIFF_HEADER_BG => 0xF5F5F5, 0x24262D;
    36: DIFF_HEADER_TEXT => 0x616167, 0xA8A8B0;

    // ── 语义状态色 ────────────────────────────────────────
    37: COLOR_ERROR => 0xFFBFB2, 0x4A211B;
    38: COLOR_ERROR_FOREGROUND => 0x590F00, 0xFFB4A8;
    39: COLOR_SUCCESS => 0xA1E5A1, 0x173D25;
    40: COLOR_SUCCESS_FOREGROUND => 0x003300, 0x7EE787;
    41: COLOR_WARNING => 0xFFD9B2, 0x4B3516;
    42: COLOR_WARNING_FOREGROUND => 0x4D2700, 0xF2CC60;
    43: COLOR_INFO => 0xC9D6F0, 0x1D3350;
    44: COLOR_INFO_FOREGROUND => 0x001133, 0x79C0FF;

    // ── 输入框 ────────────────────────────────────────────
    45: INPUT_BG => 0xffffff, 0x181A20;
    46: INPUT_BG_FOCUSED => 0xffffff, 0x1C1E25;
    47: INPUT_BORDER => 0xC5C5CB, 0x3E414B;
    48: INPUT_BORDER_FOCUSED => 0x5749F4, 0x9A92FF;
    // 自绘输入框通过 rgba 使用占位色；两套色板都显式携带透明度。
    49: INPUT_PLACEHOLDER => 0x61616799, 0xA8A8B099;
    50: INPUT_SELECTION => 0x5749F433, 0x9A92FF55;
    51: INPUT_CARET => 0x2A2933, 0xE8E8EC;

    // ── 弹窗和对话 ────────────────────────────────────────
    52: DIALOG_OVERLAY => 0x0f172a55, 0x00000088;
    53: TOOLTIP_BG => 0x2A2933, 0x2A2933;
    54: TOOLTIP_BORDER => 0x3A3955, 0x555768;

    // ── 提交历史图 ────────────────────────────────────────
    55: HISTORY_GRAPH_0 => 0xf97316, 0xfb923c;
    56: HISTORY_GRAPH_1 => 0x14b8a6, 0x2dd4bf;
    57: HISTORY_GRAPH_2 => 0x3b82f6, 0x60a5fa;
    58: HISTORY_GRAPH_3 => 0xeab308, 0xfacc15;
    59: HISTORY_GRAPH_4 => 0xef4444, 0xf87171;
    60: HISTORY_GRAPH_5 => 0x8b5cf6, 0xa78bfa;
    61: HISTORY_GRAPH_6 => 0x22c55e, 0x4ade80;
    62: HISTORY_GRAPH_7 => 0xec4899, 0xf472b6;

    // ── 引用标签色 ────────────────────────────────────────
    63: REF_LOCAL_BG => 0xE6F4EA, 0x183A27;
    64: REF_LOCAL_BORDER => 0x2DA44E, 0x56D364;
    65: REF_LOCAL_TEXT => 0x1A7F37, 0x7EE787;
    66: REF_REMOTE_BG => 0xC9D6F0, 0x1D3350;
    67: REF_REMOTE_BORDER => 0x0066FF, 0x58A6FF;
    68: REF_REMOTE_TEXT => 0x003399, 0x79C0FF;
    69: REF_TAG_BG => 0xFFF2D9, 0x483816;
    70: REF_TAG_BORDER => 0xD4A72C, 0xDDBB4D;
    71: REF_TAG_TEXT => 0x7C5800, 0xF2CC60;
    72: REF_HEAD_BG => 0x5749F4, 0x9A92FF;
    73: REF_HEAD_TEXT => 0xffffff, 0x111318;

    // ── 反馈/Toast ────────────────────────────────────────
    74: FEEDBACK_INFO_BG => 0xC9D6F0, 0x1D3350;
    75: FEEDBACK_INFO_BORDER => 0x0066FF, 0x58A6FF;
    76: FEEDBACK_INFO_TEXT => 0x001133, 0xB6DBFF;
    77: FEEDBACK_SUCCESS_BG => 0xA1E5A1, 0x173D25;
    78: FEEDBACK_SUCCESS_BORDER => 0x2DA44E, 0x56D364;
    79: FEEDBACK_SUCCESS_TEXT => 0x003300, 0xA7F3B5;
    80: FEEDBACK_WARNING_BG => 0xFFD9B2, 0x4B3516;
    81: FEEDBACK_WARNING_BORDER => 0xD4A72C, 0xDDBB4D;
    82: FEEDBACK_WARNING_TEXT => 0x4D2700, 0xFFE08A;
    83: FEEDBACK_ERROR_BG => 0xFFBFB2, 0x4A211B;
    84: FEEDBACK_ERROR_BORDER => 0xCF222E, 0xFF7B72;
    85: FEEDBACK_ERROR_TEXT => 0x590F00, 0xFFC1B8;

    // ── 进度条 ────────────────────────────────────────────
    86: PROGRESS_TRACK => 0xE0E0E3, 0x343740;
    87: PROGRESS_FILL => 0x5749F4, 0x9A92FF;

    // ── 滚动条 ────────────────────────────────────────────
    88: SCROLLBAR_TRACK => 0xF5F5F5CC, 0x24262DCC;
    89: SCROLLBAR_THUMB => 0xC5C5CBDD, 0x555863DD;
    90: SCROLLBAR_THUMB_ACTIVE => 0x939399EE, 0x777A86EE;
}

pub(crate) const HISTORY_GRAPH_COLORS: [u32; 8] = [
    HISTORY_GRAPH_0,
    HISTORY_GRAPH_1,
    HISTORY_GRAPH_2,
    HISTORY_GRAPH_3,
    HISTORY_GRAPH_4,
    HISTORY_GRAPH_5,
    HISTORY_GRAPH_6,
    HISTORY_GRAPH_7,
];

pub(crate) fn resolve_color(color: u32) -> u32 {
    resolve_color_for_variant(color, active_variant())
}

/// 主题感知的 RGB 转换入口；普通字面颜色会原样透传。
pub(crate) fn rgb(color: u32) -> Rgba {
    gpui_rgb(resolve_color(color))
}

/// 主题感知的 RGBA 转换入口；用于选区、遮罩和滚动条等含透明度颜色。
pub(crate) fn rgba(color: u32) -> Rgba {
    gpui_rgba(resolve_color(color))
}

// ── 圆角常量 ──────────────────────────────────────────────
pub(crate) const RADIUS_XS: f32 = 6.0;
pub(crate) const RADIUS_PILL: f32 = 999.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_tokens_resolve_to_distinct_light_and_dark_colors() {
        assert_eq!(
            resolve_color_for_variant(BACKGROUND, ThemeVariant::Light),
            0xffffff
        );
        assert_eq!(
            resolve_color_for_variant(BACKGROUND, ThemeVariant::Dark),
            0x111318
        );
        assert_ne!(
            resolve_color_for_variant(DIFF_ADDED_BG, ThemeVariant::Light),
            resolve_color_for_variant(DIFF_ADDED_BG, ThemeVariant::Dark)
        );
    }

    #[test]
    fn literal_colors_are_not_treated_as_theme_tokens() {
        assert_eq!(
            resolve_color_for_variant(0x123456, ThemeVariant::Dark),
            0x123456
        );
    }

    #[test]
    fn system_mode_follows_window_appearance() {
        assert_eq!(
            variant_for_mode(ThemeMode::System, WindowAppearance::Light),
            ThemeVariant::Light
        );
        assert_eq!(
            variant_for_mode(ThemeMode::System, WindowAppearance::Dark),
            ThemeVariant::Dark
        );
        assert_eq!(
            variant_for_mode(ThemeMode::Light, WindowAppearance::Dark),
            ThemeVariant::Light
        );
    }
}
