// 本文件是项目级设计系统 helper 库（按钮、徽标、对话框外壳、toast 等），
// 其中部分控件为预留的公共 API，可能暂未被业务 view 调用，属于有意保留。
#![allow(dead_code)]

use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::ui::theme::{rgb, rgba};
use crate::{
    RepositoryView,
    ui::{icons::ToolbarIcon, icons::toolbar_icon, theme},
};
use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, Div, IntoElement, MouseButton, Pixels,
    Render, SharedString, Stateful, Window, div, prelude::*, px,
};
use gpui_kit::base::{Button as BaseButton, Checkbox, CheckboxState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppToastKind {
    Info,
    Success,
    Warning,
    Error,
}

/// 通知气泡的可选点击动作：点击气泡体触发；点 ✕ 只关闭不触发
///（关闭按钮处理器已 `stop_propagation`，事件不会落入气泡体的点击）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastAction {
    OpenUpdateSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct FeedbackMessage {
    pub(crate) id: u64,
    pub(crate) kind: AppToastKind,
    pub(crate) title: &'static str,
    pub(crate) message: String,
    pub(crate) expires_at: Instant,
    /// 点击气泡体时执行的动作；None = 纯文本气泡（既有全部路径）。
    pub(crate) action: Option<ToastAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonTone {
    /// 普通命令：浅灰薄实体 + 常规轮廓（顶栏、对话框、列表操作）。
    Neutral,
    /// 次级动作：白/近白薄实体 + 弱轮廓，与蓝色主按钮并列时表达主次
    /// （最新 Pencil 稿的「提交到 &lt;分支&gt;」）。
    Secondary,
    /// 主动作：蓝色实面 + 柔和投影（「提交并推送」）。
    Primary,
    /// 危险动作：危险色实面，独立语义，不参与主次排序。
    Danger,
}

#[derive(Clone, Copy, Debug)]
struct ButtonPalette {
    bg: u32,
    hover_bg: u32,
    fg: u32,
    border: u32,
    /// 是否带接触阴影（视觉规范 §3.1：按钮用细腻接触阴影体现厚度）。
    /// 主/次动作是「抬起的实体」，普通命令与危险色保持平整。
    contact_shadow: bool,
}

struct TextTooltip {
    text: gpui::SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputFrameSize {
    Compact,
    Regular,
    Multiline,
}

impl AppToastKind {
    fn label(self) -> &'static str {
        match self {
            AppToastKind::Info => "提示",
            AppToastKind::Success => "完成",
            AppToastKind::Warning => "注意",
            AppToastKind::Error => "失败",
        }
    }

    fn palette(self) -> (u32, u32, u32) {
        match self {
            AppToastKind::Info => (
                theme::FEEDBACK_INFO_BG,
                theme::FEEDBACK_INFO_BORDER,
                theme::FEEDBACK_INFO_TEXT,
            ),
            AppToastKind::Success => (
                theme::FEEDBACK_SUCCESS_BG,
                theme::FEEDBACK_SUCCESS_BORDER,
                theme::FEEDBACK_SUCCESS_TEXT,
            ),
            AppToastKind::Warning => (
                theme::FEEDBACK_WARNING_BG,
                theme::FEEDBACK_WARNING_BORDER,
                theme::FEEDBACK_WARNING_TEXT,
            ),
            AppToastKind::Error => (
                theme::FEEDBACK_ERROR_BG,
                theme::FEEDBACK_ERROR_BORDER,
                theme::FEEDBACK_ERROR_TEXT,
            ),
        }
    }

    pub(crate) fn is_important(self) -> bool {
        matches!(self, AppToastKind::Warning | AppToastKind::Error)
    }
}

impl FeedbackMessage {
    pub(crate) fn new(id: u64, kind: AppToastKind, message: String) -> Self {
        let ttl = if kind.is_important() {
            Duration::from_secs(7)
        } else {
            Duration::from_secs(4)
        };
        Self {
            id,
            kind,
            title: kind.label(),
            message,
            expires_at: Instant::now() + ttl,
            action: None,
        }
    }

    pub(crate) fn with_toast_action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    pub(crate) fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(280.0))
            .px_2()
            .py_1()
            .rounded(px(theme::RADIUS_XS))
            .border_1()
            .border_color(rgb(theme::TOOLTIP_BORDER))
            .bg(rgb(theme::TOOLTIP_BG))
            .text_color(rgb(theme::WHITE))
            .text_size(px(12.0))
            .line_height(px(18.0))
            .shadow_lg()
            .child(self.text.clone())
    }
}

pub(crate) fn tooltip_text(text: impl Into<gpui::SharedString>, cx: &mut App) -> gpui::AnyView {
    let text = text.into();
    cx.new(move |_| TextTooltip { text }).into()
}

fn app_button_palette(tone: ButtonTone, enabled: bool) -> ButtonPalette {
    if !enabled {
        return ButtonPalette {
            bg: theme::STATE_HOVER,
            hover_bg: theme::STATE_HOVER,
            fg: theme::CONTENT_SECONDARY,
            border: theme::BORDER_MUTED,
            // 禁用态保持平整：阴影会削弱「此时不可点」的信号。
            contact_shadow: false,
        };
    }

    match tone {
        ButtonTone::Neutral => ButtonPalette {
            bg: theme::STATE_HOVER,
            hover_bg: theme::WB_ROW_HOVER,
            fg: theme::CONTENT_PRIMARY,
            border: theme::BORDER_MUTED,
            contact_shadow: false,
        },
        ButtonTone::Secondary => ButtonPalette {
            bg: theme::WB_PANEL,
            hover_bg: theme::WB_ROW_HOVER,
            fg: theme::CONTENT_PRIMARY,
            border: theme::BORDER_MUTED,
            contact_shadow: true,
        },
        ButtonTone::Primary => ButtonPalette {
            bg: theme::PRIMARY,
            hover_bg: theme::PRIMARY,
            fg: theme::PRIMARY_FOREGROUND,
            border: theme::PRIMARY,
            contact_shadow: true,
        },
        ButtonTone::Danger => ButtonPalette {
            bg: theme::DESTRUCTIVE,
            hover_bg: theme::DESTRUCTIVE,
            fg: theme::DESTRUCTIVE_FOREGROUND,
            border: theme::DESTRUCTIVE,
            contact_shadow: false,
        },
    }
}

/// 区域标题 — Funnel Sans 风格小标题（侧边栏、面板区头等）
pub(crate) fn section_label(title: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(theme::CONTENT_SECONDARY))
        .child(title)
}

