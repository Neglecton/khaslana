// `code_understanding::agent` 的状态机测试：用脚本化"模型"驱动真实的索引与
// 源码工具，覆盖 B01 登录闭环、缺边补查、预算、取消与格式修复。
//
// 脚本化模型只能证明服务侧的工具中介、来源绑定、预算与生命周期行为正确，
// 不能证明真实模型的理解能力（那需要真实模型业务验收，见开发文档 §7）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::super::analysis::{
    ANALYSIS_PROTOCOL_VERSION, AnalysisCompletion, DataObjectCategory, analysis_system_prompt,
};
use super::super::session::{UnderstandingHistorySummary, UnderstandingPromptContext};
use super::*;
use crate::code_index::{PipelineOptions, RunOutcome, run_index};

type ScriptStep = Box<dyn FnMut(&[AgentChatMessage]) -> Result<AgentTurn, AgentStreamError> + Send>;

/// 脚本化模型：按顺序消费步骤，并记录每次请求看到的对话。
struct ScriptedProvider {
    steps: Mutex<VecDeque<ScriptStep>>,
    log: Arc<Mutex<Vec<Vec<AgentChatMessage>>>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<ScriptStep>) -> (Self, Arc<Mutex<Vec<Vec<AgentChatMessage>>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                steps: Mutex::new(steps.into()),
                log: Arc::clone(&log),
            },
            log,
        )
    }
}

impl UnderstandingTurnProvider for ScriptedProvider {
    fn request(
        &self,
        messages: &[AgentChatMessage],
        _tools: &[ToolSchema],
        on_delta: &mut dyn FnMut(StreamDelta),
    ) -> Result<AgentTurn, AgentStreamError> {
        self.log
            .lock()
            .expect("脚本记录锁被污染")
            .push(messages.to_vec());
        let step = {
            let mut guard = self.steps.lock().expect("脚本队列锁被污染");
            guard.pop_front()
        };
        let Some(mut step) = step else {
            return Err(AgentStreamError::fatal("脚本步骤已用尽".to_string()));
        };
        let turn = step(messages)?;
        // 模拟流式增量：把正文作为 Content 块回放，确保事件通道可用。
        if !turn.content.is_empty() {
            on_delta(StreamDelta::Content(turn.content.clone()));
        }
        Ok(turn)
    }
}

