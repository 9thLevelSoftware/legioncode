#![forbid(unsafe_code)]
//! Derived server paths, SQLite WAL storage, single-FD ownership lock, and database maintenance.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//! let paths = t3_persistence::ServerDerivedPaths::derive(dir.path(), false, false);
//! t3_persistence::ensure_server_directories(&paths)?;
//! let env_id = t3_persistence::ensure_environment_id(&paths.environment_id_path)?;
//! assert!(!env_id.is_empty());
//! # Ok(())
//! # }
//! ```

mod claude_history_fork;
mod doc_extractor_entities;
mod doc_extractor_layout;
mod doc_extractor_legacy;
mod doc_extractor_odf_ws;
mod doc_extractor_office;
mod doc_extractor_rtf;
mod doc_extractor_xlsx;
mod doc_extractor_xml;
mod triage_prompt;
mod triage_prompt_playbook;

pub mod auth_connect_devices;
pub mod auth_mutations;
pub mod auth_records;
pub mod claude_history;
pub mod doc_extractor;
pub mod maintenance;
pub mod ownership;
pub mod pairing_tokens;
pub mod paths;
pub mod project_store;
pub mod service_env;
pub mod service_launcher;
pub mod service_preflight;
pub mod sqlite;
pub mod triage_session;

pub use auth_connect_devices::{
    ConnectApproveOutcome, ConnectRevokeOutcome, TrustedConnectDevice, WaitingConnectDevice,
    approve_connect_device_by_code, approve_connect_device_by_code_at,
    connect_device_verification_code, format_connect_device_list, list_connect_devices,
    list_connect_devices_at, normalize_verification_code, revoke_connect_device_by_code,
    revoke_connect_device_by_code_at,
};
pub use auth_mutations::{
    DEFAULT_CLI_SESSION_SUBJECT, IssueSessionInput, IssuedSession,
    format_issued_pairing_credential, format_issued_session, issue_scoped_session,
    issue_scoped_session_at, parse_cli_duration_ms, revoke_pairing_link, revoke_pairing_link_at,
    revoke_session, revoke_session_at,
};
pub use auth_records::{
    AuthRecordsError, AuthSessionRow, ClientMetadataRow, PairingLinkRow, PeerLinkRecord,
    format_pairing_credential_list, format_session_list, insert_peer_link,
    list_active_pairing_links, list_active_sessions, list_peer_links, unlink_peer_link,
};

pub use claude_history::{
    ClaudeHistoryError, ClaudeHistoryOptions, ForkSessionResult, SessionMessage, fork_session,
    get_session_messages,
};
pub use doc_extractor::{
    DOC_MAX_DEPTH, DOC_MAX_EXPANDED_BYTES, DOC_MAX_INPUT_BYTES, DOC_MAX_OUTPUT_CHARS,
    DOC_MAX_XML_NODES, DOC_TRUNCATED_SUFFIX, DocExtractError, extract_document_text,
    extract_html_fragment_text,
};
pub use maintenance::{
    CompactDatabaseResult, DATABASE_COMPACTION_RECOMMENDED_BYTES, DatabaseCompactStep,
    DatabaseInspection, DoctorOptions, DoctorRepair, DoctorRepairId, LegacyOnlyRecords,
    MaintenanceError, compact_database, compact_database_with_steps, format_bytes,
    inspect_database, plan_doctor_repairs, remove_legacy_database, render_doctor_report,
};
pub use ownership::{
    CancellationToken, ManagedPolicyRecord, ManagedPolicyState, OwnershipError,
    PersistedServerRuntimeState, SERVER_EXIT_CODE_STATE_DIR_OWNED, ServerOwnerLock,
    format_epoch_millis_utc, is_process_alive, read_live_server_runtime_state,
    read_persisted_server_runtime_state,
};
pub use pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, DEFAULT_PAIRING_TOKEN_TTL_MS, DEFAULT_SESSION_TTL_MS,
    ExchangedSession, IssuePairingTokenInput, IssuedPairingToken, PAIRING_CODE_ALPHABET,
    PAIRING_CODE_LENGTH, PairingTokenError, STANDARD_CLIENT_SCOPES, VerifiedBearerSession,
    ensure_pairing_schema, exchange_pairing_token, issue_pairing_token, role_for_scopes,
    verify_bearer_session,
};
pub use paths::{PathsError, ServerDerivedPaths, ensure_environment_id, ensure_server_directories};
pub use project_store::{
    CreateProjectInput, ProjectAddResult, ProjectDatabase, ProjectRecord, ProjectStoreError,
    RenameProjectResult, SoftDeleteProjectResult, add_or_unarchive_project, archive_project,
    create_project, ensure_project_store_schema, find_project_by_id_or_workspace,
    list_non_deleted_projects, list_projects, rename_project, soft_delete_project,
    unarchive_project,
};
pub use service_env::{parse_service_env, upsert_service_env, upsert_service_env_map};
pub use service_launcher::{
    DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, DesktopUpdateMarker, LauncherChildPlan, LauncherChildRole,
    LauncherOptions, LauncherStartupDecision, SERVICE_ENV_FILE, SERVICE_LAUNCHER_CONTEXT_ENV,
    SERVICE_LAUNCHER_LOCK_FILE, SERVICE_RESTART_PENDING_FILE, SERVICE_SERVING_LOCK_FILE,
    SERVICE_STATE_FILE, SERVICE_STOP_MARKER_FILE, SERVICE_UNIT_ENV, ServiceLauncherContext,
    ServiceLauncherError, ServiceLockGuard, ServiceState, ServiceUpdateRecord,
    compare_exact_service_versions, decode_service_launcher_context, is_exact_service_version,
    parse_legacy_service_state, parse_service_launcher_args, parse_service_state,
    plan_launcher_startup, publish_update_marker, read_service_state, remaining_update_wait_ms,
    runtime_entry_paths, runtime_exists, service_launcher_is_running, service_launcher_is_serving,
    service_launcher_lock_path, service_serving_lock_path, service_state_path, state_dir_owned,
    write_service_state,
};
pub use service_preflight::{
    FIRST_V2_MIGRATION_ID, SERVICE_LAUNCHER_PROTOCOL, SUPPORTED_LAUNCHER_PROTOCOLS,
    ServicePreflightResult, V2_MIGRATION_MANIFEST, is_supported_launcher_protocol,
    reconciled_v2_preview_history, recorded_migrations, run_service_preflight,
    skipped_v2_migrations,
};
pub use sqlite::{
    BUILD_MIGRATION_HEAD, RecordedMigration, SqliteOpenOptions, SqliteStore, SqliteStoreError,
    WAL_SIZE_LIMIT_BYTES, is_damaged_sqlite_code, is_damaged_sqlite_error, read_migration_head,
    read_recorded_migrations,
};
pub use triage_session::{
    BOOT_SERVICE_LOG_FILE, RELAY_CLIENT_LOG_FILE, TRIAGE_PLAYBOOK, TRIAGE_PROMPT_FILE_NAME,
    TriageContextInput, TriageEntry, TriageError, TriagePaths, TriageSessionFiles,
    TriageSourceClone, WriteTriageSessionInput, build_triage_app_prompt, build_triage_context,
    build_triage_launch_prompt, build_triage_seed_prompt, describe_server_process,
    serialize_composer_file_link, triage_source_cache_dir, write_triage_session,
};
