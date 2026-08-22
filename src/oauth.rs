//! OAuth 快速登录服务层（GitHub Device Flow + Gitee 授权码流）。
//!
//! 通过浏览器授权获取用户 access_token，作为 git HTTPS 认证密码，替代用户手动录入
//! PAT。仅使用同步 `ureq` 客户端，供后台任务线程阻塞调用，不引入异步运行时；代理设置
//! 由调用方传入，与 AI/更新等网络请求保持一致。
//!
//! - GitHub：Device Flow（RFC 8628），无需 client_secret、无需本地回调服务器。维护者
//!   需在 GitHub 注册 OAuth App 并勾选 "Enable Device Flow"。
//! - Gitee：授权码流 + 本地回调（127.0.0.1:17890）。Gitee 不支持 Device Flow/PKCE，
//!   公开客户端不能内置 client_secret，故 token 交换由部署在 EdgeOne 等边缘平台的
//!   broker 代办（见独立仓库 khaslana-broker 的 `edge-functions/gitee.js`），客户端只持 client_id + broker URL。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
///
/// 代理 URL 无效时返回错误而不是静默直连：用户显式配置的代理被绕过会暴露
/// 真实网络路径，必须让请求失败并提示。未配置时显式传 `None`，关闭 ureq 3
/// 默认的环境变量代理自动检测（系统代理模式由调用方读取环境变量后传入）。
fn build_agent(proxy_url: Option<String>) -> Result<ureq::Agent, String> {
    let proxy = proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| {
            ureq::Proxy::new(url).map_err(|err| format!("代理配置无效，已取消 OAuth 请求：{err}"))
        })
        .transpose()?;
    let builder = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(20)))
        .timeout_send_body(Some(Duration::from_secs(20)))
        .proxy(proxy);
    Ok(builder.build().new_agent())
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
    let agent = build_agent(proxy_url)?;
    let mut resp = agent
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .send_form([
            ("client_id", GITHUB_OAUTH_CLIENT_ID),
            ("scope", GITHUB_OAUTH_SCOPES),
        ])
        .map_err(|e| format!("请求设备码失败：{e}"))?;
    resp.body_mut()
        .read_json::<DeviceCodeResponse>()
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

