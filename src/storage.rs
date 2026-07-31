use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::ai::AiProviderSettings;
use crate::credentials::{
    CredentialRecord, CredentialScope, RemoteCredentialPolicy, StoredCredentialKind,
};
use crate::external_merge::ExternalMergeSettings;
use crate::proxy::{CustomProxySettings, NetworkProxyMode, NetworkProxySettings};
use crate::types::{DiffEncodingChoice, GitError, Result};

const DB_FILE_NAME: &str = "khaslana.sqlite3";
const SCHEMA_VERSION: i64 = 5;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RemoteCredentialBindings {
    #[serde(default)]
    pub remotes: Vec<RemoteCredentialBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteCredentialBinding {
    pub repo_path: String,
    pub remote_name: String,
    pub remote_url: String,
    pub policy: RemoteCredentialPolicy,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiffEncodingPreferences {
    pub repositories: BTreeMap<String, DiffEncodingChoice>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SessionState {
    pub repo_paths: Vec<PathBuf>,
    pub active_repo_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct LegacyStoragePaths {
    pub session: PathBuf,
    pub diff_encodings: PathBuf,
    pub remote_credentials: PathBuf,
    pub network_proxy: PathBuf,
    pub credentials: PathBuf,
}

/// 更新偏好：自动检查开关和已跳过版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePreferences {
    /// 是否自动检查更新（默认 true）。
    pub auto_check: bool,
    /// 已跳过的版本号；新版本高于此值时重新提示。
    pub skipped_version: Option<String>,
}

impl Default for UpdatePreferences {
    fn default() -> Self {
        Self {
            auto_check: true,
            skipped_version: None,
        }
    }
}

/// 应用主题偏好。System 会跟随操作系统窗口外观变化。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default)]
pub struct LegacyImportSummary {
    pub session: bool,
    pub diff_encodings: bool,
    pub remote_credentials: bool,
    pub network_proxy: bool,
    pub credentials: bool,
}

pub struct AppStorage {
    conn: Mutex<Connection>,
}

impl AppStorage {
    pub fn open_default() -> Result<Self> {
        let path = default_database_path()
            .ok_or_else(|| GitError::Message("无法定位本地配置数据库目录".to_string()))?;
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(storage_error)?;
        initialize_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(storage_error)?;
        initialize_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn recreate_default_after_failure() -> Result<Self> {
        let path = default_database_path()
            .ok_or_else(|| GitError::Message("无法定位本地配置数据库目录".to_string()))?;
        recreate_database_file(&path)?;
        Self::open(path)
    }

    pub fn load_session_state(&self) -> Result<Option<SessionState>> {
        let conn = self.lock_conn()?;
        load_session_state_from_conn(&conn)
    }

    pub fn save_session_state(&self, state: &SessionState) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_session_state_tx(&tx, state)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_diff_encoding_preferences(&self) -> Result<DiffEncodingPreferences> {
        let conn = self.lock_conn()?;
        load_diff_encoding_preferences_from_conn(&conn)
    }

    pub fn save_diff_encoding_preferences(
        &self,
        preferences: &DiffEncodingPreferences,
    ) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_diff_encoding_preferences_tx(&tx, preferences)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_remote_credential_bindings(&self) -> Result<RemoteCredentialBindings> {
        let conn = self.lock_conn()?;
        load_remote_credential_bindings_from_conn(&conn)
    }

    pub fn save_remote_credential_bindings(
        &self,
        bindings: &RemoteCredentialBindings,
    ) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_remote_credential_bindings_tx(&tx, bindings)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_proxy_settings(&self) -> Result<NetworkProxySettings> {
        let conn = self.lock_conn()?;
        load_proxy_settings_from_conn(&conn)
    }

    pub fn save_proxy_settings(&self, settings: &NetworkProxySettings) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_proxy_settings_tx(&tx, settings)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_ai_provider_settings(&self) -> Result<AiProviderSettings> {
        let conn = self.lock_conn()?;
        load_ai_provider_settings_from_conn(&conn)
    }

    pub fn save_ai_provider_settings(&self, settings: &AiProviderSettings) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_ai_provider_settings_tx(&tx, settings)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_external_merge_settings(&self) -> Result<ExternalMergeSettings> {
        let conn = self.lock_conn()?;
        load_external_merge_settings_from_conn(&conn)
    }

    pub fn save_external_merge_settings(&self, settings: &ExternalMergeSettings) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_external_merge_settings_tx(&tx, settings)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_credential_records(&self) -> Result<Vec<CredentialRecord>> {
        let conn = self.lock_conn()?;
        load_credential_records_from_conn(&conn)
    }

    pub fn save_credential_records(&self, records: &[CredentialRecord]) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_credential_records_tx(&tx, records)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_update_preferences(&self) -> Result<UpdatePreferences> {
        let conn = self.lock_conn()?;
        load_update_preferences_from_conn(&conn)
    }

    pub fn save_update_preferences(&self, preferences: &UpdatePreferences) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_update_preferences_tx(&tx, preferences)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_theme_mode(&self) -> Result<ThemeMode> {
        let conn = self.lock_conn()?;
        load_theme_mode_from_conn(&conn)
    }

    pub fn save_theme_mode(&self, mode: ThemeMode) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_theme_mode_tx(&tx, mode)?;
        tx.commit().map_err(storage_error)
    }

    pub fn load_theme_accent(&self) -> Result<usize> {
        let conn = self.lock_conn()?;
        load_theme_accent_from_conn(&conn)
    }

    pub fn save_theme_accent(&self, accent: usize) -> Result<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        save_theme_accent_tx(&tx, accent)?;
        tx.commit().map_err(storage_error)
    }

    pub fn import_legacy_json(
        &self,
        paths: &LegacyStoragePaths,
        force: bool,
    ) -> Result<LegacyImportSummary> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        let summary = import_legacy_json_tx(&tx, paths, force)?;
        tx.commit().map_err(storage_error)?;
        Ok(summary)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| GitError::Message("本地配置数据库状态异常".to_string()))
    }
}

