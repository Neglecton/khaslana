// AI 功能 UI 层：供应商设置弹窗、commit message 生成、code review 渲染。
//
// 复杂 AI 逻辑（HTTP 请求、prompt 构造）在 src/ai/ 中实现；
// 这里只负责组合布局、状态、交互和渲染。

use std::sync::Arc;

use gpui::{Context, IntoElement, Window, div, point, prelude::*, px};
use khaslana::{
    AiApiType, AiReviewResult, ChatClient, ChatMessage, ChatRole, DiffEncodingChoice, DiffLineKind,
    DiffScope,
};

use crate::ui::theme::rgb;
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
        div()
            .flex()
            .flex_col()
            .gap_3()
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
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("API Key 可选（本地模型如 Ollama 可留空）；明文保存在本地配置数据库，请勿在共享环境使用。temperature、max_tokens、超时使用默认值（0.3 / 800 / 60s）。"),
            )
            // 测试连接期间的进度/结果状态行：在弹窗内部展示，避免被弹窗遮挡。
            .when(self.busy, |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(self.status.clone()),
                )
            })
            .when(!self.busy && self.last_error.is_some(), |this| {
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
                    .child(self.button(
                        "测试连接",
                        !self.busy,
                        |this, _, _| this.test_ai_connection(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        |this, _, cx| {
                            this.save_ai_provider_settings_from_form();
                            this.notify_settings_save("AI 设置已保存", cx);
                        },
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
                // 空正文按失败处理并给出可读提示：仅返回思考过程（reasoning 模型）
                // 或完全为空时，避免按钮恢复后输入框无内容也无报错。
                khaslana::ai::validate_generated_content(
                    &result,
                    "AI 返回的提交信息为空，请重试或检查模型配置",
                    "AI 未返回提交信息正文（仅返回了思考过程），请重试或更换模型",
                )
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

    /// AI 冲突合并建议按钮是否可用。
    pub(crate) fn ai_conflict_merge_button_enabled(&self) -> bool {
        self.ai_settings.is_usable() && !self.ai_conflict_loading && !self.busy
    }

    /// 冲突工作台「AI 合并建议」入口：守卫后启动生成；
    /// 草稿已有块处理或手工修改时先弹覆盖确认。
    pub(crate) fn generate_ai_conflict_merge(&mut self) {
        if self.ai_conflict_loading {
            return;
        }
        if !self.ai_settings.is_usable() {
            self.last_error = Some("请先在 AI 设置中配置并启用供应商".into());
            return;
        }
        let Some(path) = self.conflict_workbench.selected_path.clone() else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        let Some(view) = self.conflict_workbench.files.get(&path) else {
            self.last_error = Some("请先选择一个冲突文件".into());
            return;
        };
        if view.kind != khaslana::ConflictFileKind::Text {
            self.last_error = Some("该冲突文件不是文本冲突，不支持 AI 合并建议".into());
            return;
        }
        if view.has_local_edits() {
            self.close_popups();
            self.active_dialog = Some(crate::DialogState::ConfirmAiConflictMerge { path });
            return;
        }
        self.start_ai_conflict_merge(path);
    }

    /// 启动 AI 合并建议生成（后台 Long 任务）：
    /// 取 diff3 文本 → 整文件（≤ 上限）单请求，超限按块边界分段逐段请求
    /// （携带滑动窗口对话历史）→ 拼接整份合并文件回填草稿。
    /// 任一段失败整体失败，不部分写入草稿。
    pub(crate) fn start_ai_conflict_merge(&mut self, path: String) {
        if self.ai_conflict_loading {
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

        self.ai_conflict_loading = true;
        self.status = "正在生成 AI 合并建议".into();
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
                let diff3_text =
                    service.conflict_diff3_text(&repo, std::path::Path::new(&path))?;
                let segments =
                    if diff3_text.chars().count() <= khaslana::MERGE_WHOLE_FILE_LIMIT {
                        // 整文件单请求。
                        vec![khaslana::MergeSegment {
                            text: diff3_text,
                            has_conflicts: true,
                        }]
                    } else {
                        khaslana::split_diff3_text(
                            &diff3_text,
                            khaslana::MERGE_SEGMENT_LIMIT,
                            khaslana::MERGE_SINGLE_BLOCK_LIMIT,
                        )?
                    };
                let request_segments = segments.iter().filter(|s| s.has_conflicts).count();
                let total_segments = segments.len();
                // 已完成段落的 (请求, 响应) 对话历史，供后续段请求提供连续性。
                let mut history: Vec<(ChatMessage, ChatMessage)> = Vec::new();
                let mut merged = String::new();
                let mut request_done = 0usize;
                for (index, segment) in segments.into_iter().enumerate() {
                    if !segment.has_conflicts {
                        // 纯上下文段不送模型，原样透传拼接。
                        merged.push_str(&segment.text);
                        continue;
                    }
                    request_done += 1;
                    let (system, user) = khaslana::conflict_merge_prompts(
                        &path,
                        &segment.text,
                        (request_segments > 1).then_some((request_done, request_segments)),
                    );
                    let messages = if history.is_empty() {
                        vec![system, user.clone()]
                    } else {
                        khaslana::build_segment_messages(
                            system,
                            &history,
                            user.clone(),
                            khaslana::MERGE_CONTEXT_BUDGET_CHARS,
                        )
                    };
                    // 默认 max_tokens（800）放不下整段输出：按段长放宽。
                    let mut request_settings = settings.clone();
                    request_settings.max_tokens =
                        (segment.text.chars().count() / 3 + 1024).clamp(1024, 16_384) as u32;
                    let client = ChatClient::new(request_settings, proxy_url.clone());
                    let result = client.request_stream(&messages, &mut |_delta| {})?;
                    let content = khaslana::validate_generated_content(
                        &result,
                        "AI 返回的合并结果为空，请重试或检查模型配置",
                        "AI 未返回合并结果正文（仅返回了思考过程），请重试或更换模型",
                    )?;
                    let content = khaslana::strip_code_fence(&content);
                    if khaslana::response_contains_conflict_markers(&content) {
                        return Err(khaslana::GitError::Message(format!(
                            "AI 返回的第 {request_done}/{request_segments} 段仍包含冲突标记，已放弃本次结果"
                        )));
                    }
                    // 段按行切分，非末段的输出必须以换行收尾，否则拼接处
                    // 会把两行挤成一行。
                    let mut content = content;
                    if index + 1 < total_segments && !content.ends_with('\n') {
                        content.push('\n');
                    }
                    history.push((
                        user,
                        ChatMessage {
                            role: ChatRole::Assistant,
                            content: content.clone(),
                        },
                    ));
                    merged.push_str(&content);
                    // 整文件模式只有一段，进度事件无信息量，不发。
                    if request_segments > 1 {
                        crate::send_ui_event(
                            &tx,
                            crate::UiEvent::AiConflictMergeProgress {
                                path: path.clone(),
                                segment: request_done,
                                total: request_segments,
                            },
                        );
                    }
                }
                Ok(merged)
            })();
            match result {
                Ok(draft) => {
                    crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiConflictMergeGenerated { path, draft },
                    );
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
        if self.busy || self.global_busy_tab.is_some() {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let settings = self.ai_form_settings();
        if let Err(err) = settings.validate() {
            self.last_error = Some(err.to_string());
            return;
        }
        self.begin_global_test_busy("正在测试 AI 连接");

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
            match result.and_then(|result| {
                // 空正文按失败处理：避免空评审面板 +「AI 评审已生成」假成功。
                khaslana::ai::validate_generated_content(
                    &result,
                    "AI 返回的评审内容为空，请重试或检查模型配置",
                    "AI 未返回评审正文（仅返回了思考过程），请重试或更换模型",
                )
                .map(|content| (content, result.reasoning))
            }) {
                Ok((content, reasoning)) => {
                    let review = AiReviewResult { content, reasoning };
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
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child("AI 评审"),
                    )
                    .when(loading, |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
                    ui_theme::FOREGROUND
                } else {
                    ui_theme::MUTED_FOREGROUND
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
                                    .border_color(rgb(ui_theme::BORDER))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
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
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
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
        .text_color(rgb(ui_theme::FOREGROUND))
        .child(div().child(review.content.clone()))
        .when_some(review.reasoning.clone(), |this, reasoning| {
            this.child(
                div()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("思考链："))
                    .child(div().child(reasoning)),
            )
        })
        .into_any_element()
}