/// 轮询令牌专用 Agent：关闭“HTTP 状态码转错误”，因为 GitHub 在待定/过期时
/// 返回 400 且错误详情在响应体里，需要拿到完整响应自行解析。
fn build_token_poll_agent(proxy_url: Option<String>) -> Result<ureq::Agent, String> {
    let proxy = proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| {
            ureq::Proxy::new(url).map_err(|err| format!("代理配置无效，已取消 OAuth 请求：{err}"))
        })
        .transpose()?;
    let builder = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(20)))
        .timeout_send_body(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .proxy(proxy);
    Ok(builder.build().new_agent())
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
    let agent = build_token_poll_agent(proxy_url)?;
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

        // 400 响应（如 unsupported_grant_type）的 body 里也带 error 字段，
        // 因此不做状态码判断，统一尝试解析响应体。
        let parsed_opt = match agent
            .post(TOKEN_URL)
            .header("Accept", "application/json")
            .send_form([
                ("client_id", GITHUB_OAUTH_CLIENT_ID),
                ("device_code", &device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]) {
            Ok(mut resp) => resp.body_mut().read_json::<TokenResponse>().ok(),
            // http_status_as_error 已关闭，能走到 Err 的都是网络层错误；
            // 瞬时失败等待下一轮重试（受 expires_in 兜底）。
            Err(err) => {
                tracing::warn!(target: "khaslana::oauth", "轮询令牌请求错误：{err}");
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
    let agent = build_agent(proxy_url)?;
    let mut resp = agent
        .get(USER_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", "Khaslana")
        .call()
        .map_err(|e| format!("获取用户信息失败：{e}"))?;
    let user: GithubUser = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("解析用户信息失败：{e}"))?;
    user.login.ok_or_else(|| "GitHub 未返回登录名".into())
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    login: Option<String>,
}

// ── Gitee OAuth 授权码流（本地回调 + broker 代换 token）─────────────────────

/// Gitee OAuth App 的 Client ID（公开标识，可放客户端）。
const GITEE_OAUTH_CLIENT_ID: &str =
    "078a2744281d89a8527ba545243ee9b7d9840a1f1465000c8ab9945ba2f07911";
/// broker（EdgeOne 边缘函数，见独立仓库 khaslana-broker 的 `edge-functions/gitee.js`）URL。部署后填入；为空时 Gitee 登录禁用。
/// broker 持有 client_secret，替客户端用授权码换 token，避免把 secret 放进公开发布的客户端。
const GITEE_BROKER_URL: &str = "https://khaslana-broker.suhoan.cn/gitee";
/// 本地回调服务器监听端口与回调地址（必须在 Gitee OAuth 应用里原样登记）。
const GITEE_CALLBACK_PORT: u16 = 17890;
const GITEE_REDIRECT_URI: &str = "http://localhost:17890/callback";
/// Gitee OAuth scope：`projects` 仓库读写，`user_info` 取登录名。
const GITEE_OAUTH_SCOPES: &str = "projects user_info";

const GITEE_AUTHORIZE_URL: &str = "https://gitee.com/oauth/authorize";
const GITEE_USER_API: &str = "https://gitee.com/api/v5/user";

/// Gitee 快速登录是否就绪：需要 client_id 和 broker URL 均已配置。
pub(crate) fn is_gitee_configured() -> bool {
    !GITEE_OAUTH_CLIENT_ID.trim().is_empty() && !GITEE_BROKER_URL.trim().is_empty()
}

/// 构造 Gitee 授权页地址（浏览器打开，用户登录授权后回调到本地 17890）。
pub(crate) fn gitee_authorize_url(state: &str) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        GITEE_AUTHORIZE_URL,
        GITEE_OAUTH_CLIENT_ID,
        url_encode(GITEE_REDIRECT_URI),
        url_encode(GITEE_OAUTH_SCOPES),
        url_encode(state),
    )
}

/// 运行 Gitee 授权码流：本地回调收 code → broker 换 token → 取登录名。
///
/// `on_ready` 在本地监听就绪后回调一次（用于打开浏览器，避免浏览器早于监听）。
/// 成功返回 `(access_token, 登录名)`。
pub(crate) fn gitee_run_code_flow<F>(
    proxy_url: Option<String>,
    cancel: &AtomicBool,
    on_ready: F,
) -> Result<GiteeLoginGrant, String>
where
    F: FnOnce(&str),
{
    let state = random_state();
    // 1. 绑定本地回调监听。
    let listener = TcpListener::bind(("127.0.0.1", GITEE_CALLBACK_PORT))
        .map_err(|e| format!("无法监听本地回调端口 {GITEE_CALLBACK_PORT}：{e}（可能被占用）"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("设置回调监听非阻塞失败：{e}"))?;

    let authorize_url = gitee_authorize_url(&state);
    // 2. 监听已就绪，通知调用方打开浏览器。
    on_ready(&authorize_url);

    // 3. 等待回调，最多 180s，期间响应取消。
    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        if Instant::now() >= deadline {
            return Err("等待浏览器授权超时".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                if let Some((code, cb_state, err)) = read_callback_request(&mut stream) {
                    let _ = respond_callback_success(&mut stream);
                    if let Some(err) = err {
                        return Err(format!("Gitee 拒绝授权：{err}"));
                    }
                    if cb_state.as_deref() != Some(state.as_str()) {
                        return Err("授权 state 校验失败，已放弃以防 CSRF".into());
                    }
                    break code.ok_or_else(|| "回调未携带授权码".to_string())?;
                }
                // 没读到有效请求行：继续等下一个连接。
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("回调监听错误：{e}")),
        }
    };

    // 4. 通过 broker 用 code 换 token（客户端不带 client_secret）。
    let grant = exchange_via_broker(proxy_url.clone(), &code)?;
    let access_token = grant.access_token.clone();
    // 5. 取登录名。
    let login = fetch_gitee_login(proxy_url, &access_token)?;
    Ok(GiteeLoginGrant {
        access_token,
        username: login,
        refresh_token: grant.refresh_token,
        expires_at: grant.expires_at,
    })
}

/// Gitee 登录成功后的一次性令牌授予（access_token + 可选的自动续期材料）。
///
/// `refresh_token`/`expires_at` 仅在 broker（Gitee）返回时存在；刷新必须经
/// broker 代办（需要 client_secret，客户端不持有）。
#[derive(Clone, Debug)]
pub(crate) struct GiteeLoginGrant {
    pub(crate) access_token: String,
    pub(crate) username: String,
    pub(crate) refresh_token: Option<String>,
    /// access_token 过期时间（Unix 秒）；None = broker 未返回 expires_in。
    pub(crate) expires_at: Option<i64>,
}

/// broker 刷新响应（与换码响应同构：新 access_token + 可能轮换的 refresh_token）。
#[derive(Clone, Debug)]
pub(crate) struct GiteeRefreshedGrant {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_at: Option<i64>,
}

/// 距过期不足该秒数时触发自动刷新（提前量覆盖一次远端操作的时长）。
pub(crate) const GITEE_REFRESH_MARGIN_SECS: i64 = 2 * 3600;

/// 是否需要自动刷新：过期时间已知且距现在不足提前量（含已过期）。
pub(crate) fn gitee_needs_refresh(expires_at: i64, now_secs: i64) -> bool {
    expires_at - now_secs <= GITEE_REFRESH_MARGIN_SECS
}

/// 经 broker 用 refresh_token 换新 access_token（broker 持 client_secret
/// 向 Gitee 发起 `grant_type=refresh_token`）。失败返回面向用户的中文错误。
pub(crate) fn gitee_refresh_via_broker(
    proxy_url: Option<String>,
    refresh_token: &str,
) -> Result<GiteeRefreshedGrant, String> {
    let agent = build_agent(proxy_url)?;
    let body = serde_json::json!({
        "refresh_token": refresh_token,
    });
    let mut resp = agent
        .post(GITEE_BROKER_URL)
        .header("Accept", "application/json")
        .send_json(&body)
        .map_err(|e| format!("刷新请求失败（broker）：{e}"))?;
    let parsed: BrokerTokenResponse = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("解析刷新响应失败：{e}"))?;
    let error_text = parsed
        .error_description
        .clone()
        .or_else(|| parsed.error.clone())
        .unwrap_or_else(|| "broker 未返回新令牌".to_string());
    parsed.into_grant().ok_or(error_text)
}

