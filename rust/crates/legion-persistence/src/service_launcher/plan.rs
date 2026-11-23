//! Launcher startup planning, runtime sentinel checks, and preflight integration.

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::ServerDerivedPaths;
use crate::service_preflight::{ServicePreflightResult, run_service_preflight};

use super::state::{ServiceLauncherContext, ServiceState, ServiceUpdateRecord};
use super::{SERVICE_LAUNCHER_PROTOCOL, ServiceLauncherError};

/// Child role launched by the service supervisor (`serviceLauncher.ts:67`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherChildRole {
    /// Normal active server runtime.
    Active,
    /// Candidate target runtime started during a pending self-update trial.
    Trial,
}

/// Concrete child spawn plan produced by [`plan_launcher_startup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherChildPlan {
    /// Active vs trial child role.
    pub role: LauncherChildRole,
    /// Selected child runtime version string.
    pub version: String,
    /// Executable path inside `<base_dir>/runtime/versions/<version>`.
    pub command: PathBuf,
    /// Arguments passed to the runtime executable (`["serve"]`).
    pub args: Vec<String>,
    /// SQLite database path validated by preflight.
    pub db_path: PathBuf,
    /// Decodable `LEGIONCODE_SERVICE_LAUNCHER_CONTEXT` payload.
    pub context: ServiceLauncherContext,
    /// Serialized JSON string for the `LEGIONCODE_SERVICE_LAUNCHER_CONTEXT` environment variable.
    pub context_json: String,
}

/// Outcome of planning launcher startup for a [`ServiceState`] (`serviceLauncher.ts:1120-1188`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherStartupDecision {
    /// Runtime and database preflight succeeded; spawn the planned child.
    Launch(Box<LauncherChildPlan>),
    /// A pending update must roll back to `previous_version` before starting.
    RollbackRequired {
        /// Update identifier from [`ServiceUpdateRecord::Pending`].
        update_id: String,
        /// Baseline version to revert to.
        previous_version: String,
        /// Failure marker (`"rollback-interrupted"` or `"target-runtime-missing"`).
        reason: &'static str,
    },
    /// Database/protocol preflight or active runtime verification blocked startup.
    Blocked {
        /// Target runtime version checked.
        version: String,
        /// Human-readable blocking reason.
        reason: String,
    },
}

/// Returns `<base_dir>/runtime/versions/<version>/<legioncode|legioncode.exe>` and `.install-complete` sentinel.
pub fn runtime_entry_paths(base_dir: &Path, version: &str) -> (PathBuf, PathBuf) {
    let version_dir = base_dir.join("runtime").join("versions").join(version);
    let executable = if cfg!(windows) {
        "legioncode.exe"
    } else {
        "legioncode"
    };
    (
        version_dir.join(executable),
        version_dir.join(".install-complete"),
    )
}

/// Returns `true` when `version` is installed under `base_dir` with a valid executable file and
/// matching `.install-complete` sentinel (`serviceLauncher.ts:693-704`).
pub fn runtime_exists(base_dir: &Path, version: &str) -> bool {
    let (entry_path, sentinel_path) = runtime_entry_paths(base_dir, version);
    let Ok(meta) = fs::metadata(&entry_path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let Ok(sentinel) = fs::read_to_string(&sentinel_path) else {
        return false;
    };
    sentinel.trim() == version
}

/// Plans launcher startup for `base_dir` and `state`: handles pending update restore/runtime guards,
/// runs [`run_service_preflight`], and constructs the child launch plan + `LEGIONCODE_SERVICE_LAUNCHER_CONTEXT`.
pub fn plan_launcher_startup(
    base_dir: &Path,
    state: &ServiceState,
) -> Result<LauncherStartupDecision, ServiceLauncherError> {
    let (role, version, db_path) = if let Some(ServiceUpdateRecord::Pending {
        id,
        from_version,
        target_version,
        db_path,
    }) = &state.update
    {
        let restore_pending = base_dir
            .join("runtime")
            .join("db-backup")
            .join(id)
            .join(".restore-pending")
            .exists();
        if restore_pending {
            return Ok(LauncherStartupDecision::RollbackRequired {
                update_id: id.clone(),
                previous_version: from_version.clone(),
                reason: "rollback-interrupted",
            });
        }
        if !runtime_exists(base_dir, target_version) {
            return Ok(LauncherStartupDecision::RollbackRequired {
                update_id: id.clone(),
                previous_version: from_version.clone(),
                reason: "target-runtime-missing",
            });
        }
        (
            LauncherChildRole::Trial,
            target_version.clone(),
            PathBuf::from(db_path),
        )
    } else {
        let derived = ServerDerivedPaths::derive(base_dir, false, true);
        (
            LauncherChildRole::Active,
            state.active_version.clone(),
            derived.db_path,
        )
    };

    if !runtime_exists(base_dir, &version) {
        return Ok(LauncherStartupDecision::Blocked {
            reason: format!("Selected legioncode@{version} runtime is missing or incomplete."),
            version,
        });
    }

    let protocol = if state.protocol == 0 {
        SERVICE_LAUNCHER_PROTOCOL
    } else {
        state.protocol
    };
    if let ServicePreflightResult::Blocked {
        version: blocked_v,
        reason,
    } = run_service_preflight(&db_path, protocol, &version)
    {
        return Ok(LauncherStartupDecision::Blocked {
            version: blocked_v,
            reason,
        });
    }

    let (command, _) = runtime_entry_paths(base_dir, &version);
    let context = ServiceLauncherContext {
        protocol,
        child_version: version.clone(),
        update: state.update.clone(),
    };
    let context_json =
        serde_json::to_string(&context).map_err(|source| ServiceLauncherError::Json {
            operation: "serialize_launcher_context",
            path: base_dir.to_path_buf(),
            source,
        })?;

    Ok(LauncherStartupDecision::Launch(Box::new(
        LauncherChildPlan {
            role,
            version,
            command,
            args: vec!["serve".to_owned()],
            db_path,
            context,
            context_json,
        },
    )))
}
