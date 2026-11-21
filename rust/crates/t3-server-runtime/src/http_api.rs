//! Shared [`RuntimeHttpState`] and composable Axum routers for `t3 serve`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Router;
use axum::routing::{delete, get, post};
use rusqlite::Connection;
use t3_orchestrator::Orchestrator;
use t3_remote::secrets::FileSecretStore;

use crate::error::ServerRuntimeError;
use crate::http_auth::{handle_oauth_token, handle_pair};
use crate::http_auth_admin::{
    handle_approve_connect_device, handle_create_pairing, handle_issue_session,
    handle_list_connect_devices, handle_list_pairing, handle_list_sessions,
    handle_revoke_connect_device, handle_revoke_pairing, handle_revoke_session,
};
use crate::http_integrations::{
    handle_link_peer, handle_list_peers, handle_preview_action, handle_provider_probe,
    handle_unlink_peer,
};
use crate::http_integrations_scm::{
    handle_get_scm_diff, handle_get_scm_pr, handle_post_scm_diff, handle_post_scm_pr,
};
use crate::http_projects::{handle_get_projects, handle_mutate_project};
use crate::http_traces::handle_otlp_traces;
use crate::preview_browser::PreviewBrowserRuntime;
use crate::provider_probe::ProviderProbeService;
use crate::static_assets::static_assets_router;
use crate::terminal_runtime::TerminalRuntime;

/// Shared server HTTP state mounted by `t3 serve` or standalone integration tests.
#[derive(Clone)]
pub struct RuntimeHttpState {
    db: Arc<StdMutex<Connection>>,
    /// Session cookie name checked by `BUG-03` auth precedence and set on pairing.
    pub cookie_name: String,
    /// When `true`, protected endpoints require a verified pairing session or `dev_auth_token`.
    pub require_auth: bool,
    /// Optional fixed dev token permitted in `Authorization` or `t3_dev_auth` cookie.
    pub dev_auth_token: Option<String>,
    /// Directory where `traces.ndjson` and runtime logs are written.
    pub logs_dir: PathBuf,
    /// Directory where encrypted/binary server secrets (e.g. peer link tokens) are stored.
    pub secrets_dir: PathBuf,
    /// Built frontend dist directory served via [`static_assets_router`].
    pub static_dir: Option<PathBuf>,
    /// Optional live orchestrator reference.
    pub orchestrator: Option<Arc<Orchestrator>>,
    /// Optional live terminal runtime reference.
    pub terminal_runtime: Option<Arc<TerminalRuntime>>,
    /// Provider and speech endpoint health-probe service.
    pub probe_service: Arc<ProviderProbeService>,
    /// Headless browser CDP host runtime for `/api/preview/action`.
    pub preview_browser: Arc<PreviewBrowserRuntime>,
    trace_write_lock: Arc<StdMutex<()>>,
}

impl RuntimeHttpState {
    /// Construct a new [`RuntimeHttpState`] backed by `db` and writing traces to `logs_dir/traces.ndjson`.
    pub fn new(db: Arc<StdMutex<Connection>>, logs_dir: impl Into<PathBuf>) -> Self {
        let logs = logs_dir.into();
        let secrets = logs.join("secrets");
        Self {
            db,
            cookie_name: "t3_session".to_owned(),
            require_auth: true,
            dev_auth_token: None,
            logs_dir: logs,
            secrets_dir: secrets,
            static_dir: None,
            orchestrator: None,
            terminal_runtime: None,
            probe_service: Arc::new(ProviderProbeService::default()),
            preview_browser: Arc::new(PreviewBrowserRuntime::new()),
            trace_write_lock: Arc::new(StdMutex::new(())),
        }
    }

    /// Borrow the underlying SQLite connection handle.
    pub const fn db(&self) -> &Arc<StdMutex<Connection>> {
        &self.db
    }

    /// Configure the static web bundle directory served by the fallback router.
    #[must_use]
    pub fn with_static_dir(mut self, static_dir: Option<PathBuf>) -> Self {
        self.static_dir = static_dir;
        self
    }

    /// Configure whether API endpoints require authentication (defaults to `true`).
    #[must_use]
    pub const fn with_require_auth(mut self, require_auth: bool) -> Self {
        self.require_auth = require_auth;
        self
    }

    /// Configure an optional reusable development bearer token.
    #[must_use]
    pub fn with_dev_auth_token(mut self, token: Option<String>) -> Self {
        self.dev_auth_token = token;
        self
    }