/// 小圆角药丸徽标（如变更计数 "4"、状态字母 "M"）
pub(crate) fn pill_badge(
    text: impl Into<gpui::SharedString>,
    bg: u32,
    fg: u32,
) -> impl IntoElement {
    div()
        .flex_none()
        .rounded(px(theme::RADIUS_XS))
        .bg(rgb(bg))
        .px(px(6.0))
        .py(px(1.0))
        .justify_center()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(fg))
        .child(text.into())
}

/// 状态 pill — 如 diff 标题中的"已修改"
pub(crate) fn status_pill_badge(
    text: impl Into<gpui::SharedString>,
    bg: u32,
    fg: u32,
) -> impl IntoElement {
    div()
        .flex_none()
        .rounded(px(theme::RADIUS_PILL))
        .bg(rgb(bg))
        .px(px(8.0))
        .py(px(2.0))
        .justify_center()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .child(text.into())
}

pub(crate) fn section_title(title: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .px_2()
        .py_2()
        .border_b_1()
        .border_color(rgb(theme::BORDER_MUTED))
        .bg(rgb(theme::WB_PANEL))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(theme::CONTENT_SECONDARY))
        .child(title)
}

/// 应用面板 — 扁平无装饰纯色容器
pub(crate) fn app_panel() -> Div {
    flat_panel()
}

/// 应用外壳 — 环境底色 + 窗口圆角；面板浮在其上（悬浮工作台视觉层，M3 起）。
///
/// 窗口背景是透明的（`WindowBackgroundAppearance::Transparent`），外壳自己就是那张
/// 圆角卡片：根背景按 `window_radius()` 画圆角，`overflow_hidden` 只保证子元素不越出矩形，
/// **圆角本身裁不住子元素**（gpui 的 `ContentMask` 只有 bounds），所以凡是铺满窗口的
/// 子元素（顶栏、全屏遮罩）都必须自己带同样的圆角。
pub(crate) fn app_shell_surface() -> Div {
    div()
        .relative()
        .size_full()
        .rounded(px(theme::window_radius()))
        .overflow_hidden()
        .bg(rgb(theme::WB_ENV))
}

/// 工作面板阴影：向下 5px、模糊 22px、冷色低透明度。
///
/// 阴影按面板数量增长，不能按行或代码块增长；行内与代码区不加阴影。
pub(crate) fn panel_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.0), px(5.0), rgba(theme::WB_SHADOW_PANEL).into())
            .blur_radius(px(22.0)),
    ]
}

/// 控件接触阴影：向下 2px、模糊 5px，用于顶栏命令与浮起的小实体。
pub(crate) fn control_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.0), px(2.0), rgba(theme::WB_SHADOW_CONTROL).into())
            .blur_radius(px(5.0)),
    ]
}

/// 按下时的接触阴影：比默认更短，表达「压下」（视觉规范 §4）。
/// 阴影收短不改变布局尺寸，因此按压不会引起命中区跳动。
pub(crate) fn pressed_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.0), px(1.0), rgba(theme::WB_SHADOW_CONTROL).into())
            .blur_radius(px(2.0)),
    ]
}

/// 悬浮面板 — 圆角抬起的独立实体（导航外壳、文件列表、差异外壳、提交区）。
///
/// 面板内需要裁掉子元素的方角（列表底、代码底色），所以这里直接带
/// `overflow_hidden`；面板内容若要有自己的滚动容器，请放在面板内部而不是外层，
/// 否则阴影会被父级裁切。
pub(crate) fn floating_panel() -> Div {
    div()
        .rounded(px(theme::RADIUS_PANEL))
        .bg(rgb(theme::WB_PANEL))
        .shadow(panel_shadow())
        .overflow_hidden()
}

/// 导航外壳 — 比工作面板大一档圆角、半透明底色（画板：75% 浅面）。
pub(crate) fn navigator_panel() -> Div {
    div()
        .rounded(px(theme::RADIUS_NAV))
        .bg(rgb(theme::WB_NAV))
        .shadow(panel_shadow())
        .overflow_hidden()
}

/// 设置页卡片 — 标题 + 一句说明 + 内容区（M3 设置页样板）。
///
/// 与 `floating_panel` 的区别：不裁子元素（设置页里的浮层、tooltip
/// 不能被卡片边界切掉），卡片之间靠 16px 留白分隔。
pub(crate) fn settings_card(title: &'static str, description: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .gap(px(theme::SPACE_3))
        .p(px(theme::SPACE_4))
        .rounded(px(theme::RADIUS_PANEL))
        .bg(rgb(theme::WB_PANEL))
        .shadow(panel_shadow())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_1))
                .child(
                    div()
                        .text_size(px(theme::TYPE_TITLE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::CONTENT_PRIMARY))
                        .child(title),
                )
                .when_some(description, |this, text| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_BODY))
                            .line_height(px(18.0))
                            .text_color(rgb(theme::CONTENT_SECONDARY))
                            .child(text),
                    )
                }),
        )
}

/// 设置页分段选择器外壳：浅凹底 + 内边距，里面放 `segmented_button`。
pub(crate) fn settings_segmented_group() -> Div {
    div()
        .flex()
        .w_full()
        .gap(px(theme::SPACE_2))
        .p(px(theme::SPACE_1))
        .rounded(px(theme::RADIUS_MD))
        .bg(rgb(theme::WB_INPUT_SURFACE))
}

/// 扁平面板 — 无装饰纯色容器，无边框无阴影
pub(crate) fn flat_panel() -> Div {
    div()
}

/// 页面标题行：标题、说明和命令组保持同一条基线，适合壳层和后续独立页面复用。
///
/// 只用于页面/悬浮面板顶部：底色带上与面板一致的顶部圆角，避免方角盖住
/// 面板圆角（gpui 的 overflow_hidden 裁不住圆角）。
pub(crate) fn page_header(title: &'static str, description: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(theme::SPACE_3))
        .min_h(px(40.0))
        .px(px(theme::SPACE_4))
        .rounded_t(px(theme::RADIUS_PANEL))
        .border_b_1()
        .border_color(rgb(theme::BORDER_MUTED))
        .bg(rgb(theme::SURFACE_BASE))
        .child(
            div()
                .min_w(px(0.0))
                .flex()
                .items_baseline()
                .gap(px(theme::SPACE_2))
                .child(
                    div()
                        .text_size(px(theme::TYPE_PAGE_TITLE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::CONTENT_PRIMARY))
                        .child(title),
                )
                .when_some(description, |this, description| {
                    this.child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(theme::TYPE_BODY))
                            .text_color(rgb(theme::CONTENT_SECONDARY))
                            .child(description),
                    )
                }),
        )
}

