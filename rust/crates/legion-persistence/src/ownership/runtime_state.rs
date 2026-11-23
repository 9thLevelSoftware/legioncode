//! Persisted `server-runtime.json` schema and atomic read/write/clear operations.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use super::{OwnershipError, format_iso8601_utc, is_process_alive};

/// Managed enterprise policy digest published inside `server-runtime.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicyRecord {
    /// Policy source identifier.
    pub source: String,
    /// Organization identifier.
    pub organization: String,
    /// 12-hex policy fingerprint.
    pub fingerprint: String,
}

/// Tri-state representation of `managedPolicy` in `server-runtime.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ManagedPolicyState {
    /// Field is omitted (`undefined` in TypeScript).
    #[default]
    Unspecified,
    /// Field is explicitly `null`.
    Null,
    /// Field carries a loaded managed policy summary.
    Loaded(ManagedPolicyRecord),
}

impl ManagedPolicyState {
    const fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

impl Serialize for ManagedPolicyState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unspecified | Self::Null => serializer.serialize_none(),
            Self::Loaded(record) => record.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ManagedPolicyState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Option::<ManagedPolicyRecord>::deserialize(deserializer)? {
            None => Ok(Self::Null),
            Some(record) => Ok(Self::Loaded(record)),
        }
    }
}

/// Wire-compatible representation of `PersistedServerRuntimeState` (`serverRuntimeState.ts:12-35`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedServerRuntimeState {
    /// Schema version (always `1`).
    pub version: u32,
    /// Server OS process ID.
    pub pid: i64,
    /// Unique lock-acquisition UUID stamped by [`super::ServerOwnerLock::publish`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Configured bind host, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// True when host defaulted to a private-network listener.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_network_only: Option<bool>,
    /// Bound HTTP/WebSocket port.
    pub port: u16,
    /// Reachable local origin URL (e.g. `http://127.0.0.1:3773`).
    pub origin: String,
    /// Vite dev server URL when running in dev mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_url: Option<String>,
    /// ISO-8601 UTC start timestamp.
    pub started_at: String,
    /// Whether this server is managed by the boot service launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_managed: Option<bool>,
    /// Server package version string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    /// Loaded managed policy (`Unspecified` omitted, `Null` explicit `null`, `Loaded` object).
    #[serde(default, skip_serializing_if = "ManagedPolicyState::is_unspecified")]
    pub managed_policy: ManagedPolicyState,
}

impl PersistedServerRuntimeState {
    /// Builds a v1 runtime state snapshot for the current process at `SystemTime::now()`.
    pub fn for_current_process(port: u16, host: Option<&str>) -> Self {
        Self {
            version: 1,
            pid: i64::from(std::process::id()),
            owner_id: None,
            host: host.filter(|h| !h.is_empty()).map(str::to_owned),
            private_network_only: None,
            port,
            origin: runtime_origin_for_host(host, port),
            dev_url: None,
            started_at: format_iso8601_utc(SystemTime::now()),
            service_managed: None,
            server_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            managed_policy: ManagedPolicyState::Unspecified,
        }
    }
}

/// Computes `origin` from `host` and `port`, matching `serverRuntimeState.ts:55-62`.
pub fn runtime_origin_for_host(host: Option<&str>, port: u16) -> String {
    let hostname = match host {
        Some(h) if !h.is_empty() && !matches!(h, "0.0.0.0" | "::" | "[::]") => {
            if h.contains(':') && !h.starts_with('[') {
                format!("[{h}]")
            } else {
                h.to_owned()
            }
        }
        _ => "127.0.0.1".to_owned(),
    };
    format!("http://{hostname}:{port}")
}

/// Reads and validates `server-runtime.json`, returning `Ok(None)` if missing or empty.
pub fn read_persisted_server_runtime_state(
    path: impl AsRef<Path>,
) -> Result<Option<PersistedServerRuntimeState>, OwnershipError> {
    let path = path.as_ref();
    let raw = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(OwnershipError::RuntimeState {
                operation: "read",
                path: path.to_path_buf(),
                message: err.to_string(),
            });
        }
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let state: PersistedServerRuntimeState =
        serde_json::from_str(trimmed).map_err(|err| OwnershipError::RuntimeState {
            operation: "decode",
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    if state.version != 1 {
        return Err(OwnershipError::RuntimeState {
            operation: "decode",
            path: path.to_path_buf(),
            message: format!("unsupported runtime state version: {}", state.version),
        });
    }
    Ok(Some(state))
}

/// Reads `server-runtime.json` only if present, valid, its recorded PID is alive, and—when
/// lock-aware (`owner_id` set)—the ownership lock is actively held (`#28256`).
pub fn read_live_server_runtime_state(
    path: impl AsRef<Path>,
) -> Option<PersistedServerRuntimeState> {
    let path = path.as_ref();
    let state = read_persisted_server_runtime_state(path).ok().flatten()?;
    if !is_process_alive(state.pid) {
        return None;
    }
    if state.owner_id.is_some() && !super::is_server_ownership_held(path) {
        return None;
    }
    Some(state)
}

/// Reads `server-runtime.json` when another live process currently owns this server runtime
/// (`pid > 0 && pid != current_pid`, alive, and lock held when `owner_id` is set, `#28256`).
pub fn read_contending_server_runtime_state(
    path: impl AsRef<Path>,
) -> Option<PersistedServerRuntimeState> {
    let path = path.as_ref();
    let state = read_persisted_server_runtime_state(path).ok().flatten()?;
    if state.pid <= 0 || state.pid == i64::from(std::process::id()) || !is_process_alive(state.pid)
    {
        return None;
    }
    if state.owner_id.is_some() && !super::is_server_ownership_held(path) {
        return None;
    }
    Some(state)
}

/// Atomically writes `server-runtime.json` via a sibling temp file, `sync_all`, and rename.
pub fn persist_server_runtime_state(
    path: impl AsRef<Path>,
    state: &PersistedServerRuntimeState,
) -> Result<(), OwnershipError> {
    let path = path.as_ref();
    let parent = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).map_err(|err| OwnershipError::RuntimeState {
        operation: "persist",
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    let mut contents =
        serde_json::to_string(state).map_err(|err| OwnershipError::RuntimeState {
            operation: "persist",
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    contents.push('\n');

    let temp_path = parent.join(format!(".server-runtime-{}.tmp", Uuid::new_v4().simple()));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(OwnershipError::RuntimeState {
            operation: "persist",
            path: path.to_path_buf(),
            message: err.to_string(),
        });
    }
    Ok(())
}

/// Removes `server-runtime.json` if present, ignoring `NotFound`.
pub fn clear_persisted_server_runtime_state(path: impl AsRef<Path>) -> Result<(), OwnershipError> {
    let path = path.as_ref();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(OwnershipError::RuntimeState {
            operation: "clear",
            path: path.to_path_buf(),
            message: err.to_string(),
        }),
    }
}
