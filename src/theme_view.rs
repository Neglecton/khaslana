use gpui::{Context, IntoElement, Window, WindowAppearance, div, prelude::*, px};
use khaslana::ThemeMode;
use yororen_ui::theme::GlobalTheme;

use crate::{
    DialogState, RepositoryView,
    ui::{
        components::{dialog_actions, segmented_button},
        theme::{self as ui_theme, ThemeVariant, rgb},
    },
};

impl RepositoryView {
    pub(crate) fn load_theme_mode(storage: &khaslana::AppStorage) -> ThemeMode {
        storage
            .load_theme_mode()
            .inspect_err(|err| tracing::warn!("theme preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    pub(crate) fn open_theme_settings(&mut self) {
        self.close_popups();
        self.active_dialog = Some(DialogState::ThemeSettings);
        self.last_error = None;
    }

    /// 同时更新 Khaslana 语义色板和 Yororen 全局主题，避免混用组件出现深浅色割裂。
    pub(crate) fn apply_theme_for_appearance(
        &mut self,
        appearance: WindowAppearance,
        cx: &mut Context<Self>,
    ) {
        let variant = ui_theme::variant_for_mode(self.theme_mode, appearance);
        ui_theme::set_active_variant(variant);
        cx.set_global(GlobalTheme::new(variant.window_appearance()));
        cx.notify();
    }

    fn select_theme_mode(&mut self, mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_mode = mode;
        self.apply_theme_for_appearance(window.appearance(), cx);
        window.refresh();

        match self.storage.save_theme_mode(mode) {
            Ok(()) => self.notify_success(format!("外观已切换为{}", theme_mode_label(mode)), cx),
            Err(err) => self.notify_error(format!("主题偏好保存失败：{err}"), cx),
        }
    }

    pub(crate) fn render_theme_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_variant = ui_theme::variant_for_mode(self.theme_mode, window.appearance());
        let modes = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

        self.dialog_panel("外观", cx)
            .w(px(520.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("选择应用主题。跟随系统会在操作系统外观变化时自动切换。"),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap_2()
                    .children(modes.into_iter().map(|mode| {
                        segmented_button(
                            format!("theme-mode-{}", theme_mode_id(mode)),
                            self.theme_mode == mode,
                            true,
                        )
                        .flex_1()
                        .justify_center()
                        .child(theme_mode_label(mode))
                        .on_click(cx.listener(
                            move |this, _event, window, cx| {
                                this.select_theme_mode(mode, window, cx);
                            },
                        ))
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::ACCENT))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("当前显示"),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(theme_variant_label(active_variant)),
                    ),
            )
            .child(dialog_actions().child(self.button(
                "关闭",
                true,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }
}

fn theme_mode_id(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

fn theme_mode_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "跟随系统",
        ThemeMode::Light => "浅色",
        ThemeMode::Dark => "深色",
    }
}

fn theme_variant_label(variant: ThemeVariant) -> &'static str {
    match variant {
        ThemeVariant::Light => "浅色主题",
        ThemeVariant::Dark => "深色主题",
    }
}