pub fn default_database_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "Khaslana").map(|dirs| dirs.config_dir().join(DB_FILE_NAME))
}

pub fn default_legacy_storage_paths() -> Option<LegacyStoragePaths> {
    ProjectDirs::from("", "", "Khaslana").map(|dirs| legacy_storage_paths(dirs.config_dir()))
}

pub fn legacy_storage_paths(config_dir: &Path) -> LegacyStoragePaths {
    LegacyStoragePaths {
        session: config_dir.join("session.json"),
        diff_encodings: config_dir.join("diff-encodings.json"),
        remote_credentials: config_dir.join("remote-credentials.json"),
        network_proxy: config_dir.join("network-proxy.json"),
        credentials: config_dir.join("credentials.json"),
    }
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS schema_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS session_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            active_repo_path TEXT,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS session_repositories (
            position INTEGER NOT NULL,
            path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS diff_encoding_preferences (
            repo_path TEXT PRIMARY KEY,
            choice TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS network_proxy_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL,
            http_proxy TEXT NOT NULL,
            https_proxy TEXT NOT NULL,
            socks5_proxy TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS remote_credential_bindings (
            repo_path TEXT NOT NULL,
            remote_name TEXT NOT NULL,
            remote_url TEXT NOT NULL,
            policy_kind TEXT NOT NULL,
            credential_record_id TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (repo_path, remote_name)
        );

        CREATE TABLE IF NOT EXISTS credential_records (
            id TEXT PRIMARY KEY,
            display_name TEXT,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            host TEXT NOT NULL,
            remote_url TEXT NOT NULL,
            username TEXT NOT NULL,
            key_path TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_used INTEGER
        );

        CREATE TABLE IF NOT EXISTS ai_provider_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL,
            payload TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS update_preferences (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            auto_check INTEGER NOT NULL,
            skipped_version TEXT,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS external_merge_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL,
            payload TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS theme_preferences (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL,
            accent INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .map_err(storage_error)?;
    // 幂等迁移：旧版数据库的 theme_preferences 没有 accent 列，补上。
    // CREATE TABLE IF NOT EXISTS 对已存在的表不会加列，需显式 ALTER。
    ensure_theme_accent_column(&conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )
    .map_err(storage_error)?;
    Ok(())
}

/// 检查 theme_preferences 是否有 accent 列，没有则补加（幂等）。
fn ensure_theme_accent_column(conn: &Connection) -> Result<()> {
    let has_accent = conn
        .prepare("PRAGMA table_info(theme_preferences)")
        .map_err(storage_error)?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage_error)?
        .filter_map(|col| col.ok())
        .any(|col| col == "accent");
    if !has_accent {
        conn.execute(
            "ALTER TABLE theme_preferences ADD COLUMN accent INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(storage_error)?;
    }
    Ok(())
}

fn import_legacy_json_tx(
    tx: &Transaction<'_>,
    paths: &LegacyStoragePaths,
    force: bool,
) -> Result<LegacyImportSummary> {
    // 旧 JSON 只由迁移工具读取；主程序启动后只认 SQLite 当前态。
    let mut summary = LegacyImportSummary::default();
    if force || table_is_empty(tx, "session_repositories")? {
        if let Some(state) = read_json::<SessionState>(&paths.session)? {
            save_session_state_tx(tx, &state)?;
            summary.session = true;
        }
    }
    if force || table_is_empty(tx, "diff_encoding_preferences")? {
        if let Some(preferences) = read_json::<DiffEncodingPreferences>(&paths.diff_encodings)? {
            save_diff_encoding_preferences_tx(tx, &preferences)?;
            summary.diff_encodings = true;
        }
    }
    if force || table_is_empty(tx, "remote_credential_bindings")? {
        if let Some(bindings) = read_json::<RemoteCredentialBindings>(&paths.remote_credentials)? {
            save_remote_credential_bindings_tx(tx, &bindings)?;
            summary.remote_credentials = true;
        }
    }
    if force || table_is_empty(tx, "network_proxy_settings")? {
        if let Some(settings) = read_json::<NetworkProxySettings>(&paths.network_proxy)? {
            save_proxy_settings_tx(tx, &settings)?;
            summary.network_proxy = true;
        }
    }
    if force || table_is_empty(tx, "credential_records")? {
        if let Some(index) = read_json::<CredentialIndex>(&paths.credentials)? {
            save_credential_records_tx(tx, &index.records)?;
            summary.credentials = true;
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('legacy_json_imported_at', ?1)",
        params![now_seconds().to_string()],
    )
    .map_err(storage_error)?;
    Ok(summary)
}

fn read_json<T>(path: &Path) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    serde_json::from_str(&content).map(Some).map_err(|err| {
        GitError::Message(format!("旧配置文件解析失败（{}）：{err}", path.display()))
    })
}

fn table_is_empty(conn: &Connection, table: &str) -> Result<bool> {
    let sql = format!("SELECT NOT EXISTS(SELECT 1 FROM {table} LIMIT 1)");
    conn.query_row(&sql, [], |row| row.get::<_, bool>(0))
        .map_err(storage_error)
}

fn load_session_state_from_conn(conn: &Connection) -> Result<Option<SessionState>> {
    let active_repo_path = conn
        .query_row(
            "SELECT active_repo_path FROM session_state WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_error)?
        .flatten();
    let mut stmt = conn
        .prepare("SELECT path FROM session_repositories ORDER BY position ASC")
        .map_err(storage_error)?;
    let repo_paths = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if active_repo_path.is_none() && repo_paths.is_empty() {
        return Ok(None);
    }
    Ok(Some(SessionState {
        repo_paths,
        active_repo_path: active_repo_path.map(PathBuf::from),
    }))
}

fn save_session_state_tx(tx: &Transaction<'_>, state: &SessionState) -> Result<()> {
    tx.execute("DELETE FROM session_repositories", [])
        .map_err(storage_error)?;
    for (position, path) in state.repo_paths.iter().enumerate() {
        tx.execute(
            "INSERT INTO session_repositories (position, path) VALUES (?1, ?2)",
            params![position as i64, path.to_string_lossy()],
        )
        .map_err(storage_error)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO session_state (id, active_repo_path, updated_at) VALUES (1, ?1, ?2)",
        params![
            state
                .active_repo_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            now_seconds()
        ],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_diff_encoding_preferences_from_conn(conn: &Connection) -> Result<DiffEncodingPreferences> {
    let mut stmt = conn
        .prepare("SELECT repo_path, choice FROM diff_encoding_preferences ORDER BY repo_path")
        .map_err(storage_error)?;
    let mut preferences = DiffEncodingPreferences::default();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage_error)?;
    for row in rows {
        let (repo_path, choice) = row.map_err(storage_error)?;
        preferences
            .repositories
            .insert(repo_path, diff_encoding_choice_from_db(&choice)?);
    }
    Ok(preferences)
}

fn save_diff_encoding_preferences_tx(
    tx: &Transaction<'_>,
    preferences: &DiffEncodingPreferences,
) -> Result<()> {
    tx.execute("DELETE FROM diff_encoding_preferences", [])
        .map_err(storage_error)?;
    for (repo_path, choice) in &preferences.repositories {
        tx.execute(
            "INSERT INTO diff_encoding_preferences (repo_path, choice, updated_at) VALUES (?1, ?2, ?3)",
            params![repo_path, diff_encoding_choice_to_db(*choice), now_seconds()],
        )
        .map_err(storage_error)?;
    }
    Ok(())
}

fn load_remote_credential_bindings_from_conn(
    conn: &Connection,
) -> Result<RemoteCredentialBindings> {
    let mut stmt = conn
        .prepare(
            "SELECT repo_path, remote_name, remote_url, policy_kind, credential_record_id
             FROM remote_credential_bindings
             ORDER BY repo_path, remote_name",
        )
        .map_err(storage_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RemoteCredentialBinding {
                repo_path: row.get(0)?,
                remote_name: row.get(1)?,
                remote_url: row.get(2)?,
                policy: remote_credential_policy_from_db(row.get::<_, String>(3)?, row.get(4)?)
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(StorageConversionError(err.to_string())),
                        )
                    })?,
            })
        })
        .map_err(storage_error)?;
    let mut bindings = RemoteCredentialBindings::default();
    for row in rows {
        bindings.remotes.push(row.map_err(storage_error)?);
    }
    Ok(bindings)
}

fn save_remote_credential_bindings_tx(
    tx: &Transaction<'_>,
    bindings: &RemoteCredentialBindings,
) -> Result<()> {
    tx.execute("DELETE FROM remote_credential_bindings", [])
        .map_err(storage_error)?;
    for binding in &bindings.remotes {
        let (policy_kind, credential_record_id) = remote_credential_policy_to_db(&binding.policy);
        tx.execute(
            "INSERT OR REPLACE INTO remote_credential_bindings
             (repo_path, remote_name, remote_url, policy_kind, credential_record_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                binding.repo_path,
                binding.remote_name,
                binding.remote_url,
                policy_kind,
                credential_record_id,
                now_seconds()
            ],
        )
        .map_err(storage_error)?;
    }
    Ok(())
}

fn load_proxy_settings_from_conn(conn: &Connection) -> Result<NetworkProxySettings> {
    conn.query_row(
        "SELECT mode, http_proxy, https_proxy, socks5_proxy FROM network_proxy_settings WHERE id = 1",
        [],
        |row| {
            Ok(NetworkProxySettings {
                mode: network_proxy_mode_from_db(row.get::<_, String>(0)?).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(StorageConversionError(err.to_string())),
                    )
                })?,
                custom: CustomProxySettings {
                    http_proxy: row.get(1)?,
                    https_proxy: row.get(2)?,
                    socks5_proxy: row.get(3)?,
                },
            })
        },
    )
    .optional()
    .map_err(storage_error)
    .map(|settings| settings.unwrap_or_default())
}

