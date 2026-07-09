use gpui::{Context, IntoElement, Window, div, prelude::*, px, rgb};

use crate::{
    FieldId, RepositoryView,
    ui::{components::dialog_actions, theme as ui_theme},
};

impl RepositoryView {
    pub(crate) fn render_external_merge_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("合并工具", cx)
            .w(px(580.0))
            .child(self.toggle_row(
                "external-merge-enabled",
                "启用 IntelliJ IDEA 外部合并",
                self.external_merge_enabled_form,
                |this, _, _| {
                    this.set_external_merge_enabled_form(!this.external_merge_enabled_form);
                },
                cx,
            ))
            .child(self.toggle_row(
                "external-merge-auto-open",
                "选中冲突文件时自动打开 IDEA",
                self.external_merge_auto_open_form,
                |this, _, _| {
                    this.set_external_merge_auto_open_form(!this.external_merge_auto_open_form);
                },
                cx,
            ))
            .child(self.input(FieldId::ExternalMergeIntellijPath, false, window, cx))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("路径可留空。留空时会依次检测 KHASLANA_IDEA_PATH、PATH 中的 idea64 / idea，以及常见 JetBrains 安装目录。开启自动打开后，每个冲突文件只会自动触发一次；失败后可修改设置或手动点击按钮重试。"),
            )
            .when(self.last_error.is_some(), |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::DESTRUCTIVE))
                        .truncate()
                        .child(self.last_error.clone().unwrap_or_default()),
                )
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.button(
                        "检测 IDEA",
                        !self.busy,
                        |this, _, _| this.test_external_merge_settings_from_form(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        |this, _, _| this.save_external_merge_settings_from_form_and_close(),
                        cx,
                    )),
            )
    }
}
