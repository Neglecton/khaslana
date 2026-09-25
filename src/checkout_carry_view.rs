//! 切换被未提交修改阻止时的「贮藏后重试」确认弹窗。

use crate::*;

impl RepositoryView {
    pub(crate) fn render_confirm_carry_checkout_dialog(
        &self,
        target: &CheckoutTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target_label = target.label();
        self.dialog_panel("切换被阻止", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!(
                        "{target_label} 包含与当前未提交修改冲突的更改，直接切换会被 Git 拒绝。"
                    )),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("是否先将未提交修改（含未跟踪文件）暂存到贮藏，再切换？"),
            )
            .child(self.toggle_row(
                "carry-checkout-auto-apply",
                "切换后自动应用贮藏",
                self.carry_checkout_auto_apply,
                |this, _, _| {
                    this.carry_checkout_auto_apply = !this.carry_checkout_auto_apply;
                    // 记住用户选择：经 layout_preferences 持久化，重启后仍生效。
                    this.save_layout_preferences();
                },
                cx,
            ))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(if self.carry_checkout_auto_apply {
                        "恢复时若仍有冲突，修改会保留在贮藏列表第一条，可在左侧「贮藏」区域查看。"
                    } else {
                        "关闭时只贮藏并切换，修改保留在贮藏列表第一条，需在左侧「贮藏」区域手动应用或弹出。"
                    }),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "贮藏并切换",
                        !self.busy,
                        {
                            let target = target.clone();
                            move |this, _, _| this.confirm_carry_checkout(target.clone())
                        },
                        cx,
                    )),
            )
    }
}
