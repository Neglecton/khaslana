use git2::{FetchOptions, ProxyOptions, PushOptions};
use serde::{Deserialize, Serialize};

use crate::types::{GitError, Result};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct NetworkProxySettings {
    pub mode: NetworkProxyMode,
    pub custom: CustomProxySettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum NetworkProxyMode {
    #[default]
    Disabled,
    System,
    Custom,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CustomProxySettings {
    pub http_proxy: String,
    pub https_proxy: String,
    pub socks5_proxy: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteProtocol {
    Http,
    Https,
    Ssh,
    Other,
}

impl Default for NetworkProxySettings {
    fn default() -> Self {
        Self {
            mode: NetworkProxyMode::Disabled,
            custom: CustomProxySettings::default(),
        }
    }
}

impl NetworkProxySettings {
    /// 返回给定目标 URL 应使用的代理地址（供 ureq 等通用 HTTP 客户端使用）。
    ///
    /// - Disabled：None（直连）。
    /// - System：读取 http_proxy/https_proxy 环境变量，返回非空值。
    /// - Custom：按目标协议选择 https_proxy/http_proxy/socks5_proxy。
    ///
    /// 与 git2 的 ProxyOptions 不同，这里返回字符串供非 libgit2 的 HTTP 客户端使用。
    pub fn proxy_url_for_target(&self, target_url: &str) -> Option<String> {
        match self.mode {
            NetworkProxyMode::Disabled => None,
            NetworkProxyMode::System => system_proxy_for_url(target_url),
            NetworkProxyMode::Custom => self.custom.proxy_for_remote(Some(target_url)),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.mode == NetworkProxyMode::Custom {
            self.custom.validate()?;
        }
        Ok(())
    }

    pub fn apply_to_fetch_options<'a>(
        &self,
        options: &mut FetchOptions<'a>,
        remote_url: Option<&str>,
    ) -> Result<()> {
        if let Some(proxy) = self.proxy_options_for_remote(remote_url)? {
            options.proxy_options(proxy);
        }
        Ok(())
    }

    pub fn apply_to_push_options<'a>(
        &self,
        options: &mut PushOptions<'a>,
        remote_url: Option<&str>,
    ) -> Result<()> {
        if let Some(proxy) = self.proxy_options_for_remote(remote_url)? {
            options.proxy_options(proxy);
        }
        Ok(())
    }

    pub(crate) fn proxy_options_for_remote<'a>(
        &self,
        remote_url: Option<&str>,
    ) -> Result<Option<ProxyOptions<'a>>> {
        match self.mode {
            NetworkProxyMode::Disabled => Ok(Some(ProxyOptions::new())),
            NetworkProxyMode::System => {
                let mut options = ProxyOptions::new();
                // 系统代理交给 libgit2 的 GIT_PROXY_AUTO：Git 配置优先，其次环境变量。
                if remote_url.map(remote_protocol) != Some(RemoteProtocol::Ssh) {
                    options.auto();
                }
                Ok(Some(options))
            }
            NetworkProxyMode::Custom => {
                self.custom.validate()?;
                let Some(url) = self.custom.proxy_for_remote(remote_url) else {
                    return Ok(Some(ProxyOptions::new()));
                };
                validate_proxy_url(&url, ProxyFieldKind::Any)?;
                Ok(Some(proxy_options_from_url(url)))
            }
        }
    }

    pub fn describe_for_remote(&self, remote_url: Option<&str>) -> String {
        match self.mode {
            NetworkProxyMode::Disabled => "不使用代理".into(),
            NetworkProxyMode::System => "使用 Git 配置或环境变量代理".into(),
            NetworkProxyMode::Custom => self
                .custom
                .proxy_for_remote(remote_url)
                .map(|url| format!("使用自定义代理 {url}"))
                .unwrap_or_else(|| "未匹配到自定义代理，直连".into()),
        }
    }
}

impl CustomProxySettings {
    pub fn validate(&self) -> Result<()> {
        validate_optional_proxy_url(&self.http_proxy, ProxyFieldKind::Http)?;
        validate_optional_proxy_url(&self.https_proxy, ProxyFieldKind::Https)?;
        validate_optional_proxy_url(&self.socks5_proxy, ProxyFieldKind::Socks5)?;
        Ok(())
    }

