//! 托管 SSH 身份与主机信任界面。口令仅在字段和生成任务中短暂持有。
use crate::*;
use crate::ssh_ui_resources as messages;
use gpui::AnyElement;
use gpui_kit::component::setting::{SettingGroup, SettingItem};

#[cfg(test)]
#[path = "tests/ssh_ui.rs"]
mod tests;

/// 授信响应不是任务结束：后台消费响应及 pin 落盘期间仍须接受关闭 tab 取消。
#[derive(Default)]
pub(crate) struct HostOperationRegistry {
    entries: Vec<(Option<RepoTabId>, khaslana::ssh_host_trust::HostTrustOperation)>,
}
impl HostOperationRegistry {
    pub(crate) fn register(&mut self, tab_id: Option<RepoTabId>, operation: khaslana::ssh_host_trust::HostTrustOperation) {
        self.prune_inactive();
        if !self.entries.iter().any(|(tab, token)| *tab == tab_id && token.id == operation.id) {
            self.entries.push((tab_id, operation));
        }
    }

    pub(crate) fn cancel_tab(&mut self, tab_id: RepoTabId) {
        for (tab, operation) in &self.entries {
            if *tab == Some(tab_id) { operation.cancel(); }
        }
        self.prune_inactive();
    }

    pub(crate) fn prune_inactive(&mut self) {
        self.entries.retain(|(_, operation)| operation.is_active());
    }
}

pub(crate) struct SshUiState {
    keys: Vec<khaslana::ssh_keys::ManagedSshKey>,
    hosts: Vec<(String, u16, String, String)>,
    loading: bool,
    request_id: u64,
    challenges: VecDeque<(Option<RepoTabId>, khaslana::ssh_host_trust::HostTrustRequest)>,
    previous_dialog: Option<DialogState>,
    operations: HostOperationRegistry,
    pub(crate) name: TextFieldState,
    pub(crate) comment: TextFieldState,
    pub(crate) passphrase: TextFieldState,
    pub(crate) passphrase_confirm: TextFieldState,
    pub(crate) no_passphrase: bool,
    pub(crate) generation: u64,
    pub(crate) generating: bool,
    pub(crate) generated_public_key: Option<String>,
    pub(crate) generated_key_id: Option<String>,
    generated_details: Option<(String, PathBuf, PathBuf)>,
    actual_directory: Option<PathBuf>,
    return_to_credential: bool,
    pub(crate) generated_passphrase: Option<zeroize::Zeroizing<String>>,
}

impl SshUiState {
    pub(crate) fn new(cx: &mut Context<RepositoryView>) -> Self {
        Self {
            keys: Vec::new(), hosts: Vec::new(), loading: false, request_id: 0,
            challenges: VecDeque::new(), previous_dialog: None,
            operations: HostOperationRegistry::default(),
            name: TextFieldState::localized(cx, messages::NAME),
            comment: TextFieldState::localized(cx, messages::COMMENT),
            passphrase: TextFieldState::localized(cx, messages::PASSPHRASE).secret(),
            passphrase_confirm: TextFieldState::localized(cx, messages::CONFIRM_PASSPHRASE).secret(),
            no_passphrase: false,
            generation: 0,
            generating: false,
            generated_public_key: None,
            generated_key_id: None,
            generated_details: None, actual_directory: None, return_to_credential: false,
            generated_passphrase: None,
        }
    }

