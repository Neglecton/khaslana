use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use directories::UserDirs;
use gpui::{Context, IntoElement, div, prelude::*, px};

use crate::ui::theme::rgb;
use crate::{RepositoryView, ui::theme as ui_theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalSshKey {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) from_ssh_config: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SshDiscoveryResult {
    pub(crate) keys: Vec<LocalSshKey>,
    pub(crate) agent_identities: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SshCredentialDiscoveryState {
    pub(crate) loading: bool,
    pub(crate) request_id: u64,
    pub(crate) result: Option<SshDiscoveryResult>,
    pub(crate) error: Option<String>,
}

/// 只读取公钥注释、私钥文件头和 SSH 配置路径，不读取或保存私钥正文。
pub(crate) fn discover_local_ssh_credentials() -> Result<SshDiscoveryResult, String> {
    let user_dirs = UserDirs::new().ok_or_else(|| "无法定位当前用户主目录".to_string())?;
    Ok(discover_local_ssh_credentials_in(user_dirs.home_dir()))
}

fn discover_local_ssh_credentials_in(home: &Path) -> SshDiscoveryResult {
    let ssh_dir = home.join(".ssh");
    let config_paths = ssh_config_identity_files(&ssh_dir.join("config"), home);
    let mut paths = BTreeSet::new();

    if let Ok(entries) = fs::read_dir(&ssh_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && looks_like_private_key(&path) {
                paths.insert(path);
            }
        }
    }
    for path in &config_paths {
        if path.is_file() && looks_like_private_key(path) {
            paths.insert(path.clone());
        }
    }

    let mut keys = paths
        .into_iter()
        .map(|path| {
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("SSH 私钥")
                .to_string();
            LocalSshKey {
                from_ssh_config: config_paths.contains(&path),
                path,
                label,
            }
        })
        .collect::<Vec<_>>();
    keys.sort_by(|a, b| {
        b.from_ssh_config
            .cmp(&a.from_ssh_config)
            .then_with(|| a.label.cmp(&b.label))
    });

    SshDiscoveryResult {
        keys,
        agent_identities: ssh_agent_identities(),
    }
}

fn ssh_config_identity_files(config_path: &Path, home: &Path) -> BTreeSet<PathBuf> {
    let Ok(config) = fs::read_to_string(config_path) else {
        return BTreeSet::new();
    };
    config
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            let (key, value) = line.split_once(char::is_whitespace)?;
            if !key.eq_ignore_ascii_case("IdentityFile") {
                return None;
            }
            let value = value.trim().trim_matches(['"', '\'']);
            if value.is_empty() || value.contains('%') {
                return None;
            }
            Some(expand_ssh_path(value, home))
        })
        .collect()
}

fn expand_ssh_path(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home.join(relative);
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}

fn looks_like_private_key(path: &Path) -> bool {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pub")) {
        return false;
    }
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut sample = Vec::with_capacity(4096);
    if file.take(4096).read_to_end(&mut sample).is_err() {
        return false;
    }
    sample
        .windows(b"PRIVATE KEY-----".len())
        .any(|window| window == b"PRIVATE KEY-----")
}

pub(crate) fn validate_ssh_private_key_path(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("SSH 私钥文件不存在或不是普通文件".into());
    }
    if !looks_like_private_key(path) {
        return Err("所选文件不是可识别的 OpenSSH/PEM 私钥，请勿选择 .pub 公钥文件".into());
    }
    Ok(())
}

pub(crate) fn ssh_username_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        return authority
            .rsplit_once('@')
            .map(|(username, _)| username)
            .filter(|username| !username.is_empty())
            .map(str::to_string);
    }
    let (username, host_and_path) = url.split_once('@')?;
    (host_and_path.contains(':') && !username.is_empty()).then(|| username.to_string())
}

/// 将常见的 HTTP(S) Git 远端地址转换为 SCP 风格 SSH 地址。
pub(crate) fn http_remote_to_ssh(url: &str, username: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let (authority, path) = rest.split_once('/')?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if authority.starts_with('[') {
        authority.split_once(']')?.0.trim_start_matches('[')
    } else {
        authority.split(':').next()?
    };
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    let username = username.trim();
    let username = if username.is_empty() { "git" } else { username };
    if host.contains(':') {
        Some(format!("ssh://{username}@[{host}]/{path}"))
    } else {
        Some(format!("{username}@{host}:{path}"))
    }
}