    pub fn normalized(&self) -> Self {
        Self {
            http_proxy: self.http_proxy.trim().to_string(),
            https_proxy: self.https_proxy.trim().to_string(),
            socks5_proxy: self.socks5_proxy.trim().to_string(),
        }
    }

    pub fn proxy_for_remote(&self, remote_url: Option<&str>) -> Option<String> {
        let settings = self.normalized();
        let protocol = remote_url
            .map(remote_protocol)
            .unwrap_or(RemoteProtocol::Other);
        let candidates = match protocol {
            RemoteProtocol::Http => [settings.http_proxy.as_str(), settings.socks5_proxy.as_str()],
            RemoteProtocol::Https => [
                settings.https_proxy.as_str(),
                settings.socks5_proxy.as_str(),
            ],
            RemoteProtocol::Ssh => [settings.socks5_proxy.as_str(), ""],
            RemoteProtocol::Other => ["", ""],
        };
        first_non_empty(candidates)
    }
}

fn proxy_options_from_url<'a>(url: impl AsRef<str>) -> ProxyOptions<'a> {
    let mut options = ProxyOptions::new();
    options.url(url.as_ref());
    options
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
}

/// 读取系统/环境变量代理：https URL 取 https_proxy/HTTPS_PROXY，
/// 其它取 http_proxy/HTTP_PROXY；ALL_PROXY 作为兜底。
fn system_proxy_for_url(target_url: &str) -> Option<String> {
    let protocol = remote_protocol(target_url);
    let env_keys: &[&str] = match protocol {
        RemoteProtocol::Https => &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"],
        _ => &["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"],
    };
    env_keys
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        .map(|value| value.trim().to_string())
}

#[derive(Clone, Copy)]
enum ProxyFieldKind {
    Http,
    Https,
    Socks5,
    Any,
}

fn validate_optional_proxy_url(value: &str, kind: ProxyFieldKind) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    validate_proxy_url(value, kind)
}

fn validate_proxy_url(value: &str, kind: ProxyFieldKind) -> Result<()> {
    if value.contains('\0') {
        return Err(GitError::Message("代理地址不能包含空字符".into()));
    }
    let lower = value.trim().to_ascii_lowercase();
    let valid = match kind {
        ProxyFieldKind::Http | ProxyFieldKind::Https => {
            lower.starts_with("http://") || lower.starts_with("https://")
        }
        ProxyFieldKind::Socks5 => lower.starts_with("socks5://") || lower.starts_with("socks5h://"),
        ProxyFieldKind::Any => {
            lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("socks5://")
                || lower.starts_with("socks5h://")
        }
    };
    if valid {
        Ok(())
    } else {
        Err(GitError::Message(
            "代理地址协议不支持，请使用 http://、https://、socks5:// 或 socks5h://".into(),
        ))
    }
}

fn remote_protocol(url: &str) -> RemoteProtocol {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("http://") {
        RemoteProtocol::Http
    } else if lower.starts_with("https://") {
        RemoteProtocol::Https
    } else if lower.starts_with("ssh://") || looks_like_scp_ssh_url(&lower) {
        RemoteProtocol::Ssh
    } else {
        RemoteProtocol::Other
    }
}

