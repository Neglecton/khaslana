// AI 供应商配置：接口类型、连接参数、模型选择。
//
// 第一版只支持 OpenAI Chat Completions 兼容接口；AiApiType 预留扩展，
// 后续可加入 Responses、Ollama Generate 等接口类型。

use serde::{Deserialize, Serialize};

use crate::types::GitError;
use crate::types::Result as KhaslanaResult;

/// AI 供应商接口类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiApiType {
    #[default]
    /// OpenAI 兼容的 `/chat/completions` 接口。
    ChatCompletions,
    // 未来：Responses、OllamaGenerate 等。
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiProviderSettings {
    /// 是否启用 AI 功能；默认关闭，用户配置后显式开启。
    pub enabled: bool,
    pub api_type: AiApiType,
    /// 例如 https://api.openai.com/v1 或 https://api.deepseek.com。
    pub base_url: String,
    /// API Key；第一版明文存配置，后续可迁移到 keyring。
    pub api_key: String,
    /// 模型名，例如 gpt-4o-mini / deepseek-chat。
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// 请求超时秒数，覆盖网络较慢或 reasoning 模型长输出场景。
    pub request_timeout_secs: u64,
}

impl AiApiType {
    /// 该接口类型对应的 API 路径（相对于 base_url）。
    pub fn endpoint_path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "/chat/completions",
        }
    }

    /// 设置弹窗中显示的接口类型中文名。
    pub fn label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "Chat Completions (/chat/completions)",
        }
    }
}

impl Default for AiProviderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            api_type: AiApiType::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            temperature: 0.3,
            max_tokens: 800,
            request_timeout_secs: 60,
        }
    }
}

impl AiProviderSettings {
    /// 校验配置字段；返回中文错误文案以便直接展示给用户。
    pub fn validate(&self) -> KhaslanaResult<()> {
        if self.base_url.trim().is_empty() {
            return Err(GitError::Message("请填写 Base URL".into()));
        }
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err(GitError::Message(
                "Base URL 必须以 http:// 或 https:// 开头".into(),
            ));
        }
        // API Key 允许为空（部分自部署/本地模型如 Ollama 不需要鉴权）。
        if self.model.trim().is_empty() {
            return Err(GitError::Message("请填写模型名称".into()));
        }
        Ok(())
    }

    /// 是否已配置到可用状态（启用且字段校验通过）。
    pub fn is_usable(&self) -> bool {
        self.enabled && self.validate().is_ok()
    }

    /// 去掉 base_url 末尾多余的斜杠，避免拼出 `//chat/completions`。
    pub fn normalized_base_url(&self) -> String {
        self.base_url.trim().trim_end_matches('/').to_string()
    }
}

#[cfg(test)]
#[path = "../tests/ai/config.rs"]
mod tests;
