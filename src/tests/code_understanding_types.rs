// code_understanding/types.rs（核心 DTO 契约）的单元测试，通过 #[path] 挂载于
// src/code_understanding/types.rs。

use super::*;

use crate::types::GitError;

fn namespaced_id(prefix: &str, seed: &str) -> String {
    format!(
        "{prefix}:{}",
        crate::code_index::identity::sha256_hex(seed.as_bytes())
    )
}

fn entity(prefix: &str, seed: &str) -> EntityKey {
    EntityKey::from_wire(&namespaced_id(prefix, seed)).unwrap()
}

fn relation_id(seed: &str) -> String {
    namespaced_id("r1", seed)
}

fn evidence_id(seed: &str) -> String {
    namespaced_id("e1", seed)
}

// ---------------------------------------------------------------------------
// EntityKey：ID 命名空间
// ---------------------------------------------------------------------------

#[test]
fn entity_key_namespaces_are_validated() {
    // 合法：带版本前缀 + hex。
    assert!(EntityKey::callsite(&namespaced_id("cs1", "call")).is_ok());
    assert!(EntityKey::entry(&namespaced_id("ep1", "entry")).is_ok());
    assert!(EntityKey::boundary(&namespaced_id("bn1", "boundary")).is_ok());
    // 前缀错位/伪造。
    assert!(
        EntityKey::callsite("ep1:0123abcd").is_err(),
        "cs 前缀不接受 ep id"
    );
    assert!(
        EntityKey::callsite("cs2:0123abcd").is_err(),
        "未知版本前缀拒绝"
    );
    assert!(EntityKey::callsite("cs1:").is_err(), "空摘要拒绝");
    assert!(EntityKey::callsite("cs1:zzzz").is_err(), "非 hex 摘要拒绝");
    assert!(EntityKey::callsite("arbitrary").is_err());
    assert!(EntityKey::callsite(&format!("cs1:{}", "A".repeat(64))).is_err());
}

#[test]
fn entity_key_from_wire_dispatches_by_prefix() {
    // sk1 走 SymbolKey 命名空间。
    let material = crate::code_index::identity::symbol_key_material(
        &crate::code_index::identity::SymbolKeyMaterial {
            language: "java",
            project_key: "p",
            module: "",
            source_set: "",
            package: "com.example",
            relative_path: "",
            type_chain: "T",
            kind: "method",
            name: "m",
            signature: "()",
        },
    )
    .unwrap();
    let sk = crate::code_index::identity::symbol_key_from_material(&material);
    let wire = EntityKey::from_wire(sk.as_str()).expect("sk1 键应经 from_wire 解析");
    assert!(matches!(wire, EntityKey::Symbol(_)));
    // 其余命名空间各归其位。
    assert!(matches!(
        EntityKey::from_wire(&namespaced_id("cs1", "call")),
        Ok(EntityKey::CallSite(_))
    ));
    assert!(matches!(
        EntityKey::from_wire(&namespaced_id("ep1", "entry")),
        Ok(EntityKey::Entry(_))
    ));
    assert!(matches!(
        EntityKey::from_wire(&namespaced_id("bn1", "boundary")),
        Ok(EntityKey::Boundary(_))
    ));
    assert!(EntityKey::from_wire("nope:xyz").is_err());
}

#[test]
fn entity_key_serde_roundtrip_is_tagged() {
    let key = entity("cs1", "call");
    let json = serde_json::to_string(&key).unwrap();
    assert!(
        json.contains("\"kind\""),
        "tagged 序列化必须携带 kind：{json}"
    );
    let back: EntityKey = serde_json::from_str(&json).unwrap();
    assert_eq!(key, back);
    // Symbol 变体往返。
    let material = crate::code_index::identity::symbol_key_material(
        &crate::code_index::identity::SymbolKeyMaterial {
            language: "rust",
            project_key: "p",
            module: "",
            source_set: "",
            package: "",
            relative_path: "src/lib.rs",
            type_chain: "",
            kind: "function",
            name: "f",
            signature: "()",
        },
    )
    .unwrap();
    let sk_key = EntityKey::Symbol(crate::code_index::identity::symbol_key_from_material(
        &material,
    ));
    let json = serde_json::to_string(&sk_key).unwrap();
    let back: EntityKey = serde_json::from_str(&json).unwrap();
    assert_eq!(sk_key, back);

    let invalid = r#"{"kind":"symbol","id":"x"}"#;
    assert!(serde_json::from_str::<EntityKey>(invalid).is_err());
    let invalid = r#"{"kind":"call_site","id":"cs1:42"}"#;
    assert!(serde_json::from_str::<EntityKey>(invalid).is_err());
}

