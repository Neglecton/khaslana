//! RepositoryView 的通用输入、菜单、差异与状态渲染。

use crate::*;

impl RepositoryView {
    pub(crate) fn credential_scope_button(
        &self,
        label: &'static str,
        scope: CredentialScope,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.credential_scope == scope;
        segmented_button(format!("credential-scope-{label}"), selected, enabled)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.credential_scope = scope;
                    cx.notify();
                }
            }))
            .child(label)
    }

    pub(crate) fn credential_kind_button(
        &self,
        label: &'static str,
        mode: CredentialFormMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.credential_form_mode == mode;
        segmented_button(format!("credential-kind-{label}"), selected, true)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.set_credential_form_mode(mode);
                cx.notify();
            }))
            .child(label)
    }

    pub(crate) fn toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .child(toggle_box(checked))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(label),
            )
    }

    pub(crate) fn input(
        &self,
        id: FieldId,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if id == FieldId::ConflictEditor {
            return self.conflict_editor_input(window, cx).into_any_element();
        }
        if Self::is_multiline_field(id) {
            return self.multi_line_input(id, window, cx).into_any_element();
        }
        self.single_line_input(id, compact, window, cx)
            .into_any_element()
    }

    pub(crate) fn is_multiline_field(id: FieldId) -> bool {
        matches!(
            id,
            FieldId::CommitMessage
                | FieldId::ConflictEditor
                | FieldId::TagMessage
                // 工作流模板 AI 功能需求描述（编辑器弹窗内多行输入）。
                | FieldId::WorkflowEditor(workflow_editor::WorkflowEditorFieldId::AiDescription)
                // 代码理解问题框：多行输入，Enter 发送 / Shift+Enter 换行。
                | FieldId::CodeUnderstandingQuestion
        )
    }

    fn single_line_input(
        &self,
        id: FieldId,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let field = self.field(id);
        let focused = field.focus.is_focused(window);
        input_frame(
            format!("field-{id:?}"),
            focused,
            if compact {
                InputFrameSize::Compact
            } else {
                InputFrameSize::Regular
            },
        )
        .track_focus(&field.focus)
        .key_context("TextInput")
        .on_action(cx.listener(Self::text_backspace))
        .on_action(cx.listener(Self::text_delete))
        .on_action(cx.listener(Self::text_left))
        .on_action(cx.listener(Self::text_right))
        .on_action(cx.listener(Self::text_up))
        .on_action(cx.listener(Self::text_down))
        .on_action(cx.listener(Self::text_select_left))
        .on_action(cx.listener(Self::text_select_right))
        .on_action(cx.listener(Self::text_select_up))
        .on_action(cx.listener(Self::text_select_down))
        .on_action(cx.listener(Self::text_select_all))
        .on_action(cx.listener(Self::text_home))
        .on_action(cx.listener(Self::text_end))
        .on_action(cx.listener(Self::text_paste))
        .on_action(cx.listener(Self::text_copy))
        .on_action(cx.listener(Self::text_cut))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
            if this.active_operation_blocker_message().is_some()
                && !this.operation_blocker_allows_text_field(id)
            {
                cx.stop_propagation();
                return;
            }
            // 单行框 Enter 提交所在表单（用户确认保留的文本框内行为）。
            if event.keystroke.key.as_str() == "enter" {
                this.submit_focused_field(id);
                cx.stop_propagation();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.field(id).focus);
                let position = this.field(id).index_for_mouse_position(event.position);
                let field = this.field_mut(id);
                field.is_selecting = true;
                if event.modifiers.shift {
                    field.select_to(position);
                } else {
                    field.move_to(position);
                }
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.field_mut(id).is_selecting = false;
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.field_mut(id).is_selecting = false;
                cx.notify();
            }),
        )
        .on_mouse_move(
            cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if !this.field(id).is_selecting {
                    return;
                }
                let position = this.field(id).index_for_mouse_position(event.position);
                this.field_mut(id).select_to(position);
                cx.notify();
            }),
        )
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .child(SingleLineInputElement {
            field_id: id,
            entity: cx.entity(),
        })
    }

    pub(crate) fn multi_line_input(
        &self,
        id: FieldId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let field = self.field(id);
        let focused = field.focus.is_focused(window);
        let visible_lines = multiline_input_visible_lines(id);
        let frame_size = if id == FieldId::CodeUnderstandingQuestion {
            InputFrameSize::Regular
        } else {
            InputFrameSize::Multiline
        };
        // 溢出判定综合逻辑行数与上帧自动换行行数（长行换行后同样超高）。
        let multiline_overflows = multiline_input_should_scroll(id, &field.value)
            || field.last_wrapped_line_count > visible_lines;
        input_frame(format!("field-{id:?}"), focused, frame_size)
            .track_focus(&field.focus)
            .key_context("TextInput")
            .on_action(cx.listener(Self::text_backspace))
            .on_action(cx.listener(Self::text_delete))
            .on_action(cx.listener(Self::text_left))
            .on_action(cx.listener(Self::text_right))
            .on_action(cx.listener(Self::text_select_left))
            .on_action(cx.listener(Self::text_select_right))
            .on_action(cx.listener(Self::text_select_all))
            .on_action(cx.listener(Self::text_home))
            .on_action(cx.listener(Self::text_end))
            .on_action(cx.listener(Self::text_paste))
            .on_action(cx.listener(Self::text_copy))
            .on_action(cx.listener(Self::text_cut))
            .on_action(cx.listener(Self::text_submit))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                if this.active_operation_blocker_message().is_some()
                    && !this.operation_blocker_allows_text_field(id)
                {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key.as_str() == "enter"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                {
                    // 代码理解问题框：Enter 发送、Shift+Enter 换行；
                    // IME 组合期间 Enter 只确认候选（不拦截，交给输入法）。
                    if id == FieldId::CodeUnderstandingQuestion {
                        if this.field(id).marked_range.is_some() {
                            return;
                        }
                        if event.keystroke.modifiers.shift {
                            this.field_mut(id).insert_text("\n", true);
                            cx.stop_propagation();
                            cx.notify();
                        } else {
                            this.submit_understanding_question(cx);
                            cx.stop_propagation();
                        }
                        return;
                    }
                    this.field_mut(id).insert_text("\n", true);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.field(id).focus);
                    let position = this.field(id).index_for_mouse_position(event.position);
                    let field = this.field_mut(id);
                    field.is_selecting = true;
                    if event.modifiers.shift {
                        field.select_to(position);
                    } else {
                        field.move_to(position);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(id).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(id).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !this.field(id).is_selecting {
                        return;
                    }
                    let position = this.field(id).index_for_mouse_position(event.position);
                    this.field_mut(id).select_to(position);
                    cx.notify();
                }),
            )
            .px_2()
            .py_2()
            .overflow_hidden()
            .child({
                let handle = self.scroll_handle(multiline_scroll_handle_id(id));
                let scroll_id = if id == FieldId::ConflictEditor {
                    "conflict-editor-scroll"
                } else {
                    multiline_scroll_handle_id(id)
                };
                let content = div()
                    .id(scroll_id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(MultiLineInputElement {
                        field_id: id,
                        entity: cx.entity(),
                    })
                    .into_any_element();
                let frame = scrollable_frame_when(
                    scroll_id,
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    multiline_overflows,
                    cx,
                );
                if id == FieldId::ConflictEditor {
                    // 冲突编辑器随冲突面板高度伸缩。
                    frame.into_any_element()
                } else {
                    // 普通多行输入固定可视高度；问题框按设计稿压缩为两行，
                    // 其余输入仍使用 MULTILINE_MIN_LINES。
                    // 内容超出后滚动，不再随内容无限撑高。
                    div()
                        .flex()
                        .flex_col()
                        .h(px(MULTILINE_LINE_HEIGHT * visible_lines as f32))
                        .child(frame)
                        .into_any_element()
                }
            })
    }

    fn conflict_editor_input(&self, _window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = self.field(FieldId::ConflictEditor);
        let multiline_overflows =
            multiline_input_should_scroll(FieldId::ConflictEditor, &field.value);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .track_focus(&field.focus)
            .key_context("TextInput")
            .on_action(cx.listener(Self::text_backspace))
            .on_action(cx.listener(Self::text_delete))
            .on_action(cx.listener(Self::text_left))
            .on_action(cx.listener(Self::text_right))
            .on_action(cx.listener(Self::text_select_left))
            .on_action(cx.listener(Self::text_select_right))
            .on_action(cx.listener(Self::text_select_all))
            .on_action(cx.listener(Self::text_home))
            .on_action(cx.listener(Self::text_end))
            .on_action(cx.listener(Self::text_paste))
            .on_action(cx.listener(Self::text_copy))
            .on_action(cx.listener(Self::text_cut))
            .on_action(cx.listener(Self::text_submit))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                if this.active_operation_blocker_message().is_some()
                    && !this.operation_blocker_allows_text_field(FieldId::ConflictEditor)
                {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key.as_str() == "enter"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                {
                    this.field_mut(FieldId::ConflictEditor)
                        .insert_text("\n", true);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.field(FieldId::ConflictEditor).focus);
                    let position = this
                        .field(FieldId::ConflictEditor)
                        .index_for_mouse_position(event.position);
                    let field = this.field_mut(FieldId::ConflictEditor);
                    field.is_selecting = true;
                    if event.modifiers.shift {
                        field.select_to(position);
                    } else {
                        field.move_to(position);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(FieldId::ConflictEditor).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.field_mut(FieldId::ConflictEditor).is_selecting = false;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !this.field(FieldId::ConflictEditor).is_selecting {
                        return;
                    }
                    let position = this
                        .field(FieldId::ConflictEditor)
                        .index_for_mouse_position(event.position);
                    this.field_mut(FieldId::ConflictEditor).select_to(position);
                    cx.notify();
                }),
            )
            .p_2()
            .child({
                let handle = self.scroll_handle(CONFLICT_RESULT_SCROLL_HANDLE_ID);
                let content = div()
                    .id("conflict-editor-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(MultiLineInputElement {
                        field_id: FieldId::ConflictEditor,
                        entity: cx.entity(),
                    })
                    .into_any_element();
                scrollable_frame_when(
                    "conflict-editor-scroll",
                    ScrollbarMode::Vertical,
                    content,
                    handle,
                    multiline_overflows,
                    cx,
                )
            })
    }

    /// 仓库切换下拉触发器按钮：显示当前仓库头像 + 名称 + ▾。
    pub(crate) fn render_repo_switcher_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let name = self.display_name();
        // 按钮内名称截断后，悬浮仍可确认仓库完整路径。
        let repo_tooltip = self
            .repo_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| name.clone());
        let enabled = !self.busy;
        let text_color = if enabled {
            ui_theme::FOREGROUND
        } else {
            ui_theme::MUTED_FOREGROUND
        };
        div()
            .id("repo-switcher-trigger")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .ml(px(12.0))
            .rounded(px(ui_theme::RADIUS_XS))
            // 纯鼠标触发器：不可聚焦、无键盘激活（键盘白名单见 AGENTS.md §8）。
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed())
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                    .active(|this| this.opacity(0.82))
            })
            .when(!enabled, |this| this.opacity(0.5))
            .text_color(rgb(text_color))
            .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                if this.busy {
                    return;
                }
                // 已展开时点击按钮应关闭；close_popups 会清掉菜单，故先记录原状态，
                // 仅在原本未展开时才重新打开，避免“点按钮关不掉”。
                let was_open = this.repo_switcher_menu.is_some();
                this.close_popups();
                if !was_open {
                    this.toggle_repo_switcher(window);
                }
                cx.notify();
            }))
            .child(repo_avatar(&name))
            .child(
                div()
                    .id("repo-switcher-name")
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .max_w(px(120.0))
                    .min_w(px(0.0))
                    .truncate()
                    .tooltip(move |_window, cx| tooltip_text(repo_tooltip.clone(), cx))
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("▾"),
            )
            // paint 时记录按钮的窗口坐标矩形，供下拉菜单锚定与“点击外部关闭”命中判定。
            // 纯记录、不注册鼠标事件，故不拦截按钮自身的点击；不 notify，避免重渲染循环。
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, _window, cx| {
                        entity.update(cx, |this, _cx| {
                            this.repo_switcher_anchor = Some(RepoSwitcherAnchor {
                                x: bounds.origin.x.into(),
                                y: bounds.origin.y.into(),
                                w: bounds.size.width.into(),
                                h: bounds.size.height.into(),
                            });
                        });
                    },
                )
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0)),
            )
    }

    /// 仓库切换下拉 overlay：IDEA 式三区结构（功能 / 打开项目 / 最近项目）。
    /// 组装并按当前搜索词过滤仓库切换下拉的分区数据（渲染与键盘导航共用）。
    fn repo_switcher_filtered_sections(&self) -> RepoSwitcherSections {
        let active_key = self
            .active_tab
            .and_then(|id| self.tab(id))
            .and_then(|tab| tab.path_key());

        let tabs: Vec<RepoSwitcherTabInput> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let path = tab.repo_path.as_ref()?;
                Some(RepoSwitcherTabInput {
                    key: normalize_repo_path(path),
                    name: tab.display_name(),
                    full_path: path.to_string_lossy().to_string(),
                    last_active: tab.last_active_at,
                    tab_id: tab.id,
                })
            })
            .collect();

        let recent: Vec<RepoSwitcherRecentInput> = self
            .repo_switcher_recent
            .iter()
            .map(|(path, ts)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                RepoSwitcherRecentInput {
                    key: normalize_repo_path(path),
                    name,
                    full_path: path.to_string_lossy().to_string(),
                    last_opened: *ts,
                }
            })
            .collect();

        let sections = build_repo_switcher_sections(active_key.as_deref(), tabs, recent);
        filter_repo_switcher_sections(sections, self.repo_switcher_search.value.as_str())
    }

    pub(crate) fn render_repo_switcher_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(menu) = self.repo_switcher_menu.as_ref() else {
            return div().into_any_element();
        };
        let menu = menu.clone();

        let query_active = !self.repo_switcher_search.value.trim().is_empty();
        let sections = self.repo_switcher_filtered_sections();

        // 下拉内容区的滚动句柄，内容超出最大高度时滚动并绘制滚动条。
        let switcher_handle = self.scroll_handle("repo-switcher-scroll");
        let switcher_content = div()
            .id("repo-switcher-scroll")
            .flex()
            .flex_col()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&switcher_handle)
            // ── 功能区：克隆 / 打开 / 搜索仓库 ──
            .child(self.repo_switcher_action_item(
                "repo-switcher-clone",
                ToolbarIcon::Clone,
                "克隆仓库…",
                |this, window, _cx| {
                    this.close_repo_switcher();
                    this.open_clone_dialog(window);
                },
                cx,
            ))
            .child(self.repo_switcher_action_item(
                "repo-switcher-open",
                ToolbarIcon::Open,
                "打开仓库…",
                |this, _window, _cx| {
                    this.close_repo_switcher();
                    this.browse_open();
                },
                cx,
            ))
            // 搜索仓库：默认为按钮，点击展开输入框 + 小叉
            .when(!self.repo_switcher_search_open, |this| {
                this.child(self.repo_switcher_action_item(
                    "repo-switcher-search-toggle",
                    ToolbarIcon::Search,
                    "搜索仓库",
                    |this, window, _cx| {
                        this.repo_switcher_search_open = true;
                        window.focus(&this.repo_switcher_search.focus);
                    },
                    cx,
                ))
            })
            .when(self.repo_switcher_search_open, |this| {
                this.child(
                    div()
                        .id("repo-switcher-search-row")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(div().flex_1().min_w(px(0.0)).child(self.input(
                            FieldId::RepoSwitcherSearch,
                            false,
                            window,
                            cx,
                        )))
                        .child(
                            div()
                                .id("repo-switcher-search-close")
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
                                    // 收起输入框，恢复「搜索仓库」按钮并取消过滤
                                    this.repo_switcher_search_open = false;
                                    this.repo_switcher_search.clear();
                                    cx.notify();
                                }))
                                .child("✕"),
                        ),
                )
            })
            // ── 打开项目区 ──
            .when(!sections.open.is_empty(), |this| {
                this.child(self.repo_switcher_section_header("打开项目"))
                    .children(sections.open.iter().map(|repo| {
                        self.repo_switcher_repo_item(repo.clone(), cx)
                            .into_any_element()
                    }))
            })
            // ── 最近项目区 ──
            .when(!sections.recent.is_empty(), |this| {
                this.child(self.repo_switcher_section_header("最近的项目"))
                    .children(sections.recent.iter().map(|repo| {
                        self.repo_switcher_repo_item(repo.clone(), cx)
                            .into_any_element()
                    }))
            })
            // ── 搜索无结果占位 ──
            .when(
                query_active && sections.open.is_empty() && sections.recent.is_empty(),
                |this| {
                    this.child(
                        div()
                            .px_3()
                            .py_4()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("没有匹配的仓库"),
                    )
                },
            )
            .into_any_element();

        // 外层仅做定位与最大高度约束，滚动与滚动条交给 scrollable_frame_when。
        glass_menu()
            .id("repo-switcher-menu")
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(REPO_SWITCHER_MENU_WIDTH))
            .max_h(px(REPO_SWITCHER_MENU_HEIGHT))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(scrollable_frame_when(
                "repo-switcher-scroll",
                ScrollbarMode::Vertical,
                switcher_content,
                switcher_handle,
                true,
                cx,
            ))
            .into_any_element()
    }

    /// 下拉功能区的一项（克隆/打开）。
    fn repo_switcher_action_item(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        label: &'static str,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .text_size(px(12.0))
            .text_color(rgb(ui_theme::FOREGROUND))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .child(toolbar_icon(icon, ui_theme::MUTED_FOREGROUND))
            .child(label)
    }

    /// 下拉分区小标题。
    fn repo_switcher_section_header(&self, label: &'static str) -> impl IntoElement {
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .text_size(px(11.0))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .child(label)
    }

    /// 下拉仓库行：头像 + 名称 + 完整路径；已打开项 hover 显示关闭按钮；
    /// 活动仓库使用选中色优先。
    fn repo_switcher_repo_item(
        &self,
        repo: RepoSwitcherRepo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let path_for_click = repo.full_path.clone();
        let tab_id = repo.tab_id;
        let is_active = repo.active;
        let can_close = repo.tab_id.is_some();
        let close_tab_id = repo.tab_id;

        div()
            .id(format!("repo-switcher-item-{}", repo.path_key))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(is_active, |this| this.bg(rgb(ui_theme::ACCENT)))
            .when(!is_active, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.close_repo_switcher();
                if let Some(id) = tab_id {
                    this.activate_tab(id);
                } else {
                    this.open_repo(PathBuf::from(&path_for_click));
                }
                cx.notify();
            }))
            .child(repo_avatar(&repo.name))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(1.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .truncate()
                                    .child(repo.name),
                            )
                            .when(is_active, |this| {
                                this.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child("✓"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .truncate()
                            .child(repo.full_path),
                    ),
            )
            .when(can_close, |this| {
                this.child(
                    div()
                        .id(format!("repo-switcher-close-{}", repo.path_key))
                        .flex_none()
                        .size(px(20.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(ui_theme::RADIUS_XS))
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::DESTRUCTIVE)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            if let Some(id) = close_tab_id {
                                this.close_tab(id);
                            }
                            cx.notify();
                        }))
                        .child("✕"),
                )
            })
    }

    /// 设置中心 overlay：左导航 + 右内容面板。
    pub(crate) fn render_settings_center_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(category) = self.settings_center else {
            return div().into_any_element();
        };

        let categories = [
            (
                SettingsCategory::Credentials,
                ToolbarIcon::Credentials,
                "凭据管理",
            ),
            (SettingsCategory::Proxy, ToolbarIcon::Proxy, "网络代理"),
            (SettingsCategory::Ai, ToolbarIcon::Ai, "AI 设置"),
            (
                SettingsCategory::ExternalMerge,
                ToolbarIcon::Workflow,
                "合并工具",
            ),
            (SettingsCategory::CodeIndex, ToolbarIcon::Search, "代码索引"),
            (SettingsCategory::Theme, ToolbarIcon::Globe, "外观"),
            (SettingsCategory::Update, ToolbarIcon::Update, "更新设置"),
            (SettingsCategory::Shortcuts, ToolbarIcon::Keyboard, "快捷键"),
            (SettingsCategory::About, ToolbarIcon::Info, "关于"),
        ];

        // 右侧内容面板根据当前分类渲染对应 body。
        let body: gpui::AnyElement = match category {
            SettingsCategory::Credentials => {
                self.render_credential_manager_dialog(cx).into_any_element()
            }
            SettingsCategory::Proxy => self
                .render_network_proxy_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Ai => self
                .render_ai_provider_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::ExternalMerge => self
                .render_external_merge_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::CodeIndex => self
                .render_code_index_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Theme => self
                .render_theme_settings_dialog(window, cx)
                .into_any_element(),
            SettingsCategory::Update => self.render_update_settings_dialog(cx).into_any_element(),
            SettingsCategory::Shortcuts => self.render_shortcuts_settings(cx).into_any_element(),
            SettingsCategory::About => self.render_about_settings(cx).into_any_element(),
        };
        // 右侧内容区的滚动句柄，供内容超出固定高度时滚动并绘制滚动条。
        let settings_content_handle = self.scroll_handle("settings-center-content");

        // 遮罩不承载关闭：点击遮罩背景、遮罩上方的通知气泡（含其关闭按钮）
        // 都不关闭设置中心——唯一关闭入口是弹窗右上角的「✕」（Ctrl+, 快捷键
        // 保留 toggle 语义）。遮罩自身 occlude() 挡住下层 UI 的点击。
        dialog_overlay()
            .child(
                div()
                    .id("settings-center-panel")
                    .track_focus(&self.settings_center_focus)
                    .w(px(900.0))
                    // 固定高度，弹窗大小不随分类内容多少变化；内容超出由右侧内容区滚动。
                    .h(px(640.0))
                    .min_w(px(0.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    // 顶栏
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child("设置中心"),
                            )
                            .child(
                                div()
                                    .id("settings-center-close")
                                    .size(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .cursor_pointer()
                                    .text_size(px(14.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.close_settings_center();
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    // 主体：左导航 + 右内容
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            // 左导航
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_none()
                                    .w(px(160.0))
                                    .border_r_1()
                                    .border_color(rgb(ui_theme::BORDER))
                                    .py_2()
                                    .children(categories.iter().map(|(cat, icon, label)| {
                                        let is_active = *cat == category;
                                        let cat = *cat;
                                        div()
                                            .id(format!("settings-nav-{label}"))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .text_size(px(12.0))
                                            .cursor_pointer()
                                            .when(is_active, |this| {
                                                this.bg(rgb(ui_theme::ACCENT))
                                                    .text_color(rgb(ui_theme::PRIMARY))
                                            })
                                            .when(!is_active, |this| {
                                                this.text_color(rgb(ui_theme::FOREGROUND))
                                                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                                            })
                                            .on_click(cx.listener(
                                                move |this, _event, _window, cx| {
                                                    this.select_settings_category(cat);
                                                    cx.notify();
                                                },
                                            ))
                                            .child(toolbar_icon(
                                                *icon,
                                                if is_active {
                                                    ui_theme::PRIMARY
                                                } else {
                                                    ui_theme::MUTED_FOREGROUND
                                                },
                                            ))
                                            .child(*label)
                                            .into_any_element()
                                    })),
                            )
                            // 右内容：固定高度内滚动，叠加滚动条。
                            .child(scrollable_frame_when(
                                "settings-center-content",
                                ScrollbarMode::Vertical,
                                div()
                                    .id("settings-center-content")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .p_4()
                                    .overflow_y_scroll()
                                    .track_scroll(&settings_content_handle)
                                    .child(body)
                                    .into_any_element(),
                                settings_content_handle,
                                true,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_tag_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.tag_context_menu.clone() else {
            return div().into_any_element();
        };
        let has_remotes = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.remotes.is_empty());

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(TAG_MENU_WIDTH))
            .child(context_menu_item(
                "检出标签",
                !self.busy && !self.merge_in_progress(),
                {
                    let tag = menu.tag.clone();
                    move |this| this.checkout_tag(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "浏览此标签",
                !self.busy,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_browse_tag(tag.clone())
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                "推送到远端...",
                !self.busy && has_remotes,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_tag_push_dialog(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除标签",
                !self.busy,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_delete_tag_confirm(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "删除远端标签...",
                !self.busy && has_remotes,
                {
                    let tag = menu.tag.clone();
                    let remote = self.current_remote().unwrap_or_default();
                    move |this| this.open_delete_remote_tag_confirm(remote.clone(), tag.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn render_stash_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.stash_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(STASH_MENU_WIDTH))
            .child(self.render_stash_context_menu_content(menu.index, cx))
            .into_any_element()
    }

    pub(crate) fn render_workflow_template_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(menu) = self.workflow_template_context_menu.clone() else {
            return div().into_any_element();
        };
        let edit_path = menu.path.clone();
        let copy_path = menu.path.clone();
        let bind_path = menu.path.clone();
        // 删除确认弹窗展示用名称：优先解析出的显示名不可得（坏模板也允许删），退回文件名主干
        let delete_path = menu.path.clone();
        let delete_display_name = menu
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "该模板".to_string());

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(WORKFLOW_TEMPLATE_MENU_WIDTH))
            .child(context_menu_item_with_context(
                "编辑此模板",
                !self.busy,
                move |this, cx| {
                    this.workflow_template_context_menu = None;
                    let path = edit_path.clone();
                    this.open_workflow_editor_for_path(path, false, cx);
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                "复制为副本",
                !self.busy,
                move |this, cx| {
                    this.workflow_template_context_menu = None;
                    let path = copy_path.clone();
                    this.open_workflow_editor_for_path(path, true, cx);
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                "绑定快捷键...",
                !self.busy,
                move |this, _cx| {
                    this.workflow_template_context_menu = None;
                    let path = bind_path.clone();
                    this.open_workflow_shortcut_binding_dialog(path);
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item_with_context(
                "删除模板...",
                !self.busy,
                move |this, _cx| {
                    this.workflow_template_context_menu = None;
                    let path = delete_path.clone();
                    let name = delete_display_name.clone();
                    this.open_delete_workflow_template_confirm(path, name);
                },
                cx,
            ))
            .into_any_element()
    }

    /// 打开「删除工作流模板」确认弹窗。
    pub(crate) fn open_delete_workflow_template_confirm(
        &mut self,
        path: PathBuf,
        display_name: String,
    ) {
        self.active_dialog =
            Some(DialogState::ConfirmDeleteWorkflowTemplate { path, display_name });
        self.last_error = None;
    }

    /// 删除工作流模板文件（纯本地 IO，小文件同步执行）；若它是当前加载的
    /// 工作流则同时清空详情区，避免残留失效引用。
    pub(crate) fn delete_workflow_template(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match fs::remove_file(&path) {
            Ok(()) => {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                if self.workflow_state.selected_template_path.as_ref() == Some(&path) {
                    self.clear_workflow_file();
                    self.workflow_state.selected_template_path = None;
                }
                // 模板删除后同步移除其快捷键绑定，键位当场释放。
                self.remove_workflow_shortcut_binding(&file_name, cx);
                self.refresh_workflow_templates();
                self.status = format!("工作流模板已删除：{file_name}");
                self.notify_success(format!("工作流模板已删除：{file_name}"), cx);
            }
            Err(err) => {
                self.last_error = Some(format!("工作流模板删除失败：{err}"));
            }
        }
    }

    /// 按模板文件名移除工作流快捷键绑定（大小写不敏感匹配键）；存在且有
    /// 变化时保存并重注册。删除模板与设置页「清除」共用。
    pub(crate) fn remove_workflow_shortcut_binding(&mut self, file: &str, cx: &mut Context<Self>) {
        let stale_key = self
            .workflow_shortcut_bindings
            .bindings
            .keys()
            .find(|key| key.eq_ignore_ascii_case(file))
            .cloned();
        if let Some(key) = stale_key {
            self.workflow_shortcut_bindings.bindings.remove(&key);
            self.persist_workflow_shortcut_bindings(cx);
        }
    }

    /// 模板改名保存后把快捷键绑定迁移到新文件名（键位 + 后台标志保留）。
    /// 「复制为副本」不走这里（副本是新文件、无绑定可迁）；目标名已有
    /// 陈旧孤儿绑定时直接覆盖。
    pub(crate) fn migrate_workflow_shortcut_binding(
        &mut self,
        old_file: &str,
        new_file: &str,
        cx: &mut Context<Self>,
    ) {
        if new_file.is_empty() || old_file.eq_ignore_ascii_case(new_file) {
            return;
        }
        let old_key = self
            .workflow_shortcut_bindings
            .bindings
            .keys()
            .find(|key| key.eq_ignore_ascii_case(old_file))
            .cloned();
        if let Some(old_key) = old_key {
            let binding = self
                .workflow_shortcut_bindings
                .bindings
                .remove(&old_key)
                .expect("binding key was just found");
            self.workflow_shortcut_bindings
                .bindings
                .insert(new_file.to_string(), binding);
            self.persist_workflow_shortcut_bindings(cx);
        }
    }

    pub(crate) fn render_change_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.change_context_menu.clone() else {
            return div().into_any_element();
        };
        let selected_count = self.change_selection.selected(&menu.scope).len();
        let selected_paths = self.selected_change_paths(menu.scope.clone());
        let all_paths = self.change_paths(menu.scope.clone());
        let all_count = all_paths.len();
        let can_discard = !self.busy && !self.merge_in_progress();

        let mut menu_el = glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(CHANGE_MENU_WIDTH))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.credential_context_menu.is_some() {
                        this.credential_context_menu = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            });

        menu_el = match menu.scope {
            DiffScope::Staged => menu_el
                .child(context_menu_item_with_context(
                    "复制绝对路径",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item_with_context(
                    "打开文件所在目录",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.open_file_parent_directory(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "追溯此文件",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.open_blame_file(path.clone())
                    },
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "取消暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.unstage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    "取消暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.unstage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "回滚更改...",
                    can_discard,
                    {
                        let path = menu.path.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                vec![path.clone()],
                                scope.clone(),
                                DiscardTarget::Single,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚指定更改...",
                    selected_count > 0 && can_discard,
                    {
                        let paths = selected_paths.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::Selected,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚全部更改...",
                    all_count > 0 && can_discard,
                    {
                        let paths = all_paths.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::All,
                            )
                        }
                    },
                    cx,
                )),
            DiffScope::Unstaged => menu_el
                .child(context_menu_item(
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "追溯此文件",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.open_blame_file(path.clone())
                    },
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.stage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    "暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.stage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    "回滚更改...",
                    can_discard,
                    {
                        let path = menu.path.clone();
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                vec![path.clone()],
                                scope.clone(),
                                DiscardTarget::Single,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚指定更改...",
                    selected_count > 0 && can_discard,
                    {
                        let paths = selected_paths;
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::Selected,
                            )
                        }
                    },
                    cx,
                ))
                .child(context_menu_item(
                    "回滚全部更改...",
                    all_count > 0 && can_discard,
                    {
                        let paths = all_paths;
                        let scope = menu.scope.clone();
                        move |this| {
                            this.open_discard_change_confirm_dialog(
                                paths.clone(),
                                scope.clone(),
                                DiscardTarget::All,
                            )
                        }
                    },
                    cx,
                )),
        };

        menu_el.into_any_element()
    }

    pub(crate) fn render_file_path_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.file_path_context_menu.clone() else {
            return div().into_any_element();
        };

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(FILE_PATH_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(context_menu_item_with_context(
                "复制绝对路径",
                true,
                {
                    let path = menu.path.clone();
                    move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                "打开文件所在目录",
                true,
                {
                    let path = menu.path.clone();
                    move |this, cx| this.open_file_parent_directory(path.clone(), cx)
                },
                cx,
            ))
            // 「追溯此文件」对 HEAD 版本追溯（v1 不支持对任意提交 blame）
            .child(context_menu_item(
                "查看文件历史",
                true,
                {
                    let path = menu.path.clone();
                    move |this| this.view_file_history(path.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                "追溯此文件",
                true,
                {
                    let path = menu.path.clone();
                    move |this| this.open_blame_file(path.clone())
                },
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn render_commit_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.commit_context_menu.clone() else {
            return div().into_any_element();
        };
        let is_merge_commit = menu.parent_count > 1;
        let revert_label = if is_merge_commit {
            "撤销合并提交..."
        } else {
            "回滚提交"
        };
        let can_change_repository = !self.busy && !self.merge_in_progress();

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(COMMIT_MENU_WIDTH))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.credential_context_menu.is_some() {
                        this.credential_context_menu = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("提交 {}", menu.short_oid)),
            )
            .child(menu_separator())
            .when(menu.is_unpushed, |this| {
                let can_uncommit = can_change_repository && menu.is_head;
                let label = if menu.is_head {
                    "还原到暂存区..."
                } else {
                    "还原到暂存区（仅支持最新提交）"
                };
                this.child(context_menu_item(
                    label,
                    can_uncommit,
                    {
                        let oid = menu.oid.clone();
                        let summary = menu.summary.clone();
                        move |this| {
                            this.open_uncommit_to_staged_confirm_dialog(
                                oid.clone(),
                                summary.clone(),
                            )
                        }
                    },
                    cx,
                ))
                .child(menu_separator())
            })
            .child(context_menu_item(
                "软重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Soft,
                        )
                    }
                },
                cx,
            ))
            .child(context_menu_item(
                "混合重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Mixed,
                        )
                    }
                },
                cx,
            ))
            .child(context_menu_item(
                "强制重置分支到此次提交",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        this.open_reset_confirm_dialog(
                            oid.clone(),
                            summary.clone(),
                            ResetMode::Hard,
                        )
                    }
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                revert_label,
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| {
                        if is_merge_commit {
                            this.open_revert_merge_confirm_dialog(oid.clone(), summary.clone())
                        } else {
                            this.open_revert_confirm_dialog(oid.clone(), summary.clone())
                        }
                    }
                },
                cx,
            ))
            // 拣选提交：合并提交暂不支持（需要 -m mainline 语义，后续迭代）。
            .child(context_menu_item(
                if is_merge_commit {
                    "拣选提交（暂不支持合并提交）"
                } else {
                    "拣选提交到当前分支"
                },
                can_change_repository && !is_merge_commit,
                {
                    let oid = menu.oid.clone();
                    move |this| this.cherry_pick_commit(oid.clone())
                },
                cx,
            ))
            .child(menu_separator())
            .child(context_menu_item(
                "在此提交上创建标签...",
                can_change_repository,
                {
                    let oid = menu.oid.clone();
                    let summary = menu.summary.clone();
                    move |this| this.open_tag_form_dialog(Some(oid.clone()), summary.clone())
                },
                cx,
            ))
            .child(self.commit_copy_sha_menu_item(menu.oid.clone(), cx))
            .into_any_element()
    }

    pub(crate) fn render_credential_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(menu) = self.credential_context_menu.clone() else {
            return div().into_any_element();
        };
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == menu.record_id)
            .cloned()
        else {
            return div().into_any_element();
        };

        let name = Some(credential_record_label(&record));
        let target = Some(credential_display_target(&record));
        let username = Some(record.username.clone());
        let key_path = record.key_path.clone();

        glass_menu()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(CREDENTIAL_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(self.credential_copy_menu_item("复制名称", name, "凭据名称", cx))
            .child(self.credential_copy_menu_item("复制站点/远端", target, "站点/远端", cx))
            .child(self.credential_copy_menu_item("复制用户名", username, "用户名", cx))
            .child(self.credential_copy_menu_item(
                "复制 SSH Key 路径",
                key_path,
                "SSH Key 路径",
                cx,
            ))
            .into_any_element()
    }

    fn credential_copy_menu_item(
        &self,
        label: &'static str,
        text: Option<String>,
        status_label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = text
            .as_ref()
            .is_some_and(|text| !text.is_empty() && text != "-");
        div()
            .id(format!("credential-context-menu-{label}"))
            .px_3()
            .py_1()
            .text_color(if enabled {
                rgb(ui_theme::FOREGROUND)
            } else {
                rgb(ui_theme::MUTED_FOREGROUND)
            })
            .bg(rgb(ui_theme::CARD))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                if enabled {
                    this.copy_credential_text(text.clone(), status_label, cx);
                    cx.notify();
                }
            }))
            .child(label)
    }

    fn commit_copy_sha_menu_item(&self, oid: String, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("context-menu-copy-commit-sha")
            .px_3()
            .py_1()
            .text_color(rgb(ui_theme::FOREGROUND))
            .bg(rgb(ui_theme::CARD))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.copy_commit_sha(oid.clone(), cx);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child("复制 SHA 到剪贴板")
    }

    pub(crate) fn render_encoding_dropdown(
        &self,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.encoding_menu_target != Some(target) {
            return div().into_any_element();
        }
        let current = self.current_diff_encoding_choice();
        let title = match target {
            EncodingMenuTarget::Worktree => "工作区差异编码",
            EncodingMenuTarget::History => "提交差异编码",
            EncodingMenuTarget::Stash => "贮藏差异编码",
            EncodingMenuTarget::Browse => "浏览编码",
            EncodingMenuTarget::Blame => "追溯编码",
        };

        glass_menu()
            .absolute()
            .top(px(38.0))
            .right(px(12.0))
            .w(px(ENCODING_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(title),
            )
            .child(menu_separator())
            .child(self.encoding_menu_item(DiffEncodingChoice::Auto, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Utf8, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Gb18030, current, cx))
            .child(self.encoding_menu_item(DiffEncodingChoice::Big5, current, cx))
            .into_any_element()
    }

    fn encoding_menu_item(
        &self,
        choice: DiffEncodingChoice,
        current: DiffEncodingChoice,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = choice == current;
        let label = if selected {
            format!("✓ {}", choice.label())
        } else {
            format!("  {}", choice.label())
        };
        div()
            .id(format!("context-menu-encoding-{}", choice.label()))
            .px_3()
            .py_1()
            .text_color(if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::FOREGROUND)
            })
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::CARD)
            })
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.choose_diff_encoding(choice);
                cx.notify();
            }))
            .child(label)
    }

    pub(crate) fn render_column_splitter(
        &self,
        target: ResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let active = self.resize_state(target).is_some();
        let horizontal = target == ResizeTarget::HistoryDetails;
        // 弹窗或弹层菜单打开期间分割线不响应：不显示拖拽光标、不高亮、不响应鼠标，
        // 避免弹层边缘容差区内的悬停/点击被分割线抢走。
        let interactive = column_splitter_accepts_mouse_events(
            self.active_dialog.is_some(),
            self.any_popup_menu_open(),
        );

        div()
            .flex_none()
            .relative()
            .map(|this| {
                if horizontal {
                    this.h(px(8.0)).w_full()
                } else {
                    this.w(px(8.0)).h_full()
                }
            })
            .when(interactive, |this| {
                this.cursor(if horizontal {
                    CursorStyle::ResizeRow
                } else {
                    CursorStyle::ResizeColumn
                })
                .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            })
            .bg(if active {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::CARD)
            })
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseUpEvent, _window, cx| {
                    if this.resize_state(target).is_some() {
                        this.finish_resize_column(target);
                        cx.notify();
                    }
                }),
            )
            .child(if horizontal {
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .top(px(3.0))
                    .h(px(1.0))
                    .bg(if active {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .into_any_element()
            } else {
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(px(1.0))
                    .bg(if active {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER)
                    })
                    .into_any_element()
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseDownEvent, _, _, cx| {
                                if !bounds.contains(&event.position) {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if !column_splitter_accepts_mouse_events(
                                        this.active_dialog.is_some(),
                                        this.any_popup_menu_open(),
                                    ) {
                                        this.finish_resize_column(target);
                                        cx.notify();
                                        return;
                                    }
                                    if event.click_count >= 2 {
                                        this.reset_resize_target(target);
                                    } else {
                                        this.start_resize_column(target, event);
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseMoveEvent, _, _, cx| {
                                let (resizing, active_dialog, popup_open) = {
                                    let view = entity.read(cx);
                                    (
                                        view.resize_state(target).is_some(),
                                        view.active_dialog.is_some(),
                                        view.any_popup_menu_open(),
                                    )
                                };
                                if column_splitter_should_clear_resize(
                                    active_dialog || popup_open,
                                    resizing,
                                ) {
                                    entity.update(cx, |this, cx| {
                                        this.finish_resize_column(target);
                                        cx.notify();
                                    });
                                    return;
                                }
                                if !resizing
                                    || !event.dragging()
                                    || !column_splitter_accepts_mouse_events(
                                        active_dialog,
                                        popup_open,
                                    )
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    this.update_resize_column(target, event);
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                            let (resizing, active_dialog, popup_open) = {
                                let view = entity.read(cx);
                                (
                                    view.resize_state(target).is_some(),
                                    view.active_dialog.is_some(),
                                    view.any_popup_menu_open(),
                                )
                            };
                            if !resizing {
                                return;
                            }
                            if !column_splitter_accepts_mouse_events(active_dialog, popup_open)
                                && !column_splitter_should_clear_resize(
                                    active_dialog || popup_open,
                                    resizing,
                                )
                            {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                this.finish_resize_column(target);
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0)),
            )
    }

    pub(crate) fn render_virtual_diff(
        &self,
        scroll_id: &'static str,
        diff: Option<Arc<FileDiff>>,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 二进制文件不渲染逐行 diff（也不显示「Binary files ... differ」原始行），
        // 直接显示信息占位卡片，含文件大小/新增删除信息。例外：Office 文档
        // （docx/xlsx/pptx）提取出的文本差异行——is_binary 保持 true（沿用
        // 全部二进制门控）但携带 lines，按普通行渲染文本化预览。
        if let Some(diff) = diff
            .as_deref()
            .filter(|diff| diff.is_binary && diff.lines.is_empty())
        {
            return binary_diff_placeholder(diff).into_any_element();
        }
        let model = diff_render_model_for(diff.as_deref(), headers_expanded);
        let row_count = model.row_count;
        let content_present = diff.is_some() && row_count > 0;
        // 以内容最宽的文本行作为列表水平宽度的测量基准，保证长行也能左右滚动。
        // 结果按 diff 身份缓存：大 diff（上限 2 万行）每帧重算是 O(总字符) 扫描。
        let width_measure_index = cached_widest_diff_row_index(
            diff.as_ref(),
            headers_expanded,
            &model,
            &self.widest_diff_row_cache,
        )
        .or_else(|| row_count.checked_sub(1));
        let handle = self.uniform_scroll_handle(scroll_id);
        let list_handle = handle.clone();
        let model_for_list = model.clone();
        let content = div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p_2()
            .font_family("Consolas, monospace")
            .text_size(px(12.0))
            .bg(rgb(ui_theme::CARD))
            .child(
                uniform_list(
                    scroll_id,
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let diff = diff.as_deref();
                        range
                            .map(|index| {
                                this.render_diff_row(
                                    diff,
                                    model_for_list.row_at(index),
                                    headers_expanded,
                                    header_target,
                                    &empty_message,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&list_handle)
                .with_width_from_item(width_measure_index)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0)),
            )
            .into_any_element();

        scrollable_uniform_frame(
            scroll_id,
            ScrollbarMode::Both,
            content,
            handle,
            content_present,
            cx,
        )
        .into_any_element()
    }

    /// 按差异视图上下文取对应槽位的语法高亮结果（带主题变体守卫）。
    fn syntax_spans_for_diff(&self, target: DiffHeaderTarget) -> Option<&SharedSyntaxSpans> {
        let spans = match target {
            DiffHeaderTarget::Worktree => &self.diff_syntax,
            DiffHeaderTarget::History => &self.history_diff_syntax,
            DiffHeaderTarget::Stash => &self.stash_preview.diff_syntax,
            DiffHeaderTarget::Browse => &self.browse.diff_syntax,
        };
        spans
            .as_deref()
            .filter(|spans| spans.dark == ui::theme::active_variant().is_dark())
    }

    fn render_diff_row(
        &self,
        diff: Option<&FileDiff>,
        row: DiffRenderRow,
        headers_expanded: bool,
        header_target: DiffHeaderTarget,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 仅工作区差异视图提供部分暂存交互（历史/贮藏/浏览只读）；Office
        // 文档的文本化差异是提取合成的，不能按块/按行回写（部分暂存守卫在
        // 服务层同样拒绝），这里直接不显示按钮。
        let partial_stage_enabled =
            header_target == DiffHeaderTarget::Worktree && !diff.is_some_and(|d| d.is_binary);
        match row {
            DiffRenderRow::HeaderToggle => {
                let summary = if headers_expanded {
                    "Diff 元信息（点击折叠）"
                } else {
                    "Diff 元信息（点击展开）"
                };
                diff_header_toggle(summary, header_target, cx).into_any_element()
            }
            DiffRenderRow::DiffLine(index) => {
                let Some(line) = diff.and_then(|diff| diff.lines.get(index)) else {
                    return diff_line(DiffLineKind::Context, None, None, String::new(), None)
                        .into_any_element();
                };
                // 按差异上下文取对应槽位的语法高亮（仅全文模式计算过）。
                let syntax_spans = self
                    .syntax_spans_for_diff(header_target)
                    .and_then(|spans| spans.lines.get(index).map(Vec::as_slice));
                if line.kind == DiffLineKind::Header {
                    let is_hunk_header = line.content.starts_with("@@");
                    let row_element =
                        diff_line(line.kind.clone(), None, None, line.content.clone(), None);
                    if partial_stage_enabled && is_hunk_header {
                        // hunk 分隔行右侧提供整块暂存/取消暂存按钮。
                        let is_stage = diff
                            .map(|diff| diff.scope == DiffScope::Unstaged)
                            .unwrap_or(true);
                        let label: &'static str = if is_stage {
                            "暂存此块"
                        } else {
                            "取消暂存此块"
                        };
                        let hunk_index = line.hunk_index;
                        return div()
                            .relative()
                            .child(row_element)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_1()
                                    .flex()
                                    .items_center()
                                    .child(diff_hunk_action_button(
                                        hunk_index,
                                        label,
                                        move |this| {
                                            this.apply_hunk_partial_stage(hunk_index);
                                        },
                                        cx,
                                    )),
                            )
                            .into_any_element();
                    }
                    return row_element.into_any_element();
                }
                let row_element = diff_line(
                    display_diff_line_kind(line.kind.clone(), diff.is_some_and(|d| d.untracked)),
                    line.old_lineno,
                    line.new_lineno,
                    line.content.clone(),
                    syntax_spans,
                );
                if !partial_stage_enabled {
                    return row_element.into_any_element();
                }
                let selectable = matches!(line.kind, DiffLineKind::Added | DiffLineKind::Removed);
                if !selectable {
                    return row_element.into_any_element();
                }
                // +/- 行：点击选择（Ctrl/Cmd 多选、Shift 范围）。
                // 高亮层必须放在 row_element 之后：GPUI 按子元素顺序绘制，
                // 放在前面会被行自身的不透明背景完全盖住，视觉上不可见。
                let selected = self.diff_line_selection.contains(&index);
                div()
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            let multi = event.modifiers.control || event.modifiers.platform;
                            let shift = event.modifiers.shift;
                            this.toggle_diff_line_selection(index, multi, shift);
                            cx.notify();
                        }),
                    )
                    .child(row_element)
                    .when(selected, |this| {
                        this.child(
                            // 整行半透明主题色打底：复用输入选区 token（自带 alpha，跟随主题色），
                            // 叠加在 +/- 行背景色之上仍能清晰辨认选中范围。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .bg(ui_theme::rgba(ui_theme::INPUT_SELECTION)),
                        )
                        .child(
                            // 左缘 2px 主题色实线条作为第二重视觉信号。
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(2.0))
                                .bg(rgb(ui_theme::PRIMARY)),
                        )
                    })
                    .into_any_element()
            }
            DiffRenderRow::Empty => {
                let message = diff
                    .map(|diff| {
                        if diff.is_binary {
                            "二进制文件仅显示元信息"
                        } else {
                            "没有可显示的文本差异"
                        }
                    })
                    .unwrap_or(empty_message);
                diff_line(DiffLineKind::Context, None, None, message.to_string(), None)
                    .into_any_element()
            }
        }
    }

    pub(crate) fn diff_section_header(
        &self,
        title: String,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let diff = match target {
            EncodingMenuTarget::Worktree => self.diff.as_deref(),
            EncodingMenuTarget::History => self.history_diff.as_deref(),
            EncodingMenuTarget::Stash => self.stash_preview.diff.as_deref(),
            EncodingMenuTarget::Browse => self.browse.diff.as_deref(),
            // 追溯视图没有 FileDiff；该 target 不经此头部渲染
            EncodingMenuTarget::Blame => None,
        };
        div()
            .flex_none()
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
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // 二进制文件没有全文/编码差异可言，隐藏这两个工具按钮
                    .when(!diff.is_some_and(|diff| diff.is_binary), |this| {
                        this.child(self.full_file_toggle_button(target, cx))
                            .child(self.encoding_button(diff, target, cx))
                    })
                    // 按行选择非空时的部分暂存入口（仅工作区差异视图）。
                    .when(
                        target == EncodingMenuTarget::Worktree
                            && !self.diff_line_selection.is_empty(),
                        |this| {
                            let count = self.diff_line_selection.len();
                            let is_stage = self
                                .diff
                                .as_ref()
                                .map(|diff| diff.scope == DiffScope::Unstaged)
                                .unwrap_or(true);
                            let label = if is_stage {
                                format!("暂存选中行({count})")
                            } else {
                                format!("取消暂存选中行({count})")
                            };
                            this.child(
                                // 尺寸规格与「全文/编码」工具按钮一致（px 8 / py 2 /
                                // RADIUS_XS / 11px），避免撑高差异区标题栏。
                                div()
                                    .id("stage-selected-diff-lines-button")
                                    .flex_none()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::ACCENT))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .cursor_pointer()
                                    .hover(|hover| hover.bg(rgb(ui_theme::SECONDARY)))
                                    .child(label)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.apply_selected_partial_stage();
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    // 工作区差异的「追溯」入口：打开该文件的追溯视图
                    //（规格严格复用「全文/编码」工具按钮；二进制文件不提供）。
                    .when(
                        target == EncodingMenuTarget::Worktree
                            && self.diff.as_ref().is_some_and(|diff| !diff.is_binary),
                        |this| {
                            let path = self
                                .diff
                                .as_ref()
                                .map(|diff| diff.path.clone())
                                .unwrap_or_default();
                            this.child(
                                div()
                                    .id("worktree-diff-blame-button")
                                    .flex_none()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::ACCENT))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .cursor_pointer()
                                    .hover(|hover| hover.bg(rgb(ui_theme::SECONDARY)))
                                    .child("追溯")
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_blame_file(path.clone());
                                        cx.notify();
                                    })),
                            )
                        },
                    ),
            )
    }

    fn encoding_button(
        &self,
        diff: Option<&FileDiff>,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let requested = self.current_diff_encoding_choice();
        let label = diff
            .map(diff_encoding_label)
            .unwrap_or_else(|| format!("编码：{}", requested.label()));
        div()
            .id(match target {
                EncodingMenuTarget::Worktree => "worktree-diff-encoding",
                EncodingMenuTarget::History => "history-diff-encoding",
                EncodingMenuTarget::Stash => "stash-diff-encoding",
                EncodingMenuTarget::Browse => "browse-encoding",
                EncodingMenuTarget::Blame => "blame-encoding",
            })
            .relative()
            .flex_none()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .bg(rgb(ui_theme::ACCENT))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.toggle_encoding_menu(target);
                    cx.notify();
                }),
            )
            .child(label)
    }

    pub(crate) fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status_label = if self.busy { "运行中" } else { "就绪" };
        // 代码理解后台任务条（CU2-T5）：任务运行中且已从当前页面分离时显示，
        // 提供查看进度与取消入口；后台完成只提示一次（toast 在事件处理侧）。
        let understanding_tasks = self
            .understanding_tasks
            .iter()
            .filter(|(key, state)| {
                state.session.has_active_request() && self.understanding_task_detached_from(key)
            })
            .map(|(key, state)| {
                (
                    key.clone(),
                    state.display.question.clone(),
                    state.cancel_pending,
                )
            })
            .collect::<Vec<_>>();
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_deref())
            .unwrap_or("未打开仓库");
        let staged_count = self.change_indexes.staged.len();
        let unstaged_count = self.change_indexes.unstaged.len();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(chrome_view::STATUS_BAR_HEIGHT))
            .px(px(16.0))
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .text_size(px(10.0))
            .child(
                div()
                    .flex_none()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(if self.busy {
                        ui_theme::PRIMARY
                    } else {
                        ui_theme::GIT_ADDED
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(status_label),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(branch.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(if self.busy {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::MUTED_FOREGROUND)
                    })
                    .child(if self.busy {
                        format!("{}...", self.status)
                    } else {
                        self.status.clone()
                    }),
            )
            .children(
                understanding_tasks
                    .iter()
                    .map(|(project_key, question, cancel_pending)| {
                        let repo_name = Path::new(project_key)
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| project_key.clone());
                        let question = question.clone();
                        let cancel_pending = *cancel_pending;
                        let key_for_view = project_key.clone();
                        let key_for_cancel = project_key.clone();
                        div()
                            .id(format!("understanding-taskbar-{project_key}"))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(6.0))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(div().max_w(px(280.0)).truncate().child(if cancel_pending {
                                format!("代码理解·取消中：{repo_name}")
                            } else {
                                format!(
                                    "代码理解：{repo_name} · {}",
                                    code_understanding_view::excerpt_line(&question, 24)
                                )
                            }))
                            .child(
                                div()
                                    .id(format!("understanding-taskbar-view-{project_key}"))
                                    .cursor_pointer()
                                    .hover(|this| this.text_color(rgb(ui_theme::CONTENT_PRIMARY)))
                                    .child("查看")
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.show_understanding_task(key_for_view.clone(), cx);
                                    })),
                            )
                            .when(!cancel_pending, |this| {
                                this.child(
                                    div()
                                        .id(format!("understanding-taskbar-cancel-{project_key}"))
                                        .cursor_pointer()
                                        .hover(|this| {
                                            this.text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                        })
                                        .child("取消")
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            this.cancel_understanding_task_for(&key_for_cancel, cx);
                                        })),
                                )
                            })
                    }),
            )
            .when_some(self.last_error.clone(), |this, error| {
                this.child(
                    div()
                        .max_w(px(360.0))
                        .truncate()
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(format!("错误：{error}")),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("{unstaged_count} 未暂存 · {staged_count} 已暂存")),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
    }

    pub(crate) fn has_active_loading(&self) -> bool {
        let tab = self.active_tab_state();
        tab.operation_kind.shows_progress()
            && (tab.busy || tab.loading != RepositoryLoading::default())
    }

    pub(crate) fn render_feedback_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 所有通知气泡统一在右下角堆叠（按队列顺序），不再按重要度分左右；
        // 每个气泡（成功/信息/警告/错误）都带关闭按钮（feedback_bubble 内置）。
        let mut stack = feedback_stack();
        for feedback in self.feedbacks.iter() {
            stack = stack.child(feedback_bubble(feedback, cx));
        }

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .child(stack)
            // 状态文字已在底栏展示，操作期间只保留轻量进度线，避免重复悬浮框。
            .when(self.has_active_loading(), |this| {
                this.child(bottom_progress_bar(self.progress_phase))
            })
    }

    pub(crate) fn render_credentials(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(pending) = self.pending_credential.as_ref() else {
            return div().into_any_element();
        };

        div()
            .absolute()
            .top(px(70.0))
            .right(px(18.0))
            .w(px(420.0))
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_2()
            .cursor(CursorStyle::Arrow)
            .occlude()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("需要凭据"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!("远端：{}", pending.request.url)),
            )
            .child(self.input(FieldId::CredentialUsername, true, window, cx))
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.input(FieldId::CredentialSecret, true, window, cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Ssh,
                |this| {
                    this.child(self.toggle_row(
                        "credential-use-ssh-agent",
                        "使用 SSH agent",
                        self.credential_use_ssh_agent,
                        |this, _, _| this.credential_use_ssh_agent = !this.credential_use_ssh_agent,
                        cx,
                    ))
                    .when(!self.credential_use_ssh_agent, |this| {
                        this.child(self.input(FieldId::CredentialKeyPath, true, window, cx))
                    })
                    .child(self.input(
                        FieldId::CredentialPassphrase,
                        true,
                        window,
                        cx,
                    ))
                },
            )
            .child(self.toggle_row(
                "save-credential",
                "保存到系统凭据管理器",
                self.save_credential,
                |this, _, _| this.save_credential = !this.save_credential,
                cx,
            ))
            .when(self.save_credential, |this| {
                this.child(self.input(FieldId::CredentialDisplayName, true, window, cx))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!self.save_credential, |this| this.opacity(0.55))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("复用范围"),
                    )
                    .child(self.credential_scope_button(
                        "仅此远端",
                        CredentialScope::RemoteUrl,
                        self.save_credential,
                        cx,
                    ))
                    .child(self.credential_scope_button(
                        "同站点",
                        CredentialScope::Host,
                        self.save_credential,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(self.primary_button(
                        "使用凭据",
                        true,
                        |this, _, _| this.use_credentials(),
                        cx,
                    ))
                    .child(self.button(
                        "取消",
                        true,
                        |this, _, _| this.cancel_credential_request(),
                        cx,
                    )),
            )
            .into_any_element()
    }
}
