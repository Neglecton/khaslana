use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use super::protocol::{FrameError, read_lsp_frame, write_lsp_frame};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspNotification {
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    Notification(LspNotification),
    ProtocolError(String),
    Closed,
}

#[derive(Debug, Error)]
pub enum LspClientError {
    #[error("LSP 客户端已关闭")]
    Closed,
    #[error("LSP 请求已取消")]
    Cancelled,
    #[error("LSP 请求超时")]
    Timeout,
    #[error("LSP RPC 错误 {code}：{message}")]
    Rpc { code: i64, message: String },
    #[error("LSP 协议错误：{0}")]
    Protocol(String),
}

impl From<FrameError> for LspClientError {
    fn from(value: FrameError) -> Self {
        Self::Protocol(value.to_string())
    }
}

#[derive(Clone, Default)]
pub struct RequestCancellation(Arc<AtomicBool>);

impl RequestCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 服务端反向请求的受控策略。这里只实现客户端实际承诺的能力。
#[derive(Clone)]
pub struct ServerRequestPolicy {
    inner: Arc<PolicyInner>,
}

struct PolicyInner {
    settings: Value,
    registrations: Mutex<HashMap<String, String>>,
    diagnostic_errors: Mutex<HashMap<String, usize>>,
    ready: AtomicBool,
}

impl Default for ServerRequestPolicy {
    fn default() -> Self {
        Self::new(Value::Object(Default::default()))
    }
}

impl ServerRequestPolicy {
    pub fn new(settings: Value) -> Self {
        Self {
            inner: Arc::new(PolicyInner {
                settings,
                registrations: Mutex::new(HashMap::new()),
                diagnostic_errors: Mutex::new(HashMap::new()),
                ready: AtomicBool::new(false),
            }),
        }
    }

    pub fn service_ready(&self) -> bool {
        self.inner.ready.load(Ordering::Acquire)
    }

    pub fn mark_importing(&self) {
        self.inner.ready.store(false, Ordering::Release);
    }

    pub fn watched_files_registered(&self) -> bool {
        self.inner
            .registrations
            .lock()
            .expect("LSP 注册表锁被污染")
            .values()
            .any(|method| method == "workspace/didChangeWatchedFiles")
    }

    pub fn has_diagnostic_errors(&self) -> bool {
        self.inner
            .diagnostic_errors
            .lock()
            .expect("LSP 诊断状态锁被污染")
            .values()
            .any(|count| *count > 0)
    }

    fn observe_notification(&self, notification: &LspNotification) {
        if notification.method == "language/status"
            && notification.params.get("type").and_then(Value::as_str) == Some("ServiceReady")
        {
            self.inner.ready.store(true, Ordering::Release);
        }
        if notification.method == "textDocument/publishDiagnostics" {
            let uri = notification
                .params
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let errors = notification
                .params
                .get("diagnostics")
                .and_then(Value::as_array)
                .map_or(0, |diagnostics| {
                    diagnostics
                        .iter()
                        .filter(|diagnostic| {
                            diagnostic.get("severity").and_then(Value::as_u64) == Some(1)
                        })
                        .count()
                });
            self.inner
                .diagnostic_errors
                .lock()
                .expect("LSP 诊断状态锁被污染")
                .insert(uri, errors);
        }
    }

    fn respond(&self, method: &str, params: &Value) -> Result<Value, RpcResponseError> {
        match method {
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(Value::Array(
                    items
                        .iter()
                        .map(|item| {
                            item.get("section").and_then(Value::as_str).map_or_else(
                                || self.inner.settings.clone(),
                                |section| setting_section(&self.inner.settings, section),
                            )
                        })
                        .collect(),
                ))
            }
            "client/registerCapability" => {
                let registrations = params
                    .get("registrations")
                    .and_then(Value::as_array)
                    .ok_or_else(|| RpcResponseError::invalid_params("缺少 registrations"))?;
                let mut accepted = Vec::new();
                for registration in registrations {
                    let id = registration
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| RpcResponseError::invalid_params("注册缺少 id"))?;
                    let registered_method = registration
                        .get("method")
                        .and_then(Value::as_str)
                        .ok_or_else(|| RpcResponseError::invalid_params("注册缺少 method"))?;
                    if registered_method != "workspace/didChangeWatchedFiles" {
                        return Err(RpcResponseError::method_not_found(format!(
                            "未实现动态能力：{registered_method}"
                        )));
                    }
                    accepted.push((id.to_string(), registered_method.to_string()));
                }
                let mut current = self.inner.registrations.lock().expect("LSP 注册表锁被污染");
                current.extend(accepted);
                Ok(Value::Null)
            }
            "client/unregisterCapability" => {
                let registrations = params
                    .get("unregisterations")
                    .or_else(|| params.get("unregistrations"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| RpcResponseError::invalid_params("缺少 unregisterations"))?;
                let mut current = self.inner.registrations.lock().expect("LSP 注册表锁被污染");
                for registration in registrations {
                    if let Some(id) = registration.get("id").and_then(Value::as_str) {
                        current.remove(id);
                    }
                }
                Ok(Value::Null)
            }
            "window/workDoneProgress/create" => Ok(Value::Null),
            "workspace/applyEdit" => Ok(json!({
                "applied": false,
                "failureReason": "Khaslana 的语义查询服务是只读的"
            })),
            "window/showMessageRequest" => Ok(Value::Null),
            _ => Err(RpcResponseError::method_not_found(format!(
                "未实现服务端请求：{method}"
            ))),
        }
    }
}

fn setting_section(settings: &Value, section: &str) -> Value {
    let mut value = settings;
    for part in section.split('.') {
        let Some(next) = value.get(part) else {
            return Value::Null;
        };
        value = next;
    }
    value.clone()
}