fn turn(content: &str, tool_calls: Vec<AgentToolCall>) -> AgentTurn {
    AgentTurn {
        content: content.to_string(),
        reasoning: None,
        tool_calls,
    }
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

/// 最近一次工具结果的 JSON（ToolEnvelope）。
fn last_tool_json(messages: &[AgentChatMessage]) -> serde_json::Value {
    let content = last_tool_text(messages);
    serde_json::from_str(&content)
        .unwrap_or_else(|error| panic!("工具结果应为 JSON：{error}\n{content}"))
}

fn last_tool_text(messages: &[AgentChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            AgentChatMessage::Tool { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("对话中应有工具结果")
}

/// 找出所有 read_file 结果发放的来源 ID（按调用顺序）。
fn read_source_ids(messages: &[AgentChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentChatMessage::Tool { content, .. } => serde_json::from_str::<
                serde_json::Value,
            >(content)
            .ok()
            .and_then(|value| value["data"]["source_id"].as_str().map(str::to_string)),
            _ => None,
        })
        .collect()
}

/// tool 消息里是否出现过某段文本。
fn any_tool_text(messages: &[AgentChatMessage], needle: &str) -> bool {
    messages.iter().any(|message| match message {
        AgentChatMessage::Tool { content, .. } => content.contains(needle),
        _ => false,
    })
}

fn copy_dir(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = target.join(&file_name);
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// 复制 B01 普通 Java 登录样例到临时目录并建立索引。
fn b01_fixture() -> (TempDir, PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/code_understanding_business/B01");
    assert!(fixture.is_dir(), "缺少 B01 样例：{}", fixture.display());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("b01");
    copy_dir(&fixture, &root);
    git2::Repository::init(&root).unwrap();

    let db_path = temp.path().join("index.db");
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(&root, &db_path, true, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("B01 索引未完成：{other:?}"),
    }
    (temp, root, db_path)
}

/// 增量重建索引（用于"索引缺少某文件"的缺边场景）。
fn reindex(root: &Path, db_path: &Path) {
    let mut options = PipelineOptions::new(Arc::new(AtomicBool::new(false)), Box::new(|_| {}));
    match run_index(root, db_path, false, &mut options).unwrap() {
        RunOutcome::Completed(_) => {}
        other => panic!("增量索引未完成：{other:?}"),
    }
}

fn input(root: &Path, db: &Path, question: &str) -> UnderstandingAgentInput {
    UnderstandingAgentInput {
        repo_root: root.to_path_buf(),
        index_db_path: db.to_path_buf(),
        request_id: "req-b01".to_string(),
        question: question.to_string(),
    }
}

fn run(
    input: &UnderstandingAgentInput,
    provider: &ScriptedProvider,
    cancel: &AtomicBool,
) -> (UnderstandingResult<Option<UnderstandingAnswer>>, Vec<UnderstandingEvent>) {
    let mut events = Vec::new();
    let outcome = run_understanding_agent(input, provider, cancel, &mut |event| events.push(event));
    (outcome, events)
}

const AUTH_SERVICE: &str = "src/com/example/login/AuthService.java";
const LOGIN_CONTROLLER: &str = "src/com/example/login/LoginController.java";
const USER_REPOSITORY: &str = "src/com/example/login/UserRepository.java";
const LOGIN_ATTEMPT_REPOSITORY: &str = "src/com/example/login/LoginAttemptRepository.java";
const SESSION_SERVICE: &str = "src/com/example/login/SessionService.java";

#[test]
fn b01_login_flow_reads_real_sources_and_binds_evidence() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(
        &root,
        &db_path,
        "登录逻辑怎么实现的？调用了什么，被什么调用？读写哪些表？",
    );

    // 收集本次会话真实发放的来源 ID：最终答案只能引用它们。
    let auth_source: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let step_search = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn("", vec![tool_call(
            "call-1",
            "search_symbols",
            serde_json::json!({"query": "authenticate", "path_prefix": "src"}),
        )]))
    }) as ScriptStep;

    let step_detail = Box::new(|messages: &[AgentChatMessage]| {
        let tool = last_tool_json(messages);
        let candidate_id = tool["data"]["candidates"]
            .as_array()
            .and_then(|candidates| {
                candidates
                    .iter()
                    .find(|candidate| candidate["name"] == "authenticate")
                    .or_else(|| candidates.first())
            })
            .and_then(|candidate| candidate["candidate_id"].as_str())
            .expect("search_symbols 应返回 authenticate 候选")
            .to_string();
        Ok(turn("", vec![tool_call(
            "call-2",
            "get_symbol",
            serde_json::json!({"candidate_id": candidate_id}),
        )]))
    }) as ScriptStep;

    let collected_auth = Arc::clone(&auth_source);
    let step_trace = Box::new(move |messages: &[AgentChatMessage]| {
        let tool = last_tool_json(messages);
        *collected_auth.lock().unwrap() =
            Some(tool["data"]["source"]["source_id"].as_str().expect("定义来源").to_string());
        let candidate_id = tool["data"]["candidate_id"]
            .as_str()
            .expect("详情应带候选 ID")
            .to_string();
        Ok(turn("", vec![tool_call(
            "call-3",
            "trace_calls",
            serde_json::json!({"candidate_id": candidate_id, "direction": "both"}),
        )]))
    }) as ScriptStep;

    let step_read_repository = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn(
            "",
            vec![
                tool_call(
                    "call-4",
                    "read_file",
                    serde_json::json!({"path": USER_REPOSITORY, "start_line": 1, "end_line": 80}),
                ),
                tool_call(
                    "call-5",
                    "read_file",
                    serde_json::json!({"path": LOGIN_ATTEMPT_REPOSITORY, "start_line": 1, "end_line": 40}),
                ),
                tool_call(
                    "call-6",
                    "read_file",
                    serde_json::json!({"path": SESSION_SERVICE, "start_line": 1, "end_line": 30}),
                ),
                tool_call(
                    "call-7",
                    "read_file",
                    serde_json::json!({"path": LOGIN_CONTROLLER, "start_line": 1, "end_line": 20}),
                ),
            ],
        ))
    }) as ScriptStep;

    let step_final = Box::new(move |messages: &[AgentChatMessage]| {
        let ids = read_source_ids(messages);
        assert_eq!(ids.len(), 4, "四个 read_file 都应发放来源：{messages:?}");
        let user_repo = ids[0].clone();
        let attempt_repo = ids[1].clone();
        let session = ids[2].clone();
        let controller = ids[3].clone();
        let auth = auth_source
            .lock()
            .unwrap()
            .clone()
            .expect("get_symbol 来源已收集");
        let answer = serde_json::json!({
            "version": 1,
            "summary": "登录请求由 LoginController 接收，AuthService.authenticate 校验账号与密码，成功后创建会话并更新登录信息。",
            "context": {"scope": "src/com/example/login"},
            "coverage_note": "基于当前索引与源码检索；未列出的调用方不代表不存在。",
            "findings": [
                {"text": "入口是 LoginController.login，直接调用 AuthService.authenticate。", "state": "observed", "source_ids": [controller.clone(), auth.clone()]},
                {"text": "密码错误时累加 failed_attempts 并记录 BAD_PASSWORD。", "state": "observed", "condition": "密码校验失败", "source_ids": [auth.clone()]},
                {"text": "用户不存在时不更新账号，只记录 USER_NOT_FOUND。", "state": "observed", "condition": "用户不存在", "source_ids": [auth.clone()]}
            ],
            "callers": [
                {"name": "LoginController#login(String,String)", "relative_path": LOGIN_CONTROLLER, "detail": "登录入口", "kind": "direct", "source_ids": [controller.clone()]}
            ],
            "callees": [
                {"name": "UserRepository#findActiveByUsername", "relative_path": USER_REPOSITORY, "kind": "direct", "source_ids": [user_repo.clone()]},
                {"name": "PasswordVerifier#matches", "kind": "candidate", "detail": "密码校验边界，实现未在本次范围展开", "source_ids": [auth.clone()]}
            ],
            "data_accesses": [
                {"object": "app_user", "category": "db_object", "operations": ["read", "update"],
                 "conditions": ["按用户名且 enabled = TRUE 读取", "失败累加 failed_attempts", "成功更新 last_login_at 并清零"],
                 "access_method": "UserRepository 内嵌 SQL", "state": "observed", "source_ids": [user_repo.clone()]},
                {"object": "login_attempt", "category": "db_object", "operations": ["insert"],
                 "conditions": ["用户不存在/密码错误/成功三个分支都会写入"],
                 "access_method": "LoginAttemptRepository.record", "state": "observed", "source_ids": [attempt_repo.clone()]},
                {"object": "in_memory_sessions", "category": "cache", "operations": ["insert"],
                 "conditions": ["仅密码校验成功后写入会话 ID"],
                 "access_method": "SessionService.create", "state": "observed", "source_ids": [session.clone()]}
            ],
            "steps": [
                {"id": "entry", "title": "接收登录请求", "source_ids": [controller.clone()]},
                {"id": "lookup", "title": "按用户名读取启用用户", "source_ids": [user_repo.clone(), auth.clone()]},
                {"id": "verify", "title": "校验密码", "source_ids": [auth.clone()]},
                {"id": "fail", "title": "记录失败尝试", "detail": "账号或密码错误分支", "source_ids": [attempt_repo.clone(), auth.clone()]},
                {"id": "success", "title": "创建会话并更新登录信息", "source_ids": [session.clone(), user_repo.clone()]},
                {"id": "audit", "title": "记录成功尝试", "source_ids": [attempt_repo.clone()]}
            ],
            "links": [
                {"from": "entry", "to": "lookup", "kind": "call"},
                {"from": "lookup", "to": "verify", "kind": "sequence_hint"},
                {"from": "verify", "to": "fail", "kind": "branch", "label": "密码错误"},
                {"from": "verify", "to": "success", "kind": "branch", "label": "校验通过"},
                {"from": "success", "to": "audit", "kind": "sequence_hint"}
            ],
            "unknowns": [
                "PasswordHashLibrary 是样例内的哈希校验边界，不能推断生产算法",
                "样例不连接真实数据库，不能声明运行时记录或事务结果"
            ],
            "completion": "complete"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    }) as ScriptStep;

    let (provider, log) = ScriptedProvider::new(vec![
        step_search,
        step_detail,
        step_trace,
        step_read_repository,
        step_final,
    ]);
    let cancel = AtomicBool::new(false);
    let (outcome, events) = run(&input, &provider, &cancel);
    let answer = outcome.expect("闭环应成功").expect("未取消应有答案");

    assert!(!answer.sources.is_empty(), "完成答案应保留本问实际发放的来源");
    assert_eq!(answer.analysis.context.project_key.len(), 8);
    assert_eq!(answer.analysis.context.request_id, "req-b01");
    assert_eq!(
        answer.analysis.context.question,
        "登录逻辑怎么实现的？调用了什么，被什么调用？读写哪些表？"
    );
    assert_eq!(answer.analysis.version, ANALYSIS_PROTOCOL_VERSION);
    assert_eq!(answer.analysis.data_accesses.len(), 3);
    let user_access = answer
        .analysis
        .data_accesses
        .iter()
        .find(|access| access.object == "app_user")
        .expect("应报告 app_user");
    assert_eq!(user_access.operations.len(), 2);
    let session_access = answer
        .analysis
        .data_accesses
        .iter()
        .find(|access| access.object == "in_memory_sessions")
        .expect("缓存副作用应单列");
    assert_eq!(session_access.category, DataObjectCategory::Cache);
    assert!(
        answer
            .analysis
            .unknowns
            .iter()
            .any(|unknown| unknown.contains("样例"))
    );
    assert_eq!(answer.analysis.callers.len(), 1);
    assert_eq!(answer.analysis.links.len(), 5);

    let tool_names: Vec<&str> = answer
        .steps
        .iter()
        .filter_map(|step| match step {
            UnderstandingStep::ToolCall { name, error, .. } => {
                assert!(!error, "工具不应失败");
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_names,
        vec![
            "search_symbols",
            "get_symbol",
            "trace_calls",
            "read_file",
            "read_file",
            "read_file",
            "read_file"
        ]
    );

    assert!(
        events
            .iter()
            .any(|event| matches!(event, UnderstandingEvent::Done(_))),
        "应发出完成事件"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UnderstandingEvent::Delta { content: Some(_), .. })),
        "流式增量应透传"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UnderstandingEvent::Step(_))),
        "工具步骤应回传"
    );

    // 模型侧确实看到过 trace_calls 的覆盖说明，避免把无边当无调用。
    let requests = log.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|messages| any_tool_text(messages, "无边不表示")),
        "trace 结果应明示索引线索限制"
    );
}

