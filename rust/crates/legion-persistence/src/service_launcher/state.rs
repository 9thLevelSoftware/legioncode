//! `ServiceState` JSON codec and validation matching `apps/server/src/cloud/serviceProtocol.ts`.

use std::cmp::Ordering;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::service_preflight::{SERVICE_LAUNCHER_PROTOCOL, is_supported_launcher_protocol};

use super::ServiceLauncherError;
use super::version::{compare_exact_service_versions, is_exact_service_version};

/// Active or terminal self-update record tracked inside [`ServiceState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ServiceUpdateRecord {
    /// Update trial pending activation/commit (`status = "pending"`).
    #[serde(rename_all = "camelCase")]
    Pending {
        /// Unique update identifier.
        id: String,
        /// Baseline version being upgraded from.
        from_version: String,
        /// Candidate target version under trial.
        target_version: String,
        /// Database path passed to backup/preflight steps.
        db_path: String,
    },
    /// Update succeeded and committed (`status = "committed"`).
    #[serde(rename_all = "camelCase")]
    Committed {
        /// Unique update identifier.
        id: String,
        /// Baseline version before the update.
        from_version: String,
        /// Committed active version.
        target_version: String,
        /// Optional outcome note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Update rolled back cleanly (`status = "rolled-back"`).
    #[serde(rename_all = "camelCase")]
    RolledBack {
        /// Unique update identifier.
        id: String,
        /// Retained baseline version.
        from_version: String,
        /// Target version that rolled back.
        target_version: String,
        /// Optional rollback reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Update failed prior to commit (`status = "failed"`).
    #[serde(rename_all = "camelCase")]
    Failed {
        /// Unique update identifier.
        id: String,
        /// Retained baseline version.
        from_version: String,
        /// Target version that failed.
        target_version: String,
        /// Optional failure reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl ServiceUpdateRecord {
    fn id(&self) -> &str {
        match self {
            Self::Pending { id, .. }
            | Self::Committed { id, .. }
            | Self::RolledBack { id, .. }
            | Self::Failed { id, .. } => id,
        }
    }

    fn baseline_version(&self) -> &str {
        match self {
            Self::Pending { from_version, .. }
            | Self::Committed { from_version, .. }
            | Self::RolledBack { from_version, .. }
            | Self::Failed { from_version, .. } => from_version,
        }
    }

    fn target_version(&self) -> &str {
        match self {
            Self::Pending { target_version, .. }
            | Self::Committed { target_version, .. }
            | Self::RolledBack { target_version, .. }
            | Self::Failed { target_version, .. } => target_version,
        }
    }

    fn is_valid_fields(&self) -> bool {
        if self.id().trim().is_empty()
            || !is_exact_service_version(self.baseline_version())
            || !is_exact_service_version(self.target_version())
            || compare_exact_service_versions(self.target_version(), self.baseline_version())
                != Ordering::Greater
        {
            return false;
        }
        match self {
            Self::Pending { db_path, .. } => !db_path.trim().is_empty(),
            Self::Committed { reason, .. }
            | Self::RolledBack { reason, .. }
            | Self::Failed { reason, .. } => {
                reason.as_deref().is_none_or(|text| !text.trim().is_empty())
            }
        }
    }
}

/// Durable service launcher selection record (`serviceProtocol.ts:123-127`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceState {
    /// Service launcher protocol version (written as [`SERVICE_LAUNCHER_PROTOCOL`]).
    pub protocol: i64,
    /// Currently selected active runtime version string.
    pub active_version: String,
    /// Pending or terminal update record, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<ServiceUpdateRecord>,
}

/// Environment payload injected into launched child servers via `LEGIONCODE_SERVICE_LAUNCHER_CONTEXT`
/// (`serviceProtocol.ts:129-133`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceLauncherContext {
    /// Protocol spoken by the launching service process (`3` or `4`).
    pub protocol: i64,
    /// Version of the child runtime being started.
    pub child_version: String,
    /// Pending or terminal update state inherited from [`ServiceState`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<ServiceUpdateRecord>,
}

