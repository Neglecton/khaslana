//! 原始事实与覆盖诊断（DEV-03）。图只是兼容投影；这里保留解析器实际
//! 看见的定义、导入和每一个调用点，未解析调用不会在关系阶段被丢掉。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileFacts {
    pub rel_path: String,
    pub sha256: String,
    pub adapter_versions_json: String,
    pub parse_status: String,
    pub facts_json: String,
    pub extracted_at: u64,
}

impl FileFacts {
    pub fn from_json(
        rel_path: String,
        sha256: String,
        parse_status: &str,
        facts: serde_json::Value,
        extracted_at: u64,
    ) -> Self {
        let adapter_versions = if facts.get("java").is_some_and(|value| !value.is_null()) {
            vec![("generic", "2"), ("java", "1")]
        } else {
            vec![("generic", "2")]
        };
        Self {
            rel_path,
            sha256,
            adapter_versions_json: serde_json::to_string(&adapter_versions)
                .expect("固定适配器版本可序列化"),
            parse_status: parse_status.to_string(),
            facts_json: serde_json::to_string(&facts).expect("原始事实可序列化"),
            extracted_at,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallFact {
    pub owner_qn: Option<String>,
    pub rel_path: String,
    pub sha256: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub receiver_expr: String,
    pub callee_expr: String,
    pub argument_exprs: Vec<String>,
    pub resolution: String,
    pub target_qn: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct DiagnosticFact {
    pub rel_path: String,
    pub adapter: String,
    pub code: String,
    pub detail: String,
    pub range_json: String,
}

#[derive(Clone, Debug, Default)]
pub struct SnapshotFacts {
    pub files_discovered: u64,
    pub excluded_count: u64,
    pub files: Vec<FileFacts>,
    pub calls: Vec<CallFact>,
    pub diagnostics: Vec<DiagnosticFact>,
}
