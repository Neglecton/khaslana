//! GitHub OAuth Device Flow 服务层。
//!
//! 通过 OAuth 2.0 设备授权流程（RFC 8628）获取用户 access_token，作为 git HTTPS
//! 认证密码，替代用户手动录入 PAT。仅使用同步 `ureq` 客户端，供后台任务线程阻塞调用，
//! 不引入异步运行时；代理设置由调用方传入，与 AI/更新等网络请求保持一致。
//!
//! Device Flow 无需 client_secret，也不需要本地回调服务器，最适合桌面应用。
//! 维护者需在 GitHub 注册一个 OAuth App 并勾选 "Enable Device Flow"。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// GitHub OAuth App 的 Client ID（Device Flow 不需要 Client Secret）。
///
/// 注册步骤：https://github.com/settings/applications/new
/// - Authorization callback URL：任意合法 URL（Device Flow 不使用回调）。
/// - 注册后在该应用设置里勾选 **Enable Device Flow**。
/// - 把得到的 Client ID 填到下面。
const GITHUB_OAUTH_CLIENT_ID: &str = "Ov23liInhZJveSEa5GFs";

/// Device Flow 申请的 OAuth scope：
/// - `repo`：覆盖私有/公有仓库的读写（含 push）。
/// - `workflow`：允许推送 `.github/workflows` 工作流文件（否则会被 GitHub 拒绝）。
const GITHUB_OAUTH_SCOPES: &str = "repo workflow";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_URL: &str = "https://api.github.com/user";

/// 是否已配置 client_id（空占位时流程直接拒绝，避免发无效请求）。
pub(crate) fn is_configured() -> bool {
    !GITHUB_OAUTH_CLIENT_ID.trim().is_empty()
}

/// 构建带代理和超时的 ureq Agent（与代理设置保持一致）。
fn build_agent(proxy_url: Option<String>) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(20));
    if let Some(url) = proxy_url
        && let Ok(proxy) = ureq::Proxy::new(&url)
    {
        builder = builder.proxy(proxy);
    }
    builder.build()
}

/// 设备码请求响应。
#[derive(Debug, Deserialize)]
pub(crate) struct DeviceCodeResponse {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    /// 已带 user_code 的完整验证地址（浏览器打开后自动预填验证码）。
    pub(crate) verification_uri_complete: Option<String>,
    pub(crate) verification_uri: Option<String>,
    /// 设备码有效期（秒）。
    pub(crate) expires_in: u64,
    /// 轮询间隔（秒）。
    pub(crate) interval: u64,
}

impl DeviceCodeResponse {
    /// 优先使用带验证码的完整地址，缺失时回退到通用验证页。
    pub(crate) fn verification_url(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .or(self.verification_uri.as_deref())
            .unwrap_or("https://github.com/login/device")
    }
}

/// 请求设备码。
pub(crate) fn request_device_code(proxy_url: Option<String>) -> Result<DeviceCodeResponse, String> {
    let agent = build_agent(proxy_url);
    let resp = agent
        .post(DEVICE_CODE_URL)
        .set("Accept", "application/json")
        .send_form(&[
            ("client_id", GITHUB_OAUTH_CLIENT_ID),
            ("scope", GITHUB_OAUTH_SCOPES),
        ])
        .map_err(|e| format!("请求设备码失败：{e}"))?;
    resp.into_json::<DeviceCodeResponse>()
        .map_err(|e| format!("解析设备码响应失败：{e}"))
}

/// GitHub 令牌端点响应（成功与待定/错误共用同一结构，按字段是否存在判定）。
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    #[allow(dead_code)]
    token_type: Option<String>,
    #[allow(dead_code)]
    scope: Option<String>,
    error: Option<String>,
    #[allow(dead_code)]
    error_description: Option<String>,
    /// `slow_down` 时返回的新轮询间隔。
    #[allow(dead_code)]
    interval: Option<u64>,
}

/// 根据令牌响应判定轮询动作：
/// - `Ok(Some(token))`：成功拿到令牌。
/// - `Ok(None)`：继续轮询（pending 或 slow_down）。
/// - `Err`：终止（过期、拒绝或未知错误）。
fn classify_token(parsed: TokenResponse, interval: &mut u64) -> Result<Option<String>, String> {
    if let Some(token) = parsed.access_token {
        return Ok(Some(token));
    }
    match parsed.error.as_deref() {
        // 缺少 error 字段但也没有 token：视为待定，继续轮询。
        None | Some("authorization_pending") => Ok(None),
        Some("slow_down") => {
            // RFC 8628：收到 slow_down 后把间隔增加 5 秒。
            *interval += 5;
            Ok(None)
        }
        Some("expired_token") => Err("设备码已过期，请重新登录".into()),
        Some("access_denied") => Err("已拒绝授权".into()),
        Some(other) => Err(format!("GitHub 返回错误：{other}")),
    }
}