    fn clear_form(&mut self) {
        self.name.clear();
        self.comment.clear();
        self.passphrase.clear();
        self.passphrase_confirm.clear();
        self.no_passphrase = false;
        self.generated_public_key = None;
        self.generated_key_id = None;
        self.generated_details = None;
        self.actual_directory = None;
        self.generated_passphrase = None;
        // 只撤离表单，不回滚已生成的磁盘密钥；迟到结果不能重开表单。
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SshUiEvent {
    Loaded { request_id: u64, result: std::result::Result<Vec<khaslana::ssh_keys::ManagedSshKey>, String> },
    Directory { generation: u64, result: std::result::Result<PathBuf, String> },
    GeneratedDetails { generation: u64, key_id: String, fingerprint: String, result: std::result::Result<PathBuf, String> },
    HostsLoaded { request_id: u64, result: std::result::Result<Vec<(String, u16, String, String)>, String> },
    Generated { generation: u64, result: std::result::Result<khaslana::ssh_keys::ManagedSshKey, String> },
    KeyPath { key_id: String, generation: u64, result: std::result::Result<PathBuf, String> },
    Deleted { result: std::result::Result<(), String> },
    Challenge { tab_id: Option<RepoTabId>, request: khaslana::ssh_host_trust::HostTrustRequest },
}

#[derive(Clone)]
pub(crate) struct TabHostTrustProvider {
    pub(crate) tx: Sender<UiEvent>,
    pub(crate) tab_id: Option<RepoTabId>,
}
impl khaslana::ssh_host_trust::HostTrustProvider for TabHostTrustProvider {
    fn request(&self, request: khaslana::ssh_host_trust::HostTrustRequest) {
        if self.tx.try_send(UiEvent::SshUi(SshUiEvent::Challenge { tab_id: self.tab_id, request: request.clone() })).is_err() {
            let _ = request.response.send(khaslana::ssh_host_trust::HostTrustDecision::Reject);
        }
    }
}

pub(crate) fn generation_form_valid(name: &str, passphrase: &str, confirm: &str, no_passphrase: bool) -> bool {
    !name.trim().is_empty()
        && if no_passphrase { passphrase.is_empty() && confirm.is_empty() }
        else { !passphrase.is_empty() && passphrase == confirm }
}

impl RepositoryView {
    pub(crate) fn reload_ssh_keys(&mut self) {
        self.ssh_ui.request_id = self.ssh_ui.request_id.wrapping_add(1);
        let request_id = self.ssh_ui.request_id;
        self.ssh_ui.loading = true;
        let manager = khaslana::ssh_keys::SshKeyManager::new(self.storage.clone());
        let tx = self.tx.clone();
        let storage = self.storage.clone();
        let host_tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = storage.list_ssh_host_pins().map(|pins| pins.into_iter()
                .map(|(host, port, algorithm, raw)| (host, port, algorithm, khaslana::ssh_host_trust::fingerprint(&raw)))
                .collect()).map_err(|error| error.to_string());
            send_ui_event(&host_tx, UiEvent::SshUi(SshUiEvent::HostsLoaded { request_id, result }));
        });
        self.tasks.spawn(TaskKind::Short, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| manager.list()))
                .map_err(|_| khaslana::ui_i18n::encode(messages::FAILED, &[]))
                .and_then(|result| result.map_err(|error| error.to_string()));
            send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::Loaded { request_id, result }));
        });
    }

    pub(crate) fn generate_managed_ssh_key(&mut self) {
        if self.active_dialog != Some(DialogState::GenerateSshKey) || self.ssh_ui.generating
            || self.ssh_ui.generated_key_id.is_some() { return; }
        if !generation_form_valid(&self.ssh_ui.name.value, &self.ssh_ui.passphrase.value,
            &self.ssh_ui.passphrase_confirm.value, self.ssh_ui.no_passphrase) {
            self.last_error = Some(khaslana::ui_i18n::encode(messages::INVALID_FORM, &[]));
            return;
        }
        self.ssh_ui.generated_passphrase = (!self.ssh_ui.no_passphrase)
            .then(|| zeroize::Zeroizing::new(self.ssh_ui.passphrase.value.clone()));
        let protection = if self.ssh_ui.no_passphrase { khaslana::ssh_keys::KeyProtection::Unencrypted }
            else { khaslana::ssh_keys::KeyProtection::Passphrase(self.ssh_ui.passphrase.value.clone()) };
        let request = khaslana::ssh_keys::GenerateSshKeyRequest {
            display_name: self.ssh_ui.name.value.clone(), comment: self.ssh_ui.comment.value.clone(), protection,
        };
        self.ssh_ui.passphrase.clear();
        self.ssh_ui.passphrase_confirm.clear();
        self.ssh_ui.generating = true;
        let generation = self.ssh_ui.generation;
        let manager = khaslana::ssh_keys::SshKeyManager::new(self.storage.clone());
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            // 不记录 panic payload，以免第三方错误意外包含口令。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| manager.generate(request)))
                .map_err(|_| khaslana::ui_i18n::encode(messages::FAILED, &[]))
                .and_then(|result| result.map_err(|error| error.to_string()));
            send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::Generated { generation, result }));
        });
    }

    pub(crate) fn use_managed_ssh_key(&mut self, key_id: String) {
        let returning = self.ssh_ui.return_to_credential && self.active_dialog == Some(DialogState::GenerateSshKey);
        let generated = self.ssh_ui.generated_key_id.as_deref() == Some(key_id.as_str());
        let passphrase = if generated { self.ssh_ui.generated_passphrase.take() } else { None };
        if !returning { self.open_credential_form(); }
        self.active_dialog = Some(DialogState::CredentialForm { editing: None });
        self.set_credential_form_mode(CredentialFormMode::Ssh);
        self.ssh_ui.clear_form();
        self.ssh_ui.return_to_credential = false;
        self.ssh_ui.generated_passphrase = passphrase;
        self.ssh_ui.generation = self.ssh_ui.generation.wrapping_add(1);
        let generation = self.ssh_ui.generation;
        let manager = khaslana::ssh_keys::SshKeyManager::new(self.storage.clone());
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = manager.private_key_path(&key_id).map_err(|error| error.to_string());
            send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::KeyPath { key_id, generation, result }));
        });
    }

    fn delete_managed_ssh_key(&mut self, key_id: String) {
        let manager = khaslana::ssh_keys::SshKeyManager::new(self.storage.clone());
        let tx = self.tx.clone();
        self.active_dialog = None;
        self.request_overlay_focus_restore();
        self.tasks.spawn(TaskKind::Short, move || {
            let result = manager.delete(&key_id).map_err(|error| error.to_string());
            send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::Deleted { result }));
        });
    }

    pub(crate) fn handle_ssh_ui_event(&mut self, event: SshUiEvent, cx: &mut Context<Self>) {
        match event {
            SshUiEvent::Directory { generation, result } => {
                if generation != self.ssh_ui.generation || self.active_dialog != Some(DialogState::GenerateSshKey) { return; }
                match result { Ok(path) => self.ssh_ui.actual_directory = Some(path), Err(error) => self.last_error = Some(error) }
            }
            SshUiEvent::GeneratedDetails { generation, key_id, fingerprint, result } => {
                if generation != self.ssh_ui.generation || self.ssh_ui.generated_key_id.as_deref() != Some(key_id.as_str()) { return; }
                match result {
                    Ok(path) => { let public = path.with_extension("pub"); self.ssh_ui.generated_details = Some((fingerprint, path, public)); }
                    Err(error) => self.last_error = Some(error),
                }
            }
            SshUiEvent::Loaded { request_id, result } => {
                if request_id != self.ssh_ui.request_id { return; }
                self.ssh_ui.loading = false;
                match result { Ok(keys) => self.ssh_ui.keys = keys, Err(error) => self.last_error = Some(error) }
            }
            SshUiEvent::HostsLoaded { request_id, result } => {
                if request_id != self.ssh_ui.request_id { return; }
                match result { Ok(hosts) => self.ssh_ui.hosts = hosts, Err(error) => self.last_error = Some(error) }
            }
            SshUiEvent::Generated { generation, result } => {
                self.ssh_ui.generating = false;
                match result {
                    Ok(key) => {
                        if generation == self.ssh_ui.generation && self.active_dialog == Some(DialogState::GenerateSshKey) {
                            self.ssh_ui.generated_public_key = Some(key.public_key.clone());
                            self.ssh_ui.generated_key_id = Some(key.id.clone());
                            let manager = khaslana::ssh_keys::SshKeyManager::new(self.storage.clone());
                            let tx = self.tx.clone();
                            self.tasks.spawn(TaskKind::Short, move || {
                                let result = manager.private_key_path(&key.id).map_err(|error| error.to_string());
                                send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::GeneratedDetails { generation, key_id: key.id, fingerprint: key.fingerprint, result }));
                            });
                        }
                        self.reload_ssh_keys();
                    }
                    Err(error) => if generation == self.ssh_ui.generation { self.ssh_ui.generated_passphrase = None; self.last_error = Some(error); },
                }
            }
            SshUiEvent::KeyPath { key_id, generation, result } => {
                if generation != self.ssh_ui.generation || !matches!(self.active_dialog, Some(DialogState::CredentialForm { .. })) { return; }
                match result {
                    Ok(path) => {
                        self.use_discovered_ssh_key(path);
                        if let Some(passphrase) = self.ssh_ui.generated_passphrase.take() {
                            self.credential_passphrase.set_value(passphrase.as_str());
                        }
                        if let Some(key) = self.ssh_ui.keys.iter().find(|key| key.id == key_id) {
                            self.credential_display_name.set_value(key.display_name.clone());
                        }
                    }
                    Err(error) => { self.ssh_ui.generated_passphrase = None; self.last_error = Some(error); },
                }
            }
            SshUiEvent::Deleted { result } => { if let Err(error) = result { self.notify_error(error, cx); } self.reload_ssh_keys(); }
            SshUiEvent::Challenge { tab_id, request } => {
                self.ssh_ui.operations.register(tab_id, request.operation.clone());
                if !request.is_active()
                    || tab_id.is_some_and(|id| self.tab(id).is_none()) {
                    request.operation.cancel();
                    let _ = request.response.send(khaslana::ssh_host_trust::HostTrustDecision::Reject);
                    return;
                }
                self.ssh_ui.challenges.push_back((tab_id, request));
                self.show_next_ssh_host_challenge();
            }
        }
    }

    fn show_next_ssh_host_challenge(&mut self) {
        if matches!(self.active_dialog, Some(DialogState::VerifySshHost { .. })) { return; }
        if let Some((_, request)) = self.ssh_ui.challenges.front() {
            self.ssh_ui.previous_dialog = self.active_dialog.take();
            self.active_dialog = Some(DialogState::VerifySshHost { challenge_id: request.id });
        }
    }

    pub(crate) fn respond_ssh_host_challenge(&mut self, challenge_id: u64, trust: bool) {
        let Some((tab_id, request)) = self.ssh_ui.challenges.front() else { return; };
        if request.id != challenge_id || self.active_dialog != Some(DialogState::VerifySshHost { challenge_id }) { return; }
        let valid = request.is_active()
            && tab_id.is_none_or(|id| self.tab(id).is_some());
        let decision = if trust && valid { khaslana::ssh_host_trust::HostTrustDecision::TrustAndSave }
            else { khaslana::ssh_host_trust::HostTrustDecision::Reject };
        let _ = request.response.send(decision);
        if !trust || !valid { request.operation.cancel(); }
        self.ssh_ui.challenges.pop_front();
        if trust && valid { self.reload_ssh_keys(); }
        self.active_dialog = self.ssh_ui.previous_dialog.take();
        self.request_overlay_focus_restore();
        self.show_next_ssh_host_challenge();
    }

    pub(crate) fn cancel_ssh_host_operations_for_tab(&mut self, tab_id: RepoTabId) {
        // 先同步取消登记 token，再移除挑战。已回答的挑战虽不在队列仍在登记表中。
        self.ssh_ui.operations.cancel_tab(tab_id);
        let closing_visible = self.ssh_ui.challenges.front().is_some_and(|(tab, request)|
            *tab == Some(tab_id) && self.active_dialog == Some(DialogState::VerifySshHost { challenge_id: request.id }));
        self.ssh_ui.challenges.retain(|(tab, request)| {
            if *tab == Some(tab_id) {
                let _ = request.response.send(khaslana::ssh_host_trust::HostTrustDecision::Reject);
                false
            } else { true }
        });
        if closing_visible {
            self.active_dialog = self.ssh_ui.previous_dialog.take();
            self.request_overlay_focus_restore();
            self.show_next_ssh_host_challenge();
        }
    }

    pub(crate) fn tick_ssh_host_challenges(&mut self, cx: &mut Context<Self>) {
        self.ssh_ui.operations.prune_inactive();
        let expired = self.ssh_ui.challenges.front().and_then(|(tab_id, request)|
            (!request.is_active()
                || tab_id.is_some_and(|id| self.tab(id).is_none())).then_some(request.id));
        if let Some(id) = expired { self.respond_ssh_host_challenge(id, false); cx.notify(); }
    }

    pub(crate) fn settings_ssh_groups(&self, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        vec![SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(khaslana::ui_i18n::text(messages::KEYS),
                Some(khaslana::ui_i18n::text(messages::KEY_HELP).into())))
            .item(SettingItem::render({ let view = cx.entity(); move |_, _, cx| view.update(cx, |this, cx| {
                let mut rows = div().flex().flex_col().gap_2().min_w(px(0.0))
                    .child(div().flex().flex_wrap().gap_2()
                        .child(this.primary_button(khaslana::ui_i18n::text(messages::GENERATE), !this.ssh_ui.generating, |this, _, _| this.open_generate_ssh_key(), cx))
                        .child(this.button(khaslana::ui_i18n::text(messages::REFRESH), !this.ssh_ui.loading, |this, _, _| this.reload_ssh_keys(), cx)));
                if this.ssh_ui.keys.is_empty() { rows = rows.child(placeholder_row(khaslana::ui_i18n::text(messages::EMPTY_KEYS))); }
                for key in this.ssh_ui.keys.clone() {
                    let public_key = key.public_key.clone(); let id = key.id.clone();
                    let delete_id = key.id.clone(); let name = key.display_name.clone();
                    rows = rows.child(div().id(format!("managed-key-{}", key.id)).flex().flex_col().gap_1().py_2().border_b_1().border_color(rgb(ui_theme::BORDER_MUTED))
                        .child(div().text_size(px(ui_theme::TYPE_BODY)).child(key.display_name))
                        .child(div().text_size(px(11.0)).text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(key.fingerprint))
                        .child(div().flex().flex_wrap().gap_2()
                            .child(this.button(khaslana::ui_i18n::text(messages::COPY_PUBLIC), true, move |this, _, cx| this.copy_managed_public_key(public_key.clone(), cx), cx))
                            .child(this.button(khaslana::ui_i18n::text(messages::USE_CREDENTIAL), true, move |this, _, _| this.use_managed_ssh_key(id.clone()), cx))
                            .child(this.danger_button(khaslana::ui_i18n::text(messages::DELETE), true, move |this, _, _| {
                                this.active_dialog = Some(DialogState::DeleteSshKey { key_id: delete_id.clone(), name: name.clone() });
                            }, cx))));
                }
                rows
            }) })),
            SettingGroup::new()
                .item(crate::settings_center::settings_group_heading(khaslana::ui_i18n::text(messages::HOSTS), Some(khaslana::ui_i18n::text(messages::HOST_HELP).into())))
                .item(SettingItem::render({ let view = cx.entity(); move |_, _, cx| view.update(cx, |this, cx| {
                    let mut rows = div().flex().flex_col().gap_2().min_w(px(0.0));
                    if this.ssh_ui.hosts.is_empty() { rows = rows.child(placeholder_row(khaslana::ui_i18n::text(messages::EMPTY_HOSTS))); }
                    for (host, port, algorithm, fingerprint) in this.ssh_ui.hosts.clone() {
                        let title = format!("{host}:{port} · {algorithm}");
                        rows = rows.child(div().id(format!("trusted-host-{host}-{port}-{algorithm}")).flex().flex_col().gap_1().py_2().border_b_1().border_color(rgb(ui_theme::BORDER_MUTED))
                            .child(div().text_size(px(ui_theme::TYPE_BODY)).child(title))
                            .child(div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(11.0)).text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(fingerprint.clone()))
                            .child(this.button(khaslana::ui_i18n::text(messages::COPY_FINGERPRINT), true, move |this, _, cx| this.copy_ssh_fingerprint(fingerprint.clone(), cx), cx))
                            .child(this.danger_button(khaslana::ui_i18n::text(messages::FORGET), true, move |this, _, _| {
                                this.active_dialog = Some(DialogState::ForgetSshHost { host: host.clone(), port, algorithm: algorithm.clone() });
                            }, cx)));
                    }
                    rows
                }) }))]
    }

    pub(crate) fn render_forget_ssh_host_dialog(&self, host: String, port: u16, _algorithm: String, cx: &mut Context<Self>) -> AnyElement {
        let label = format!("{host}:{port}");
        self.dialog_panel(khaslana::ui_i18n::text(messages::FORGET_HOST), cx)
            .child(danger_callout(khaslana::ui_i18n::format(messages::FORGET_HELP, &[("host", &label)])))
            .child(dialog_actions()
                .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), true, |this, _, _| this.close_dialog(), cx))
                .child(self.danger_button(khaslana::ui_i18n::text(messages::FORGET), true, move |this, _, _| {
                    let storage = this.storage.clone(); let tx = this.tx.clone(); let host = host.clone();
                    this.close_dialog();
                    this.tasks.spawn(TaskKind::Short, move || {
                        let result = storage.delete_ssh_host_pin(&host, port).map_err(|error| error.to_string());
                        send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::Deleted { result }));
                    });
                }, cx)))
            .into_any_element()
    }

    pub(crate) fn render_delete_ssh_key_dialog(&self, key_id: String, name: String, cx: &mut Context<Self>) -> AnyElement {
        self.dialog_panel(khaslana::ui_i18n::text(messages::DELETE_KEY), cx)
            .child(danger_callout(khaslana::ui_i18n::format(messages::DELETE_HELP, &[("name", &name)])))
            .child(dialog_actions()
                .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), true, |this, _, _| this.close_dialog(), cx))
                .child(self.danger_button(khaslana::ui_i18n::text(messages::DELETE), true, move |this, _, _| this.delete_managed_ssh_key(key_id.clone()), cx)))
            .into_any_element()
    }

    pub(crate) fn render_verify_ssh_host_dialog(&self, challenge_id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some((_, request)) = self.ssh_ui.challenges.front().filter(|(_, request)| request.id == challenge_id) else { return div().into_any_element(); };
        self.dialog_panel(khaslana::ui_i18n::text(messages::UNKNOWN_HOST), cx)
            // 捕获阶段拦截 Enter，避免聚焦的信任按钮由 Kit 默认键位激活。
            .capture_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" { cx.stop_propagation(); window.prevent_default(); }
            }))
            .child(div().text_size(px(ui_theme::TYPE_BODY)).child(format!("{}:{} · {}", request.endpoint.host, request.endpoint.port, request.algorithm)))
            .child(div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(12.0)).child(request.fingerprint.clone()))
            .child(self.button(khaslana::ui_i18n::text(messages::COPY_FINGERPRINT), true, {
                let fingerprint = request.fingerprint.clone();
                move |this, _, cx| this.copy_ssh_fingerprint(fingerprint.clone(), cx)
            }, cx))
            .child(div().text_size(px(12.0)).text_color(rgb(ui_theme::CONTENT_SECONDARY)).child(khaslana::ui_i18n::text(messages::VERIFY_HELP)))
            .child(dialog_actions()
                .child(self.button(khaslana::ui_i18n::text(messages::CANCEL), true, move |this, _, _| this.respond_ssh_host_challenge(challenge_id, false), cx))
                .child(self.primary_button(khaslana::ui_i18n::text(messages::TRUST), request.is_active(),
                    move |this, _, _| this.respond_ssh_host_challenge(challenge_id, true), cx)))
            .into_any_element()
    }

    pub(crate) fn open_generate_ssh_key(&mut self) {
        if self.ssh_ui.generating { return; }
        self.ssh_ui.clear_form();
        self.ssh_ui.return_to_credential = matches!(self.active_dialog, Some(DialogState::CredentialForm { .. }));
        self.active_dialog = Some(DialogState::GenerateSshKey);
        let storage = self.storage.clone(); let tx = self.tx.clone(); let generation = self.ssh_ui.generation;
        self.tasks.spawn(TaskKind::Short, move || {
            let result = storage.managed_ssh_root().map_err(|error| error.to_string());
            send_ui_event(&tx, UiEvent::SshUi(SshUiEvent::Directory { generation, result }));
        });
        self.last_error = None;
    }

    pub(crate) fn close_ssh_key_form(&mut self) {
        self.ssh_ui.clear_form();
        self.active_dialog = self.ssh_ui.return_to_credential.then_some(DialogState::CredentialForm { editing: None });
        self.ssh_ui.return_to_credential = false;
        self.request_overlay_focus_restore();
    }

    pub(crate) fn copy_ssh_fingerprint(&mut self, fingerprint: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(fingerprint));
        self.notify_success(khaslana::ui_i18n::encode(messages::FINGERPRINT_COPIED, &[]), cx);
    }

    pub(crate) fn copy_managed_public_key(&mut self, public_key: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(public_key));
        self.notify_success(khaslana::ui_i18n::encode(messages::PUBLIC_COPIED, &[]), cx);
    }

    pub(crate) fn render_generate_ssh_key_dialog(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let generated = self.ssh_ui.generated_public_key.clone();
        let running = self.ssh_ui.generating;
        let (width, height) = dialog_panel_size(window, 560.0, 740.0);
        self.dialog_panel(khaslana::ui_i18n::text(messages::GENERATE), cx)
            .w(width).max_h(height).overflow_y_scroll().min_w(px(0.0))
            .child(div().text_size(px(ui_theme::TYPE_BODY)).text_color(rgb(ui_theme::CONTENT_SECONDARY))
                .child(khaslana::ui_i18n::text(messages::KEY_HELP)))
            .when_some(self.ssh_ui.actual_directory.as_ref(), |panel, path| panel.child(
                div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(12.0)).child(khaslana::ui_i18n::format(messages::DIRECTORY, &[("path", &path.display().to_string())]))))
            .when_some(self.ssh_ui.generated_details.as_ref(), |panel, (fingerprint, private, public)| panel
                .child(div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(12.0)).child(fingerprint.clone()))
                .child(div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(12.0)).child(khaslana::ui_i18n::format(messages::PRIVATE_PATH, &[("path", &private.display().to_string())])))
                .child(div().min_w(px(0.0)).w_full().whitespace_normal().text_size(px(12.0)).child(khaslana::ui_i18n::format(messages::PUBLIC_PATH, &[("path", &public.display().to_string())]))))
            .when(running, |panel| panel.child(div().text_size(px(ui_theme::TYPE_BODY)).child(khaslana::ui_i18n::text(messages::GENERATING))))
            .when(generated.is_none() && !running, |panel| panel
                .child(self.input(FieldId::SshKeyName, false, window, cx))
                .child(self.input(FieldId::SshKeyComment, false, window, cx))
                .when(!self.ssh_ui.no_passphrase, |panel| panel
                    .child(self.input(FieldId::SshKeyPassphrase, false, window, cx))
                    .child(self.input(FieldId::SshKeyPassphraseConfirm, false, window, cx)))
                .child(self.toggle_row("ssh-key-no-passphrase", khaslana::ui_i18n::text(messages::NO_PASSPHRASE), self.ssh_ui.no_passphrase,
                    |this, _, _| {
                        if this.ssh_ui.generating { return; }
                        this.ssh_ui.no_passphrase = !this.ssh_ui.no_passphrase;
                        this.ssh_ui.passphrase.clear();
                        this.ssh_ui.passphrase_confirm.clear();
                    }, cx)))
            .when_some(generated.clone(), |panel, public_key| panel
                .child(div().text_size(px(ui_theme::TYPE_BODY)).child(khaslana::ui_i18n::text(messages::GENERATED)))
                .child(div().text_size(px(12.0)).text_color(rgb(ui_theme::CONTENT_SECONDARY)).min_w(px(0.0)).w_full().whitespace_normal().child(public_key)))
            .child(dialog_actions().flex_wrap()
                .child(self.button(khaslana::ui_i18n::text(if generated.is_some() { messages::CLOSE } else { messages::CANCEL }), true,
                    |this, _, _| this.close_ssh_key_form(), cx))
                .when_some(generated, |actions, public_key| actions
                    .child(self.button(khaslana::ui_i18n::text(messages::COPY_PUBLIC), true,
                        move |this, _, cx| this.copy_managed_public_key(public_key.clone(), cx), cx))
                    .child(self.primary_button(khaslana::ui_i18n::text(messages::USE_CREDENTIAL), self.ssh_ui.generated_key_id.is_some(),
                        |this, _, _| {
                            if let Some(id) = this.ssh_ui.generated_key_id.clone() { this.use_managed_ssh_key(id); }
                        }, cx)))
                .when(self.ssh_ui.generated_public_key.is_none(), |actions| actions
                    .child(self.primary_button(khaslana::ui_i18n::text(if running { messages::GENERATING } else { messages::GENERATE }), !running,
                        |this, _, _| this.generate_managed_ssh_key(), cx))))
            .into_any_element()
    }
}
