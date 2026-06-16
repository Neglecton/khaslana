// 文本输入 action handler 模块：键盘/剪贴板动作分发。
//
// 从 main.rs 抽出的 text_backspace/delete/left/right/up/down/select_*/home/end/
// paste/copy/cut/submit 等 action handler，以及 submit_focused_field、
// notify_text_field_changed、focused_text_field、operation_blocker_allows_text_field、
// is_multiline_field。这些 handler 通过 on_action(cx.listener(Self::text_*)) 在
// main.rs 的渲染方法里注册，跨 impl 块解析。纯位置搬移，不改逻辑。

use gpui::{App, Context, Window};

use crate::{
    DialogState, FieldId, RepositoryView, TextBackspace, TextCopy, TextCut, TextDelete, TextDown,
    TextEnd, TextHome, TextLeft, TextPaste, TextRight, TextSelectAll, TextSelectDown,
    TextSelectLeft, TextSelectRight, TextSelectUp, TextSubmit, TextUp,
};

use gpui::ClipboardItem;

impl RepositoryView {
    pub(crate) fn submit_focused_field(&mut self, field: FieldId) {
        if matches!(field, FieldId::CommitMessage) {
            self.commit();
        } else if matches!(field, FieldId::ConflictEditor) {
            self.apply_selected_conflict_draft(false);
        } else if matches!(field, FieldId::CloneUrl | FieldId::ClonePath) {
            if self.active_dialog == Some(DialogState::CloneRepo) {
                self.clone_repo();
            }
        } else if matches!(field, FieldId::BranchName) {
            if self.active_dialog == Some(DialogState::CreateBranch) {
                self.create_branch();
            }
        } else if matches!(field, FieldId::BranchRename) {
            if let Some(DialogState::RenameBranch { branch }) = self.active_dialog.clone() {
                self.rename_branch(branch);
            }
        } else if matches!(field, FieldId::StashMessage) {
            if self.active_dialog == Some(DialogState::StashForm) {
                self.save_stash();
            }
        } else if matches!(field, FieldId::RemoteName | FieldId::RemoteUrl) {
            if let Some(DialogState::RemoteForm { editing }) = self.active_dialog.clone() {
                self.save_remote(editing);
            }
        } else if matches!(field, FieldId::RemoteBranchName) {
            if let Some(DialogState::RemoteBranchOperation { kind }) = self.active_dialog.clone() {
                self.confirm_remote_branch_operation(kind);
            }
        } else if matches!(field, FieldId::RemoteBranchSearch) {
            self.remote_branch_operation.branch_dropdown_open = false;
        } else if matches!(
            field,
            FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
        ) {
            if self.active_dialog == Some(DialogState::NetworkProxySettings) {
                self.save_network_proxy_settings();
            }
        } else if matches!(
            field,
            FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
        ) {
            if self.active_dialog == Some(DialogState::AiProviderSettings) {
                self.save_ai_provider_settings_from_form();
            }
        } else if matches!(
            field,
            FieldId::CredentialSecret
                | FieldId::CredentialPassphrase
                | FieldId::CredentialUsername
                | FieldId::CredentialKeyPath
                | FieldId::CredentialRemoteUrl
                | FieldId::CredentialDisplayName
        ) {
            if matches!(self.active_dialog, Some(DialogState::CredentialForm { .. })) {
                self.save_credential_form();
            } else {
                self.use_credentials();
            }
        }
    }

    pub(crate) fn notify_text_field_changed(&mut self, field: FieldId) {
        if matches!(field, FieldId::WorkflowInput(_)) {
            self.workflow_input_changed();
        }
    }

    pub(crate) fn focused_text_field(&self, window: &Window, cx: &App) -> Option<FieldId> {
        let field = self.focused_field(window, cx)?;
        if self.active_operation_blocker_message().is_some()
            && !self.operation_blocker_allows_text_field(field)
        {
            return None;
        }
        Some(field)
    }

    pub(crate) fn operation_blocker_allows_text_field(&self, field: FieldId) -> bool {
        self.pending_credential.is_some()
            && matches!(
                field,
                FieldId::CredentialUsername
                    | FieldId::CredentialSecret
                    | FieldId::CredentialKeyPath
                    | FieldId::CredentialPassphrase
            )
    }

    pub(crate) fn text_backspace(
        &mut self,
        _: &TextBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_backward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_delete(
        &mut self,
        _: &TextDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).delete_forward();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_left(&mut self, _: &TextLeft, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(false);
            cx.notify();
        }
    }

    pub(crate) fn text_right(
        &mut self,
        _: &TextRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(false);
            cx.notify();
        }
    }

    pub(crate) fn text_up(&mut self, _: &TextUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(-1, false);
            cx.notify();
        }
    }

    pub(crate) fn text_down(&mut self, _: &TextDown, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(1, false);
            cx.notify();
        }
    }

    pub(crate) fn text_select_left(
        &mut self,
        _: &TextSelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_left(true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_right(
        &mut self,
        _: &TextSelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).move_right(true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_up(
        &mut self,
        _: &TextSelectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(-1, true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_down(
        &mut self,
        _: &TextSelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx)
            && Self::is_multiline_field(field)
        {
            self.field_mut(field).move_vertical(1, true);
            cx.notify();
        }
    }

    pub(crate) fn text_select_all(
        &mut self,
        _: &TextSelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            self.field_mut(field).select_all();
            cx.notify();
        }
    }

    pub(crate) fn text_home(&mut self, _: &TextHome, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_start(false);
            } else {
                self.field_mut(field).move_caret_to(0, false);
            }
            cx.notify();
        }
    }

    pub(crate) fn text_end(&mut self, _: &TextEnd, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if Self::is_multiline_field(field) {
                self.field_mut(field).move_to_line_end(false);
            } else {
                let end = self.field(field).value.len();
                self.field_mut(field).move_caret_to(end, false);
            }
            cx.notify();
        }
    }

    pub(crate) fn text_paste(
        &mut self,
        _: &TextPaste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.field_mut(field).replace_text_in_utf16_range_with_mode(
                None,
                &text,
                Self::is_multiline_field(field),
            );
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_copy(&mut self, _: &TextCopy, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn text_cut(&mut self, _: &TextCut, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused_text_field(window, cx) else {
            return;
        };
        if let Some(text) = self.field(field).copyable_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.field_mut(field).delete_selection();
            self.notify_text_field_changed(field);
            cx.notify();
        }
    }

    pub(crate) fn text_submit(
        &mut self,
        _: &TextSubmit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_text_field(window, cx) {
            if field == FieldId::CommitMessage {
                self.commit();
                cx.notify();
            } else if field == FieldId::ConflictEditor {
                self.apply_selected_conflict_draft(false);
                cx.notify();
            } else if matches!(
                field,
                FieldId::ProxyHttpUrl | FieldId::ProxyHttpsUrl | FieldId::ProxySocks5Url
            ) && self.active_dialog == Some(DialogState::NetworkProxySettings)
            {
                self.save_network_proxy_settings();
                cx.notify();
            } else if matches!(
                field,
                FieldId::AiBaseUrl | FieldId::AiApiKey | FieldId::AiModel
            ) && self.active_dialog == Some(DialogState::AiProviderSettings)
            {
                self.save_ai_provider_settings_from_form();
                cx.notify();
            }
        }
    }

    pub(crate) fn is_multiline_field(id: FieldId) -> bool {
        matches!(id, FieldId::CommitMessage | FieldId::ConflictEditor)
    }
}