/// 读取回调 HTTP 请求，解析出 `(code, state, error)`。
///
/// 循环读取直到收齐请求头（`\r\n\r\n`）或超长放弃，避免 TCP 分片时单次 read
/// 读不完整就误判为无效请求。同时校验 Host 头必须是本机回调地址，防止
/// 恶意网页借 DNS rebinding 把伪造请求投给本地端口。
fn read_callback_request(
    stream: &mut TcpStream,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    const MAX_REQUEST_BYTES: usize = 8192;
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_REQUEST_BYTES {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let req = std::str::from_utf8(&buf).ok()?;
    // 请求行形如：GET /callback?code=xxx&state=yyy HTTP/1.1
    let request_line = req.lines().next()?;
    let path = request_line.split_whitespace().nth(1)?;

    // Host 校验：只接受本机回调地址，阻断 DNS rebinding 场景下
    // 攻击者域名指向本机端口的伪造回调。
    let host_ok = req.lines().skip(1).any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if !name.trim().eq_ignore_ascii_case("host") {
            return false;
        }
        let host = value.trim();
        host == "localhost:17890" || host == "127.0.0.1:17890"
    });
    if !host_ok {
        return None;
    }

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    Some((
        query_param(query, "code"),
        query_param(query, "state"),
        query_param(query, "error"),
    ))
}

