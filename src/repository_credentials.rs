//! RepositoryView 的凭据、OAuth 与远端管理动作。

use crate::*;

impl RepositoryView {
    pub(crate) fn open_credential_form(&mut self) {
        self.credential_context_menu = None;
        let suggested_remote_url = self.snapshot.as_ref().and_then(|snapshot| {
            self.selected_remote
                .as_deref()
                .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
                .or_else(|| {
                    snapshot
                        .remotes
                        .iter()
                        .find(|remote| remote.name == "origin")
                })
                .or_else(|| snapshot.remotes.first())
                .map(|remote| remote.url.clone())
        });
        self.reset_credential_form();
        if let Some(url) = suggested_remote_url {
            self.credential_remote_url.set_value(url.clone());
            let mode = credential_form_mode_for_request(&CredentialRequest {
                url,
                username_from_url: None,
                allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                    | git2::CredentialType::SSH_KEY,
                repo_path: None,
                remote_name: None,
                operation_id: None,
            });
            self.credential_form_mode = mode;
            if mode == CredentialFormMode::Ssh {
                self.credential_username.set_value(
                    ssh_credentials::ssh_username_from_url(&self.credential_remote_url.value)
                        .unwrap_or_else(|| "git".into()),
                );
                self.discover_ssh_credentials_if_needed();
            }
        }
        self.active_dialog = Some(DialogState::CredentialForm { editing: None });
        self.last_error = None;
    }

    fn reset_credential_form(&mut self) {
        if self.ssh_credential_discovery.loading {
            // 关闭表单后忽略尚未完成的扫描结果，避免异步结果覆盖其他界面状态。
            self.ssh_credential_discovery.request_id = self
                .ssh_credential_discovery
                .request_id
                .wrapping_add(1)
                .max(1);
            self.ssh_credential_discovery.loading = false;
        }
        // 作废可能进行中的 OAuth 登录：递增请求号让迟到事件被忽略，并停止后台任务。
        if self.oauth_login_flow.loading {
            self.oauth_login_flow.request_id =
                self.oauth_login_flow.request_id.wrapping_add(1).max(1);
            self.oauth_login_flow.loading = false;
        }
        if let Some(cancel) = self.oauth_login_flow.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.oauth_login_flow.provider = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        self.oauth_login_flow.error = None;
        self.credential_form_mode = CredentialFormMode::Https;
        self.credential_scope = CredentialScope::RemoteUrl;
        self.credential_use_ssh_agent = false;
        self.credential_remote_url.clear();
        self.credential_username.clear();
        self.credential_secret.clear();
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        self.credential_display_name.clear();
    }

    pub(crate) fn close_credential_form(&mut self) {
        self.reset_credential_form();
        self.open_credential_manager();
        self.last_error = None;
        self.feedbacks
            .retain(|feedback| feedback.kind != AppToastKind::Error);
    }

    pub(crate) fn set_credential_form_mode(&mut self, mode: CredentialFormMode) {
        self.credential_form_mode = mode;
        self.last_error = None;
        if mode == CredentialFormMode::Ssh {
            if self.credential_username.value.trim().is_empty() {
                self.credential_username.set_value("git");
            }
            if let Some(ssh_url) = ssh_credentials::http_remote_to_ssh(
                &self.credential_remote_url.value,
                &self.credential_username.value,
            ) {
                self.credential_remote_url.set_value(ssh_url.clone());
                self.status = format!("已将适用远端地址切换为 SSH：{ssh_url}");
            }
            self.discover_ssh_credentials_if_needed();
        }
    }

    fn discover_ssh_credentials_if_needed(&mut self) {
        if self.ssh_credential_discovery.result.is_none() && !self.ssh_credential_discovery.loading
        {
            self.discover_ssh_credentials();
        }
    }

