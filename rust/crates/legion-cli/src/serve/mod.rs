//! HTTP, WebSocket, and MCP server lifecycle (`legioncode serve`) and `spawn_server` test harness.

pub mod cli_entry;
pub mod cli_flags;
pub mod config;
mod init;
pub mod mcp;
mod mcp_milestones;
mod mcp_tools;
mod mcp_wf_feedback;
pub(crate) mod rpc;
mod rpc_terminal;
pub mod rpc_wave_d;
pub mod rpc_wave_h;
mod startup_repair;
pub mod surface_routes;
pub mod voice_ws;
pub mod ws_mcp;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{any, get};
use axum::serve::ListenerExt as _;
use axum::{Json, Router};
use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use legion_contracts::ids::EnvironmentId;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::outbox::{EffectOutboxWorker, OutboxWorkerOptions};
use legion_persistence::ownership::{PersistedServerRuntimeState, ServerOwnerLock};
use legion_persistence::paths::{
    ServerDerivedPaths, ensure_environment_id, ensure_server_directories,
};
use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore};
use legion_persistence::{CancellationToken, read_migration_head};
use legion_protocol::milestones::MilestoneHub;
use legion_protocol::rpc_codec::OutputWindowRegistry;
use legion_server_runtime::{
    RuntimeHttpState, RuntimeOutboxExecutor, TerminalRuntime, runtime_api_router,
    static_assets_router,
};
use legion_workflows::store::WorkflowStore;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use config::{DEFAULT_PORT, ServeConfig, ServeError, ServeMode, UNIT_SERVE};
use config::{build_descriptor, format_bind_addr};
use init::{build_runtime_http_state, init_workflow_and_bg_tasks};

pub(crate) struct AppState {
    pub descriptor: ExecutionEnvironmentDescriptor,
    pub orchestrator: Arc<Orchestrator>,
    pub runtime_http: Arc<RuntimeHttpState>,
    pub terminal_runtime: Arc<TerminalRuntime>,
    pub workflow_store: Option<WorkflowStore>,
    pub paths: ServerDerivedPaths,
    pub next_client_id: AtomicU64,
    pub live_clients: Mutex<HashSet<u64>>,
    pub output_windows: Mutex<OutputWindowRegistry>,
}

/// Live server handle returned by [`spawn_server`].
pub struct RunningServer {
    addr: SocketAddr,
    origin: String,
    paths: ServerDerivedPaths,
    environment_id: EnvironmentId,
    orchestrator: Arc<Orchestrator>,
    terminal_runtime: Arc<TerminalRuntime>,
    outbox_worker: Option<EffectOutboxWorker>,
    fence: CancellationToken,
    owner_lock: Option<ServerOwnerLock>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), std::io::Error>>>,
    bg_tasks: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("addr", &self.addr)
            .field("origin", &self.origin)
            .field("environment_id", &self.environment_id)
            .finish_non_exhaustive()
    }
}

impl RunningServer {
    /// Bound socket address (useful when `config.port == 0`).
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Bound TCP port.
    pub const fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Reachable HTTP origin string (for example `http://127.0.0.1:3773`).
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Derived paths under the server's `base_dir`.
    pub const fn paths(&self) -> &ServerDerivedPaths {
        &self.paths
    }

    /// Persistent server [`EnvironmentId`].
    pub const fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    /// Orchestrator wired to `statev2.sqlite`.
    pub fn orchestrator(&self) -> &Orchestrator {
        &self.orchestrator
    }

    /// Background outbox worker draining turns, checkpoints, and rollbacks.
    pub const fn outbox_worker(&self) -> Option<&EffectOutboxWorker> {
        self.outbox_worker.as_ref()
    }

    /// Shared live terminal session runtime.
    pub const fn terminal_runtime(&self) -> &Arc<TerminalRuntime> {
        &self.terminal_runtime
    }

    /// Cancellation fence triggered if the ownership lock is deleted or replaced.
    pub fn lock_fence(&self) -> CancellationToken {
        self.fence.clone()
    }

    /// Gracefully stops the HTTP listener, drains and stops the outbox worker, releases the
    /// state-dir lock, and cleans `server-runtime.json`.
    pub async fn shutdown(mut self) -> Result<(), ServeError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|err| ServeError::ServerTask(err.to_string()))?
                .map_err(|err| ServeError::ServerTask(err.to_string()))?;
        }
        for handle in self.bg_tasks.drain(..) {
            handle.abort();
        }
        if let Some(worker) = self.outbox_worker.take() {
            worker
                .shutdown()
                .await
                .map_err(|err| ServeError::ServerTask(err.to_string()))?;
        }
        if let Some(lock) = self.owner_lock.take() {
            lock.release()?;
        }
        Ok(())
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        for handle in self.bg_tasks.drain(..) {
            handle.abort();
        }
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Starts the Safe Rust HTTP/WebSocket/MCP server and returns a [`RunningServer`] handle.
pub async fn spawn_server(config: ServeConfig) -> Result<RunningServer, ServeError> {
    spawn_server_with_dev_url(config, None).await
}

