use super::*;

#[test]
fn proxy_mode_help_keeps_each_mode_distinct() {
    assert!(proxy_mode_help(NetworkProxyMode::Disabled).contains("显式直连"));
    assert!(proxy_mode_help(NetworkProxyMode::System).contains("自动代理"));
    assert!(proxy_mode_help(NetworkProxyMode::Custom).contains("SOCKS5"));
}

#[test]
fn proxy_mode_disabled_reason_explains_busy_state() {
    assert_eq!(
        proxy_mode_disabled_reason(false),
        Some("当前操作进行中，请稍候")
    );
    assert_eq!(proxy_mode_disabled_reason(true), None);
}
