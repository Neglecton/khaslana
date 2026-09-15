use super::*;

#[test]
fn elapsed_label_keeps_minute_precision_until_an_hour() {
    assert_eq!(
        understanding_elapsed_label(std::time::Duration::from_secs(18)),
        "00:18"
    );
    assert_eq!(
        understanding_elapsed_label(std::time::Duration::from_secs(605)),
        "10:05"
    );
    assert_eq!(
        understanding_elapsed_label(std::time::Duration::from_secs(3600 + 62)),
        "1:01:02"
    );
}

#[test]
fn timeline_step_actions_are_user_facing_not_tool_json() {
    // 面向用户动作：不出现工具名本身，参数摘要剥掉工具名前缀。
    let read = understanding_step_action(&UnderstandingStep::ToolCall {
        name: "read_file".to_string(),
        args_summary: "read_file \"src/git/service.rs\":1-80".to_string(),
        result_excerpt: "读完 80 行".to_string(),
        error: false,
    });
    assert!(read.label.starts_with("阅读源码 "));
    assert!(read.label.contains("src/git/service.rs"));
    assert!(!read.label.contains("read_file"));
    assert_eq!(read.detail, None);

    let search = understanding_step_action(&UnderstandingStep::ToolCall {
        name: "search_symbols".to_string(),
        args_summary: "search_symbols 登录".to_string(),
        result_excerpt: "3 个候选".to_string(),
        error: false,
    });
    assert_eq!(search.label, "查找符号 登录");

    let trace = understanding_step_action(&UnderstandingStep::ToolCall {
        name: "trace_calls".to_string(),
        args_summary: "trace_calls abc (both)".to_string(),
        result_excerpt: String::new(),
        error: false,
    });
    assert!(trace.label.starts_with("核对调用关系"));

    let reasoning = understanding_step_action(&UnderstandingStep::Reasoning {
        text: "先确认入口再核对副作用".to_string(),
    });
    assert_eq!(reasoning.label, "思考");
    assert_eq!(reasoning.detail.as_deref(), Some("先确认入口再核对副作用"));
}

#[test]
fn failed_tool_step_keeps_the_failure_reason_as_detail() {
    let failed = understanding_step_action(&UnderstandingStep::ToolCall {
        name: "read_file".to_string(),
        args_summary: "read_file \"src/missing.rs\":1-10".to_string(),
        result_excerpt: "文件不存在".to_string(),
        error: true,
    });
    assert_eq!(failed.detail.as_deref(), Some("文件不存在"));
}

#[test]
fn index_state_badges_match_the_page_copy() {
    assert_eq!(UnderstandingIndexState::Ready.badge_label(), "索引就绪");
    assert_eq!(UnderstandingIndexState::Busy.badge_label(), "索引中");
    assert_eq!(UnderstandingIndexState::Disabled.badge_label(), "索引已停用");
    assert_eq!(UnderstandingIndexState::Missing.badge_label(), "索引未就绪");
    assert_eq!(
        UnderstandingIndexState::NoRepo.badge_label(),
        "未打开仓库"
    );
    // 就绪与停用必须配色不同，避免「可提问」与「可能过期」视觉混淆。
    assert_ne!(
        UnderstandingIndexState::Ready.palette(),
        UnderstandingIndexState::Disabled.palette()
    );
}

#[test]
fn java_semantics_stay_reserved_and_only_apply_to_java_sources() {
    // 非 Java 来源不显示语义区。
    assert!(understanding_java_capability("src/git/service.rs").is_none());
    assert!(understanding_java_capability("pom.xml").is_none());
    // Java 来源在接入 JLS-T3 之前恒为降级态，但必须给出可读原因。
    for path in ["src/main/java/AuthService.java", "WEB/Service.JAVA"] {
        let capability = understanding_java_capability(path).expect("Java 来源应给出能力");
        assert!(!capability.ready);
        assert!(capability.title.contains("Java 语义增强"));
        assert!(capability.detail.contains("仍可阅读当前源码"));
    }
}

#[test]
fn understanding_repo_key_matches_the_code_index_key() {
    // 回归：索引库目录、`code_index_stats`、`code_index_preferences` 都以
    // `normalize_repo_path` 为准。会话里保存的仓库路径可能使用正斜杠，
    // 而索引任务写入用的是 canonicalize 后的 `\\?\d:\...`；
    // 若 T5 直接用原始路径寻址，就会查不到已经建好的索引。
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let raw = dir.to_string_lossy();
    let with_forward = raw.replace('\\', "/");
    let with_back = raw.replace('/', "\\");
    let forward_key =
        khaslana::ai::review_store::repo_key(&crate::normalize_repo_path(Path::new(&with_forward)));
    let back_key =
        khaslana::ai::review_store::repo_key(&crate::normalize_repo_path(Path::new(&with_back)));
    assert_eq!(
        forward_key, back_key,
        "同一仓库的不同分隔符写法必须折叠到同一个索引键"
    );
    assert_eq!(
        forward_key,
        khaslana::ai::review_store::repo_key(&crate::normalize_repo_path(dir))
    );
}