/// 命令组：只负责紧凑排列，不携带阴影或容器边框。
pub(crate) fn command_group() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(theme::SPACE_1))
}

/// 空状态：使用克制的图文层级而不是卡片堆叠。
///
/// 区块级空态（无仓库、无差异内容）通过 builder 构建，可带一行可执行
/// 命令（视觉规范 §2：「无仓库」「干净工作区」等状态要有明确入口，
/// 不能只有文字）；`EmptyState::fill` 让空态在剩余空间里居中呈现。
/// 列表内占位（正在加载/暂无数据）继续走 `panel_empty_row`，
/// 保持行高节奏不破坏虚拟列表测量。
pub(crate) fn empty_state(title: &'static str, detail: &'static str) -> Div {
    EmptyState::new(title).detail(detail).build()
}

/// 带动作行的空状态 builder：`empty_state(..)` 是无动作的快捷入口。
pub(crate) struct EmptyState {
    title: &'static str,
    detail: Option<&'static str>,
    actions: Vec<AnyElement>,
    fill: bool,
}

impl EmptyState {
    pub(crate) fn new(title: &'static str) -> Self {
        Self {
            title,
            detail: None,
            actions: Vec::new(),
            fill: false,
        }
    }

    /// 说明文字（title 下方的一句补充）。
    pub(crate) fn detail(mut self, detail: &'static str) -> Self {
        self.detail = Some(detail);
        self
    }

    /// 追加一个命令槽元素（通常是一枚按钮）。
    pub(crate) fn action(mut self, action: AnyElement) -> Self {
        self.actions.push(action);
        self
    }

    /// 占满剩余空间并垂直居中：用于页面级/区块级空态（无仓库、未选中文件）。
    /// 不开启时按内容自适应高度，兼容列表内的局部占位。
    pub(crate) fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub(crate) fn build(self) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(theme::SPACE_2))
            .p(px(theme::SPACE_6))
            .when(self.fill, |this| this.flex_1().min_h(px(0.0)))
            .text_align(gpui::TextAlign::Center)
            .child(
                div()
                    .text_size(px(theme::TYPE_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::CONTENT_PRIMARY))
                    .child(self.title),
            )
            .when_some(self.detail, |this, detail| {
                this.child(
                    div()
                        .max_w(px(360.0))
                        .text_size(px(theme::TYPE_BODY))
                        .line_height(px(18.0))
                        .text_color(rgb(theme::CONTENT_SECONDARY))
                        .child(detail),
                )
            })
            .when(!self.actions.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(theme::SPACE_2))
                        // 顶部留半档间距：动作行与说明文字之间保持呼吸感，
                        // 不会在无说明时凭空多出一段。
                        .mt(px(theme::SPACE_1))
                        .children(self.actions),
                )
            })
    }
}

/// 通用图标按钮视觉底座。调用方追加 `on_click`；禁用态自动保留原因提示。
/// 使用 Kit 基础按钮承载焦点、Tab 导航及 Enter/Space 激活语义。
pub(crate) fn icon_button(
    id: String,
    icon_kind: ToolbarIcon,
    label: &'static str,
    enabled: bool,
) -> BaseButton {
    let tooltip = if enabled {
        label
    } else {
        "当前状态不可用"
    };
    BaseButton::new(id)
        .disabled(!enabled)
        .accessibility_label(label)
        .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
        .flex_none()
        .size(px(theme::CONTROL_HEIGHT_REGULAR))
        .rounded(px(theme::RADIUS_XS))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(if enabled {
            theme::CONTENT_SECONDARY
        } else {
            theme::CONTENT_TERTIARY
        }))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                .active(|this| this.opacity(0.8))
        })
        .when(!enabled, |this| this.cursor_not_allowed().opacity(0.55))
        .tooltip(move |_window, cx| tooltip_text(tooltip, cx))
        .child(toolbar_icon(
            icon_kind,
            if enabled {
                theme::CONTENT_SECONDARY
            } else {
                theme::CONTENT_TERTIARY
            },
        ))
}

fn icon_command_button_disabled_reason(label: &'static str) -> &'static str {
    match label {
        "更多命令" => "当前操作进行中，暂不可展开更多命令",
        "设置" => "当前操作进行中，暂不可打开设置",
        "展开上下文导航" | "收起上下文导航" => "此页面不显示导航区",
        _ => "当前状态不可用",
    }
}

/// 图标命令按钮：由 Kit 基础按钮统一提供焦点、Tab 导航及 Enter/Space 激活。
pub(crate) fn icon_command_button(
    id: String,
    icon_kind: ToolbarIcon,
    label: &'static str,
    enabled: bool,
    on_activate: impl Fn(&mut RepositoryView, &mut Window, &mut Context<RepositoryView>) + 'static,
    cx: &mut Context<RepositoryView>,
) -> BaseButton {
    let tooltip = if enabled {
        label
    } else {
        icon_command_button_disabled_reason(label)
    };
    BaseButton::new(id)
        .disabled(!enabled)
        .accessibility_label(label)
        .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
        .flex_none()
        .size(px(theme::CONTROL_HEIGHT_REGULAR))
        .rounded(px(theme::RADIUS_XS))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(if enabled {
            theme::CONTENT_SECONDARY
        } else {
            theme::CONTENT_TERTIARY
        }))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                .active(|this| this.opacity(0.8))
        })
        .when(!enabled, |this| this.cursor_not_allowed().opacity(0.55))
        .tooltip(move |_window, cx| tooltip_text(tooltip, cx))
        .on_click(cx.listener(move |this, _event, window, cx| {
            if enabled {
                on_activate(this, window, cx);
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
}

/// 玻璃面板 — 保留给弹窗/上下文菜单等需要浮层效果的场景
pub(crate) fn glass_panel() -> Div {
    div()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::BORDER_MUTED))
        .bg(rgb(theme::WB_PANEL))
        .shadow_lg()
}

/// 菜单容器 — 弹出菜单使用
pub(crate) fn glass_menu() -> Div {
    glass_panel()
        .py_1()
        .flex()
        .flex_col()
        .text_size(px(12.0))
        .occlude()
}

/// 计数徽标 — 旧版 metric_badge，保留接口但更新配色
pub(crate) fn metric_badge(label: impl Into<gpui::SharedString>, tone: u32) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(px(2.0))
        .rounded_full()
        .border_1()
        .border_color(rgb(tone))
        .bg(rgb(theme::STATE_HOVER))
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(tone))
        .child(label.into())
}

