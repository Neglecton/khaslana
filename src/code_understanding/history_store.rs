// 代码理解的本地完成历史（CU2-T5）。
//
// 只保存结构有效的 `Completed` 或已明确标出未完成范围的终态 `Partial`
// （development.md §9.1 固定规则）；Failed、Cancelled、流式半截正文与
// 截断/坏 JSON 结果一律不落盘。历史按仓库隔离（`<数据目录>/code-understanding/
// <repo哈希8>/<毫秒时间戳>.json`），不做云同步、团队分享或跨仓库合并。
//
// 存储形态复用 AI 评审记录（`ai/review_store.rs`）的仓库隔离与坏文件跳过
// 方式；按开发文档 §9.4 要求，写入采用临时文件加原子替换——评审记录以
// 「一条一个唯一文件名」天然免覆盖，这里仍按文档补 rename 原子性，防止
// 半截 JSON 落盘。来源以自有可往返结构存储：`SourceRef` 只有 `Serialize`
// 且构造带校验，历史上读取时经 `to_source_ref` 重建并保持同一套约束。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::code_index::SourceRef;
use crate::types::{GitError, Result};

use super::agent::UnderstandingStep;
use super::analysis::{AnalysisCompletion, AnalysisResult};
use super::source::source_id_of;
use super::{ErrorCode, UnderstandingError};

/// 记录格式版本；读取时版本不符的记录跳过（视为坏文件）。
pub const UNDERSTANDING_HISTORY_FORMAT_VERSION: u32 = 1;
/// 每个仓库保留的完成记录上限，超出时删除最旧的。
pub const UNDERSTANDING_HISTORY_MAX_RECORDS: usize = 30;
/// 历史弹窗首屏列出的记录条数。
pub const UNDERSTANDING_HISTORY_LIST_LIMIT: usize = 20;

/// 可往返的来源快照。与 `SourceRef` 字段一一对应；读取时经 `to_source_ref`
/// 重建（构造校验保证范围/hash 结构有效，与工具发放时同一套规则）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandingSourceRecord {
    pub project_key: String,
    pub generation: u64,
    pub relative_path: String,
    pub content_sha256: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
}

impl UnderstandingSourceRecord {
    pub fn from_source_ref(source: &SourceRef) -> Self {
        Self {
            project_key: source.project_key.clone(),
            generation: source.generation,
            relative_path: source.relative_path.clone(),
            content_sha256: source.content_sha256.clone(),
            start_byte: source.start_byte,
            end_byte: source.end_byte,
            start_line: source.start_line,
            end_line: source.end_line,
        }
    }

    /// 重建校验过的 `SourceRef`；结构非法时给出中文错误。
    pub fn to_source_ref(&self) -> std::result::Result<SourceRef, UnderstandingError> {
        SourceRef::new(crate::code_index::SourceRefParts {
            project_key: self.project_key.clone(),
            generation: self.generation,
            relative_path: self.relative_path.clone(),
            content_sha256: self.content_sha256.clone(),
            start_byte: self.start_byte,
            end_byte: self.end_byte,
            start_line: self.start_line,
            end_line: self.end_line,
        })
        .map_err(|error| {
            UnderstandingError::new(
                ErrorCode::AnswerInvalid,
                format!("历史来源结构无效：{error}"),
            )
        })
    }

    pub fn source_id(&self) -> String {
        self.to_source_ref()
            .map(|source| source_id_of(&source))
            .unwrap_or_default()
    }
}

/// 一条持久化的代码理解完成记录（写入时快照）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnderstandingHistoryRecord {
    pub format_version: u32,
    /// 仓库绝对路径（项目键）。
    pub project_key: String,
    pub session_key: String,
    /// 请求代际（会话内递增；恢复后仅作展示，不参与路由）。
    pub generation: u64,
    pub question: String,
    /// complete / partial（partial 须已明确标出未查完范围才被允许写入）。
    pub completion: AnalysisCompletion,
    pub analysis: AnalysisResult,
    pub steps: Vec<UnderstandingStep>,
    pub reasoning: Option<String>,
    /// 本问实际发放且完成时通过 hash 复验的来源。
    pub sources: Vec<UnderstandingSourceRecord>,
    pub model: String,
    pub started_at_millis: u64,
    pub finished_at_millis: u64,
    pub duration_secs: u64,
    /// 回答生成时绑定的索引代际（`AnalysisResult.context.index_generation` 同源）。
    pub index_generation: u64,
}

/// 某仓库的历史目录：`<base>/code-understanding/<repo哈希8>/`。
/// repo 键复用 `ai::review_store::repo_key`（FNV-1a + 小写折叠），与索引库、
/// 评审记录共用同一套仓库标识约定。
fn history_dir(base_dir: &Path, repo_path: &str) -> PathBuf {
    base_dir
        .join("code-understanding")
        .join(crate::ai::review_store::repo_key(repo_path))
}