#[test]
fn missing_index_edge_is_recovered_by_source_search() {
    // 索引建立后再放回调用方文件：索引缺符号/边，但源码仍可检索与阅读。
    let (_temp, root, db_path) = b01_fixture();
    let controller_path = root.join(LOGIN_CONTROLLER);
    let controller_bytes = std::fs::read(&controller_path).unwrap();
    std::fs::remove_file(&controller_path).unwrap();
    reindex(&root, &db_path);
    std::fs::write(&controller_path, &controller_bytes).unwrap();

    let input = input(
        &root,
        &db_path,
        "谁还调用了这个认证服务？这里的会话怎么创建？",
    );

    let step_search = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn("", vec![tool_call(
            "call-1",
            "search_symbols",
            serde_json::json!({"query": "authenticate"}),
        )]))
    }) as ScriptStep;

    let step_trace = Box::new(|messages: &[AgentChatMessage]| {
        let tool = last_tool_json(messages);
        let candidate_id = tool["data"]["candidates"]
            .as_array()
            .and_then(|candidates| candidates.first())
            .and_then(|candidate| candidate["candidate_id"].as_str())
            .expect("应有存活候选")
            .to_string();
        Ok(turn("", vec![tool_call(
            "call-2",
            "trace_calls",
            serde_json::json!({"candidate_id": candidate_id, "direction": "inbound"}),
        )]))
    }) as ScriptStep;

    let step_search_code = Box::new(|messages: &[AgentChatMessage]| {
        let trace_text = last_tool_text(messages);
        assert!(
            trace_text.contains("无边不表示"),
            "trace 覆盖说明：{trace_text}"
        );
        Ok(turn(
            "索引里没有 inbound 边，我用源码搜索补查调用方。",
            vec![tool_call(
                "call-3",
                "search_code",
                serde_json::json!({"query": "authenticate(", "suffix": ".java"}),
            )],
        ))
    }) as ScriptStep;

    let step_read_caller = Box::new(|messages: &[AgentChatMessage]| {
        let tool = last_tool_json(messages);
        let matches = tool["data"]["matches"].as_array().expect("应有搜索命中");
        assert!(
            matches
                .iter()
                .any(|hit| hit["relative_path"] == LOGIN_CONTROLLER),
            "源码搜索应命中调用方：{tool}"
        );
        Ok(turn(
            "",
            vec![tool_call(
                "call-4",
                "read_file",
                serde_json::json!({"path": LOGIN_CONTROLLER, "start_line": 1, "end_line": 20}),
            )],
        ))
    }) as ScriptStep;

    let step_final = Box::new(|messages: &[AgentChatMessage]| {
        let ids = read_source_ids(messages);
        let caller_source = ids.first().expect("应发放调用方来源").clone();
        let answer = serde_json::json!({
            "summary": "AuthService.authenticate 的直接调用方是 LoginController.login（索引无边，经源码搜索确认）。",
            "coverage_note": "索引中没有 inbound 调用边，本次调用方结论来自源码搜索与阅读；可能仍有未发现的调用点。",
            "findings": [
                {"text": "LoginController.login 直接调用 authenticate。", "state": "observed", "source_ids": [caller_source.clone()]}
            ],
            "callers": [
                {"name": "LoginController#login", "relative_path": LOGIN_CONTROLLER, "kind": "direct", "detail": "索引缺边，由 search_code + read_file 确认", "source_ids": [caller_source.clone()]}
            ],
            "callees": [],
            "data_accesses": [],
            "steps": [{"id": "s1", "title": "找到调用方", "source_ids": [caller_source.clone()]}],
            "links": [],
            "unknowns": ["索引缺少该调用边，可能仍有其它调用方"],
            "completion": "partial"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    }) as ScriptStep;

    let (provider, _log) = ScriptedProvider::new(vec![
        step_search,
        step_trace,
        step_search_code,
        step_read_caller,
        step_final,
    ]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let answer = outcome.expect("缺边也应能回答").expect("未取消应有答案");
    assert_eq!(answer.analysis.completion, AnalysisCompletion::Partial);
    assert_eq!(answer.analysis.callers.len(), 1);
    assert!(answer.analysis.coverage_note.contains("索引中没有"));
}

#[test]
fn tool_budget_exhaustion_stops_with_budget_error() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    // 永不收尾的模型：每轮都发起一个工具调用。
    let steps: Vec<ScriptStep> = (0..(UNDERSTANDING_MAX_TOOL_CALLS + 5))
        .map(|index| {
            Box::new(move |_messages: &[AgentChatMessage]| {
                Ok(turn(
                    "",
                    vec![tool_call(
                        &format!("call-{index}"),
                        "search_code",
                        serde_json::json!({"query": "authenticate"}),
                    )],
                ))
            }) as ScriptStep
        })
        .collect();
    let (provider, _log) = ScriptedProvider::new(steps);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("触顶应报错");
    assert_eq!(error.code, ErrorCode::BudgetExceeded);
    assert!(
        error.message.contains("上限"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("已读取的来源仍可在本次会话查看"),
        "触顶错误应说明部分结果保留：{}",
        error.message
    );
}

/// 模型对话里是否出现过某段文本（任意角色）。
fn any_message_text(messages: &[AgentChatMessage], needle: &str) -> bool {
    messages.iter().any(|message| match message {
        AgentChatMessage::System(text)
        | AgentChatMessage::User(text)
        | AgentChatMessage::Assistant { content: text, .. } => text.contains(needle),
        AgentChatMessage::Tool { content, .. } => content.contains(needle),
    })
}

/// 回归：收尾轮的 JSON 格式错误同样得到一次修复机会。
///
/// 真实运行里最容易出现的组合是「累计结果体积触顶 + 模型在催促下写错字段」：
/// 此前 `force_finish` 分支直接跳过了格式修复，一次字段名写错就让整轮分析作废，
/// 连 partial 都拿不到（用户实测报错即为此）。
#[test]
fn forced_finish_still_gets_one_format_repair() {
    let (_temp, root, db_path) = b01_fixture();
    // 造一个够大的可索引文件：单条工具结果被截断到 8K，读十几次就能累计触顶。
    let big_relative = "src/com/example/login/BigSource.java";
    let body: String = (0..1400)
        .map(|index| format!("// filler {index:04} {}", "x".repeat(80)))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(root.join(big_relative), body).unwrap();
    reindex(&root, &db_path);

    let input = input(&root, &db_path, "登录逻辑？");
    // 脚本按对话状态自适应：未触顶就继续读大文件；进入收尾轮先给不合法 JSON，
    // 收到修复指令后给出合法 JSON。这样不依赖「第几轮触顶」的精确推算。
    let step = |index: usize| -> ScriptStep {
        Box::new(move |messages: &[AgentChatMessage]| {
            if any_message_text(messages, "不是可用的最终结果") {
                let source_id = read_source_ids(messages)
                    .first()
                    .cloned()
                    .unwrap_or_default();
                let answer = serde_json::json!({
                    "summary": "已读取的范围内未发现与登录相关的实现。",
                    "findings": [{
                        "text": "所读文件是填充内容，没有登录逻辑。",
                        "state": "observed",
                        "source_ids": [source_id]
                    }],
                    "steps": [{"id": "s1", "title": "读取大文件", "source_ids": [source_id]}],
                    "links": [],
                    "completion": "partial"
                });
                return Ok(turn(&answer.to_string(), Vec::new()));
            }
            if any_message_text(messages, "工具调用预算已用尽") {
                return Ok(turn("{\"summary\": \"预算已用尽\", \"findings\": [", Vec::new()));
            }
            // 首轮先做一次小范围读取：超长结果会被截成半截 JSON（模型与校验都无法
            // 直视），需要留下一条可解析的工具结果作为合法来源。
            if index == 0 {
                return Ok(turn(
                    "",
                    vec![tool_call(
                        "small-0",
                        "read_file",
                        serde_json::json!({"path": AUTH_SERVICE, "start_line": 1, "end_line": 8}),
                    )],
                ));
            }
            let start = 1 + (index % 4) * 200;
            Ok(turn(
                "",
                vec![tool_call(
                    &format!("big-{index}"),
                    "read_file",
                    serde_json::json!({
                        "path": big_relative,
                        "start_line": start,
                        "end_line": start + 399
                    }),
                )],
            ))
        }) as ScriptStep
    };
    let steps: Vec<ScriptStep> = (0..24).map(step).collect();
    let (provider, log) = ScriptedProvider::new(steps);
    let cancel = AtomicBool::new(false);
    let (outcome, events) = run(&input, &provider, &cancel);

    let answer = outcome
        .expect("收尾轮的格式错误应被修复，而不是直接让整轮分析作废")
        .expect("未取消应有答案");
    assert_eq!(answer.analysis.completion, AnalysisCompletion::Partial);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UnderstandingEvent::Done(_))),
        "修复成功后应发出完成事件"
    );
    let log = log.lock().expect("脚本记录锁被污染");
    let repaired = log.last().expect("应有一次修复请求");
    assert!(
        any_message_text(repaired, "不是可用的最终结果"),
        "修复请求必须带上具体的校验问题"
    );
    assert!(
        matches!(
            repaired.last(),
            Some(AgentChatMessage::User(text)) if text.contains("不是可用的最终结果")
        ),
        "修复指令必须是最后一条消息：中间不应再插入工具调用"
    );
}

