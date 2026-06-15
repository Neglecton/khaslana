use gpui::{CursorStyle, Div, IntoElement, MouseButton, div, prelude::*, px, rgb, rgba};

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

pub(crate) fn should_render_operation_blocker(blocker: OperationBlocker, busy: bool) -> bool {
    blocker.blocks_interaction() && busy
}

pub(crate) fn operation_blocker_overlay(message: impl Into<gpui::SharedString>, phase: u64) -> Div {
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
                .w(px(360.0))
                .p_4()
                .rounded_sm()
                .border_1()
                .border_color(rgb(ui_theme::GLASS_BORDER))
                .bg(rgba(ui_theme::GLASS_BG_STRONG))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(ui_theme::TEXT))
                        .child(
                            div()
                                .size(px(10.0))
                                .rounded_full()
                                .bg(rgb(ui_theme::PROGRESS_FILL)),
                        )
                        .child(message.into()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(ui_theme::TEXT_MUTED))
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
        should_render_operation_blocker(tab.operation_blocker, tab.busy).then(|| tab.status.clone())
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
mod tests {
    use super::{OperationBlocker, should_render_operation_blocker};

    #[test]
    fn operation_blocker_renders_only_when_modal_and_busy() {
        assert!(!should_render_operation_blocker(
            OperationBlocker::None,
            true
        ));
        assert!(!should_render_operation_blocker(
            OperationBlocker::Modal,
            false
        ));
        assert!(should_render_operation_blocker(
            OperationBlocker::Modal,
            true
        ));
    }
}
