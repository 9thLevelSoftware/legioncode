//! Pure Safe Rust support-bundle writer for `t3 triage` (`apps/server/src/triage/triageSession.ts`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::ownership::{
    OwnershipError, format_epoch_millis_utc, is_process_alive, read_persisted_server_runtime_state,
};
use crate::paths::ServerDerivedPaths;
pub use crate::triage_prompt::{
    BOOT_SERVICE_LOG_FILE, RELAY_CLIENT_LOG_FILE, TRIAGE_PLAYBOOK, TRIAGE_PROMPT_FILE_NAME,
    TriageContextInput, TriageEntry, TriagePaths, TriageSourceClone, build_triage_app_prompt,
    build_triage_context, build_triage_launch_prompt, build_triage_seed_prompt,
    serialize_composer_file_link,
};

/// Errors returned while inspecting server state or creating a triage session directory.
#[derive(Debug, Error)]
pub enum TriageError {
    /// Underlying filesystem error creating directories or writing `context.md` / playbook files.
    #[error("triage I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Ownership runtime-state inspection error.
    #[error("triage server-state inspection error: {0}")]
    Ownership(#[from] OwnershipError),
}

/// Input required by [`write_triage_session`].
#[derive(Debug, Clone)]
pub struct WriteTriageSessionInput {
    /// Server base directory (`--base-dir` / T3 home).
    pub base_dir: PathBuf,
    /// Derived runtime directory tree (`paths.state_dir`, `paths.db_path`, etc.).
    pub paths: ServerDerivedPaths,
    /// Entry surface (`Cli` or `App`).
    pub entry: TriageEntry,
    /// Human-readable server liveness summary produced by [`describe_server_process`].
    pub server: String,
    /// Optional server-side source checkout clone state.
    pub source_clone: Option<TriageSourceClone>,
    /// Optional explicit millisecond UTC timestamp (defaults to `SystemTime::now()`).
    pub now_ms: Option<i64>,
    /// Optional explicit OS description string (defaults to Rust `env::consts` tuple).
    pub os_label: Option<String>,
    /// Optional runtime version banner recorded in `context.md`.
    pub runtime_version: Option<String>,
}

/// Files created by [`write_triage_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageSessionFiles {
    /// `<state_dir>/triage/<timestamp>` directory.
    pub session_dir: PathBuf,
    /// `<session_dir>/context.md`.
    pub context_file_path: PathBuf,
    /// `<session_dir>/triage-playbook.md`.
    pub prompt_file_path: PathBuf,
}

/// Returns the source checkout cache directory (`<base_dir>/source`) matching `triageSourceCacheDir`.
pub fn triage_source_cache_dir(base_dir: &Path) -> PathBuf {
    base_dir.join("source")
}

/// Inspects `server-runtime.json` and reports whether the recorded server PID is alive
/// (`triageSession.ts:23-38`).
pub fn describe_server_process(server_runtime_state_path: &Path) -> Result<String, TriageError> {
    let Some(state) = read_persisted_server_runtime_state(server_runtime_state_path)? else {
        return Ok(
            "not running (no server-runtime.json; the server may never have started here)"
                .to_owned(),
        );
    };
    let version = state.server_version.as_deref().map_or_else(
        || "version not recorded".to_owned(),
        |v| format!("version {v}"),
    );
    if !is_process_alive(state.pid) {
        return Ok(format!(
            "not running (state file is stale: pid {} is dead; last origin {}, {version})",
            state.pid, state.origin
        ));
    }
    Ok(format!(
        "running (pid {}, {}, {version})",
        state.pid, state.origin
    ))
}

/// Writes `context.md` and `triage-playbook.md` into `<state_dir>/triage/<timestamp>`
/// (`triageSession.ts:48-102`).
pub fn write_triage_session(
    input: &WriteTriageSessionInput,
) -> Result<TriageSessionFiles, TriageError> {
    let now_ms = input.now_ms.unwrap_or_else(current_epoch_millis);
    let generated_at = format_epoch_millis_utc(now_ms);
    let dir_stamp = generated_at.replace([':', '.'], "-");

    let paths = &input.paths;
    let session_dir = paths.state_dir.join("triage").join(dir_stamp);
    fs::create_dir_all(&session_dir)?;

    let os = input
        .os_label
        .clone()
        .unwrap_or_else(|| format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
    let runtime_version = input
        .runtime_version
        .clone()
        .unwrap_or_else(|| format!("rust-{}", env!("CARGO_PKG_VERSION")));

    let context_file_path = session_dir.join("context.md");
    let context_md = build_triage_context(&TriageContextInput {
        generated_at,
        entry: input.entry.clone(),
        os,
        runtime_version,
        server: input.server.clone(),
        source_clone: input.source_clone.clone(),
        paths: TriagePaths {
            state_dir: paths.state_dir.display().to_string(),
            db_path: paths.db_path.display().to_string(),
            settings_path: paths.settings_path.display().to_string(),
            logs_dir: paths.logs_dir.display().to_string(),
            service_log_path: paths
                .logs_dir
                .join(BOOT_SERVICE_LOG_FILE)
                .display()
                .to_string(),
            desktop_backend_log_glob: paths
                .logs_dir
                .join("server-child*.log*")
                .display()
                .to_string(),
            relay_client_log_path: paths
                .logs_dir
                .join(RELAY_CLIENT_LOG_FILE)
                .display()
                .to_string(),
            server_trace_path: paths.server_trace_path.display().to_string(),
            provider_event_log_path: paths.provider_event_log_path.display().to_string(),
            terminal_logs_dir: paths.terminal_logs_dir.display().to_string(),
            provider_status_cache_dir: paths.provider_status_cache_dir.display().to_string(),
            secrets_dir: paths.secrets_dir.display().to_string(),
            source_cache_dir: triage_source_cache_dir(&input.base_dir)
                .display()
                .to_string(),
        },
    });
    fs::write(&context_file_path, context_md)?;

    let prompt_file_path = session_dir.join(TRIAGE_PROMPT_FILE_NAME);
    let seed_prompt =
        build_triage_seed_prompt(&context_file_path.display().to_string(), &input.entry);
    fs::write(&prompt_file_path, seed_prompt)?;

    Ok(TriageSessionFiles {
        session_dir,
        context_file_path,
        prompt_file_path,
    })
}

fn current_epoch_millis() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(0)
}