#[test]
fn derived_ids_are_stable_and_namespaced() {
    let material = crate::code_index::identity::symbol_key_material(
        &crate::code_index::identity::SymbolKeyMaterial {
            language: "java",
            project_key: "proj",
            module: "app",
            source_set: "main",
            package: "com.example",
            relative_path: "",
            type_chain: "Controller",
            kind: "method",
            name: "cancel",
            signature: "()",
        },
    )
    .unwrap();
    let owner = crate::code_index::identity::symbol_key_from_material(&material);
    let callsite = EntityKey::callsite_from_parts(&owner, "src/Controller.java", 10, 20, 0);
    assert_eq!(
        callsite,
        EntityKey::callsite_from_parts(&owner, "src/Controller.java", 10, 20, 0)
    );
    assert!(callsite.id().starts_with("cs1:"));
    assert_eq!(callsite.id().len(), 68);

    let entry = EntityKey::entry_from_parts("http_route", "app", &callsite, "POST /orders/cancel");
    let boundary = EntityKey::boundary_from_parts("proj", 9, &relation_id("source"), "external", 0);
    assert!(entry.id().starts_with("ep1:"));
    assert!(boundary.id().starts_with("bn1:"));

    let relation = relation_id_from_parts(
        &entry,
        Some(&boundary),
        RelationKind::Handles,
        Some("spring.request_mapping"),
        None,
        &[],
    );
    assert!(relation.starts_with("r1:"));
    assert_eq!(relation.len(), 67);

    let source_ref = sample_source_ref();
    let evidence = evidence_id_from_parts(
        EvidenceKind::Annotation,
        &[source_ref],
        Some("spring.request_mapping"),
        &"ab".repeat(32),
    );
    assert!(evidence.starts_with("e1:"));
    assert_eq!(evidence.len(), 67);
}

// ---------------------------------------------------------------------------
// RelationKind / Certainty / ResolutionState 语义
// ---------------------------------------------------------------------------

#[test]
fn relation_record_keeps_certainty_and_resolution_orthogonal() {
    // 接口调用：CALLS 目标确定（resolved + syntactic），但运行时实现是候选
    // ——确定性不替代分派候选，这正是 §4.4 要求的正交性。
    let relation = RelationRecord {
        relation_id: relation_id("declared-call"),
        source_key: entity("ep1", "entry"),
        target_key: None,
        kind: RelationKind::Calls,
        certainty: Certainty::Syntactic,
        resolution_state: ResolutionState::Resolved,
        rule_id: None,
        evidence_ids: vec![evidence_id("call")],
        candidate_group: None,
        conditions: vec![],
        heuristic_score: None,
    };
    assert_eq!(relation.certainty, Certainty::Syntactic);
    assert_eq!(relation.resolution_state, ResolutionState::Resolved);

    let json = serde_json::to_string(&relation).unwrap();
    let back: RelationRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(relation, back);
}

#[test]
fn relation_kind_serde_uses_snake_case() {
    let json = serde_json::to_string(&RelationKind::DispatchCandidate).unwrap();
    assert_eq!(json, "\"dispatch_candidate\"");
    let back: RelationKind = serde_json::from_str(&json).unwrap();
    assert_eq!(back, RelationKind::DispatchCandidate);
    assert_eq!(RelationKind::InjectsCandidate.as_str(), "injects_candidate");
}

// ---------------------------------------------------------------------------
// GraphSlice 不变量
// ---------------------------------------------------------------------------

fn fixture_snapshot() -> crate::code_index::identity::IndexSnapshot {
    crate::code_index::identity::IndexSnapshot {
        schema_version: 3,
        generation: 9,
        adapter_versions: vec![],
        manifest_digest: "ee".repeat(32),
        indexed_at: 0,
        coverage: Default::default(),
    }
}