fn save_proxy_settings_tx(tx: &Transaction<'_>, settings: &NetworkProxySettings) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO network_proxy_settings
         (id, mode, http_proxy, https_proxy, socks5_proxy, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![
            network_proxy_mode_to_db(settings.mode),
            settings.custom.http_proxy,
            settings.custom.https_proxy,
            settings.custom.socks5_proxy,
            now_seconds()
        ],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_update_preferences_from_conn(conn: &Connection) -> Result<UpdatePreferences> {
    conn.query_row(
        "SELECT auto_check, skipped_version FROM update_preferences WHERE id = 1",
        [],
        |row| {
            Ok(UpdatePreferences {
                auto_check: row.get::<_, bool>(0)?,
                skipped_version: row.get::<_, Option<String>>(1)?,
            })
        },
    )
    .optional()
    .map_err(storage_error)
    .map(|prefs| prefs.unwrap_or_default())
}

fn save_update_preferences_tx(tx: &Transaction<'_>, preferences: &UpdatePreferences) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO update_preferences
         (id, auto_check, skipped_version, updated_at)
         VALUES (1, ?1, ?2, ?3)",
        params![
            if preferences.auto_check { 1 } else { 0 },
            preferences.skipped_version,
            now_seconds()
        ],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_theme_mode_from_conn(conn: &Connection) -> Result<ThemeMode> {
    conn.query_row(
        "SELECT mode FROM theme_preferences WHERE id = 1",
        [],
        |row| {
            theme_mode_from_db(row.get::<_, String>(0)?).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(StorageConversionError(err.to_string())),
                )
            })
        },
    )
    .optional()
    .map_err(storage_error)
    .map(|mode| mode.unwrap_or_default())
}