/// 对话框遮罩层
pub(crate) fn dialog_overlay() -> Div {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        // 遮罩铺满整窗，圆角必须与外壳一致，否则窗口圆角外会留下一圈灰色。
        .rounded(px(theme::window_radius()))
        .bg(rgba(theme::DIALOG_OVERLAY))
        .cursor(CursorStyle::Arrow)
        .occlude()
}

/// 弹窗面板与窗口边缘之间的可见间距（审查 R3 的安全边距）。
const DIALOG_PANEL_VIEWPORT_MARGIN: f32 = 24.0;
/// 面板尺寸下限：极端小窗（高 DPI 下逻辑视口更小）也不缩到这个尺寸以下，
/// 保证标题、关闭与底部操作行始终可用。
const DIALOG_PANEL_MIN_WIDTH: f32 = 480.0;
const DIALOG_PANEL_MIN_HEIGHT: f32 = 320.0;

/// 弹窗面板尺寸按视口钳制：`min(设计尺寸, 可用空间)`。
///
/// 最小窗 860×520（125% DPI 下逻辑视口只有 688×416）里，大于视口的固定
/// 尺寸会被根圆角裁掉——标题/关闭与底部内容点不到，内部滚动也修不了
/// 整个面板越界（审查 R3）。
pub(crate) fn dialog_panel_size(
    window: &Window,
    design_width: f32,
    design_height: f32,
) -> (Pixels, Pixels) {
    let viewport = window.viewport_size();
    let available_width = f32::from(viewport.width) - DIALOG_PANEL_VIEWPORT_MARGIN * 2.0;
    let available_height = f32::from(viewport.height) - DIALOG_PANEL_VIEWPORT_MARGIN * 2.0;
    let (width, height) = clamp_dialog_panel_size(
        design_width,
        design_height,
        available_width,
        available_height,
    );
    (px(width), px(height))
}

/// 钳制计算（纯函数，供 [`dialog_panel_size`] 与单测共用）：
/// 取 min(设计尺寸, 可用空间)，并保留下限——极端小窗也不缩到下限以下，
/// 否则标题与操作行会挤没。
fn clamp_dialog_panel_size(
    design_width: f32,
    design_height: f32,
    available_width: f32,
    available_height: f32,
) -> (f32, f32) {
    (
        design_width
            .min(available_width)
            .max(DIALOG_PANEL_MIN_WIDTH),
        design_height
            .min(available_height)
            .max(DIALOG_PANEL_MIN_HEIGHT),
    )
}

/// 对话框面板
pub(crate) fn dialog_panel(title: impl Into<gpui::SharedString>) -> Stateful<Div> {
    let title: gpui::SharedString = title.into();
    let id_suffix: String = title.to_string();
    div()
        .id(format!("dialog-{id_suffix}"))
        .w(px(480.0))
        .p_4()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::BORDER_MUTED))
        .bg(rgb(theme::WB_PANEL))
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .cursor(CursorStyle::Arrow)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .pb_1()
                .border_b_1()
                .border_color(rgb(theme::BORDER_MUTED))
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(theme::CONTENT_PRIMARY))
                        .truncate()
                        .child(title),
                ),
        )
}

/// 对话框底部操作行
pub(crate) fn dialog_actions() -> Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap_2()
        .pt_2()
        .border_t_1()
        .border_color(rgb(theme::BORDER_MUTED))
}

/// 危险操作提示框
pub(crate) fn danger_callout(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::FEEDBACK_ERROR_BORDER))
        .bg(rgb(theme::FEEDBACK_ERROR_BG))
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(rgb(theme::FEEDBACK_ERROR_TEXT))
        .child(message.into())
}

/// 输入框外壳
pub(crate) fn input_frame(id: String, focused: bool, size: InputFrameSize) -> Stateful<Div> {
    let height = match size {
        InputFrameSize::Compact => px(28.0),
        InputFrameSize::Regular => px(34.0),
        InputFrameSize::Multiline => px(92.0),
    };
    div()
        .id(id)
        .relative()
        .w_full()
        .min_h(height)
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(if focused {
            rgb(theme::INPUT_BORDER_FOCUSED)
        } else {
            rgb(theme::INPUT_BORDER)
        })
        .bg(if focused {
            rgb(theme::INPUT_BG_FOCUSED)
        } else {
            rgb(theme::INPUT_BG)
        })
        .text_size(px(12.0))
        .line_height(px(18.0))
        .cursor(CursorStyle::IBeam)
        .when(!focused, |this| {
            this.hover(|this| this.bg(rgb(theme::STATE_HOVER)))
        })
        .when(focused, |this| {
            this.shadow_sm()
                .border_color(rgb(theme::INPUT_BORDER_FOCUSED))
        })
}

/// 分段按钮 — 旧版 segmented_button，保留接口但更新配色
pub(crate) fn segmented_button(id: String, selected: bool, enabled: bool) -> BaseButton {
    BaseButton::new(id)
        .selected(selected)
        .disabled(!enabled)
        .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
        .flex_none()
        .min_h(px(28.0))
        .px_2()
        .py_1()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(if selected {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::BORDER_MUTED)
        })
        .bg(if selected {
            rgb(theme::STATE_SELECTION)
        } else {
            rgb(theme::WB_PANEL)
        })
        .text_size(px(12.0))
        .text_color(if selected {
            rgb(theme::PRIMARY)
        } else if enabled {
            rgb(theme::CONTENT_SECONDARY)
        } else {
            rgb(theme::BORDER_MUTED)
        })
        .font_weight(if selected {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        })
        .when(selected, |this| this.shadow_sm())
        .when(enabled, |this| this.cursor_pointer())
        .when(!enabled, |this| this.cursor_not_allowed().opacity(0.68))
        .when(enabled, |this| {
            this.hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListRowVisualRule {
    background: u32,
    shows_selection_indicator: bool,
}

const fn list_row_visual_rule(selected: bool) -> ListRowVisualRule {
    ListRowVisualRule {
        background: if selected {
            theme::PRIMARY_SUBTLE
        } else {
            theme::SURFACE_BASE
        },
        shows_selection_indicator: selected,
    }
}

/// 平面列表行：默认无完整边框/阴影，选中态使用淡色底和左侧 2px 指示条。
pub(crate) fn list_row_surface(id: String, selected: bool) -> Stateful<Div> {
    let rule = list_row_visual_rule(selected);
    div()
        .id(id)
        .relative()
        .min_h(px(theme::ROW_HEIGHT_COMPACT))
        .rounded(px(theme::RADIUS_XS))
        .bg(rgb(rule.background))
        .hover(|this| {
            if selected {
                this.bg(rgb(theme::PRIMARY_SUBTLE))
            } else {
                this.bg(rgb(theme::WB_ROW_HOVER))
            }
        })
        .when(rule.shows_selection_indicator, |this| {
            this.child(
                div()
                    .absolute()
                    .top(px(theme::SPACE_1))
                    .bottom(px(theme::SPACE_1))
                    .left(px(0.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(rgb(theme::PRIMARY)),
            )
        })
}

/// 区块计数徽标的完整定义：数量 + 一对语义配色。
///
/// 同一语义必须用同一配色（如「已暂存」走主色、「未暂存」走次级色），
/// 因此配色随徽标一起传递，调用方不再各自拼 `bg`/`fg`。
/// 调用方负责只在 `count > 0` 时构造徽标（0 没有展示价值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelBadge {
    pub(crate) count: usize,
    pub(crate) bg: u32,
    pub(crate) fg: u32,
}

impl PanelBadge {
    pub(crate) const fn new(count: usize, bg: u32, fg: u32) -> Self {
        Self { count, bg, fg }
    }

    /// 渲染为浅色小药丸标签（10px 半粗，与各列表计数徽标规格一致）。
    pub(crate) fn render(self) -> impl IntoElement {
        div()
            .flex_none()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(theme::RADIUS_PILL))
            .bg(rgb(self.bg))
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(self.fg))
            .child(self.count.to_string())
    }
}

/// 区块标题行的底色语义。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelHeaderSurface {
    /// 分组带：`WB_SECTION_HEADER` 底色，用于列表/分区上方，替代贯穿分割线。
    #[default]
    Grouped,
    /// 随父面板：不自铺底色，用于已经抬起的卡片内部（如提交条里的「提交信息」）。
    Parent,
}

