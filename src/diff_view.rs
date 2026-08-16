//! 差异视图的全文/紧凑切换模块。
//!
//! 全文视图本质上是把 diff 的上下文行数拉满（`FULL_FILE_CONTEXT_LINES`），
//! libgit2 会把整份文件作为上下文输出，改动行依旧按 Added/Removed 高亮。
//! 这意味着全文视图能复用现有的差异渲染、编码检测、虚拟列表与横向滚动能力。
//!
//! 当文件体积超过 `FULL_FILE_MAX_BYTES` 时，GitService 层在分配逐行 String 之前
//! 就会返回 `FULL_FILE_TOO_LARGE_MESSAGE` 错误，UI 据此自动回退到紧凑差异。

use gpui::{Context, IntoElement, MouseButton, MouseDownEvent, div, prelude::*, px};

use crate::ui::theme::rgb;
use crate::{EncodingMenuTarget, MainMode, RepositoryView, ui::theme as ui_theme};

impl RepositoryView {
    /// 全文/差异切换按钮，放在差异区域标题栏编码按钮旁。
    /// 激活（全文）态使用强调色高亮，非激活态使用弱化样式。
    pub(crate) fn full_file_toggle_button(
        &self,
        target: EncodingMenuTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.full_file_view;
        // 按钮文案表达"点击后切换到的模式"
        let label = if active { "差异" } else { "全文" };
        div()
            .id(match target {
                EncodingMenuTarget::Worktree => "worktree-full-file-toggle",
                EncodingMenuTarget::History => "history-full-file-toggle",
                EncodingMenuTarget::Stash => "stash-full-file-toggle",
                EncodingMenuTarget::Browse => "browse-full-file-toggle",
                // 追溯视图不经 diff_section_header 渲染该按钮（视图本身即整份文件）
                EncodingMenuTarget::Blame => "blame-full-file-toggle",
            })
            .flex_none()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .when(active, |this| {
                this.bg(rgb(ui_theme::ACCENT))
                    .text_color(rgb(ui_theme::PRIMARY))
            })
            .when(!active, |this| {
                this.bg(rgb(ui_theme::ACCENT))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            })
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.toggle_full_file_view(cx);
                    cx.notify();
                }),
            )
            .child(label)
    }

    /// 切换全文视图开关并重新加载当前可见的差异。
    /// 缓存 key 包含 `full_file` 字段，因此紧凑/全文两套缓存互不污染，无需清空。
    pub(crate) fn toggle_full_file_view(&mut self, cx: &mut Context<Self>) {
        self.full_file_view = !self.full_file_view;
        self.status = if self.full_file_view {
            "已切换为全文视图".to_string()
        } else {
            "已切换为差异视图".to_string()
        };
        self.reload_visible_diffs_after_full_file_change();
        cx.notify();
    }

    /// 切换全文开关后扇出重新加载当前可见的三类差异（工作区/历史/贮藏）。
    fn reload_visible_diffs_after_full_file_change(&mut self) {
        if let Some(diff) = self.diff.clone() {
            self.load_diff(diff.path.clone(), diff.scope.clone());
        }
        if self.main_mode == MainMode::History
            && let Some(path) = self.history_selected_file.clone()
        {
            self.select_history_file_with_reload(path, true);
        }
        if self.main_mode == MainMode::Stash
            && let Some(path) = self.stash_preview.selected_file.clone()
        {
            self.select_stash_file(path, true);
        }
        if self.main_mode == MainMode::Browse
            && self.browse.view_mode == crate::BrowseViewMode::Diff
        {
            self.browse.diff = None;
            self.browse.diff_headers_expanded = false;
            self.load_browse_current();
        }
    }

    /// 全文视图加载因文件过大失败时，自动回退到紧凑差异视图。
    /// 在错误事件处理闭包末尾调用，检测 `last_error` 是否为全文过大错误。
    pub(crate) fn revert_full_file_if_too_large_error(&mut self) {
        let is_too_large = self
            .last_error
            .as_deref()
            .is_some_and(|err| err.contains(khaslana::FULL_FILE_TOO_LARGE_MESSAGE));
        if is_too_large && self.full_file_view {
            self.full_file_view = false;
            self.last_error = None;
            self.status = "文件过大，已回退到差异视图".to_string();
            self.reload_visible_diffs_after_full_file_change();
        }
    }
}
