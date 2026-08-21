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

    /// 是否深色变体；语法高亮等按深浅二选一的能力用它分流。
    pub(crate) const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
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

        /// 解析静态色板 token（主色族 token 不在此处处理，由 accent 预设动态解析）。
        fn resolve_static_token(color: u32, variant: ThemeVariant) -> u32 {
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
    // 重命名 / 类型变更：橙（浅色深橙、深色橙，白字可读）
    91: GIT_RENAMED => 0xBC4C00, 0xDB6D28;

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
    // 主色族 token（48 INPUT_BORDER_FOCUSED、50 INPUT_SELECTION）见下方 accent 声明。
    49: INPUT_PLACEHOLDER => 0x61616799, 0xA8A8B099;
    51: INPUT_CARET => 0x2A2933, 0xE8E8EC;
    // hunk 分隔行底色：需与 CARD（0xffffff/0x181A20）拉开明显层次
    //（DIFF_HEADER_BG 与 ACCENT/TILE 同值太接近底色）。
    92: DIFF_HUNK_BG => 0xE9E9EE, 0x2A2D36;

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
    // 主色族 token（72 REF_HEAD_BG）见下方 accent 声明。
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
    // 主色族 token（87 PROGRESS_FILL）见下方 accent 声明。

    // ── 滚动条 ────────────────────────────────────────────
    88: SCROLLBAR_TRACK => 0xF5F5F5CC, 0x24262DCC;
    89: SCROLLBAR_THUMB => 0xC5C5CBDD, 0x555863DD;
    90: SCROLLBAR_THUMB_ACTIVE => 0x939399EE, 0x777A86EE;

    // ── Focus Workbench 设计基础 ───────────────────────────
    // 旧 token 保持不变；以下语义 token 供新壳层和后续 view 逐步迁移。
    93: SURFACE_CANVAS => 0xF8F9FB, 0x101216;
    94: SURFACE_BASE => 0xFFFFFF, 0x181A20;
    95: SURFACE_RAISED => 0xFFFFFF, 0x20232B;
    96: SURFACE_SUNKEN => 0xF1F3F6, 0x16181D;
    97: SURFACE_OVERLAY => 0xFFFFFF, 0x242730;
    98: CONTENT_PRIMARY => 0x20232B, 0xF0F1F4;
    99: CONTENT_SECONDARY => 0x5E6470, 0xB3B8C3;
    100: CONTENT_TERTIARY => 0x7E8491, 0x858B96;
    101: STATE_HOVER => 0xECEFF4, 0x2B2F39;
    102: STATE_SELECTION => 0xE8E6FB, 0x302D55;
    // 主色族 token（103 STATE_FOCUS_RING）见下方 accent 声明。
    104: BORDER_MUTED => 0xE4E7EC, 0x2D313A;
    105: BORDER_STRONG => 0xB8BEC9, 0x505663;
    106: RAIL_SURFACE => 0xF4F6F9, 0x15171C;
    107: TITLEBAR_SURFACE => 0xFFFFFF, 0x181A20;
    108: SHADOW_ELEVATION_1 => 0x1820331F, 0x00000052;
    109: SHADOW_ELEVATION_2 => 0x18203333, 0x00000080;
}

// ── 主色族 token（受 accent 预设动态控制）─────────────────
// 这些 token 的真实色值由当前激活的 accent 预设决定，运行时可切换。
// const 仅声明 token ID，解析在 resolve_accent_token 中完成。
pub(crate) const PRIMARY: u32 = THEME_TOKEN_PREFIX | 5;
pub(crate) const PRIMARY_FOREGROUND: u32 = THEME_TOKEN_PREFIX | 6;
pub(crate) const PRIMARY_SUBTLE: u32 = THEME_TOKEN_PREFIX | 7;
pub(crate) const INPUT_BORDER_FOCUSED: u32 = THEME_TOKEN_PREFIX | 48;
pub(crate) const INPUT_SELECTION: u32 = THEME_TOKEN_PREFIX | 50;
pub(crate) const REF_HEAD_BG: u32 = THEME_TOKEN_PREFIX | 72;
pub(crate) const PROGRESS_FILL: u32 = THEME_TOKEN_PREFIX | 87;
pub(crate) const STATE_FOCUS_RING: u32 = THEME_TOKEN_PREFIX | 103;

