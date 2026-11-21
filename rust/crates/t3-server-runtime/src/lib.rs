#![forbid(unsafe_code)]
//! Live Server I/O Engine for `t3 serve`.

pub mod acp_protocol_io;
pub mod acp_registry;
pub mod acp_subprocess;
pub mod device_hub;
pub mod error;
pub mod git_checkpoints;
pub mod git_checkpoints_cli;
pub mod http_api;
mod http_api_routes;
pub mod http_auth;
pub mod http_auth_admin;
pub mod http_auth_guard;
pub mod http_integrations;
pub mod http_integrations_scm;
pub mod http_mcp_signin;
pub mod http_peer_pairing;
pub mod http_peer_relay;
pub mod http_peer_relay_client;
pub mod http_preview_tunnel;
pub mod http_projects;
pub mod http_projects_models;
pub mod http_projects_read;
pub mod http_traces;
pub mod http_workspace_folders;
pub mod non_acp_http;
pub mod non_acp_subprocess;
pub mod non_acp_subprocess_loop;
pub mod non_acp_subprocess_proto;
pub mod non_acp_turn_sink;
pub mod outbox_checkpoints;
pub mod outbox_executor;
pub mod outbox_extra;
pub mod outbox_state;
pub mod outbox_turn_dispatch;
pub mod preview_browser;
pub mod preview_browser_catalog;
pub mod preview_browser_diag;
mod preview_browser_resize;
pub mod preview_cdp;
pub mod provider_probe;
pub mod replay_driver;
pub mod startup_auto_pull;
pub mod static_assets;
pub mod static_assets_http;
pub mod storage_cleanup;
pub mod terminal_runtime;
pub mod terminal_session;
pub mod turn_events;
pub mod turn_events_builders;
pub mod turn_events_updates;

pub use acp_registry::{AcpDriverSpec, AcpProcessRegistry};
pub use acp_subprocess::{AcpTurnInput, execute_acp_turn};
pub use device_hub::{DeviceHubBackend, DeviceHubProcessState, LocalDeviceHubManager};
pub use error::ServerRuntimeError;
pub use git_checkpoints::{
    CaptureCheckpointRequest, CapturedCheckpoint, GitCheckpointEngine, RollbackExecutionOutcome,
};
pub use http_api::{RuntimeHttpState, runtime_api_router, runtime_full_router};
pub use http_auth::{AuthenticatedPrincipal, authenticate_headers};
pub use http_auth_guard::{authorize_request, authorize_scopes};
pub use http_mcp_signin::{McpOAuthFlowConfig, McpSignInRegistry};
pub use http_peer_relay_client::{
    relay_peer_thread_history, relay_peer_thread_item, relay_peer_thread_stream,
};
pub use http_projects_models::{HttpProject, HttpProjectSnapshot};
pub use http_projects_read::{
    ProjectScriptAction, ProjectSettingsOverride, apply_saved_project_settings,
    read_project_with_saved_settings, resolve_project_scripts_value,
};
pub use http_traces::{format_redacted_access_log, normalize_otlp_payload};
pub use http_workspace_folders::{
    add_task_folder_repository_service, get_workspace_task_folder_service,
    list_workspace_folders_service, scan_workspace_folders_service,
};
pub use non_acp_http::{NonAcpHttpDriver, NonAcpHttpTurnInput, execute_non_acp_http_turn};
pub use non_acp_subprocess::{
    NonAcpProcessRegistry, NonAcpSubprocessInput, execute_non_acp_subprocess_turn,
};
pub use non_acp_subprocess_proto::NonAcpSubprocessKind;
pub use outbox_executor::{ProviderDriverMode, RuntimeOutboxExecutor};
pub use outbox_state::utc_now_iso;
pub use preview_browser::{
    PreviewAction, PreviewBrowserRuntime, PreviewLaunchDiagnostic, diagnose_browser_launch_failure,
    discover_browser_executable,
};
pub use preview_browser_catalog::{
    ChromeArchiveSpec, ChromePlatform, MANAGED_CHROME_VERSION, all_chrome_archive_specs,
    chrome_archive_spec,
};
pub use preview_cdp::{CdpConnection, CdpKeyEventParams, CdpMouseEventParams};
pub use provider_probe::{ProviderProbeResult, ProviderProbeService, ProviderProbeTarget};
pub use replay_driver::{ReplayTurnInput, execute_dev_replay_turn, select_dev_replay_scenario};
pub use startup_auto_pull::{
    AutoPullOutcome, AutoPullProjectSpec, GitStatusFetchOptions, StatusFetchCoordinator,
    auto_pull_projects,
};
pub use static_assets::{serve_workspace_asset_file, static_assets_router};
pub use static_assets_http::{
    is_workspace_preview_entry_path, resolve_previewable_workspace_asset,
};
pub use storage_cleanup::{
    CleanupGitStatusRunner, ForkPullRequestRef, OsCleanupGitStatusRunner, WorktreeCleanupCandidate,
    WorktreeCleanupEntry, contains_non_dangling_nested_git_repo, evaluate_and_clean_worktrees,
    find_merged_pr_in_head_repository, is_dangling_gitfile,
};
pub use terminal_runtime::TerminalRuntime;
pub use terminal_session::{TerminalOpenRequest, TerminalOpenResult, TerminalSnapshotResult};
pub use turn_events::{TurnEmitContext, commit_runtime_events};

/// Crate version reported by `t3-server-runtime`.
pub const SERVER_RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the crate version string for `t3-server-runtime`.
pub const fn server_runtime_version() -> &'static str {
    SERVER_RUNTIME_VERSION
}
