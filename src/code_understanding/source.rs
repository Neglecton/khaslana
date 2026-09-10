//! V2 代码理解使用的只读源码访问层。
//!
//! 这里刻意不复用 GitService：理解 agent 读取的是当前项目工作区，并且工具面
//! 只能看到索引发现规则允许的文本文件。每次会话固定一份允许文件清单，来源
//! 引用绑定内容 hash；文件变化后旧引用立即失效。

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ai::review_store::repo_key;
use crate::code_index::{
    ProjectContext, SourceRef, SourceRefParts, content_fingerprint, discover_files, sha256_hex,
};

use super::{ErrorCode, UnderstandingError};

pub type UnderstandingResult<T> = std::result::Result<T, UnderstandingError>;

pub const SOURCE_FILE_MAX_BYTES: u64 = 1024 * 1024;
pub const SOURCE_READ_MAX_LINES: usize = 200;
pub const SOURCE_READ_MAX_CHARS: usize = 8_000;
pub const SOURCE_SEARCH_MAX_FILES: usize = 1_000;
pub const SOURCE_SEARCH_MAX_RESULTS: usize = 50;
pub const SOURCE_TREE_MAX_DEPTH: usize = 4;
pub const SOURCE_TREE_MAX_ENTRIES: usize = 200;
const SOURCE_SEARCH_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLine {
    pub line_number: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRead {
    pub source_id: String,
    pub source_ref: SourceRef,
    pub lines: Vec<SourceLine>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSearchMatch {
    pub relative_path: String,
    pub line_number: u32,
    pub line_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSearchResult {
    pub matches: Vec<SourceSearchMatch>,
    pub scanned_files: usize,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTreeEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeEntry {
    pub relative_path: String,
    pub kind: FileTreeEntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeResult {
    pub entries: Vec<FileTreeEntry>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct AllowedFile {
    absolute_path: PathBuf,
    size: u64,
}

/// 单次理解会话的只读来源注册表。
pub struct SourceService {
    context: ProjectContext,
    canonical_root: PathBuf,
    generation: u64,
    allowed_files: BTreeMap<String, AllowedFile>,
    issued_sources: Mutex<HashMap<String, SourceRef>>,
}

impl SourceService {
    pub fn open(context: ProjectContext, generation: u64) -> UnderstandingResult<Self> {
        let canonical_root = std::fs::canonicalize(&context.canonical_root).map_err(|error| {
            understanding_error(
                ErrorCode::SourceMissing,
                format!("项目根目录不存在或无法读取：{error}"),
            )
        })?;
        if !canonical_root.is_dir() {
            return Err(understanding_error(
                ErrorCode::SourceMissing,
                "项目根路径不是目录",
            ));
        }
        let actual_key = repo_key(&canonical_root.to_string_lossy());
        if actual_key != context.project_key {
            return Err(understanding_error(
                ErrorCode::OutsideProject,
                "项目键与项目根目录不匹配",
            ));
        }

        let discovered = discover_files(&canonical_root).map_err(|error| {
            understanding_error(
                ErrorCode::SourceMissing,
                format!("读取项目文件清单失败：{error}"),
            )
        })?;
        let allowed_files = discovered
            .files
            .into_iter()
            .map(|file| {
                (
                    file.rel_path,
                    AllowedFile {
                        absolute_path: file.abs_path,
                        size: file.size,
                    },
                )
            })
            .collect();

        Ok(Self {
            context,
            canonical_root,
            generation,
            allowed_files,
            issued_sources: Mutex::new(HashMap::new()),
        })
    }

    pub fn context(&self) -> &ProjectContext {
        &self.context
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 诊断：返回原始 map 的 (key, computed_id) 对。
    pub(crate) fn debug_keys(&self) -> Vec<(String, String)> {
        let map = self.issued_sources.lock().expect("来源注册表锁被污染");
        let mut out: Vec<(String, String)> = map
            .iter()
            .map(|(key, value): (&String, &SourceRef)| (key.clone(), source_id(value)))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// 本会话已发放的来源快照（按路径、起始行排序）；用于生成检索范围摘要。
    pub fn issued_sources(&self) -> Vec<SourceRef> {
        let mut sources: Vec<SourceRef> = self
            .issued_sources
            .lock()
            .expect("来源注册表锁被污染")
            .values()
            .cloned()
            .collect();
        sources.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then(left.start_line.cmp(&right.start_line))
        });
        sources
    }

    pub fn read_file(
        &self,
        relative_path: &str,
        start_line: u32,
        end_line: u32,
    ) -> UnderstandingResult<SourceRead> {
        if start_line == 0 || end_line < start_line {
            return Err(understanding_error(
                ErrorCode::AnswerInvalid,
                "读取行范围无效：行号从 1 开始，结束行不得小于起始行",
            ));
        }
        let relative_path = normalize_relative_file_path(relative_path)?;
        let bytes = self.read_allowed_bytes(&relative_path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            understanding_error(
                ErrorCode::EncodingUnsupported,
                format!("文件不是 UTF-8 文本：{relative_path}"),
            )
        })?;
        let spans = line_spans(text.as_bytes());
        if start_line as usize > spans.len() {
            return Err(understanding_error(
                ErrorCode::SourceMissing,
                format!("起始行超出文件范围：{relative_path}:{start_line}"),
            ));
        }

        let requested_end = end_line as usize;
        let max_end = (start_line as usize + SOURCE_READ_MAX_LINES - 1).min(spans.len());
        let selected_end = requested_end.min(max_end);
        let mut lines = Vec::new();
        let mut chars = 0usize;
        let mut actual_end_byte = spans[start_line as usize - 1].0;
        let mut char_limited = false;

        for (line_index, &(line_start, line_end)) in spans
            .iter()
            .enumerate()
            .take(selected_end)
            .skip(start_line as usize - 1)
        {
            let raw_line = &text[line_start..line_end];
            let line_text = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let remaining = SOURCE_READ_MAX_CHARS.saturating_sub(chars);
            if remaining == 0 {
                char_limited = true;
                break;
            }
            let (shown, complete) = take_chars(line_text, remaining);
            let shown_bytes = shown.len();
            lines.push(SourceLine {
                line_number: line_index as u32 + 1,
                text: shown.to_string(),
            });
            chars += shown.chars().count();
            if complete {
                actual_end_byte = line_end;
            } else {
                actual_end_byte = line_start + shown_bytes;
                char_limited = true;
                break;
            }
        }

        let actual_end_line = lines
            .last()
            .map(|line| line.line_number)
            .unwrap_or(start_line);
        let line_limited = requested_end > max_end && max_end < spans.len();
        let truncated = char_limited || line_limited;
        let truncation_reason = if char_limited {
            Some(format!("单次读取最多返回 {SOURCE_READ_MAX_CHARS} 个字符"))
        } else if line_limited {
            Some(format!("单次读取最多返回 {SOURCE_READ_MAX_LINES} 行"))
        } else {
            None
        };
        let source_ref = SourceRef::new(SourceRefParts {
            project_key: self.context.project_key.clone(),
            generation: self.generation,
            relative_path: relative_path.clone(),
            content_sha256: content_fingerprint(&bytes),
            start_byte: spans[start_line as usize - 1].0 as u64,
            end_byte: actual_end_byte as u64,
            start_line,
            end_line: actual_end_line,
        })
        .map_err(|error| {
            understanding_error(
                ErrorCode::AnswerInvalid,
                format!("创建源码引用失败：{error}"),
            )
        })?;
        let source_id = source_id(&source_ref);
        self.issued_sources
            .lock()
            .expect("来源注册表锁被污染")
            .insert(source_id.clone(), source_ref.clone());

        Ok(SourceRead {
            source_id,
            source_ref,
            lines,
            truncated,
            truncation_reason,
        })
    }

    /// 只接受本会话实际发放的来源 ID，并重新校验文件内容。
    pub fn validate_source(&self, source_id: &str) -> UnderstandingResult<SourceRef> {
        let source_ref = self
            .issued_sources
            .lock()
            .expect("来源注册表锁被污染")
            .get(source_id)
            .cloned()
            .ok_or_else(|| {
                if std::env::var_os("KHASLANA_DEBUG_SOURCES").is_some() {
                    eprintln!(
                        "DEBUG source miss: len={} escaped={:?}",
                        source_id.len(),
                        source_id
                    );
                    for (key, value) in self
                        .issued_sources
                        .lock()
                        .expect("来源注册表锁被污染")
                        .iter()
                    {
                        eprintln!(
                            "DEBUG   key_len={} key={:?} value_id={:?}",
                            key.len(),
                            key,
                            source_id_of(value)
                        );
                    }
                }
                understanding_error(ErrorCode::AnswerInvalid, "来源 ID 不是本次会话发放")
            })?;
        self.validate_source_ref(&source_ref)
    }

    /// 重新校验已完成答案中保存的来源；用于追问历史和来源点击。
    ///
    /// 与 [`Self::validate_source`] 不同，这里不要求来源仍在当前工具实例的发放表中，
    /// 但仍严格核对项目、索引代际、允许路径、文件 hash 和字节范围。
    pub fn validate_source_ref(&self, source_ref: &SourceRef) -> UnderstandingResult<SourceRef> {
        if source_ref.project_key != self.context.project_key {
            return Err(understanding_error(
                ErrorCode::OutsideProject,
                "来源不属于当前项目",
            ));
        }
        if source_ref.generation != self.generation {
            return Err(understanding_error(
                ErrorCode::GenerationMismatch,
                "来源对应的索引代际已失效",
            ));
        }
        let bytes = self.read_allowed_bytes(&source_ref.relative_path)?;
        if content_fingerprint(&bytes) != source_ref.content_sha256 {
            return Err(understanding_error(
                ErrorCode::SourceChanged,
                format!("源码已变化：{}", source_ref.relative_path),
            ));
        }
        if source_ref.end_byte > bytes.len() as u64 {
            return Err(understanding_error(
                ErrorCode::SourceChanged,
                format!("源码范围已失效：{}", source_ref.relative_path),
            ));
        }
        Ok(source_ref.clone())
    }

    pub fn search_code(
        &self,
        query: &str,
        is_regex: bool,
        path_prefix: Option<&str>,
        suffix: Option<&str>,
        limit: usize,
    ) -> UnderstandingResult<SourceSearchResult> {
        if query.trim().is_empty() {
            return Err(understanding_error(
                ErrorCode::AnswerInvalid,
                "搜索内容不能为空",
            ));
        }
        let prefix = path_prefix
            .filter(|value| !value.trim().is_empty())
            .map(normalize_relative_directory_path)
            .transpose()?;
        let suffix = suffix
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_ascii_lowercase());
        let regex = is_regex
            .then(|| Regex::new(query))
            .transpose()
            .map_err(|error| {
                understanding_error(ErrorCode::AnswerInvalid, format!("正则表达式无效：{error}"))
            })?;
        let limit = limit.clamp(1, SOURCE_SEARCH_MAX_RESULTS);
        let started = Instant::now();
        let mut matches = Vec::new();
        let mut scanned_files = 0usize;
        let mut truncation_reasons = Vec::new();

        for (relative_path, allowed) in &self.allowed_files {
            if let Some(prefix) = prefix.as_deref()
                && !is_under_prefix(relative_path, prefix)
            {
                continue;
            }
            if let Some(suffix) = suffix.as_deref()
                && !relative_path.to_ascii_lowercase().ends_with(suffix)
            {
                continue;
            }
            if scanned_files >= SOURCE_SEARCH_MAX_FILES {
                truncation_reasons.push(format!(
                    "候选文件超过 {SOURCE_SEARCH_MAX_FILES} 个，已返回部分结果"
                ));
                break;
            }
            if started.elapsed() >= SOURCE_SEARCH_DEADLINE {
                truncation_reasons.push("本地搜索达到 2 秒截止时间，已返回部分结果".to_string());
                break;
            }
            scanned_files += 1;
            if allowed.size > SOURCE_FILE_MAX_BYTES {
                continue;
            }
            let Ok(bytes) = self.read_allowed_bytes(relative_path) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                let matched = regex
                    .as_ref()
                    .map(|regex| regex.is_match(line))
                    .unwrap_or_else(|| line.contains(query));
                if !matched {
                    continue;
                }
                matches.push(SourceSearchMatch {
                    relative_path: relative_path.clone(),
                    line_number: index as u32 + 1,
                    line_text: truncate_chars(line, 240),
                });
                if matches.len() >= limit {
                    truncation_reasons.push(format!("命中超过本次上限 {limit} 条"));
                    break;
                }
            }
            if matches.len() >= limit {
                break;
            }
        }

        Ok(SourceSearchResult {
            matches,
            scanned_files,
            truncated: !truncation_reasons.is_empty(),
            truncation_reasons,
        })
    }

    pub fn get_file_tree(
        &self,
        relative_directory: Option<&str>,
        depth: usize,
        limit: usize,
    ) -> UnderstandingResult<FileTreeResult> {
        let prefix = relative_directory
            .filter(|value| !value.trim().is_empty())
            .map(normalize_relative_directory_path)
            .transpose()?
            .unwrap_or_default();
        if !prefix.is_empty()
            && !self
                .allowed_files
                .keys()
                .any(|path| is_under_prefix(path, &prefix))
        {
            return Err(understanding_error(
                ErrorCode::SourceMissing,
                format!("目录不存在或不在允许范围内：{prefix}"),
            ));
        }
        let depth = depth.clamp(1, SOURCE_TREE_MAX_DEPTH);
        let limit = limit.clamp(1, SOURCE_TREE_MAX_ENTRIES);
        let mut entries = BTreeMap::new();

        for relative_path in self.allowed_files.keys() {
            if !prefix.is_empty() && !is_under_prefix(relative_path, &prefix) {
                continue;
            }
            let remainder = if prefix.is_empty() {
                relative_path.as_str()
            } else {
                relative_path
                    .strip_prefix(&prefix)
                    .and_then(|value| value.strip_prefix('/'))
                    .unwrap_or_default()
            };
            let parts: Vec<&str> = remainder.split('/').collect();
            for index in 0..parts.len().min(depth) {
                let child = parts[..=index].join("/");
                let full = if prefix.is_empty() {
                    child
                } else {
                    format!("{prefix}/{child}")
                };
                let kind = if index + 1 == parts.len() {
                    FileTreeEntryKind::File
                } else {
                    FileTreeEntryKind::Directory
                };
                entries.entry(full).or_insert(kind);
            }
        }

        let total = entries.len();
        let entries = entries
            .into_iter()
            .take(limit)
            .map(|(relative_path, kind)| FileTreeEntry {
                relative_path,
                kind,
            })
            .collect();
        let truncated = total > limit;
        Ok(FileTreeResult {
            entries,
            truncated,
            truncation_reason: truncated.then(|| format!("目录条目超过本次上限 {limit} 条")),
        })
    }

    fn read_allowed_bytes(&self, relative_path: &str) -> UnderstandingResult<Vec<u8>> {
        let Some(allowed) = self.allowed_files.get(relative_path) else {
            let candidate = self.canonical_root.join(relative_path);
            let code = if candidate.exists() {
                ErrorCode::ExcludedPath
            } else {
                ErrorCode::SourceMissing
            };
            return Err(understanding_error(
                code,
                format!("文件不存在或被项目规则排除：{relative_path}"),
            ));
        };
        if allowed.size > SOURCE_FILE_MAX_BYTES {
            return Err(understanding_error(
                ErrorCode::BudgetExceeded,
                format!("文件超过 1 MiB 读取上限：{relative_path}"),
            ));
        }
        let canonical = std::fs::canonicalize(&allowed.absolute_path).map_err(|error| {
            understanding_error(
                ErrorCode::SourceMissing,
                format!("文件不存在或无法读取：{relative_path}（{error}）"),
            )
        })?;
        if !canonical.starts_with(&self.canonical_root) {
            return Err(understanding_error(
                ErrorCode::OutsideProject,
                format!("文件真实路径超出项目范围：{relative_path}"),
            ));
        }
        let metadata = std::fs::metadata(&canonical).map_err(|error| {
            understanding_error(
                ErrorCode::SourceMissing,
                format!("无法读取文件元数据：{relative_path}（{error}）"),
            )
        })?;
        if metadata.len() > SOURCE_FILE_MAX_BYTES {
            return Err(understanding_error(
                ErrorCode::BudgetExceeded,
                format!("文件超过 1 MiB 读取上限：{relative_path}"),
            ));
        }
        let bytes = std::fs::read(&canonical).map_err(|error| {
            understanding_error(
                ErrorCode::SourceMissing,
                format!("读取文件失败：{relative_path}（{error}）"),
            )
        })?;
        if bytes.iter().take(8 * 1024).any(|byte| *byte == 0) {
            return Err(understanding_error(
                ErrorCode::EncodingUnsupported,
                format!("拒绝读取二进制文件：{relative_path}"),
            ));
        }
        Ok(bytes)
    }
}

fn understanding_error(code: ErrorCode, message: impl Into<String>) -> UnderstandingError {
    UnderstandingError::new(code, message)
}

fn normalize_relative_file_path(path: &str) -> UnderstandingResult<String> {
    let normalized = normalize_relative_path(path, false)?;
    if normalized.is_empty() {
        return Err(understanding_error(
            ErrorCode::OutsideProject,
            "文件路径不能为空",
        ));
    }
    Ok(normalized)
}

fn normalize_relative_directory_path(path: &str) -> UnderstandingResult<String> {
    normalize_relative_path(path.trim_end_matches('/'), true)
}

fn normalize_relative_path(path: &str, allow_empty: bool) -> UnderstandingResult<String> {
    if path.contains(['\\', ':', '\0']) || Path::new(path).is_absolute() {
        return Err(understanding_error(
            ErrorCode::OutsideProject,
            format!("路径不是规范的项目内相对路径：{path}"),
        ));
    }
    if path.is_empty() && allow_empty {
        return Ok(String::new());
    }
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(understanding_error(
            ErrorCode::OutsideProject,
            format!("路径不是规范的项目内相对路径：{path}"),
        ));
    }
    Ok(path.to_string())
}

fn is_under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn line_spans(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            spans.push((start, index));
            start = index + 1;
        }
    }
    if start < bytes.len() || spans.is_empty() {
        spans.push((start, bytes.len()));
    }
    spans
}

fn take_chars(value: &str, max_chars: usize) -> (&str, bool) {
    if value.chars().count() <= max_chars {
        return (value, true);
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    (&value[..end], false)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let (prefix, complete) = take_chars(value, max_chars);
    if complete {
        prefix.to_string()
    } else {
        format!("{prefix}…")
    }
}

/// 供诊断输出使用：与发放时同一算法。
pub(crate) fn source_id_of(source_ref: &SourceRef) -> String {
    source_id(source_ref)
}

fn source_id(source_ref: &SourceRef) -> String {
    let wire = serde_json::to_vec(source_ref).expect("SourceRef 必须可序列化");
    format!("sr1:{}", sha256_hex(&wire))
}
