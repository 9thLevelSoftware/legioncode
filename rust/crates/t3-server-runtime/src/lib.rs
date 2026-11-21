#![forbid(unsafe_code)]
//! Live Server I/O Engine for `t3 serve`.

pub mod acp_protocol_io;
pub mod acp_registry;
pub mod acp_subprocess;
pub mod error;
pub mod git_checkpoints;
pub mod git_checkpoints_cli;
pub mod http_api;
pub mod http_auth;
pub mod http_auth_admin;
pub mod http_auth_guard;
pub mod http_integrations;
pub mod http_integrations_scm;
pub mod http_projects;
pub mod http_projects_models;
pub mod http_traces;
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
pub mod preview_browser_diag;
pub mod preview_cdp;
pub mod provider_probe;
pub mod replay_driver;
pub mod static_assets;
pub mod static_assets_http;
pub mod terminal_runtime;
pub mod terminal_session;
pub mod turn_events;
pub mod turn_events_builders;
pub mod turn_events_updates;

pub use acp_registry::{AcpDriverSpec, AcpProcessRegistry};
pub use acp_subprocess::{AcpTurnInput, execute_acp_turn};
pub use error::ServerRuntimeError;
pub use git_checkpoints::{
    CaptureCheckpointRequest, CapturedCheckpoint, GitCheckpointEngine, RollbackExecutionOutcome,
};
pub use http_api::{RuntimeHttpState, runtime_api_router, runtime_full_router};
pub use http_auth::{AuthenticatedPrincipal, authenticate_headers};
pub use http_auth_guard::{authorize_request, authorize_scopes};
pub use http_projects_models::{HttpProject, HttpProjectSnapshot};
pub use http_traces::normalize_otlp_payload;
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
pub use preview_cdp::{CdpConnection, CdpKeyEventParams, CdpMouseEventParams};
pub use provider_probe::{ProviderProbeResult, ProviderProbeService, ProviderProbeTarget};
pub use replay_driver::{ReplayTurnInput, execute_dev_replay_turn, select_dev_replay_scenario};
pub use static_assets::static_assets_router;
pub use terminal_runtime::TerminalRuntime;
pub use terminal_session::{TerminalOpenRequest, TerminalOpenResult, TerminalSnapshotResult};
pub use turn_events::{TurnEmitContext, commit_runtime_events};

/// Crate version reported by `t3-server-runtime`.
pub const SERVER_RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the crate version string for `t3-server-runtime`.
pub const fn server_runtime_version() -> &'static str {
    SERVER_RUNTIME_VERSION
}
