// identity.rs（稳定键/SourceRef/快照元数据）的单元测试，通过 #[path] 挂载于
// src/code_index/identity.rs，use super::* 可访问其私有项。

use super::*;

// ---------------------------------------------------------------------------
// SymbolKey
// ---------------------------------------------------------------------------

#[test]
fn symbol_key_is_idempotent_and_versioned() {
    let material = symbol_key_material(&SymbolKeyMaterial {
        language: "java",
        project_key: "d1e2f3a4",
        module: "app-core",
        source_set: "main",
        package: "com.example.alpha",
        relative_path: "",
        type_chain: "Runner",
        kind: "method",
        name: "run",
        signature: "(String)",
    })
    .unwrap();
    let a = symbol_key_from_material(&material);
    let b = symbol_key_from_material(&material);
    assert_eq!(a, b, "同一材料必须得到同一键（幂等）");
    assert!(a.as_str().starts_with("sk1:"));
    assert_eq!(a.digest().len(), 64);
    assert!(a.digest().bytes().all(|c| c.is_ascii_hexdigit()));
    // 键随材料语义变化：任一字段不同应产生不同键。
    let other = symbol_key_from_material(
        &symbol_key_material(&SymbolKeyMaterial {
            language: "java",
            project_key: "d1e2f3a4",
            module: "app-core",
            source_set: "main",
            package: "com.example.alpha",
            relative_path: "",
            type_chain: "Runner",
            kind: "method",
            name: "run",
            signature: "(int)",
        })
        .unwrap(),
    );
    assert_ne!(a, other, "签名不同不得得到同一键");
}

#[test]
fn symbol_key_parse_rejects_invalid_input() {
    let material = symbol_key_material(&SymbolKeyMaterial {
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
    })
    .unwrap();
    let key = symbol_key_from_material(&material);
    // 合法键可往返。
    let parsed = parse_symbol_key(key.as_str()).expect("合法键应可解析");
    assert_eq!(parsed, key);
    // 前缀错误。
    assert!(parse_symbol_key("xx:abcd").is_err());
    assert!(parse_symbol_key("abcd").is_err());
    // 摘要长度/字符错误。
    assert!(parse_symbol_key("sk1:abcd").is_err());
    assert!(parse_symbol_key("sk1:zzzz").is_err());
    let long_non_hex = format!("sk1:{}", "g".repeat(64));
    assert!(parse_symbol_key(&long_non_hex).is_err());
    assert!(parse_symbol_key(&format!("sk1:{}", "A".repeat(64))).is_err());
    let invalid_json = format!("\"sk1:{}\"", "A".repeat(64));
    assert!(serde_json::from_str::<SymbolKey>(&invalid_json).is_err());
}

#[test]
fn symbol_key_fields_do_not_bleed_across_boundaries() {
    // 字段错位以及字段内出现旧分隔符时均不得产生相同键。
    let a = symbol_key_material(&SymbolKeyMaterial {
        language: "java",
        project_key: "ab",
        module: "c\u{1f}d",
        source_set: "",
        package: "",
        relative_path: "",
        type_chain: "",
        kind: "method",
        name: "f",
        signature: "()",
    })
    .unwrap();
    let b = symbol_key_material(&SymbolKeyMaterial {
        language: "java",
        project_key: "ab\u{1f}c",
        module: "d",
        source_set: "",
        package: "",
        relative_path: "",
        type_chain: "",
        kind: "method",
        name: "f",
        signature: "()",
    })
    .unwrap();
    assert_ne!(
        symbol_key_from_material(&a),
        symbol_key_from_material(&b),
        "project=ab/module=c 与 project=a/module=bc 是不同身份"
    );
}

#[test]
fn ordinary_language_symbol_key_includes_relative_path() {
    let base = SymbolKeyMaterial {
        language: "rust",
        project_key: "p",
        module: "",
        source_set: "",
        package: "",
        relative_path: "src/a.rs",
        type_chain: "",
        kind: "function",
        name: "run",
        signature: "()",
    };
    let mut moved = base;
    moved.relative_path = "src/b.rs";
    assert_ne!(
        symbol_key_from_material(&symbol_key_material(&base).unwrap()),
        symbol_key_from_material(&symbol_key_material(&moved).unwrap())
    );
}

#[test]
fn symbol_key_material_requires_path_for_ordinary_languages() {
    let material = SymbolKeyMaterial {
        language: "rust",
        project_key: "p",
        module: "",
        source_set: "",
        package: "",
        relative_path: "",
        type_chain: "",
        kind: "function",
        name: "run",
        signature: "()",
    };
    assert!(symbol_key_material(&material).is_err());
}

// ---------------------------------------------------------------------------
// content_fingerprint
// ---------------------------------------------------------------------------

