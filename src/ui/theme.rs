// Khaslana 主题色 — Minimal Ink 色板
// 对应 pencil 设计图中的 CSS 变量系统

// ── 基础色板 ──────────────────────────────────────────────
pub(crate) const BACKGROUND: u32 = 0xffffff;
pub(crate) const FOREGROUND: u32 = 0x2A2933;
pub(crate) const CARD: u32 = 0xffffff;
pub(crate) const CARD_FOREGROUND: u32 = 0x2A2933;
pub(crate) const PRIMARY: u32 = 0x5749F4;
pub(crate) const PRIMARY_FOREGROUND: u32 = 0xffffff;
/// PRIMARY 的浅色变体（~10% 不透明度叠加白色），用于选中行/hover 行背景
pub(crate) const PRIMARY_SUBTLE: u32 = 0xE8E6FB;
pub(crate) const SECONDARY: u32 = 0xD9D9DB;
pub(crate) const SECONDARY_FOREGROUND: u32 = 0x2A2933;
pub(crate) const MUTED_FOREGROUND: u32 = 0x616167;
pub(crate) const ACCENT: u32 = 0xF5F5F5;
pub(crate) const ACCENT_FOREGROUND: u32 = 0x2A2933;
pub(crate) const DESTRUCTIVE: u32 = 0xCC3314;
pub(crate) const DESTRUCTIVE_FOREGROUND: u32 = 0xffffff;
pub(crate) const BORDER: u32 = 0xC5C5CB;
pub(crate) const INPUT: u32 = 0xC5C5CB;
pub(crate) const WHITE: u32 = 0xffffff;
pub(crate) const POPOVER: u32 = 0xffffff;
pub(crate) const POPOVER_FOREGROUND: u32 = 0x2A2933;
pub(crate) const TILE: u32 = 0xF5F5F5;

// ── 侧边栏 ───────────────────────────────────────────────
pub(crate) const SIDEBAR: u32 = 0xffffff;
pub(crate) const SIDEBAR_FOREGROUND: u32 = 0x939399;
pub(crate) const SIDEBAR_ACCENT: u32 = 0xF5F5F5;
pub(crate) const SIDEBAR_ACCENT_FOREGROUND: u32 = 0x2A2933;
pub(crate) const SIDEBAR_PRIMARY_FOREGROUND: u32 = 0x2A2933;
pub(crate) const SIDEBAR_BORDER: u32 = 0xD9D9DB;

// ── Git 状态色 ─────────────────────────────────────────────
pub(crate) const GIT_ADDED: u32 = 0x2DA44E;
pub(crate) const GIT_MODIFIED: u32 = 0x0066FF;
pub(crate) const GIT_REMOVED: u32 = 0xCF222E;
pub(crate) const GIT_UNTRACKED: u32 = 0x8B949E;

// ── Diff 配色 ─────────────────────────────────────────────
pub(crate) const DIFF_ADDED_BG: u32 = 0xE6F4EA;
pub(crate) const DIFF_ADDED_TEXT: u32 = 0x1A7F37;
pub(crate) const DIFF_REMOVED_BG: u32 = 0xFFEBE9;
pub(crate) const DIFF_REMOVED_TEXT: u32 = 0xCF222E;
pub(crate) const DIFF_HEADER_BG: u32 = 0xF5F5F5;
pub(crate) const DIFF_HEADER_TEXT: u32 = 0x616167;

// ── 语义状态色 ────────────────────────────────────────────
pub(crate) const COLOR_ERROR: u32 = 0xFFBFB2;
pub(crate) const COLOR_ERROR_FOREGROUND: u32 = 0x590F00;
pub(crate) const COLOR_SUCCESS: u32 = 0xA1E5A1;
pub(crate) const COLOR_SUCCESS_FOREGROUND: u32 = 0x003300;
pub(crate) const COLOR_WARNING: u32 = 0xFFD9B2;
pub(crate) const COLOR_WARNING_FOREGROUND: u32 = 0x4D2700;
pub(crate) const COLOR_INFO: u32 = 0xC9D6F0;
pub(crate) const COLOR_INFO_FOREGROUND: u32 = 0x001133;

