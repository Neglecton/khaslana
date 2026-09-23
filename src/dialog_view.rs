//! RepositoryView 的应用级对话框渲染。

use crate::*;
use gpui_kit::base::FocusTrapElement;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{Disableable, button::Button, menu::DropdownMenu};

impl RepositoryView {
    pub(crate) fn render_dialogs(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(dialog) = self.active_dialog.clone() else {
            return div().into_any_element();
        };

        let content = match dialog {
            DialogState::CloneRepo => self.render_clone_dialog(window, cx).into_any_element(),
            DialogState::CreateBranch => self
                .render_create_branch_dialog(window, cx)
                .into_any_element(),
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
        self.dialog_panel("克隆仓库", cx)
            .child(self.input(FieldId::CloneUrl, false, window, cx))
            .child(self.input(FieldId::ClonePath, false, window, cx))
            .child(self.toggle_row(
                "clone-recursive-submodules",
                "递归克隆子模块",
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
                        "填写远程仓库 URL 和父文件夹后显示最终代码路径".to_string()
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .child(self.button(
                        "选择目录",
                        !self.busy,
                        |this, _, _| this.browse_clone_target(),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "取消",
                                !self.busy,
                                |this, _, _| this.close_dialog(),
                                cx,
                            ))
                            .child(self.primary_button(
                                "克隆",
                                !self.busy,
                                |this, _, _| this.clone_repo(),
                                cx,
                            )),
                    ),
            )
    }

    fn render_confirm_window_close_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel("关闭 Khaslana", cx)
            .w(px(520.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("要直接退出应用，还是让 Khaslana 继续在系统托盘中运行？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("缩小到托盘后，可点击托盘图标恢复主窗口，或从托盘菜单退出。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| this.cancel_window_close(), cx))
                    .child(self.primary_button(
                        "缩小到托盘",
                        true,
                        |this, window, cx| this.minimize_to_tray(window, cx),
                        cx,
                    ))
                    .child(self.danger_button(
                        "直接退出",
                        true,
                        |this, _, cx| this.exit_application(cx),
                        cx,
                    )),
            )
    }