fn save_theme_mode_tx(tx: &Transaction<'_>, mode: ThemeMode) -> Result<()> {
    // 只更新 mode 列，保留 accent（用 UPSERT 的 UPDATE 语义，行不存在时先插入）。
    tx.execute(
        "INSERT INTO theme_preferences (id, mode, accent, updated_at) VALUES (1, ?1, 0, ?2)
         ON CONFLICT(id) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
        params![theme_mode_to_db(mode), now_seconds()],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_theme_accent_from_conn(conn: &Connection) -> Result<usize> {
    // accent 列可能因旧库未迁移而缺失，但 ensure_theme_accent_column 已保证它存在。
    conn.query_row(
        "SELECT accent FROM theme_preferences WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(storage_error)
    // 行不存在或读取失败时回退默认 0
    .map(|value| value.unwrap_or(0).max(0) as usize)
}

fn save_theme_accent_tx(tx: &Transaction<'_>, accent: usize) -> Result<()> {
    let accent = accent as i64;
    tx.execute(
        "INSERT INTO theme_preferences (id, mode, accent, updated_at)
         VALUES (1, 'system', ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET accent = excluded.accent, updated_at = excluded.updated_at",
        params![accent, now_seconds()],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_ai_provider_settings_from_conn(conn: &Connection) -> Result<AiProviderSettings> {
    // AI 配置整体存为 JSON payload，便于后续增字段而不改表结构。
    conn.query_row(
        "SELECT payload FROM ai_provider_settings WHERE id = 1",
        [],
        |row| {
            let payload: String = row.get(0)?;
            serde_json::from_str::<AiProviderSettings>(&payload).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(StorageConversionError(format!("AI 配置解析失败：{err}"))),
                )
            })
        },
    )
    .optional()
    .map_err(storage_error)
    .map(|settings| settings.unwrap_or_default())
}

fn save_ai_provider_settings_tx(tx: &Transaction<'_>, settings: &AiProviderSettings) -> Result<()> {
    let payload = serde_json::to_string(settings)
        .map_err(|err| GitError::Message(format!("AI 配置序列化失败：{err}")))?;
    tx.execute(
        "INSERT OR REPLACE INTO ai_provider_settings (id, enabled, payload, updated_at)
         VALUES (1, ?1, ?2, ?3)",
        params![if settings.enabled { 1 } else { 0 }, payload, now_seconds()],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_external_merge_settings_from_conn(conn: &Connection) -> Result<ExternalMergeSettings> {
    // 外部合并配置整体存为 JSON payload，便于后续支持更多工具类型。
    conn.query_row(
        "SELECT payload FROM external_merge_settings WHERE id = 1",
        [],
        |row| {
            let payload: String = row.get(0)?;
            serde_json::from_str::<ExternalMergeSettings>(&payload).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(StorageConversionError(format!(
                        "外部合并工具配置解析失败：{err}"
                    ))),
                )
            })
        },
    )
    .optional()
    .map_err(storage_error)
    .map(|settings| settings.unwrap_or_default())
}

fn save_external_merge_settings_tx(
    tx: &Transaction<'_>,
    settings: &ExternalMergeSettings,
) -> Result<()> {
    let payload = serde_json::to_string(settings)
        .map_err(|err| GitError::Message(format!("外部合并工具配置序列化失败：{err}")))?;
    tx.execute(
        "INSERT OR REPLACE INTO external_merge_settings (id, enabled, payload, updated_at)
         VALUES (1, ?1, ?2, ?3)",
        params![if settings.enabled { 1 } else { 0 }, payload, now_seconds()],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn load_credential_records_from_conn(conn: &Connection) -> Result<Vec<CredentialRecord>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, display_name, scope, kind, host, remote_url, username, key_path,
                    created_at, updated_at, last_used
             FROM credential_records
             ORDER BY updated_at DESC, host ASC, remote_url ASC, username ASC",
        )
        .map_err(storage_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CredentialRecord {
                id: row.get(0)?,
                display_name: row.get(1)?,
                scope: credential_scope_from_db(row.get::<_, String>(2)?).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(StorageConversionError(err.to_string())),
                    )
                })?,
                kind: stored_credential_kind_from_db(row.get::<_, String>(3)?).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(StorageConversionError(err.to_string())),
                    )
                })?,
                host: row.get(4)?,
                remote_url: row.get(5)?,
                username: row.get(6)?,
                key_path: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
                last_used: row.get(10)?,
            })
        })
        .map_err(storage_error)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn save_credential_records_tx(tx: &Transaction<'_>, records: &[CredentialRecord]) -> Result<()> {
    // 这里仅保存凭据索引元数据，密码、PAT 和 SSH passphrase 仍由系统 Keyring 托管。
    tx.execute("DELETE FROM credential_records", [])
        .map_err(storage_error)?;
    for record in records {
        tx.execute(
            "INSERT OR REPLACE INTO credential_records
             (id, display_name, scope, kind, host, remote_url, username, key_path,
              created_at, updated_at, last_used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.id,
                record.display_name,
                credential_scope_to_db(record.scope),
                stored_credential_kind_to_db(record.kind),
                record.host,
                record.remote_url,
                record.username,
                record.key_path,
                record.created_at,
                record.updated_at,
                record.last_used,
            ],
        )
        .map_err(storage_error)?;
    }
    Ok(())
}

