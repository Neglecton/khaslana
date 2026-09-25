//! RepositoryView 的通用输入、菜单、差异与状态渲染。

use crate::*;
use gpui_kit::base::{Button as BaseButton, FocusTrapElement};
use gpui_kit::component::{Disableable, Sizable, Size, status_bar::StatusBar, switch::Switch};

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

    /// 受控滑块开关（Kit Switch）：点击或键盘（Tab 聚焦 + Enter/Space）激活后
    /// 经 `on_change` 回传**请求值**，由调用方写回状态。
    ///
    /// Kit 的 Switch 是受控组件：`on_change` 只携带请求值，不替调用方落状态；
    /// 且它在启用时不拦截冒泡，所以不要在同一元素的外层容器再挂同一动作
    /// （会双重触发）。需要「点文字也能切换」时给文字单独挂处理器。
    pub(crate) fn toggle_switch(
        &self,
        id: impl Into<gpui::ElementId>,
        checked: bool,
        disabled: bool,
        on_change: impl Fn(&mut Self, bool, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Switch::new(id)
            .checked(checked)
            .disabled(disabled)
            .with_size(Size::Small)
            .on_change(cx.listener(move |this, next: &bool, window, cx| {
                on_change(this, *next, window, cx);
                cx.notify();
            }))
    }

    /// 「开关 + 文字」设置行：开关本体可交互，文字区独立可点，
    /// 两者都走同一个动作（互不重叠，不会双重触发）。
    pub(crate) fn toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on_click = Rc::new(on_click);
        let label_click = on_click.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.toggle_switch(
                id,
                checked,
                false,
                move |this, _next, window, cx| on_click(this, window, cx),
                cx,
            ))
            .child(
                div()
                    .id(format!("{id}-label"))
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        label_click(this, window, cx);
                        cx.notify();
                    }))
                    .child(label),
            )
    }

    /// 渲染字段输入框。M7 起全部字段（静态 + 工作流动态）均迁至 Kit，
    /// 宿主由 `ensure_kit_fields` 每帧建好（渲染期只有 `&self`，建不了实体）；
    /// 自绘回退路径已随旧控件一并删除——无宿主即编程错误，渲染空占位
    /// 而不是静默回退到第二套输入实现。
    pub(crate) fn input(
        &self,
        id: FieldId,
        compact: bool,
        _window: &Window,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(field) = self.kit_field(id) else {
            debug_assert!(
                false,
                "FieldId {id:?} 没有 Kit 输入宿主（ensure_kit_fields 未覆盖）"
            );
            return div().into_any_element();
        };
        let blocked = self.active_operation_blocker_message().is_some()
            && !self.operation_blocker_allows_text_field(id);
        field.render(compact, blocked)
    }

    pub(crate) fn is_multiline_field(id: FieldId) -> bool {
        matches!(
            id,
            FieldId::CommitMessage
                | FieldId::TagMessage
                // 工作流模板 AI 功能需求描述（编辑器弹窗内多行输入）。
                | FieldId::WorkflowEditor(workflow_editor::WorkflowEditorFieldId::AiDescription)
        )
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
            ui_theme::CONTENT_PRIMARY
        } else {
            ui_theme::CONTENT_SECONDARY
        };
        BaseButton::new("repo-switcher-trigger")
            .disabled(!enabled)
            .accessibility_label("切换仓库")
            .focus_visible(|this| this.border_1().border_color(rgb(ui_theme::PRIMARY)))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .h(px(ui_theme::CONTROL_HEIGHT_TOOLBAR))
            .px(px(ui_theme::SPACE_3))
            .ml(px(ui_theme::SPACE_4))
            .rounded(px(ui_theme::RADIUS_MD))
            // 画板：仅显示仓库名的白色薄实体（图标 + 名称 + 展开箭头）。
            .bg(rgb(ui_theme::WB_PANEL))
            .shadow(crate::ui::components::control_shadow())
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed())
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
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
            .child(toolbar_icon(
                ToolbarIcon::Open,
                if enabled {
                    ui_theme::CONTENT_SECONDARY
                } else {
                    ui_theme::CONTENT_TERTIARY
                },
            ))
            .child(
                div()
                    .id("repo-switcher-name")
                    .text_size(px(ui_theme::TYPE_BODY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .max_w(px(140.0))
                    .min_w(px(0.0))
                    .truncate()
                    .tooltip(move |_window, cx| tooltip_text(repo_tooltip.clone(), cx))
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                |this, window, cx| {
                    this.close_repo_switcher();
                    this.open_clone_dialog(window, cx);
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
                    |this, window, cx| {
                        this.repo_switcher_search_open = true;
                        window.focus(&this.repo_switcher_search.focus, cx);
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
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
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
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
            .on_click(cx.listener(move |this, _event, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .child(toolbar_icon(icon, ui_theme::CONTENT_SECONDARY))
            .child(label)
    }

    /// 下拉分区小标题。
    fn repo_switcher_section_header(&self, label: &'static str) -> impl IntoElement {
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .text_size(px(11.0))
            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
            .when(is_active, |this| this.bg(rgb(ui_theme::STATE_HOVER)))
            .when(!is_active, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
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
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
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
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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

    /// 设置中心 overlay：Kit `Settings` 组件族（可拖拽侧栏 + 搜索 + 分组内容）。
    ///
    /// 侧栏与搜索由 Kit 渲染，活动页渲染时同步到 `settings_center`；
    /// 页头、分组和重置按钮仍由 Kit 负责（见 `settings_center::render_settings_kit`）。
    pub(crate) fn render_settings_center_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(category) = self.settings_center else {
            return div().into_any_element();
        };

        let settings = self.render_settings_kit(category, window, cx);
        // 面板尺寸按视口钳制：最小窗（860×520，高 DPI 下逻辑视口更小）里
        // 900×640 的固定尺寸会被根圆角裁掉，标题/关闭与底部内容点不到
        // （审查 R3）。
        let (panel_width, panel_height) = dialog_panel_size(window, 900.0, 640.0);

        // 遮罩不承载关闭：点击遮罩背景、遮罩上方的通知气泡（含其关闭按钮）
        // 都不关闭设置中心——唯一关闭入口是弹窗右上角的「✕」（Ctrl+, 快捷键
        // 保留 toggle 语义）。遮罩自身 occlude() 挡住下层 UI 的点击。
        // 焦点圈挂在遮罩上：Tab/Shift+Tab 在设置中心内循环，不会漏到下层。
        dialog_overlay()
            .id("settings-center-overlay")
            .focus_trap("settings-center-overlay-trap", &self.settings_center_focus)
            .child(
                div()
                    .id("settings-center-panel")
                    .w(panel_width)
                    // 高度按视口钳制，弹窗大小不随分类内容多少变化；
                    // 内容超出由 Kit SettingPage 的虚拟列表滚动。
                    .h(panel_height)
                    .min_w(px(0.0))
                    .rounded(px(ui_theme::RADIUS_PANEL))
                    .bg(rgb(ui_theme::WB_PANEL))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    // 顶栏：不画底边线，靠留白与侧栏的底色分区。
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .text_size(px(ui_theme::TYPE_PAGE_TITLE))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child("设置中心"),
                            )
                            .child(
                                // 关闭按钮用 Kit 基础按钮：可 Tab 聚焦、
                                // Enter/Space 激活，与其它壳层按钮同一套
                                // 键盘语义（审查 R6）。
                                BaseButton::new("settings-center-close")
                                    .focus_visible(|this| {
                                        this.border_1().border_color(rgb(ui_theme::PRIMARY))
                                    })
                                    .flex_none()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(ui_theme::RADIUS_SM))
                                    .bg(gpui::rgba(0x00000000))
                                    .text_size(px(14.0))
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.close_settings_center();
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    // 主体：Kit Settings（左：搜索 + 单层分类导航；右：SettingPage）。
                    // 底部内距 12px：滚动视口底缘不贴面板底缘，内容溢出时在
                    // 面板内侧裁切（设计稿 2026-09-23 二次调整）；侧栏底色随之
                    // 停在底缘上方，露出的面板底色即这条内距。
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .w_full()
                            .pb(px(12.0))
                            .child(settings),
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
            .id("tag-menu")
            .focus_trap("tag-menu-focus-trap", &self.context_menu_focus)
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(TAG_MENU_WIDTH))
            .child(context_menu_item(
                self,
                "tag-menu",
                "checkout-tag",
                "检出标签",
                !self.busy && !self.merge_in_progress(),
                {
                    let tag = menu.tag.clone();
                    move |this| this.checkout_tag(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                self,
                "tag-menu",
                "browse-tag",
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
                self,
                "tag-menu",
                "push-tag",
                "推送到远端...",
                !self.busy && has_remotes,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_tag_push_dialog(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                self,
                "tag-menu",
                "delete-tag",
                "删除标签",
                !self.busy,
                {
                    let tag = menu.tag.clone();
                    move |this| this.open_delete_tag_confirm(tag.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                self,
                "tag-menu",
                "delete-remote-tag",
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
            .id("stash-menu")
            .focus_trap("stash-menu-focus-trap", &self.context_menu_focus)
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
            .id("workflow-template-menu")
            .focus_trap(
                "workflow-template-menu-focus-trap",
                &self.context_menu_focus,
            )
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(WORKFLOW_TEMPLATE_MENU_WIDTH))
            .child(context_menu_item_with_context(
                self,
                "workflow-template-menu",
                "edit-template",
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
                self,
                "workflow-template-menu",
                "duplicate-template",
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
                self,
                "workflow-template-menu",
                "bind-shortcut",
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
                self,
                "workflow-template-menu",
                "delete-template",
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
            .id("change-menu")
            .focus_trap("change-menu-focus-trap", &self.context_menu_focus)
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
                    self,
                    "change-menu",
                    "copy-absolute-path",
                    "复制绝对路径",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item_with_context(
                    self,
                    "change-menu",
                    "open-parent-directory",
                    "打开文件所在目录",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this, cx| this.open_file_parent_directory(path.clone(), cx)
                    },
                    cx,
                ))
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "view-file-history",
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "blame-file",
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
                    self,
                    "change-menu",
                    "unstage-selected",
                    "取消暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.unstage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "unstage-all",
                    "取消暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.unstage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "discard-single",
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
                    self,
                    "change-menu",
                    "discard-selected",
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
                    self,
                    "change-menu",
                    "discard-all",
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
                    self,
                    "change-menu",
                    "view-file-history",
                    "查看文件历史",
                    true,
                    {
                        let path = menu.path.clone();
                        move |this| this.view_file_history(path.clone())
                    },
                    cx,
                ))
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "blame-file",
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
                    self,
                    "change-menu",
                    "stage-selected",
                    "暂存选定文件",
                    selected_count > 0 && !self.busy,
                    |this| this.stage_selected(),
                    cx,
                ))
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "stage-all",
                    "暂存所有文件",
                    all_count > 0 && !self.busy,
                    |this| this.stage_all(),
                    cx,
                ))
                .child(menu_separator())
                .child(context_menu_item(
                    self,
                    "change-menu",
                    "discard-single",
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
                    self,
                    "change-menu",
                    "discard-selected",
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
                    self,
                    "change-menu",
                    "discard-all",
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
            .id("file-path-menu")
            .focus_trap("file-path-menu-focus-trap", &self.context_menu_focus)
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
                self,
                "file-path-menu",
                "copy-absolute-path",
                "复制绝对路径",
                true,
                {
                    let path = menu.path.clone();
                    move |this, cx| this.copy_file_absolute_path(path.clone(), cx)
                },
                cx,
            ))
            .child(context_menu_item_with_context(
                self,
                "file-path-menu",
                "open-parent-directory",
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
                self,
                "file-path-menu",
                "view-file-history",
                "查看文件历史",
                true,
                {
                    let path = menu.path.clone();
                    move |this| this.view_file_history(path.clone())
                },
                cx,
            ))
            .child(context_menu_item(
                self,
                "file-path-menu",
                "blame-file",
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
            .id("commit-menu")
            .focus_trap("commit-menu-focus-trap", &self.context_menu_focus)
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
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                    self,
                    "commit-menu",
                    "uncommit-to-staged",
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
                self,
                "commit-menu",
                "reset-soft",
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
                self,
                "commit-menu",
                "reset-mixed",
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
                self,
                "commit-menu",
                "reset-hard",
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
                self,
                "commit-menu",
                "revert",
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
                self,
                "commit-menu",
                "cherry-pick",
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
                self,
                "commit-menu",
                "create-tag",
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
            .id("credential-menu")
            .focus_trap("credential-menu-focus-trap", &self.context_menu_focus)
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
            .child(self.credential_copy_menu_item(
                "credential-menu",
                "copy-name",
                "复制名称",
                name,
                "凭据名称",
                cx,
            ))
            .child(self.credential_copy_menu_item(
                "credential-menu",
                "copy-target",
                "复制站点/远端",
                target,
                "站点/远端",
                cx,
            ))
            .child(self.credential_copy_menu_item(
                "credential-menu",
                "copy-username",
                "复制用户名",
                username,
                "用户名",
                cx,
            ))
            .child(self.credential_copy_menu_item(
                "credential-menu",
                "copy-key-path",
                "复制 SSH Key 路径",
                key_path,
                "SSH Key 路径",
                cx,
            ))
            .into_any_element()
    }

    /// 凭据菜单的复制条目：与通用 `context_menu_item` 同一套键盘模型
    /// （登记动作 + 选中高亮），仅复制目标与成功提示不同。
    fn credential_copy_menu_item(
        &self,
        menu_id: &str,
        id: &str,
        label: &'static str,
        text: Option<String>,
        status_label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = text
            .as_ref()
            .is_some_and(|text| !text.is_empty() && text != "-");
        let on_click = {
            let text = text.clone();
            Rc::new(
                move |this: &mut RepositoryView, cx: &mut Context<RepositoryView>| {
                    this.copy_credential_text(text.clone(), status_label, cx);
                },
            ) as Rc<dyn Fn(&mut RepositoryView, &mut Context<RepositoryView>)>
        };
        let selected = self.register_context_menu_action(menu_id, id, enabled, on_click);
        context_menu_row(id, label, enabled, selected).on_click(cx.listener(
            move |this, _event, _window, cx| {
                cx.stop_propagation();
                if enabled {
                    this.copy_credential_text(text.clone(), status_label, cx);
                    cx.notify();
                }
            },
        ))
    }

    /// 提交菜单的「复制 SHA」条目：同通用键盘模型（登记 + 选中高亮）。
    fn commit_copy_sha_menu_item(&self, oid: String, cx: &mut Context<Self>) -> impl IntoElement {
        let on_click = {
            let oid = oid.clone();
            Rc::new(
                move |this: &mut RepositoryView, cx: &mut Context<RepositoryView>| {
                    this.copy_commit_sha(oid.clone(), cx);
                },
            ) as Rc<dyn Fn(&mut RepositoryView, &mut Context<RepositoryView>)>
        };
        let selected = self.register_context_menu_action("commit-menu", "copy-sha", true, on_click);
        context_menu_row("copy-sha", "复制 SHA 到剪贴板", true, selected)
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
    }

    pub(crate) fn render_column_splitter(
        &self,
        target: ResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let active = self.resize_state(target).is_some();
        let horizontal = matches!(
            target,
            ResizeTarget::HistoryDetails | ResizeTarget::HistoryInspectorHeight
        );
        // 弹窗或弹层菜单打开期间分割线不响应：不显示拖拽光标、不高亮、不响应鼠标，
        // 避免弹层边缘容差区内的悬停/点击被分割线抢走。
        let interactive = column_splitter_accepts_mouse_events(
            self.active_dialog.is_some(),
            self.any_popup_menu_open(),
        );
        // 悬浮工作台：拖拽区默认就是面板之间的空隙，不画线；悬停才浮出指示条，
        // 拖拽中常亮。侧栏那一列的面板间隙由拖拽区自身宽度提供（16px），
        // 页面内部相邻面板仍用 8px。
        let gap = if target == ResizeTarget::Sidebar {
            crate::chrome_view::SHELL_PADDING
        } else if target == ResizeTarget::HistoryInspectorHeight {
            12.0
        } else {
            ui_theme::SPACE_2
        };
        let indicator_color = if active {
            rgb(ui_theme::PRIMARY)
        } else {
            rgb(ui_theme::BORDER_STRONG)
        };

        div()
            .id(format!("column-splitter-{target:?}"))
            .group("column-splitter")
            .flex_none()
            .relative()
            .items_center()
            .justify_center()
            .map(|this| {
                if horizontal {
                    this.h(px(gap)).w_full()
                } else {
                    this.w(px(gap)).h_full()
                }
            })
            .when(interactive, |this| {
                this.cursor(if horizontal {
                    CursorStyle::ResizeRow
                } else {
                    CursorStyle::ResizeColumn
                })
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
                if target == ResizeTarget::HistoryInspectorHeight {
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .top(px(4.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w(px(48.0))
                                .h(px(4.0))
                                .rounded_full()
                                .bg(indicator_color),
                        )
                        .into_any_element()
                } else {
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .top(px(3.0))
                        .h(px(2.0))
                        .rounded_full()
                        .when(!active, |this| this.opacity(0.0))
                        .when(interactive && !active, |this| {
                            this.group_hover("column-splitter", |this| this.opacity(1.0))
                        })
                        .bg(indicator_color)
                        .into_any_element()
                }
            } else {
                div()
                    .absolute()
                    .left(px((gap - 2.0) / 2.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(px(2.0))
                    .rounded_full()
                    .when(!active, |this| this.opacity(0.0))
                    .when(interactive && !active, |this| {
                        this.group_hover("column-splitter", |this| this.opacity(1.0))
                    })
                    .bg(indicator_color)
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

    pub(crate) fn render_status(&self) -> impl IntoElement {
        let status_label = if self.busy { "运行中" } else { "就绪" };
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_deref())
            .unwrap_or("未打开仓库");
        let staged_count = self.change_indexes.staged.len();
        let unstaged_count = self.change_indexes.unstaged.len();
        // 状态栏用 Kit StatusBar 的三区结构（left 固定左、center 伸缩、right 固定右）。
        // Kit 默认带底色与顶边线，这里覆盖掉：状态栏坐在外壳的环境底上，
        // 自己铺色会糊掉窗口底部两角，面板投影已经足够分层。
        // 单行窄条：垂直内边距归零、9px 小字，状态点与间距同步缩小。
        StatusBar::new()
            .h(px(chrome_view::STATUS_BAR_HEIGHT))
            .py(px(0.0))
            .px(px(ui_theme::SPACE_4))
            .bg(ui_theme::rgba(0x00000000))
            .border_color(ui_theme::rgba(0x00000000))
            .text_size(px(9.0))
            .left(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(5.0))
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
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(status_label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(branch.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(if self.busy {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::CONTENT_SECONDARY)
                    })
                    .child(if self.busy {
                        format!("{}...", self.status)
                    } else {
                        self.status.clone()
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
            .right(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("{unstaged_count} 未暂存 · {staged_count} 已暂存")),
            )
            .right(
                div()
                    .flex_none()
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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

        // 焦点圈：凭据回调期间焦点移入提示面板，Tab 在面板内循环，
        // 不会漏到被它遮住的下层交互。
        div()
            .id("credential-prompt")
            .focus_trap("credential-prompt-trap", &self.credential_prompt_focus)
            .absolute()
            .top(px(70.0))
            .right(px(18.0))
            .w(px(420.0))
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
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
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("需要凭据"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
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