#[test]
fn graph_slice_accepts_complete_endpoints() {
    let entry = entity("ep1", "entry");
    let boundary = entity("bn1", "boundary");
    let slice = GraphSlice {
        snapshot: fixture_snapshot(),
        nodes: vec![GraphSliceNode {
            key: entry.clone(),
            name: "A".to_string(),
            qualified_name: "com.example.A".to_string(),
            kind_label: "Class".to_string(),
            module: None,
        }],
        relations: vec![RelationRecord {
            relation_id: relation_id("entry-boundary"),
            source_key: entry,
            target_key: Some(boundary.clone()),
            kind: RelationKind::Calls,
            certainty: Certainty::Unknown,
            resolution_state: ResolutionState::Unresolved,
            rule_id: None,
            evidence_ids: vec![],
            candidate_group: None,
            conditions: vec![],
            heuristic_score: None,
        }],
        boundary_nodes: vec![BoundaryNode {
            key: boundary,
            name: "未解析调用".to_string(),
            reason: "unresolved call".to_string(),
        }],
        coverage: Default::default(),
        truncated: false,
        truncation_reasons: vec![],
        next_cursor: None,
    };
    slice.slice_invariants().expect("端点齐全应通过校验");
}

#[test]
fn graph_slice_rejects_dangling_endpoints() {
    // target 指向不存在节点。
    let slice = GraphSlice {
        snapshot: fixture_snapshot(),
        nodes: vec![GraphSliceNode {
            key: entity("ep1", "entry"),
            name: "A".to_string(),
            qualified_name: "A".to_string(),
            kind_label: "Class".to_string(),
            module: None,
        }],
        relations: vec![RelationRecord {
            relation_id: relation_id("dangling"),
            source_key: entity("ep1", "entry"),
            target_key: Some(entity("bn1", "missing-boundary")),
            kind: RelationKind::Calls,
            certainty: Certainty::Unknown,
            resolution_state: ResolutionState::Unresolved,
            rule_id: None,
            evidence_ids: vec![],
            candidate_group: None,
            conditions: vec![],
            heuristic_score: None,
        }],
        boundary_nodes: vec![],
        coverage: Default::default(),
        truncated: false,
        truncation_reasons: vec![],
        next_cursor: None,
    };
    assert!(
        slice.slice_invariants().is_err(),
        "悬空端点必须被拒绝，不得渲染伪关系"
    );
}

#[test]
fn graph_slice_rejects_missing_target_and_mistyped_boundary() {
    let entry = entity("ep1", "entry");
    let mut slice = GraphSlice {
        snapshot: fixture_snapshot(),
        nodes: vec![GraphSliceNode {
            key: entry.clone(),
            name: "A".to_string(),
            qualified_name: "A".to_string(),
            kind_label: "entry".to_string(),
            module: None,
        }],
        relations: vec![RelationRecord {
            relation_id: relation_id("missing-target"),
            source_key: entry.clone(),
            target_key: None,
            kind: RelationKind::Calls,
            certainty: Certainty::Unknown,
            resolution_state: ResolutionState::Unresolved,
            rule_id: None,
            evidence_ids: vec![],
            candidate_group: None,
            conditions: vec![],
            heuristic_score: None,
        }],
        boundary_nodes: vec![],
        coverage: Default::default(),
        truncated: false,
        truncation_reasons: vec![],
        next_cursor: None,
    };
    assert!(slice.slice_invariants().is_err());

    slice.relations[0].target_key = Some(entry.clone());
    slice.boundary_nodes.push(BoundaryNode {
        key: entry,
        name: "错误边界".to_string(),
        reason: "wrong kind".to_string(),
    });
    assert!(slice.slice_invariants().is_err());
}

// ---------------------------------------------------------------------------
// AnswerDocument 校验
// ---------------------------------------------------------------------------

fn sample_document() -> AnswerDocument {
    AnswerDocument {
        protocol_version: protocol_version(),
        project_key: "proj".to_string(),
        generation: 9,
        request_id: "req-1".to_string(),
        scope: "全项目".to_string(),
        summary: "取消订单在 OrderService".to_string(),
        claims: vec![AnswerClaim {
            id: "c1".to_string(),
            text: "Controller.cancel 调用 OrderService.cancelOrder".to_string(),
            evidence_ids: vec![evidence_id("call")],
            nature: Nature::Observed,
        }],
        steps: vec![AnswerStep {
            id: "s1".to_string(),
            title: "入口".to_string(),
            claim_ids: vec!["c1".to_string()],
            entity_keys: vec![entity("ep1", "entry")],
            relation_ids: vec![relation_id("declared-call")],
        }],
        focus_entity_keys: vec![],
        evidence_ids: vec![evidence_id("call")],
        uncertainties: vec![Uncertainty {
            kind: UncertaintyKind::AmbiguousCandidates,
            message: "存在两个实现候选".to_string(),
        }],
        coverage: Default::default(),
        completion_status: CompletionStatus::Partial,
    }
}