fn looks_like_scp_ssh_url(value: &str) -> bool {
    let Some(at) = value.find('@') else {
        return false;
    };
    let Some(colon) = value[at + 1..].find(':') else {
        return false;
    };
    !value[..at].contains("://") && colon > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_proxy_settings_disable_proxy() {
        let settings: NetworkProxySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.mode, NetworkProxyMode::Disabled);
        assert_eq!(settings.custom, CustomProxySettings::default());
    }

    #[test]
    fn proxy_settings_accept_missing_custom_fields() {
        let settings: NetworkProxySettings = serde_json::from_str(
            r#"{"mode":"Custom","custom":{"https_proxy":"http://127.0.0.1:7890"}}"#,
        )
        .unwrap();
        assert_eq!(settings.mode, NetworkProxyMode::Custom);
        assert_eq!(settings.custom.http_proxy, "");
        assert_eq!(settings.custom.https_proxy, "http://127.0.0.1:7890");
        assert_eq!(settings.custom.socks5_proxy, "");
    }

    #[test]
    fn custom_proxy_validates_supported_protocols() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: CustomProxySettings {
                http_proxy: "http://127.0.0.1:7890".into(),
                https_proxy: "https://proxy.example:443".into(),
                socks5_proxy: "socks5h://127.0.0.1:1080".into(),
            },
        };
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn custom_proxy_rejects_wrong_field_protocols() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: CustomProxySettings {
                http_proxy: "socks5://127.0.0.1:1080".into(),
                ..Default::default()
            },
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn custom_proxy_rejects_invalid_protocol_only_when_custom_mode() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Disabled,
            custom: CustomProxySettings {
                http_proxy: "socks5://127.0.0.1:1080".into(),
                ..Default::default()
            },
        };
        assert!(settings.validate().is_ok());

        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: settings.custom,
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn custom_proxy_selects_proxy_by_remote_protocol() {
        let custom = CustomProxySettings {
            http_proxy: "http://http-proxy:8080".into(),
            https_proxy: "http://https-proxy:8080".into(),
            socks5_proxy: "socks5://127.0.0.1:1080".into(),
        };
        assert_eq!(
            custom.proxy_for_remote(Some("http://example.com/repo.git")),
            Some("http://http-proxy:8080".into())
        );
        assert_eq!(
            custom.proxy_for_remote(Some("https://example.com/repo.git")),
            Some("http://https-proxy:8080".into())
        );
        assert_eq!(
            custom.proxy_for_remote(Some("git@example.com:team/repo.git")),
            Some("socks5://127.0.0.1:1080".into())
        );
    }

    #[test]
    fn custom_proxy_falls_back_to_socks5_for_https_when_specific_missing() {
        let custom = CustomProxySettings {
            socks5_proxy: "socks5://127.0.0.1:1080".into(),
            ..Default::default()
        };
        assert_eq!(
            custom.proxy_for_remote(Some("https://example.com/repo.git")),
            Some("socks5://127.0.0.1:1080".into())
        );
    }

    #[test]
    fn custom_proxy_does_not_apply_socks5_to_file_remote() {
        let custom = CustomProxySettings {
            socks5_proxy: "socks5://127.0.0.1:1080".into(),
            ..Default::default()
        };
        assert_eq!(custom.proxy_for_remote(Some("file:///tmp/repo.git")), None);
    }

    #[test]
    fn proxy_url_for_target_disabled_returns_none() {
        let settings = NetworkProxySettings::default();
        assert_eq!(
            settings.proxy_url_for_target("https://api.openai.com"),
            None
        );
    }

    #[test]
    fn proxy_url_for_target_custom_https_picks_https_proxy() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: CustomProxySettings {
                https_proxy: "http://127.0.0.1:7890".into(),
                ..Default::default()
            },
        };
        assert_eq!(
            settings.proxy_url_for_target("https://api.openai.com"),
            Some("http://127.0.0.1:7890".into())
        );
    }

    #[test]
    fn proxy_url_for_target_custom_http_picks_http_proxy() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: CustomProxySettings {
                http_proxy: "http://127.0.0.1:7890".into(),
                ..Default::default()
            },
        };
        assert_eq!(
            settings.proxy_url_for_target("http://localhost:1234"),
            Some("http://127.0.0.1:7890".into())
        );
    }

    #[test]
    fn proxy_url_for_target_custom_falls_back_to_socks5() {
        let settings = NetworkProxySettings {
            mode: NetworkProxyMode::Custom,
            custom: CustomProxySettings {
                socks5_proxy: "socks5://127.0.0.1:1080".into(),
                ..Default::default()
            },
        };
        assert_eq!(
            settings.proxy_url_for_target("https://api.openai.com"),
            Some("socks5://127.0.0.1:1080".into())
        );
    }
}
