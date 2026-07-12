use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, Context, CursorStyle, Div, IntoElement, MouseButton, Render, Stateful, Window,
    div, prelude::*, px, rgb, rgba,
};
use yororen_ui::component::{IconName, icon};

use crate::{
    RepositoryView,
    ui::{icons::ToolbarIcon, icons::toolbar_icon, theme},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub(crate) struct FeedbackMessage {
    pub(crate) id: u64,
    pub(crate) kind: AppToastKind,
    pub(crate) title: &'static str,
    pub(crate) message: String,
    pub(crate) expires_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonTone {
    Neutral,
    Primary,
    Danger,
}

#[derive(Clone, Copy, Debug)]
struct ButtonPalette {
    bg: u32,
    hover_bg: u32,
    fg: u32,
    border: u32,
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
        }
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
            bg: theme::ACCENT,
            hover_bg: theme::ACCENT,
            fg: theme::MUTED_FOREGROUND,
            border: theme::BORDER,
        };
    }

    match tone {
        ButtonTone::Neutral => ButtonPalette {
            bg: theme::ACCENT,
            hover_bg: theme::SECONDARY,
            fg: theme::FOREGROUND,
            border: theme::BORDER,
        },
        ButtonTone::Primary => ButtonPalette {
            bg: theme::PRIMARY,
            hover_bg: theme::PRIMARY,
            fg: theme::PRIMARY_FOREGROUND,
            border: theme::PRIMARY,
        },
        ButtonTone::Danger => ButtonPalette {
            bg: theme::DESTRUCTIVE,
            hover_bg: theme::DESTRUCTIVE,
            fg: theme::DESTRUCTIVE_FOREGROUND,
            border: theme::DESTRUCTIVE,
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
        .text_color(rgb(theme::SIDEBAR_FOREGROUND))
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

/// 模式切换药丸按钮（工作区/提交记录/工作流）
pub(crate) fn mode_pill(
    id: String,
    label: &'static str,
    icon: Option<ToolbarIcon>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .rounded(px(theme::RADIUS_PILL))
        .px(px(14.0))
        .py(px(6.0))
        .justify_center()
        .cursor_pointer()
        .bg(if active {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::WHITE)
        })
        .text_color(if active {
            rgb(theme::PRIMARY_FOREGROUND)
        } else {
            rgb(theme::MUTED_FOREGROUND)
        })
        .text_size(px(13.0))
        .font_weight(if active {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::MEDIUM
        })
        .when_some(icon, |this, icon| {
            this.child(toolbar_icon(
                icon,
                if active {
                    theme::PRIMARY_FOREGROUND
                } else {
                    theme::MUTED_FOREGROUND
                },
            ))
        })
        .child(label)
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
        .border_color(rgb(theme::BORDER))
        .bg(rgb(theme::CARD))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(theme::MUTED_FOREGROUND))
        .child(title)
}

/// 应用面板 — 扁平无装饰纯色容器
pub(crate) fn app_panel() -> Div {
    flat_panel()
}

/// 应用外壳 — 纯色背景，去掉旧版渐变和玻璃态装饰
pub(crate) fn app_shell_surface() -> Div {
    div().relative().size_full().bg(rgb(theme::BACKGROUND))
}

/// 工具栏 — 扁平边框条，去掉旧版阴影、玻璃态、内部高亮线
pub(crate) fn hero_toolbar() -> Div {
    div()
        .border_b_1()
        .border_color(rgb(theme::BORDER))
        .bg(rgb(theme::CARD))
}

/// 扁平面板 — 无装饰纯色容器，无边框无阴影
pub(crate) fn flat_panel() -> Div {
    div()
}

/// 玻璃面板 — 保留给弹窗/上下文菜单等需要浮层效果的场景
pub(crate) fn glass_panel() -> Div {
    div()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .bg(rgb(theme::CARD))
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
        .bg(rgb(theme::ACCENT))
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
        .bg(rgba(theme::DIALOG_OVERLAY))
        .cursor(CursorStyle::Arrow)
        .occlude()
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
        .border_color(rgb(theme::BORDER))
        .bg(rgb(theme::CARD))
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
                .border_color(rgb(theme::BORDER))
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(theme::FOREGROUND))
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
        .border_color(rgb(theme::BORDER))
}

/// 危险操作提示框
pub(crate) fn danger_callout(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(rgb(theme::DESTRUCTIVE))
        .bg(rgb(theme::COLOR_ERROR))
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(rgb(theme::COLOR_ERROR_FOREGROUND))
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
            this.hover(|this| this.bg(rgb(theme::ACCENT)))
        })
        .when(focused, |this| {
            this.shadow_sm()
                .border_color(rgb(theme::INPUT_BORDER_FOCUSED))
        })
}

/// 分段按钮 — 旧版 segmented_button，保留接口但更新配色
pub(crate) fn segmented_button(id: String, selected: bool, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .min_h(px(28.0))
        .px_2()
        .py_1()
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(if selected {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::BORDER)
        })
        .bg(if selected {
            rgb(theme::ACCENT)
        } else {
            rgb(theme::CARD)
        })
        .text_size(px(12.0))
        .text_color(if selected {
            rgb(theme::PRIMARY)
        } else if enabled {
            rgb(theme::MUTED_FOREGROUND)
        } else {
            rgb(theme::BORDER)
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
            this.hover(|this| this.bg(rgb(theme::SECONDARY)))
        })
}