fn sample_source_ref() -> crate::code_index::identity::SourceRef {
    crate::code_index::identity::SourceRef::new(crate::code_index::identity::SourceRefParts {
        project_key: "proj".to_string(),
        generation: 9,
        relative_path: "src/Controller.java".to_string(),
        content_sha256: "aa".repeat(32),
        start_byte: 0,
        end_byte: 10,
        start_line: 1,
        end_line: 1,
    })
    .unwrap()
}

fn sample_bundle() -> EvidenceBundle {
    EvidenceBundle {
        records: vec![EvidenceRecord {
            evidence_id: evidence_id("call"),
            kind: EvidenceKind::CallSite,
            source_refs: vec![sample_source_ref()],
            rule_id: None,
            relation_ids: vec![relation_id("declared-call")],
            excerpt_digest: "ff".repeat(32),
        }],
    }
}

fn sample_graph() -> GraphSlice {
    let entry = entity("ep1", "entry");
    let target = entity("bn1", "external-target");
    GraphSlice {
        snapshot: fixture_snapshot(),
        nodes: vec![GraphSliceNode {
            key: entry.clone(),
            name: "Controller.cancel".to_string(),
            qualified_name: "com.example.Controller.cancel".to_string(),
            kind_label: "method".to_string(),
            module: None,
        }],
        relations: vec![RelationRecord {
            relation_id: relation_id("declared-call"),
            source_key: entry,
            target_key: Some(target.clone()),
            kind: RelationKind::Calls,
            certainty: Certainty::Syntactic,
            resolution_state: ResolutionState::External,
            rule_id: None,
            evidence_ids: vec![evidence_id("call")],
            candidate_group: None,
            conditions: vec![],
            heuristic_score: None,
        }],
        boundary_nodes: vec![BoundaryNode {
            key: target,
            name: "外部目标".to_string(),
            reason: "external".to_string(),
        }],
        coverage: Default::default(),
        truncated: false,
        truncation_reasons: vec![],
        next_cursor: None,
    }
}

#[test]
fn answer_document_validates_against_bundle() {
    let document = sample_document();
    validate_answer_document(&document, &sample_bundle(), &sample_graph())
        .expect("封闭证据集内的引用应通过");
}

#[test]
fn answer_document_rejects_unknown_evidence_and_claims() {
    let mut document = sample_document();
    // 未知证据 ID。
    document.evidence_ids.push("e-ghost".to_string());
    let bundle = EvidenceBundle { records: vec![] };
    assert!(
        validate_answer_document(&document, &bundle, &sample_graph()).is_err(),
        "bundle 外的证据引用必须拒绝"
    );

    // 未知 claim 引用。
    let mut document = sample_document();
    document.steps[0].claim_ids.push("c-ghost".to_string());
    assert!(validate_answer_document(&document, &sample_bundle(), &sample_graph()).is_err());
}

#[test]
fn answer_document_rejects_generation_and_protocol_mismatch() {
    let document = sample_document();
    let mut graph = sample_graph();
    graph.snapshot.generation = 8;
    assert!(
        validate_answer_document(&document, &sample_bundle(), &graph).is_err(),
        "代际不符必须拒绝（不跨代拼证据）"
    );
    let mut stale = sample_document();
    stale.protocol_version = protocol_version() + 1;
    assert!(validate_answer_document(&stale, &sample_bundle(), &sample_graph()).is_err());
}

#[test]
fn answer_document_rejects_fabricated_relation_and_unbacked_claim() {
    let mut document = sample_document();
    document.steps[0].relation_ids = vec![relation_id("fabricated")];
    assert!(validate_answer_document(&document, &sample_bundle(), &sample_graph()).is_err());

    let mut document = sample_document();
    document.claims[0].evidence_ids.clear();
    assert!(validate_answer_document(&document, &sample_bundle(), &sample_graph()).is_err());
}

