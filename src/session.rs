// 会话与持久化包装器模块：打开本地配置数据库、加载/保存会话与各项偏好。
//
// 从 main.rs 抽出的纯持久化 I/O 包装器，集中 RepositoryView 对 AppStorage 的
// 访问。无渲染耦合，方法仍挂在 impl RepositoryView 上跨文件调用。
// 纯位置搬移，未改变任何方法体逻辑。

use std::collections::BTreeSet;
use std::sync::Arc;

use git2::Repository;

use khaslana::{
    AiProviderSettings, DiffEncodingPreferences, NetworkProxySettings, RemoteCredentialBindings,
    SessionState,
};

use crate::{LoadPriority, RepositoryView, dedupe_repo_paths, normalize_repo_path};

impl RepositoryView {
    pub(crate) fn open_storage() -> (Arc<khaslana::AppStorage>, Option<String>, Option<String>) {
        match khaslana::AppStorage::open_default() {
            Ok(storage) => (Arc::new(storage), None, None),
            Err(first_err) => {
                tracing::warn!("local config database open failed, recreating: {first_err}");
                match khaslana::AppStorage::recreate_default_after_failure() {
                    Ok(storage) => (
                        Arc::new(storage),
                        Some("本地配置数据库已重建".to_string()),
                        Some(format!("原数据库打开失败，已创建空数据库：{first_err}")),
                    ),
                    Err(second_err) => {
                        tracing::warn!(
                            "local config database recreate failed, using memory database: {second_err}"
                        );
                        let storage =
                            khaslana::AppStorage::open_in_memory().unwrap_or_else(|err| {
                                panic!("无法创建临时配置数据库：{err}");
                            });
                        (
                            Arc::new(storage),
                            Some("正在使用临时配置数据库".to_string()),
                            Some(format!("本地配置数据库不可用：{second_err}")),
                        )
                    }
                }
            }
        }
    }

    pub(crate) fn load_session_state(&self) -> Option<SessionState> {
        match self.storage.load_session_state() {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!("session load skipped: {err}");
                None
            }
        }
    }

    pub(crate) fn load_diff_encoding_preferences(
        storage: &khaslana::AppStorage,
    ) -> DiffEncodingPreferences {
        storage
            .load_diff_encoding_preferences()
            .inspect_err(|err| tracing::warn!("diff encoding preferences load skipped: {err}"))
            .unwrap_or_default()
    }

    pub(crate) fn load_remote_credential_bindings(
        storage: &khaslana::AppStorage,
    ) -> RemoteCredentialBindings {
        storage
            .load_remote_credential_bindings()
            .inspect_err(|err| tracing::warn!("remote credential bindings load skipped: {err}"))
            .unwrap_or_default()
    }

    pub(crate) fn load_proxy_settings(storage: &khaslana::AppStorage) -> NetworkProxySettings {
        storage
            .load_proxy_settings()
            .inspect_err(|err| tracing::warn!("network proxy settings load skipped: {err}"))
            .unwrap_or_default()
    }

    pub(crate) fn load_ai_provider_settings(storage: &khaslana::AppStorage) -> AiProviderSettings {
        storage
            .load_ai_provider_settings()
            .inspect_err(|err| tracing::warn!("ai provider settings load skipped: {err}"))
            .unwrap_or_default()
    }

    pub(crate) fn save_diff_encoding_preferences(&self) {
        if let Err(err) = self
            .storage
            .save_diff_encoding_preferences(&self.diff_encoding_preferences)
        {
            tracing::warn!("diff encoding preferences write skipped: {err}");
        }
    }

    pub(crate) fn save_remote_credential_bindings(&self) {
        let Ok(bindings) = self.remote_credential_bindings.lock() else {
            tracing::warn!("remote credential bindings state read skipped");
            return;
        };
        if let Err(err) = self.storage.save_remote_credential_bindings(&bindings) {
            tracing::warn!("remote credential bindings write skipped: {err}");
        }
    }

    pub(crate) fn save_ai_provider_settings(&self) {
        if let Err(err) = self.storage.save_ai_provider_settings(&self.ai_settings) {
            tracing::warn!("ai provider settings write skipped: {err}");
        }
    }

    pub(crate) fn save_proxy_settings(&self) {
        if let Err(err) = self.storage.save_proxy_settings(&self.proxy_settings) {
            tracing::warn!("network proxy settings write skipped: {err}");
        }
    }

    pub(crate) fn save_session(&self) {
        if self.restoring_session {
            return;
        }
        let repo_paths = dedupe_repo_paths(
            self.tabs
                .iter()
                .filter_map(|tab| tab.repo_path.clone())
                .collect::<Vec<_>>(),
        );
        let active_repo_path = self
            .active_tab()
            .and_then(|tab| tab.repo_path.as_ref())
            .cloned();
        let state = SessionState {
            repo_paths,
            active_repo_path,
        };
        if let Err(err) = self.storage.save_session_state(&state) {
            tracing::warn!("session write skipped: {err}");
        }
    }

    pub(crate) fn restore_session(&mut self) {
        let Some(session) = self.load_session_state() else {
            return;
        };
        self.restoring_session = true;
        let mut restored = Vec::new();
        let mut failed = 0usize;
        let mut seen = BTreeSet::new();

        for path in session.repo_paths {
            let key = normalize_repo_path(&path);
            if !seen.insert(key) {
                continue;
            }
            if !path.exists() || Repository::open(&path).is_err() {
                failed += 1;
                continue;
            }
            restored.push(path);
        }

        if restored.is_empty() {
            if failed > 0 {
                self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
                self.fallback_tab.status = "会话恢复失败".to_string();
            }
            self.restoring_session = false;
            self.save_session();
            return;
        }

        let active_key = session
            .active_repo_path
            .as_ref()
            .map(|path| normalize_repo_path(path));
        let mut active = None;
        for path in restored {
            let id = self.ensure_tab_for_path(path.clone());
            if active_key.as_deref() == Some(normalize_repo_path(&path).as_str()) {
                active = Some(id);
            }
        }
        if let Some(active) = active.or(self.active_tab) {
            self.active_tab = Some(active);
        }
        if failed > 0 {
            self.fallback_tab.last_error = Some(format!("{failed} 个上次打开的仓库无法恢复"));
        }

        let tabs = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        for tab_id in tabs {
            if let Some(path) = self.tab(tab_id).and_then(|tab| tab.repo_path.clone()) {
                self.queue_repository_load(
                    tab_id,
                    path,
                    "正在恢复仓库",
                    "仓库已恢复",
                    LoadPriority::Background,
                );
            }
        }
        self.restoring_session = false;
        self.save_session();
    }
}
