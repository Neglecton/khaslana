pub mod ai;
pub mod credentials;
pub mod external_merge;
pub mod git;
pub mod proxy;
pub mod storage;
pub mod syntax;
pub mod types;
pub mod update;
pub mod workflow;

pub use credentials::{
    CredentialProvider, CredentialRecord, CredentialRequest, CredentialScope, CredentialStore,
    GitCredential, KeyringCredentialStore, MemoryCredentialStore, PromptCredentialProvider,
    RemoteCredentialPolicy, StoredCredentialKind, credential_display_target,
    credential_key_filename, credential_kind_label, credential_record_is_compatible_with_url,
    credential_record_label, credential_record_matches_remote_url, credential_scope_label,
    normalize_remote_url, test_credential_connection,
};
pub use external_merge::ExternalMergeSettings;
pub use git::{
    BrowseRefKind, FULL_FILE_TOO_LARGE_MESSAGE, GitService, HistoryRefsCache, LineSelection,
    NoopProgress, ProgressEmitter, SelectedDiffLine, SelectionSide,
};
pub use proxy::{CustomProxySettings, NetworkProxyMode, NetworkProxySettings};
pub use storage::{
    AppStorage, DiffEncodingPreferences, MigrationOutcome, RemoteCredentialBinding,
    RemoteCredentialBindings, SessionState, ShortcutBindings, ThemeMode, UpdatePreferences,
    apply_pending_portable_migration, default_database_path, legacy_database_dir,
    legacy_database_path, portable_database_dir, portable_database_path, portable_migrated_marker,
    portable_pending_marker,
};
pub use types::*;
pub use workflow::{
    RemoteBranchGuardAction, WorkflowDefinition, WorkflowExecutor, WorkflowInputDefinition,
    WorkflowPreview, WorkflowPreviewStep, WorkflowProgressEvent, WorkflowRunOptions,
    WorkflowRunResult, WorkflowStep, parse_workflow_json5,
};

pub use ai::{
    AgentChatMessage, AgentEvent, AgentToolCall, AgentTurn, AiApiType, AiProviderSettings,
    AiReviewRecord, AiReviewResult, AiReviewStep, ChatClient, ChatMessage, ChatResult, ChatRole,
    MERGE_CONTEXT_BUDGET_CHARS, MERGE_SEGMENT_LIMIT, MERGE_SINGLE_BLOCK_LIMIT,
    MERGE_WHOLE_FILE_LIMIT, MergeSegment, ReviewAgentInput, StreamDelta, ToolSchema,
    build_segment_messages, conflict_merge_prompts, file_diff_to_patch_text, list_review_records,
    response_contains_conflict_markers, run_review_agent, save_review_record, split_diff3_text,
    split_reasoning, strip_code_fence, validate_generated_content,
};

// 浏览模式领域类型已在 types::* 中重新导出（BrowseTarget / BrowseEntry / BrowseEntryKind / BrowseFileContent）。
