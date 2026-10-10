//! 程序内搜索：从仓库切换列表与功能目录定位页面，不依赖代码索引。
//! 输入复用字段适配，Kit Command 负责虚拟列表、指针选择和滚动。

mod catalog;
mod repositories;
pub(crate) use repositories::PendingSearchBranch;

use gpui::{
    Context, Entity, FontWeight, IntoElement, KeyDownEvent, MouseButton, Window, div, prelude::*,
    px,
};
use gpui_kit::base::FocusTrapElement;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{Disableable, IndexPath};

use crate::ui::components::{
    dialog_overlay, dialog_panel_size, floating_panel, icon_button, tooltip_text,
};
use crate::ui::icons::ToolbarIcon;
use crate::ui::theme::{self as theme, rgb};
use crate::{FieldId, MainMode, RepositoryView, SettingsCategory};
use catalog::{SearchEntry, SearchScope, SearchTarget, filter_entries, function_entries};

pub(crate) struct CodeSearchPaletteState {
    command: Entity<CommandState>,
    query: String,
    scope: SearchScope,
    repositories: Option<repositories::RepositorySearchCatalog>,
    catalog_changed: bool,
}

impl RepositoryView {
    pub(crate) fn toggle_code_search_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.code_search_palette.is_some() {
            self.close_code_search();
        } else {
            self.open_code_search(window, cx);
        }
    }

    pub(crate) fn open_code_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popups();
        self.code_palette_search.clear();
        self.code_search_palette = Some(CodeSearchPaletteState {
            command: cx.new(|cx| CommandState::new(window, cx)),
            query: self.code_palette_search.value.clone(),
            scope: SearchScope::default(),
            repositories: None,
            catalog_changed: false,
        });
        // 挂载后由 maintain_overlay_focus 聚焦字段，保留触发器的返回焦点。
        cx.notify();
    }

    pub(crate) fn close_code_search(&mut self) {
        if self.code_search_palette.is_some() {
            self.request_overlay_focus_restore();
        }
        self.code_search_palette = None;
    }

    pub(crate) fn sync_code_search_scope(&mut self) {
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        if !palette.scope.scans_saved_repositories(&self.code_palette_search.value) {
            // 丢弃目录会取消扫描；迟到结果仍由请求代际守卫拒绝。
            palette.repositories = None;
        } else if palette.repositories.is_none() {
            let repositories = self.start_saved_repository_search();
            self.code_search_palette.as_mut().unwrap().repositories = Some(repositories);
        }
    }

    fn app_search_entries(&self) -> Vec<SearchEntry> {
        let mut entries = function_entries(
            self.active_tab_id(),
            self.busy,
            self.snapshot.as_ref(),
            self.ai_settings.enabled,
        );
        // 空输入只构造设置结果，不遍历各仓库的分支、标签和贮藏。
        if self.code_palette_search.value.trim().is_empty() {
            return filter_entries(entries, "");
        }
        let scope = self
            .code_search_palette
            .as_ref()
            .map(|palette| palette.scope)
            .unwrap_or_default();
        for tab in &self.tabs {
            if !scope.includes_repository(tab.id, self.active_tab_id()) {
                continue;
            }
            let Some(path) = tab.repo_path.as_ref() else {
                continue;
            };
            let repo_name = tab.display_name();
            entries.push(SearchEntry::new(
                repo_name.clone(),
                path.to_string_lossy().to_string(),
                "仓库",
                "项目 repository repo",
                SearchTarget::Repository(tab.id),
            ));
            // 活动仓库始终读当前真值；其他仓库只使用它们自己的已加载快照。
            let snapshot = if self.active_tab_id() == Some(tab.id) {
                self.snapshot.as_ref()
            } else {
                tab.snapshot.as_ref()
            };
            if let Some(snapshot) = snapshot {
                catalog::append_repository_entries(&mut entries, tab.id, &repo_name, snapshot);
            }
        }
        if let Some(tab_id) = self.active_tab_id() {
            for template in &self.workflow_templates {
                entries.push(SearchEntry::new(
                    template.display_name.clone(),
                    template.path.to_string_lossy().to_string(),
                    "工作流",
                    "模板 workflow template",
                    SearchTarget::Workflow(tab_id, template.path.clone()),
                ));
            }
        }
        if let Some(repositories) = self
            .code_search_palette
            .as_ref()
            .and_then(|palette| palette.repositories.as_ref())
        {
            for repo in &repositories.repositories {
                entries.extend(repositories::saved_repository_entries(repo));
            }
        }
        for entry in &mut entries {
            let target_tab = match &entry.target {
                SearchTarget::Repository(id)
                | SearchTarget::Branch(id, ..)
                | SearchTarget::Tag(id, ..)
                | SearchTarget::Remote(id, ..)
                | SearchTarget::Stash(id, ..) => Some(*id),
                _ => None,
            };
            // 搜索可以继续导航当前仓库，但不能绕过仓库切换器的操作中守卫。
            if self.busy && target_tab.is_some() && target_tab != self.active_tab_id() {
                entry.disabled_reason = Some("当前操作进行中，暂不可切换仓库");
            }
            if self.busy
                && matches!(
                    entry.target,
                    SearchTarget::SavedRepository(_) | SearchTarget::SavedBranch(..)
                )
            {
                entry.disabled_reason = Some("当前操作进行中，暂不可打开其他仓库");
            }
        }
        filter_entries(entries, &self.code_palette_search.value)
    }

    /// 用业务身份重新查目录，避免迟到的点击回调操作已关闭仓库或已删除对象。
    fn confirm_app_search(
        &mut self,
        entry: SearchEntry,
        session: &Entity<CommandState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.code_search_palette.as_ref().is_none_or(|palette| {
            &palette.command != session
                || palette.query != self.code_palette_search.value
                || palette.catalog_changed
        }) {
            return;
        }
        let Some(current) = self
            .app_search_entries()
            .into_iter()
            .find(|candidate| candidate.target == entry.target && candidate.title == entry.title)
        else {
            return;
        };
        if let Some(reason) = current.disabled_reason {
            self.notify_warning(reason, cx);
            return;
        }
        self.close_code_search();
        match current.target {
            SearchTarget::Settings(category) => {
                if self.settings_center.is_none() {
                    self.open_settings_center();
                }
                self.select_settings_category(category);
            }
            SearchTarget::AiSettings(tab) => {
                if self.settings_center.is_none() {
                    self.open_settings_center();
                }
                self.select_settings_category(SettingsCategory::Ai);
                self.select_ai_settings_tab(tab);
            }
            target => {
                self.close_settings_center();
                match target {
                    SearchTarget::Mode(tab_id, mode) | SearchTarget::Feature(tab_id, _, mode) => {
                        self.activate_tab(tab_id);
                        if mode == MainMode::Browse && self.browse.target.is_none() {
                            if let Some(branch) = self
                                .snapshot
                                .as_ref()
                                .and_then(|snapshot| {
                                    snapshot.branches.iter().find(|branch| branch.is_head)
                                })
                                .cloned()
                            {
                                self.open_browse_branch(branch.name, branch.kind);
                            } else {
                                self.set_main_mode(mode);
                            }
                        } else {
                            self.set_main_mode(mode);
                        }
                    }
                    SearchTarget::Branch(tab_id, name, _kind) => {
                        self.activate_tab(tab_id);
                        self.locate_search_branch(name);
                    }
                    SearchTarget::SavedRepository(path) => self.open_repo(path),
                    SearchTarget::SavedBranch(path, name, kind) => {
                        self.open_saved_search_branch(path, name, kind)
                    }
                    SearchTarget::Tag(tab_id, name) => {
                        self.activate_tab(tab_id);
                        self.open_browse_tag(name);
                    }
                    SearchTarget::Remote(tab_id, name) => {
                        self.activate_tab(tab_id);
                        self.selected_remote = Some(name);
                        self.open_remote_manager();
                    }
                    SearchTarget::Stash(tab_id, oid) => {
                        self.activate_tab(tab_id);
                        if let Some(index) = self
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| {
                                snapshot.stashes.iter().find(|stash| stash.oid == oid)
                            })
                            .map(|stash| stash.index)
                        {
                            self.view_stash(index);
                        }
                    }
                    SearchTarget::Repository(tab_id) => self.activate_tab(tab_id),
                    SearchTarget::Workflow(tab_id, path) => {
                        self.activate_tab(tab_id);
                        self.set_main_mode(MainMode::Workflow);
                        self.load_workflow_file(path, cx);
                    }
                    SearchTarget::OpenRepository => self.browse_open(),
                    SearchTarget::CloneRepository => self.open_clone_dialog(window, cx),
                    SearchTarget::CreateBranch(_) => self.open_create_branch_dialog(),
                    SearchTarget::CreateTag(_) => {
                        self.open_tag_form_dialog(None, "当前 HEAD".into())
                    }
                    SearchTarget::CreateStash(_) => self.open_stash_dialog(),
                    SearchTarget::Submodules(_) => self.open_submodule_manager(),
                    SearchTarget::Remotes(_) => self.open_remote_manager(),
                    SearchTarget::Pull(_) => {
                        self.open_remote_branch_operation(crate::RemoteBranchOperationKind::Pull)
                    }
                    SearchTarget::Push(_) => {
                        self.open_remote_branch_operation(crate::RemoteBranchOperationKind::Push)
                    }
                    SearchTarget::WorkflowEditor(_) => self.open_workflow_editor(cx),
                    SearchTarget::ReviewHistory(_) => self.open_ai_review_history(),
                    SearchTarget::Settings(_) | SearchTarget::AiSettings(_) => unreachable!(),
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_code_search_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.code_search_palette.is_none() {
            return div().into_any_element();
        }
        let query = self.code_palette_search.value.clone();
        let palette = self.code_search_palette.as_mut().unwrap();
        if palette.query != query || palette.catalog_changed {
            // 查询变更后从新结果第一项开始，不能继承旧列表位置误选其他对象。
            palette.command = cx.new(|cx| CommandState::new(window, cx));
            palette.query = query;
            palette.catalog_changed = false;
        }
        let command_state = palette.command.clone();
        let search_all = palette.scope == SearchScope::AllRepositories;
        let entries = self.app_search_entries();
        let count = entries.len();
        let progress_label = self
            .code_search_palette
            .as_ref()
            .unwrap()
            .repositories
            .as_ref()
            .map(|repositories| repositories.progress_label(count))
            .unwrap_or_else(|| format!("{count} 个结果"));
        let items: Vec<_> = entries
            .iter()
            .map(|entry| {
                let entry = entry.clone();
                CommandItem::new()
                    .label(entry.title.clone())
                    .disabled(entry.disabled_reason.is_some())
                    .child(move |_, _| {
                        let subtitle = entry
                            .disabled_reason
                            .map(str::to_owned)
                            .unwrap_or_else(|| entry.subtitle.clone());
                        let tooltip = format!("{}\n{}", entry.title, subtitle);
                        div()
                            .id(format!(
                                "app-search-item-{:?}-{}",
                                entry.target, entry.title
                            ))
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w(px(0.0))
                            .gap_3()
                            .tooltip(move |_, cx| tooltip_text(tooltip.clone(), cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(entry.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_META))
                                            .text_color(rgb(theme::CONTENT_SECONDARY))
                                            .truncate()
                                            .child(subtitle),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(theme::RADIUS_XS))
                                    .bg(rgb(theme::SURFACE_SUNKEN))
                                    .text_size(px(theme::TYPE_META))
                                    .text_color(rgb(theme::CONTENT_SECONDARY))
                                    .child(entry.group),
                            )
                    })
            })
            .collect();
        let owner = cx.entity().downgrade();
        let confirm_entries = entries.clone();
        let confirm_session = command_state.clone();
        let command = Command::new(&command_state)
            .searchable(false)
            .filterable(false)
            .bordered(false)
            .items(items)
            .w_full()
            .bg(rgb(theme::WB_PANEL))
            .empty(|_, _, _| {
                div()
                    .py_6()
                    .px_4()
                    .text_size(px(13.0))
                    .text_color(rgb(theme::CONTENT_SECONDARY))
                    .child("没有匹配结果，请尝试分支名称、功能名称或其他关键词。")
            })
            .on_confirm(move |index, window, cx| {
                if let Some(entry) = confirm_entries.get(index.row).cloned() {
                    let _ = owner.update(cx, |this, cx| {
                        this.confirm_app_search(entry, &confirm_session, window, cx)
                    });
                }
            });
        let (width, height) = dialog_panel_size(window, 680.0, 500.0);
        let command = command.max_h(px(f32::from(height) - 196.0));
        dialog_overlay()
            .id("code-palette-overlay")
            .focus_trap("code-palette-overlay-trap", &self.code_palette_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_code_search();
                    cx.notify();
                }),
            )
            .child(
                floating_panel()
                    .id("code-search-palette")
                    .w(width)
                    .h(height)
                    .border_1()
                    .border_color(rgb(theme::BORDER_MUTED))
                    .flex()
                    .flex_col()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        // 组合输入的候选导航、确认和取消必须交还 Kit/IME。
                        if this.kit_field_has_ime_composition(
                            FieldId::CodePaletteSearch,
                            window,
                            cx,
                        ) {
                            return;
                        }
                        let key = event.keystroke.key.as_str();
                        if key == "escape" {
                            this.close_code_search();
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        // 列表自身或关闭按钮取得焦点时使用各自的 Kit 键盘语义。
                        if !this.code_palette_search.focus.is_focused(window) {
                            return;
                        }
                        if matches!(key, "up" | "down" | "enter")
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            let selected = command_state
                                .read(cx)
                                .selected_index()
                                .map(|index| index.row);
                            if key == "enter" {
                                if let Some(entry) =
                                    selected.and_then(|index| entries.get(index)).cloned()
                                {
                                    this.confirm_app_search(entry, &command_state, window, cx);
                                }
                            } else if let Some(next) =
                                catalog::next_selection(&entries, selected, key == "down")
                            {
                                command_state.update(cx, |state, cx| {
                                    state.set_selected_index(Some(IndexPath::new(next)), window, cx)
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .pt_3()
                            .pb_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("全局搜索"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_META))
                                            .text_color(rgb(theme::CONTENT_SECONDARY))
                                            .child("搜索分支、功能、设置、仓库和工作流"),
                                    ),
                            )
                            .child(
                                icon_button(
                                    "app-search-close".into(),
                                    ToolbarIcon::Close,
                                    "关闭全局搜索",
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_code_search();
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(div().flex_none().px_4().pb_3().child(self.input(
                        FieldId::CodePaletteSearch,
                        false,
                        window,
                        cx,
                    )))
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .pb_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme::CONTENT_SECONDARY))
                                    .child(if search_all { "搜索全部仓库" } else { "仅搜索当前仓库" }),
                            )
                            .child(self.toggle_switch(
                                "app-search-all-repositories",
                                search_all,
                                false,
                                |this, next, _, _| {
                                    if let Some(palette) = this.code_search_palette.as_mut() {
                                        palette.scope = if next {
                                            SearchScope::AllRepositories
                                        } else {
                                            SearchScope::CurrentRepository
                                        };
                                        palette.catalog_changed = true;
                                    }
                                    this.sync_code_search_scope();
                                },
                                cx,
                            ).label("搜索全部仓库")),
                    )
                    .child(div().flex_1().min_h(px(0.0)).px_2().child(command))
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(theme::BORDER_MUTED))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(theme::TYPE_META))
                            .text_color(rgb(theme::CONTENT_SECONDARY))
                            .child(progress_label)
                            .child("↑↓ 选择 · Enter 定位 · Esc 关闭"),
                    ),
            )
            .into_any_element()
    }
}
