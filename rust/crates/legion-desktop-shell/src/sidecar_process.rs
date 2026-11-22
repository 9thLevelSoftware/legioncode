//! Managed local `legioncode serve` sidecar spawn, loopback readiness polling,
//! ephemeral pairing bootstrap tokens, and kernel job supervision.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use legion_contracts::brand::ACTIVE_BRAND;
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, IssuePairingTokenInput, issue_pairing_token,
};
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::service_preflight::ServicePreflightResult;
use legion_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken};
use sha2::{Digest, Sha256};

use crate::http_loopback::http_request;
use crate::sidecar::{SidecarController, build_spawn_spec};
use crate::sidecar_discovery::{SidecarConfig, SidecarError, SidecarMode};

static EPHEMERAL_TOKEN_SEQ: AtomicU64 = AtomicU64::new(1);

const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const READINESS_HTTP_TIMEOUT: Duration = Duration::from_millis(400);
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// Active supervised local `legioncode serve` sidecar owned by the desktop window.
#[derive(Debug)]
pub struct ManagedDesktopSidecar {
    /// Configured controller used to launch this sidecar.
    pub controller: SidecarController,
    /// Process-tree supervisor (Win32 `JobHandle` on Windows; child reaper on Unix).
    pub supervisor: JobProcessSupervisor,
    /// Supervisor process token.
    pub token: ProcessToken,
    /// Sidecar OS process identifier if still attached.
    pub pid: Option<u32>,
    /// Bound loopback TCP port (`127.0.0.1:<port>`).
    pub port: u16,
    /// Loopback HTTP base URL (`http://127.0.0.1:<port>`).
    pub http_base_url: String,
    /// Loopback WebSocket base URL (`ws://127.0.0.1:<port>/ws`).
    pub ws_base_url: String,
    /// Ephemeral administrative bootstrap token persisted in `statev2.sqlite`.
    pub bootstrap_token: String,
    /// Derived runtime paths for `<legionHome>`.
    pub paths: ServerDerivedPaths,
    output_tail: Arc<Mutex<String>>,
}

impl Drop for ManagedDesktopSidecar {
    fn drop(&mut self) {
        let _ = self.supervisor.terminate_tree(0);
    }
}

