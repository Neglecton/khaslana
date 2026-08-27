// 快捷键设置页 UI：列出全部可配置动作，支持录制新快捷键与恢复默认。

use std::ops::DerefMut;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px};

use crate::ui::{components::tooltip_text, theme::rgb};
use crate::{
    RepositoryView, ShortcutAction, ShortcutRecordingTarget, ui::theme as ui_theme, workflow_view,
};

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
pub(crate) fn keystroke_to_string(event: &KeyDownEvent) -> String {
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

fn shortcut_row_is_recording(
    recording: Option<&ShortcutRecordingTarget>,
    action: ShortcutAction,
) -> bool {
    matches!(
        recording,
        Some(ShortcutRecordingTarget::App(recording_action)) if *recording_action == action
    )
}

/// 快捷键设置按钮只在录制期间禁用“恢复默认”，避免录入中的状态被旁路修改。
fn shortcut_reset_enabled(recording: Option<&ShortcutRecordingTarget>) -> bool {
    recording.is_none()
}

fn shortcut_reset_disabled_reason(
    recording: Option<&ShortcutRecordingTarget>,
) -> Option<&'static str> {
    (!shortcut_reset_enabled(recording)).then_some("请先结束快捷键录制")
}

impl RepositoryView {
    /// 快捷键设置页 body。
    pub(crate) fn render_shortcuts_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = self.recording_shortcut.as_ref();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("应用快捷键"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("点击「重新绑定」后按下组合键录入；按 Esc 取消录制。点「恢复默认」复位单条快捷键。"),
                    ),
            )
            // 动作列表以轻量分隔组织，避免每条快捷键形成独立卡片。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .children(ShortcutAction::ALL.iter().map(|action| {
                        let action_val = *action;
                        let is_recording = shortcut_row_is_recording(recording, action_val);
                        let keystroke = action_val.keystroke(&self.shortcut_bindings).to_string();
                        let display = format_keystroke(&keystroke);
                        let is_default = action_val.default_keystroke() == keystroke.as_str();
                        let reset_enabled = shortcut_reset_enabled(recording);
                        let reset_disabled_reason = shortcut_reset_disabled_reason(recording);

                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER_MUTED))
                            .when(is_recording, |this| this.bg(rgb(ui_theme::STATE_SELECTION)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(action_val.label()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(120.0))
                            .text_size(px(11.0))
                            .font_family("Consolas, monospace")
                            .text_color(rgb(if is_recording {
                                ui_theme::CONTENT_PRIMARY
                            } else {
                                ui_theme::CONTENT_SECONDARY
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
                            // 重新绑定 / 取消按钮：自绘并用唯一 id，避免 button 组件 label 相同导致 id 冲突。
                            .child(
                                div()
                                    .id(format!("shortcut-rebind-{}", action_val.action_id()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .min_h(px(28.0))
                                    .px_3()
                                    .py_1()
                                    .border_1()
                                    .border_color(rgb(ui_theme::BORDER_MUTED))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::SURFACE_RAISED))
                                    .text_size(px(12.0))
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .cursor_pointer()
                                    .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                                    .on_click(cx.listener(move |this, _event, window, cx| {
                                        if shortcut_row_is_recording(
                                            this.recording_shortcut.as_ref(),
                                            action_val,
                                        ) {
                                            // 取消录制，恢复正常绑定。
                                            this.recording_shortcut = None;
                                            crate::register_all_key_bindings(
                                                &mut cx.deref_mut(),
                                                &this.shortcut_bindings,
                                                &this.workflow_shortcut_bindings,
                                                false,
                                            );
                                        } else {
                                            // 进入录制态：夺取焦点到设置中心面板（使 keydown dispatch_path 进入 overlay），
                                            // 跳过快捷键绑定（使按键不匹配 action，keydown 能正常到达 capture_key_down）。
                                            this.recording_shortcut =
                                                Some(ShortcutRecordingTarget::App(action_val));
                                            window.focus(&this.settings_center_focus);
                                            crate::register_all_key_bindings(
                                                &mut cx.deref_mut(),
                                                &this.shortcut_bindings,
                                                &this.workflow_shortcut_bindings,
                                                true,
                                            );
                                        }
                                        cx.notify();
                                    }))
                                    .child(if is_recording { "取消" } else { "重新绑定" }),
                            )
                            .when(!is_default, |this_row| {
                                this_row.child(
                                    div()
                                        .id(format!("shortcut-reset-{}", action_val.action_id()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .min_h(px(28.0))
                                        .px_3()
                                        .py_1()
                                        .border_1()
                                        .border_color(rgb(ui_theme::BORDER_MUTED))
                                        .rounded(px(ui_theme::RADIUS_XS))
                                        .bg(rgb(ui_theme::SURFACE_RAISED))
                                        .text_size(px(12.0))
                                        .text_color(rgb(if reset_enabled {
                                            ui_theme::CONTENT_PRIMARY
                                        } else {
                                            ui_theme::CONTENT_TERTIARY
                                        }))
                                        .when(reset_enabled, |this| {
                                            this.cursor_pointer()
                                                .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                                        })
                                        .when(!reset_enabled, |this| {
                                            this.cursor_not_allowed().opacity(0.62)
                                        })
                                        .when_some(reset_disabled_reason, |this, reason| {
                                            this.tooltip(move |_window, cx| tooltip_text(reason, cx))
                                        })
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            if reset_enabled {
                                                // 恢复默认与录制走同一条统一冲突检查：
                                                // 默认键位可能已被其它动作的自定义键或
                                                // 某个工作流绑定占用，占用时拒绝并提示。
                                                let default_keystroke =
                                                    action_val.default_keystroke();
                                                let target = ShortcutRecordingTarget::App(
                                                    action_val,
                                                );
                                                if let Some(conflict) =
                                                    crate::find_keystroke_conflict(
                                                        &this.shortcut_bindings,
                                                        &this.workflow_shortcut_bindings,
                                                        &target,
                                                        default_keystroke,
                                                    )
                                                {
                                                    this.notify_warning(
                                                        format!(
                                                            "快捷键 {} 已被「{}」占用，无法恢复默认",
                                                            format_keystroke(default_keystroke),
                                                            conflict.describe()
                                                        ),
                                                        cx,
                                                    );
                                                    return;
                                                }
                                                this.shortcut_bindings.bindings.insert(
                                                    action_val.action_id().to_string(),
                                                    default_keystroke.to_string(),
                                                );
                                                this.save_shortcut_bindings();
                                                crate::register_all_key_bindings(
                                                    &mut cx.deref_mut(),
                                                    &this.shortcut_bindings,
                                                    &this.workflow_shortcut_bindings,
                                                    false,
                                                );
                                                cx.notify();
                                            }
                                        }))
                                        .child("恢复默认"),
                                )
                            }),
                    )
                            .into_any_element()
                    })))
            // 工作流快捷键分区：绑定入口在工作流页模板右键菜单，此处展示与清除。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mt_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child("工作流快捷键"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(
                                "在工作流页右键模板行选择「绑定快捷键...」录制；此处可查看与清除。",
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .children(if self.workflow_shortcut_bindings.bindings.is_empty() {
                        vec![
                            div()
                                .py_2()
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::CONTENT_TERTIARY))
                                .child("暂无工作流快捷键")
                                .into_any_element(),
                        ]
                    } else {
                        self.workflow_shortcut_bindings
                            .bindings
                            .iter()
                            .map(|(file, binding)| {
                                        let file = file.clone();
                                        let display = format_keystroke(&binding.keystroke);
                                        let background = binding.background;
                                        let template_name = workflow_view::workflow_display_name_for_file(
                                            &self.workflow_templates,
                                            &file,
                                        );
                                        let clear_enabled =
                                            shortcut_reset_enabled(self.recording_shortcut.as_ref());

                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .py_2()
                                            .border_b_1()
                                            .border_color(rgb(ui_theme::BORDER_MUTED))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .min_w(px(0.0))
                                                            .truncate()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                                            .child(template_name),
                                                    )
                                                    .when(background, |this| {
                                                        this.child(
                                                            div()
                                                                .flex_none()
                                                                .px(px(6.0))
                                                                .py(px(1.0))
                                                                .rounded(px(ui_theme::RADIUS_PILL))
                                                                .bg(rgb(ui_theme::SECONDARY))
                                                                .text_size(px(10.0))
                                                                .text_color(rgb(ui_theme::SECONDARY_FOREGROUND))
                                                                .child("后台"),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .w(px(120.0))
                                                    .text_size(px(11.0))
                                                    .font_family("Consolas, monospace")
                                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                                    .text_align(gpui::TextAlign::Center)
                                                    .child(display),
                                            )
                                            .child(
                                                div()
                                                    .id(format!("workflow-shortcut-clear-{file}"))
                                                    .flex()
                                                    .flex_none()
                                                    .items_center()
                                                    .justify_center()
                                                    .min_h(px(28.0))
                                                    .px_3()
                                                    .py_1()
                                                    .border_1()
                                                    .border_color(rgb(ui_theme::BORDER_MUTED))
                                                    .rounded(px(ui_theme::RADIUS_XS))
                                                    .bg(rgb(ui_theme::SURFACE_RAISED))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(if clear_enabled {
                                                        ui_theme::CONTENT_PRIMARY
                                                    } else {
                                                        ui_theme::CONTENT_TERTIARY
                                                    }))
                                                    .when(clear_enabled, |this| {
                                                        this.cursor_pointer()
                                                            .hover(|this| this.bg(rgb(ui_theme::STATE_HOVER)))
                                                    })
                                                    .when(!clear_enabled, |this| {
                                                        this.cursor_not_allowed().opacity(0.62)
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _event, _window, cx| {
                                                            if clear_enabled {
                                                                this.workflow_shortcut_bindings
                                                                    .bindings
                                                                    .remove(&file);
                                                                this.persist_workflow_shortcut_bindings(cx);
                                                            }
                                                        },
                                                    ))
                                                    .child("清除"),
                                            )
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                                    }),
            )
    }
}

#[cfg(test)]
#[path = "tests/shortcuts_view.rs"]
mod tests;
