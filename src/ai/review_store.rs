// AI 评审记录本地落盘。
//
// 每次评审完成后由后台任务线程写一份 JSON 文件到
// `<数据目录>/ai-reviews/<repo哈希8>/<毫秒时间戳>.json`，按仓库只保留
// 最近 MAX_STORED_RECORDS 条；历史弹窗经 `list_review_records` 读回。
// 选 JSON 文件而非 SQLite：记录自带完整轨迹（steps 可达数百 KB），文件
// 天然按记录隔离、便于备份清理，也避免把大文本塞进主库。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ai::review::AiReviewResult;
use crate::types::{GitError, Result};

/// 每个仓库保留的记录上限，超出时删除最旧的。
pub const MAX_STORED_RECORDS: usize = 30;

/// 一份持久化的评审记录（写入时快照，含完整轨迹）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiReviewRecord {
    /// 记录 id（文件名主干，毫秒时间戳）。
    pub id: String,
    /// 仓库绝对路径。
    pub repo_path: String,
    pub target_display_name: String,
    pub target_commit_oid: String,
    /// 生成时使用的模型名（历史列表展示）。
    pub model: String,
    /// 完成时刻（Unix 毫秒）。
    pub created_at_millis: u64,
    /// 生成耗时（秒）。
    pub duration_secs: u64,
    /// 变更文件数。
    pub file_count: usize,
    pub result: AiReviewResult,
}

/// 仓库路径 → 稳定短哈希（FNV-1a 32 位 hex）。手写而非 `DefaultHasher`：
/// std 哈希不保证跨版本稳定，目录名漂移会让旧记录「失联」。
/// 哈希前先做小写折叠：Windows 路径大小写不敏感，`D:\Repo` 与 `d:\repo`
/// 是同一仓库，不折叠会因盘符/目录大小写变化生成新目录导致旧记录「失联」。
/// 仓库路径 → 稳定短哈希键（FNV-1a + 小写折叠）。跨 crate 共享：
/// code_index 的索引库目录与 ai-reviews 同一套仓库标识约定。
pub fn repo_key(repo_path: &str) -> String {
    let normalized = repo_path.trim_end_matches(['/', '\\']).to_lowercase();
    fnv1a_32(normalized.as_bytes())
}

/// 大小写折叠规则上线前的老键：仅用于兼容读取旧记录目录，不再写入。
fn legacy_repo_key(repo_path: &str) -> String {
    let normalized = repo_path.trim_end_matches(['/', '\\']);
    fnv1a_32(normalized.as_bytes())
}

fn fnv1a_32(bytes: &[u8]) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// 某仓库的记录目录。
fn repo_record_dir(base_dir: &Path, repo_path: &str) -> PathBuf {
    base_dir.join("ai-reviews").join(repo_key(repo_path))
}

/// 保存一条评审记录，返回记录 id。文件名 = 完成时刻毫秒（同毫秒冲突加
/// `-2`/`-3` 后缀），写入后清理只保留最近 `MAX_STORED_RECORDS` 条。
pub fn save_review_record(base_dir: &Path, mut record: AiReviewRecord) -> Result<String> {
    let dir = repo_record_dir(base_dir, &record.repo_path);
    fs::create_dir_all(&dir)
        .map_err(|err| GitError::Message(format!("创建评审记录目录失败：{err}")))?;

    let id = unique_record_id(&dir, record.created_at_millis);
    record.id = id.clone();
    let json = serde_json::to_string(&record)
        .map_err(|err| GitError::Message(format!("序列化评审记录失败：{err}")))?;
    let path = dir.join(format!("{id}.json"));
    fs::write(&path, json).map_err(|err| GitError::Message(format!("写入评审记录失败：{err}")))?;

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

/// 只保留最近 `MAX_STORED_RECORDS` 条。文件名以毫秒时间戳为前缀，字典序
/// ≈ 时间序（`-n` 后缀只出现在同毫秒内，排序仍正确）。
fn prune_records(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if names.len() <= MAX_STORED_RECORDS {
        return;
    }
    names.sort();
    let excess = names.len() - MAX_STORED_RECORDS;
    for name in names.into_iter().take(excess) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// 列出某仓库最近的评审记录（按完成时间倒序，最多 `limit` 条）。
/// 单个文件损坏跳过不报错（仅记 warn），不让一条坏记录拖垮整个列表。
///
/// 同时读新键目录与旧键目录（大小写折叠前的键）：折叠规则上线前的旧
/// 记录仍可见（写入只走新键，旧目录不再增长）。文件名以毫秒时间戳为
/// 前缀、字典序 ≈ 时间序，先按文件名倒序取最新若干条再解析，不必把
/// 目录里全部记录（单条可达数百 KB）读进内存。
pub fn list_review_records(
    base_dir: &Path,
    repo_path: &str,
    limit: usize,
) -> Result<Vec<AiReviewRecord>> {
    let mut dirs = vec![repo_record_dir(base_dir, repo_path)];
    let legacy = base_dir.join("ai-reviews").join(legacy_repo_key(repo_path));
    if legacy != dirs[0] {
        dirs.push(legacy);
    }

    let mut records = Vec::new();
    for dir in &dirs {
        if records.len() >= limit {
            break;
        }
        collect_latest_records(dir, limit - records.len(), &mut records);
    }
    // 两个目录合并后时间序可能交错，统一排序再截断。
    records.sort_by(|a, b| {
        b.created_at_millis
            .cmp(&a.created_at_millis)
            .then_with(|| b.id.cmp(&a.id))
    });
    records.truncate(limit);
    Ok(records)
}

/// 解析单个目录里最新的记录，直到 `want` 条或文件读完；坏文件跳过继续。
fn collect_latest_records(dir: &Path, want: usize, records: &mut Vec<AiReviewRecord>) {
    if want == 0 || !dir.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names.reverse();
    for name in names {
        if records.len() >= want {
            break;
        }
        let path = dir.join(name);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<AiReviewRecord>(&text) {
            Ok(record) => records.push(record),
            Err(err) => tracing::warn!(
                target: "khaslana::ai",
                "跳过损坏的评审记录 {}: {err}",
                path.display()
            ),
        }
    }
}

#[cfg(test)]
#[path = "../tests/ai/review_store.rs"]
mod tests;