/// 复选框
pub(crate) fn toggle_box(checked: bool) -> impl IntoElement {
    div()
        .size(px(14.0))
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(if checked {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::BORDER)
        })
        .bg(if checked {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::CARD)
        })
        .child(
            div()
                .w_full()
                .h_full()
                .when(checked, |this| this.child("✓"))
                .items_center()
                .justify_center()
                .text_color(rgb(theme::PRIMARY_FOREGROUND))
                .text_size(px(10.0)),
        )
}

/// 列表行表面 — 选中/未选中
pub(crate) fn list_row_surface(id: String, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .rounded(px(theme::RADIUS_XS))
        .border_1()
        .border_color(if selected {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::BORDER)
        })
        .bg(if selected {
            rgb(theme::ACCENT)
        } else {
            rgb(theme::CARD)
        })
        .shadow_sm()
        .hover(|this| this.bg(rgb(theme::SECONDARY)))
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
            rgb(theme::BORDER)
        })
        .bg(if active {
            rgb(theme::ACCENT)
        } else {
            rgb(theme::CARD)
        })
        .text_color(if active {
            rgb(theme::PRIMARY)
        } else {
            rgb(theme::MUTED_FOREGROUND)
        })
        .font_weight(gpui::FontWeight::BOLD)
        .child(label)
}

/// Toast 容器定位
pub(crate) fn feedback_stack(important: bool) -> Div {
    div()
        .absolute()
        .bottom(px(54.0))
        .when(important, |this| this.right(px(18.0)))
        .when(!important, |this| this.left(px(18.0)))
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
        .bg(rgb(theme::CARD))
        .shadow_lg()
        .flex()
        .gap_3()
        .child(feedback_icon(dot, soft_bg, text))
        .child(
            div()
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
                        .text_color(rgb(theme::FOREGROUND))
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
                .text_color(rgb(theme::MUTED_FOREGROUND))
                .hover(|this| this.bg(rgb(theme::ACCENT)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    this.feedbacks.retain(|feedback| feedback.id != feedback_id);
                    cx.notify();
                }))
                .child(icon(IconName::Close).size(px(12.0)).inherit_color(true)),
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
        .bg(rgb(theme::CARD))
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
        .bg(rgb(theme::CARD))
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
            || message.contains("已复制")
            || message.contains("已添加")
            || message.contains("已更新")
            || message.contains("已新增")
            || message.contains("已删除")
            || message.contains("已刷新")
            || message.contains("已提交")
            || message.contains("工作流")
    }

    pub(crate) fn button(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label,
            None,
            None,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
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
            label,
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
            label,
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
            label,
            Some(icon),
            None,
            ButtonTone::Neutral,
            enabled,
            on_click,
            cx,
        )
    }

    /// 变更区域图标按钮 — 设计图：22×22 圆角方块，纯图标，无边框
    /// hover 显示 ACCENT 背景，icon 14px MUTED_FOREGROUND 色
    pub(crate) fn change_icon_button(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon_color = if enabled {
            theme::MUTED_FOREGROUND
        } else {
            theme::MUTED_FOREGROUND
        };
        div()
            .id(label)
            .flex_none()
            .size(px(22.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::ACCENT)))
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
        div()
            .id(label)
            .flex_none()
            .size(px(22.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::ACCENT)))
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
    /// hover 显示 ACCENT 背景
    pub(crate) fn change_row_icon_button(
        &self,
        label: &'static str,
        icon: ToolbarIcon,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(label)
            .flex_none()
            .size(px(20.0))
            .rounded(px(theme::RADIUS_XS))
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::ACCENT)))
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

    pub(crate) fn primary_button(
        &self,
        label: &'static str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.app_button(
            label,
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
        self.app_button(label, None, None, ButtonTone::Danger, enabled, on_click, cx)
    }

    fn app_button(
        &self,
        label: &'static str,
        icon: Option<ToolbarIcon>,
        badge: Option<usize>,
        tone: ButtonTone,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    fn app_button_with_click_event(
        &self,
        label: &'static str,
        icon: Option<ToolbarIcon>,
        badge: Option<usize>,
        tone: ButtonTone,
        enabled: bool,
        on_click: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = app_button_palette(tone, enabled);
        let disabled_reason = self.disabled_reason(enabled, "当前状态不可用");
        let bg_color = if enabled { palette.bg } else { theme::ACCENT };
        let text_color = if enabled {
            palette.fg
        } else {
            theme::MUTED_FOREGROUND
        };
        div()
            .id(label)
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
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.78))
            .when(enabled, |this| {
                this.hover(move |this| this.bg(rgb(palette.hover_bg)))
                    .active(|this| this.opacity(0.82))
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
                    .child(label)
                    .when_some(badge.filter(|count| *count > 0), |this, count| {
                        this.child(button_badge(count))
                    }),
            )
    }
}