/// 回一个最小 HTML 告知用户登录成功、可关闭页面。
fn respond_callback_success(stream: &mut TcpStream) -> std::io::Result<()> {
    let body = "<!doctype html><meta charset='utf-8'><title>登录成功</title>\
<body style='font-family:sans-serif;text-align:center;margin-top:60px'>\
登录成功，可以关闭此页面返回 Khaslana。</body>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

/// 通过 broker 用授权码换 token（broker 持有 client_secret 向 Gitee 交换）。
fn exchange_via_broker(
    proxy_url: Option<String>,
    code: &str,
) -> Result<GiteeRefreshedGrant, String> {
    let agent = build_agent(proxy_url)?;
    let body = serde_json::json!({
        "code": code,
        "redirect_uri": GITEE_REDIRECT_URI,
    });
    let mut resp = agent
        .post(GITEE_BROKER_URL)
        .header("Accept", "application/json")
        .send_json(&body)
        .map_err(|e| format!("令牌交换失败（broker）：{e}"))?;
    let parsed: BrokerTokenResponse = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("解析令牌响应失败：{e}"))?;
    // 优先用 Gitee 返回的中文 error_description，便于用户理解。
    let error_text = parsed
        .error_description
        .clone()
        .or_else(|| parsed.error.clone())
        .unwrap_or_else(|| "broker 未返回令牌".to_string());
    parsed.into_grant().ok_or(error_text)
}

/// broker 响应体：换码与刷新共用（Gitee 原样字段 + broker 透传）。
#[derive(Debug, Deserialize)]
struct BrokerTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

impl BrokerTokenResponse {
    fn into_grant(self) -> Option<GiteeRefreshedGrant> {
        let expires_at = self.expires_in.map(|secs| now_unix_secs() + secs.max(0));
        self.access_token.map(|access_token| GiteeRefreshedGrant {
            access_token,
            refresh_token: self.refresh_token,
            expires_at,
        })
    }
}

/// 当前 Unix 秒。
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 用 access_token 获取 Gitee 登录名（作为 git 认证用户名）。
///
/// token 走 `Authorization: token` 头而不是 URL query，避免令牌进入
/// 服务端/中间层访问日志。
fn fetch_gitee_login(proxy_url: Option<String>, token: &str) -> Result<String, String> {
    let agent = build_agent(proxy_url)?;
    let mut resp = agent
        .get(GITEE_USER_API)
        .header("Authorization", format!("token {token}"))
        .call()
        .map_err(|e| format!("获取 Gitee 用户信息失败：{e}"))?;
    let user: GiteeUser = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("解析 Gitee 用户信息失败：{e}"))?;
    user.login.ok_or_else(|| "Gitee 未返回登录名".into())
}

#[derive(Debug, Deserialize)]
struct GiteeUser {
    login: Option<String>,
}

/// 生成 CSRF state：128 位 OS CSPRNG 随机数，足够不可预测。
///
/// CSPRNG 不可用时退化为时间戳+计数器方案（仅削弱防伪造强度，不阻断登录）。
fn random_state() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    } else {
        tracing::warn!(target: "khaslana::oauth", "CSPRNG 不可用，state 退化为时间戳方案");
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{nanos:x}{n:x}")
    }
}

/// 在查询串中取某个 key 的值（已百分号解码）。
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

