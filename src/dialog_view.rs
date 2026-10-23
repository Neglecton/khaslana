//! RepositoryView 的应用级对话框渲染。

#[path = "ui_resources/dialog_view_messages.rs"]
mod messages;

use crate::*;
use gpui_kit::base::FocusTrapElement;
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::component::{Disableable, Sizable, Size, button::Button, menu::DropdownMenu};

/// 冻结操作列须容纳两个不可收缩的按钮，不能依靠溢出绘制侵入左侧数据栏。
/// 每个标签预留 48px（覆盖 12px 字号的中英文 Test / Delete），另计按钮两侧
/// 各 12px 内边距与 1px 边框、4px 按钮间距、行两侧各 8px 内边距。
const CREDENTIAL_TABLE_OP_COL_W: f32 = credential_actions_column_width(48.0, 48.0);

const fn credential_actions_column_width(test_label_width: f32, delete_label_width: f32) -> f32 {
    test_label_width + delete_label_width + 2.0 * (24.0 + 2.0) + 4.0 + 16.0
}
/// 凭据表数据列最小总宽：固定列 512px + 列间距 40px + 行内边距 16px +
/// 「站点 / 远端」列最小 120px；窄视口时经横向滚动查看完整内容。
const CREDENTIAL_TABLE_DATA_MIN_W: f32 = 688.0;
/// 凭据表「站点 / 远端」列最小宽（唯一弹性列的收缩下限）。
const CREDENTIAL_TABLE_TARGET_MIN_W: f32 = 120.0;
/// 凭据表行高：操作按钮（32px）撑起的高度；左右两栏拆分后行高必须
/// 固定一致，纵向滚动时行才能对齐。
const CREDENTIAL_ROW_H: f32 = 48.0;