#[test]
fn answer_document_rejects_cross_generation_source_ref() {
    let document = sample_document();
    let mut bundle = sample_bundle();
    bundle.records[0].source_refs[0].generation = 8;
    assert!(validate_answer_document(&document, &bundle, &sample_graph()).is_err());
}

// ---------------------------------------------------------------------------
// ErrorCode
// ---------------------------------------------------------------------------

#[test]
fn error_code_defaults_and_display() {
    assert!(!ErrorCode::IndexMissing.retryable());
    assert!(ErrorCode::IndexBusy.retryable());
    assert!(!ErrorCode::OutsideProject.retryable(), "越界读取不可重试");
    assert_eq!(ErrorCode::SourceChanged.to_string(), "源码已变化");
}

#[test]
fn understanding_error_display_includes_code() {
    let e = UnderstandingError::new(ErrorCode::SourceChanged, "Order.java 已被外部修改");
    assert!(e.retryable());
    let text = e.to_string();
    assert!(text.contains("SourceChanged"));
    assert!(text.contains("Order.java"));
    // 转 GitError 不丢内容。
    let git_err: GitError = e.into();
    assert!(git_err.to_string().contains("Order.java"));
}

#[test]
fn schema_v3_sql_blocks_execute_with_foreign_keys_enabled() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/code-understanding/validation/schema-v3-ddl.md");
    let markdown = std::fs::read_to_string(path).expect("schema v3 文档应可读取");
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    let mut in_sql = false;
    let mut block = String::new();
    let mut count = 0;
    for line in markdown.lines() {
        if line.trim() == "```sql" {
            in_sql = true;
            block.clear();
        } else if in_sql && line.trim() == "```" {
            conn.execute_batch(&block)
                .unwrap_or_else(|error| panic!("DDL SQL 块执行失败：{error}\n{block}"));
            in_sql = false;
            count += 1;
        } else if in_sql {
            block.push_str(line);
            block.push('\n');
        }
    }
    assert!(count >= 10, "应执行全部 DDL SQL 块");

    let invalid_key = format!("sk1:{}", "A".repeat(64));
    assert!(
        conn.execute(
            "INSERT INTO entities(entity_key, entity_kind, rel_path) VALUES (?1, 'symbol', '')",
            [&invalid_key],
        )
        .is_err(),
        "DDL 必须拒绝非小写 hex 实体键"
    );

    let symbol_key = namespaced_id("sk1", "owner");
    let callsite_key = namespaced_id("cs1", "callsite");
    conn.execute(
        "INSERT INTO entities(entity_key, entity_kind, rel_path) VALUES (?1, 'symbol', 'src/A.java')",
        [&symbol_key],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO entities(entity_key, entity_kind, rel_path) VALUES (?1, 'callsite', 'src/A.java')",
        [&callsite_key],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_hashes(rel_path, sha256) VALUES ('src/A.java', ?1)",
        ["aa".repeat(32)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO nodes(id, label, name, qualified_name, symbol_key) VALUES (1, 'Method', 'run', 'A.run', ?1)",
        [&symbol_key],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO call_sites(callsite_key, owner_key, rel_path, start_byte, end_byte, start_line, end_line, content_sha256, callee_expr, resolution) VALUES (?1, ?2, 'src/A.java', 0, 3, 1, 1, ?3, 'run', '{}')",
        rusqlite::params![callsite_key, symbol_key, "aa".repeat(32)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relations(relation_id, source_key, target_key, kind, certainty, resolution_state) VALUES (?1, ?2, ?3, 'calls', 'syntactic', 'resolved')",
        rusqlite::params![relation_id("fk"), symbol_key, callsite_key],
    )
    .unwrap();
    conn.execute("DELETE FROM entities WHERE entity_key = ?1", [&symbol_key])
        .unwrap();
    let remaining_relations: i64 = conn
        .query_row("SELECT count(*) FROM relations", [], |row| row.get(0))
        .unwrap();
    let remaining_callsites: i64 = conn
        .query_row("SELECT count(*) FROM call_sites", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining_relations, 0, "删除 owner 实体必须级联清理关系");
    assert_eq!(
        remaining_callsites, 0,
        "删除 owner 节点必须级联清理调用点明细"
    );

    let violations: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}
