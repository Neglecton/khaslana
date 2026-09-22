use gpui::{Context, IntoElement, Window, WindowAppearance, div, prelude::*, px, rgb as gpui_rgb};
use khaslana::ThemeMode;

use crate::{
    RepositoryView,
    ui::{
        components::segmented_button,
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

    /// 同时更新 Khaslana 语义色板和 Yororen 全局主题，避免混用组件出现深浅色割裂。
    /// Yororen 组件的聚焦边框会跟随当前主题色，其余保持默认色板。
    pub(crate) fn apply_theme_for_appearance(
        &mut self,
        appearance: WindowAppearance,
        cx: &mut Context<Self>,
    ) {
        let variant = ui_theme::variant_for_mode(self.theme_mode, appearance);
        let variant_changed = ui_theme::active_variant() != variant;
        ui_theme::set_active_variant(variant);
        ui_theme::set_active_accent(self.theme_accent);
        // Kit 组件与自绘视图共用同一套语义 token；Kit 的外观由 ThemeColor 一比一
        // 派生，所以这里重建整个 Theme 而不是改零散字段。
        crate::ui::kit_theme::apply(cx, variant, ui_theme::active_accent());
        if variant_changed {
            // 语法高亮颜色绑定深浅主题（syntect 内置主题二选一）：
            // 清空全部槽位并按新变体从现存内容补算，不做 git 重载。
            self.invalidate_and_refresh_syntax_highlights();
        }
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

    pub(crate) fn load_theme_accent(storage: &khaslana::AppStorage) -> usize {
        storage
            .load_theme_accent()
            .inspect_err(|err| tracing::warn!("theme accent load skipped: {err}"))
            .unwrap_or(0)
    }

    fn select_theme_accent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_accent = index;
        self.apply_theme_for_appearance(window.appearance(), cx);
        window.refresh();

        let label = ui_theme::ACCENT_PRESETS
            .get(index)
            .map(|(name, _)| *name)
            .unwrap_or("靛蓝");
        match self.storage.save_theme_accent(index) {
            Ok(()) => self.notify_success(format!("主题色已切换为{label}"), cx),
            Err(err) => self.notify_error(format!("主题色保存失败：{err}"), cx),
        }
    }

    pub(crate) fn render_theme_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_variant = ui_theme::variant_for_mode(self.theme_mode, window.appearance());
        let modes = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(ui_theme::SPACE_4))
            .child(
                crate::ui::components::settings_card(
                    "显示模式",
                    Some("选择应用主题。跟随系统会在操作系统外观变化时自动切换。"),
                )
                .child(crate::ui::components::settings_segmented_group().children(
                    modes.into_iter().map(|mode| {
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
                    }),
                )),
            )
            .child(
                crate::ui::components::settings_card(
                    "主题色",
                    Some("主题色影响按钮、选中态、链接和进度条等强调色。"),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .w_full()
                        .gap(px(ui_theme::SPACE_2))
                        .children(ui_theme::ACCENT_PRESETS.iter().enumerate().map(
                            |(index, (name, palette))| {
                                let selected = self.theme_accent == index;
                                // 色块展示预设的主色；选中状态只用强调边框，不叠加卡片。
                                let swatch_color = match active_variant {
                                    ThemeVariant::Light => palette.primary.0,
                                    ThemeVariant::Dark => palette.primary.1,
                                };
                                div()
                                    .id(format!("theme-accent-{index}"))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .hover(|this| {
                                        this.bg(rgb(ui_theme::WB_ROW_HOVER))
                                            .rounded(px(ui_theme::RADIUS_SM))
                                    })
                                    .px_2()
                                    .py_2()
                                    .rounded(px(ui_theme::RADIUS_SM))
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .h(px(28.0))
                                            .rounded_full()
                                            .bg(gpui_rgb(swatch_color)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(ui_theme::TYPE_META))
                                            .text_color(if selected {
                                                rgb(ui_theme::CONTENT_PRIMARY)
                                            } else {
                                                rgb(ui_theme::CONTENT_SECONDARY)
                                            })
                                            .child(*name),
                                    )
                                    .when(selected, |this| {
                                        this.border_1().border_color(rgb(ui_theme::PRIMARY))
                                    })
                                    .on_click(cx.listener(move |this, _event, window, cx| {
                                        this.select_theme_accent(index, window, cx);
                                    }))
                            },
                        )),
                ),
            )
            .child(
                crate::ui::components::settings_card("当前显示", None).child(
                    div()
                        .text_size(px(ui_theme::TYPE_BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                        .child(theme_variant_label(active_variant)),
                ),
            )
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

#[cfg(test)]
#[path = "tests/theme_view.rs"]
mod tests;