    fn render_portable_migration_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog_panel("迁移到便携目录", cx)
            .w(px(540.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("检测到应用数据当前存放在 C 盘系统目录，是否迁移到程序所在目录？"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        "迁移后，数据库、更新缓存和工作流模板将统一保存在可执行文件同级的 \
                         data/ 目录，便于整体备份并减少 C 盘占用。点击「迁移并重启」后应用将关闭，\
                         并在下次启动时自动完成数据搬运。若选择「保持现状」，后续可在「设置」-「更新设置」中手动执行迁移。",
                    ),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        "保持现状",
                        true,
                        |this, _, _| this.dismiss_portable_migration(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "迁移并重启",
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
                "检测到程序当前位于可能被自动清理的目录（临时目录或聊天软件的接收文件目录）。\
                 这类目录会被系统或聊天软件的清理功能定期清空，届时程序与其中的数据都会丢失。"
            }
            _ => {
                "检测到程序当前位于下载文件夹。下载文件夹可能被系统「存储感知」\
                或清理工具定期清空，届时程序与其中的数据都会丢失。"
            }
        };
        let data_at_risk = khaslana::portable_database_path().is_some_and(|path| path.exists());
        let data_note = if data_at_risk {
            "数据目前也存放在该目录中，强烈建议立即移动。"
        } else {
            "你的数据已保存在安全位置，仅程序本体存在丢失风险。"
        };
        let target_label = khaslana::exe_relocation_target_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "未知".to_string());
        self.dialog_panel("移动到安全目录", cx)
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
                    .child(format!("{data_note}点击「移动并重启」后应用将关闭，程序与数据会被搬到 {target_label} 并从新位置重新启动。若选择「保持现状」，之后可在「设置」-「更新设置」中手动执行移动。")),
            )
            .child(
                dialog_actions()
                    .child(self.button(
                        "保持现状",
                        true,
                        |this, _, _| this.dismiss_exe_relocation(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "移动并重启",
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
        self.dialog_panel("新建分支", cx)
            .child(self.input(FieldId::BranchName, false, window, cx))
            .child(self.toggle_row(
                "create-branch-checkout",
                "创建成功后切换到新分支",
                self.create_branch_checkout,
                |this, _, _| this.create_branch_checkout = !this.create_branch_checkout,
                cx,
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "创建",
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
        self.dialog_panel("重命名分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("当前分支：{branch}")),
            )
            .child(self.input(FieldId::BranchRename, false, window, cx))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "重命名",
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
        let mode_label = reset_mode_label(mode);
        let mode_help = reset_mode_help(mode);
        self.dialog_panel("确认重置分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("将当前分支重置到该提交。{mode_label}：{mode_help}")),
            )
            .when(mode == ResetMode::Hard, |this| {
                this.child(danger_callout(
                    "强制重置会移动当前分支，目标提交之后的已提交代码会从分支历史中移除。确认前请确保目标提交正确。",
                ))
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认重置",
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
        self.dialog_panel("确认回滚提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("确认后会创建一个新的提交，用于撤销该提交引入的修改。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认回滚",
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
        self.dialog_panel("确认撤销合并提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("确认后会创建一个新的提交，用于撤销这次合并相对主线引入的修改。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("该操作不会删除原合并提交，也不会重写分支历史；若产生冲突，请在冲突解决中心处理后手动提交。"),
            )
            .child(danger_callout(
                "这等价于 git revert -m 1，表示保留合并提交的第一父提交一侧。后续再次合并同一分支时，Git 会认为这次合并的改动曾被主动撤销。",
            ))
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认撤销合并",
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
        self.dialog_panel("确认还原到暂存区", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("目标提交：{} {}", short_oid(&oid), summary)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("确认后会撤销该提交记录，并把该提交引入的修改保留在暂存区。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("该操作只支持当前分支最新且尚未推送的普通提交。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认还原",
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
        self.dialog_panel("修补已推送的提交", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child("当前最新提交已推送到远端。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("修补会重写这条提交，之后必须用强制推送才能覆盖远端历史；当前版本暂不支持强推，其他协作者的本地历史会与远端分叉。"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("建议仅对尚未推送的提交使用修补。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        if and_push { "仍要修补并推送" } else { "仍要修补" },
                        !self.busy,
                        move |this, _, _| {
                            let message = this.commit_message.value.trim().to_string();
                            if and_push {
                                let Some(remote) = this.current_remote() else {
                                    this.last_error = Some("当前仓库没有远端".into());
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
                format!("目标提交：{} {}", short_oid(oid), summary)
            }
            (None, _) => "目标提交：HEAD（当前分支最新提交）".to_string(),
        };
        self.dialog_panel("创建标签", cx)
            .child(self.input(FieldId::TagName, false, window, cx))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(target_label),
            )
            .child(self.toggle_row(
                "tag-annotated-toggle",
                "创建附注标签（记录标签信息与创建者，发布推荐）",
                self.tag_annotated,
                |this, _, _| this.tag_annotated = !this.tag_annotated,
                cx,
            ))
            .when(self.tag_annotated, |this| {
                this.child(self.input(FieldId::TagMessage, false, window, cx))
            })
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "创建",
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
            .unwrap_or_else(|| "选择远端".to_string());
        let menu_remotes = remotes.clone();
        self.dialog_panel("推送标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("标签：{tag}")),
            )
            .child(
                Button::new("tag-push-remote-select")
                    .label(trigger_label)
                    .accessibility_label("选择远端")
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
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.primary_button(
                        "推送",
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
        self.dialog_panel("删除标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("标签：{tag}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("确认后删除本地标签，不影响远端标签。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
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
        self.dialog_panel("删除远端标签", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("远端标签：{remote}/{tag}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("确认后从远端删除该标签，已发布的版本引用将不可再用，删除后无法恢复。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
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
            DiscardTarget::Single => "目标文件".to_string(),
            DiscardTarget::Selected => format!("选定文件（{count} 个）"),
            DiscardTarget::All => match scope {
                DiffScope::Staged => format!("暂存区全部文件（{count} 个）"),
                DiffScope::Unstaged => format!("修改区全部文件（{count} 个）"),
            },
        };
        let preview = discard_paths_preview(&paths);
        let help = match scope {
            DiffScope::Staged => {
                "将丢弃这些文件全部未提交更改，包括暂存区和工作区。新增文件会被删除，删除文件会被恢复。此操作无法从 Khaslana 内撤销。"
            }
            DiffScope::Unstaged => {
                "将仅丢弃这些文件尚未暂存的更改，已暂存内容会保留。未跟踪新增文件会被删除。此操作无法从 Khaslana 内撤销。"
            }
        };
        self.dialog_panel("确认回滚更改", cx)
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
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认回滚",
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
        self.dialog_panel("覆盖现有冲突处理？", cx)
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
                    .child("该文件的草稿已有块处理或手工修改，生成 AI 合并建议会覆盖这些内容。"),
            )
            .child(danger_callout(
                "覆盖后已接受的块、忽略操作和手工编辑都会丢失，需要重新处理。",
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        "返回保留现状",
                        !self.busy,
                        |this, _, _| this.close_dialog(),
                        cx,
                    ))
                    .child(self.danger_button(
                        "覆盖并生成",
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
            .unwrap_or_else(|| "当前冲突文件".to_string());

        self.dialog_panel("仍有未处理代码块", cx)
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
                    .child(format!(
                        "还有 {unresolved_count} 个代码块未处理，是否继续标记已解决？"
                    )),
            )
            .child(danger_callout(
                "继续后会直接把当前结果写入工作区并从索引中移除冲突标记。",
            ))
            .child(
                dialog_actions()
                    .child(self.button(
                        "返回继续处理",
                        !self.busy,
                        |this, _, _| this.cancel_pending_conflict_resolve(),
                        cx,
                    ))
                    .child(self.danger_button(
                        "继续解决",
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
                "（此版本未附版本说明）".to_string()
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
            .unwrap_or_else(|| "未知".to_string());
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
            .unwrap_or_else(|| "无".to_string());

        let update_group = SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                "更新",
                Some("启动时自动检查与手动检查；跳过版本只影响自动提示。".into()),
            ))
            .item(SettingItem::new(
                "当前版本",
                SettingField::render({
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .text_size(px(ui_theme::TYPE_BODY))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(format!("v{version}"))
                        })
                    }
                }),
            ))
            .item(SettingItem::new(
                "自动检查更新",
                SettingField::switch(
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
                ),
            ))
            // 测试版（Beta）更新渠道：勾选后检测/安装所有版本（含预发布），
            // 未勾选只走正式版清单（与旧版本行为一致）。切换后下次检查生效。
            .item(
                SettingItem::new(
                    "接收测试版（Beta）更新",
                    SettingField::switch(
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
                    ),
                )
                .description("开启后同时检测并安装测试版；测试版可能不稳定"),
            )
            .item(SettingItem::new(
                "已跳过版本",
                SettingField::render({
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .text_size(px(ui_theme::TYPE_BODY))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                .child(skipped_label.clone())
                        })
                    }
                }),
            ))
            .item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        dialog_actions()
                            .child(this.primary_button(
                                "立即检查",
                                !this.update_checking && !this.busy,
                                |this, _, _| this.start_update_check(UpdateCheckTrigger::Manual),
                                cx,
                            ))
                            .child(this.button(
                                "清除跳过",
                                this.update_preferences.skipped_version.is_some(),
                                |this, _, _| this.clear_skipped_version(),
                                cx,
                            ))
                    })
                }
            }));

        let mut channel_group = SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                "渠道与迁移",
                Some("待安装的新版本，以及数据目录与程序位置的迁移入口。".into()),
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
                                            .child(format!("发现新版本 v{update_version}")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                            .child(format!("发布于 {update_published_at}"))
                                            .child(format!("包大小 {update_size}")),
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
                                    // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                                    .children(update_notes.lines().map(|line| {
                                        div().child(if line.is_empty() {
                                            " ".to_string()
                                        } else {
                                            line.to_string()
                                        })
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(this.primary_button(
                                        "立即更新",
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
                                                        .unwrap_or_else(|| "准备下载...".into()),
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
                                    .child("数据目录"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(format!("当前：{current_db_label}")),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(
                                        "可将数据从 C 盘系统目录迁移到程序所在目录，便于整体备份并减少 C 盘占用。",
                                    ),
                            )
                            .child(this.primary_button(
                                "迁移到便携目录",
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
                                    .child("程序位置"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(format!("当前：{relocation_db_label}")),
                            )
                            .child(
                                div().text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(
                                    "程序当前位于可能被清理的目录（临时/聊天软件接收/下载目录），\
                                     建议把程序与数据移动到独立的安全目录。",
                                ),
                            )
                            .child(this.primary_button(
                                "移动到安全目录",
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

    /// 「关于」页的分组（Kit `SettingGroup` 列表）。
    ///
    /// 纯展示条目：版本号 + 渠道徽标 + 版本说明，没有可写字段，因此不声明
    /// `on_reset` / `default_value`——页头「重置」按钮对本页不适用
    /// （`resettable` 由页面构建方控制，展示项也不会被判定为脏）。
    /// 版本说明由发版流水线经 KHASLANA_RELEASE_NOTES 编译期嵌入；本地
    /// 开发构建或未配置时显示占位文案。
    ///
    /// 主线把本函数接入 `settings_pane_groups` 前暂无调用方。
    #[allow(dead_code)]
    pub(crate) fn settings_about_groups(&self, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let version = env!("CARGO_PKG_VERSION");
        let channel = update::current_channel();
        let notes = update::current_release_notes();
        let notes_display = if notes.trim().is_empty() {
            "（此版本未附版本说明）".to_string()
        } else {
            notes.to_string()
        };
        let beta_channel = channel == "测试版";

        vec![SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                "版本",
                Some("更新渠道与自动检查可在「更新设置」中配置。".into()),
            ))
            .item(SettingItem::new(
                "版本号",
                SettingField::render({
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(ui_theme::TYPE_BODY))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                                        .child(format!("Khaslana v{version}")),
                                )
                                .child(
                                    // 渠道徽标：按版本号预发布段推断（正式版 / 测试版）。
                                    div()
                                        .id("about-channel-badge")
                                        .flex_none()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(ui_theme::RADIUS_PILL))
                                        .bg(rgb(if beta_channel {
                                            ui_theme::FEEDBACK_WARNING_BG
                                        } else {
                                            ui_theme::PRIMARY_SUBTLE
                                        }))
                                        .text_size(px(10.0))
                                        .text_color(rgb(if beta_channel {
                                            ui_theme::FEEDBACK_WARNING_TEXT
                                        } else {
                                            ui_theme::PRIMARY
                                        }))
                                        .child(channel),
                                )
                        })
                    }
                }),
            ))
            .item(SettingItem::new(
                "版本说明",
                SettingField::render({
                    let view = cx.entity();
                    move |_options, _window, cx| {
                        view.update(cx, |_this, _cx| {
                            div()
                                .id("about-release-notes")
                                .max_h(px(280.0))
                                .overflow_y_scroll()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                                .children(notes_display.lines().map(|line| {
                                    div().child(if line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        line.to_string()
                                    })
                                }))
                        })
                    }
                }),
            ))]
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

        self.dialog_panel(format!("发现新版本 v{version}"), cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("发布于 {published_at}")),
            )
            .child(
                // 版本说明：多行可滚动（内容多时可翻看，不再 120px 硬截断）。
                div()
                    .id("new-version-notes")
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    // GPUI 无 pre-wrap：按行拆分渲染，空行用空格占位保持行高。
                    .children(notes.lines().map(|line| {
                        div().child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    })),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(format!("包大小：{:.1} MB", size_mb)),
            )
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        "立即更新",
                        !self.update_downloading && !self.busy,
                        |this, _, _| {
                            this.active_dialog = None;
                            this.start_update_download();
                        },
                        cx,
                    ))
                    .child(self.button(
                        "跳过此版本",
                        !self.update_downloading,
                        move |this, _, _| this.skip_version(&version_owned),
                        cx,
                    ))
                    .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_confirm_install_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staging_dir = self.staging_dir_for_install.clone();
        let version_owned = version.to_string();

        self.dialog_panel("更新准备就绪", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!(
                        "版本 v{version} 已下载并校验通过，应用将重启以完成安装。"
                    )),
            )
            .child(danger_callout(
                "安装过程中应用会自动退出并重启，请确保没有未保存的工作。",
            ))
            .child(
                dialog_actions()
                    .child(self.primary_button(
                        "立即重启",
                        true,
                        move |this, _, cx| {
                            if let Some(dir) = staging_dir.clone() {
                                this.install_update(&dir, &version_owned, cx);
                            } else {
                                this.update_error = Some("staging 目录丢失".into());
                            }
                        },
                        cx,
                    ))
                    .child(self.button("稍后", true, |this, _, _| this.close_dialog(), cx)),
            )
    }

    fn render_no_write_permission_dialog(
        &self,
        version: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("无法自动更新", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!(
                        "当前目录没有写入权限，无法自动安装新版本（v{version}）。请手动下载新版本："
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "打开 CNB 下载页",
                        true,
                        |_, _, _| {
                            open_url("https://cnb.cool/suhoan/khaslana-release");
                        },
                        cx,
                    ))
                    .child(self.button(
                        "打开 GitHub Release",
                        true,
                        |_, _, _| {
                            open_url("https://github.com/FuturePrayer/khaslana/releases");
                        },
                        cx,
                    )),
            )
            .child(dialog_actions().child(self.button(
                "关闭",
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
            vec![placeholder_row("暂无远端。可以点击“新增远端”添加。").into_any_element()]
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
                            .child("远端管理"),
                    )
                    .child(self.primary_button(
                        "新增远端",
                        self.repo_path.is_some() && !self.busy,
                        |this, _, _| this.open_remote_form(None),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("远端地址会同时作为 fetch 和 push URL；凭据只从已保存凭据中选择。"),
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
                "关闭",
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
            .child(div().flex_none().w(px(104.0)).child("名称"))
            .child(div().flex_1().min_w(px(0.0)).child("地址"))
            .child(div().flex_none().w(px(180.0)).child("凭据"))
            .child(div().flex_none().w(px(106.0)).child("操作"))
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
            RemoteCredentialPolicy::NoCredential => "无凭据".to_string(),
            RemoteCredentialPolicy::Record(record_id) => self
                .credential_records
                .iter()
                .find(|record| record.id == record_id)
                .map(credential_record_label)
                .unwrap_or_else(|| "凭据不存在".to_string()),
            RemoteCredentialPolicy::AutoMatch => self
                .matching_credential_for_remote_url(&remote.url)
                .map(|record| format!("自动：{}", credential_record_label(record)))
                .unwrap_or_else(|| "自动匹配".to_string()),
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
                        "编辑",
                        !self.busy,
                        move |this, _, _| this.open_remote_form(Some(edit_name.clone())),
                        cx,
                    ))
                    .child(self.danger_button(
                        "删除",
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
            "编辑远端"
        } else {
            "新增远端"
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
                            .child("绑定凭据"),
                    )
                    .child(self.remote_credential_picker(cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
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
                "自动匹配保存凭据".to_string(),
                true,
                cx,
            )
            .into_any_element(),
        );
        rows.push(
            self.remote_credential_option(
                RemoteCredentialPolicy::NoCredential,
                "无凭据".to_string(),
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
                        label.push_str("（不匹配）");
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
        self.dialog_panel("删除远端", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("确认删除远端：{name}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("这只会删除当前仓库的远端配置，不会删除任何已保存凭据。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.active_dialog = Some(DialogState::RemoteManager);
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "确认删除",
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
        self.dialog_panel("删除远端分支", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("确认删除远端分支：{full_name}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        "这会删除远端仓库上的分支，并刷新本地远端分支列表；不会删除同名本地分支。",
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
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
                            .child("快速登录"),
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
                                .child(format!(
                                    "已在浏览器打开{provider_label}，请确认验证码后完成登录："
                                )),
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
                                    "取消",
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
                        .child(format!("请在浏览器中完成{provider_label}登录...")),
                );
            }
        } else if !oauth::is_gitee_configured() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        "Gitee 登录需由维护者部署令牌交换服务（见 AGENTS.md）；GitHub 可直接使用。",
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
            .unwrap_or_else(|| "未知记录".to_string());
        let testing = self.busy || self.global_busy_tab.is_some();
        self.dialog_panel("测试凭据连接", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("凭据：{record_label}")),
            )
            .child(self.input(FieldId::CredentialTestUrl, false, window, cx))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(
                        "将使用此地址发起一次 Git 连接验证凭据。建议填写真实仓库地址；                         裸站点地址（如 https://gitee.com）可能因服务器不发起认证而无法验证凭据。",
                    ),
            )
            .when_some(self.credential_test_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::FEEDBACK_ERROR_TEXT))
                        .child(error),
                )
            })
            .child(
                dialog_actions()
                    .child(
                        self.button("取消", true, |this, _, _| this.close_dialog(), cx),
                    )
                    .child(self.primary_button(
                        "开始测试",
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
        self.dialog_panel("添加凭据", cx)
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
                            .child("类型"),
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
                    this.child(self.render_ssh_credential_discovery(cx))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child("推荐优先使用 SSH Agent；使用私钥文件时，应用只保存路径，密码短语仍存入系统 Keyring。"),
                    )
                    .child(self.toggle_row(
                        "credential-form-use-ssh-agent",
                        "使用 SSH Agent（不保存私钥路径）",
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
                                    "选择私钥文件",
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
                            .child("复用范围"),
                    )
                    .child(self.credential_scope_button(
                        "仅此远端",
                        CredentialScope::RemoteUrl,
                        true,
                        cx,
                    ))
                    .child(self.credential_scope_button("同站点", CredentialScope::Host, true, cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| this.close_credential_form(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
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
                "凭据",
                Some(
                    "密文仅保存在系统凭据管理器；这里不显示、不复制密码、PAT 或 SSH 密码短语。"
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
                                "添加凭据",
                                !this.busy,
                                |this, _, _| this.open_credential_form(),
                                cx,
                            ))
                            .child(this.button(
                                "刷新",
                                !this.busy,
                                |this, _, _| this.reload_credential_records("凭据列表已刷新"),
                                cx,
                            ))
                    })
                }
            }))
            .item(SettingItem::render({
                let view = cx.entity();
                move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        let rows = if this.credential_records.is_empty() {
                            vec![
                                placeholder_row(
                                    "暂无已保存凭据。远程操作时勾选保存后会出现在这里。",
                                )
                                .into_any_element(),
                            ]
                        } else {
                            this.credential_records
                                .iter()
                                .cloned()
                                .map(|record| {
                                    this.credential_record_row(record, cx).into_any_element()
                                })
                                .collect::<Vec<_>>()
                        };
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
                            .child(this.credential_manager_header())
                            .child({
                                let handle = this.scroll_handle("credential-record-list");
                                let content = div()
                                    .id("credential-record-list")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .gap_0()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .overflow_y_scroll()
                                    .track_scroll(&handle)
                                    .children(rows)
                                    .into_any_element();
                                scrollable_frame_when(
                                    "credential-record-list",
                                    ScrollbarMode::Vertical,
                                    content,
                                    handle,
                                    !this.credential_records.is_empty(),
                                    cx,
                                )
                            })
                    })
                }
            }));


        vec![group]
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
                .dialog_panel("凭据详情", cx)
                .w(px(560.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child("凭据记录不存在，可能已经被删除。"),
                )
                .child(div().flex().justify_end().child(self.button(
                    "关闭",
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

        self.dialog_panel("凭据详情", cx)
            .w(px(640.0))
            .max_h(px(620.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.0))
                    .child(self.credential_detail_row("名称", display_name))
                    .child(self.credential_detail_row(
                        "类型",
                        credential_kind_label(record.kind).to_string(),
                    ))
                    .child(self.credential_detail_row(
                        "复用范围",
                        credential_scope_label(record.scope).to_string(),
                    ))
                    .child(self.credential_detail_row("站点 / 远端", target))
                    .child(self.credential_detail_row("用户名", record.username))
                    .child(self.credential_detail_row("SSH Key 路径", key_path))
                    .child(
                        self.credential_detail_row("创建时间", timestamp_label(record.created_at)),
                    )
                    .child(
                        self.credential_detail_row("更新时间", timestamp_label(record.updated_at)),
                    )
                    .child(self.credential_detail_row("最后使用时间", last_used)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("密码、PAT 和 SSH 密码短语不会在这里显示。"),
            )
            .child(div().flex().justify_end().child(self.button(
                "关闭",
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

    fn credential_manager_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
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
            .child(div().flex_none().w(px(112.0)).truncate().child("名称"))
            .child(div().flex_none().w(px(88.0)).truncate().child("类型"))
            .child(div().flex_none().w(px(64.0)).truncate().child("范围"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .child("站点 / 远端"),
            )
            .child(div().flex_none().w(px(72.0)).truncate().child("用户名"))
            .child(div().flex_none().w(px(68.0)).truncate().child("SSH Key"))
            .child(div().flex_none().w(px(108.0)).truncate().child("更新时间"))
            .child(div().flex_none().w(px(112.0)).truncate().child("操作"))
    }

    fn credential_record_row(
        &self,
        record: CredentialRecord,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record_id = record.id.clone();
        let detail_id = record.id.clone();
        let menu_id = record.id.clone();
        let delete_id = record.id.clone();
        let actions_id = record.id.clone();
        let label = credential_record_label(&record);
        let target = credential_display_target(&record);
        let key_file = credential_key_filename(&record);
        let display_name = record.display_name.clone().unwrap_or_else(|| label.clone());
        div()
            .id(format!("credential-record-{}", record.id))
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
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
                    .id(format!("credential-record-actions-{actions_id}"))
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
                    .min_w(px(0.0))
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
            .child(
                div()
                    .flex_none()
                    .w(px(112.0))
                    .flex()
                    .justify_end()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.button(
                        "测试",
                        !self.busy,
                        move |this, _, cx| {
                            cx.stop_propagation();
                            this.open_test_credential_dialog(record_id.clone());
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "删除",
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
        self.dialog_panel("删除凭据", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(format!("确认删除凭据：{label}")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child("删除会同时移除非敏感索引和系统凭据管理器中的密文。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "取消",
                        !self.busy,
                        |this, _, _| {
                            this.open_credential_manager();
                        },
                        cx,
                    ))
                    .child(self.danger_button(
                        "确认删除",
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
