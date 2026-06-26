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
