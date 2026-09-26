//! 切换被未提交修改阻止时的确认弹窗：贮藏后切换。

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
                    .child("是否贮藏当前修改（含未跟踪文件）后再切换？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("贮藏并切换：修改留在贮藏中，稍后从左侧「贮藏」手动应用"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "贮藏并切换",
                        !self.busy,
                        {
                            let target = target.clone();
                            move |this, _, _| this.confirm_carry_checkout_stash_only(target.clone())
                        },
                        cx,
                    )),
            )
    }
}
