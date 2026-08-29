// 全局符号搜索面板（Ctrl+P）：输入即查代码索引（FTS5），双栏展示
// 结果列表与选中符号的调用关系详情，Enter 打开内置追溯视图。
// 查询与详情均走 Short 任务池 + seq 代际守卫（乱序结果丢弃）。
//
// 键盘行为（↑↓ 切换 / Enter 确认 / Esc 关闭）在面板层 capture_key_down
// 拦截——先于输入框的 TextUp/TextDown/提交处理；该例外已记入
// AGENTS.md §8 键盘白名单。

use gpui::{Context, IntoElement, KeyDownEvent, MouseButton, Window, div, prelude::*, px};

use crate::tasks::TaskKind;
use crate::ui::components::dialog_overlay;
use crate::ui::theme::{self as ui_theme, rgb};
use crate::{FieldId, RepositoryView, UiEvent, send_ui_event};
use khaslana::code_index::{DetailOutcome, SymbolDetail, TraceHop, search_symbols, symbol_detail};

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
        self.code_search_palette = Some(crate::CodeSearchPaletteState::default());
        // 面板打开即聚焦输入框（重开续用上次关键词，立即按现有输入查询）。
        window.focus(&self.code_palette_search.focus);
        self.on_code_palette_input_changed();
        cx.notify();
    }

    pub(crate) fn close_code_search(&mut self) {
        self.code_search_palette = None;
    }

    /// 输入变化：清空旧结果并发起查询（seq 守卫，乱序结果丢弃）。
    pub(crate) fn on_code_palette_input_changed(&mut self) {
        self.code_palette_search_seq = self.code_palette_search_seq.wrapping_add(1);
        let seq = self.code_palette_search_seq;
        let query = self.code_palette_search.value.trim().to_string();
        let repo_db = self
            .active_repo_key()
            .and_then(|key| Self::index_db_path(&key).map(|db| (key, db)));
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        palette.results.clear();
        palette.detail = None;
        let Some((_repo_key, db_path)) = repo_db.filter(|_| !query.is_empty()) else {
            palette.searching = false;
            return;
        };
        palette.searching = true;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let hits = search_symbols(&db_path, &query, 50).unwrap_or_default();
            send_ui_event(&tx, UiEvent::CodePaletteSearchFinished { seq, hits });
        });
    }

    pub(crate) fn handle_code_palette_search_finished(
        &mut self,
        seq: u64,
        hits: Vec<khaslana::code_index::SearchHit>,
    ) {
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        if seq != self.code_palette_search_seq {
            return;
        }
        palette.searching = false;
        palette.results = hits;
        palette.selected_index = 0;
        self.request_code_palette_detail();
    }

    pub(crate) fn handle_code_palette_detail_finished(
        &mut self,
        seq: u64,
        detail: Option<Box<SymbolDetail>>,
    ) {
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        if seq != self.code_palette_detail_seq {
            return;
        }
        palette.detail = detail.map(|boxed| *boxed);
        palette.detail_loading = false;
    }

    fn palette_move_selection(&mut self, delta: isize) {
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        if palette.results.is_empty() {
            return;
        }
        let len = palette.results.len() as isize;
        let next = (palette.selected_index as isize + delta).clamp(0, len - 1);
        palette.selected_index = next as usize;
        self.scroll_selection_into_view();
        self.request_code_palette_detail();
    }

    fn palette_select(&mut self, index: usize) {
        let Some(palette) = self.code_search_palette.as_mut() else {
            return;
        };
        if index >= palette.results.len() {
            return;
        }
        palette.selected_index = index;
        self.scroll_selection_into_view();
        self.request_code_palette_detail();
    }

    /// 把选中行滚入可视窗口（↑↓ 越过可见范围时跟随；行高为固定估算值，
    /// 与渲染行的 px 尺寸保持一致）。列高按面板 520 - 标题/输入/操作行
    /// 估算，取保守的 10 行视口。
    fn scroll_selection_into_view(&mut self) {
        const ROW_HEIGHT: f32 = 26.0;
        const VISIBLE_ROWS: f32 = 10.0;
        let Some(palette) = self.code_search_palette.as_ref() else {
            return;
        };
        let selected = palette.selected_index as f32;
        let total = palette.results.len() as f32;
        let handle = self.scroll_handle("code-palette-results");
        let mut offset = f32::from(handle.offset().y);
        let sel_top = selected * ROW_HEIGHT;
        if sel_top < offset {
            offset = sel_top;
        } else if sel_top + ROW_HEIGHT > offset + VISIBLE_ROWS * ROW_HEIGHT {
            offset = sel_top + ROW_HEIGHT - VISIBLE_ROWS * ROW_HEIGHT;
        }
        let max_offset = (total * ROW_HEIGHT - VISIBLE_ROWS * ROW_HEIGHT).max(0.0);
        handle.set_offset(gpui::point(gpui::px(0.0), gpui::px(offset.min(max_offset))));
    }

    /// 请求选中符号的详情（qualified_name 精确查，无歧义）。
    fn request_code_palette_detail(&mut self) {
        self.code_palette_detail_seq = self.code_palette_detail_seq.wrapping_add(1);
        let seq = self.code_palette_detail_seq;
        let Some(palette) = self.code_search_palette.as_ref() else {
            return;
        };
        let Some(hit) = palette.results.get(palette.selected_index) else {
            return;
        };
        let Some(repo_key) = self.active_repo_key() else {
            return;
        };
        let Some(db_path) = Self::index_db_path(&repo_key) else {
            return;
        };
        let Some(repo_root) = self.active_tab().and_then(|tab| tab.repo_path.clone()) else {
            return;
        };
        let name = hit.qualified_name.clone();
        let tx = self.tx.clone();
        if let Some(palette) = self.code_search_palette.as_mut() {
            palette.detail_loading = true;
        }
        self.tasks.spawn(TaskKind::Short, move || {
            let detail = match symbol_detail(&db_path, Some(&repo_root), &name) {
                Ok(DetailOutcome::Found(detail)) => Some(detail),
                _ => None,
            };
            send_ui_event(&tx, UiEvent::CodePaletteDetailFinished { seq, detail });
        });
    }

    /// Enter 默认动作：关闭面板并打开内置追溯视图（最接近「跳到代码」）。
    fn palette_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self
            .code_search_palette
            .as_ref()
            .and_then(|palette| palette.results.get(palette.selected_index))
        else {
            return;
        };
        let path = hit.file_path.clone();
        self.close_code_search();
        self.open_blame_file(path);
        cx.notify();
    }

    pub(crate) fn render_code_search_palette(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(palette) = self.code_search_palette.as_ref() else {
            return div().into_any_element();
        };
        let selected = palette.selected_index;
        let searching = palette.searching;
        let has_repo = self.active_repo_key().is_some();

        let has_results = !palette.results.is_empty();

        // 结果列表行。
        let rows: Vec<gpui::AnyElement> = palette
            .results
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let is_selected = index == selected;
                div()
                    .id(format!("code-palette-row-{index}"))
                    .when(is_selected, |this| this.bg(rgb(ui_theme::SECONDARY)))
                    .when(!is_selected, |this| {
                        this.hover(|style| style.bg(rgb(ui_theme::SECONDARY)))
                    })
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.palette_select(index);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_1()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::SURFACE_SUNKEN))
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(hit.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(150.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(hit.name.clone()),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(format!("{}:{}", hit.file_path, hit.start_line)),
                    )
                    .into_any_element()
            })
            .collect();

        // 详情栏。
        let detail_pane: gpui::AnyElement = match palette.detail.as_ref() {
            Some(detail) => {
                let callers: Vec<gpui::AnyElement> =
                    detail.callers.iter().map(render_palette_hop).collect();
                let callees: Vec<gpui::AnyElement> =
                    detail.callees.iter().map(render_palette_hop).collect();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1()
                                    .rounded(px(ui_theme::RADIUS_XS))
                                    .bg(rgb(ui_theme::SURFACE_SUNKEN))
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .child(detail.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(detail.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(format!(
                                "{} · 第 {}-{} 行",
                                detail.file_path, detail.start_line, detail.end_line
                            )),
                    )
                    .when(!detail.callers.is_empty(), |this| {
                        this.child(palette_section_title("被调用（上游）"))
                            .children(callers)
                    })
                    .when(!detail.callees.is_empty(), |this| {
                        this.child(palette_section_title("调用（下游）"))
                            .children(callees)
                    })
                    .when(
                        detail.callers.is_empty() && detail.callees.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("索引中没有该符号的调用关系边"),
                            )
                        },
                    )
                    .into_any_element()
            }
            None => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(if palette.detail_loading {
                    "详情加载中…".to_string()
                } else if searching {
                    "查询中…".to_string()
                } else if !has_repo {
                    "当前没有打开的仓库".to_string()
                } else {
                    "输入关键词检索符号（支持驼峰拆分，如 push branch 命中 pushBranch）".to_string()
                })
                .into_any_element(),
        };

        // 底部操作行 + 提示。
        let file_path = palette
            .results
            .get(selected)
            .map(|hit| hit.file_path.clone());
        let has_selection = file_path.is_some();
        let actions_row = div()
            .flex()
            .items_center()
            .gap_2()
            .pt_2()
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .child(self.button(
                "追溯此文件",
                has_selection,
                |this, _, cx| {
                    this.palette_confirm(cx);
                },
                cx,
            ))
            .child(self.button(
                "文件历史",
                has_selection,
                {
                    let path = file_path.clone();
                    move |this, _, _| {
                        if let Some(path) = path.clone() {
                            this.close_code_search();
                            this.view_file_history(path);
                        }
                    }
                },
                cx,
            ))
            .child(self.button(
                "复制路径",
                has_selection,
                {
                    let path = file_path.clone();
                    move |this, _, cx| {
                        if let Some(path) = path.clone() {
                            this.close_code_search();
                            this.copy_file_absolute_path(path, cx);
                        }
                    }
                },
                cx,
            ))
            .child(self.button(
                "打开目录",
                has_selection,
                {
                    let path = file_path.clone();
                    move |this, _, cx| {
                        if let Some(path) = path.clone() {
                            this.close_code_search();
                            this.open_file_parent_directory(path, cx);
                        }
                    }
                },
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .text_right()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("↑↓ 切换 · Enter 追溯 · Esc 关闭"),
            );

        dialog_overlay()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // 点击遮罩关闭（面板自身 stop_propagation 挡住内部点击）。
                    this.close_code_search();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("code-search-palette")
                    .w(px(760.0))
                    .h(px(520.0))
                    .p_4()
                    .rounded(px(ui_theme::RADIUS_XS))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // 键盘拦截：capture 阶段先于输入框处理 ↑/↓/Enter/Esc。
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        match event.keystroke.key.as_str() {
                            "down" => {
                                this.palette_move_selection(1);
                                cx.stop_propagation();
                            }
                            "up" => {
                                this.palette_move_selection(-1);
                                cx.stop_propagation();
                            }
                            "enter" => {
                                this.palette_confirm(cx);
                                cx.stop_propagation();
                            }
                            "escape" => {
                                this.close_code_search();
                                cx.stop_propagation();
                                cx.notify();
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("符号搜索"),
                    )
                    .child(self.input(FieldId::CodePaletteSearch, false, window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .gap_3()
                            .child(
                                div()
                                    .id("code-palette-results")
                                    .w(px(320.0))
                                    .min_h(px(0.0))
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle("code-palette-results"))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .when(has_results, |this| this.children(rows))
                                    .when(!has_results && !searching, |this| {
                                        this.child(
                                            div()
                                                .pt_2()
                                                .text_size(px(12.0))
                                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                                .child(if has_repo {
                                                    "无匹配符号。改用更短的词（如 push 代替 pushBranch）"
                                                } else {
                                                    "当前没有打开的仓库"
                                                }),
                                        )
                                    }),
                            )
                            .child(div().w(px(1.0)).h_full().bg(rgb(ui_theme::BORDER_MUTED)))
                            .child(
                                div()
                                    .id("code-palette-detail")
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .min_w(px(0.0))
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle("code-palette-detail"))
                                    .child(detail_pane),
                            ),
                    )
                    .child(actions_row),
            )
            .into_any_element()
    }
}

fn palette_section_title(text: &str) -> gpui::AnyElement {
    div()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
        .child(text.to_string())
        .into_any_element()
}

fn render_palette_hop(hop: &TraceHop) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .py_px()
        .min_w(px(0.0))
        .child(
            div()
                .flex_none()
                .px_1()
                .rounded(px(ui_theme::RADIUS_XS))
                .bg(rgb(ui_theme::SURFACE_SUNKEN))
                .text_size(px(10.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(format!("H{} {}", hop.hop, hop.risk)),
        )
        .child(
            div()
                .flex_none()
                .max_w(px(170.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                .child(hop.name.clone()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(hop.file_path.clone()),
        )
        .into_any_element()
}
