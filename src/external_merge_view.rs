use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    thread,
};

use gpui::{Context, IntoElement, Window, div, prelude::*, px};

use crate::ui::theme::rgb;
use crate::{
    FieldId, RepositoryView, UiEvent, send_ui_event,
    ui::{components::dialog_actions, theme as ui_theme},
};

static PENDING_EXTERNAL_MERGE_PATH: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn external_merge_detection_label(
    settings: &khaslana::ExternalMergeSettings,
    detection: Option<&(khaslana::ExternalMergeSettings, bool)>,
) -> &'static str {
    match detection.filter(|(detected_settings, _)| detected_settings == settings) {
        Some((_, true)) => "✓ IDEA 可用",
        Some((_, false)) => "检测失败 · 重试",
        None => "检测 IDEA",
    }
}

fn pending_external_merge_path_cell() -> &'static Mutex<Option<String>> {
    PENDING_EXTERNAL_MERGE_PATH.get_or_init(|| Mutex::new(None))
}

fn pending_external_merge_path() -> Option<String> {
    pending_external_merge_path_cell()
        .lock()
        .ok()
        .and_then(|pending| pending.clone())
}

fn set_pending_external_merge_path(path: String) {
    if let Ok(mut pending) = pending_external_merge_path_cell().lock() {
        *pending = Some(path);
    }
}

fn take_pending_external_merge_path() -> Option<String> {
    pending_external_merge_path_cell()
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
}

fn clear_pending_external_merge_path() {
    if let Ok(mut pending) = pending_external_merge_path_cell().lock() {
        *pending = None;
    }
}

impl RepositoryView {
    pub(crate) fn render_external_merge_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pending_path = pending_external_merge_path();
        let save_label = if pending_path.is_some() {
            "保存并继续"
        } else {
            "保存"
        };
        let detection_label = external_merge_detection_label(
            &self.external_merge_form_settings(),
            self.external_merge_detection.as_ref(),
        );

