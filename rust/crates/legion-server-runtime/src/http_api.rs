//! Shared [`RuntimeHttpState`] and composable Axum routers for `legioncode serve`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Router;
use legion_contracts::policy::ManagedPolicySummary;
use legion_orchestrator::Orchestrator;
use legion_remote::secrets::FileSecretStore;
use rusqlite::Connection;

use crate::error::ServerRuntimeError;
use crate::http_api_routes::build_runtime_api_router;
use crate::http_mcp_signin::McpSignInRegistry;
use crate::http_projects_read::ProjectSettingsOverride;
use crate::preview_browser::PreviewBrowserRuntime;
use crate::provider_probe::ProviderProbeService;
use crate::static_assets::static_assets_router;
use crate::terminal_runtime::TerminalRuntime;

/// Shared server HTTP state mounted by `legioncode serve` or standalone integration tests.
#[derive(Clone)]
pub struct RuntimeHttpState {
    db: Arc<StdMutex<Connection>>,
    /// Session cookie name checked by `BUG-03` auth precedence and set on pairing.
    pub cookie_name: String,
    /// When `true`, protected endpoints require a verified pairing session or `dev_auth_token`.
    pub require_auth: bool,
    /// Optional fixed dev token permitted in `Authorization` or `legioncode_dev_auth` cookie.
    pub dev_auth_token: Option<String>,
    /// Directory where `traces.ndjson` and runtime logs are written.
    pub logs_dir: PathBuf,
    /// Directory where encrypted/binary server secrets (e.g. peer link tokens) are stored.
    pub secrets_dir: PathBuf,
    /// Built frontend dist directory served via [`static_assets_router`].
    pub static_dir: Option<PathBuf>,
    /// Allowed canonical workspace root prefixes for `/api/assets/workspace` symlink checks (`#39491`).
    pub allowed_workspace_roots: Vec<PathBuf>,
    /// Server-side `enablePreviewTunnel` setting gated with [`RuntimeHttpState::managed_policy`] (`#39415`).
    pub enable_preview_tunnel: bool,
    /// Optional managed enterprise policy summary governing preview tunnel permissions (`#39415`).
    pub managed_policy: Option<ManagedPolicySummary>,
    /// Whether peer links / transitive pairing and thread relays are enabled on this environment.
    pub linked_environments_enabled: bool,
    /// Optional live orchestrator reference.
    pub orchestrator: Option<Arc<Orchestrator>>,
    /// Optional live terminal runtime reference.
    pub terminal_runtime: Option<Arc<TerminalRuntime>>,
    /// Provider and speech endpoint health-probe service.
    pub probe_service: Arc<ProviderProbeService>,
    /// Headless browser CDP host runtime for `/api/preview/action`.
    pub preview_browser: Arc<PreviewBrowserRuntime>,
    /// Registered MCP server OAuth sign-in flows (`#39921`).
    pub mcp_sign_in: Arc<McpSignInRegistry>,
    /// Per-project saved Settings overrides (`#39676`, `#35925`).
    pub project_settings_overrides: Arc<StdMutex<BTreeMap<String, ProjectSettingsOverride>>>,
    access_logs: Arc<StdMutex<Vec<String>>>,
    trace_write_lock: Arc<StdMutex<()>>,
}

impl RuntimeHttpState {
    /// Construct a new [`RuntimeHttpState`] backed by `db` and writing traces to `logs_dir/traces.ndjson`.
    pub fn new(db: Arc<StdMutex<Connection>>, logs_dir: impl Into<PathBuf>) -> Self {
        let logs = logs_dir.into();
        let secrets = logs.join("secrets");
        Self {
            db,
            cookie_name: "legioncode_session".to_owned(),
            require_auth: true,
            dev_auth_token: None,
            logs_dir: logs,
            secrets_dir: secrets,
            static_dir: None,
            allowed_workspace_roots: Vec::new(),
            enable_preview_tunnel: true,
            managed_policy: None,
            linked_environments_enabled: true,
            orchestrator: None,
            terminal_runtime: None,
            probe_service: Arc::new(ProviderProbeService::default()),
            preview_browser: Arc::new(PreviewBrowserRuntime::new()),
            mcp_sign_in: Arc::new(McpSignInRegistry::new()),
            project_settings_overrides: Arc::new(StdMutex::new(BTreeMap::new())),
            access_logs: Arc::new(StdMutex::new(Vec::new())),
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

    /// Override the primary session cookie name (defaults to `"legioncode_session"`).
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

    /// Configure preview tunnel setting and optional managed policy override (`#39415`).
    #[must_use]
    pub fn with_preview_tunnel_policy(
        mut self,
        enable_preview_tunnel: bool,
        managed_policy: Option<ManagedPolicySummary>,
    ) -> Self {
        self.enable_preview_tunnel = enable_preview_tunnel;
        self.managed_policy = managed_policy;
        self
    }

    /// Configure allowed workspace roots for `/api/assets/workspace` validation (`#39491`).
    #[must_use]
    pub fn with_allowed_workspace_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.allowed_workspace_roots = roots;
        self
    }

    /// Enable or disable peer thread relay & transitive pairing (`#39406`, `#40036`).
    #[must_use]
    pub const fn with_linked_environments_enabled(mut self, enabled: bool) -> Self {
        self.linked_environments_enabled = enabled;
        self
    }

    /// Register saved Settings overrides (`defaultProjectScripts`, `defaultModelSelection`, `trackedFolders`)
    /// for a project (`#39676`, `#35925`).
    pub fn set_project_settings_override(
        &self,
        project_id: impl Into<String>,
        cfg: ProjectSettingsOverride,
    ) {
        if let Ok(mut guard) = self.project_settings_overrides.lock() {
            guard.insert(project_id.into(), cfg);
        }
    }

    pub(crate) fn record_access_log(&self, line: String) {
        if let Ok(mut guard) = self.access_logs.lock() {
            guard.push(line);
        }
    }

    /// Returns recorded redacted HTTP access log lines (`#40133`).
    pub fn access_logs(&self) -> Vec<String> {
        self.access_logs.lock().ok().map_or_default(|g| g.clone())
    }
}

/// Build the API routes (`/api/auth/*`, `/oauth/token`, `/api/projects*`, `/api/scm/*`,
/// `/api/peers*`, `/api/links/*`, `/api/workspace/*`, `/api/mcp/sign-in/*`, `/api/preview/*`,
/// `/api/providers/probe`, `/api/assets/workspace`, `/api/observability/v1/traces`).
pub fn runtime_api_router(state: Arc<RuntimeHttpState>) -> Router {
    build_runtime_api_router(state)
}

/// Build a full server router combining [`runtime_api_router`] and [`static_assets_router`].
pub fn runtime_full_router(state: Arc<RuntimeHttpState>) -> Router {
    let static_dir = state.static_dir.clone();
    runtime_api_router(state).fallback_service(static_assets_router(static_dir))
}