/// 轮询 access token，直到成功、被取消或设备码过期。
///
/// `interval` 为初始轮询间隔（来自设备码响应），遇到 `slow_down` 会自动递增。
/// `cancel` 被置位时立即返回错误，便于 UI 取消登录。
pub(crate) fn poll_for_token(
    proxy_url: Option<String>,
    device_code: String,
    mut interval: u64,
    expires_in: u64,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let agent = build_agent(proxy_url);
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        if Instant::now() >= deadline {
            return Err("设备码已过期，请重新登录".into());
        }
        // 首次轮询前需等待一个间隔（否则 GitHub 返回 slow_down）；分段睡眠以便及时响应取消。
        for _ in 0..interval {
            if cancel.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }

        let parsed_opt = match agent
            .post(TOKEN_URL)
            .set("Accept", "application/json")
            .send_form(&[
                ("client_id", GITHUB_OAUTH_CLIENT_ID),
                ("device_code", &device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]) {
            Ok(resp) => resp.into_json::<TokenResponse>().ok(),
            // HTTP 错误（如 400 unsupported_grant_type）也尝试解析 body 中的 error。
            Err(ureq::Error::Status(_, resp)) => resp.into_json::<TokenResponse>().ok(),
            Err(ureq::Error::Transport(err)) => {
                tracing::warn!(target: "khaslana::oauth", "轮询令牌网络错误：{err}");
                None
            }
        };
        let Some(parsed) = parsed_opt else {
            // 网络瞬时错误或不可解析：等待下一轮重试（受 expires_in 兜底）。
            continue;
        };
        match classify_token(parsed, &mut interval)? {
            Some(token) => return Ok(token),
            None => continue,
        }
    }
}

/// 用 access token 获取用户登录名（作为 git 认证的用户名）。
pub(crate) fn fetch_login(proxy_url: Option<String>, token: &str) -> Result<String, String> {
    let agent = build_agent(proxy_url);
    let resp = agent
        .get(USER_URL)
        .set("Accept", "application/json")
        .set("Authorization", &format!("token {token}"))
        .set("User-Agent", "Khaslana")
        .call()
        .map_err(|e| format!("获取用户信息失败：{e}"))?;
    let user: GithubUser = resp
        .into_json()
        .map_err(|e| format!("解析用户信息失败：{e}"))?;
    user.login.ok_or_else(|| "GitHub 未返回登录名".into())
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    login: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_id_is_configured() {
        assert!(is_configured(), "GITHUB_OAUTH_CLIENT_ID 不能为空");
    }

    #[test]
    fn parse_device_code_with_complete_uri() {
        let json = r#"{
            "device_code":"dc123",
            "user_code":"ABCD-1234",
            "verification_uri":"https://github.com/login/device",
            "verification_uri_complete":"https://github.com/login/device?user_code=ABCD-1234",
            "expires_in":900,
            "interval":5
        }"#;
        let r: DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.user_code, "ABCD-1234");
        assert_eq!(r.device_code, "dc123");
        assert_eq!(
            r.verification_url(),
            "https://github.com/login/device?user_code=ABCD-1234"
        );
        assert_eq!(r.expires_in, 900);
        assert_eq!(r.interval, 5);
    }

    #[test]
    fn verification_url_falls_back_when_complete_missing() {
        let json = r#"{
            "device_code":"dc",
            "user_code":"ABCD",
            "verification_uri":"https://github.com/login/device",
            "expires_in":900,
            "interval":5
        }"#;
        let r: DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.verification_url(), "https://github.com/login/device");
    }

    #[test]
    fn classify_token_success() {
        let parsed = TokenResponse {
            access_token: Some("gho_token".into()),
            token_type: Some("bearer".into()),
            scope: Some("repo workflow".into()),
            error: None,
            error_description: None,
            interval: None,
        };
        let mut interval = 5;
        assert_eq!(
            classify_token(parsed, &mut interval).unwrap(),
            Some("gho_token".to_string())
        );
        assert_eq!(interval, 5);
    }

    #[test]
    fn classify_token_pending_keeps_interval() {
        let parsed = TokenResponse {
            access_token: None,
            token_type: None,
            scope: None,
            error: Some("authorization_pending".into()),
            error_description: None,
            interval: None,
        };
        let mut interval = 5;
        assert_eq!(classify_token(parsed, &mut interval).unwrap(), None);
        assert_eq!(interval, 5);
    }

    #[test]
    fn classify_token_slow_down_increases_interval() {
        let parsed = TokenResponse {
            access_token: None,
            token_type: None,
            scope: None,
            error: Some("slow_down".into()),
            error_description: None,
            interval: Some(10),
        };
        let mut interval = 5;
        assert_eq!(classify_token(parsed, &mut interval).unwrap(), None);
        assert_eq!(interval, 10); // +5
    }

    #[test]
    fn classify_token_expired_is_terminal() {
        let parsed = TokenResponse {
            access_token: None,
            token_type: None,
            scope: None,
            error: Some("expired_token".into()),
            error_description: None,
            interval: None,
        };
        let mut interval = 5;
        assert!(classify_token(parsed, &mut interval).is_err());
    }

    #[test]
    fn classify_token_denied_is_terminal() {
        let parsed = TokenResponse {
            access_token: None,
            token_type: None,
            scope: None,
            error: Some("access_denied".into()),
            error_description: None,
            interval: None,
        };
        let mut interval = 5;
        assert!(classify_token(parsed, &mut interval).is_err());
    }

    #[test]
    fn parse_github_user() {
        let json = r#"{"login":"octocat","id":1,"name":"The Octocat"}"#;
        let u: GithubUser = serde_json::from_str(json).unwrap();
        assert_eq!(u.login.as_deref(), Some("octocat"));
    }

    #[test]
    fn poll_returns_cancelled_when_flag_set() {
        let cancel = AtomicBool::new(true);
        let err = poll_for_token(None, "dc".into(), 5, 900, &cancel).unwrap_err();
        assert_eq!(err, "已取消");
    }
}