// ── 输入框 ────────────────────────────────────────────────
pub(crate) const INPUT_BG: u32 = 0xffffff;
pub(crate) const INPUT_BG_FOCUSED: u32 = 0xffffff;
pub(crate) const INPUT_BORDER: u32 = 0xC5C5CB;
pub(crate) const INPUT_BORDER_FOCUSED: u32 = 0x5749F4;
pub(crate) const INPUT_PLACEHOLDER: u32 = 0x616167;
pub(crate) const INPUT_SELECTION: u32 = 0x5749F433;
pub(crate) const INPUT_CARET: u32 = 0x2A2933;

// ── 弹窗和对话 ───────────────────────────────────────────
pub(crate) const DIALOG_OVERLAY: u32 = 0x0f172a55;
pub(crate) const TOOLTIP_BG: u32 = 0x2A2933;
pub(crate) const TOOLTIP_BORDER: u32 = 0x3A3955;

// ── 提交历史图颜色 ────────────────────────────────────────
pub(crate) const HISTORY_GRAPH_COLORS: [u32; 8] = [
    0xf97316, 0x14b8a6, 0x3b82f6, 0xeab308, 0xef4444, 0x8b5cf6, 0x22c55e, 0xec4899,
];

// ── 引用标签色 ────────────────────────────────────────────
pub(crate) const REF_LOCAL_BG: u32 = 0xE6F4EA;
pub(crate) const REF_LOCAL_BORDER: u32 = 0x2DA44E;
pub(crate) const REF_LOCAL_TEXT: u32 = 0x1A7F37;
pub(crate) const REF_REMOTE_BG: u32 = 0xC9D6F0;
pub(crate) const REF_REMOTE_BORDER: u32 = 0x0066FF;
pub(crate) const REF_REMOTE_TEXT: u32 = 0x003399;
pub(crate) const REF_TAG_BG: u32 = 0xFFF2D9;
pub(crate) const REF_TAG_BORDER: u32 = 0xD4A72C;
pub(crate) const REF_TAG_TEXT: u32 = 0x7C5800;
pub(crate) const REF_HEAD_BG: u32 = 0x5749F4;
pub(crate) const REF_HEAD_TEXT: u32 = 0xffffff;

// ── 反馈/Toast ────────────────────────────────────────────
pub(crate) const FEEDBACK_INFO_BG: u32 = 0xC9D6F0;
pub(crate) const FEEDBACK_INFO_BORDER: u32 = 0x0066FF;
pub(crate) const FEEDBACK_INFO_TEXT: u32 = 0x001133;
pub(crate) const FEEDBACK_SUCCESS_BG: u32 = 0xA1E5A1;
pub(crate) const FEEDBACK_SUCCESS_BORDER: u32 = 0x2DA44E;
pub(crate) const FEEDBACK_SUCCESS_TEXT: u32 = 0x003300;
pub(crate) const FEEDBACK_WARNING_BG: u32 = 0xFFD9B2;
pub(crate) const FEEDBACK_WARNING_BORDER: u32 = 0xD4A72C;
pub(crate) const FEEDBACK_WARNING_TEXT: u32 = 0x4D2700;
pub(crate) const FEEDBACK_ERROR_BG: u32 = 0xFFBFB2;
pub(crate) const FEEDBACK_ERROR_BORDER: u32 = 0xCF222E;
pub(crate) const FEEDBACK_ERROR_TEXT: u32 = 0x590F00;

// ── 进度条 ────────────────────────────────────────────────
pub(crate) const PROGRESS_TRACK: u32 = 0xE0E0E3;
pub(crate) const PROGRESS_FILL: u32 = 0x5749F4;

// ── 滚动条 ────────────────────────────────────────────────
pub(crate) const SCROLLBAR_TRACK: u32 = 0xF5F5F5CC;
pub(crate) const SCROLLBAR_THUMB: u32 = 0xC5C5CBDD;
pub(crate) const SCROLLBAR_THUMB_ACTIVE: u32 = 0x939399EE;

// ── 圆角常量 ──────────────────────────────────────────────
pub(crate) const RADIUS_XS: f32 = 6.0;
pub(crate) const RADIUS_PILL: f32 = 999.0;
