use gpui::{Context, IntoElement, Window, WindowAppearance, div, prelude::*, px, rgb as gpui_rgb};
use khaslana::ThemeMode;
use yororen_ui::theme::{GlobalTheme, Theme, ThemeSet};

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
        cx.set_global(yororen_global_theme(
            variant.window_appearance(),
            ui_theme::active_accent(),
        ));
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
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("显示模式"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("选择应用主题。跟随系统会在操作系统外观变化时自动切换。"),
                    )
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .gap_2()
                            .p_1()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::SURFACE_SUNKEN))
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
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("主题色"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("主题色影响按钮、选中态、链接和进度条等强调色。"),
                    )
                    .child(div().flex().flex_wrap().w_full().gap_2().children(
                        ui_theme::ACCENT_PRESETS.iter().enumerate().map(
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
                                        this.bg(rgb(ui_theme::STATE_HOVER))
                                            .rounded(px(ui_theme::RADIUS_XS))
                                    })
                                    .px_2()
                                    .py_2()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .h(px(28.0))
                                            .rounded_full()
                                            .bg(gpui_rgb(swatch_color)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
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
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("当前显示"),
                    )
                    .child(
                        div()
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

/// 构造注入 Khaslana 语义色板的 Yororen 全局主题。
/// 0.2 API 公开了 surface/content/border/action/status/shadow 全字段，故直接桥接；
/// 不依赖私有实现，也不为外观统一升级依赖。
fn yororen_global_theme(
    appearance: WindowAppearance,
    accent: &ui_theme::AccentPalette,
) -> GlobalTheme {
    let mut light = Theme::default_light();
    apply_yororen_palette(&mut light, ThemeVariant::Light, accent);
    let mut dark = Theme::default_dark();
    apply_yororen_palette(&mut dark, ThemeVariant::Dark, accent);
    let themes = ThemeSet::new(light).dark(dark);
    GlobalTheme::new_with_themes(appearance, themes)
}

fn apply_yororen_palette(
    theme: &mut Theme,
    variant: ThemeVariant,
    accent: &ui_theme::AccentPalette,
) {
    let color = |token| gpui_rgb(ui_theme::resolve_color_for_variant(token, variant)).into();
    let accent_color = |pair: (u32, u32)| match variant {
        ThemeVariant::Light => gpui_rgb(pair.0).into(),
        ThemeVariant::Dark => gpui_rgb(pair.1).into(),
    };

    theme.surface.canvas = color(ui_theme::SURFACE_CANVAS);
    theme.surface.base = color(ui_theme::SURFACE_BASE);
    theme.surface.raised = color(ui_theme::SURFACE_RAISED);
    theme.surface.sunken = color(ui_theme::SURFACE_SUNKEN);
    theme.surface.hover = color(ui_theme::STATE_HOVER);

    theme.content.primary = color(ui_theme::CONTENT_PRIMARY);
    theme.content.secondary = color(ui_theme::CONTENT_SECONDARY);
    theme.content.tertiary = color(ui_theme::CONTENT_TERTIARY);
    theme.content.disabled = color(ui_theme::CONTENT_TERTIARY);
    theme.content.on_primary = accent_color(accent.foreground);
    theme.content.on_status = color(ui_theme::CONTENT_PRIMARY);

    theme.border.default = color(ui_theme::BORDER);
    theme.border.muted = color(ui_theme::BORDER_MUTED);
    theme.border.focus = accent_color(accent.focused_border);
    theme.border.divider = color(ui_theme::BORDER_MUTED);

    theme.action.neutral.bg = color(ui_theme::SURFACE_RAISED);
    theme.action.neutral.hover_bg = color(ui_theme::STATE_HOVER);
    theme.action.neutral.active_bg = color(ui_theme::SECONDARY);
    theme.action.neutral.fg = color(ui_theme::CONTENT_PRIMARY);
    theme.action.neutral.disabled_bg = color(ui_theme::SURFACE_SUNKEN);
    theme.action.neutral.disabled_fg = color(ui_theme::CONTENT_TERTIARY);

    theme.action.primary.bg = accent_color(accent.primary);
    theme.action.primary.hover_bg = accent_color(accent.primary);
    theme.action.primary.active_bg = accent_color(accent.primary);
    theme.action.primary.fg = accent_color(accent.foreground);
    theme.action.primary.disabled_bg = color(ui_theme::SURFACE_SUNKEN);
    theme.action.primary.disabled_fg = color(ui_theme::CONTENT_TERTIARY);

    theme.action.danger.bg = color(ui_theme::DESTRUCTIVE);
    theme.action.danger.hover_bg = color(ui_theme::DESTRUCTIVE);
    theme.action.danger.active_bg = color(ui_theme::DESTRUCTIVE);
    theme.action.danger.fg = color(ui_theme::DESTRUCTIVE_FOREGROUND);
    theme.action.danger.disabled_bg = color(ui_theme::SURFACE_SUNKEN);
    theme.action.danger.disabled_fg = color(ui_theme::CONTENT_TERTIARY);

    theme.status.success.bg = color(ui_theme::FEEDBACK_SUCCESS_BG);
    theme.status.success.fg = color(ui_theme::FEEDBACK_SUCCESS_TEXT);
    theme.status.warning.bg = color(ui_theme::FEEDBACK_WARNING_BG);
    theme.status.warning.fg = color(ui_theme::FEEDBACK_WARNING_TEXT);
    theme.status.error.bg = color(ui_theme::FEEDBACK_ERROR_BG);
    theme.status.error.fg = color(ui_theme::FEEDBACK_ERROR_TEXT);
    theme.status.info.bg = color(ui_theme::FEEDBACK_INFO_BG);
    theme.status.info.fg = color(ui_theme::FEEDBACK_INFO_TEXT);

    theme.shadow.elevation_1 = color(ui_theme::SHADOW_ELEVATION_1);
    theme.shadow.elevation_2 = color(ui_theme::SHADOW_ELEVATION_2);
}