fn recreate_database_file(path: &Path) -> Result<()> {
    if path.exists() {
        let backup = path.with_extension(format!("sqlite3.broken.{}", now_seconds()));
        fs::rename(path, backup)?;
    }
    Ok(())
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn storage_error(err: rusqlite::Error) -> GitError {
    GitError::Message(format!("本地配置数据库错误：{err}"))
}

fn network_proxy_mode_to_db(mode: NetworkProxyMode) -> &'static str {
    match mode {
        NetworkProxyMode::Disabled => "disabled",
        NetworkProxyMode::System => "system",
        NetworkProxyMode::Custom => "custom",
    }
}

fn network_proxy_mode_from_db(value: String) -> Result<NetworkProxyMode> {
    match value.as_str() {
        "disabled" => Ok(NetworkProxyMode::Disabled),
        "system" => Ok(NetworkProxyMode::System),
        "custom" => Ok(NetworkProxyMode::Custom),
        _ => Err(GitError::Message(format!("未知代理模式：{value}"))),
    }
}

fn theme_mode_to_db(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

fn theme_mode_from_db(value: String) -> Result<ThemeMode> {
    match value.as_str() {
        "system" => Ok(ThemeMode::System),
        "light" => Ok(ThemeMode::Light),
        "dark" => Ok(ThemeMode::Dark),
        _ => Err(GitError::Message(format!("未知主题模式：{value}"))),
    }
}

fn diff_encoding_choice_to_db(choice: DiffEncodingChoice) -> &'static str {
    match choice {
        DiffEncodingChoice::Auto => "auto",
        DiffEncodingChoice::Utf8 => "utf8",
        DiffEncodingChoice::Gb18030 => "gb18030",
        DiffEncodingChoice::Big5 => "big5",
    }
}

