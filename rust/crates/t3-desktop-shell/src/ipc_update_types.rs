//! Typed update, DB preflight, and boot-service IPC payloads for [`crate::ipc::DesktopIpcRouter`].

use serde::{Deserialize, Serialize};
use t3_contracts::policy::ManagedUpdatesPolicy;
use t3_edge::desktop_updates::{
    DesktopUpdateChannel, DesktopUpdateMachine, PendingUpdate, UpdateErrorContext, UpdateStatus,
    VerifiedArtifact,
};
use t3_persistence::service_preflight::ServicePreflightResult;

/// Parameters for `desktop.updateStep`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStepParams {
    /// Optional serialized update machine snapshot to step instead of the router's internal one.
    #[serde(default)]
    pub state: Option<DesktopUpdateMachine>,
    /// Update state transition to apply.
    #[serde(flatten)]
    pub action: UpdateStepAction,
}

/// Supported state transitions for `desktop.updateStep` (`BUG-28`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum UpdateStepAction {
    /// Read current update machine state without mutating it.
    GetState,
    /// Reset update machine to a channel and running version.
    #[serde(rename_all = "camelCase")]
    Reset {
        /// Release channel override.
        #[serde(default)]
        channel: Option<DesktopUpdateChannel>,
        /// Currently running version override.
        #[serde(default)]
        current_version: Option<String>,
    },
    /// Apply an optional [`ManagedUpdatesPolicy`].
    #[serde(rename_all = "camelCase")]
    ApplyManagedPolicy {
        /// Managed update policy limits.
        #[serde(default)]
        policy: Option<ManagedUpdatesPolicy>,
    },
    /// Begin an update check while preserving any retained `V1` download.
    #[serde(rename_all = "camelCase")]
    BeginCheck {
        /// ISO-8601 check timestamp.
        checked_at: String,
    },
    /// Offer candidate `V2` in `pending_check` without clearing `V1`.
    #[serde(rename_all = "camelCase")]
    UpdateAvailable {
        /// Offered version string (`V2`).
        version: String,
        /// ISO-8601 discovery timestamp.
        checked_at: String,
        /// Optional release note bullets.
        #[serde(default)]
        release_notes: Vec<String>,
    },
    /// Record an up-to-date check response.
    #[serde(rename_all = "camelCase")]
    NoUpdate {
        /// ISO-8601 check timestamp.
        checked_at: String,
    },
    /// Record a check failure (`BUG-28` restores retained `V1` if present).
    #[serde(rename_all = "camelCase")]
    CheckFailure {
        /// Error summary.
        message: String,
        /// ISO-8601 failure timestamp.
        checked_at: String,
    },
    /// Start downloading the pending `V2` candidate.
    BeginDownload,
    /// Update download progress (`0..=100`).
    #[serde(rename_all = "camelCase")]
    DownloadProgress {
        /// Progress percentage.
        percent: u8,
    },
    /// Record a download failure (`BUG-28` restores retained `V1` if present).
    #[serde(rename_all = "camelCase")]
    DownloadFailure {
        /// Error summary.
        message: String,
    },
    /// Promote a verified download (`V1` or `V2`) into `retained_download`.
    #[serde(rename_all = "camelCase")]
    DownloadComplete {
        /// Verified version string.
        version: String,
        /// Expected SHA-512 digest (128-char hex or base64).
        sha512: String,
        /// Artifact byte length.
        size_bytes: u64,
        /// Optional UTF-8 payload to verify against `sha512` before promoting.
        #[serde(default)]
        payload_utf8: Option<String>,
    },
    /// Record an installer launch failure while retaining `retained_download`.
    #[serde(rename_all = "camelCase")]
    InstallFailure {
        /// Error summary.
        message: String,
    },
}

/// Result returned by `desktop.updateStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStepResult {
    /// Current lifecycle status.
    pub status: UpdateStatus,
    /// Active release channel.
    pub channel: DesktopUpdateChannel,
    /// Currently running application version.
    pub current_version: String,
    /// Verified installer retained on disk (`V1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_download: Option<VerifiedArtifact>,
    /// Candidate release currently offered or downloading (`V2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_check: Option<PendingUpdate>,
    /// Artifact immediately eligible for installation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installable_artifact: Option<VerifiedArtifact>,
    /// Latest normalized error message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Latest error operation context, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_context: Option<UpdateErrorContext>,
    /// Full state machine snapshot for stateless roundtripping.
    pub state: DesktopUpdateMachine,
}

/// Parameters for `desktop.preflightDb`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PreflightDbParams {
    /// Explicit SQLite database file path.
    #[serde(alias = "dbPath")]
    pub database_path: Option<String>,
    /// Fallback T3 home directory when `database_path` is omitted.
    pub base_dir: Option<String>,
    /// Selects `dev/` vs `userdata/` when deriving from `base_dir`.
    pub is_development: Option<bool>,
    /// Whether `base_dir` was explicitly configured.
    pub base_dir_is_explicit: Option<bool>,
    /// Service launcher protocol version (defaults to `4`).
    pub launcher_protocol: Option<i64>,
    /// Running version string (defaults to crate version).
    pub version: Option<String>,
}

/// Result returned by `desktop.preflightDb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightDbResult {
    /// Database path inspected.
    pub database_path: String,
    /// `true` when preflight returned `ServicePreflightResult::Ready`.
    pub ready: bool,
    /// Structured preflight result (`ready` or `blocked`).
    pub preflight: ServicePreflightResult,
}

/// Parameters for `desktop.renderServiceUnits`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RenderServiceUnitsParams {
    /// Launcher command and args (defaults to `["/usr/bin/t3", "__service-launcher"]`).
    pub program: Option<Vec<String>>,
    /// Served T3 home directory (`T3CODE_HOME`).
    #[serde(alias = "t3Home")]
    pub base_dir: Option<String>,
    /// Service log file path.
    pub log_path: Option<String>,
    /// Destination systemd unit path.
    pub unit_path: Option<String>,
    /// Optional `T3CODE_INSTALL_BIN_DIR` value.
    pub install_bin_dir: Option<String>,
    /// macOS `WorkingDirectory` home directory.
    pub home_dir: Option<String>,
    /// macOS `PATH` environment string inside launchd plist.
    pub environment_path: Option<String>,
    /// Windows Task Scheduler `<UserId>`.
    pub windows_user_id: Option<String>,
    /// Windows `conhost.exe` executable path.
    pub conhost_path: Option<String>,
}

/// Canonical unit filenames produced by `t3-edge::boot_service_units`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUnitFileNames {
    /// Systemd user unit filename (`t3code.service`).
    pub systemd: String,
    /// macOS launchd plist filename (`com.t3tools.t3code.service.plist`).
    pub launchd: String,
    /// Windows Task Scheduler XML filename (`t3code-task.xml`).
    pub windows_task: String,
}

/// Result returned by `desktop.renderServiceUnits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderServiceUnitsResult {
    /// Rendered Linux systemd user unit text.
    pub systemd_unit: String,
    /// Rendered macOS launchd property list XML.
    pub launchd_plist: String,
    /// Rendered Windows Task Scheduler XML text.
    pub windows_task_xml: String,
    /// Lowercase SHA-256 hex digest of the UTF-16LE + BOM encoded Task XML bytes.
    pub windows_task_utf16le_bom_sha256: String,
    /// Byte length of the UTF-16LE + BOM encoded Task XML payload.
    pub windows_task_utf16le_bom_bytes: usize,
    /// Canonical output filenames for all three platforms.
    pub file_names: ServiceUnitFileNames,
}
