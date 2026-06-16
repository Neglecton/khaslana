// AI 功能 UI 层：供应商设置弹窗、commit message 生成、code review 渲染。
//
// 复杂 AI 逻辑（HTTP 请求、prompt 构造）在 src/ai/ 中实现；
// 这里只负责组合布局、状态、交互和渲染。

use std::sync::Arc;

use gpui::{Context, IntoElement, Window, div, point, prelude::*, px, rgb};
use khaslana::{
    AiApiType, AiReviewResult, ChatClient, ChatMessage, ChatRole, DiffEncodingChoice, DiffLineKind,
    DiffScope,
};

use crate::{
    FieldId, RepositoryView,
    ui::{components::dialog_actions, theme as ui_theme},
    ui_helpers::{ScrollbarMode, scrollable_frame_when},
};

impl RepositoryView {
    /// 渲染 AI 供应商设置弹窗。
    pub(crate) fn render_ai_provider_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("AI 设置", cx)
            .w(px(560.0))
            .child(self.toggle_row(
                "ai-enabled",
                "启用 AI 功能",
                self.ai_enabled_form,
                |this, _, _| {
                    this.set_ai_enabled_form(!this.ai_enabled_form);
                },
                cx,
            ))
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::TEXT_MUTED))
                    .child(format!("接口类型：{}", AiApiType::ChatCompletions.label())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.input(FieldId::AiBaseUrl, false, window, cx))
                    .child(self.input(FieldId::AiApiKey, false, window, cx))
                    .child(self.input(FieldId::AiModel, false, window, cx)),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .bg(rgb(ui_theme::PANEL_TINT))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::TEXT_FAINT))
                    .child("API Key 可选（本地模型如 Ollama 可留空）；明文保存在本地配置数据库，请勿在共享环境使用。temperature、max_tokens、超时使用默认值（0.3 / 800 / 60s）。"),
            )
            // 测试连接期间的进度/结果状态行：在弹窗内部展示，避免被弹窗遮挡。
            .when(self.busy, |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::TEXT_MUTED))
                        .child(self.status.clone()),
                )
            })
            .when(!self.busy && self.last_error.is_some(), |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::DANGER_STRONG))
                        .truncate()
                        .child(self.last_error.clone().unwrap_or_default()),
                )
            })
            .child(
                dialog_actions()
                    // 取消按钮始终可用，即使测试连接进行中也能关闭弹窗。
                    .child(self.button("取消", true, |this, _, _| this.close_dialog(), cx))
                    .child(self.button(
                        "测试连接",
                        !self.busy,
                        |this, _, _| this.test_ai_connection(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        |this, _, _| this.save_ai_provider_settings_from_form_and_close(),
                        cx,
                    )),
            )
    }

    /// AI 生成提交信息按钮是否可用。
    pub(crate) fn ai_commit_button_enabled(&self) -> bool {
        self.ai_settings.is_usable() && !self.ai_commit_loading && !self.busy
    }

    /// 渲染 commit message 输入框下方的 AI 生成按钮。
    pub(crate) fn render_ai_commit_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.ai_commit_button_enabled();
        let label = if self.ai_commit_loading {
            "生成中..."
        } else {
            "AI 生成"
        };
        div().flex().items_center().child(self.button(
            label,
            enabled,
            |this, _, _| this.generate_ai_commit_message(),
            cx,
        ))
    }

    /// 触发 AI 生成 commit message。
    pub(crate) fn generate_ai_commit_message(&mut self) {
        if self.ai_commit_loading {
            return;
        }
        if !self.ai_settings.is_usable() {
            self.last_error = Some("请先在 AI 设置中配置并启用供应商".into());
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let Some(repo_path) = self.repo_path.clone() else {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        };
        let staged_paths = self.change_paths(DiffScope::Staged);
        if staged_paths.is_empty() {
            self.last_error = Some("暂存区没有文件，无法生成提交信息".into());
            return;
        }

        self.ai_commit_loading = true;
        self.ai_commit_buffer.clear();
        self.status = "正在生成提交信息".into();
        self.last_error = None;

        let service = self.service_for_tab(tab_id);
        let settings = self.ai_settings.clone();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target(&settings.normalized_base_url());
        let tx = self.tx.clone();
        self.tasks.spawn(crate::TaskKind::Long, move || {
            let result = (|| -> khaslana::Result<String> {
                let repo = git2::Repository::open(&repo_path)?;
                // 收集所有 staged 文件的 diff 文本。
                let mut diff_text = String::new();
                for path in &staged_paths {
                    let file_diff = service.diff_for_path(
                        &repo,
                        std::path::Path::new(path),
                        DiffScope::Staged,
                        false,
                        DiffEncodingChoice::Utf8,
                    )?;
                    if file_diff.is_binary {
                        diff_text.push_str(&format!("--- {} (二进制文件)\n", path));
                        continue;
                    }
                    diff_text.push_str(&format!("--- a/{}\n", path));
                    diff_text.push_str(&file_diff_to_patch_text(&file_diff));
                }
                let (system, user) = khaslana::ai::commit_message_prompts(&diff_text, None);
                let client = ChatClient::new(settings, proxy_url);
                // 流式请求：每个 content chunk 增量推回 UI，让用户实时看到生成进度。
                let tx = tx.clone();
                let result = client.request_stream(&[system, user], &mut |delta| {
                    if let khaslana::StreamDelta::Content(text) = delta {
                        crate::send_ui_event(
                            &tx,
                            crate::UiEvent::AiCommitMessageDelta { delta: text },
                        );
                    }
                })?;
                Ok(result.content)
            })();
            match result {
                Ok(message) => {
                    crate::send_ui_event(&tx, crate::UiEvent::AiCommitMessageGenerated { message });
                }
                Err(err) => {
                    crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiRequestFailed {
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 测试 AI 连接：发送一个最小请求。
    pub(crate) fn test_ai_connection(&mut self) {
        if self.busy {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let settings = self.ai_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.busy = true;
        self.status = "正在测试 AI 连接".into();
        self.last_error = None;

        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target(&settings.normalized_base_url());
        let tx = self.tx.clone();
        self.tasks.spawn(crate::TaskKind::Long, move || {
            let client = ChatClient::new(settings, proxy_url);
            let test_message = ChatMessage {
                role: ChatRole::User,
                content: "请回复 OK".into(),
            };
            match client.request(&[test_message]) {
                Ok(result) => {
                    let message = if result.content.trim().is_empty() {
                        "AI 连接测试通过（返回空内容）".to_string()
                    } else {
                        format!(
                            "AI 连接测试通过：{}",
                            result.content.chars().take(50).collect::<String>()
                        )
                    };
                    crate::send_ui_event(&tx, crate::UiEvent::AiConnectionTested { message });
                }
                Err(err) => {
                    crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiRequestFailed {
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// AI code review 按钮是否可用。
    pub(crate) fn ai_review_button_enabled(&self) -> bool {
        self.ai_settings.is_usable()
            && !self.ai_review_loading
            && !self.busy
            && self.browse.diff.is_some()
    }

    /// 触发 AI code review 当前选中文件。
    pub(crate) fn generate_ai_review(&mut self) {
        if self.ai_review_loading {
            return;
        }
        if !self.ai_settings.is_usable() {
            self.last_error = Some("请先在 AI 设置中配置并启用供应商".into());
            return;
        }
        let Some(diff) = self.browse.diff.clone() else {
            self.last_error = Some("请先选择要评审的差异文件".into());
            return;
        };
        let file_path = diff.path.clone();
        let branch_name = self
            .browse
            .target
            .as_ref()
            .map(|target| target.display_name.clone())
            .unwrap_or_else(|| "目标分支".to_string());
        let diff_text = if diff.is_binary {
            format!("（{file_path} 是二进制文件，无法评审）")
        } else {
            file_diff_to_patch_text(&diff)
        };

        self.ai_review_loading = true;
        self.ai_review = None;
        self.ai_review_buffer.clear();
        self.ai_review_reasoning_buffer.clear();
        self.scroll_handle("ai-review-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.status = "正在生成 AI 评审".into();
        self.last_error = None;

        let settings = self.ai_settings.clone();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target(&settings.normalized_base_url());
        let tx = self.tx.clone();
        self.tasks.spawn(crate::TaskKind::Long, move || {
            let (system, user) = khaslana::ai::review_prompts(&file_path, &diff_text, &branch_name);
            let client = ChatClient::new(settings, proxy_url);
            // 流式请求：content 和 reasoning 增量分别推回 UI，实时渲染评审内容。
            let tx = tx.clone();
            let result = client.request_stream(&[system, user], &mut |delta| {
                let (content_delta, reasoning_delta) = match delta {
                    khaslana::StreamDelta::Content(text) => (Some(text), None),
                    khaslana::StreamDelta::Reasoning(text) => (None, Some(text)),
                };
                crate::send_ui_event(
                    &tx,
                    crate::UiEvent::AiReviewDelta {
                        content_delta,
                        reasoning_delta,
                    },
                );
            });
            match result {
                Ok(result) => {
                    let review = AiReviewResult {
                        content: result.content,
                        reasoning: result.reasoning,
                    };
                    crate::send_ui_event(&tx, crate::UiEvent::AiReviewGenerated { review });
                }
                Err(err) => {
                    crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiRequestFailed {
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 渲染 AI 评审面板（可折叠），放在对比模式 diff 视图下方。
    pub(crate) fn render_ai_review_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.ai_review_loading;
        let review = self.ai_review.clone();
        let expanded = self.ai_review_expanded;

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::HEADER_BG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::ACCENT_STRONG))
                            .child("AI 评审"),
                    )
                    .when(loading, |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::TEXT_FAINT))
                                .child("生成中..."),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(review.clone(), |this, _review| {
                        this.child(self.button(
                            if expanded { "收起" } else { "展开" },
                            !loading,
                            |this, _, _| this.ai_review_expanded = !this.ai_review_expanded,
                            cx,
                        ))
                        .child(self.button(
                            "重新生成",
                            self.ai_review_button_enabled(),
                            |this, _, _| this.generate_ai_review(),
                            cx,
                        ))
                    })
                    .child(self.button(
                        "AI Review",
                        self.ai_review_button_enabled(),
                        |this, _, _| this.generate_ai_review(),
                        cx,
                    )),
            );

        let body = if loading {
            // 流式生成时实时显示缓冲内容（正文 + 可选思考链）。
            let content = self.ai_review_buffer.clone();
            let reasoning = (!self.ai_review_reasoning_buffer.is_empty())
                .then(|| self.ai_review_reasoning_buffer.clone());
            let has_output = !content.is_empty() || reasoning.is_some();
            let content_view = div()
                .px_3()
                .py_2()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(rgb(if has_output {
                    ui_theme::TEXT
                } else {
                    ui_theme::TEXT_FAINT
                }))
                .child(if has_output {
                    String::new()
                } else {
                    "正在等待 AI 返回评审结果...".to_string()
                })
                .when(has_output, |this| {
                    this.child(div().child(content.clone())).when_some(
                        reasoning,
                        |this, reasoning| {
                            this.child(
                                div()
                                    .mt_2()
                                    .pt_2()
                                    .border_t_1()
                                    .border_color(rgb(ui_theme::BORDER_MUTED))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::TEXT_FAINT))
                                    .child(
                                        div().font_weight(gpui::FontWeight::BOLD).child("思考链："),
                                    )
                                    .child(div().child(reasoning)),
                            )
                        },
                    )
                });
            let handle = self.scroll_handle("ai-review-scroll");
            Some(
                scrollable_frame_when(
                    "ai-review-scroll",
                    ScrollbarMode::Vertical,
                    content_view.into_any_element(),
                    handle,
                    has_output,
                    cx,
                )
                .into_any_element(),
            )
        } else if let Some(review) = review {
            if expanded {
                let handle = self.scroll_handle("ai-review-scroll");
                Some(
                    scrollable_frame_when(
                        "ai-review-scroll",
                        ScrollbarMode::Vertical,
                        render_review_content(&review),
                        handle,
                        true,
                        cx,
                    )
                    .into_any_element(),
                )
            } else {
                Some(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::TEXT_MUTED))
                        .truncate()
                        .child(review.content.chars().take(80).collect::<String>())
                        .into_any_element(),
                )
            }
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .max_h(px(360.0))
            .min_h(px(0.0))
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::PANEL_BG))
            .child(header)
            .when_some(body, |this, body| {
                this.child(div().flex_1().min_h(px(0.0)).child(body))
            })
    }
}

/// 把 FileDiff 转成 patch 风格文本，供 AI prompt 使用。
fn file_diff_to_patch_text(diff: &khaslana::FileDiff) -> String {
    if diff.is_binary {
        return format!("（{} 是二进制文件）\n", diff.path);
    }
    let mut text = String::new();
    for line in &diff.lines {
        match line.kind {
            DiffLineKind::Header => {
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Context => {
                text.push(' ');
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Added => {
                text.push('+');
                text.push_str(&line.content);
                text.push('\n');
            }
            DiffLineKind::Removed => {
                text.push('-');
                text.push_str(&line.content);
                text.push('\n');
            }
        }
    }
    text
}

/// 渲染评审正文 + 可选思考链。
fn render_review_content(review: &Arc<AiReviewResult>) -> gpui::AnyElement {
    div()
        .px_3()
        .py_2()
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(rgb(ui_theme::TEXT))
        .child(div().child(review.content.clone()))
        .when_some(review.reasoning.clone(), |this, reasoning| {
            this.child(
                div()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::TEXT_FAINT))
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("思考链："))
                    .child(div().child(reasoning)),
            )
        })
        .into_any_element()
}
