// 快捷键设置页 UI：列出全部可配置动作，支持录制新快捷键与恢复默认。

use std::ops::DerefMut;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px};

use crate::ui::theme::rgb;
use crate::{RepositoryView, ShortcutAction, ui::theme as ui_theme};

/// 把 GPUI keystroke 字符串格式化为用户可读的显示文本。
/// 例如 "ctrl-shift-f" → "Ctrl+Shift+F"，"f5" → "F5"，"ctrl-," → "Ctrl+,"
pub(crate) fn format_keystroke(keystroke: &str) -> String {
    keystroke
        .split('-')
        .map(|part| match part {
            "ctrl" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            "cmd" | "super" | "win" => "Super".to_string(),
            "comma" => ",".to_string(),
            "minus" => "-".to_string(),
            "plus" => "+".to_string(),
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "backspace" => "Backspace".to_string(),
            "delete" => "Delete".to_string(),
            "tab" => "Tab".to_string(),
            "space" => "Space".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            _ => {
                // 单字符键名大写（如 "f" → "F"）；功能键原样（如 "f5"、"home"→"Home"）
                if part.len() == 1 {
                    part.to_uppercase()
                } else {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().chain(chars).collect(),
                        None => String::new(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 把 KeyDownEvent 的 keystroke 转为 GPUI 绑定格式的字符串（如 "ctrl-shift-f"）。
/// 修饰键按 ctrl > alt > shift > platform 排序，主键小写。
fn keystroke_to_string(event: &KeyDownEvent) -> String {
    let ks = &event.keystroke;
    let mut parts = Vec::new();
    if ks.modifiers.control {
        parts.push("ctrl");
    }
    if ks.modifiers.alt {
        parts.push("alt");
    }
    if ks.modifiers.shift {
        parts.push("shift");
    }
    if ks.modifiers.platform {
        parts.push("cmd");
    }
    // 主键直接使用 GPUI keystroke 的 key 字段（已小写）。
    let key = ks.key.as_ref();
    parts.push(key);
    parts.join("-")
}

impl RepositoryView {
    /// 快捷键设置页 body。
    pub(crate) fn render_shortcuts_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let recording = self.recording_shortcut;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if let Some(action) = this.recording_shortcut {
                    // Esc 取消录制。
                    if event.keystroke.key.as_str() == "escape" {
                        this.recording_shortcut = None;
                        cx.notify();
                        return;
                    }
                    let ks = keystroke_to_string(event);
                    // 冲突检查：若已被其它动作占用则拒绝并提示。
                    if let Some(conflict) = crate::find_shortcut_conflict(&this.shortcut_bindings, action, &ks) {
                        this.recording_shortcut = None;
                        this.notify_warning(
                            format!("快捷键 {} 已被「{}」占用", format_keystroke(&ks), conflict.label()),
                            cx,
                        );
                        return;
                    }
                    // 通过检查，更新绑定。
                    this.shortcut_bindings.bindings.insert(action.action_id().to_string(), ks);
                    this.recording_shortcut = None;
                    this.save_shortcut_bindings();
                    crate::register_all_key_bindings(&mut cx.deref_mut(), &this.shortcut_bindings);
                    cx.notify();
                }
            }))
            // 说明文字
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("点击「重新绑定」后按下组合键录入；按 Esc 取消录制。点「恢复默认」复位单条快捷键。"),
            )
            // 动作列表
            .children(ShortcutAction::ALL.iter().map(|action| {
                let action_val = *action;
                let is_recording = recording == Some(action_val);
                let keystroke = action_val.keystroke(&self.shortcut_bindings).to_string();
                let display = format_keystroke(&keystroke);
                let is_default = action_val.default_keystroke() == keystroke.as_str();

                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .py(px(4.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(action_val.label()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(120.0))
                            .text_size(px(11.0))
                            .font_family("Consolas, monospace")
                            .text_color(rgb(if is_recording {
                                ui_theme::FOREGROUND
                            } else {
                                ui_theme::MUTED_FOREGROUND
                            }))
                            .text_align(gpui::TextAlign::Center)
                            .child(if is_recording {
                                "按下组合键…".to_string()
                            } else {
                                display
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap_1()
                            .child(self.button(
                                if is_recording { "取消" } else { "重新绑定" },
                                true,
                                move |this, _window, _cx| {
                                    if this.recording_shortcut == Some(action_val) {
                                        this.recording_shortcut = None;
                                    } else {
                                        this.recording_shortcut = Some(action_val);
                                    }
                                },
                                cx,
                            ))
                            .when(!is_default, |this_row| {
                                this_row.child(self.button(
                                    "恢复默认",
                                    true,
                                    move |this, _window, cx| {
                                        this.shortcut_bindings.bindings.insert(
                                            action_val.action_id().to_string(),
                                            action_val.default_keystroke().to_string(),
                                        );
                                        this.save_shortcut_bindings();
                                        crate::register_all_key_bindings(
                                            &mut cx.deref_mut(),
                                            &this.shortcut_bindings,
                                        );
                                    },
                                    cx,
                                ))
                            }),
                    )
                    .into_any_element()
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_keystroke_ctrl_shift_combo() {
        assert_eq!(format_keystroke("ctrl-shift-f"), "Ctrl+Shift+F");
        assert_eq!(format_keystroke("ctrl-shift-l"), "Ctrl+Shift+L");
    }

    #[test]
    fn format_keystroke_single_key() {
        assert_eq!(format_keystroke("f5"), "F5");
    }

    #[test]
    fn format_keystroke_ctrl_comma() {
        assert_eq!(format_keystroke("ctrl-,"), "Ctrl+,");
    }

    #[test]
    fn format_keystroke_ctrl_number() {
        assert_eq!(format_keystroke("ctrl-1"), "Ctrl+1");
    }

    #[test]
    fn find_conflict_detects_other_action() {
        // 默认绑定中 refresh=f5, fetch=ctrl-shift-f。
        let bindings = crate::default_shortcut_bindings();
        // 给 refresh 绑定 ctrl-shift-f（fetch 的默认）应冲突到 fetch。
        let conflict = crate::find_shortcut_conflict(
            &bindings,
            ShortcutAction::Refresh,
            "ctrl-shift-f",
        );
        assert_eq!(conflict, Some(ShortcutAction::Fetch));
    }

    #[test]
    fn find_conflict_no_self_conflict() {
        let bindings = crate::default_shortcut_bindings();
        // 一个动作的当前绑定不与自己冲突。
        let conflict = crate::find_shortcut_conflict(
            &bindings,
            ShortcutAction::Refresh,
            "f5",
        );
        assert_eq!(conflict, None);
    }
}