impl ManagedDesktopSidecar {
    /// Returns captured stdout/stderr tail for failure diagnostics.
    #[must_use]
    pub fn captured_output(&self) -> String {
        self.output_tail
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

/// Allocates an ephemeral unused TCP port on `127.0.0.1`.
pub fn pick_unused_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| format!("failed binding ephemeral loopback port: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| format!("failed reading bound loopback port: {err}"))?;
    Ok(addr.port())
}

/// Returns deterministic CLI arguments for desktop-managed `legioncode serve`.
#[must_use]
pub fn desktop_serve_launch_args(port: u16, base_dir: &Path) -> Vec<String> {
    vec![
        "serve".to_owned(),
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        port.to_string(),
        "--base-dir".to_owned(),
        base_dir.to_string_lossy().into_owned(),
        "--mode".to_owned(),
        "desktop".to_owned(),
        "--no-browser".to_owned(),
    ]
}

/// Generates a high-entropy 32-character hex token without external dependencies.
#[must_use]
pub fn generate_ephemeral_hex_token() -> String {
    let seq = EPHEMERAL_TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u128, |d| d.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(seq.to_le_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// Mints a short-lived pairing bootstrap credential directly inside `<state_dir>/statev2.sqlite`.
pub fn mint_desktop_pairing_token(db_path: &Path, label: &str) -> Result<String, String> {
    let now_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0_u128, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX);
    let input = IssuePairingTokenInput::default()
        .with_scopes(ADMINISTRATIVE_CLIENT_SCOPES.iter().copied())
        .with_label(label);
    let record = issue_pairing_token(db_path, &input, now_ms)
        .map_err(|err| format!("failed minting desktop bootstrap token: {err}"))?;
    Ok(record.credential)
}

/// Resolves an explicit or bundled static web client asset directory if available on disk.
#[must_use]
pub fn discover_static_web_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit
        && dir.join("index.html").is_file()
    {
        return Some(dir.to_path_buf());
    }
    if let Some(env_dir) = ACTIVE_BRAND
        .read_env("STATIC_DIR")
        .map(PathBuf::from)
        .filter(|d| d.join("index.html").is_file())
    {
        return Some(env_dir);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        for candidate in [
            exe_dir.join("web-app"),
            exe_dir.join("client"),
            exe_dir
                .parent()
                .map_or_else(|| exe_dir.join("web-app"), |parent| parent.join("web-app")),
        ] {
            if candidate.join("index.html").is_file() {
                return Some(candidate);
            }
        }
        for ancestor in exe_dir.ancestors().take(6) {
            for rel in ["apps/web/dist", "apps/server/dist/client"] {
                let candidate = ancestor.join(rel);
                if candidate.join("index.html").is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Launches the local `legioncode serve` sidecar, assigns it to `JobProcessSupervisor`,
/// mints a pairing bootstrap token, and waits for `/.well-known/legioncode/environment`.
pub fn start_managed_desktop_sidecar(
    base_dir: &Path,
    base_dir_is_explicit: bool,
    app_version: &str,
    explicit_static_dir: Option<&Path>,
) -> Result<ManagedDesktopSidecar, String> {
    let port = pick_unused_loopback_port()?;
    let paths = ServerDerivedPaths::derive(base_dir, false, base_dir_is_explicit);
    ensure_server_directories(&paths)
        .map_err(|err| format!("failed preparing sidecar directories: {err}"))?;

    let mut extra_env = BTreeMap::new();
    if let Some(static_dir) = discover_static_web_dir(explicit_static_dir) {
        extra_env.insert(
            format!("{}STATIC_DIR", ACTIVE_BRAND.primary_env_prefix),
            static_dir.to_string_lossy().into_owned(),
        );
    }
    let controller = SidecarController::new(SidecarConfig {
        base_dir: base_dir.to_path_buf(),
        mode: SidecarMode::Production,
        base_dir_is_explicit,
        app_version: app_version.to_owned(),
        host: "127.0.0.1".to_owned(),
        port,
        no_browser: true,
        bootstrap_fd: None,
        extra_env,
        ..SidecarConfig::default()
    });

    if let ServicePreflightResult::Blocked { reason, .. } = controller.run_preflight() {
        return Err(reason);
    }

    let bootstrap_token = mint_desktop_pairing_token(&paths.db_path, "LegionCode Desktop")?;
    let discovery = controller.discover_runtime();
    if !discovery.found {
        return Err(SidecarError::RuntimeNotFound {
            candidates: discovery.candidates_checked,
        }
        .to_string());
    }

    let args = desktop_serve_launch_args(port, base_dir);
    let sanitized_env = controller.sanitize_host_env();
    let spec = build_spawn_spec(
        &discovery.binary_path,
        &args,
        base_dir,
        sanitized_env,
        controller.config().windows_no_window,
    );

    let mut supervisor =
        JobProcessSupervisor::new().map_err(|err| format!("supervisor init failed: {err}"))?;
    let (token, mut pipes) = supervisor
        .spawn(&spec)
        .map_err(|err| format!("failed spawning legioncode sidecar: {err}"))?;
    let pid = supervisor.pid(token);

    let output_tail = Arc::new(Mutex::new(String::new()));
    if let Some(stdout) = pipes.take_stdout() {
        spawn_pipe_drain(stdout, output_tail.clone());
    }
    if let Some(stderr) = pipes.take_stderr() {
        spawn_pipe_drain(stderr, output_tail.clone());
    }

    let http_base_url = format!("http://127.0.0.1:{port}");
    let ws_base_url = format!("ws://127.0.0.1:{port}/ws");
    let mut managed = ManagedDesktopSidecar {
        controller,
        supervisor,
        token,
        pid,
        port,
        http_base_url: http_base_url.clone(),
        ws_base_url,
        bootstrap_token,
        paths,
        output_tail,
    };

    poll_environment_ready(&mut managed, &http_base_url, DEFAULT_READINESS_TIMEOUT)?;
    Ok(managed)
}

/// Polls `<http_base_url>/.well-known/legioncode/environment` until `200 OK`, timeout, or child exit.
/// Polls `<http_base_url>/.well-known/legioncode/environment` until `200 OK`, timeout, or child exit.
pub fn poll_environment_ready(
    managed: &mut ManagedDesktopSidecar,
    http_base_url: &str,
    timeout: Duration,
) -> Result<(), String> {
    let token = managed.token;
    let output_tail = Arc::clone(&managed.output_tail);
    let supervisor = &mut managed.supervisor;
    wait_until_environment_ready(http_base_url, timeout, move || {
        match supervisor.try_wait(token) {
            Ok(Some(status)) => {
                let tail = output_tail.lock().map(|g| g.clone()).unwrap_or_default();
                Err(format!(
                    "legioncode sidecar exited prematurely with {status} before becoming ready\n{tail}"
                ))
            }
            Ok(None) => Ok(()),
            Err(err) => Err(format!("sidecar wait error: {err}")),
        }
    })
    .map_err(|err| format!("{err}\n{}", managed.captured_output()))
}

/// Core polling loop for `<http_base_url>/.well-known/legioncode/environment`.
pub fn wait_until_environment_ready(
    http_base_url: &str,
    timeout: Duration,
    mut check_alive: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let url = format!(
        "{}/.well-known/legioncode/environment",
        http_base_url.trim_end_matches('/')
    );
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        check_alive()?;
        match http_request("GET", &url, &[], None, READINESS_HTTP_TIMEOUT) {
            Ok(resp) if (200..300).contains(&resp.status) => return Ok(()),
            Ok(resp) => last_error = format!("HTTP {}", resp.status),
            Err(err) => last_error = err,
        }
        thread::sleep(READINESS_POLL_INTERVAL);
    }
    Err(format!(
        "timed out waiting for `{url}` after {}s (last error: {last_error})",
        timeout.as_secs()
    ))
}

fn spawn_pipe_drain<R: Read + Send + 'static>(mut reader: R, sink: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    if let Ok(mut guard) = sink.lock() {
                        guard.push_str(&chunk);
                        if guard.len() > MAX_CAPTURE_BYTES {
                            let trim_at = guard.len() - MAX_CAPTURE_BYTES;
                            let aligned = guard
                                .char_indices()
                                .find(|&(idx, _)| idx >= trim_at)
                                .map_or(guard.len(), |(idx, _)| idx);
                            guard.drain(..aligned);
                        }
                    }
                }
            }
        }
    });
}