struct RpcResponseError {
    code: i64,
    message: String,
}

impl RpcResponseError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
        }
    }
}

type PendingSender = mpsc::Sender<Result<Value, LspClientError>>;

struct ClientInner {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Mutex<HashMap<i64, PendingSender>>,
    next_id: AtomicI64,
    closed: AtomicBool,
    events: Mutex<mpsc::Receiver<ClientEvent>>,
    policy: ServerRequestPolicy,
}

#[derive(Clone)]
pub struct LspClient {
    inner: Arc<ClientInner>,
}

impl LspClient {
    pub fn start<R, W>(reader: R, writer: W, policy: ServerRequestPolicy) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::channel();
        let inner = Arc::new(ClientInner {
            writer: Mutex::new(Box::new(writer)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
            events: Mutex::new(event_rx),
            policy,
        });
        let reader_inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("khaslana-lsp-reader".to_string())
            .spawn(move || reader_loop(reader, reader_inner, event_tx))
            .expect("无法启动 LSP 读取线程");
        Self { inner }
    }

    pub fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: &RequestCancellation,
    ) -> Result<Value, LspClientError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(LspClientError::Closed);
        }
        if cancellation.is_cancelled() {
            return Err(LspClientError::Cancelled);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inner
            .pending
            .lock()
            .expect("LSP pending 锁被污染")
            .insert(id, tx);
        if let Err(error) = self.write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })) {
            self.remove_pending(id);
            return Err(error);
        }

        let deadline = Instant::now() + timeout;
        loop {
            if cancellation.is_cancelled() {
                self.cancel_request(id);
                return Err(LspClientError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                self.cancel_request(id);
                return Err(LspClientError::Timeout);
            }
            let wait = (deadline - now).min(Duration::from_millis(20));
            match rx.recv_timeout(wait) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.remove_pending(id);
                    return Err(LspClientError::Closed);
                }
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), LspClientError> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    pub fn try_events(&self) -> Vec<ClientEvent> {
        let receiver = self.inner.events.lock().expect("LSP 事件队列锁被污染");
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    pub fn policy(&self) -> &ServerRequestPolicy {
        &self.inner.policy
    }

    pub fn shutdown(&self, timeout: Duration) -> Result<(), LspClientError> {
        let result = self.request(
            "shutdown",
            Value::Null,
            timeout,
            &RequestCancellation::default(),
        );
        let _ = self.notify("exit", Value::Null);
        result.map(|_| ())
    }

    fn cancel_request(&self, id: i64) {
        self.remove_pending(id);
        let _ = self.notify("$/cancelRequest", json!({"id": id}));
    }

    fn remove_pending(&self, id: i64) {
        self.inner
            .pending
            .lock()
            .expect("LSP pending 锁被污染")
            .remove(&id);
    }

    fn write(&self, value: Value) -> Result<(), LspClientError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(LspClientError::Closed);
        }
        let mut writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| LspClientError::Protocol("LSP 写锁被污染".to_string()))?;
        write_lsp_frame(&mut *writer, &value).map_err(Into::into)
    }
}

fn reader_loop<R: Read>(reader: R, inner: Arc<ClientInner>, events: mpsc::Sender<ClientEvent>) {
    let mut reader = BufReader::new(reader);
    loop {
        match read_lsp_frame(&mut reader) {
            Ok(Some(message)) => dispatch_message(&inner, &events, message),
            Ok(None) => break,
            Err(error) => {
                let _ = events.send(ClientEvent::ProtocolError(error.to_string()));
                fail_all_pending(&inner, error.to_string());
                break;
            }
        }
    }
    inner.closed.store(true, Ordering::Release);
    fail_all_pending(&inner, "LSP 连接已关闭".to_string());
    let _ = events.send(ClientEvent::Closed);
}

fn dispatch_message(inner: &Arc<ClientInner>, events: &mpsc::Sender<ClientEvent>, message: Value) {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = message.get("id").cloned() {
            let response = match inner.policy.respond(method, &params) {
                Ok(result) => json!({"jsonrpc":"2.0", "id":id, "result":result}),
                Err(error) => json!({
                    "jsonrpc":"2.0",
                    "id":id,
                    "error":{"code":error.code,"message":error.message}
                }),
            };
            if let Ok(mut writer) = inner.writer.lock() {
                let _ = write_lsp_frame(&mut *writer, &response);
            }
        } else {
            let notification = LspNotification {
                method: method.to_string(),
                params,
            };
            inner.policy.observe_notification(&notification);
            let _ = events.send(ClientEvent::Notification(notification));
        }
        return;
    }

    let Some(id) = message.get("id").and_then(Value::as_i64) else {
        let _ = events.send(ClientEvent::ProtocolError(
            "响应缺少数值请求 ID".to_string(),
        ));
        return;
    };
    let sender = inner
        .pending
        .lock()
        .expect("LSP pending 锁被污染")
        .remove(&id);
    let Some(sender) = sender else {
        return;
    };
    let result = if let Some(error) = message.get("error") {
        let base_message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未知 RPC 错误");
        let rpc_message = error.get("data").map_or_else(
            || base_message.to_string(),
            |data| format!("{base_message}（data: {data}）"),
        );
        Err(LspClientError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: rpc_message,
        })
    } else {
        Ok(message.get("result").cloned().unwrap_or(Value::Null))
    };
    let _ = sender.send(result);
}

fn fail_all_pending(inner: &ClientInner, message: String) {
    let pending = std::mem::take(&mut *inner.pending.lock().expect("LSP pending 锁被污染"));
    for (_, sender) in pending {
        let _ = sender.send(Err(LspClientError::Protocol(message.clone())));
    }
}
