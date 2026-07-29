use std::time::{Duration, Instant};

use gpui::{CursorStyle, Div, IntoElement, MouseButton, div, prelude::*, px};

use crate::ui::theme::{rgb, rgba};
use crate::{RepositoryView, ui::theme as ui_theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OperationBlocker {
    #[default]
    None,
    Modal,
}

impl OperationBlocker {
    pub(crate) fn blocks_interaction(self) -> bool {
        matches!(self, Self::Modal)
    }
}

/// 遮罩层延迟显示的阈值：操作开始后经过该时长仍未完成，才渲染遮罩层。
/// 交互阻断（鼠标捕获、文本框禁用）仍立即生效，此值只影响视觉遮罩层。
/// 用于避免切换分支等本地快速操作一闪而过的闪烁。
pub(crate) const OPERATION_BLOCKER_VISIBLE_DELAY: Duration = Duration::from_millis(300);

pub(crate) fn should_render_operation_blocker(
    blocker: OperationBlocker,
    busy: bool,
    started: Option<Instant>,
    now: Instant,
) -> bool {
    if !(blocker.blocks_interaction() && busy) {
        return false;
    }
    match started {
        Some(started) => now.saturating_duration_since(started) >= OPERATION_BLOCKER_VISIBLE_DELAY,
        None => true,
    }
}

pub(crate) fn wrap_operation_message(message: &str) -> String {
    // 远端推送等状态常包含两个很长的分支名；在“到”前后主动换行，避免遮罩层标题溢出。
    message.replace(" 到 ", " 到\n")
}

pub(crate) fn operation_blocker_overlay(message: impl Into<String>, phase: u64) -> Div {
    let message = wrap_operation_message(&message.into());
    let offset = ((phase % 7) as f32 - 2.0) * 42.0;
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(ui_theme::DIALOG_OVERLAY))
        .cursor(CursorStyle::Arrow)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .w(px(520.0))
                .p_4()
                .rounded_sm()
                .border_1()
                .border_color(rgb(ui_theme::BORDER))
                .bg(rgb(ui_theme::CARD))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(ui_theme::FOREGROUND))
                        .child(
                            div()
                                .flex_none()
                                .mt(px(5.0))
                                .size(px(10.0))
                                .rounded_full()
                                .bg(rgb(ui_theme::PROGRESS_FILL)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .line_height(px(20.0))
                                .child(message),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("请等待当前操作完成，期间不能进行其它操作。"),
                )
                .child(
                    div()
                        .relative()
                        .h(px(4.0))
                        .overflow_hidden()
                        .rounded_full()
                        .bg(rgb(ui_theme::PROGRESS_TRACK))
                        .child(
                            div()
                                .absolute()
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .left(px(offset))
                                .w(px(150.0))
                                .rounded_full()
                                .bg(rgb(ui_theme::PROGRESS_FILL)),
                        ),
                ),
        )
}

impl RepositoryView {
    pub(crate) fn active_operation_blocker_message(&self) -> Option<String> {
        let tab = self.active_tab_state();
        should_render_operation_blocker(
            tab.operation_blocker,
            tab.busy,
            tab.operation_blocker_started,
            Instant::now(),
        )
        .then(|| tab.status.clone())
    }

    pub(crate) fn render_operation_blocker(&self) -> impl IntoElement {
        self.active_operation_blocker_message()
            .map(|message| {
                operation_blocker_overlay(message, self.progress_phase).into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element())
    }
}

#[cfg(test)]
#[path = "tests/operation_blocker_view.rs"]
mod tests;