        self.dialog_panel("合并工具", cx)
            .w(px(580.0))
            .when_some(pending_path, |this, path| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(ui_theme::COLOR_WARNING))
                        .bg(rgb(ui_theme::COLOR_WARNING))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
                        .child(format!(
                            "尚未找到可用的 IntelliJ IDEA。配置完成后将继续解决：{path}"
                        )),
                )
            })
            .child(self.toggle_row(
                "external-merge-enabled",
                "启用 IntelliJ IDEA 外部合并",
                self.external_merge_enabled_form,
                |this, _, _| {
                    this.set_external_merge_enabled_form_with_detection(
                        !this.external_merge_enabled_form,
                    );
                },
                cx,
            ))
            .child(self.toggle_row(
                "external-merge-auto-open",
                "选中冲突文件时自动打开 IDEA",
                self.external_merge_auto_open_form,
                |this, _, _| {
                    this.set_external_merge_auto_open_form_with_detection(
                        !this.external_merge_auto_open_form,
                    );
                },
                cx,
            ))
            .child(self.input(FieldId::ExternalMergeIntellijPath, false, window, cx))
            .child(self.button(
                "选择 IDEA 程序",
                !self.busy,
                |this, _, _| this.browse_external_merge_executable(),
                cx,
            ))
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
                    .child("路径可留空。留空时会依次检测 KHASLANA_IDEA_PATH、PATH 中的 idea64 / idea，以及常见 JetBrains 安装目录。开启或保存时会立即验证；未找到工具时不会静默结束，而是保留当前操作并提示配置。"),
            )
            .when(self.last_error.is_some(), |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::DESTRUCTIVE))
                        .child(self.last_error.clone().unwrap_or_default()),
                )
            })
            .child(
                dialog_actions()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| this.cancel_external_merge_settings(),
                        cx,
                    ))
                    .child(self.button(
                        detection_label,
                        !self.busy,
                        |this, _, _| this.test_external_merge_settings_from_form(),
                        cx,
                    ))
                    .child(self.primary_button(
                        save_label,
                        !self.busy,
                        |this, _, _| this.save_external_merge_settings_from_form_and_resume(),
                        cx,
                    )),
            )
    }

    pub(crate) fn request_external_merge_for_path(&mut self, path: String) -> bool {
        let validation = if self.external_merge_settings.enabled {
            khaslana::external_merge::resolve_intellij_idea_command_with_settings(
                &self.external_merge_settings,
            )
        } else {
            Err(khaslana::GitError::Message("外部合并工具未启用".into()))
        };

        if let Err(err) = validation {
            set_pending_external_merge_path(path);
            self.open_external_merge_settings();
            if !self.external_merge_enabled_form {
                self.set_external_merge_enabled_form(true);
            }
            self.status = "需要配置 IntelliJ IDEA 合并工具".into();
            self.last_error = Some(format!(
                "无法启动外部合并工具：{err}。请选择 IDEA 程序路径，保存后会自动继续当前冲突。"
            ));
            return false;
        }

        self.start_external_merge_operation(path);
        true
    }

    fn start_external_merge_operation(&mut self, path: String) {
        let settings = self.external_merge_settings.clone();
        self.diff = None;
        self.diff_headers_expanded = false;
        self.reset_uniform_scroll("diff-scroll");
        self.with_repo_blocking(
            "IntelliJ IDEA 合并结果已应用",
            move |service, repo| {
                service.resolve_conflict_with_intellij_idea_settings(
                    repo,
                    Path::new(&path),
                    &settings,
                )
            },
        );
    }

    pub(crate) fn save_external_merge_settings_from_form_and_resume(&mut self) {
        let settings = self.external_merge_form_settings();
        let has_pending_merge = pending_external_merge_path().is_some();

        if has_pending_merge && !settings.enabled {
            self.last_error = Some("请先启用外部合并工具，再保存并继续当前冲突".into());
            return;
        }

        let resolved_path = if settings.enabled {
            match khaslana::external_merge::resolve_intellij_idea_command_with_settings(&settings) {
                Ok(path) => Some(path),
                Err(err) => {
                    self.status = "需要配置 IntelliJ IDEA 合并工具".into();
                    self.last_error = Some(format!(
                        "未找到可用的 IntelliJ IDEA：{err}。请填写或选择正确的程序路径。"
                    ));
                    return;
                }
            }
        } else {
            None
        };

        self.external_merge_settings = settings;
        self.save_external_merge_settings();
        self.status = resolved_path
            .map(|path| format!("合并工具设置已保存：{}", path.display()))
            .unwrap_or_else(|| "合并工具设置已保存".into());
        self.last_error = None;
        self.active_dialog = None;

        if let Some(path) = take_pending_external_merge_path() {
            self.start_external_merge_operation(path);
        }
    }

    pub(crate) fn cancel_external_merge_settings(&mut self) {
        clear_pending_external_merge_path();
        self.close_dialog();
    }

    pub(crate) fn set_external_merge_enabled_form_with_detection(&mut self, enabled: bool) {
        self.set_external_merge_enabled_form(enabled);
        if !enabled {
            return;
        }

        let settings = self.external_merge_form_settings();
        match khaslana::external_merge::resolve_intellij_idea_command_with_settings(&settings) {
            Ok(path) => {
                self.external_merge_detection = Some((settings, true));
                self.status = format!("已找到 IntelliJ IDEA：{}", path.display());
                self.last_error = None;
            }
            Err(err) => {
                self.external_merge_detection = Some((settings, false));
                self.status = "需要配置 IntelliJ IDEA 合并工具".into();
                self.last_error = Some(format!(
                    "启用前需要配置 IntelliJ IDEA：{err}。请填写路径或点击“选择 IDEA 程序”。"
                ));
            }
        }
    }

    pub(crate) fn set_external_merge_auto_open_form_with_detection(&mut self, enabled: bool) {
        self.set_external_merge_auto_open_form(enabled);
        if !enabled {
            return;
        }

        let settings = self.external_merge_form_settings();
        match khaslana::external_merge::resolve_intellij_idea_command_with_settings(&settings) {
            Ok(path) => {
                self.external_merge_detection = Some((settings, true));
                self.status = format!("已找到 IntelliJ IDEA：{}", path.display());
                self.last_error = None;
            }
            Err(err) => {
                self.external_merge_detection = Some((settings, false));
                self.status = "需要配置 IntelliJ IDEA 合并工具".into();
                self.last_error = Some(format!(
                    "开启自动打开前需要配置 IntelliJ IDEA：{err}。请填写路径或点击“选择 IDEA 程序”。"
                ));
            }
        }
    }

    pub(crate) fn browse_external_merge_executable(&mut self) {
        self.status = "正在选择 IntelliJ IDEA 程序...".to_string();
        self.last_error = None;
        let tx = self.tx.clone();
        // Windows 原生文件框有自己的 COM / 消息循环，不能阻塞在 GPUI 事件回调中。
        thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title("选择 IntelliJ IDEA 启动程序");
            #[cfg(windows)]
            let dialog = dialog.add_filter("IntelliJ IDEA", &["exe", "bat", "cmd"]);
            let path = dialog.pick_file();
            send_ui_event(&tx, UiEvent::ExternalMergeExecutableSelected { path });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_button_only_shows_result_for_current_settings() {
        let settings = khaslana::ExternalMergeSettings {
            enabled: true,
            auto_open_intellij: false,
            intellij_path: "idea64.exe".into(),
        };
        let succeeded = (settings.clone(), true);
        assert_eq!(
            external_merge_detection_label(&settings, Some(&succeeded)),
            "✓ IDEA 可用"
        );

        let mut changed = settings.clone();
        changed.intellij_path = "other.exe".into();
        assert_eq!(
            external_merge_detection_label(&changed, Some(&succeeded)),
            "检测 IDEA"
        );
    }
}