#[test]
fn duplicate_call_ids_are_not_executed_twice() {    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let step_duplicate = Box::new(|_messages: &[AgentChatMessage]| {
        let args = serde_json::json!({"path": AUTH_SERVICE, "start_line": 1, "end_line": 8});
        Ok(turn(
            "",
            vec![
                tool_call("dup", "read_file", args.clone()),
                tool_call("dup", "read_file", args),
            ],
        ))
    }) as ScriptStep;

    let step_final = Box::new(|messages: &[AgentChatMessage]| {
        let source_id = read_source_ids(messages)
            .first()
            .cloned()
            .expect("首次调用应发放来源");
        let answer = serde_json::json!({
            "summary": "读取了认证服务定义。",
            "findings": [{"text": "authenticate 读取用户并校验密码。", "state": "observed", "source_ids": [source_id]}],
            "steps": [{"id": "s1", "title": "读取认证服务", "source_ids": [source_id]}],
            "links": [],
            "completion": "partial"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    }) as ScriptStep;

    let (provider, _log) = ScriptedProvider::new(vec![step_duplicate, step_final]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let answer = outcome.expect("重复 call_id 不应中断").expect("应有答案");

    let tool_steps: Vec<(bool, String)> = answer
        .steps
        .iter()
        .filter_map(|step| match step {
            UnderstandingStep::ToolCall {
                result_excerpt,
                error,
                ..
            } => Some((*error, result_excerpt.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(tool_steps.len(), 2);
    assert!(!tool_steps[0].0, "首次调用应成功");
    assert!(tool_steps[1].0, "重复调用应标记失败");
    assert!(tool_steps[1].1.contains("重复"), "{}", tool_steps[1].1);
}

#[test]
fn invalid_output_triggers_one_format_repair() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let step_read = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn(
            "",
            vec![tool_call(
                "call-1",
                "read_file",
                serde_json::json!({"path": AUTH_SERVICE, "start_line": 1, "end_line": 8}),
            )],
        ))
    }) as ScriptStep;
    let step_bad = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn("我读了 AuthService，它调用 UserRepository。", Vec::new()))
    }) as ScriptStep;
    let step_good = Box::new(|messages: &[AgentChatMessage]| {
        let source_id = read_source_ids(messages)
            .first()
            .cloned()
            .expect("read_file 来源");
        let answer = serde_json::json!({
            "summary": "认证服务读取用户并校验密码。",
            "findings": [{"text": "authenticate 读取用户。", "state": "observed", "source_ids": [source_id]}],
            "steps": [{"id": "s1", "title": "读取用户", "source_ids": [source_id]}],
            "links": [],
            "completion": "partial"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    }) as ScriptStep;

    let (provider, log) = ScriptedProvider::new(vec![step_read, step_bad, step_good]);
    let cancel = AtomicBool::new(false);
    let (outcome, events) = run(&input, &provider, &cancel);
    let answer = outcome.expect("修复后应成功").expect("应有答案");
    assert_eq!(answer.analysis.findings.len(), 1);

    let requests = log.lock().unwrap();
    let last = requests.last().expect("应有最终请求");
    assert!(
        last.iter().any(|message| matches!(
            message,
            AgentChatMessage::User(text) if text.contains("只输出一个符合系统提示协议")
        )),
        "应注入格式修复指令"
    );
    assert!(
        last.iter().any(|message| matches!(
            message,
            AgentChatMessage::User(text)
                if text.contains("合法 source_id 白名单") && text.contains("sr1:")
        )),
        "修复指令应明确列出本问合法来源"
    );
    assert!(
        last.iter().any(|message| matches!(
            message,
            AgentChatMessage::Assistant { content, .. } if content.contains("我读了 AuthService")
        )),
        "中间正文应回填为 assistant 消息"
    );
    drop(requests);

    assert!(events.iter().any(|event| matches!(
        event,
        UnderstandingEvent::Step(UnderstandingStep::Message { text }) if text.contains("我读了")
    )));
}

#[test]
fn repeated_invalid_output_fails_after_single_repair() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let bad = || -> ScriptStep {
        Box::new(|_messages: &[AgentChatMessage]| Ok(turn("不是 JSON", Vec::new())))
    };
    let (provider, _log) = ScriptedProvider::new(vec![bad(), bad()]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("两次坏输出应失败");
    assert_eq!(error.code, ErrorCode::AnswerInvalid);
    assert!(error.message.contains("合法 JSON"), "{}", error.message);
}

#[test]
fn forged_source_id_is_rejected_by_validation() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let forged = |_messages: &[AgentChatMessage]| {
        let answer = serde_json::json!({
            "summary": "伪造来源。",
            "findings": [{"text": "结论", "state": "observed", "source_ids": ["sr1:forged"]}],
            "steps": [],
            "links": [],
            "completion": "complete"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    };
    let (provider, _log) = ScriptedProvider::new(vec![
        Box::new(forged) as ScriptStep,
        Box::new(forged) as ScriptStep,
    ]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("伪造来源应失败");
    assert_eq!(error.code, ErrorCode::AnswerInvalid);
    assert!(error.message.contains("sr1:forged"), "{}", error.message);
}

#[test]
fn unknown_object_category_must_be_marked_unknown() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let bad_answer = |_messages: &[AgentChatMessage]| {
        let answer = serde_json::json!({
            "summary": "把未知对象当已确认表。",
            "findings": [{"text": "读取了用户", "state": "observed", "source_ids": ["sr1:x"]}],
            "data_accesses": [{
                "object": "some_table", "category": "unknown", "operations": ["read"],
                "state": "observed", "source_ids": ["sr1:x"]
            }],
            "steps": [],
            "links": [],
            "completion": "complete"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    };
    let (provider, _log) = ScriptedProvider::new(vec![
        Box::new(bad_answer) as ScriptStep,
        Box::new(bad_answer) as ScriptStep,
    ]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("未知对象标 observed 应被拒绝");
    assert!(
        error.message.contains("对象类别未知"),
        "{}",
        error.message
    );
}

#[test]
fn dangling_flow_link_is_rejected() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let bad_answer = |_messages: &[AgentChatMessage]| {
        let answer = serde_json::json!({
            "summary": "悬空连接。",
            "findings": [{"text": "结论", "state": "observed", "source_ids": ["sr1:x"]}],
            "steps": [{"id": "s1", "title": "步骤", "source_ids": ["sr1:x"]}],
            "links": [{"from": "s1", "to": "missing", "kind": "call"}],
            "completion": "complete"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    };
    let (provider, _log) = ScriptedProvider::new(vec![
        Box::new(bad_answer) as ScriptStep,
        Box::new(bad_answer) as ScriptStep,
    ]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("悬空连接应被拒绝");
    assert!(error.message.contains("不存在的步骤"), "{}", error.message);
}

#[test]
fn cancellation_stops_before_the_next_round() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel_flag);
    let step_first = Box::new(move |_messages: &[AgentChatMessage]| {
        // 第一轮发起工具调用时用户取消：工具执行完，下一轮边界退出。
        flag.store(true, Ordering::Relaxed);
        Ok(turn(
            "",
            vec![tool_call(
                "call-1",
                "search_symbols",
                serde_json::json!({"query": "authenticate"}),
            )],
        ))
    }) as ScriptStep;
    let step_second = Box::new(|_messages: &[AgentChatMessage]| -> Result<AgentTurn, AgentStreamError> {
        panic!("取消后不应再发起模型请求");
    }) as ScriptStep;

    let (provider, _log) = ScriptedProvider::new(vec![step_first, step_second]);
    let (outcome, events) = run(&input, &provider, &cancel_flag);
    assert!(outcome.expect("取消不是失败").is_none());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, UnderstandingEvent::Done(_))),
        "取消不应发完成事件"
    );
}

#[test]
fn cancellation_during_retry_backoff_stops_without_extra_attempt() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel_flag);
    let step = Box::new(move |_messages: &[AgentChatMessage]| {
        flag.store(true, Ordering::Relaxed);
        Err(AgentStreamError::transient("网络中断".to_string()))
    }) as ScriptStep;
    let (provider, _log) = ScriptedProvider::new(vec![step]);
    let (outcome, _events) = run(&input, &provider, &cancel_flag);
    assert!(outcome.expect("退避期间取消不算失败").is_none());
}

#[test]
fn transient_failure_is_retried_once_then_succeeds() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    // 脚本化 provider 每次请求消费一个步骤：重试是对同一轮再发一次请求，
    // 因此第一次尝试与重试各需要一个步骤。
    let step_first_attempt = Box::new(|_messages: &[AgentChatMessage]| {
        Err(AgentStreamError::transient("AI 流读取失败".to_string()))
    }) as ScriptStep;
    let step_retry = Box::new(|_messages: &[AgentChatMessage]| {
        Ok(turn(
            "",
            vec![tool_call(
                "call-1",
                "read_file",
                serde_json::json!({"path": AUTH_SERVICE, "start_line": 1, "end_line": 8}),
            )],
        ))
    }) as ScriptStep;
    let step_final = Box::new(|messages: &[AgentChatMessage]| {
        let source_id = read_source_ids(messages)
            .first()
            .cloned()
            .expect("read_file 来源");
        let answer = serde_json::json!({
            "summary": "重试后成功。",
            "findings": [{"text": "结论", "state": "observed", "source_ids": [source_id]}],
            "steps": [{"id": "s1", "title": "读取", "source_ids": [source_id]}],
            "links": [],
            "completion": "complete"
        });
        Ok(turn(&answer.to_string(), Vec::new()))
    }) as ScriptStep;

    let (provider, _log) =
        ScriptedProvider::new(vec![step_first_attempt, step_retry, step_final]);
    let cancel = AtomicBool::new(false);
    let (outcome, events) = run(&input, &provider, &cancel);
    assert!(outcome.expect("重试后应成功").is_some());
    assert!(
        events.iter().any(|event| matches!(
            event,
            UnderstandingEvent::Progress(text) if text.contains("正在重试第 1/3 次")
        )),
        "应发出重试进度"
    );
}