/// 区块标题的语义色：普通区块用正文色，正在查看的对象（如当前差异文件）用强调色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelTitleTone {
    #[default]
    Neutral,
    Accent,
}

impl PanelTitleTone {
    const fn color(self) -> u32 {
        match self {
            PanelTitleTone::Neutral => theme::CONTENT_PRIMARY,
            PanelTitleTone::Accent => theme::PRIMARY,
        }
    }
}

/// 面板区块标题行 —— 工作区变更分区、提交导航/详情/文件、差异区共用同一条。
///
/// 设计约定（悬浮工作台视觉规范 §4）：**分组底色（`WB_SECTION_HEADER`）替代贯穿分割线**，
/// 留白与底色表达分组；左侧是标题与可选计数徽标，右侧是命令槽。列表内容区回到
/// 面板底色，标题因此成为列表上方那条可辨认的分组带。
///
/// `padding_x` 与该列行内容的左内边距对齐（变更/差异列 16px、历史列 12px），
/// 标题才能和下方行文本落在同一条基线上。
pub(crate) struct PanelSectionHeader {
    title: SharedString,
    badge: Option<PanelBadge>,
    tone: PanelTitleTone,
    surface: PanelHeaderSurface,
    padding_x: f32,
    top_rounded: bool,
    actions: Vec<AnyElement>,
}

/// 区块标题行的入口：`panel_section_header("标题").badge(..).build()`。
pub(crate) fn panel_section_header(title: impl Into<SharedString>) -> PanelSectionHeader {
    PanelSectionHeader::new(title)
}

impl PanelSectionHeader {
    pub(crate) fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            badge: None,
            tone: PanelTitleTone::Neutral,
            surface: PanelHeaderSurface::Grouped,
            padding_x: theme::SPACE_3,
            top_rounded: false,
            actions: Vec::new(),
        }
    }

    pub(crate) fn badge(mut self, badge: PanelBadge) -> Self {
        self.badge = Some(badge);
        self
    }

    pub(crate) fn accent_title(mut self) -> Self {
        self.tone = PanelTitleTone::Accent;
        self
    }

    pub(crate) fn surface(mut self, surface: PanelHeaderSurface) -> Self {
        self.surface = surface;
        self
    }

    pub(crate) fn padding_x(mut self, padding_x: f32) -> Self {
        self.padding_x = padding_x;
        self
    }

    /// 标记该标题行是所在悬浮面板的**第一行**：分组底色要带上与面板一致的
    /// 顶部圆角。gpui 的 `overflow_hidden` 只裁矩形、裁不住圆角，方角的标题
    /// 底色会盖住面板圆角，让面板外角看起来是直角。
    pub(crate) fn top_rounded(mut self) -> Self {
        self.top_rounded = true;
        self
    }

    /// 追加一个命令槽元素（按添加顺序从左到右排列）。
    pub(crate) fn action(mut self, action: AnyElement) -> Self {
        self.actions.push(action);
        self
    }

    /// 批量追加命令槽元素；`Option` 也接受（`None` 表示没有命令）。
    pub(crate) fn actions(mut self, actions: impl IntoIterator<Item = AnyElement>) -> Self {
        self.actions.extend(actions);
        self
    }

    pub(crate) fn build(self) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_2))
            .px(px(self.padding_x))
            .py(px(theme::SPACE_2))
            .when(self.surface == PanelHeaderSurface::Grouped, |this| {
                this.bg(rgb(theme::WB_SECTION_HEADER))
            })
            .when(self.top_rounded, |this| {
                this.rounded_t(px(theme::RADIUS_PANEL))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w(px(0.0))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(theme::TYPE_BODY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(self.tone.color()))
                            .child(self.title),
                    )
                    .when_some(self.badge, |this, badge| this.child(badge.render())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE_1))
                    .children(self.actions),
            )
    }
}

/// 空状态/加载提示行的对齐语义。
///
/// 区块级空态居中（提示整块区域没有内容），列表级提示左对齐
/// （与列表行文字同一起点，读起来仍是一个列表项）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaceholderAlign {
    Start,
    Center,
}

/// 区块内与列表内的空状态/加载提示行：无卡片框、无边框、弱化文字，
/// 与所在面板底色连成一片（视觉规范 §4：状态不靠逐行卡片表达）。
///
/// `row_height` 由调用方给出，用于保持该区域原本的行高节奏
/// （列表占位 `ROW_HEIGHT_COMPACT`、变更分区 `CHANGE_ROW_HEIGHT`）。
/// `padding_x` 默认 `SPACE_3`；与 `PanelSectionHeader::padding_x` 同理，
/// 需要与所在列表行文本对齐时由调用方覆盖（变更列表行是 16px）。
pub(crate) fn panel_empty_row(
    text: impl Into<SharedString>,
    row_height: f32,
    align: PlaceholderAlign,
) -> Div {
    panel_empty_row_aligned(text, row_height, align, theme::SPACE_3)
}