fn ssh_agent_identities() -> Vec<String> {
    let mut command = Command::new("ssh-add");
    command.arg("-l");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

impl RepositoryView {
    pub(crate) fn render_ssh_credential_discovery(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = &self.ssh_credential_discovery;
        let detect_label = if state.loading {
            "正在检测本机 SSH..."
        } else {
            "检测本机 SSH"
        };
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .child(
                div()
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
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child("本机 SSH 身份"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("检测 ~/.ssh 私钥、SSH config 的 IdentityFile 和 Agent 已加载身份"),
                            ),
                    )
                    .child(self.button(
                        detect_label,
                        !state.loading && !self.busy,
                        |this, _, _| this.discover_ssh_credentials(),
                        cx,
                    )),
            );

        if let Some(error) = state.error.clone() {
            panel = panel.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::DESTRUCTIVE))
                    .child(error),
            );
        }

        if let Some(result) = state.result.as_ref() {
            if !result.agent_identities.is_empty() {
                let count = result.agent_identities.len();
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .rounded_sm()
                        .bg(rgb(ui_theme::ACCENT))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .child(format!("SSH Agent · 已加载 {count} 个身份")),
                        )
                        .child(self.button(
                            "一键使用 Agent",
                            !self.busy,
                            |this, _, _| this.use_discovered_ssh_agent(),
                            cx,
                        )),
                );
            }

            for key in result.keys.iter().take(8) {
                let path = key.path.clone();
                let source = if key.from_ssh_config {
                    "SSH config"
                } else {
                    "~/.ssh"
                };
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(ui_theme::BORDER))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(ui_theme::FOREGROUND))
                                        .child(format!("{} · {source}", key.label)),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.0))
                                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                        .child(path.display().to_string()),
                                ),
                        )
                        .child(self.button(
                            "一键使用",
                            !self.busy,
                            move |this, _, _| this.use_discovered_ssh_key(path.clone()),
                            cx,
                        )),
                );
            }

            if result.agent_identities.is_empty() && result.keys.is_empty() && !state.loading {
                panel = panel.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("未发现可用身份，可使用下方“选择私钥文件”手动指定。"),
                );
            }
        }

        panel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_private_keys_and_config_identity_files() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(
            ssh.join("id_ed25519"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n",
        )
        .unwrap();
        fs::write(ssh.join("id_ed25519.pub"), "ssh-ed25519 public").unwrap();
        fs::write(
            ssh.join("work_key"),
            "-----BEGIN RSA PRIVATE KEY-----\nsecret\n",
        )
        .unwrap();
        fs::write(
            ssh.join("config"),
            "Host work\n  IdentityFile ~/.ssh/work_key\n",
        )
        .unwrap();

        let result = discover_local_ssh_credentials_in(temp.path());

        assert_eq!(result.keys.len(), 2);
        assert!(
            result
                .keys
                .iter()
                .any(|key| key.label == "work_key" && key.from_ssh_config)
        );
        assert!(!result.keys.iter().any(|key| key.label.ends_with(".pub")));
        assert!(validate_ssh_private_key_path(&ssh.join("id_ed25519")).is_ok());
        assert!(validate_ssh_private_key_path(&ssh.join("id_ed25519.pub")).is_err());
    }

    #[test]
    fn expands_home_and_relative_identity_paths() {
        let home = Path::new("C:/Users/tester");
        assert_eq!(
            expand_ssh_path("~/.ssh/id_ed25519", home),
            home.join(".ssh/id_ed25519")
        );
        assert_eq!(expand_ssh_path("keys/work", home), home.join("keys/work"));
    }

    #[test]
    fn extracts_username_from_common_ssh_urls() {
        assert_eq!(
            ssh_username_from_url("git@github.com:owner/repo.git").as_deref(),
            Some("git")
        );
        assert_eq!(
            ssh_username_from_url("ssh://alice@example.com/repo").as_deref(),
            Some("alice")
        );
        assert_eq!(ssh_username_from_url("https://example.com/repo"), None);
    }

    #[test]
    fn converts_common_https_remote_to_ssh() {
        assert_eq!(
            http_remote_to_ssh("https://github.com/owner/repo.git", "git").as_deref(),
            Some("git@github.com:owner/repo.git")
        );
        assert_eq!(
            http_remote_to_ssh("https://git.example.com:8443/team/repo.git", "alice")
                .as_deref(),
            Some("alice@git.example.com:team/repo.git")
        );
        assert_eq!(http_remote_to_ssh("git@example.com:repo.git", "git"), None);
    }
}