/// 百分号解码（`%XX` 与 `+`→空格）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把字符串编码为 application/x-www-form-urlencoded 片段（仅保留 unreserved 字符）。
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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

    #[test]
    fn gitee_authorize_url_contains_required_params() {
        let url = gitee_authorize_url("xyz");
        assert!(url.starts_with("https://gitee.com/oauth/authorize?"));
        assert!(url.contains("client_id="));
        // redirect_uri 的冒号/斜杠必须被编码
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A17890%2Fcallback"));
        // scope 的空格必须被编码
        assert!(url.contains("scope=projects%20user_info"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=xyz"));
    }

    #[test]
    fn url_encode_encodes_spaces_and_specials() {
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("https://x"), "https%3A%2F%2Fx");
        assert_eq!(url_encode("ab-_~1"), "ab-_~1");
    }

    #[test]
    fn query_param_and_percent_decode() {
        let q = "code=abc&state=xyz123";
        assert_eq!(query_param(q, "code").as_deref(), Some("abc"));
        assert_eq!(query_param(q, "state").as_deref(), Some("xyz123"));
        assert!(query_param(q, "missing").is_none());

        // 百分号解码
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
    }

    #[test]
    fn read_callback_request_parses_code_and_state() {
        // 模拟一段回调 HTTP 请求。
        let req = "GET /callback?code=abcdef&state=st123 HTTP/1.1\r\nHost: localhost\r\n\r\n";
        // read_callback_request 需要一个真实 TcpStream，这里用内存管道不现实，
        // 因此直接验证其依赖的查询解析逻辑。
        let path = req.lines().next().unwrap();
        let p = path.split_whitespace().nth(1).unwrap();
        let query = p.split_once('?').map(|(_, q)| q).unwrap_or("");
        assert_eq!(query_param(query, "code").as_deref(), Some("abcdef"));
        assert_eq!(query_param(query, "state").as_deref(), Some("st123"));
    }

    #[test]
    fn broker_token_response_parses_success_and_error() {
        let ok: BrokerTokenResponse = serde_json::from_str(
            r#"{"access_token":"gtoken","refresh_token":"rnew","expires_in":86400}"#,
        )
        .unwrap();
        assert_eq!(ok.access_token.as_deref(), Some("gtoken"));
        assert_eq!(ok.refresh_token.as_deref(), Some("rnew"));
        assert_eq!(ok.expires_in, Some(86400));
        assert!(ok.error.is_none());

        let err: BrokerTokenResponse =
            serde_json::from_str(r#"{"error":"invalid_grant","error_description":"bad code"}"#)
                .unwrap();
        assert!(err.access_token.is_none());
        assert_eq!(err.error.as_deref(), Some("invalid_grant"));
    }

    #[test]
    fn broker_response_into_grant_computes_expires_at() {
        let ok: BrokerTokenResponse = serde_json::from_str(
            r#"{"access_token":"gtoken","refresh_token":"rnew","expires_in":86400}"#,
        )
        .unwrap();
        let grant = ok.into_grant().expect("access_token 存在时应产出 grant");
        assert_eq!(grant.access_token, "gtoken");
        assert_eq!(grant.refresh_token.as_deref(), Some("rnew"));
        // expires_at = now + expires_in，允许计算与断言之间的毫秒级误差
        let now = now_unix_secs();
        let expected = now + 86400;
        assert!(
            (expected - 5..=expected + 5).contains(&grant.expires_at.unwrap()),
            "expires_at 应约为 now+86400，实际 {:?}",
            grant.expires_at
        );

        // 无 expires_in：expires_at 为 None（未知过期时间，不做自动刷新）
        let no_expiry: BrokerTokenResponse =
            serde_json::from_str(r#"{"access_token":"gtoken"}"#).unwrap();
        let grant = no_expiry.into_grant().unwrap();
        assert!(grant.expires_at.is_none());

        // 无 access_token：不产出 grant（走错误文案路径）
        let no_token: BrokerTokenResponse =
            serde_json::from_str(r#"{"refresh_token":"r"}"#).unwrap();
        assert!(no_token.into_grant().is_none());
    }

    #[test]
    fn gitee_needs_refresh_margin_boundaries() {
        let now = 1_700_000_000i64;
        // 距过期恰好等于提前量：需要刷新（边界含等号）
        assert!(gitee_needs_refresh(now + GITEE_REFRESH_MARGIN_SECS, now));
        // 提前量 + 1 秒：不需要
        assert!(!gitee_needs_refresh(
            now + GITEE_REFRESH_MARGIN_SECS + 1,
            now
        ));
        // 已过期：需要刷新
        assert!(gitee_needs_refresh(now - 1, now));
        // 全新令牌（24h，远大于 2h 余量）：不需要
        assert!(!gitee_needs_refresh(now + 86400, now));
    }

    #[test]
    fn parse_gitee_user() {
        let u: GiteeUser = serde_json::from_str(r#"{"login":"someone","id":42}"#).unwrap();
        assert_eq!(u.login.as_deref(), Some("someone"));
    }

    #[test]
    fn random_state_is_uniqueish_and_nonempty() {
        let a = random_state();
        let b = random_state();
        assert!(!a.is_empty());
        // 计数器递增，两次不同
        assert_ne!(a, b);
    }
}