#[test]
fn retry_exhaustion_reports_attempt_count() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    // transient_once 只允许再试 1 次：总计 2 次尝试、1 秒退避。
    let failing = || -> ScriptStep {
        Box::new(|_messages: &[AgentChatMessage]| {
            Err(AgentStreamError::transient_once("截断".to_string()))
        })
    };
    let (provider, _log) = ScriptedProvider::new(vec![failing(), failing()]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("重试耗尽应失败");
    assert_eq!(error.code, ErrorCode::ProviderUnavailable);
    assert!(
        error.message.contains("已自动重试 1 次"),
        "{}",
        error.message
    );
}

#[test]
fn tool_unsupported_endpoint_maps_to_dedicated_error() {
    let (_temp, root, db_path) = b01_fixture();
    let input = input(&root, &db_path, "登录逻辑？");

    let step = Box::new(|_messages: &[AgentChatMessage]| {
        Err(AgentStreamError::fatal(
            "AI 端点返回 HTTP 422，疑似不支持工具调用（tools）。".to_string(),
        ))
    }) as ScriptStep;
    let (provider, _log) = ScriptedProvider::new(vec![step]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("不支持工具调用应直接失败");
    assert_eq!(error.code, ErrorCode::ProviderToolUnsupported);
    assert!(
        error.message.contains("不支持工具调用"),
        "{}",
        error.message
    );
}

#[test]
fn missing_index_reports_index_error_without_calling_model() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("empty");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let input = input(&root, &temp.path().join("missing.db"), "登录逻辑？");

    let step = Box::new(|_messages: &[AgentChatMessage]| -> Result<AgentTurn, AgentStreamError> {
        panic!("索引缺失时不应请求模型");
    }) as ScriptStep;
    let (provider, _log) = ScriptedProvider::new(vec![step]);
    let cancel = AtomicBool::new(false);
    let (outcome, _events) = run(&input, &provider, &cancel);
    let error = outcome.expect_err("缺索引应报错");
    assert_eq!(error.code, ErrorCode::IndexMissing);
}