    /// Override the primary session cookie name (defaults to `"t3_session"`).
    #[must_use]
    pub fn with_cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Attach an [`Orchestrator`] handle.
    #[must_use]
    pub fn with_orchestrator(mut self, orchestrator: Arc<Orchestrator>) -> Self {
        self.orchestrator = Some(orchestrator);
        self
    }

    /// Attach a [`TerminalRuntime`] handle.
    #[must_use]
    pub fn with_terminal_runtime(mut self, runtime: Arc<TerminalRuntime>) -> Self {
        self.terminal_runtime = Some(runtime);
        self
    }

    /// Override the directory holding persisted binary server secrets.
    #[must_use]
    pub fn with_secrets_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.secrets_dir = dir.into();
        self
    }

    /// Override the [`ProviderProbeService`] used by `/api/providers/probe`.
    #[must_use]
    pub fn with_probe_service(mut self, service: ProviderProbeService) -> Self {
        self.probe_service = Arc::new(service);
        self
    }

    /// Override the [`PreviewBrowserRuntime`] used by `/api/preview/action`.
    #[must_use]
    pub fn with_preview_browser(mut self, browser: PreviewBrowserRuntime) -> Self {
        self.preview_browser = Arc::new(browser);
        self
    }

    /// Return `<logs_dir>/traces.ndjson`.
    pub fn traces_file_path(&self) -> PathBuf {
        self.logs_dir.join("traces.ndjson")
    }

    /// Open a [`FileSecretStore`] rooted at `self.secrets_dir`.
    pub fn secrets_store(&self) -> Result<FileSecretStore, ServerRuntimeError> {
        FileSecretStore::new(&self.secrets_dir).map_err(|err| {
            ServerRuntimeError::Persistence(format!("secrets store init error: {err}"))
        })
    }

    /// Borrow the [`ProviderProbeService`].
    pub fn probe_service(&self) -> &ProviderProbeService {
        &self.probe_service
    }

    /// Borrow the [`PreviewBrowserRuntime`].
    pub fn preview_browser(&self) -> &PreviewBrowserRuntime {
        &self.preview_browser
    }

    pub(crate) fn trace_write_lock(&self) -> Arc<StdMutex<()>> {
        Arc::clone(&self.trace_write_lock)
    }
}

/// Build the API routes (`/api/auth/*`, `/oauth/token`, `/api/projects*`, `/api/scm/*`,
/// `/api/peers*`, `/api/providers/probe`, `/api/preview/action`, `/api/observability/v1/traces`).
pub fn runtime_api_router(state: Arc<RuntimeHttpState>) -> Router {
    Router::new()
        .route("/api/auth/pair", post(handle_pair))
        .route("/oauth/token", post(handle_oauth_token))
        .route(
            "/api/auth/pairing",
            get(handle_list_pairing).post(handle_create_pairing),
        )
        .route("/api/auth/pairing/{id}", delete(handle_revoke_pairing))
        .route(
            "/api/auth/sessions",
            get(handle_list_sessions).post(handle_issue_session),
        )
        .route("/api/auth/sessions/{id}", delete(handle_revoke_session))
        .route(
            "/api/auth/connect-devices",
            get(handle_list_connect_devices),
        )
        .route(
            "/api/auth/connect-devices/approve",
            post(handle_approve_connect_device),
        )
        .route(
            "/api/auth/connect-devices/revoke",
            post(handle_revoke_connect_device),
        )
        .route("/api/projects", get(handle_get_projects))
        .route("/api/projects/mutate", post(handle_mutate_project))
        .route(
            "/api/scm/pr",
            get(handle_get_scm_pr).post(handle_post_scm_pr),
        )
        .route(
            "/api/scm/diff",
            get(handle_get_scm_diff).post(handle_post_scm_diff),
        )
        .route("/api/peers", get(handle_list_peers).post(handle_link_peer))
        .route("/api/peers/{id}", delete(handle_unlink_peer))
        .route("/api/providers/probe", post(handle_provider_probe))
        .route("/api/preview/action", post(handle_preview_action))
        .route("/api/observability/v1/traces", post(handle_otlp_traces))
        .with_state(state)
}

/// Build a full server router combining [`runtime_api_router`] and [`static_assets_router`].
pub fn runtime_full_router(state: Arc<RuntimeHttpState>) -> Router {
    let static_dir = state.static_dir.clone();
    runtime_api_router(state).fallback_service(static_assets_router(static_dir))
}
