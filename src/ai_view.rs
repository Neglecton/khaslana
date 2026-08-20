// AI 功能 UI 层：供应商设置弹窗、commit message 生成、code review 渲染。
//
// 复杂 AI 逻辑（HTTP 请求、prompt 构造）在 src/ai/ 中实现；
// 这里只负责组合布局、状态、交互和渲染。

use std::sync::Arc;

use gpui::{Context, IntoElement, Window, div, point, prelude::*, px};
use khaslana::{AiApiType, ChatClient, ChatMessage, ChatRole, DiffEncodingChoice, DiffScope};

use crate::ui::theme::rgb;
use crate::{
    FieldId, RepositoryView,
    ui::{
        components::{dialog_actions, dialog_overlay, empty_state},
        icons::ToolbarIcon,
        theme as ui_theme,
    },
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
                    diff_text.push_str(&khaslana::ai::file_diff_to_patch_text(&file_diff));
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
            && self.browse.target.is_some()
            && !self.browse.compare_files.is_empty()
    }

    /// 触发 Agentic AI 评审：覆盖分支比较的全部差异文件（diff-first，
    /// 模型按需调用工具深入代码），过程轨迹实时回传时间线；完成后由
    /// 任务线程落盘到评审记录，即使中途切换目标也继续后台执行。
    pub(crate) fn generate_ai_review(&mut self, cx: &mut Context<Self>) {
        if self.ai_review_loading {
            return;
        }
        if !self.ai_settings.is_usable() {
            self.last_error = Some("请先在 AI 设置中配置并启用供应商".into());
            return;
        }
        // 并发上限：在途任务（含后台分离的）达到上限时阻止新开。
        if self.ai_review_running_tasks >= crate::MAX_CONCURRENT_AI_REVIEWS {
            let message = format!(
                "已有 {} 个 AI 评审任务在进行中，请等待完成或取消后再试",
                crate::MAX_CONCURRENT_AI_REVIEWS
            );
            self.last_error = Some(message.clone());
            self.notify_error(message, cx);
            return;
        }
        let Some(target) = self.browse.target.clone() else {
            self.last_error = Some("请先进入分支比较模式".into());
            return;
        };
        if self.browse.compare_files.is_empty() {
            self.last_error = Some("当前分支比较没有差异文件".into());
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
        let service = self.service_for_tab(tab_id);

        // 代际登记：事件携带代际，切目标/取消后旧任务事件不再进面板。
        let generation = self.ai_review_next_generation;
        self.ai_review_next_generation += 1;
        self.ai_review_active_generation = Some(generation);
        self.ai_review_running_tasks += 1;
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.ai_review_cancel = Some(cancel_flag.clone());

        self.ai_review_loading = true;
        self.ai_review = None;
        self.ai_review_steps.clear();
        self.ai_review_step_expanded.clear();
        self.ai_review_live_reasoning.clear();
        self.ai_review_live_content.clear();
        self.ai_review_loaded_label = None;
        self.ai_review_progress = Some("正在准备评审上下文…".into());
        // 生成期间自动占满右侧区域，实时展示思维链与工具轨迹。
        self.ai_review_expanded = true;
        self.scroll_handle("ai-review-scroll")
            .set_offset(point(px(0.0), px(0.0)));
        self.status = "正在生成 AI 评审".into();
        self.last_error = None;

        let settings = self.ai_settings.clone();
        let model = settings.model.clone();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target(&settings.normalized_base_url());
        let repo_path_string = repo_path.display().to_string();
        let file_count = self.browse.compare_files.len();
        let data_dir = khaslana::storage::active_data_dir();
        let input = khaslana::ai::ReviewAgentInput {
            repo_path,
            target_display_name: target.display_name,
            target_commit_oid: target.commit_oid,
            compare_files: self.browse.compare_files.clone(),
        };
        let tx = self.tx.clone();
        let started_at = std::time::Instant::now();
        // 评审 agent 是分钟级任务（多轮流式 + 重试）且允许 3 个并发，
        // 走独立 ai 池避免占满 long 池饿死 fetch/push 等网络操作。
        self.tasks.spawn(crate::TaskKind::Ai, move || {
            // panic 兜底（第二层）：TaskExecutor 的 catch_unwind 只发
            // BackgroundTaskPanicked，无法区分任务种类、不会归位评审并发
            // 计数与加载标志——任务体一旦 panic，并发名额会永久泄漏
            // （3 次后评审入口锁死）。这里对任务体再包一层，panic 时补发
            // 该代际的 AiReviewFailed 走常规失败通道精确复位。
            let panic_tx = tx.clone();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let client = ChatClient::new(settings, proxy_url);
                let tx = tx.clone();
                // 完成事件由返回值统一发送（Generated/Failed/Cancelled）；
                // lib 侧的 Done 事件忽略，避免双发。
                let mut on_event = |event: khaslana::ai::AgentEvent| match event {
                    khaslana::ai::AgentEvent::Step(step) => crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiReviewStepAdded {
                            generation: generation,
                            step,
                        },
                    ),
                    khaslana::ai::AgentEvent::Progress(message) => crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiReviewProgress {
                            generation: generation,
                            message,
                        },
                    ),
                    khaslana::ai::AgentEvent::Delta { content, reasoning } => crate::send_ui_event(
                        &tx,
                        crate::UiEvent::AiReviewDelta {
                            generation: generation,
                            content_delta: content,
                            reasoning_delta: reasoning,
                        },
                    ),
                    khaslana::ai::AgentEvent::Done(_) => {}
                };
                match khaslana::ai::run_review_agent(
                    &input,
                    &service,
                    &client,
                    &cancel_flag,
                    &mut on_event,
                ) {
                    Ok(Some(review)) => {
                        // 落盘在任务线程完成：即使 UI 已分离（切目标/退出浏览）
                        // 也能保存，历史弹窗随时可查。
                        let saved = match &data_dir {
                            Some(dir) => {
                                let created_at_millis = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|elapsed| elapsed.as_millis() as u64)
                                    .unwrap_or(0);
                                khaslana::ai::save_review_record(
                                    dir,
                                    khaslana::AiReviewRecord {
                                        id: String::new(),
                                        repo_path: repo_path_string.clone(),
                                        target_display_name: input.target_display_name.clone(),
                                        target_commit_oid: input.target_commit_oid.clone(),
                                        model: model.clone(),
                                        created_at_millis,
                                        duration_secs: started_at.elapsed().as_secs(),
                                        file_count,
                                        result: review.clone(),
                                    },
                                )
                                .is_ok()
                            }
                            None => {
                                tracing::warn!(
                                    target: "khaslana::ai",
                                    "无法定位数据目录，评审记录未保存"
                                );
                                false
                            }
                        };
                        crate::send_ui_event(
                            &tx,
                            crate::UiEvent::AiReviewGenerated {
                                generation: generation,
                                review,
                                saved,
                            },
                        );
                    }
                    Ok(None) => {
                        // 用户取消：任务在轮次边界退出，UI 已复位，仅归位计数。
                        crate::send_ui_event(&tx, crate::UiEvent::AiReviewCancelled);
                    }
                    Err(err) => {
                        crate::send_ui_event(
                            &tx,
                            crate::UiEvent::AiReviewFailed {
                                generation,
                                error: err.to_string(),
                            },
                        );
                    }
                }
            }));
            if let Err(payload) = outcome {
                let message = crate::tasks::panic_message(payload);
                tracing::error!(target: "khaslana::ai", "AI 评审任务 panic：{message}");
                crate::send_ui_event(
                    &panic_tx,
                    crate::UiEvent::AiReviewFailed {
                        generation,
                        error: format!("AI 评审任务异常终止：{message}"),
                    },
                );
            }
        });
    }

    /// 渲染 AI 评审面板：展开为全区域模式（替换 diff 视图占满右侧），
    /// 收起为底部单行条。生成中也可收起（进度显示在底部条 + 可取消）。
    pub(crate) fn render_ai_review_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.ai_review_expanded {
            self.render_ai_review_full_area(cx).into_any_element()
        } else {
            self.render_ai_review_collapsed_bar(cx).into_any_element()
        }
    }

    /// 全区域模式：标题栏（进度 + 收起/重新生成）+ 滚动正文
    ///（步骤时间线 + Markdown 评审结果）。
    fn render_ai_review_full_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.ai_review_loading;
        let review = self.ai_review.clone();
        let has_steps = !self.ai_review_steps.is_empty();
        let has_live =
            !self.ai_review_live_reasoning.is_empty() || !self.ai_review_live_content.is_empty();
        let has_content = has_steps || review.is_some() || has_live;
        let handle = self.scroll_handle("ai-review-scroll");

        let status_text = if let Some(label) = &self.ai_review_loaded_label {
            // 历史记录展示态：标签优先于进度/完成文案。
            label.clone()
        } else if loading {
            self.ai_review_progress
                .clone()
                .unwrap_or_else(|| "生成中…".to_string())
        } else if let Some(review) = &review {
            format!("完成 · 共 {} 个步骤", review.steps.len())
        } else {
            String::new()
        };

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
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child("AI 评审"),
                    )
                    .when(!status_text.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .truncate()
                                .child(status_text),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .when(review.is_some(), |this| {
                        this.child(self.button(
                            "复制结论",
                            true,
                            |this, _, cx| {
                                let Some(review) = this.ai_review.clone() else {
                                    return;
                                };
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    review.content.clone(),
                                ));
                                this.status = "已复制评审结论".into();
                                this.last_error = None;
                                this.notify_success(this.status.clone(), cx);
                            },
                            cx,
                        ))
                    })
                    .child(self.button(
                        "历史",
                        true,
                        |this, _, _| {
                            this.close_popups();
                            this.open_ai_review_history();
                        },
                        cx,
                    ))
                    .when(loading, |this| {
                        this.child(self.button(
                            "取消",
                            true,
                            |this, _, _| {
                                this.cancel_ai_review();
                            },
                            cx,
                        ))
                    })
                    .child(self.button(
                        "收起",
                        true,
                        |this, _, _| {
                            this.ai_review_expanded = false;
                        },
                        cx,
                    ))
                    .when(!loading, |this| {
                        this.child(self.button(
                            if has_content {
                                "重新生成"
                            } else {
                                "AI Review"
                            },
                            self.ai_review_button_enabled(),
                            |this, _, cx| this.generate_ai_review(cx),
                            cx,
                        ))
                    }),
            );

        let live_reasoning = self.ai_review_live_reasoning.clone();
        let live_content = self.ai_review_live_content.clone();
        let has_live = !live_reasoning.is_empty() || !live_content.is_empty();

        // 滚动结构与仓库切换下拉同构：外层有界（flex_1 + min_h 0）+
        // scrollable_frame_when 直接子元素 + 内容 div 只挂 id/overflow/
        // track_scroll。不要给内容 div 再叠 flex_1/min_h 中间层——约束
        // 不会沿多层 flex 收缩链传递确定高度，多一层就滚不动。
        let content = div()
            .id("ai-review-scroll")
            .flex()
            .flex_col()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&handle)
            .px_3()
            .py_2()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(rgb(ui_theme::FOREGROUND))
            .child(self.render_review_timeline(cx))
            .when_some(review, |this, review| {
                this.child(
                    div()
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .child(crate::markdown_view::render_markdown(&review.content)),
                )
            })
            // 末轮流式正文：边生成边按 Markdown 渲染（半截文档由解析器
            // 在 EOF 收尾），完成态由 review.content 定格（同一数据源）。
            .when(loading && !live_content.is_empty(), |this| {
                this.child(
                    div()
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .child(crate::markdown_view::render_markdown(&live_content)),
                )
            })
            // live 区为空时的兜底提示（工具执行间隙/初始准备）。
            .when(loading && !has_live, |this| {
                this.child(
                    div()
                        .mt_2()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(if has_steps {
                            "工具执行中…"
                        } else {
                            "正在准备评审上下文…"
                        }),
                )
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .bg(rgb(ui_theme::CARD))
            .child(header)
            .child(scrollable_frame_when(
                "ai-review-scroll",
                ScrollbarMode::Vertical,
                content.into_any_element(),
                handle,
                has_content,
                cx,
            ))
    }

    /// 收起态：底部单行条（进度或 80 字符正文预览 + 展开/重新生成）。
    fn render_ai_review_collapsed_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let review = self.ai_review.clone();
        let has_content = review.is_some() || !self.ai_review_steps.is_empty();
        let preview = if let Some(label) = &self.ai_review_loaded_label {
            label.clone()
        } else if self.ai_review_loading {
            self.ai_review_progress
                .clone()
                .unwrap_or_else(|| "正在生成 AI 评审…".to_string())
        } else if let Some(review) = &review {
            review
                .content
                .replace('\n', " ")
                .chars()
                .take(80)
                .collect::<String>()
        } else if !self.ai_review_steps.is_empty() {
            "评审未完成，可展开检查执行轨迹".to_string()
        } else {
            "未生成".to_string()
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(32.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::PRIMARY))
                    .flex_none()
                    .child("AI 评审"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(preview),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .when(self.ai_review_loading, |this| {
                        // 生成中收起到底部条：可展开回去看进度，也可取消。
                        this.child(self.button(
                            "展开",
                            true,
                            |this, _, _| {
                                this.ai_review_expanded = true;
                            },
                            cx,
                        ))
                        .child(self.button(
                            "取消",
                            true,
                            |this, _, _| {
                                this.cancel_ai_review();
                            },
                            cx,
                        ))
                    })
                    .when(!self.ai_review_loading, |this| {
                        this.child(self.button(
                            "历史",
                            true,
                            |this, _, _| {
                                this.close_popups();
                                this.open_ai_review_history();
                            },
                            cx,
                        ))
                        .when(has_content, |this| {
                            this.child(self.button(
                                "展开",
                                true,
                                |this, _, _| {
                                    this.ai_review_expanded = true;
                                },
                                cx,
                            ))
                        })
                        .child(self.button(
                            if review.is_some() {
                                "重新生成"
                            } else {
                                "AI Review"
                            },
                            self.ai_review_button_enabled(),
                            |this, _, cx| this.generate_ai_review(cx),
                            cx,
                        ))
                    }),
            )
    }

    /// 评审历史弹窗：最近记录列表，点击「打开」载入面板；背景点击关闭。
    pub(crate) fn render_ai_review_history(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(state) = &self.ai_review_history else {
            return div().into_any_element();
        };
        let handle = self.scroll_handle("ai-review-history-scroll");

        let body = if state.loading {
            div()
                .py_6()
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child("正在加载评审记录…")
                .into_any_element()
        } else if let Some(error) = &state.error {
            div()
                .py_6()
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::DESTRUCTIVE))
                .child(format!("加载失败：{error}"))
                .into_any_element()
        } else if state.records.is_empty() {
            empty_state(Some(ToolbarIcon::Ai), "暂无评审记录", None::<&'static str>)
                .into_any_element()
        } else {
            // 滚动结构照仓库切换下拉同构：外层有界 + scrollable_frame_when
            // 直接子元素 + 内容 div 只挂 id/overflow/track_scroll。
            let rows = state
                .records
                .clone()
                .into_iter()
                .map(|record| self.render_ai_review_history_row(record, cx))
                .collect::<Vec<_>>();
            let content = div()
                .id("ai-review-history-scroll")
                .flex()
                .flex_col()
                .w_full()
                .overflow_y_scroll()
                .track_scroll(&handle)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(div().w(px(92.0)).flex_none().child("完成时间"))
                        .child(div().flex_1().min_w(px(0.0)).child("目标分支"))
                        .child(div().w(px(110.0)).flex_none().child("模型"))
                        .child(div().w(px(64.0)).flex_none().text_right().child("步骤")),
                )
                .children(rows);
            scrollable_frame_when(
                "ai-review-history-scroll",
                ScrollbarMode::Vertical,
                content.into_any_element(),
                handle,
                true,
                cx,
            )
            .into_any_element()
        };

        dialog_overlay()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.close_ai_review_history();
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("ai-review-history-panel")
                    .w(px(600.0))
                    .max_h(px(480.0))
                    .flex()
                    .flex_col()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(gpui::MouseButton::Right, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("评审历史"),
                            )
                            .child(
                                div()
                                    .id("ai-review-history-close")
                                    .flex_none()
                                    .size(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .text_size(px(12.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .cursor_pointer()
                                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.close_ai_review_history();
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    // 列表主体：有界（flex_1 + min_h 0）承接滚动。
                    .child(div().flex_1().min_h(px(0.0)).p_2().child(body)),
            )
            .into_any_element()
    }

    /// 历史列表单行。
    fn render_ai_review_history_row(
        &self,
        record: khaslana::AiReviewRecord,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let date =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(record.created_at_millis as i64)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "时间未知".to_string());
        let target_display_name = record.target_display_name.clone();
        let model = record.model.clone();
        let step_count = record.result.steps.len();
        div()
            .id(format!("ai-review-history-{}", record.id))
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .py(px(5.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_ai_review_record(record.clone());
                cx.notify();
            }))
            .child(
                div()
                    .w(px(92.0))
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(date),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .truncate()
                    .child(target_display_name),
            )
            .child(
                div()
                    .w(px(110.0))
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .truncate()
                    .child(model),
            )
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .text_right()
                    .child(format!("{step_count} 步")),
            )
            .into_any_element()
    }

    /// 步骤时间线（Codex/ZCode 式）：左侧竖轨 + 节点圆点，行默认折叠只显
    /// 一行摘要，点击展开 TILE 底等宽详情块；思维链与工具调用错开颜色，
    /// 错误步骤用警告色。
    fn render_review_timeline(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let has_steps = !self.ai_review_steps.is_empty();
        let live_reasoning = self.ai_review_live_reasoning.clone();
        let show_live = self.ai_review_loading && !live_reasoning.is_empty();
        if !has_steps && !show_live {
            return div().into_any_element();
        }
        let rows = self
            .ai_review_steps
            .iter()
            .enumerate()
            .map(|(index, step)| self.render_review_step_row(index, step, cx))
            .collect::<Vec<_>>();
        div()
            .relative()
            .flex()
            .flex_col()
            // 竖轨：绝对定位细线，先声明先绘制，节点圆点覆盖其上。
            .child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(1.0))
                    .bg(rgb(ui_theme::BORDER)),
            )
            .children(rows)
            // live 思考行：流式期间的瞬时「思考中…」，思维链全文灰色
            // 小字实时变长；轮次落定（Step(Reasoning) 到达）后由正式
            // 时间线行（「思考：{首行摘要}」）取而代之。
            .when(show_live, |this| {
                this.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .w(px(7.0))
                                .h(px(7.0))
                                .rounded_full()
                                .mt(px(6.0))
                                .bg(rgb(ui_theme::PRIMARY)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_size(px(10.0))
                                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                                .child("✻"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                                .child("思考中…"),
                                        ),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(px(11.0))
                                        .line_height(px(16.0))
                                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                        .child(live_reasoning),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    /// 时间线单行：节点圆点 + 摘要（可点击展开详情）；Message 步骤整段
    /// 直出不折叠（模型明确说的话，不该被下一个工具消息覆盖）。
    fn render_review_step_row(
        &self,
        index: usize,
        step: &khaslana::AiReviewStep,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 中间轮 assistant 正文：非折叠整段直出，无展开交互。
        if let khaslana::AiReviewStep::Message { text } = step {
            return div()
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded_full()
                        .mt(px(6.0))
                        .bg(rgb(ui_theme::PRIMARY)),
                )
                .child(
                    div().flex_1().min_w(px(0.0)).flex().flex_col().child(
                        div()
                            .flex()
                            .items_start()
                            .gap_1()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("❝"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(11.0))
                                    .line_height(px(17.0))
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(text.clone()),
                            ),
                    ),
                )
                .into_any_element();
        }

        let expanded = self.ai_review_step_expanded.contains(&index);
        let is_error = step.is_error();
        let is_reasoning = matches!(step, khaslana::AiReviewStep::Reasoning { .. });
        let summary = step.summary();
        let detail = step.detail().to_string();
        let summary_color = if is_error {
            ui_theme::DESTRUCTIVE
        } else if is_reasoning {
            ui_theme::MUTED_FOREGROUND
        } else {
            ui_theme::FOREGROUND
        };
        let marker = if is_reasoning {
            "✻"
        } else if expanded {
            "▾"
        } else {
            "▸"
        };

        div()
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded_full()
                    .mt(px(6.0))
                    .bg(rgb(if is_error {
                        ui_theme::DESTRUCTIVE
                    } else if is_reasoning {
                        ui_theme::MUTED_FOREGROUND
                    } else {
                        ui_theme::PRIMARY
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    // 摘要行可点击展开/收起详情。
                    .child(
                        div()
                            .id(format!("ai-review-step-{index}"))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                if this.ai_review_step_expanded.contains(&index) {
                                    this.ai_review_step_expanded.remove(&index);
                                } else {
                                    this.ai_review_step_expanded.insert(index);
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child(marker),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(11.0))
                                    .text_color(rgb(summary_color))
                                    .truncate()
                                    .child(summary),
                            ),
                    )
                    .when(expanded, |this| {
                        this.child(
                            div()
                                .mt_1()
                                .mb_2()
                                .p_2()
                                .rounded(px(ui_theme::RADIUS_XS))
                                .bg(rgb(ui_theme::TILE))
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .font_family("Consolas, monospace")
                                .text_color(rgb(if is_error {
                                    ui_theme::DESTRUCTIVE
                                } else {
                                    ui_theme::MUTED_FOREGROUND
                                }))
                                .child(detail),
                        )
                    }),
            )
            .into_any_element()
    }
}