fn diff_encoding_choice_from_db(value: &str) -> Result<DiffEncodingChoice> {
    match value {
        "auto" => Ok(DiffEncodingChoice::Auto),
        "utf8" => Ok(DiffEncodingChoice::Utf8),
        "gb18030" => Ok(DiffEncodingChoice::Gb18030),
        "big5" => Ok(DiffEncodingChoice::Big5),
        _ => Err(GitError::Message(format!("未知 diff 编码偏好：{value}"))),
    }
}

fn remote_credential_policy_to_db(policy: &RemoteCredentialPolicy) -> (&'static str, Option<&str>) {
    match policy {
        RemoteCredentialPolicy::AutoMatch => ("auto", None),
        RemoteCredentialPolicy::NoCredential => ("none", None),
        RemoteCredentialPolicy::Record(id) => ("record", Some(id.as_str())),
    }
}

fn remote_credential_policy_from_db(
    kind: String,
    credential_record_id: Option<String>,
) -> Result<RemoteCredentialPolicy> {
    match kind.as_str() {
        "auto" => Ok(RemoteCredentialPolicy::AutoMatch),
        "none" => Ok(RemoteCredentialPolicy::NoCredential),
        "record" => credential_record_id
            .map(RemoteCredentialPolicy::Record)
            .ok_or_else(|| GitError::Message("远端凭据绑定缺少记录 ID".to_string())),
        _ => Err(GitError::Message(format!("未知远端凭据策略：{kind}"))),
    }
}