#[test]
fn tool_args_summary_is_human_readable() {
    assert_eq!(
        tool_args_summary("search_symbols", r#"{"query":"authenticate"}"#),
        "search_symbols authenticate"
    );
    assert_eq!(
        tool_args_summary(
            "read_file",
            r#"{"path":"src/A.java","start_line":3,"end_line":9}"#
        ),
        "read_file src/A.java:3-9"
    );
    assert_eq!(
        tool_args_summary(
            "trace_calls",
            r#"{"candidate_id":"sc1:1","direction":"inbound"}"#
        ),
        "trace_calls sc1:1 (inbound)"
    );
    assert_eq!(tool_args_summary("get_file_tree", "{}"), "get_file_tree /");
    assert_eq!(
        tool_args_summary("search_code", r#"{"query":"INSERT INTO","suffix":".xml"}"#),
        "search_code INSERT INTO [.xml]"
    );
}

#[test]
fn budget_accounting_names_the_tripped_limit() {
    let mut budget = ToolBudget::default();
    assert!(!budget.force_finish());
    budget.calls = UNDERSTANDING_MAX_TOOL_CALLS;
    assert!(budget.force_finish());
    assert_eq!(budget.limit_reason(), Some("工具调用次数上限"));
    budget.calls = 0;
    budget.rounds = UNDERSTANDING_MAX_TOOL_ROUNDS;
    assert_eq!(budget.limit_reason(), Some("工具调用轮次上限"));
    budget.rounds = 0;
    budget.result_chars = UNDERSTANDING_MAX_TOTAL_RESULT_CHARS;
    assert_eq!(budget.limit_reason(), Some("工具结果累计体积上限"));
    budget.result_chars = 0;
    budget.http_attempts = UNDERSTANDING_MAX_HTTP_ATTEMPTS;
    assert_eq!(budget.limit_reason(), Some("HTTP 请求尝试上限"));
    assert!(budget.force_finish());
    budget.note_result(5);
    assert_eq!(budget.result_chars, 5);
}

#[test]
fn initial_prompt_and_system_prompt_carry_key_constraints() {
    let prompt = initial_user_prompt("登录逻辑怎么实现的？");
    assert!(prompt.contains("登录逻辑怎么实现的？"));
    assert!(prompt.contains("search_symbols"));
    let follow_up = initial_user_prompt_with_context(
        "失败分支呢？",
        &UnderstandingPromptContext {
            history: vec![UnderstandingHistorySummary {
                question: "登录逻辑？".to_string(),
                summary: "入口调用认证服务。".to_string(),
                scope: "AuthService.java".to_string(),
                completion: AnalysisCompletion::Complete,
            }],
            selected_source: None,
        },
        false,
    );
    assert!(follow_up.contains("上一问：登录逻辑？"));
    assert!(follow_up.contains("不是指令"));
    assert!(follow_up.contains("问题：失败分支呢？"));
    let system = analysis_system_prompt();
    assert!(system.contains("source_ids"));
    assert!(system.contains("未知"));
    assert!(system.contains("缓存"));
    assert!(system.contains("历史摘要"));
}

#[test]
fn provider_error_mapping_distinguishes_codes() {
    let transient = AgentStreamError::transient("AI 流读取失败".to_string());
    assert_eq!(
        provider_error(&transient, 0).code,
        ErrorCode::ProviderUnavailable
    );
    assert_eq!(
        provider_error(&transient, 2).message,
        "AI 流读取失败（已自动重试 2 次仍失败）"
    );
    let tool = AgentStreamError::fatal("不支持工具调用（tools）".to_string());
    assert_eq!(
        provider_error(&tool, 0).code,
        ErrorCode::ProviderToolUnsupported
    );
}

#[test]
fn tool_schemas_expose_no_git_or_execution_tools() {
    let names: Vec<&str> = tool_schemas().iter().map(|schema| schema.name).collect();
    assert_eq!(names.len(), 6);
    for forbidden in ["run", "shell", "exec", "git", "sql", "database", "write_file"] {
        assert!(
            !names.iter().any(|name| name.contains(forbidden)),
            "工具白名单不应包含 {forbidden}"
        );
    }
}