/// 一个主题色预设的浅色/深色色对。元组按 (浅色, 深色) 顺序。
#[derive(Clone, Copy, Debug)]
pub(crate) struct AccentPalette {
    /// 主色（按钮、选中态、链接）
    pub primary: (u32, u32),
    /// 主色上的文字
    pub foreground: (u32, u32),
    /// 主色淡背景（hover / 选中底色）
    pub subtle: (u32, u32),
    /// 输入框聚焦边框
    pub focused_border: (u32, u32),
    /// 输入选区（带透明度）
    pub selection: (u32, u32),
    /// HEAD 引用标签底色
    pub head_bg: (u32, u32),
    /// 进度条填充色
    pub progress_fill: (u32, u32),
}

/// 预置主题色。索引 0「靛蓝」为默认，其值与历史硬编码完全一致（回归保护）。
/// 其余 8 种按色相环选取，每种均手工调配浅/深与派生色，保证深色下可读性。
pub(crate) const ACCENT_PRESETS: &[(&str, AccentPalette)] = &[
    // 0 靛蓝（默认，沿用原值）
    (
        "靛蓝",
        AccentPalette {
            primary: (0x5749F4, 0x9A92FF),
            foreground: (0xFFFFFF, 0x111318),
            subtle: (0xE8E6FB, 0x302D55),
            focused_border: (0x5749F4, 0x9A92FF),
            selection: (0x5749F433, 0x9A92FF55),
            head_bg: (0x5749F4, 0x9A92FF),
            progress_fill: (0x5749F4, 0x9A92FF),
        },
    ),
    // 1 紫罗兰
    (
        "紫罗兰",
        AccentPalette {
            primary: (0x7C3AED, 0xA78BFA),
            foreground: (0xFFFFFF, 0x14101F),
            subtle: (0xEDE5FE, 0x2A1F4A),
            focused_border: (0x7C3AED, 0xA78BFA),
            selection: (0x7C3AED33, 0xA78BFA55),
            head_bg: (0x7C3AED, 0xA78BFA),
            progress_fill: (0x7C3AED, 0xA78BFA),
        },
    ),
    // 2 玫红
    (
        "玫红",
        AccentPalette {
            primary: (0xDB2777, 0xF472B6),
            foreground: (0xFFFFFF, 0x1D0A14),
            subtle: (0xFCE7F3, 0x3D1428),
            focused_border: (0xDB2777, 0xF472B6),
            selection: (0xDB277733, 0xF472B655),
            head_bg: (0xDB2777, 0xF472B6),
            progress_fill: (0xDB2777, 0xF472B6),
        },
    ),
    // 3 橙
    (
        "橙",
        AccentPalette {
            primary: (0xEA580C, 0xFB923C),
            foreground: (0xFFFFFF, 0x1D0E03),
            subtle: (0xFFEAD2, 0x3A1E08),
            focused_border: (0xEA580C, 0xFB923C),
            selection: (0xEA580C33, 0xFB923C55),
            head_bg: (0xEA580C, 0xFB923C),
            progress_fill: (0xEA580C, 0xFB923C),
        },
    ),
    // 4 青
    (
        "青",
        AccentPalette {
            primary: (0x0891B2, 0x22D3EE),
            foreground: (0xFFFFFF, 0x04212B),
            subtle: (0xCFFAFE, 0x0A333D),
            focused_border: (0x0891B2, 0x22D3EE),
            selection: (0x0891B233, 0x22D3EE55),
            head_bg: (0x0891B2, 0x22D3EE),
            progress_fill: (0x0891B2, 0x22D3EE),
        },
    ),
    // 5 翠绿
    (
        "翠绿",
        AccentPalette {
            primary: (0x16A34A, 0x4ADE80),
            foreground: (0xFFFFFF, 0x052114),
            subtle: (0xDCFCE7, 0x0D3320),
            focused_border: (0x16A34A, 0x4ADE80),
            selection: (0x16A34A33, 0x4ADE8055),
            head_bg: (0x16A34A, 0x4ADE80),
            progress_fill: (0x16A34A, 0x4ADE80),
        },
    ),
    // 6 石墨
    (
        "石墨",
        AccentPalette {
            primary: (0x374151, 0x9CA3AF),
            foreground: (0xFFFFFF, 0x111318),
            subtle: (0xE5E7EB, 0x262A33),
            focused_border: (0x374151, 0x9CA3AF),
            selection: (0x37415133, 0x9CA3AF55),
            head_bg: (0x374151, 0x9CA3AF),
            progress_fill: (0x374151, 0x9CA3AF),
        },
    ),
    // 7 金棕
    (
        "金棕",
        AccentPalette {
            primary: (0xA16207, 0xFACC15),
            foreground: (0xFFFFFF, 0x1F1804),
            subtle: (0xFEF3C7, 0x332A0E),
            focused_border: (0xA16207, 0xFACC15),
            selection: (0xA1620733, 0xFACC1555),
            head_bg: (0xA16207, 0xFACC15),
            progress_fill: (0xA16207, 0xFACC15),
        },
    ),
    // 8 天蓝
    (
        "天蓝",
        AccentPalette {
            primary: (0x2563EB, 0x60A5FA),
            foreground: (0xFFFFFF, 0x0A1430),
            subtle: (0xDBEAFE, 0x122140),
            focused_border: (0x2563EB, 0x60A5FA),
            selection: (0x2563EB33, 0x60A5FA55),
            head_bg: (0x2563EB, 0x60A5FA),
            progress_fill: (0x2563EB, 0x60A5FA),
        },
    ),
];