/// `panel_empty_row` 的显式左内边距变体：空态提示与列表行文本同基线。
pub(crate) fn panel_empty_row_aligned(
    text: impl Into<SharedString>,
    row_height: f32,
    align: PlaceholderAlign,
    padding_x: f32,
) -> Div {
    div()
        .flex_none()
        .w_full()
        .min_h(px(row_height))
        .flex()
        .items_center()
        .px(px(padding_x))
        .py(px(theme::SPACE_2))
        .text_size(px(theme::TYPE_BODY))
        .line_height(px(18.0))
        .text_color(rgb(theme::CONTENT_SECONDARY))
        .map(|this| match align {
            PlaceholderAlign::Start => this.justify_start(),
            PlaceholderAlign::Center => this.justify_center(),
        })
        .child(text.into())
}

/// 状态药丸
pub(crate) fn status_pill(label: &'static str, active: bool) -> impl IntoElement {
    div()
        .flex_none()
        .min_h(px(24.0))
        .px_2()
        .py_1()
        .rounded_full()
        .border_1()
        .border_color(if active {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::BORDER_MUTED)
        })
        .bg(if active {
            rgb(theme::STATE_SELECTION)
        } else {
            rgb(theme::WB_PANEL)
        })
        .text_color(if active {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::CONTENT_SECONDARY)
        })
        .font_weight(gpui::FontWeight::BOLD)
        .child(label)
}

/// Toast 容器定位：所有通知气泡（Info/Success/Warning/Error）统一在右下角
/// 堆叠展示；每个气泡自带关闭按钮（见 `feedback_bubble`）。
pub(crate) fn feedback_stack() -> Div {
    div()
        .absolute()
        .bottom(px(54.0))
        .right(px(18.0))
        .w(px(340.0))
        .flex()
        .flex_col()
        .gap_2()
}

fn feedback_icon(label: &'static str, bg: u32, text: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.0))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(px(12.0))
        .line_height(px(22.0))
        .text_align(gpui::TextAlign::Center)
        .font_weight(gpui::FontWeight::BOLD)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h_full()
                .text_align(gpui::TextAlign::Center)
                .child(label),
        )
}

pub(crate) fn feedback_bubble(
    feedback: &FeedbackMessage,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    let (soft_bg, border, text) = feedback.kind.palette();
    let dot = match feedback.kind {
        AppToastKind::Info => "i",
        AppToastKind::Success => "✓",
        AppToastKind::Warning => "!",
        AppToastKind::Error => "×",
    };
    let feedback_id = feedback.id;

    div()
        .id(format!("feedback-{}", feedback.id))
        .w_full()
        .px_3()
        .py_2()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(theme::WB_PANEL))
        .shadow_lg()
        .when_some(feedback.action, |this, action| {
            // 可点击气泡：点击气泡体直达对应页面；点 ✕ 只关闭不触发。
            this.cursor_pointer()
                .hover(|this| this.border_color(rgb(theme::STATE_HOVER)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    match action {
                        ToastAction::OpenUpdateSettings => this.open_update_settings_center(),
                    }
                    this.feedbacks.retain(|feedback| feedback.id != feedback_id);
                    cx.notify();
                }))
        })
        .flex()
        .gap_3()
        .child(feedback_icon(dot, soft_bg, text))
        .child(
            div()
                // 文字列占满剩余宽度：关闭按钮被推到气泡右缘（配合行首图标的
                // 顶部对齐，关闭按钮落在气泡右上角），而不是挤在文字后面。
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(text))
                        .child(feedback.title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(theme::CONTENT_PRIMARY))
                        .child(feedback.message.clone()),
                ),
        )
        .child(
            div()
                .id(format!("feedback-close-{}", feedback.id))
                .flex_none()
                .size(px(22.0))
                .rounded(px(theme::RADIUS_XS))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rgb(theme::CONTENT_SECONDARY))
                .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    this.feedbacks.retain(|feedback| feedback.id != feedback_id);
                    cx.notify();
                }))
                // 关闭按钮图标走项目自绘 toolbar_icon（svg 直连、与侧边栏 ✕ 同款）。
                // 此前用的 yororen Icon 包装在该气泡内未渲染出图形（仅剩 22px 空槽）。
                .child(toolbar_icon(ToolbarIcon::Close, theme::CONTENT_SECONDARY)),
        )
}

/// 内联错误气泡
pub(crate) fn inline_error_bubble(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .flex_none()
        .max_w(px(460.0))
        .px_2()
        .py_1()
        .rounded_full()
        .border_1()
        .border_color(rgb(theme::FEEDBACK_ERROR_BORDER))
        .bg(rgb(theme::WB_PANEL))
        .text_color(rgb(theme::FEEDBACK_ERROR_TEXT))
        .truncate()
        .child(message.into())
}

/// 底部进度条
pub(crate) fn bottom_progress_bar(phase: u64) -> impl IntoElement {
    let offset = ((phase % 7) as f32 - 2.0) * 72.0;
    div()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .h(px(3.0))
        .overflow_hidden()
        .bg(rgb(theme::PROGRESS_TRACK))
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(offset))
                .w(px(260.0))
                .rounded_full()
                .bg(rgb(theme::PROGRESS_FILL)),
        )
}

/// 操作加载条
pub(crate) fn operation_loading_bar(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .absolute()
        .left(px(16.0))
        .right(px(16.0))
        .bottom(px(46.0))
        .h(px(34.0))
        .px_3()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::FEEDBACK_INFO_BORDER))
        .bg(rgb(theme::WB_PANEL))
        .shadow_lg()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.0))
        .text_color(rgb(theme::PRIMARY))
        .child(
            div()
                .size(px(8.0))
                .rounded_full()
                .bg(rgb(theme::PROGRESS_FILL)),
        )
        .child(div().min_w(px(0.0)).truncate().child(message.into()))
}

fn format_badge_count(count: usize) -> String {
    if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    }
}

/// 工具栏角标徽标 — 紫色主色调
fn button_badge(count: usize) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(16.0))
        .h(px(16.0))
        .px_1()
        .rounded_full()
        .bg(rgb(theme::PRIMARY))
        .text_color(rgb(theme::PRIMARY_FOREGROUND))
        .text_size(px(10.0))
        .line_height(px(14.0))
        .child(format_badge_count(count))
}

/// 拉取/推送差异数角标 — 用于工具栏按钮旁的 ↓N / ↑N 标识
pub(crate) fn sync_badge(label: &'static str, count: usize) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(theme::PRIMARY))
        .child(format!("{}{}", label, count))
}