#[test]
fn content_fingerprint_matches_known_sha256() {
    assert_eq!(
        content_fingerprint(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let hex = content_fingerprint(b"abc");
    assert_eq!(
        hex,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// ---------------------------------------------------------------------------
// SourceRef
// ---------------------------------------------------------------------------

#[test]
fn source_ref_accepts_valid_range() {
    let r = SourceRef::new(SourceRefParts {
        project_key: "proj".to_string(),
        generation: 7,
        relative_path: "src/main.rs".to_string(),
        content_sha256: "aa".repeat(32),
        start_byte: 0,
        end_byte: 100,
        start_line: 1,
        end_line: 5,
    })
    .expect("合法区间应可构造");
    assert_eq!(r.start_byte, 0);
    assert_eq!(r.end_byte, 100);
    assert_eq!(r.start_line, 1);
}

#[test]
fn source_ref_rejects_invalid_ranges() {
    let fp = "bb".repeat(32);
    let valid = || SourceRefParts {
        project_key: "p".to_string(),
        generation: 1,
        relative_path: "a.rs".to_string(),
        content_sha256: fp.clone(),
        start_byte: 0,
        end_byte: 10,
        start_line: 1,
        end_line: 2,
    };
    // end < start。
    let mut parts = valid();
    parts.start_byte = 50;
    assert!(
        SourceRef::new(parts).is_err(),
        "字节区间 end < start 必须拒绝"
    );
    // 行号 0。
    let mut parts = valid();
    parts.start_line = 0;
    assert!(SourceRef::new(parts).is_err(), "行号必须从 1 开始");
    // end_line < start_line。
    let mut parts = valid();
    parts.start_line = 3;
    assert!(SourceRef::new(parts).is_err());
    // 缺指纹。
    let mut parts = valid();
    parts.content_sha256.clear();
    assert!(SourceRef::new(parts).is_err());
    let mut parts = valid();
    parts.content_sha256 = "AA".repeat(32);
    assert!(SourceRef::new(parts).is_err());
    // 缺路径。
    let mut parts = valid();
    parts.relative_path.clear();
    assert!(SourceRef::new(parts).is_err());
    let mut parts = valid();
    parts.relative_path = "../a.rs".to_string();
    assert!(SourceRef::new(parts).is_err());
    let mut parts = valid();
    parts.relative_path = "C:/a.rs".to_string();
    assert!(SourceRef::new(parts).is_err());
}

#[test]
fn source_ref_serde_roundtrip_preserves_invariants() {
    let r = SourceRef::new(SourceRefParts {
        project_key: "proj".to_string(),
        generation: 3,
        relative_path: "java/X.java".to_string(),
        content_sha256: "cc".repeat(32),
        start_byte: 12,
        end_byte: 48,
        start_line: 2,
        end_line: 4,
    })
    .unwrap();
    let json = serde_json::to_string(&r).unwrap();
    let back: SourceRef = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
    back.validate().expect("往返后不变量保持");

    let invalid = json.replace("java/X.java", "../X.java");
    assert!(serde_json::from_str::<SourceRef>(&invalid).is_err());
}

// ---------------------------------------------------------------------------
// IndexSnapshot / CoverageSummary / ProjectContext
// ---------------------------------------------------------------------------

#[test]
fn index_snapshot_serde_roundtrip() {
    let snapshot = IndexSnapshot {
        schema_version: 3,
        generation: 42,
        adapter_versions: vec![
            ("generic".to_string(), "1".to_string()),
            ("java".to_string(), "1".to_string()),
        ],
        manifest_digest: "dd".repeat(32),
        indexed_at: 1_700_000_000_000,
        coverage: CoverageSummary {
            files_discovered: 100,
            files_parsed: 90,
            files_partial: 5,
            files_skipped: 5,
            files_unreadable: 0,
            calls_total: 500,
            calls_resolved: 400,
            calls_ambiguous: 40,
            calls_unresolved: 50,
            calls_external: 10,
        },
    };
    assert_eq!(snapshot.coverage.total_calls(), 500);
    snapshot.coverage.validate().unwrap();
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: IndexSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snapshot, back);
}

#[test]
fn coverage_rejects_inconsistent_totals() {
    let coverage = CoverageSummary {
        files_discovered: 1,
        files_parsed: 1,
        calls_total: 2,
        calls_resolved: 1,
        ..Default::default()
    };
    assert!(coverage.validate().is_err());
}

#[test]
fn project_context_roundtrip() {
    let ctx = ProjectContext {
        project_key: "d:\\repo".to_string(),
        canonical_root: "D:\\Repo".to_string(),
        index_db_path: "D:\\data\\code-index\\abcd1234\\index.db".to_string(),
    };
    let json = serde_json::to_string(&ctx).unwrap();
    let back: ProjectContext = serde_json::from_str(&json).unwrap();
    assert_eq!(ctx, back);
}