/// 当前激活的主题色索引（运行时可切换）。
static ACTIVE_ACCENT: AtomicU8 = AtomicU8::new(0);

/// 设置当前主题色预设索引。越界时回退到默认（靛蓝）。
pub(crate) fn set_active_accent(index: usize) {
    let clamped = if index < ACCENT_PRESETS.len() {
        index as u8
    } else {
        0
    };
    ACTIVE_ACCENT.store(clamped, Ordering::Relaxed);
}

/// 当前激活的主题色预设。
pub(crate) fn active_accent() -> &'static AccentPalette {
    let index = ACTIVE_ACCENT.load(Ordering::Relaxed) as usize;
    ACCENT_PRESETS
        .get(index)
        .map(|(_, palette)| palette)
        .unwrap_or(&ACCENT_PRESETS[0].1)
}

/// 主题色聚焦环沿用原有浅/深主题的透明度，只把 RGB 换成当前 accent 的聚焦边框色。
/// 透明度用十进制表达，避免在业务 view 中散落 RGBA 字面颜色。
const FOCUS_RING_LIGHT_ALPHA: u32 = 85;
const FOCUS_RING_DARK_ALPHA: u32 = 102;

const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color << 8) | alpha
}

/// 解析主色族 token：按当前激活的 accent 预设取浅/深值。
/// 非主色族 token 返回 None，交由静态色板处理。
fn resolve_accent_token(color: u32, variant: ThemeVariant) -> Option<u32> {
    let accent = active_accent();
    let pick = |pair: (u32, u32)| match variant {
        ThemeVariant::Light => pair.0,
        ThemeVariant::Dark => pair.1,
    };
    match color {
        PRIMARY => Some(pick(accent.primary)),
        PRIMARY_FOREGROUND => Some(pick(accent.foreground)),
        PRIMARY_SUBTLE => Some(pick(accent.subtle)),
        INPUT_BORDER_FOCUSED => Some(pick(accent.focused_border)),
        INPUT_SELECTION => Some(pick(accent.selection)),
        REF_HEAD_BG => Some(pick(accent.head_bg)),
        PROGRESS_FILL => Some(pick(accent.progress_fill)),
        STATE_FOCUS_RING => Some(with_alpha(
            pick(accent.focused_border),
            match variant {
                ThemeVariant::Light => FOCUS_RING_LIGHT_ALPHA,
                ThemeVariant::Dark => FOCUS_RING_DARK_ALPHA,
            },
        )),
        _ => None,
    }
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

/// 按 variant 解析颜色：主色族 token 走 accent 预设，其余走静态色板，字面颜色原样透传。
pub(crate) fn resolve_color_for_variant(color: u32, variant: ThemeVariant) -> u32 {
    if let Some(resolved) = resolve_accent_token(color, variant) {
        return resolved;
    }
    resolve_static_token(color, variant)
}

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

// ── Focus Workbench 尺度 token ─────────────────────────────
// 采用 4px 间距基线；名称表达密度角色，避免业务 view 散落魔法数。
// 部分 token 为后续页面 owner 预留，仍由设计系统文档和单测约束。
#[allow(dead_code)]
pub(crate) const SPACE_1: f32 = 4.0;
pub(crate) const SPACE_2: f32 = 8.0;
pub(crate) const SPACE_3: f32 = 12.0;
pub(crate) const SPACE_4: f32 = 16.0;
#[allow(dead_code)]
pub(crate) const SPACE_5: f32 = 20.0;
pub(crate) const SPACE_6: f32 = 24.0;

pub(crate) const TYPE_META: f32 = 11.0;
pub(crate) const TYPE_BODY: f32 = 12.0;
pub(crate) const TYPE_TITLE: f32 = 14.0;
pub(crate) const TYPE_PAGE_TITLE: f32 = 16.0;

#[allow(dead_code)]
pub(crate) const CONTROL_HEIGHT_COMPACT: f32 = 28.0;
pub(crate) const CONTROL_HEIGHT_REGULAR: f32 = 32.0;
pub(crate) const ROW_HEIGHT_COMPACT: f32 = 28.0;
pub(crate) const ROW_HEIGHT_REGULAR: f32 = 36.0;
pub(crate) const TITLEBAR_HEIGHT: f32 = 44.0;
/// Context Navigator 收起态窄条宽度（只剩展开箭头 + 模式图标）。
pub(crate) const NAVIGATOR_COLLAPSED_WIDTH: f32 = 48.0;

pub(crate) const RADIUS_XS: f32 = 6.0;
pub(crate) const RADIUS_SM: f32 = 8.0;
#[allow(dead_code)]
pub(crate) const RADIUS_MD: f32 = 10.0;
pub(crate) const RADIUS_PILL: f32 = 999.0;

/// 动效只用于 hover / active 的瞬态反馈；不引入会干扰桌面操作的装饰性动画。
#[allow(dead_code)]
pub(crate) const MOTION_FAST_MS: u32 = 120;
#[allow(dead_code)]
pub(crate) const MOTION_STANDARD_MS: u32 = 180;

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

    #[test]
    fn accent_presets_have_expected_count_and_default_indigo() {
        // 预置 9 种主题色
        assert_eq!(ACCENT_PRESETS.len(), 9);
        // 默认预设 0 是「靛蓝」，且其主色与历史硬编码值完全一致（回归保护）
        let (name, palette) = &ACCENT_PRESETS[0];
        assert_eq!(*name, "靛蓝");
        assert_eq!(palette.primary, (0x5749F4, 0x9A92FF));
        assert_eq!(palette.focused_border, (0x5749F4, 0x9A92FF));
        assert_eq!(palette.selection, (0x5749F433, 0x9A92FF55));
        assert_eq!(palette.head_bg, (0x5749F4, 0x9A92FF));
        assert_eq!(palette.progress_fill, (0x5749F4, 0x9A92FF));
    }

    #[test]
    fn accent_switching_changes_primary_resolution() {
        // 重置为默认，避免其他测试干扰
        set_active_accent(0);
        assert_eq!(
            resolve_color_for_variant(PRIMARY, ThemeVariant::Light),
            0x5749F4
        );
        // 切换到紫罗兰（索引 1）
        set_active_accent(1);
        assert_eq!(
            resolve_color_for_variant(PRIMARY, ThemeVariant::Light),
            0x7C3AED
        );
        assert_eq!(
            resolve_color_for_variant(PRIMARY, ThemeVariant::Dark),
            0xA78BFA
        );
        // 派生 token 也跟随
        assert_eq!(
            resolve_color_for_variant(INPUT_BORDER_FOCUSED, ThemeVariant::Light),
            0x7C3AED
        );
        assert_eq!(
            resolve_color_for_variant(PROGRESS_FILL, ThemeVariant::Dark),
            0xA78BFA
        );
        // 聚焦环复用当前 accent 主色，同时保留浅/深主题透明度。
        assert_eq!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Light),
            with_alpha(ACCENT_PRESETS[1].1.focused_border.0, FOCUS_RING_LIGHT_ALPHA)
        );
        assert_eq!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Dark),
            with_alpha(ACCENT_PRESETS[1].1.focused_border.1, FOCUS_RING_DARK_ALPHA)
        );
        // 还原默认，避免污染后续测试
        set_active_accent(0);
    }

    #[test]
    fn accent_out_of_range_falls_back_to_default() {
        set_active_accent(999);
        assert_eq!(
            resolve_color_for_variant(PRIMARY, ThemeVariant::Light),
            0x5749F4
        );
        set_active_accent(0);
    }

    #[test]
    fn non_accent_tokens_unaffected_by_accent_switch() {
        set_active_accent(0);
        let bg_light = resolve_color_for_variant(BACKGROUND, ThemeVariant::Light);
        let selection_light = resolve_color_for_variant(STATE_SELECTION, ThemeVariant::Light);
        set_active_accent(2);
        // 非主色族 token 不应受 accent 切换影响
        assert_eq!(
            resolve_color_for_variant(BACKGROUND, ThemeVariant::Light),
            bg_light
        );
        assert_eq!(
            resolve_color_for_variant(STATE_SELECTION, ThemeVariant::Light),
            selection_light
        );
        set_active_accent(0);
    }

    #[test]
    fn focus_ring_follows_accent_without_changing_static_state_tokens() {
        set_active_accent(0);
        let indigo_ring_light = resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Light);
        let indigo_ring_dark = resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Dark);
        let indigo_hover = resolve_color_for_variant(STATE_HOVER, ThemeVariant::Light);
        let indigo_selection = resolve_color_for_variant(STATE_SELECTION, ThemeVariant::Dark);

        set_active_accent(4);
        assert_eq!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Light),
            with_alpha(ACCENT_PRESETS[4].1.focused_border.0, FOCUS_RING_LIGHT_ALPHA)
        );
        assert_eq!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Dark),
            with_alpha(ACCENT_PRESETS[4].1.focused_border.1, FOCUS_RING_DARK_ALPHA)
        );
        assert_ne!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Light),
            indigo_ring_light
        );
        assert_ne!(
            resolve_color_for_variant(STATE_FOCUS_RING, ThemeVariant::Dark),
            indigo_ring_dark
        );
        // hover/selection 等非 accent 状态 token 保持原语义色。
        assert_eq!(
            resolve_color_for_variant(STATE_HOVER, ThemeVariant::Light),
            indigo_hover
        );
        assert_eq!(
            resolve_color_for_variant(STATE_SELECTION, ThemeVariant::Dark),
            indigo_selection
        );
        set_active_accent(0);
    }

    #[test]
    fn focus_workbench_tokens_keep_density_and_theme_layers() {
        assert_eq!(SPACE_1, 4.0);
        assert_eq!(SPACE_6, 24.0);
        assert_eq!(TITLEBAR_HEIGHT, 44.0);
        assert_eq!(NAVIGATOR_COLLAPSED_WIDTH, 48.0);
        assert!(CONTROL_HEIGHT_COMPACT < CONTROL_HEIGHT_REGULAR);
        assert!(ROW_HEIGHT_COMPACT < ROW_HEIGHT_REGULAR);
        assert_ne!(
            resolve_color_for_variant(SURFACE_CANVAS, ThemeVariant::Light),
            resolve_color_for_variant(SURFACE_CANVAS, ThemeVariant::Dark)
        );
        assert_ne!(
            resolve_color_for_variant(STATE_HOVER, ThemeVariant::Light),
            resolve_color_for_variant(STATE_SELECTION, ThemeVariant::Light)
        );
    }
}