impl RepositoryView {
    pub(crate) fn notify_toast(
        &mut self,
        kind: AppToastKind,
        message: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into().to_string();
        if message.trim().is_empty() {
            return;
        }
        self.next_feedback_id = self.next_feedback_id.wrapping_add(1).max(1);
        self.feedbacks
            .push_back(FeedbackMessage::new(self.next_feedback_id, kind, message));
        while self.feedbacks.len() > 5 {
            self.feedbacks.pop_front();
        }
        cx.notify();
    }

    /// 带点击动作的通知气泡：点击气泡体执行动作后移除气泡；寿命与普通
    /// 气泡一致（按类别 4s / 7s 自动消失），点 ✕ 视为「稍后」仅关闭。
    pub(crate) fn notify_toast_with_action(
        &mut self,
        kind: AppToastKind,
        message: impl Into<gpui::SharedString>,
        action: ToastAction,
        cx: &mut Context<Self>,
    ) {
        let message = message.into().to_string();
        if message.trim().is_empty() {
            return;
        }
        self.next_feedback_id = self.next_feedback_id.wrapping_add(1).max(1);
        self.feedbacks.push_back(
            FeedbackMessage::new(self.next_feedback_id, kind, message).with_toast_action(action),
        );
        while self.feedbacks.len() > 5 {
            self.feedbacks.pop_front();
        }
        cx.notify();
    }

    pub(crate) fn notify_success(
        &mut self,
        message: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notify_toast(AppToastKind::Success, message, cx);
    }

    pub(crate) fn notify_warning(
        &mut self,
        message: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notify_toast(AppToastKind::Warning, message, cx);
    }

    pub(crate) fn notify_error(
        &mut self,
        message: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notify_toast(AppToastKind::Error, message, cx);
    }

    pub(crate) fn notify_completion(&mut self, message: &str, cx: &mut Context<Self>) {
        if message.contains("失败") || message.contains("冲突") {
            self.notify_warning(message.to_string(), cx);
        } else {
            self.notify_success(message.to_string(), cx);
        }
    }

    pub(crate) fn should_toast_completion(message: &str) -> bool {
        message.contains("完成")
            || message.contains("失败")
            || message.contains("冲突")
            || message.contains("测试通过")
            || message.contains("已复制")
            || message.contains("已添加")
            || message.contains("已更新")
            || message.contains("已新增")
            || message.contains("已删除")
            || message.contains("已刷新")
            || message.contains("已提交")
            || message.contains("工作流")
    }

    /// 普通命令按钮。文案是静态字面量：按钮的 ElementId 与无障碍标签都由它派生，
    /// 多行列表里请改用带显式 ID 的行内按钮 helper。
    pub(crate) fn button(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label.into(),
            None,
            None,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
    }