/// Starts the Safe Rust HTTP/WebSocket/MCP server with an explicit Vite `--dev-url` override.
pub async fn spawn_server_with_dev_url(
    config: ServeConfig,
    dev_url: Option<String>,
) -> Result<RunningServer, ServeError> {
    let paths = ServerDerivedPaths::derive(&config.base_dir, false, config.base_dir_is_explicit);
    ensure_server_directories(&paths)?;
    let env_id_raw = ensure_environment_id(&paths.environment_id_path)?;
    let environment_id = EnvironmentId::new(env_id_raw)?;

    let owner_lock = ServerOwnerLock::acquire(&paths.server_runtime_state_path)?;
    let fence = owner_lock.fence_token();

    let store = SqliteStore::open(&paths.db_path)?;
    let migration_head = read_migration_head(&paths.db_path)
        .unwrap_or(BUILD_MIGRATION_HEAD)
        .max(BUILD_MIGRATION_HEAD);
    let database_migration = u32::try_from(migration_head).ok();

    let orchestrator = Arc::new(Orchestrator::from_sqlite_store(store));
    startup_repair::repair_startup_state(&orchestrator);
    let descriptor = build_descriptor(environment_id.clone(), &config.cwd, database_migration);

    let bind_host = config
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(if config.lan { "0.0.0.0" } else { "127.0.0.1" });
    let bind_addr = format_bind_addr(bind_host, config.port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|source| ServeError::Bind {
            addr: bind_addr.clone(),
            source,
        })?;
    let local_addr = listener.local_addr().map_err(|source| ServeError::Bind {
        addr: bind_addr,
        source,
    })?;

    let mut runtime_state =
        PersistedServerRuntimeState::for_current_process(local_addr.port(), config.host.as_deref());
    runtime_state.dev_url.clone_from(&dev_url);
    if config.lan && config.host.is_none() {
        runtime_state.private_network_only = Some(true);
    }
    let published = owner_lock.publish(runtime_state)?;

    let outbox_executor = RuntimeOutboxExecutor::new(Arc::clone(&orchestrator));
    outbox_executor.set_settings_path(paths.settings_path.clone());
    outbox_executor.set_secrets_dir(paths.secrets_dir.clone());
    let outbox_worker =
        orchestrator.spawn_outbox_worker(outbox_executor, OutboxWorkerOptions::default());
    let terminal_runtime = Arc::new(TerminalRuntime::default());
    let milestone_hub = Arc::new(MilestoneHub::new());
    let (workflow_store, workflow_manager, bg_tasks) =
        init_workflow_and_bg_tasks(&config, &paths, &orchestrator, &milestone_hub);

    let runtime_http = build_runtime_http_state(
        &config,
        &paths,
        bind_host,
        local_addr.port(),
        &orchestrator,
        &terminal_runtime,
        &milestone_hub,
        workflow_manager.as_ref(),
        dev_url,
    );
    let state = Arc::new(AppState {
        descriptor,
        orchestrator: Arc::clone(&orchestrator),
        runtime_http: Arc::clone(&runtime_http),
        terminal_runtime: Arc::clone(&terminal_runtime),
        workflow_store,
        paths: paths.clone(),
        next_client_id: AtomicU64::new(1),
        live_clients: Mutex::new(HashSet::new()),
        output_windows: Mutex::new(OutputWindowRegistry::new()),
    });
    let router = build_server_router(state, runtime_http, config.static_dir);
    let nodelay_listener = listener.tap_io(|s| {
        let _ = s.set_nodelay(true);
    });
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let fence_wait = fence.clone();
    let task = tokio::spawn(async move {
        axum::serve(nodelay_listener, router)
            .with_graceful_shutdown(async move {
                tokio::select! { _ = shutdown_rx => {} () = fence_wait.cancelled() => {} }
            })
            .await
    });
    Ok(RunningServer {
        addr: local_addr,
        origin: published.origin,
        paths,
        environment_id,
        orchestrator,
        terminal_runtime,
        outbox_worker: Some(outbox_worker),
        fence,
        owner_lock: Some(owner_lock),
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
        bg_tasks,
    })
}

fn build_server_router(
    state: Arc<AppState>,
    runtime_http: Arc<RuntimeHttpState>,
    static_dir: Option<std::path::PathBuf>,
) -> Router {
    Router::new()
        .route(
            "/.well-known/legioncode/environment",
            get(handle_environment),
        )
        .route("/ws", get(ws_mcp::handle_ws))
        .route("/ws/voice", get(voice_ws::handle_voice_ws))
        .route("/mcp", any(ws_mcp::handle_mcp))
        .route("/__surface/app", get(surface_routes::handle_surface_app))
        .route("/__surface/app/", get(surface_routes::handle_surface_app))
        .route(
            "/__surface/app/{*path}",
            get(surface_routes::handle_surface_app),
        )
        .with_state(state)
        .merge(runtime_api_router(runtime_http))
        .fallback_service(static_assets_router(static_dir))
}

async fn handle_environment(
    State(state): State<Arc<AppState>>,
) -> Json<ExecutionEnvironmentDescriptor> {
    Json(state.descriptor.clone())
}

/// CLI runner for `legioncode serve`, handling flags natively and honoring `RUST_ANYTHING_UNIT_SERVE=original`.
pub async fn run(args: &[String]) -> i32 {
    cli_entry::run_entry(cli_flags::ServerEntryKind::Serve, args, args).await
}