    pub(crate) fn discover_ssh_credentials(&mut self) {
        if self.ssh_credential_discovery.loading {
            return;
        }
        self.ssh_credential_discovery.request_id = self
            .ssh_credential_discovery
            .request_id
            .wrapping_add(1)
            .max(1);
        let request_id = self.ssh_credential_discovery.request_id;
        self.ssh_credential_discovery.loading = true;
        self.ssh_credential_discovery.error = None;
        self.status = "正在检测本机 SSH 身份".into();
        self.last_error = None;
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Short, move || {
            match ssh_credentials::discover_local_ssh_credentials() {
                Ok(result) => send_ui_event(
                    &tx,
                    UiEvent::SshCredentialsDiscovered { request_id, result },
                ),
                Err(error) => send_ui_event(
                    &tx,
                    UiEvent::SshCredentialDiscoveryFailed { request_id, error },
                ),
            }
        });
    }

    pub(crate) fn use_discovered_ssh_agent(&mut self) {
        self.credential_use_ssh_agent = true;
        self.credential_key_path.clear();
        self.credential_passphrase.clear();
        if self.credential_display_name.value.trim().is_empty()
            || self.credential_display_name.value.starts_with("SSH · ")
        {
            self.credential_display_name.set_value("本机 SSH Agent");
        }
        self.status = "已选择本机 SSH Agent".into();
        self.last_error = None;
    }

    pub(crate) fn use_discovered_ssh_key(&mut self, path: PathBuf) {
        if self.credential_key_path.value.trim() != path.to_string_lossy() {
            self.credential_passphrase.clear();
        }
        self.credential_use_ssh_agent = false;
        self.credential_key_path
            .set_value(path.display().to_string());
        if (self.credential_display_name.value.trim().is_empty()
            || self.credential_display_name.value == "本机 SSH Agent")
            && let Some(name) = path.file_name().and_then(|name| name.to_str())
        {
            self.credential_display_name
                .set_value(format!("SSH · {name}"));
        }
        self.status = format!("已选择 SSH 私钥：{}", path.display());
        self.last_error = None;
    }

    pub(crate) fn browse_credential_ssh_key(&mut self) {
        self.status = "正在选择 SSH 私钥...".into();
        self.last_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title("选择 SSH 私钥")
                .pick_file();
            send_ui_event(&tx, UiEvent::CredentialSshKeyFileSelected { path });
        });
    }

    /// OAuth 登录公共前置：递增 request_id、置 loading、建取消标记。返回 (request_id, cancel)。
    fn begin_oauth_login(&mut self, provider: OAuthProvider) -> (u64, Arc<AtomicBool>) {
        self.oauth_login_flow.request_id = self.oauth_login_flow.request_id.wrapping_add(1).max(1);
        let request_id = self.oauth_login_flow.request_id;
        self.oauth_login_flow.loading = true;
        self.oauth_login_flow.provider = Some(provider);
        self.oauth_login_flow.error = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.oauth_login_flow.cancel = Some(cancel.clone());
        self.last_error = None;
        (request_id, cancel)
    }

    /// 启动 GitHub OAuth Device Flow 登录：后台请求设备码 → 显示并打开浏览器 → 轮询令牌。
    pub(crate) fn start_github_login(&mut self) {
        if self.oauth_login_flow.loading {
            return;
        }
        if !oauth::is_configured() {
            self.last_error = Some("尚未配置 GitHub OAuth Client ID，请联系维护者".into());
            return;
        }
        let (request_id, cancel) = self.begin_oauth_login(OAuthProvider::Github);
        self.status = "正在请求 GitHub 设备码...".into();
        // 复用全局代理设置，与 AI/更新等网络请求保持一致。
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target("https://github.com/");
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            // 1. 请求设备码。
            let device = match oauth::request_device_code(proxy_url.clone()) {
                Ok(d) => d,
                Err(error) => {
                    send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error });
                    return;
                }
            };
            let user_code = device.user_code.clone();
            let verification_uri = device.verification_url().to_string();
            let device_code = device.device_code.clone();
            let interval = device.interval.max(1);
            let expires_in = device.expires_in;
            send_ui_event(
                &tx,
                UiEvent::OAuthLoginReady {
                    request_id,
                    provider: OAuthProvider::Github,
                    url: verification_uri,
                    user_code: Some(user_code),
                },
            );
            // 2. 轮询令牌（取消由 UI 通过 cancel 标记触发）。
            match oauth::poll_for_token(
                proxy_url.clone(),
                device_code,
                interval,
                expires_in,
                cancel.as_ref(),
            ) {
                Ok(token) => {
                    // 3. 用令牌换取登录名，作为 git 认证用户名。令牌不写日志。
                    match oauth::fetch_login(proxy_url, &token) {
                        Ok(username) => send_ui_event(
                            &tx,
                            UiEvent::OAuthLoginSucceeded {
                                request_id,
                                provider: OAuthProvider::Github,
                                username,
                                token,
                                gitee_refresh: None,
                            },
                        ),
                        Err(error) => {
                            send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error })
                        }
                    }
                }
                Err(error) => send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error }),
            }
        });
    }

    /// 启动 Gitee OAuth 授权码流登录：本地回调收 code → broker 换 token → 取登录名。
    pub(crate) fn start_gitee_login(&mut self) {
        if self.oauth_login_flow.loading {
            return;
        }
        if !oauth::is_gitee_configured() {
            self.last_error = Some("尚未配置 Gitee 登录服务（broker URL），详见 AGENTS.md".into());
            return;
        }
        let (request_id, cancel) = self.begin_oauth_login(OAuthProvider::Gitee);
        self.status = "正在等待浏览器完成 Gitee 授权...".into();
        let proxy_url = self
            .proxy_settings
            .proxy_url_for_target("https://gitee.com/");
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            let result = oauth::gitee_run_code_flow(proxy_url, cancel.as_ref(), |url| {
                // 本地回调监听已就绪，通知 UI 打开浏览器（验证码仅 GitHub 有，Gitee 为 None）。
                send_ui_event(
                    &tx,
                    UiEvent::OAuthLoginReady {
                        request_id,
                        provider: OAuthProvider::Gitee,
                        url: url.to_string(),
                        user_code: None,
                    },
                );
            });
            match result {
                Ok(grant) => send_ui_event(
                    &tx,
                    UiEvent::OAuthLoginSucceeded {
                        request_id,
                        provider: OAuthProvider::Gitee,
                        username: grant.username,
                        token: grant.access_token,
                        gitee_refresh: grant.refresh_token.zip(grant.expires_at),
                    },
                ),
                Err(error) => send_ui_event(&tx, UiEvent::OAuthLoginFailed { request_id, error }),
            }
        });
    }

    /// 取消进行中的 OAuth 登录。
    pub(crate) fn cancel_oauth_login(&mut self) {
        if let Some(cancel) = self.oauth_login_flow.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.oauth_login_flow.loading = false;
        self.oauth_login_flow.provider = None;
        self.oauth_login_flow.user_code = None;
        self.oauth_login_flow.verification_uri = None;
        self.status = "已取消登录".into();
    }

    pub(crate) fn save_credential_form(&mut self) {
        let url = self.credential_remote_url.value.trim().to_string();
        if url.is_empty() {
            self.last_error = Some("需要填写适用远端 URL".into());
            return;
        }
        let inferred_mode = credential_form_mode_for_request(&CredentialRequest {
            url: url.clone(),
            username_from_url: None,
            allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                | git2::CredentialType::SSH_KEY,
            repo_path: None,
            remote_name: None,
            operation_id: None,
        });
        if inferred_mode != self.credential_form_mode {
            self.last_error = Some(match self.credential_form_mode {
                CredentialFormMode::Ssh => {
                    "当前“适用远端 URL”不是 SSH 地址，请使用 git@主机:仓库路径 或 ssh:// 地址"
                        .into()
                }
                CredentialFormMode::Https => {
                    "当前“适用远端 URL”不是 HTTP(S) 地址，请切换到 SSH 凭据或修改地址".into()
                }
            });
            return;
        }
        let username = self
            .credential_username
            .value
            .trim()
            .to_string()
            .if_empty_then(|| "git".into());
        let display_name = optional_display_name(&self.credential_display_name.value);
        let credential = match self.credential_form_mode {
            CredentialFormMode::Https => {
                if self.credential_secret.value.is_empty() {
                    self.last_error = Some("需要填写密码或 PAT".into());
                    return;
                }
                GitCredential::UserPass {
                    username,
                    secret: self.credential_secret.value.clone(),
                    display_name,
                    save_to_keyring: true,
                    scope: self.credential_scope,
                }
            }
            CredentialFormMode::Ssh => {
                let key_path = self.credential_key_path.value.trim().to_string();
                if !self.credential_use_ssh_agent && key_path.is_empty() {
                    self.last_error = Some("需要填写 SSH 私钥路径或选择使用 SSH agent".into());
                    return;
                }
                if !self.credential_use_ssh_agent
                    && let Err(error) =
                        ssh_credentials::validate_ssh_private_key_path(Path::new(&key_path))
                {
                    self.last_error = Some(error);
                    return;
                }
                GitCredential::SshPassphrase {
                    username,
                    private_key_path: (!self.credential_use_ssh_agent).then_some(key_path),
                    passphrase: (!self.credential_passphrase.value.is_empty())
                        .then(|| self.credential_passphrase.value.clone()),
                    display_name,
                    save_to_keyring: true,
                    scope: self.credential_scope,
                }
            }
        };
        let request = CredentialRequest {
            url,
            username_from_url: Some(credential.username().to_string()),
            allowed_types: match self.credential_form_mode {
                CredentialFormMode::Https => git2::CredentialType::USER_PASS_PLAINTEXT,
                CredentialFormMode::Ssh => git2::CredentialType::SSH_KEY,
            },
            repo_path: None,
            remote_name: None,
            operation_id: None,
        };
        match self.credential_store.save_record(&request, &credential) {
            Ok(record) => {
                self.reset_credential_form();
                self.open_credential_manager();
                self.last_error = None;
                self.reload_credential_records("凭据已添加");
                self.pending_gitee_refresh_record = Some(record.id);
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }
    }

    pub(crate) fn open_remote_manager(&mut self) {
        if self.repo_path.is_none() {
            self.last_error = Some("请先打开一个仓库".into());
            return;
        }
        self.close_popups();
        self.active_dialog = Some(DialogState::RemoteManager);
        self.reload_credential_records("远端管理已打开");
    }

    pub(crate) fn open_remote_form(&mut self, editing: Option<String>) {
        let remote = match editing.as_ref() {
            Some(name) => {
                let Some(snapshot) = self.snapshot.as_ref() else {
                    self.last_error = Some("请先打开一个仓库".into());
                    return;
                };
                let Some(remote) = snapshot.remotes.iter().find(|remote| remote.name == *name)
                else {
                    self.last_error = Some("远端不存在".into());
                    return;
                };
                Some(remote.clone())
            }
            None => None,
        };

        self.remote_credential_policy = RemoteCredentialPolicy::AutoMatch;
        if let Some(remote) = remote {
            self.remote_name.set_value(remote.name.clone());
            self.remote_url.set_value(remote.url.clone());
            if let Some(repo_path) = self.repo_path.as_ref() {
                self.remote_credential_policy =
                    self.remote_credential_policy_for_remote(repo_path, &remote.name, &remote.url);
            }
        } else {
            self.remote_name.clear();
            self.remote_url.clear();
        }
        self.active_dialog = Some(DialogState::RemoteForm { editing });
        self.last_error = None;
    }

    pub(crate) fn open_delete_remote_confirm(&mut self, name: String) {
        self.active_dialog = Some(DialogState::ConfirmDeleteRemote { name });
        self.last_error = None;
    }

    pub(crate) fn open_delete_remote_branch_confirm(&mut self, remote_branch: String) {
        let Some((remote, branch)) = remote_branch.split_once('/') else {
            self.last_error = Some(format!("远端分支名称无效：{remote_branch}"));
            return;
        };
        self.branch_context_menu = None;
        self.active_dialog = Some(DialogState::ConfirmDeleteRemoteBranch {
            remote: remote.to_string(),
            branch: branch.to_string(),
        });
        self.last_error = None;
    }

    pub(crate) fn save_remote(&mut self, editing: Option<String>) {
        let name = self.remote_name.value.trim().to_string();
        let url = self.remote_url.value.trim().to_string();
        if name.is_empty() {
            self.last_error = Some("需要填写远端名称".into());
            return;
        }
        if url.is_empty() {
            self.last_error = Some("需要填写远端地址".into());
            return;
        }
        if name.contains(char::is_whitespace) || name.contains('\\') || name.starts_with('-') {
            self.last_error = Some(format!("远端名称无效：{name}"));
            return;
        }

        let existing_remotes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.remotes.clone())
            .unwrap_or_default();
        if editing.as_deref() != Some(name.as_str())
            && existing_remotes.iter().any(|remote| remote.name == name)
        {
            self.last_error = Some(format!("远端名称已存在：{name}"));
            return;
        }

        let selected_record =
            if let RemoteCredentialPolicy::Record(id) = &self.remote_credential_policy {
                let Some(record) = self
                    .credential_records
                    .iter()
                    .find(|record| record.id == *id)
                    .cloned()
                else {
                    self.last_error = Some("所选凭据记录不存在".into());
                    return;
                };
                Some(record)
            } else {
                None
            };
        if let Some(record) = selected_record.as_ref() {
            let compatible = match record.scope {
                CredentialScope::RemoteUrl => {
                    credential_record_is_compatible_with_url(record, &url)
                }
                CredentialScope::Host => credential_record_matches_remote_url(record, &url),
            };
            if !compatible {
                self.last_error = Some("所选凭据与远端地址协议或站点不匹配".into());
                return;
            }
            if record.scope == CredentialScope::RemoteUrl
                && let Err(err) = self
                    .credential_store
                    .update_record_remote_url(&record.id, &url)
            {
                self.last_error = Some(err.to_string());
                return;
            }
            if record.scope == CredentialScope::Host
                && let Err(err) = self.credential_store.touch_record(&record.id)
            {
                self.last_error = Some(err.to_string());
                return;
            }
            self.reload_credential_records("远端凭据绑定已更新");
        }

        if let Some(repo_path) = self.repo_path.as_ref() {
            let request = CredentialRequest {
                url: url.clone(),
                username_from_url: None,
                allowed_types: git2::CredentialType::USER_PASS_PLAINTEXT
                    | git2::CredentialType::SSH_KEY,
                repo_path: Some(repo_path.clone()),
                remote_name: Some(name.clone()),
                operation_id: None,
            };
            set_remote_binding_for_request(
                &self.remote_credential_bindings,
                &request,
                self.remote_credential_policy.clone(),
            );
            self.save_remote_credential_bindings();
        }

        let old_selected = self.selected_remote.clone();
        if let Some(old_name) = editing.as_ref() {
            if old_selected.as_deref() == Some(old_name.as_str()) {
                self.selected_remote = Some(name.clone());
            }
        }
        let new_name = name.clone();
        match editing {
            Some(old_name) => {
                self.with_repo("远端已更新", move |service, repo| {
                    service.update_remote(
                        repo,
                        &RemoteName::new(old_name),
                        &RemoteName::new(new_name),
                        &url,
                    )
                });
            }
            None => {
                self.selected_remote = Some(name.clone());
                self.with_repo("远端已新增", move |service, repo| {
                    service.add_remote(repo, &RemoteName::new(name), &url)
                });
            }
        }
    }

    pub(crate) fn delete_remote(&mut self, name: String) {
        if self.selected_remote.as_deref() == Some(name.as_str()) {
            self.selected_remote = None;
        }
        self.with_repo("远端已删除", move |service, repo| {
            service.delete_remote(repo, &RemoteName::new(name))
        });
    }

    pub(crate) fn delete_remote_branch(&mut self, remote: String, branch: String) {
        self.with_repo("远端分支已删除", move |service, repo| {
            service.delete_remote_branch(repo, &RemoteName::new(remote), &BranchName::new(branch))
        });
    }

    pub(crate) fn reload_credential_records(&mut self, message: &'static str) {
        self.credential_context_menu = None;
        match self.credential_store.list_records() {
            Ok(records) => {
                self.credential_records = records;
                self.status = message.to_string();
                self.last_error = None;
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }
    }

    pub(crate) fn open_delete_credential_confirm(&mut self, record_id: String, label: String) {
        self.active_dialog = Some(DialogState::ConfirmDeleteCredential { record_id, label });
        self.credential_context_menu = None;
        self.last_error = None;
    }

    pub(crate) fn open_credential_details(&mut self, record_id: String) {
        self.credential_context_menu = None;
        self.active_dialog = Some(DialogState::CredentialDetails { record_id });
        self.last_error = None;
    }

    pub(crate) fn open_credential_context_menu(
        &mut self,
        record_id: String,
        event: &MouseDownEvent,
        window: &Window,
    ) {
        self.branch_context_menu = None;
        self.change_context_menu = None;
        self.tag_context_menu = None;
        self.stash_context_menu = None;
        self.commit_context_menu = None;
        self.encoding_menu_target = None;
        let (x, y) =
            clamped_menu_position(event, window, CREDENTIAL_MENU_WIDTH, CREDENTIAL_MENU_HEIGHT);
        self.reset_context_menu_selection();
        self.credential_context_menu = Some(CredentialContextMenu { record_id, x, y });
    }

    pub(crate) fn copy_credential_text(
        &mut self,
        text: Option<String>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            self.last_error = Some(format!("{label}为空，无法复制"));
            self.credential_context_menu = None;
            self.notify_warning(format!("{label}为空，无法复制"), cx);
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status = format!("已复制{label}");
        self.last_error = None;
        self.credential_context_menu = None;
        self.notify_success(self.status.clone(), cx);
    }

    pub(crate) fn delete_credential_record(&mut self, record_id: String) {
        match self.credential_store.delete_record(&record_id) {
            Ok(()) => {
                self.open_credential_manager();
                self.credential_context_menu = None;
                self.reload_credential_records("凭据已删除");
            }
            Err(err) => {
                self.open_credential_manager();
                self.last_error = Some(err.to_string());
            }
        }
    }

    /// 打开凭据测试地址确认弹窗：预填记录保存的远端地址，用户可改成
    /// 其它地址（典型：裸主机地址换成真实仓库地址）再开始测试。
    pub(crate) fn open_test_credential_dialog(&mut self, record_id: String) {
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        self.credential_test_error = None;
        self.credential_test_url
            .set_value(record.remote_url.clone());
        self.active_dialog = Some(DialogState::TestCredential { record_id });
    }

    /// 凭据测试弹窗的「开始测试」：非空 → 协议族 → HTTPS 同站点校验，
    /// 全过才关窗发起连接；任一失败写弹窗内错误条、不关窗。
    pub(crate) fn confirm_test_credential(&mut self) {
        let Some(DialogState::TestCredential { record_id }) = self.active_dialog.clone() else {
            return;
        };
        let Some(record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            // 记录列表已刷新导致记录消失：直接关窗提示。
            self.close_dialog();
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        let url = self.credential_test_url.value.trim().to_string();
        if let Err(error) = validate_credential_test_url(record.kind, &record.host, &url) {
            self.credential_test_error = Some(error);
            return;
        }
        self.credential_test_error = None;
        self.close_dialog();
        self.test_credential_record(record_id, url);
    }

    fn test_credential_record(&mut self, record_id: String, url: String) {
        if self.busy || self.global_busy_tab.is_some() {
            self.last_error = Some("已有操作正在运行".into());
            return;
        }
        let Some(mut record) = self
            .credential_records
            .iter()
            .find(|record| record.id == record_id)
            .cloned()
        else {
            self.last_error = Some("凭据记录不存在".into());
            return;
        };
        // 连接目标用弹窗确认的地址（记录本体保持原值，测试不改凭据）。
        record.remote_url = url;
        self.begin_global_test_busy("正在测试凭据连接");
        let store: Arc<dyn CredentialStore> = self.credential_store.clone();
        let tx = self.tx.clone();
        self.tasks.spawn(TaskKind::Long, move || {
            match test_credential_connection(store.as_ref(), &record) {
                Ok(()) => {
                    let records = store.list_records().unwrap_or_default();
                    send_ui_event(
                        &tx,
                        UiEvent::CredentialRecordsLoaded {
                            records,
                            message: "凭据测试通过".to_string(),
                        },
                    );
                }
                Err(err) => {
                    send_ui_event(
                        &tx,
                        UiEvent::OperationFailed {
                            tab_id: None,
                            error: err.to_string(),
                        },
                    );
                }
            }
        });
    }

    pub(crate) fn matching_credential_for_remote_url(
        &self,
        url: &str,
    ) -> Option<&CredentialRecord> {
        self.credential_records
            .iter()
            .filter(|record| credential_record_matches_remote_url(record, url))
            .max_by(|a, b| {
                let a_scope = match a.scope {
                    CredentialScope::RemoteUrl => 1,
                    CredentialScope::Host => 0,
                };
                let b_scope = match b.scope {
                    CredentialScope::RemoteUrl => 1,
                    CredentialScope::Host => 0,
                };
                a_scope
                    .cmp(&b_scope)
                    .then_with(|| a.last_used.unwrap_or(0).cmp(&b.last_used.unwrap_or(0)))
                    .then_with(|| a.updated_at.cmp(&b.updated_at))
            })
    }

    pub(crate) fn remote_credential_policy_for_remote(
        &self,
        repo_path: &Path,
        remote_name: &str,
        remote_url: &str,
    ) -> RemoteCredentialPolicy {
        let (repo_key, remote_key) = remote_binding_key(repo_path, remote_name);
        self.remote_credential_bindings
            .lock()
            .ok()
            .and_then(|bindings| {
                bindings
                    .remotes
                    .iter()
                    .find(|binding| {
                        binding.repo_path == repo_key
                            && binding.remote_name == remote_key
                            && normalize_remote_url(&binding.remote_url)
                                == normalize_remote_url(remote_url)
                    })
                    .map(|binding| binding.policy.clone())
            })
            .unwrap_or(RemoteCredentialPolicy::AutoMatch)
    }
}
