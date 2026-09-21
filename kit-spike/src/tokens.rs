//! 工作区样板的设计 token。
//!
//! 取值来源：`docs/gpui-kit-visual-spec.md` 与 Pencil 画板 `Sgcwi`（无暂存）、
//! `a4JtW`（有暂存）、`GuA1a`（侧栏收起）、`lESCi`（组件状态），逐项按画板实测数值。
//! 视图只引用这里的语义名，不散写颜色，方便后续替换为真实主题变量。

use gpui_kit::{BoxShadow, Hsla, px, rgba};

/// `0xRRGGBBAA` → `Hsla`。
fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

// ---------------------------------------------------------------- 表面与层级

/// 环境底：应用内部背景。
pub fn env_bg() -> Hsla {
    c(0xF1F5FD_FF)
}

/// 顶部工具栏底色。
pub fn toolbar_bg() -> Hsla {
    c(0xF9FBFF_FF)
}

/// 导航面板：画板为 75% 不透明浅面。
pub fn nav_surface() -> Hsla {
    c(0xF9FBFF_E6)
}

/// 工作面板：文件列表与差异外壳。
pub fn panel() -> Hsla {
    c(0xFFFF_FFFF)
}

/// 悬浮面板：提交区使用 92% 白。
pub fn panel_veil() -> Hsla {
    c(0xFFFF_FFE8)
}

/// 输入区浅面（提交输入框、全局搜索）。
pub fn input_surface() -> Hsla {
    c(0xF5F8FD_FF)
}

/// 导航模式选中底。
pub fn selected_nav() -> Hsla {
    c(0xDDEAFF_FF)
}

/// 文件行选中底。
pub fn selected_row() -> Hsla {
    c(0xE7F0FF_FF)
}

/// 列表行 hover 底（画板未给，取同族更浅一档）。
pub fn hover_row() -> Hsla {
    c(0xEFF4FD_FF)
}

/// 分支行选中底。
pub fn selected_branch() -> Hsla {
    c(0xE7EEFC_FF)
}

// ------------------------------------------------------------------ 文字层级

/// 页面主标题与品牌字。
pub fn text_heading() -> Hsla {
    c(0x172447_FF)
}

/// 面板标题、文件名、被强调的正文。
pub fn text_strong() -> Hsla {
    c(0x25385E_FF)
}

/// 代码与普通正文。
pub fn text_body() -> Hsla {
    c(0x344A71_FF)
}

/// 标题栏命令文字与页面统计（比正文稍深）。
pub fn text_secondary() -> Hsla {
    c(0x425782_FF)
}

/// 导航条目文字。
pub fn text_nav() -> Hsla {
    c(0x526585_FF)
}

/// 弱化说明文字。
pub fn text_muted() -> Hsla {
    c(0x7182A1_FF)
}

/// 次级说明与占位说明。
pub fn text_subtle() -> Hsla {
    c(0x7383A0_FF)
}

/// 文件行的增删统计。
pub fn text_stats() -> Hsla {
    c(0x6883A0_FF)
}

/// 底部状态栏。
pub fn text_status() -> Hsla {
    c(0x7D8DA8_FF)
}

/// 差异行号。
pub fn text_line_no() -> Hsla {
    c(0x8B99AF_FF)
}

/// 输入占位文字。
pub fn text_placeholder() -> Hsla {
    c(0x8A98AF_FF)
}

/// hunk 头文字。
pub fn text_hunk() -> Hsla {
    c(0x7D8EAA_FF)
}

// ---------------------------------------------------------------- 品牌与状态

pub fn primary() -> Hsla {
    c(0x3478F6_FF)
}

pub fn primary_hover() -> Hsla {
    c(0x4286FF_FF)
}

pub fn primary_active() -> Hsla {
    c(0x2867D9_FF)
}

pub fn primary_foreground() -> Hsla {
    c(0xFFFF_FFFF)
}

/// 导航选中项的文字与图标（比主色更深一档）。
pub fn nav_selected() -> Hsla {
    c(0x246AF0_FF)
}

/// 分支选中文字。
pub fn branch_selected() -> Hsla {
    c(0x2866D6_FF)
}

/// 新增统计文字。
pub fn add_text() -> Hsla {
    c(0x16846C_FF)
}

/// 删除统计文字。
pub fn del_text() -> Hsla {
    c(0xCC6175_FF)
}

/// 修改状态徽标。
pub fn badge_modified() -> Hsla {
    c(0xC38B42_FF)
}

/// 新增状态徽标。
pub fn badge_added() -> Hsla {
    c(0x3478F6_FF)
}

// -------------------------------------------------------------------- 差异色

pub fn diff_add_bg() -> Hsla {
    c(0xE9F8F1_FF)
}

pub fn diff_del_bg() -> Hsla {
    c(0xFCEEF1_FF)
}

pub fn diff_add_fg() -> Hsla {
    c(0x18775F_FF)
}

pub fn diff_del_fg() -> Hsla {
    c(0xB55570_FF)
}

pub fn diff_hunk_bg() -> Hsla {
    c(0xF0F5FD_FF)
}

// ---------------------------------------------------------------------- 描边

pub fn border() -> Hsla {
    c(0xC7D3E7_FF)
}

pub fn border_soft() -> Hsla {
    c(0xE3EAF6_FF)
}

pub fn focus_ring() -> Hsla {
    c(0x7BA8F6_FF)
}

// ---------------------------------------------------------------------- 控件

/// 次级/禁用按钮底（提交到分支的浅面）。
pub fn control_quiet() -> Hsla {
    c(0xEDF1F7_FF)
}

// ---------------------------------------------------------------------- 阴影

/// 工作面板阴影：向下 5px、模糊 22px、冷色 6%。
pub fn panel_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(5.), c(0x3A5D98_10)).blur_radius(px(22.))]
}

/// 普通控件接触阴影：向下 2px、模糊 5px。
pub fn control_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(2.), c(0x315183_0D)).blur_radius(px(5.))]
}

/// 主按钮阴影：向下 4px、模糊 8px、主色 15%。
pub fn primary_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(4.), c(0x3478F6_25)).blur_radius(px(8.))]
}

// ---------------------------------------------------------------------- 节奏

/// 外壳与面板圆角。
pub const RADIUS_PANEL: f32 = 16.;
/// 导航外壳圆角。
pub const RADIUS_NAV: f32 = 18.;
/// 控件圆角（按钮、输入、分段）。
pub const RADIUS_CONTROL: f32 = 10.;
/// 列表行圆角。
pub const RADIUS_ROW: f32 = 9.;

/// 面板间距。
pub const GAP_PANEL: f32 = 16.;

/// 正文（中文）字号。
pub const FONT_BODY: f32 = 13.;
/// 面板标题字号。
pub const FONT_PANEL_TITLE: f32 = 14.;
/// 页面主标题字号。
pub const FONT_PAGE_TITLE: f32 = 26.;
/// 元信息字号。
pub const FONT_META: f32 = 12.;
/// 最小元信息字号。
pub const FONT_MICRO: f32 = 11.;
/// 代码字号。
pub const FONT_CODE: f32 = 12.;
/// 工具栏控件高度。
pub const H_CONTROL: f32 = 36.;
/// 文件行高度。
pub const H_FILE_ROW: f32 = 44.;
/// 差异代码行高。
pub const H_DIFF_LINE: f32 = 24.;
