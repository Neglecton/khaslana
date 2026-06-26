use super::*;

#[test]
fn default_is_disabled_and_chat_completions() {
    let settings = AiProviderSettings::default();
    assert!(!settings.enabled);
    assert_eq!(settings.api_type, AiApiType::ChatCompletions);
    assert_eq!(settings.api_type.endpoint_path(), "/chat/completions");
    assert!(!settings.is_usable());
}

#[test]
fn validate_rejects_empty_fields() {
    let mut settings = AiProviderSettings::default();
    settings.enabled = true;

    assert!(settings.validate().is_err()); // 缺 base_url 和 model

    settings.base_url = "https://api.openai.com/v1".into();
    assert!(settings.validate().is_err()); // 仍缺 model

    settings.model = "gpt-4o-mini".into();
    assert!(settings.validate().is_ok()); // api_key 可选，不填也能通过
}

#[test]
fn validate_allows_empty_api_key() {
    let mut settings = AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = "http://localhost:11434".into();
    settings.api_key = "".into(); // 本地模型如 Ollama 不需要 API Key
    settings.model = "llama3".into();
    assert!(settings.validate().is_ok());
}

#[test]
fn validate_rejects_non_http_base_url() {
    let mut settings = AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = "ftp://example.com".into();
    settings.api_key = "sk-test".into();
    settings.model = "gpt-4o-mini".into();
    let err = settings.validate().unwrap_err().to_string();
    assert!(err.contains("http://"));
}

#[test]
fn json_round_trip() {
    let mut settings = AiProviderSettings::default();
    settings.enabled = true;
    settings.base_url = "https://api.deepseek.com".into();
    settings.api_key = "sk-abc".into();
    settings.model = "deepseek-chat".into();
    settings.temperature = 0.1;
    settings.max_tokens = 1200;

    let json = serde_json::to_string(&settings).unwrap();
    let parsed: AiProviderSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.enabled, settings.enabled);
    assert_eq!(parsed.api_type, settings.api_type);
    assert_eq!(parsed.base_url, settings.base_url);
    assert_eq!(parsed.api_key, settings.api_key);
    assert_eq!(parsed.model, settings.model);
    assert_eq!(parsed.temperature, settings.temperature);
    assert_eq!(parsed.max_tokens, settings.max_tokens);
}

#[test]
fn normalized_base_url_strips_trailing_slash() {
    let mut settings = AiProviderSettings::default();
    settings.base_url = "https://api.openai.com/v1/".into();
    assert_eq!(settings.normalized_base_url(), "https://api.openai.com/v1");
}

#[test]
fn is_usable_requires_enabled_and_valid() {
    let mut settings = AiProviderSettings::default();
    settings.base_url = "https://api.openai.com/v1".into();
    settings.api_key = "sk-test".into();
    settings.model = "gpt-4o-mini".into();

    // 未启用：不可用
    assert!(!settings.is_usable());

    settings.enabled = true;
    assert!(settings.is_usable());
}