fn credential_scope_to_db(scope: CredentialScope) -> &'static str {
    match scope {
        CredentialScope::RemoteUrl => "remote_url",
        CredentialScope::Host => "host",
    }
}

fn credential_scope_from_db(value: String) -> Result<CredentialScope> {
    match value.as_str() {
        "remote_url" => Ok(CredentialScope::RemoteUrl),
        "host" => Ok(CredentialScope::Host),
        _ => Err(GitError::Message(format!("未知凭据作用域：{value}"))),
    }
}

fn stored_credential_kind_to_db(kind: StoredCredentialKind) -> &'static str {
    match kind {
        StoredCredentialKind::HttpsUserPass => "https_user_pass",
        StoredCredentialKind::SshKey => "ssh_key",
    }
}

fn stored_credential_kind_from_db(value: String) -> Result<StoredCredentialKind> {
    match value.as_str() {
        "https_user_pass" => Ok(StoredCredentialKind::HttpsUserPass),
        "ssh_key" => Ok(StoredCredentialKind::SshKey),
        _ => Err(GitError::Message(format!("未知凭据类型：{value}"))),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CredentialIndex {
    #[serde(default)]
    records: Vec<CredentialRecord>,
}

#[derive(Debug)]
struct StorageConversionError(String);

impl std::fmt::Display for StorageConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for StorageConversionError {}

#[cfg(test)]
#[path = "tests/storage.rs"]
mod tests;