    /// 轻量文字按钮：左侧图标 + 右侧文字，无边框、无底色，hover 才出浅底。
    ///
    /// 用于「AI 生成」这类行内文字入口——设计稿中它不是带轮廓的实体按钮，
    /// 与旁边的输入框并列时不能抢焦点。禁用态用 tooltip 说明原因。
    pub(crate) fn ghost_button<T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static>(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        disabled_hint: Option<&'static str>,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        let hint = if enabled {
            None
        } else {
            Some(disabled_hint.unwrap_or("当前状态不可用"))
        };
        // 图标与文字同色：AI 入口用主色强调，禁用时统一转为弱化灰。
        let fg = if enabled {
            theme::PRIMARY
        } else {
            theme::CONTENT_TERTIARY
        };
        BaseButton::new(label)
            .disabled(!enabled)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_none()
            .flex()
            .items_center()
            // 图标与文字贴近（设计稿：AI 生成是图标 + 紧凑文字）。
            .gap(px(6.0))
            .h(px(theme::CONTROL_HEIGHT_TOOLBAR))
            .px(px(theme::SPACE_2))
            .rounded(px(theme::RADIUS_SM))
            .text_color(rgb(fg))
            .text_size(px(theme::TYPE_BODY))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
                    .active(|this| this.opacity(0.8))
            })
            .when(!enabled, |this| this.cursor_not_allowed())
            .when_some(hint, |this, text| {
                this.tooltip(move |_window, cx| tooltip_text(text, cx))
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(icon, fg))
            .child(label)
    }

    pub(crate) fn toolbar_button(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label.into(),
            Some(icon),
            None,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
    }

    pub(crate) fn toolbar_button_with_badge(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        badge: Option<usize>,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label.into(),
            Some(icon),
            badge,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
    }

    pub(crate) fn toolbar_button_with_click_event(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button_with_click_event(
            label.into(),
            Some(icon),
            None,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
    }

    /// 变更区域图标按钮 — 设计图：22×22 圆角方块，纯图标，无边框
    /// hover 显示 STATE_HOVER 背景，icon 14px CONTENT_SECONDARY 色
    pub(crate) fn change_icon_button(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon_color = if enabled {
            theme::CONTENT_SECONDARY
        } else {
            theme::CONTENT_SECONDARY
        };
        BaseButton::new(label)
            .disabled(!enabled)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_none()
            .size(px(22.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.4).cursor_not_allowed())
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(icon, icon_color))
    }

    /// 变更区域危险图标按钮 — 设计图：22×22 圆角方块，DESTRUCTIVE 色图标
    pub(crate) fn change_destructive_icon_button(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 设计图用 trash-2 图标，DESTRUCTIVE 色
        BaseButton::new(label)
            .disabled(!enabled)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_none()
            .size(px(22.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.4).cursor_not_allowed())
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(ToolbarIcon::Trash, theme::DESTRUCTIVE))
    }

    /// 变更行内图标按钮 — 设计图：20×20 圆角方块，纯图标 12px，无边框
    /// hover 显示 STATE_HOVER 背景
    ///
    /// `id` 必须由调用方给出「页面 + 条目键 + 动作」（例：`change-row-action-unstaged-src/a.rs`）：
    /// 该按钮在每个变更行里都出现，拿中文 label 当 ElementId 会让所有行共用同一份
    /// 键控状态（焦点与 tooltip 宿主互相串）。
    pub(crate) fn change_row_icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        icon: ToolbarIcon,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        BaseButton::new(id)
            .disabled(!enabled)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_none()
            .size(px(20.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::STATE_HOVER)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.4).cursor_not_allowed())
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(toolbar_icon(icon, icon_color))
    }

    /// 次级动作按钮：白/近白薄实体 + 弱轮廓 + 接触阴影。与蓝色主按钮并列时表达
    /// 「先做这个、再做那个」的主次（最新 Pencil 稿的「提交到 &lt;分支&gt;」）。
    /// 启用条件与业务守卫语义与 `primary_button` 完全一致，只换视觉权重。
    ///
    /// 文案允许是运行时字符串（如「提交到 &lt;当前分支&gt;」），因此这里收 `SharedString`
    /// 而不是 `&'static str`；调用方传 `"提交到 x".into()`。按钮的 ElementId 由文案派生，
    /// 所以同一页面上的按钮文案必须唯一——提交条满足这一点（分支名即条目键）；
    /// 多行列表按钮请改用带显式 ID 的行内按钮 helper。
    pub(crate) fn secondary_button<T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static>(
        &self,
        label: SharedString,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        self.app_button(
            label,
            None,
            None,
            ButtonTone::Secondary,
            enabled,
            on_click,
            cx,
        )
    }

    pub(crate) fn primary_button<T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static>(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        self.app_button(
            label.into(),
            None,
            None,
            ButtonTone::Primary,
            enabled,
            on_click,
            cx,
        )
    }

    pub(crate) fn danger_button(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label.into(),
            None,
            None,
            ButtonTone::Danger,
            enabled,
            on_click,
            cx,
        )
    }

    /// 次级动作按钮（前置图标版）：「提交到 &lt;分支&gt;」按设计稿在文字前
    /// 加勾图标，主次关系与无图标版完全一致。
    pub(crate) fn secondary_button_with_icon<
        T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    >(
        &self,
        label: SharedString,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        self.app_button(
            label,
            Some(icon),
            None,
            ButtonTone::Secondary,
            enabled,
            on_click,
            cx,
        )
    }

    /// 主动作按钮（前置图标版）：「提交并推送」按设计稿在文字前加上箭头。
    pub(crate) fn primary_button_with_icon<
        T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    >(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        self.app_button(
            label.into(),
            Some(icon),
            None,
            ButtonTone::Primary,
            enabled,
            on_click,
            cx,
        )
    }

    /// 「复选框 + 文字」行：Kit Checkbox（方框 + 勾），文字区独立可点，
    /// 两者走同一动作（互不重叠，不会双重触发）。
    ///
    /// 设计稿的「修补上一次提交」是复选框而不是开关；方框与勾由调用方自绘，
    /// 选中态用主色填充，未选中是弱轮廓空心框。Kit `Checkbox.on_change` 的
    /// 签名比 Switch 多一个 `ClickEvent` 且按值传状态，`cx.listener` 适配不了，
    /// 这里经 `cx.entity()` 回写宿主；动作因此不携带 `Window`。
    pub(crate) fn checkbox_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let on_click = Rc::new(on_click);
        let box_click = on_click.clone();
        let label_click = on_click.clone();
        let box_border = if checked {
            theme::PRIMARY
        } else {
            theme::BORDER_STRONG
        };
        div()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_2))
            .child(
                Checkbox::new(id)
                    .checked(checked)
                    .accessibility_label(label)
                    .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
                    .flex_none()
                    .size(px(16.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(box_border))
                    .bg(rgb(if checked {
                        theme::PRIMARY
                    } else {
                        theme::WB_PANEL
                    }))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_change(move |_state: CheckboxState, _event: &ClickEvent, _window, cx| {
                        entity.update(cx, |this, cx| {
                            box_click(this, cx);
                            cx.notify();
                        });
                    })
                    .when(checked, |this| {
                        this.child(toolbar_icon(ToolbarIcon::Check, theme::PRIMARY_FOREGROUND))
                    }),
            )
            .child(
                div()
                    .id(format!("{id}-label"))
                    .cursor_pointer()
                    .text_size(px(theme::TYPE_BODY))
                    .text_color(rgb(theme::CONTENT_PRIMARY))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        label_click(this, cx);
                        cx.notify();
                    }))
                    .child(label),
            )
    }

    fn app_button<T: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static>(
        &self,
        label: SharedString,
        icon: Option<ToolbarIcon>,
        badge: Option<usize>,
        tone: ButtonTone,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        self.app_button_with_click_event(
            label,
            icon,
            badge,
            tone,
            enabled,
            move |this, _event, window, cx| on_click(this, window, cx),
            cx,
        )
    }

    fn app_button_with_click_event<
        T: Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
    >(
        &self,
        label: SharedString,
        icon: Option<ToolbarIcon>,
        badge: Option<usize>,
        tone: ButtonTone,
        enabled: bool,
        on_click: T,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        let palette = app_button_palette(tone, enabled);
        let disabled_reason = self.disabled_reason(enabled, "当前状态不可用");
        let bg_color = if enabled {
            palette.bg
        } else {
            theme::STATE_HOVER
        };
        let text_color = if enabled {
            palette.fg
        } else {
            theme::CONTENT_SECONDARY
        };
        let accessibility_label = label.clone();
        BaseButton::new(label)
            .disabled(!enabled)
            .accessibility_label(accessibility_label.clone())
            .focus_visible(|this| this.border_color(rgb(theme::PRIMARY)))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .min_h(px(28.0))
            .px_3()
            .py_1()
            .border_1()
            .border_color(rgb(palette.border))
            .rounded(px(theme::RADIUS_XS))
            .bg(rgb(bg_color))
            .text_color(rgb(text_color))
            .text_size(px(12.0))
            .font_weight(if tone == ButtonTone::Primary {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::NORMAL
            })
            .when(enabled && palette.contact_shadow, |this| {
                this.shadow(control_shadow())
            })
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.78))
            .when(enabled, |this| {
                this.hover(move |this| this.bg(rgb(palette.hover_bg)))
                    // 按下：阴影收短 + 轻微压暗，表达「压下」而不改变尺寸。
                    .active(|this| this.opacity(0.9).shadow(pressed_shadow()))
            })
            .when_some(disabled_reason, |this, tooltip| {
                this.tooltip(move |_window, cx| tooltip_text(tooltip, cx))
            })
            .on_click(cx.listener(move |this, event, window, cx| {
                if enabled {
                    let previous_status = this.status.clone();
                    let previous_busy = this.busy;
                    let previous_feedback_count = this.feedbacks.len();
                    on_click(this, event, window, cx);
                    if this.feedbacks.len() == previous_feedback_count {
                        if let Some(error) = this.last_error.clone() {
                            this.notify_error(error, cx);
                        } else if !previous_busy
                            && !this.busy
                            && this.status != previous_status
                            && Self::should_toast_completion(&this.status)
                        {
                            this.notify_success(this.status.clone(), cx);
                        }
                    }
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .when_some(icon, |this, icon| {
                        this.child(toolbar_icon(icon, text_color))
                    })
                    .child(accessibility_label)
                    .when_some(badge.filter(|count| *count > 0), |this, count| {
                        this.child(button_badge(count))
                    }),
            )
    }
}

#[cfg(test)]
#[path = "../tests/ui/components.rs"]
mod tests;