/// Parses and validates a protocol-3 or protocol-4 [`ServiceState`] JSON string
/// (`serviceProtocol.ts:246-277`), normalizing `protocol` to [`SERVICE_LAUNCHER_PROTOCOL`].
pub fn parse_service_state(raw: &str) -> Option<ServiceState> {
    let mut parsed: ServiceState = serde_json::from_str(raw).ok()?;
    if !is_supported_launcher_protocol(parsed.protocol)
        || !is_exact_service_version(&parsed.active_version)
    {
        return None;
    }
    if let Some(update) = &parsed.update {
        if !update.is_valid_fields() {
            return None;
        }
        let matches_active = match update {
            ServiceUpdateRecord::Pending { from_version, .. }
            | ServiceUpdateRecord::RolledBack { from_version, .. }
            | ServiceUpdateRecord::Failed { from_version, .. } => {
                from_version == &parsed.active_version
            }
            ServiceUpdateRecord::Committed { target_version, .. } => {
                target_version == &parsed.active_version
            }
        };
        if !matches_active {
            return None;
        }
    }
    parsed.protocol = SERVICE_LAUNCHER_PROTOCOL;
    Some(parsed)
}

/// Parses a legacy protocol-2 `{ "protocol": 2, "activeVersion": "..." }` record and upgrades it
/// in memory to [`SERVICE_LAUNCHER_PROTOCOL`] (`serviceProtocol.ts:279-295`).
pub fn parse_legacy_service_state(raw: &str) -> Option<ServiceState> {
    let Value::Object(map) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    if map.len() != 2 || map.get("protocol").and_then(Value::as_i64) != Some(2) {
        return None;
    }
    let active_version = map.get("activeVersion").and_then(Value::as_str)?;
    if !is_exact_service_version(active_version) {
        return None;
    }
    Some(ServiceState {
        protocol: SERVICE_LAUNCHER_PROTOCOL,
        active_version: active_version.to_owned(),
        update: None,
    })
}

/// Parses and validates `LEGIONCODE_SERVICE_LAUNCHER_CONTEXT` JSON (`serviceProtocol.ts:319-350`).
pub fn decode_service_launcher_context(raw: &str) -> Option<ServiceLauncherContext> {
    let parsed: ServiceLauncherContext = serde_json::from_str(raw).ok()?;
    if !is_supported_launcher_protocol(parsed.protocol)
        || !is_exact_service_version(&parsed.child_version)
    {
        return None;
    }
    if let Some(update) = &parsed.update {
        if !update.is_valid_fields() {
            return None;
        }
        let selected = match update {
            ServiceUpdateRecord::Pending { target_version, .. }
            | ServiceUpdateRecord::Committed { target_version, .. } => target_version,
            ServiceUpdateRecord::RolledBack { from_version, .. }
            | ServiceUpdateRecord::Failed { from_version, .. } => from_version,
        };
        if parsed.child_version != *selected {
            return None;
        }
    }
    Some(parsed)
}

/// Reads and validates `service-state.json`, adopting legacy protocol-2 records in memory
/// (`serviceLauncher.ts:652-657`).
pub fn read_service_state(file_path: &Path) -> Result<ServiceState, ServiceLauncherError> {
    let contents = fs::read_to_string(file_path).map_err(|source| ServiceLauncherError::Io {
        operation: "read_service_state",
        path: file_path.to_path_buf(),
        source,
    })?;
    parse_service_state(&contents)
        .or_else(|| parse_legacy_service_state(&contents))
        .ok_or_else(|| ServiceLauncherError::InvalidServiceState {
            path: file_path.to_path_buf(),
        })
}

/// Atomically writes `service-state.json` via sibling temp file, `sync_all`, and rename
/// (`serviceLauncher.ts:667-670`).
pub fn write_service_state(
    file_path: &Path,
    state: &ServiceState,
) -> Result<(), ServiceLauncherError> {
    if let Some(parent) = file_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| ServiceLauncherError::Io {
            operation: "create_service_state_dir",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut json =
        serde_json::to_string_pretty(state).map_err(|source| ServiceLauncherError::Json {
            operation: "serialize_service_state",
            path: file_path.to_path_buf(),
            source,
        })?;
    json.push('\n');

    let parent = match file_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let file_name = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("service-state.json");
    let temp_path = parent.join(format!(
        ".{file_name}.{}.{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));

    let write_res = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, file_path)
    })();

    if let Err(source) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(ServiceLauncherError::Io {
            operation: "write_service_state",
            path: file_path.to_path_buf(),
            source,
        });
    }
    Ok(())
}