/// 保存一条完成记录，返回文件名 id（完成时刻毫秒，同毫秒冲突加 `-2`/`-3`）。
/// 写入走「临时文件 + 原子替换」：先写 `<id>.json.tmp` 再 rename 到目标，
/// 进程中断也不会留下半截 JSON。
pub fn save_understanding_history_record(
    base_dir: &Path,
    record: UnderstandingHistoryRecord,
) -> Result<String> {
    let dir = history_dir(base_dir, &record.project_key);
    fs::create_dir_all(&dir)
        .map_err(|err| GitError::Message(format!("创建代码理解历史目录失败：{err}")))?;

    let id = unique_record_id(&dir, record.finished_at_millis);
    let path = dir.join(format!("{id}.json"));
    let json = serde_json::to_string(&record)
        .map_err(|err| GitError::Message(format!("序列化代码理解历史失败：{err}")))?;
    let tmp_path = dir.join(format!("{id}.json.tmp"));
    fs::write(&tmp_path, json)
        .map_err(|err| GitError::Message(format!("写入代码理解历史失败：{err}")))?;
    // 目标 id 由 unique_record_id 保证不存在，Windows 上 rename 到不存在
    // 的目标同样是原子替换语义。
    fs::rename(&tmp_path, &path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        GitError::Message(format!("保存代码理解历史失败：{err}"))
    })?;

    prune_records(&dir);
    Ok(id)
}

/// 生成不冲突的记录 id：毫秒时间戳为主，冲突时追加序号后缀。
fn unique_record_id(dir: &Path, millis: u64) -> String {
    let base = millis.to_string();
    if !dir.join(format!("{base}.json")).exists() {
        return base;
    }
    for n in 2u32.. {
        let candidate = format!("{base}-{n}");
        if !dir.join(format!("{candidate}.json")).exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// 将 `<毫秒时间戳>[-序号].json` 转为稳定的时间排序键。
///
/// 不能直接按文件名字典序排序：测试、导入或时间异常时可能出现不同位数的
/// 时间戳，此时 `10.json` 会错误地排在 `2.json` 前面。
fn record_name_sort_key(name: &str) -> (u64, u32) {
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let (millis, sequence) = stem
        .split_once('-')
        .map_or((stem, "1"), |(millis, sequence)| (millis, sequence));
    (millis.parse().unwrap_or(0), sequence.parse().unwrap_or(0))
}

/// 只保留最近 `UNDERSTANDING_HISTORY_MAX_RECORDS` 条。文件名按数值时间戳与
/// 同毫秒序号排序，避免不同位数的时间戳被字典序排错。
fn prune_records(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if names.len() <= UNDERSTANDING_HISTORY_MAX_RECORDS {
        return;
    }
    names.sort_by(|left, right| {
        record_name_sort_key(left)
            .cmp(&record_name_sort_key(right))
            .then_with(|| left.cmp(right))
    });
    let excess = names.len() - UNDERSTANDING_HISTORY_MAX_RECORDS;
    for name in names.into_iter().take(excess) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// 列出某仓库最近的完成记录（按完成时间倒序，最多 `limit` 条）。
/// 单个文件损坏（坏 JSON / 版本不符 / 来源结构非法）跳过不报错（仅记
/// warn），不让一条坏记录拖垮整个历史列表；读取失败同样不算请求失败。
pub fn list_understanding_history_records(
    base_dir: &Path,
    repo_path: &str,
    limit: usize,
) -> Result<Vec<UnderstandingHistoryRecord>> {
    let dir = history_dir(base_dir, repo_path);
    if limit == 0 || !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&dir)
        .map_err(|err| GitError::Message(format!("读取代码理解历史失败：{err}")))?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort_by(|left, right| {
        record_name_sort_key(left)
            .cmp(&record_name_sort_key(right))
            .then_with(|| left.cmp(right))
    });
    names.reverse();

    let mut records = Vec::new();
    for name in names {
        if records.len() >= limit {
            break;
        }
        let path = dir.join(name);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<UnderstandingHistoryRecord>(&text) {
            Ok(record) if record.format_version == UNDERSTANDING_HISTORY_FORMAT_VERSION => {
                records.push(record)
            }
            Ok(record) => tracing::warn!(
                target: "khaslana::code_understanding",
                "跳过版本不符的历史记录 {}（format_version={}）",
                path.display(),
                record.format_version
            ),
            Err(err) => tracing::warn!(
                target: "khaslana::code_understanding",
                "跳过损坏的代码理解历史 {}: {err}",
                path.display()
            ),
        }
    }
    Ok(records)
}

#[cfg(test)]
#[path = "../tests/code_understanding_history.rs"]
mod tests;
