//! 未打开仓库的只读分支目录和延迟导航。扫描期间不创建 tab，不检出分支。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::Context;
use khaslana::{BranchInfo, BranchKind, GitService, RepositorySnapshot};

use super::catalog::{SearchEntry, SearchTarget};
use crate::tasks::{TaskKind, panic_message};
use crate::{RepositoryView, UiEvent, normalize_repo_path, send_ui_event};

pub(super) struct SavedRepositorySearch {
    pub path: PathBuf,
    pub branches: Option<Result<Vec<BranchInfo>, String>>,
}

pub(super) struct RepositorySearchCatalog {
    pub request_id: u64,
    pub repositories: Vec<SavedRepositorySearch>,
    pub cancelled: Arc<AtomicBool>,
}

impl RepositorySearchCatalog {
    pub fn new(request_id: u64, paths: Vec<PathBuf>) -> Self {
        Self {
            request_id,
            repositories: paths
                .into_iter()
                .map(|path| SavedRepositorySearch {
                    path,
                    branches: None,
                })
                .collect(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn apply(
        &mut self,
        request_id: u64,
        path: &Path,
        result: Result<Vec<BranchInfo>, String>,
    ) -> bool {
        if request_id != self.request_id || self.cancelled.load(Ordering::Relaxed) {
            return false;
        }
        let Some(repo) = self.repositories.iter_mut().find(|repo| repo.path == path) else {
            return false;
        };
        repo.branches = Some(result);
        true
    }

    pub fn progress_label(&self, count: usize) -> String {
        let pending = self
            .repositories
            .iter()
            .filter(|repo| repo.branches.is_none())
            .count();
        let failed = self
            .repositories
            .iter()
            .filter(|repo| matches!(&repo.branches, Some(Err(_))))
            .count();
        if pending > 0 {
            format!("{count} 个结果 · 正在读取 {pending} 个仓库")
        } else if failed > 0 {
            format!("{count} 个结果 · {failed} 个仓库不可用")
        } else {
            format!("{count} 个结果")
        }
    }
}

impl Drop for RepositorySearchCatalog {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// 与仓库切换器采用同一身份规则；已打开和重复记录均不重复扫描。
pub(super) fn unopened_repository_paths(
    open: impl IntoIterator<Item = PathBuf>,
    recent: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut seen: HashSet<_> = open
        .into_iter()
        .map(|path| normalize_repo_path(&path))
        .collect();
    recent
        .into_iter()
        .filter(|path| seen.insert(normalize_repo_path(path)))
        .collect()
}

pub(super) fn saved_repository_entries(repo: &SavedRepositorySearch) -> Vec<SearchEntry> {
    let name = repo
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.path.to_string_lossy().into_owned());
    let mut repository = SearchEntry::new(
        name.clone(),
        repo.path.to_string_lossy().into_owned(),
        "仓库",
        "项目 repository repo",
        SearchTarget::SavedRepository(repo.path.clone()),
    );
    if let Some(Err(error)) = &repo.branches {
        repository.disabled_reason = Some("仓库目录不可用，无法读取分支");
        repository.subtitle = format!("{} · {error}", repo.path.display());
    }
    let mut entries = vec![repository];
    if let Some(Ok(branches)) = &repo.branches {
        for branch in branches {
            let group = if branch.kind == BranchKind::Local {
                "本地分支"
            } else {
                "远端分支"
            };
            entries.push(SearchEntry::new(
                &branch.name,
                format!("{name} · {} · 打开仓库并定位分支", repo.path.display()),
                group,
                "branch 分支",
                SearchTarget::SavedBranch(
                    repo.path.clone(),
                    branch.name.clone(),
                    branch.kind.clone(),
                ),
            ));
        }
    }
    entries
}

pub(super) fn read_repository_branches(
    service: &GitService,
    path: &Path,
) -> Result<Vec<BranchInfo>, String> {
    let repo = git2::Repository::open(path).map_err(|error| error.to_string())?;
    service.branches(&repo).map_err(|error| error.to_string())
}

#[derive(Clone, Debug)]
pub(crate) struct PendingSearchBranch {
    pub load_id: u64,
    pub name: String,
    pub kind: BranchKind,
}

impl PendingSearchBranch {
    pub(super) fn take_matching(
        pending: &mut Option<Self>,
        event_load_id: u64,
        current_load_id: u64,
        is_active: bool,
    ) -> Option<Self> {
        if pending
            .as_ref()
            .is_none_or(|pending| pending.load_id != event_load_id)
        {
            return None;
        }
        let pending = pending.take();
        if current_load_id == event_load_id && is_active {
            pending
        } else {
            None
        }
    }

    pub(super) fn matches_branch(&self, snapshot: &RepositorySnapshot) -> bool {
        snapshot
            .branches
            .iter()
            .any(|branch| branch.name == self.name && branch.kind == self.kind)
    }
}

impl RepositoryView {
    pub(super) fn start_saved_repository_search(&mut self) -> RepositorySearchCatalog {
        // 与切换器一致的短 SQLite 查询只取路径；Git/文件系统重任务交给任务池。
        self.repo_switcher_recent = self
            .storage
            .load_recent_repos()
            .unwrap_or_else(|_| self.repo_switcher_recent.clone());
        let paths = unopened_repository_paths(
            self.tabs.iter().filter_map(|tab| tab.repo_path.clone()),
            self.repo_switcher_recent
                .iter()
                .map(|(path, _)| path.clone()),
        );
        self.code_search_request_seq = self.code_search_request_seq.wrapping_add(1);
        let catalog = RepositorySearchCatalog::new(self.code_search_request_seq, paths.clone());
        let request_id = catalog.request_id;
        let cancelled = catalog.cancelled.clone();
        let service = self.service_for_tab(self.active_tab_id().unwrap_or(crate::RepoTabId(0)));
        let tx = self.tx.clone();
        if !paths.is_empty() {
            // 至多一个扫描任务串行读取，避免所有最近仓库占满短任务池。
            self.tasks.spawn(TaskKind::Long, move || {
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        read_repository_branches(&service, &path)
                    }))
                    .unwrap_or_else(|panic| Err(format!("读取分支失败：{}", panic_message(panic))));
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    send_ui_event(
                        &tx,
                        UiEvent::AppSearchRepositoryLoaded {
                            request_id,
                            path,
                            result,
                        },
                    );
                }
            });
        }
        catalog
    }

    pub(crate) fn handle_app_search_repository_loaded(
        &mut self,
        request_id: u64,
        path: PathBuf,
        result: Result<Vec<BranchInfo>, String>,
    ) {
        if let Some(palette) = self.code_search_palette.as_mut() {
            if palette.scope.scans_saved_repositories(&self.code_palette_search.value)
                && palette.repositories.as_mut().is_some_and(|repositories| {
                    repositories.apply(request_id, &path, result)
                })
            {
                // 新结果可能改变排序；旧列表回调须等新模型挂载后才能再次确认。
                palette.catalog_changed = true;
            }
        }
    }

    pub(super) fn open_saved_search_branch(
        &mut self,
        path: PathBuf,
        name: String,
        kind: BranchKind,
    ) {
        self.open_repo(path.clone());
        if self.last_error.is_some() {
            return;
        }
        if let Some(tab) = self.repo_path.as_ref() {
            if normalize_repo_path(tab) != normalize_repo_path(&path) {
                return;
            }
        } else {
            return;
        }
        self.pending_search_branch = Some(PendingSearchBranch {
            load_id: self.repository_load_id,
            name,
            kind,
        });
    }

    pub(crate) fn finish_pending_search_branch(
        &mut self,
        tab_id: crate::RepoTabId,
        load_id: u64,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_tab_id() == Some(tab_id)
            && self.top_overlay_kind() == crate::TopOverlayKind::None;
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        let Some(pending) = PendingSearchBranch::take_matching(
            &mut tab.pending_search_branch,
            load_id,
            tab.repository_load_id,
            is_active,
        ) else {
            return;
        };
        if !tab
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| pending.matches_branch(snapshot))
        {
            self.notify_warning(format!("分支已不存在：{}", pending.name), cx);
            return;
        }
        self.locate_search_branch(pending.name);
    }

    pub(super) fn locate_search_branch(&mut self, name: String) {
        self.set_history_file_filter(None);
        self.set_history_scope(khaslana::HistoryScope::AllRefs);
        self.commit_graph_search.clear();
        self.commit_graph.highlight_ahead_only = false;
        self.open_commit_graph();
        self.set_commit_graph_highlight(Some(name));
    }
}

#[cfg(test)]
#[path = "../tests/code_palette_repositories.rs"]
mod tests;