impl RepositoryView {
    pub(crate) fn render_dialogs(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(dialog) = self.active_dialog.clone() else {
            return div().into_any_element();
        };
        // 防御旧状态：V2 在主页面展示，不绘制重复的模态编辑器。
        if matches!(dialog, DialogState::WorkflowEditor) && self.workflow_document_editor_visible() {
            return div().into_any_element();
        }

        let content = match dialog {
            DialogState::CloneRepo => self.render_clone_dialog(window, cx).into_any_element(),
            DialogState::CreateBranch => self
                .render_create_branch_dialog(window, cx)
                .into_any_element(),
            DialogState::CreateRecoveryBranch => self.render_recovery_branch_dialog(window, cx),
            DialogState::RenameBranch { branch } => self
                .render_rename_branch_dialog(branch, window, cx)
                .into_any_element(),
            DialogState::ConfirmReset { oid, summary, mode } => self
                .render_confirm_reset_dialog(oid, summary, mode, cx)
                .into_any_element(),
            DialogState::ConfirmRevert { oid, summary } => self
                .render_confirm_revert_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmRevertMerge { oid, summary } => self
                .render_confirm_revert_merge_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmUncommitToStaged { oid, summary } => self
                .render_confirm_uncommit_to_staged_dialog(oid, summary, cx)
                .into_any_element(),
            DialogState::ConfirmAmendPushed { and_push } => self
                .render_confirm_amend_pushed_dialog(and_push, cx)
                .into_any_element(),
            DialogState::TagForm {
                target_oid,
                target_summary,
            } => self
                .render_tag_form_dialog(target_oid, target_summary, window, cx)
                .into_any_element(),
            DialogState::TagPush { tag } => self
                .render_tag_push_dialog(tag, window, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteTag { tag } => self
                .render_confirm_delete_tag_dialog(tag, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemoteTag { remote, tag } => self
                .render_confirm_delete_remote_tag_dialog(remote, tag, cx)
                .into_any_element(),
            DialogState::ConfirmDiscardChange {
                scope,
                target,
                paths,
            } => self
                .render_confirm_discard_change_dialog(scope, target, paths, cx)
                .into_any_element(),
            DialogState::CredentialDetails { record_id } => self
                .render_credential_details_dialog(record_id, cx)
                .into_any_element(),
            DialogState::CredentialForm { editing } => self
                .render_credential_form_dialog(editing, window, cx)
                .into_any_element(),
            DialogState::TestCredential { record_id } => self
                .render_test_credential_dialog(record_id, window, cx)
                .into_any_element(),
            DialogState::GenerateSshKey => self.render_generate_ssh_key_dialog(window, cx),
            DialogState::DeleteSshKey { key_id, name } => self.render_delete_ssh_key_dialog(key_id, name, cx),
            DialogState::ForgetSshHost { host, port, algorithm } => self.render_forget_ssh_host_dialog(host, port, algorithm, cx),
            DialogState::VerifySshHost { challenge_id } => self.render_verify_ssh_host_dialog(challenge_id, cx),
            DialogState::SubmoduleManager => {
                self.render_submodule_manager_dialog(cx).into_any_element()
            }
            DialogState::RemoteManager => self
                .render_remote_manager_dialog(window, cx)
                .into_any_element(),
            DialogState::RemoteForm { editing } => self
                .render_remote_form_dialog(editing, window, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemote { name } => self
                .render_confirm_delete_remote_dialog(name, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteRemoteBranch { remote, branch } => self
                .render_confirm_delete_remote_branch_dialog(remote, branch, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteCredential { record_id, label } => self
                .render_confirm_delete_credential_dialog(record_id, label, cx)
                .into_any_element(),
            DialogState::StashForm => self.render_stash_form_dialog(window, cx).into_any_element(),
            DialogState::AiSkillImport => self.render_ai_skill_import_dialog(cx).into_any_element(),
            DialogState::AiSkillDetails { name } => self.render_ai_skill_details_dialog(&name, cx).into_any_element(),
            DialogState::AiSkillRemove { name } => self.render_ai_skill_remove_dialog(&name, cx).into_any_element(),
            DialogState::AiMcpForm => self.render_ai_mcp_form_dialog(window, cx).into_any_element(),
            DialogState::AiMcpBuiltinDetails => self.render_ai_mcp_builtin_details_dialog(cx).into_any_element(),
            DialogState::AiMcpRemove { name } => self.render_ai_mcp_remove_dialog(&name, cx).into_any_element(),
            DialogState::RemoteWorkflowTemplates => self
                .render_remote_workflow_template_dialog(window, cx)
                .into_any_element(),
            DialogState::ConfirmWorkflowExternal { tab_id, definition_generation, permissions } => self
                .render_workflow_external_confirm_dialog(tab_id, definition_generation, &permissions, cx)
                .into_any_element(),
            DialogState::WorkflowEditor => self
                .render_workflow_editor_dialog(window, cx)
                .into_any_element(),
            DialogState::ConfirmWorkflowEditComments => self
                .render_confirm_workflow_edit_comments(cx)
                .into_any_element(),
            DialogState::ConfirmDeleteWorkflowTemplate { path, display_name } => self
                .render_confirm_delete_workflow_template_dialog(path, display_name, cx)
                .into_any_element(),
            DialogState::ConfirmDeleteCodeIndex {
                repo_key,
                display_name,
            } => self
                .render_code_index_delete_confirm(&repo_key, &display_name, cx)
                .into_any_element(),
            DialogState::ConfirmDropStash { index, message } => self
                .render_confirm_drop_stash_dialog(index, message, cx)
                .into_any_element(),
            DialogState::ConfirmPopStash { index, message } => self
                .render_confirm_pop_stash_dialog(index, message, cx)
                .into_any_element(),
            DialogState::ConfirmCarryCheckout { target } => self
                .render_confirm_carry_checkout_dialog(&target, cx)
                .into_any_element(),
            DialogState::WorkflowShortcutBinding { file } => self
                .render_workflow_shortcut_binding_dialog(file.as_str(), cx)
                .into_any_element(),
            DialogState::RemoteBranchOperation { kind } => self
                .render_remote_branch_operation_dialog(kind, window, cx)
                .into_any_element(),
            DialogState::ConfirmConflictResolve => self
                .render_confirm_conflict_resolve_dialog(cx)
                .into_any_element(),
            DialogState::ConfirmAiConflictMerge { path } => self
                .render_confirm_ai_conflict_merge_dialog(path, cx)
                .into_any_element(),
            DialogState::ConfirmAbortMerge => self
                .render_confirm_abort_merge_dialog(cx)
                .into_any_element(),
            DialogState::ConfirmWindowClose => self
                .render_confirm_window_close_dialog(cx)
                .into_any_element(),
            // ── 更新对话框 ──
            DialogState::NewVersionAvailable {
                version,
                notes,
                published_at,
                size,
            } => self
                .render_new_version_dialog(&version, &notes, &published_at, size, cx)
                .into_any_element(),
            DialogState::ConfirmInstallUpdate { version } => self
                .render_confirm_install_dialog(&version, cx)
                .into_any_element(),
            DialogState::UpdateNoWritePermission { version } => self
                .render_no_write_permission_dialog(&version, cx)
                .into_any_element(),
            DialogState::PortableMigrationPrompt => {
                self.render_portable_migration_dialog(cx).into_any_element()
            }
            DialogState::ExeRelocationPrompt => {
                self.render_exe_relocation_dialog(cx).into_any_element()
            }
        };

        // 焦点圈：弹窗打开时焦点由 maintain_overlay_focus 移入其中，
        // Tab/Shift+Tab 在圈内循环，不会漏到遮罩下层的按钮与输入。
        dialog_overlay()
            .id("dialog-overlay")
            .focus_trap("dialog-overlay-trap", &self.dialog_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.close_credential_context_menu(cx);
                    cx.stop_propagation();
                }),
            )
            .child(content)
            .into_any_element()
    }

    fn render_clone_dialog(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = infer_clone_target_path(&self.clone_url.value, &self.clone_path.value)
            .map(|path| path.display().to_string());
        self.dialog_panel(khaslana::ui_i18n::text(messages::CLONE_REPOSITORY), cx)
            .child(self.input(FieldId::CloneUrl, false, window, cx))
            .child(self.input(FieldId::ClonePath, false, window, cx))
            .child(self.toggle_row(
                "clone-recursive-submodules",
                khaslana::ui_i18n::text(messages::CLONE_SUBMODULES_RECURSIVELY),
                self.clone_recursive_submodules,
                |this, _, _| this.clone_recursive_submodules = !this.clone_recursive_submodules,
                cx,
            ))
            .child(
                div()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(rgb(if preview.is_some() {
                        ui_theme::CONTENT_SECONDARY
                    } else {
                        ui_theme::CONTENT_SECONDARY
                    }))
                    .child(preview.unwrap_or_else(|| {
                        khaslana::ui_i18n::text(messages::ENTER_A_REMOTE_REPOSITORY_URL_AND_PARENT_FOLDER_TO_SEE).to_string()
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::SELECT_FOLDER),
                        !self.busy,
                        |this, _, _| this.browse_clone_target(),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                khaslana::ui_i18n::text(messages::CANCEL),
                                !self.busy,
                                |this, _, _| this.close_dialog(),
                                cx,
                            ))
                            .child(self.primary_button(
                                khaslana::ui_i18n::text(messages::CLONE),
                                !self.busy,
                                |this, _, _| this.clone_repo(),
                                cx,
                            )),
                    ),
            )
    }

    fn render_confirm_window_close_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CLOSE_KHASLANA), cx)
            .w(px(520.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::text(messages::EXIT_THE_APPLICATION_OR_KEEP_KHASLANA_RUNNING_IN_THE_SYSTEM)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::WHEN_MINIMIZED_TO_THE_TRAY_CLICK_THE_TRAY_ICON_TO)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), true, |this, _, _| this.cancel_window_close(), cx))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::MINIMIZE_TO_TRAY),
                        true,
                        |this, window, cx| this.minimize_to_tray(window, cx),
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::EXIT),
                        true,
                        |this, _, cx| this.exit_application(cx),
                        cx,
                    )),
            )
    }

    fn render_portable_migration_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::MIGRATE_TO_PORTABLE_DIRECTORY), cx)
            .w(px(540.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::text(messages::APPLICATION_DATA_IS_CURRENTLY_IN_A_SYSTEM_DIRECTORY_ON_DRIVE)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        khaslana::ui_i18n::text(messages::PORTABLE_MIGRATION_DETAILS),
                    ),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::KEEP_CURRENT_LOCATION),
                        true,
                        |this, _, _| this.dismiss_portable_migration(),
                        cx,
                    ))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::MIGRATE_AND_RESTART),
                        true,
                        |this, _, _| this.confirm_portable_migration(),
                        cx,
                    )),
            )
    }

    /// 程序位置风险搬迁弹窗：exe 位于临时/聊天软件接收/下载目录时建议
    /// 把程序与数据一起移到安全目录。文案区分风险级别，并说明数据当前
    /// 是否已在安全位置（新用户经解析规则直接落固定目录）。
    fn render_exe_relocation_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let risk = khaslana::current_exe_location_risk();
        let risk_text = match risk {
            khaslana::ExeLocationRisk::Volatile => {
                khaslana::ui_i18n::text(messages::VOLATILE_LOCATION_WARNING)
            }
            _ => {
                khaslana::ui_i18n::text(messages::DOWNLOAD_LOCATION_WARNING)
            }
        };
        let data_at_risk = khaslana::portable_database_path().is_some_and(|path| path.exists());
        let target_label = khaslana::exe_relocation_target_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::UNKNOWN).to_string());
        self.dialog_panel(khaslana::ui_i18n::text(messages::MOVE_TO_A_SAFE_DIRECTORY), cx)
            .w(px(540.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(risk_text),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::format(if data_at_risk { messages::MOVE_PROGRAM_AND_DATA } else { messages::MOVE_PROGRAM_ONLY }, &[("target_label", &target_label)])),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::KEEP_CURRENT_LOCATION),
                        true,
                        |this, _, _| this.dismiss_exe_relocation(),
                        cx,
                    ))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::MOVE_AND_RESTART),
                        true,
                        |this, _, _| this.confirm_exe_relocation(),
                        cx,
                    )),
            )
    }

    fn render_create_branch_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 基础分支只列本地分支：远端分支需先检出为本地分支（切换远端分支入口）。
        // 选中值在打开对话框时已预填为当前 HEAD 分支（分离 HEAD 时为空）。
        let branches = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .branches
                    .iter()
                    .filter(|branch| branch.kind == BranchKind::Local)
                    .map(|branch| branch.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let selected_base = self.create_branch_base.clone();
        let disabled = self.busy;
        self.dialog_panel(khaslana::ui_i18n::text(messages::CREATE_BRANCH), cx)
            .child(self.input(FieldId::BranchName, false, window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(khaslana::ui_i18n::text(messages::BASE_BRANCH)),
                    )
                    .child(
                        Button::new("create-branch-base-select")
                            .label(
                                selected_base
                                    .clone()
                                    .unwrap_or_else(|| khaslana::ui_i18n::text(messages::CURRENT_HEAD).to_string()),
                            )
                            .accessibility_label(khaslana::ui_i18n::text(messages::SELECT_BASE_BRANCH))
                            .outline()
                            .dropdown_caret(true)
                            .disabled(disabled)
                            // 控件行以 12px 正文为基准，按钮取最小档对齐输入框字号。
                            .with_size(Size::XSmall)
                            .w_full()
                            .h(px(34.0))
                            .dropdown_menu(move |menu, _window, _cx| {
                                // 对话框面板固定 480px、四周 16px 内边距，触发器
                                // 占满内容行（448px）；菜单按内容自适应，设同宽
                                // 下限与触发器对齐。
                                let menu = menu.min_w(px(448.0)).scrollable(true).max_h(px(240.0));
                                branches.iter().fold(menu, |menu, branch| {
                                    let selected =
                                        selected_base.as_deref() == Some(branch.as_str());
                                    menu.menu_with_check(
                                        branch.clone(),
                                        selected,
                                        Box::new(SelectCreateBranchBase {
                                            branch: Some(branch.clone()),
                                        }),
                                    )
                                })
                            }),
                    ),
            )
            .child(self.toggle_row(
                "create-branch-checkout",
                khaslana::ui_i18n::text(messages::SWITCH_TO_THE_NEW_BRANCH_AFTER_CREATION),
                self.create_branch_checkout,
                |this, _, _| this.create_branch_checkout = !this.create_branch_checkout,
                cx,
            ))
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::CREATE),
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.create_branch(),
                        cx,
                    )),
            )
    }

    fn render_rename_branch_dialog(
        &self,
        branch: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::RENAME_BRANCH), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::format(messages::CURRENT_BRANCH_BRANCH, &[("branch", &(branch).to_string())])),
            )
            .child(self.input(FieldId::BranchRename, false, window, cx))
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::RENAME),
                        !self.busy,
                        {
                            let branch = branch.clone();
                            move |this, _, _| this.rename_branch(branch.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_reset_dialog(
        &self,
        oid: String,
        summary: String,
        mode: ResetMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CONFIRM_BRANCH_RESET), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TARGET_COMMIT_OID_SUMMARY, &[("oid", &(short_oid(&oid)).to_string()), ("summary", &(summary).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(match mode { ResetMode::Soft => messages::RESET_SOFT_DESCRIPTION, ResetMode::Mixed => messages::RESET_MIXED_DESCRIPTION, ResetMode::Hard => messages::RESET_HARD_DESCRIPTION })),
            )
            .when(mode == ResetMode::Hard, |this| {
                this.child(danger_callout(
                    khaslana::ui_i18n::text(messages::A_HARD_RESET_MOVES_THE_CURRENT_BRANCH_AND_REMOVES_LATER),
                ))
            })
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_RESET),
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.reset_to_commit(oid.clone(), mode)
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_revert_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CONFIRM_REVERT_COMMIT), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TARGET_COMMIT_OID_SUMMARY, &[("oid", &(short_oid(&oid)).to_string()), ("summary", &(summary).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_CREATES_A_NEW_COMMIT_THAT_REVERTS_THE_CHANGES_INTRODUCED)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_REVERT),
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.revert_commit(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_revert_merge_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CONFIRM_REVERT_MERGE_COMMIT), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TARGET_COMMIT_OID_SUMMARY, &[("oid", &(short_oid(&oid)).to_string()), ("summary", &(summary).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_CREATES_A_NEW_COMMIT_THAT_REVERTS_THE_CHANGES_INTRODUCED_2)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THE_ORIGINAL_MERGE_COMMIT_IS_RETAINED_AND_HISTORY_IS_NOT)),
            )
            .child(danger_callout(
                khaslana::ui_i18n::text(messages::THIS_IS_EQUIVALENT_TO_GIT_REVERT_M_PRESERVING_THE_FIRST),
            ))
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_REVERT_MERGE),
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.revert_merge_commit(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_uncommit_to_staged_dialog(
        &self,
        oid: String,
        summary: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CONFIRM_UNDO_TO_STAGING), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TARGET_COMMIT_OID_SUMMARY, &[("oid", &(short_oid(&oid)).to_string()), ("summary", &(summary).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_REMOVES_THE_COMMIT_AND_KEEPS_ITS_CHANGES_IN_THE)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::ONLY_THE_LATEST_UNPUSHED_NON_MERGE_COMMIT_ON_THE_CURRENT)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_UNDO),
                        !self.busy,
                        {
                            let oid = oid.clone();
                            move |this, _, _| this.uncommit_to_staged(oid.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_amend_pushed_dialog(
        &self,
        and_push: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::AMEND_A_PUSHED_COMMIT), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::text(messages::THE_LATEST_COMMIT_HAS_ALREADY_BEEN_PUSHED_TO_THE_REMOTE)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::AMENDING_REWRITES_THIS_COMMIT_UPDATING_THE_REMOTE_THEN_REQUIRES_A)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::AMEND_ONLY_COMMITS_THAT_HAVE_NOT_BEEN_PUSHED)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        if and_push { khaslana::ui_i18n::text(messages::AMEND_AND_PUSH_ANYWAY) } else { khaslana::ui_i18n::text(messages::AMEND_ANYWAY) },
                        !self.busy,
                        move |this, _, _| {
                            let message = this.commit_message.value.trim().to_string();
                            if and_push {
                                let Some(remote) = this.current_remote() else {
                                    this.last_error = Some(khaslana::ui_i18n::encode(messages::THIS_REPOSITORY_HAS_NO_REMOTE, &[]));
                                    return;
                                };
                                this.perform_amend_and_push(message, remote);
                            } else {
                                this.perform_amend(message);
                            }
                        },
                        cx,
                    )),
            )
    }

    /// 创建标签对话框：名称 + 附注开关 + 附注信息（多行）+ 目标提交展示。
    fn render_tag_form_dialog(
        &self,
        target_oid: Option<String>,
        target_summary: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target_label = match (&target_oid, &target_summary) {
            (Some(oid), summary) => {
                khaslana::ui_i18n::format(messages::TARGET_COMMIT_OID_SUMMARY, &[("oid", &(short_oid(oid)).to_string()), ("summary", &(summary).to_string())])
            }
            (None, _) => khaslana::ui_i18n::text(messages::TARGET_COMMIT_HEAD_LATEST_COMMIT_ON_THE_CURRENT_BRANCH).to_string(),
        };
        self.dialog_panel(khaslana::ui_i18n::text(messages::CREATE_TAG), cx)
            .child(self.input(FieldId::TagName, false, window, cx))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(target_label),
            )
            .child(self.toggle_row(
                "tag-annotated-toggle",
                khaslana::ui_i18n::text(messages::CREATE_ANNOTATED_TAG_RECORDS_MESSAGE_AND_CREATOR_RECOMMENDED_FOR_RELEASES),
                self.tag_annotated,
                |this, _, _| this.tag_annotated = !this.tag_annotated,
                cx,
            ))
            .when(self.tag_annotated, |this| {
                this.child(self.input(FieldId::TagMessage, false, window, cx))
            })
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::CREATE),
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.create_tag(),
                        cx,
                    )),
            )
    }

    /// 推送标签对话框：选择远端后推送。
    fn render_tag_push_dialog(
        &self,
        tag: String,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        let selected_remote = self
            .tag_push_remote
            .clone()
            .or_else(|| remotes.first().map(|remote| remote.name.clone()));
        let disabled = remotes.is_empty() || self.busy;
        let trigger_label = selected_remote
            .clone()
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::SELECT_REMOTE).to_string());
        let menu_remotes = remotes.clone();
        self.dialog_panel(khaslana::ui_i18n::text(messages::PUSH_TAG), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TAG_TAG, &[("tag", &(tag).to_string())])),
            )
            .child(
                Button::new("tag-push-remote-select")
                    .label(trigger_label)
                    .accessibility_label(khaslana::ui_i18n::text(messages::SELECT_REMOTE))
                    .outline()
                    .dropdown_caret(true)
                    .disabled(disabled)
                    .w_full()
                    .h(px(34.0))
                    .dropdown_menu(move |menu, _window, _cx| {
                        menu_remotes.iter().fold(
                            menu.scrollable(true).max_h(px(240.0)),
                            |menu, remote| {
                                menu.menu_with_check(
                                    remote.name.clone(),
                                    selected_remote.as_deref() == Some(remote.name.as_str()),
                                    Box::new(SelectTagPushRemote {
                                        remote: remote.name.clone(),
                                    }),
                                )
                            },
                        )
                    }),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::PUSH),
                        !remotes.is_empty() && !self.busy,
                        {
                            let tag = tag.clone();
                            move |this, _, _| this.push_tag(tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_tag_dialog(
        &self,
        tag: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_TAG), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::TAG_TAG, &[("tag", &(tag).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_DELETES_THE_LOCAL_TAG_ONLY_REMOTE_TAGS_ARE_NOT)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_DELETION),
                        !self.busy,
                        {
                            let tag = tag.clone();
                            move |this, _, _| this.delete_tag(tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_remote_tag_dialog(
        &self,
        remote: String,
        tag: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_REMOTE_TAG), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::REMOTE_TAG_REMOTE_TAG, &[("remote", &(remote).to_string()), ("tag", &(tag).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_DELETES_THE_TAG_FROM_THE_REMOTE_PUBLISHED_VERSION_REFERENCES)),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_DELETION),
                        !self.busy,
                        {
                            let remote = remote.clone();
                            let tag = tag.clone();
                            move |this, _, _| this.delete_remote_tag(remote.clone(), tag.clone())
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_discard_change_dialog(
        &self,
        scope: DiffScope,
        target: DiscardTarget,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = paths.len();
        let target_label = match target {
            DiscardTarget::Single => khaslana::ui_i18n::text(messages::TARGET_FILE).to_string(),
            DiscardTarget::Selected => khaslana::ui_i18n::format(messages::SELECTED_FILES_COUNT, &[("count", &(count).to_string())]),
            DiscardTarget::All => match scope {
                DiffScope::Staged => khaslana::ui_i18n::format(messages::ALL_STAGED_FILES_COUNT, &[("count", &(count).to_string())]),
                DiffScope::Unstaged => khaslana::ui_i18n::format(messages::ALL_UNSTAGED_FILES_COUNT, &[("count", &(count).to_string())]),
            },
        };
        let preview = discard_paths_preview(&paths);
        let help = match scope {
            DiffScope::Staged => {
                khaslana::ui_i18n::text(messages::DISCARD_ALL_UNCOMMITTED_CHANGES_IN_THESE_FILES_BOTH_STAGED_AND)
            }
            DiffScope::Unstaged => {
                khaslana::ui_i18n::text(messages::DISCARD_ONLY_UNSTAGED_CHANGES_IN_THESE_FILES_STAGED_CHANGES_ARE)
            }
        };
        self.dialog_panel(khaslana::ui_i18n::text(messages::CONFIRM_DISCARD_CHANGES), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(target_label),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(preview),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(help),
            )
            .child(
                dialog_actions()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_REVERT),
                        !self.busy,
                        {
                            let paths = paths.clone();
                            move |this, _, _| {
                                this.discard_change(paths.clone(), scope.clone(), target.clone())
                            }
                        },
                        cx,
                    )),
            )
    }

    /// AI 合并建议覆盖确认：草稿已有块处理或手工编辑时，确认后才生成并覆盖。
    fn render_confirm_ai_conflict_merge_dialog(
        &self,
        path: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::OVERWRITE_EXISTING_CONFLICT_RESOLUTION), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(path.clone()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_DRAFT_ALREADY_CONTAINS_BLOCK_RESOLUTIONS_OR_MANUAL_EDITS_GENERATING)),
            )
            .child(danger_callout(
                khaslana::ui_i18n::text(messages::ACCEPTED_BLOCKS_IGNORED_BLOCKS_AND_MANUAL_EDITS_WILL_BE_LOST),
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::GO_BACK_AND_KEEP_CHANGES),
                        !self.busy,
                        |this, _, _| this.close_dialog(),
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::OVERWRITE_AND_GENERATE),
                        !self.busy,
                        move |this, _, _| {
                            this.close_dialog();
                            this.start_ai_conflict_merge(path.clone());
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_conflict_resolve_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.conflict_workbench.pending_resolve.clone();
        let unresolved_count = pending
            .as_ref()
            .map(|item| item.unresolved_count)
            .unwrap_or(0);
        let path = pending
            .as_ref()
            .map(|item| item.path.clone())
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::CURRENT_CONFLICT_FILE).to_string());

        self.dialog_panel(khaslana::ui_i18n::text(messages::UNRESOLVED_BLOCKS_REMAIN), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(path),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::format(messages::UNRESOLVED_COUNT_BLOCKS_ARE_STILL_UNRESOLVED_MARK_THIS_FILE_AS, &[("unresolved_count", &(unresolved_count).to_string())])),
            )
            .child(danger_callout(
                khaslana::ui_i18n::text(messages::THE_CURRENT_RESULT_WILL_BE_WRITTEN_TO_THE_WORKING_TREE),
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::GO_BACK_TO_RESOLVING),
                        !self.busy,
                        |this, _, _| this.cancel_pending_conflict_resolve(),
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::RESOLVE_ANYWAY),
                        !self.busy,
                        |this, _, _| this.confirm_pending_conflict_resolve(),
                        cx,
                    )),
            )
    }

    // ── 更新对话框渲染 ──────────────────────────────────────────────────

    /// 更新设置页的分组（Kit `SettingGroup` 列表）。
    ///
    /// 两组：「更新」承载版本信息、渠道开关与检查动作；「渠道与迁移」承载
    /// 新版本卡片与数据目录 / 程序位置入口。开关走 `SettingField::switch`
    /// （内部即 Kit `Switch`）：值闭包读 `update_preferences`，写回闭包落状态
    /// 并 `save_update_preferences()`；新版本卡片、目录入口与按钮组是复合
    /// 自绘内容，走 `render` 通道原样保留。
    ///
    /// `Entity` 非 `Copy`：每个 `move` 闭包各自 `cx.entity()` 取一份句柄，
    /// 多个闭包不能共用同一份（迁移样板曾在此报 use of moved value）。
    ///
    /// 主线把本函数接入 `settings_pane_groups` 前暂无调用方。
    #[allow(dead_code)]
    pub(crate) fn settings_update_groups(&self, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let version = env!("CARGO_PKG_VERSION");
        // 新版本卡片数据：版本号 / 发布时间 / 版本说明 / 包大小。
        let available_update_display = self.available_update.as_ref().map(|manifest| {
            let size = manifest
                .platforms
                .get("windows-x86_64")
                .map(|asset| format_byte_size(asset.size))
                .unwrap_or_default();
            let notes = if manifest.notes.trim().is_empty() {
                khaslana::ui_i18n::text(messages::NO_RELEASE_NOTES_FOR_THIS_VERSION).to_string()
            } else {
                manifest.notes.clone()
            };
            (
                manifest.version.clone(),
                manifest.published_at.clone(),
                notes,
                size,
            )
        });
        // 仅当存在可迁移的旧库时，在更新设置中常驻「迁移到便携目录」入口；
        // dismiss 标记只抑制启动时的自动弹窗，不影响此处手动入口。
        let migration_available = self.portable_migration_available();
        let current_db_label = khaslana::default_database_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::UNKNOWN).to_string());
        // 程序位于临时/聊天软件接收/下载目录时，常驻「移动到安全目录」入口；
        // dismiss 标记只抑制启动时的自动弹窗，不影响此处手动入口。
        let relocation_available = self.exe_relocation_available();
        // 迁移入口的闭包会 move current_db_label，这里单独克隆一份。
        let relocation_db_label = current_db_label.clone();
        let skipped_label = self
            .update_preferences
            .skipped_version
            .clone()
            .map(|v| format!("v{v}"))
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::NONE).to_string());

        let update_group = SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                khaslana::ui_i18n::text(messages::UPDATES),
                Some(khaslana::ui_i18n::text(messages::AUTOMATIC_CHECKS_AT_STARTUP_AND_MANUAL_CHECKS_SKIPPED_VERSIONS_ONLY).into()),
            ))
            .item(crate::settings_center::settings_item_row(
                khaslana::ui_i18n::text(messages::CURRENT_VERSION),
                None,
                {
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .text_size(px(ui_theme::TYPE_BODY))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(format!("v{version}"))
                        })
                    }
                },
            ))
            .item(crate::settings_center::settings_switch_row(
                khaslana::ui_i18n::text(messages::CHECK_FOR_UPDATES_AUTOMATICALLY),
                None,
                {
                    let view = cx.entity();
                    move |cx: &App| view.read(cx).update_preferences.auto_check
                },
                {
                    let view = cx.entity();
                    move |value: bool, cx: &mut App| {
                        view.update(cx, |this, cx| {
                            this.update_preferences.auto_check = value;
                            this.save_update_preferences();
                            cx.notify();
                        })
                    }
                },
            ))
            // 测试版（Beta）更新渠道：勾选后检测/安装所有版本（含预发布），
            // 未勾选只走正式版清单（与旧版本行为一致）。切换后下次检查生效。
            .item(crate::settings_center::settings_switch_row(
                khaslana::ui_i18n::text(messages::RECEIVE_BETA_UPDATES),
                Some(khaslana::ui_i18n::text(messages::ALSO_CHECK_FOR_AND_INSTALL_BETA_RELEASES_THEY_MAY_BE)),
                {
                    let view = cx.entity();
                    move |cx: &App| view.read(cx).update_preferences.include_beta
                },
                {
                    let view = cx.entity();
                    move |value: bool, cx: &mut App| {
                        view.update(cx, |this, cx| {
                            this.update_preferences.include_beta = value;
                            this.save_update_preferences();
                            cx.notify();
                        })
                    }
                },
            ))
            .item(crate::settings_center::settings_item_row(
                khaslana::ui_i18n::text(messages::SKIPPED_VERSION),
                None,
                {
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .text_size(px(ui_theme::TYPE_BODY))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(skipped_label.clone())
                        })
                    }
                },
            ))
            .item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        dialog_actions()
                            .child(this.primary_button(
                                khaslana::ui_i18n::text(messages::CHECK_NOW),
                                !this.update_checking && !this.busy,
                                |this, _, _| this.start_update_check(UpdateCheckTrigger::Manual),
                                cx,
                            ))
                            .child(this.button(
                                khaslana::ui_i18n::text(messages::CLEAR_SKIPPED_VERSION),
                                this.update_preferences.skipped_version.is_some(),
                                |this, _, _| this.clear_skipped_version(),
                                cx,
                            ))
                    })
                }
            }));

        let mut channel_group = SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                khaslana::ui_i18n::text(messages::CHANNELS_AND_MIGRATION),
                Some(khaslana::ui_i18n::text(messages::PENDING_UPDATES_DATA_DIRECTORY_MIGRATION_AND_APPLICATION_RELOCATION).into()),
            ));
        // 检测到新版本：页内常驻展示版本信息 + 版本说明 + 立即更新入口。
        if let Some((update_version, update_published_at, update_notes, update_size)) =
            available_update_display
        {
            channel_group = channel_group.item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        div()
                            .id("available-update-card")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p(px(ui_theme::SPACE_3))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .border_1()
                            .border_color(rgb(ui_theme::PRIMARY))
                            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .text_size(px(12.0))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(ui_theme::PRIMARY))
                                            .child(khaslana::ui_i18n::format(messages::NEW_VERSION_V_UPDATE_VERSION_AVAILABLE, &[("update_version", &(update_version).to_string())])),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                            .child(khaslana::ui_i18n::format(messages::RELEASED_UPDATE_PUBLISHED_AT, &[("update_published_at", &(update_published_at).to_string())]))
                                            .child(khaslana::ui_i18n::format(messages::PACKAGE_SIZE_UPDATE_SIZE, &[("update_size", &(update_size).to_string())])),
                                    ),
                            )
                            .child(
                                div()
                                    .id("available-update-notes")
                                    .max_h(px(180.0))
                                    .overflow_y_scroll()
                                    .text_size(px(12.0))
                                    .line_height(px(18.0))
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(crate::markdown_view::render_markdown(
                                        &update_notes,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(this.primary_button(
                                        khaslana::ui_i18n::text(messages::UPDATE_NOW),
                                        !this.update_downloading,
                                        |this, _window, cx| {
                                            this.start_update_download();
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .when(this.update_downloading, |card| {
                                        card.child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                                .child(
                                                    this.update_download_progress
                                                        .clone()
                                                        .unwrap_or_else(|| khaslana::ui_i18n::text(messages::PREPARING_DOWNLOAD).into()),
                                                ),
                                        )
                                    }),
                            )
                    })
                }
            }));
        }
        if migration_available {
            channel_group = channel_group.item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.0))
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(khaslana::ui_i18n::text(messages::DATA_DIRECTORY)),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(khaslana::ui_i18n::format(messages::CURRENT_CURRENT_DB_LABEL, &[("current_db_label", &(current_db_label).to_string())])),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(
                                        khaslana::ui_i18n::text(messages::MOVE_DATA_FROM_THE_SYSTEM_DIRECTORY_ON_DRIVE_C_TO),
                                    ),
                            )
                            .child(this.primary_button(
                                khaslana::ui_i18n::text(messages::MIGRATE_TO_PORTABLE_DIRECTORY),
                                true,
                                |this, _, _| {
                                    this.active_dialog = Some(DialogState::PortableMigrationPrompt);
                                },
                                cx,
                            ))
                    })
                }
            }));
        }
        if relocation_available {
            channel_group = channel_group.item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.0))
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                    .child(khaslana::ui_i18n::text(messages::APPLICATION_LOCATION)),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(khaslana::ui_i18n::format(messages::CURRENT_RELOCATION_DB_LABEL, &[("relocation_db_label", &(relocation_db_label).to_string())])),
                            )
                            .child(
                                div().text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(
                                    khaslana::ui_i18n::text(messages::RELOCATION_DETAILS),
                                ),
                            )
                            .child(this.primary_button(
                                khaslana::ui_i18n::text(messages::MOVE_TO_A_SAFE_DIRECTORY),
                                true,
                                |this, _, _| {
                                    this.active_dialog = Some(DialogState::ExeRelocationPrompt);
                                },
                                cx,
                            ))
                    })
                }
            }));
        }

        vec![update_group, channel_group]
    }

    fn render_new_version_dialog(
        &self,
        version: &str,
        notes: &str,
        published_at: &str,
        size: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let size_mb = size as f64 / 1_048_576.0;
        let version_owned = version.to_string();

        self.dialog_panel(khaslana::ui_i18n::format(messages::NEW_VERSION_V_VERSION_AVAILABLE, &[("version", &(version).to_string())]), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::format(messages::RELEASED_PUBLISHED_AT, &[("published_at", &(published_at).to_string())])),
            )
            .child(
                // 版本说明：Markdown 渲染，多行可滚动（内容多时可翻看）。
                div()
                    .id("new-version-notes")
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(crate::markdown_view::render_markdown(notes)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::format(messages::PACKAGE_SIZE_SIZE_MB_MB, &[("size_mb", &format!("{:.1}", size_mb))])),
            )
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::UPDATE_NOW),
                        !self.update_downloading && !self.busy,
                        |this, _, _| {
                            this.active_dialog = None;
                            this.start_update_download();
                        },
                        cx,
                    ))
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::SKIP_THIS_VERSION),
                        !self.update_downloading,
                        move |this, _, _| this.skip_version(&version_owned),
                        cx,
                    ))
                    .child(self.button(khaslana::ui_i18n::text(messages::LATER), true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_confirm_install_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staging_dir = self.staging_dir_for_install.clone();
        let version_owned = version.to_string();

        self.dialog_panel(khaslana::ui_i18n::text(messages::UPDATE_READY), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::VERSION_V_VERSION_HAS_BEEN_DOWNLOADED_AND_VERIFIED_THE_APPLICATION, &[("version", &(version).to_string())])),
            )
            .child(danger_callout(
                khaslana::ui_i18n::text(messages::THE_APPLICATION_EXITS_AND_RESTARTS_DURING_INSTALLATION_SAVE_YOUR_WORK),
            ))
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::RESTART_NOW),
                        true,
                        move |this, _, cx| {
                            if let Some(dir) = staging_dir.clone() {
                                this.install_update(&dir, &version_owned, cx);
                            } else {
                                this.update_error = Some(khaslana::ui_i18n::encode(messages::THE_STAGING_DIRECTORY_IS_MISSING, &[]));
                            }
                        },
                        cx,
                    ))
                    .child(self.button(khaslana::ui_i18n::text(messages::LATER), true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_no_write_permission_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::CANNOT_UPDATE_AUTOMATICALLY), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::THE_CURRENT_DIRECTORY_IS_NOT_WRITABLE_SO_VERSION_V_VERSION, &[("version", &(version).to_string())])),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::OPEN_CNB_DOWNLOADS),
                        true,
                        |_, _, _| {
                            open_url("https://cnb.cool/suhoan/khaslana-release");
                        },
                        cx,
                    ))
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::OPEN_GITHUB_RELEASE),
                        true,
                        |_, _, _| {
                            open_url("https://github.com/FuturePrayer/khaslana/releases");
                        },
                        cx,
                    )),
            )
            .child(dialog_actions().child(self.button(
                khaslana::ui_i18n::text(messages::CLOSE),
                true,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }

    fn render_remote_manager_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        let rows = if remotes.is_empty() {
            vec![placeholder_row(khaslana::ui_i18n::text(messages::NO_REMOTES_YET_CLICK_ADD_REMOTE_TO_CREATE_ONE)).into_any_element()]
        } else {
            remotes
                .into_iter()
                .map(|remote| self.remote_manager_row(remote, cx).into_any_element())
                .collect::<Vec<_>>()
        };
        // 面板尺寸按视口钳制（审查 R3）：820×620 在最小窗/高 DPI 下越界。
        let (panel_width, panel_max_height) = dialog_panel_size(window, 820.0, 620.0);

        div()
            .id("dialog-远端管理")
            .w(panel_width)
            .max_h(panel_max_height)
            .p_4()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .cursor(CursorStyle::Arrow)
            .occlude()
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                if this.mouse_down_inside_context_menu(event) {
                    return;
                }
                if this.credential_context_menu.is_some() {
                    this.credential_context_menu = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(khaslana::ui_i18n::text(messages::MANAGE_REMOTES)),
                    )
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::ADD_REMOTE),
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.open_remote_form(None),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THE_REMOTE_ADDRESS_IS_USED_FOR_BOTH_FETCH_AND_PUSH)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h(px(0.0))
                    .max_h(px(420.0))
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER_MUTED))
                    .rounded_sm()
                    .child(self.remote_manager_header())
                    .child({
                        let handle = self.scroll_handle("remote-manager-list");
                        let content = div()
                            .id("remote-manager-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_0()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&handle)
                            .children(rows)
                            .into_any_element();
                        scrollable_frame_when(
                            "remote-manager-list",
                            ScrollbarMode::Vertical,
                            content,
                            handle,
                            self.snapshot
                                .as_ref()
                                .is_some_and(|snapshot| !snapshot.remotes.is_empty()),
                            cx,
                        )
                    }),
            )
            .child(div().flex().justify_end().child(self.button(
                khaslana::ui_i18n::text(messages::CLOSE),
                !self.busy,
                |this, _, _| this.close_dialog(),
                cx,
            )))
    }

    fn remote_manager_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
            .child(div().flex_none().w(px(104.0)).child(khaslana::ui_i18n::text(messages::NAME)))
            .child(div().flex_1().min_w(px(0.0)).child(khaslana::ui_i18n::text(messages::ADDRESS)))
            .child(div().flex_none().w(px(180.0)).child(khaslana::ui_i18n::text(messages::CREDENTIALS)))
            .child(div().flex_none().w(px(106.0)).child(khaslana::ui_i18n::text(messages::ACTIONS)))
    }

    fn remote_manager_row(&self, remote: RemoteInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let edit_name = remote.name.clone();
        let delete_name = remote.name.clone();
        let policy = self
            .repo_path
            .as_ref()
            .map(|repo_path| {
                self.remote_credential_policy_for_remote(repo_path, &remote.name, &remote.url)
            })
            .unwrap_or(RemoteCredentialPolicy::AutoMatch);
        let credential_label = match policy {
            RemoteCredentialPolicy::NoCredential => khaslana::ui_i18n::text(messages::NO_CREDENTIALS).to_string(),
            RemoteCredentialPolicy::Record(record_id) => self
                .credential_records
                .iter()
                .find(|record| record.id == record_id)
                .map(credential_record_label)
                .unwrap_or_else(|| khaslana::ui_i18n::text(messages::CREDENTIAL_RECORD_MISSING).to_string()),
            RemoteCredentialPolicy::AutoMatch => self
                .matching_credential_for_remote_url(&remote.url)
                .map(|record| khaslana::ui_i18n::format(messages::AUTOMATIC_RECORD, &[("record", &(credential_record_label(record)).to_string())]))
                .unwrap_or_else(|| khaslana::ui_i18n::text(messages::AUTOMATIC_MATCH).to_string()),
        };

        div()
            .id(format!("remote-manager-row-{}", remote.name))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::WB_PANEL))
            .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
            .child(
                div()
                    .flex_none()
                    .w(px(104.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .truncate()
                    .child(remote.name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .truncate()
                    .child(remote.url),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(180.0))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(credential_label),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(106.0))
                    .flex()
                    .gap_1()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::EDIT),
                        !self.busy,
                        move |this, _, _| this.open_remote_form(Some(edit_name.clone())),
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::DELETE),
                        !self.busy,
                        move |this, _, _| this.open_delete_remote_confirm(delete_name.clone()),
                        cx,
                    )),
            )
    }

    fn render_remote_form_dialog(
        &self,
        editing: Option<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = if editing.is_some() {
            khaslana::ui_i18n::text(messages::EDIT_REMOTE)
        } else {
            khaslana::ui_i18n::text(messages::ADD_REMOTE)
        };
        self.dialog_panel(title, cx)
            .w(px(560.0))
            .child(self.input(FieldId::RemoteName, false, window, cx))
            .child(self.input(FieldId::RemoteUrl, false, window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(khaslana::ui_i18n::text(messages::BIND_CREDENTIALS)),
                    )
                    .child(self.remote_credential_picker(cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::CANCEL),
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::SAVE),
                        !self.busy,
                        move |this, _, _| this.save_remote(editing.clone()),
                        cx,
                    )),
            )
    }

    fn remote_credential_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.remote_url.value.trim().to_string();
        let mut rows = Vec::new();
        rows.push(
            self.remote_credential_option(
                RemoteCredentialPolicy::AutoMatch,
                khaslana::ui_i18n::text(messages::AUTOMATICALLY_MATCH_SAVED_CREDENTIALS).to_string(),
                true,
                cx,
            )
            .into_any_element(),
        );
        rows.push(
            self.remote_credential_option(
                RemoteCredentialPolicy::NoCredential,
                khaslana::ui_i18n::text(messages::NO_CREDENTIALS).to_string(),
                true,
                cx,
            )
            .into_any_element(),
        );
        rows.extend(
            self.credential_records
                .iter()
                .cloned()
                .map(|record| {
                    let compatible = if url.is_empty() {
                        true
                    } else {
                        match record.scope {
                            CredentialScope::RemoteUrl => {
                                credential_record_is_compatible_with_url(&record, &url)
                            }
                            CredentialScope::Host => {
                                credential_record_matches_remote_url(&record, &url)
                            }
                        }
                    };
                    let mut label = credential_record_label(&record);
                    if record.scope == CredentialScope::Host {
                        label = format!("{label} ({})", credential_display_target(&record));
                    }
                    if !compatible {
                        label.push_str(khaslana::ui_i18n::text(messages::NOT_A_MATCH));
                    }
                    self.remote_credential_option(
                        RemoteCredentialPolicy::Record(record.id),
                        label,
                        compatible,
                        cx,
                    )
                    .into_any_element()
                })
                .collect::<Vec<_>>(),
        );

        div()
            .flex()
            .flex_col()
            .max_h(px(168.0))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .rounded_sm()
            .bg(rgb(ui_theme::WB_PANEL))
            .children(rows)
    }

    fn remote_credential_option(
        &self,
        policy: RemoteCredentialPolicy,
        label: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.remote_credential_policy == policy;
        let id_label = match &policy {
            RemoteCredentialPolicy::AutoMatch => "auto",
            RemoteCredentialPolicy::NoCredential => "none",
            RemoteCredentialPolicy::Record(record_id) => record_id.as_str(),
        };
        div()
            .id(format!("remote-credential-option-{id_label}"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(if selected {
                rgb(ui_theme::PRIMARY_SUBTLE)
            } else {
                rgb(ui_theme::WB_PANEL)
            })
            .text_size(px(12.0))
            .text_color(if !enabled {
                rgb(ui_theme::CONTENT_SECONDARY)
            } else if selected {
                rgb(ui_theme::PRIMARY)
            } else {
                rgb(ui_theme::CONTENT_PRIMARY)
            })
            .cursor_pointer()
            .when(enabled, |this| {
                this.hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
            })
            .child(
                div()
                    .flex_none()
                    .size(px(10.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::BORDER_MUTED)
                    })
                    .bg(if selected {
                        rgb(ui_theme::PRIMARY)
                    } else {
                        rgb(ui_theme::WB_PANEL)
                    }),
            )
            .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.remote_credential_policy = policy.clone();
                    cx.notify();
                }
            }))
    }

    fn render_confirm_delete_remote_dialog(
        &self,
        name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_REMOTE), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::DELETE_REMOTE_NAME, &[("name", &(name).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_ONLY_REMOVES_THE_REMOTE_CONFIGURATION_FROM_THIS_REPOSITORY_SAVED)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::CANCEL),
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_DELETION),
                        !self.busy,
                        move |this, _, _| this.delete_remote(name.clone()),
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_remote_branch_dialog(
        &self,
        remote: String,
        branch: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let full_name = format!("{remote}/{branch}");
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_REMOTE_BRANCH), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::DELETE_REMOTE_BRANCH_FULL_NAME, &[("full_name", &(full_name).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        khaslana::ui_i18n::text(messages::THIS_DELETES_THE_BRANCH_ON_THE_REMOTE_AND_REFRESHES_LOCAL),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_DELETION),
                        !self.busy,
                        move |this, _, _| this.delete_remote_branch(remote.clone(), branch.clone()),
                        cx,
                    )),
            )
    }

    /// OAuth 品牌矩形按钮：显示带文字的品牌 logo（按当前主题选浅/深变体），点击触发登录。
    /// 两个按钮用统一固定宽度，避免 logo 长宽比不同导致不等宽。
    fn oauth_brand_button(
        &self,
        brand: OauthBrand,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon_h = 18.0_f32;
        let icon_w = icon_h * brand.aspect();
        // 按当前主题取适配的 logo 变体。GPUI 的 svg() 只能按 text_color 做单色 alpha 蒙版，
        // 会丢掉品牌色（Gitee 红、彩色文字）；改用 img() 走 render_single_frame，保留 SVG 原始 fill。
        div()
            .id(brand.id_str())
            .flex()
            .items_center()
            .justify_center()
            .w(px(140.0))
            .h(px(36.0))
            .rounded(px(ui_theme::RADIUS_XS))
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
            .child(
                img(brand.lockup_path())
                    .h(px(icon_h))
                    .w(px(icon_w))
                    .flex_none(),
            )
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        on_click(this, window, cx);
                        cx.notify();
                    }))
            })
            .when(!enabled, |this| this.opacity(0.5))
    }

    /// OAuth 快速登录区：品牌按钮 + 进行中的验证码/取消面板 + 错误提示。
    fn render_oauth_login_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let flow = &self.oauth_login_flow;
        let github_enabled = !flow.loading && !self.busy;
        let gitee_enabled = github_enabled && oauth::is_gitee_configured();
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .bg(rgb(ui_theme::WB_PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                            .child(khaslana::ui_i18n::text(messages::QUICK_SIGN_IN)),
                    )
                    .child(self.oauth_brand_button(
                        OauthBrand::Github,
                        github_enabled,
                        |this, _, _| this.start_github_login(),
                        cx,
                    ))
                    .child(self.oauth_brand_button(
                        OauthBrand::Gitee,
                        gitee_enabled,
                        |this, _, _| this.start_gitee_login(),
                        cx,
                    )),
            );

        if flow.loading {
            let provider_label = flow.provider.map(|p| p.label()).unwrap_or("OAuth");
            if let Some(code) = flow.user_code.clone() {
                // GitHub Device Flow：显示用户验证码 + 取消。
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(khaslana::ui_i18n::format(messages::PROVIDER_LABEL_IS_OPEN_IN_YOUR_BROWSER_VERIFY_THE_CODE, &[("provider_label", &(provider_label).to_string())])),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .font_family("Consolas")
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child(code),
                                )
                                .child(self.button(
                                    khaslana::ui_i18n::text(messages::CANCEL),
                                    !self.busy,
                                    |this, _, _| this.cancel_oauth_login(),
                                    cx,
                                )),
                        ),
                );
            } else {
                // Gitee 授权码流（或设备码尚未到位）：等待浏览器授权。
                panel = panel.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(khaslana::ui_i18n::format(messages::COMPLETE_PROVIDER_LABEL_SIGN_IN_IN_YOUR_BROWSER, &[("provider_label", &(provider_label).to_string())])),
                );
            }
        } else if !oauth::is_gitee_configured() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        khaslana::ui_i18n::text(messages::GITEE_SIGN_IN_REQUIRES_A_MAINTAINER_DEPLOYED_TOKEN_EXCHANGE_SERVICE),
                    ),
            );
        }

        if let Some(error) = flow.error.clone() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::DESTRUCTIVE))
                    .child(error),
            );
        }

        panel
    }

    /// 凭据测试地址确认弹窗：预填记录远端地址；说明裸主机地址的局限。
    fn render_test_credential_dialog(
        &self,
        record_id: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record_label = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .map(|record| {
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.username.clone())
            })
            .unwrap_or_else(|| khaslana::ui_i18n::text(messages::UNKNOWN_RECORD).to_string());
        let testing = self.busy || self.global_busy_tab.is_some();
        self.dialog_panel(khaslana::ui_i18n::text(messages::TEST_CREDENTIAL_CONNECTION), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::CREDENTIAL_RECORD_LABEL, &[("record_label", &(record_label).to_string())])),
            )
            .child(self.input(FieldId::CredentialTestUrl, false, window, cx))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        khaslana::ui_i18n::text(messages::THIS_ADDRESS_IS_USED_FOR_A_GIT_CONNECTION_TO_VERIFY),
                    ),
            )
            .when_some(self.credential_test_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(credential_test_error_text(&error)),
                )
            })
            .child(
                dialog_actions()
                    .child(
                        self.button(khaslana::ui_i18n::text(messages::CANCEL), true, |this, _, _| this.close_dialog(), cx),
                    )
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::START_TEST),
                        !testing,
                        |this, _, _| this.confirm_test_credential(),
                        cx,
                    )),
            )
    }

    fn render_credential_form_dialog(
        &self,
        _editing: Option<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::ADD_CREDENTIALS), cx)
            .w(px(680.0))
            .max_h(px(760.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(khaslana::ui_i18n::text(messages::TYPE)),
                    )
                    .child(self.credential_kind_button("HTTPS", CredentialFormMode::Https, cx))
                    .child(self.credential_kind_button("SSH", CredentialFormMode::Ssh, cx)),
            )
            .child(self.input(FieldId::CredentialDisplayName, false, window, cx))
            .child(self.input(FieldId::CredentialRemoteUrl, false, window, cx))
            .child(self.input(FieldId::CredentialUsername, false, window, cx))
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.input(FieldId::CredentialSecret, false, window, cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Https,
                |this| this.child(self.render_oauth_login_section(cx)),
            )
            .when(
                self.credential_form_mode == CredentialFormMode::Ssh,
                |this| {
                    this.child(self.button(khaslana::ui_i18n::text(crate::ssh_ui_resources::GENERATE), !self.ssh_ui.generating,
                        |this, _, _| this.open_generate_ssh_key(), cx))
                    .child(self.render_ssh_credential_discovery(cx))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(khaslana::ui_i18n::text(messages::PREFER_SSH_AGENT_WHEN_USING_A_PRIVATE_KEY_FILE_ONLY)),
                    )
                    .child(self.toggle_row(
                        "credential-form-use-ssh-agent",
                        khaslana::ui_i18n::text(messages::USE_SSH_AGENT_DO_NOT_SAVE_A_PRIVATE_KEY_PATH),
                        self.credential_use_ssh_agent,
                        |this, _, _| {
                            this.credential_use_ssh_agent = !this.credential_use_ssh_agent;
                            if this.credential_use_ssh_agent {
                                this.credential_key_path.clear();
                                this.credential_passphrase.clear();
                            }
                        },
                        cx,
                    ))
                    .when(!self.credential_use_ssh_agent, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(self.input(
                                            FieldId::CredentialKeyPath,
                                            false,
                                            window,
                                            cx,
                                        )),
                                )
                                .child(self.button(
                                    khaslana::ui_i18n::text(messages::SELECT_PRIVATE_KEY_FILE),
                                    !self.busy,
                                    |this, _, _| this.browse_credential_ssh_key(),
                                    cx,
                                )),
                        )
                        .child(self.input(
                            FieldId::CredentialPassphrase,
                            false,
                            window,
                            cx,
                        ))
                    })
                },
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(khaslana::ui_i18n::text(messages::REUSE_SCOPE)),
                    )
                    .child(self.credential_scope_button(
                        khaslana::ui_i18n::text(messages::THIS_REMOTE_ONLY),
                        CredentialScope::RemoteUrl,
                        true,
                        cx,
                    ))
                    .child(self.credential_scope_button(khaslana::ui_i18n::text(messages::SAME_HOST), CredentialScope::Host, true, cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::CANCEL),
                        !self.busy,
                        |this, _, _| this.close_credential_form(),
                        cx,
                    ))
                    .child(self.primary_button(
                        khaslana::ui_i18n::text(messages::SAVE),
                        !self.busy,
                        |this, _, _| this.save_credential_form(),
                        cx,
                    )),
            )
    }

    /// 凭据管理页的分组（Kit `SettingGroup` 列表）。
    ///
    /// 一组「凭据」：操作按钮 + 凭据列表（行内点击看详情、右键菜单、测试 /
    /// 删除）；新增/编辑表单打开时内嵌在列表之后。表单的 Enter 提交、IME
    /// 与操作遮罩语义都挂在 `active_dialog == CredentialForm` 上，不随承载
    /// 从模态弹窗改为页内分区而改变。表单输入全部经
    /// `this.input(FieldId, compact, window, cx)` 复用 `src/ui/fields.rs` 的
    /// `TextFieldState` 宿主，不新建第二套输入状态。
    ///
    /// `Entity` 非 `Copy`：每个 `move` 闭包各自 `cx.entity()` 取一份句柄。
    ///
    /// 主线把本函数接入 `settings_pane_groups` 前暂无调用方。
    #[allow(dead_code)]
    pub(crate) fn settings_credentials_groups(&self, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let group = SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                khaslana::ui_i18n::text(messages::CREDENTIALS),
                Some(
                    khaslana::ui_i18n::text(messages::SECRETS_ARE_STORED_ONLY_IN_THE_SYSTEM_CREDENTIAL_MANAGER_PASSWORDS)
                        .into(),
                ),
            ))
            .item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(this.primary_button(
                                khaslana::ui_i18n::text(messages::ADD_CREDENTIALS),
                                !this.busy,
                                |this, _, _| this.open_credential_form(),
                                cx,
                            ))
                            .child(this.button(
                                khaslana::ui_i18n::text(messages::REFRESH),
                                !this.busy,
                                |this, _, _| this.reload_credential_records(khaslana::ui_i18n::text(messages::CREDENTIAL_LIST_REFRESHED)),
                                cx,
                            ))
                    })
                }
            }))
            .item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        let empty = this.credential_records.is_empty();
                        let mut data_rows = Vec::new();
                        let mut action_rows = Vec::new();
                        if empty {
                            data_rows.push(
                                placeholder_row(
                                    khaslana::ui_i18n::text(messages::NO_SAVED_CREDENTIALS_SAVE_CREDENTIALS_DURING_A_REMOTE_OPERATION_TO),
                                )
                                .into_any_element(),
                            );
                        } else {
                            for record in this.credential_records.clone() {
                                data_rows.push(
                                    this.credential_record_data_row(record.clone(), cx)
                                        .into_any_element(),
                                );
                                action_rows.push(
                                    this.credential_record_actions_row(record, cx)
                                        .into_any_element(),
                                );
                            }
                        }
                        // 左右两栏共用同一垂直句柄：两栏视口高度与内容总高一致，
                        // 句柄记录的 max_offset 互相覆盖也无偏差，滚动天然同步。
                        let v_handle = this.scroll_handle("credential-record-list");
                        let h_handle = this.scroll_handle("credential-table-h");
                        let entity = cx.entity();
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .max_h(px(440.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER_MUTED))
                            .rounded_sm()
                            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                cx.stop_propagation();
                            })
                            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                                cx.stop_propagation();
                            })
                            // 表头：数据列部分随横向滚动与行区同步，操作列固定。
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .min_w(px(0.0))
                                    .items_stretch()
                                    .border_b_1()
                                    .border_color(rgb(ui_theme::BORDER_MUTED))
                                    .bg(rgb(ui_theme::WB_PANEL))
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(
                                        div()
                                            .id("credential-table-header-h")
                                            .flex_1()
                                            .min_w(px(0.0))
                                            // restrict_scroll_to_axis：容器只横向滚动，
                                            // 不把垂直滚轮转成水平滚动，避免抢走行区滚动。
                                            .overflow_x_scroll()
                                            .restrict_scroll_to_axis()
                                            .track_scroll(&h_handle)
                                            .child(this.credential_manager_header_data()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(CREDENTIAL_TABLE_OP_COL_W))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .py_2()
                                            .child(khaslana::ui_i18n::text(messages::ACTIONS)),
                                    ),
                            )
                            // 主体：左栏数据列横向滚动，右栏操作列横向冻结；
                            // 两栏同处一个垂直滚动语义（共享句柄），纵向一起滚。
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .min_h(px(0.0))
                                            .relative()
                                            .child(
                                                div()
                                                    .id("credential-table-rows-h")
                                                    .h_full()
                                                    .overflow_x_scroll()
                                                    .restrict_scroll_to_axis()
                                                    .track_scroll(&h_handle)
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(
                                                                CREDENTIAL_TABLE_DATA_MIN_W,
                                                            ))
                                                            .h_full()
                                                            .child(
                                                                div()
                                                                    .id("credential-record-list")
                                                                    .h_full()
                                                                    .overflow_y_scroll()
                                                                    .track_scroll(&v_handle)
                                                                    .child(
                                                                        div()
                                                                            .flex()
                                                                            .flex_col()
                                                                            .children(data_rows),
                                                                    ),
                                                            ),
                                                    ),
                                            )
                                            .when(!empty, |table| {
                                                table.child(
                                                    crate::ui_helpers::table_scrollbar_overlay(
                                                        entity,
                                                        "credential-record-table",
                                                        v_handle.clone(),
                                                        h_handle.clone(),
                                                        true,
                                                    ),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("credential-table-actions-v")
                                            .flex_none()
                                            .w(px(CREDENTIAL_TABLE_OP_COL_W))
                                            .overflow_y_scroll()
                                            .track_scroll(&v_handle)
                                            .child(
                                                div().flex().flex_col().children(action_rows),
                                            ),
                                    ),
                            )
                    })
                }
            }));

        let mut groups = vec![group];
        groups.extend(self.settings_ssh_groups(cx));
        groups
    }

    fn render_credential_details_dialog(
        &self,
        record_id: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            return self
                .dialog_panel(khaslana::ui_i18n::text(messages::CREDENTIAL_DETAILS), cx)
                .w(px(560.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(khaslana::ui_i18n::text(messages::THIS_CREDENTIAL_RECORD_DOES_NOT_EXIST_IT_MAY_HAVE_BEEN)),
                )
                .child(div().flex().justify_end().child(self.button(
                    khaslana::ui_i18n::text(messages::CLOSE),
                    !self.busy,
                    |this, _, _| this.open_credential_manager(),
                    cx,
                )));
        };

        let display_name = record
            .display_name
            .clone()
            .unwrap_or_else(|| credential_record_label(&record));
        let target = credential_display_target(&record);
        let key_path = record.key_path.clone().unwrap_or_else(|| "-".to_string());
        let last_used = record
            .last_used
            .map(timestamp_label)
            .unwrap_or_else(|| "-".to_string());

        self.dialog_panel(khaslana::ui_i18n::text(messages::CREDENTIAL_DETAILS), cx)
            .w(px(640.0))
            .max_h(px(620.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.0))
                    .child(self.credential_detail_row(khaslana::ui_i18n::text(messages::NAME), display_name))
                    .child(self.credential_detail_row(
                        khaslana::ui_i18n::text(messages::TYPE),
                        credential_kind_label(record.kind).to_string(),
                    ))
                    .child(self.credential_detail_row(
                        khaslana::ui_i18n::text(messages::REUSE_SCOPE),
                        credential_scope_label(record.scope).to_string(),
                    ))
                    .child(self.credential_detail_row(khaslana::ui_i18n::text(messages::HOST_REMOTE), target))
                    .child(self.credential_detail_row(khaslana::ui_i18n::text(messages::USERNAME), record.username))
                    .child(self.credential_detail_row(khaslana::ui_i18n::text(messages::SSH_KEY_PATH), key_path))
                    .child(
                        self.credential_detail_row(khaslana::ui_i18n::text(messages::CREATED), timestamp_label(record.created_at)),
                    )
                    .child(
                        self.credential_detail_row(khaslana::ui_i18n::text(messages::UPDATED), timestamp_label(record.updated_at)),
                    )
                    .child(self.credential_detail_row(khaslana::ui_i18n::text(messages::LAST_USED), last_used)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::PASSWORDS_PATS_AND_SSH_PASSPHRASES_ARE_NEVER_DISPLAYED_HERE)),
            )
            .child(div().flex().justify_end().child(self.button(
                khaslana::ui_i18n::text(messages::CLOSE),
                !self.busy,
                |this, _, _| this.open_credential_manager(),
                cx,
            )))
    }

    fn credential_detail_row(&self, label: &'static str, value: String) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .w(px(96.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(value),
            )
    }

    /// 凭据表数据列表头：随横向滚动与行数据列同步；操作列表头固定在右栏。
    fn credential_manager_header_data(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .min_w(px(CREDENTIAL_TABLE_DATA_MIN_W))
            .flex_1()
            .child(div().flex_none().w(px(112.0)).truncate().child(khaslana::ui_i18n::text(messages::NAME)))
            .child(div().flex_none().w(px(88.0)).truncate().child(khaslana::ui_i18n::text(messages::TYPE)))
            .child(div().flex_none().w(px(64.0)).truncate().child(khaslana::ui_i18n::text(messages::SCOPE)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(CREDENTIAL_TABLE_TARGET_MIN_W))
                    .truncate()
                    .child(khaslana::ui_i18n::text(messages::HOST_REMOTE)),
            )
            .child(div().flex_none().w(px(72.0)).truncate().child(khaslana::ui_i18n::text(messages::USERNAME)))
            .child(div().flex_none().w(px(68.0)).truncate().child("SSH Key"))
            .child(div().flex_none().w(px(108.0)).truncate().child(khaslana::ui_i18n::text(messages::UPDATED)))
    }

    /// 凭据表数据行（左栏）：随横向滚动；点击打开详情、右键打开上下文菜单。
    fn credential_record_data_row(
        &self,
        record: CredentialRecord,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let detail_id = record.id.clone();
        let menu_id = record.id.clone();
        let label = credential_record_label(&record);
        let target = credential_display_target(&record);
        let key_file = credential_key_filename(&record);
        let display_name = record.display_name.clone().unwrap_or_else(|| label.clone());
        div()
            .id(format!("credential-record-{}", record.id))
            .flex()
            .flex_none()
            .w_full()
            .h(px(CREDENTIAL_ROW_H))
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::WB_PANEL))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_credential_details(detail_id.clone());
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.open_credential_context_menu(menu_id.clone(), event, window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(112.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .truncate()
                    .child(display_name),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(88.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .truncate()
                    .child(credential_kind_label(record.kind)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(64.0))
                    .text_color(rgb(ui_theme::PRIMARY))
                    .truncate()
                    .child(credential_scope_label(record.scope)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(CREDENTIAL_TABLE_TARGET_MIN_W))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .truncate()
                    .child(target),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(72.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .truncate()
                    .child(record.username),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(68.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .truncate()
                    .child(key_file),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(108.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .truncate()
                    .child(timestamp_label(record.updated_at)),
            )
    }

    /// 凭据表操作行（右栏冻结列）：横向不滚动，与左栏数据行共享垂直句柄同步。
    fn credential_record_actions_row(
        &self,
        record: CredentialRecord,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record_id = record.id.clone();
        let delete_id = record.id.clone();
        let detail_id = record.id.clone();
        let menu_id = record.id.clone();
        let label = credential_record_label(&record);
        div()
            .id(format!("credential-record-actions-row-{}", record.id))
            .flex()
            .flex_none()
            .h(px(CREDENTIAL_ROW_H))
            .items_center()
            .justify_end()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(rgb(ui_theme::BORDER_MUTED))
            .text_size(px(12.0))
            .bg(rgb(ui_theme::WB_PANEL))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(ui_theme::WB_ROW_HOVER)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.open_credential_details(detail_id.clone());
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.open_credential_context_menu(menu_id.clone(), event, window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::TEST),
                        !self.busy,
                        move |this, _, cx| {
                            cx.stop_propagation();
                            this.open_test_credential_dialog(record_id.clone());
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::DELETE),
                        !self.busy,
                        move |this, _, cx| {
                            cx.stop_propagation();
                            this.open_delete_credential_confirm(delete_id.clone(), label.clone());
                        },
                        cx,
                    )),
            )
    }

    fn render_confirm_delete_credential_dialog(
        &self,
        record_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_CREDENTIALS), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(khaslana::ui_i18n::format(messages::DELETE_CREDENTIAL_LABEL, &[("label", &(label).to_string())])),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(khaslana::ui_i18n::text(messages::THIS_REMOVES_BOTH_THE_NON_SENSITIVE_INDEX_RECORD_AND_THE)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        khaslana::ui_i18n::text(messages::CANCEL),
                        !self.busy,
                        |this, _, _| {
                            this.open_credential_manager();
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        khaslana::ui_i18n::text(messages::CONFIRM_DELETION),
                        !self.busy,
                        move |this, _, _| this.delete_credential_record(record_id.clone()),
                        cx,
                    )),
            )
    }

    pub(crate) fn dialog_panel(
        &self,
        title: impl Into<gpui::SharedString>,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        ui_dialog_panel(title)
    }
}

// 验证器返回结构化消息；在最后显示边界解析，不能把编码交给文本元素。
fn credential_test_error_text(error: &str) -> String {
    khaslana::ui_i18n::resolve(error)
}

#[cfg(test)]
#[path = "tests/dialog_view.rs"]
mod tests;
