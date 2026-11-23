//! Desktop App Activation Control protocol types and deterministic address derivation
//! matching `packages/shared/src/desktopAppControl.ts` and `packages/contracts/src/desktopAppActivation.ts`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Wire protocol version for desktop app activation (`desktopAppActivation.ts:5`).
pub const DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION: u32 = 1;

/// Maximum incoming activation request line length in bytes (`64 KiB`).
pub const MAX_CONTROL_REQUEST_BYTES: usize = 64 * 1024;

/// Target platform controlling desktop activation socket / named pipe addressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopControlPlatform {
    /// Windows Named Pipe (`\\.\pipe\legioncode-app-<24hex>`).
    Win32,
    /// Linux `AF_UNIX` socket (`107`-byte `sun_path` limit).
    Linux,
    /// macOS `AF_UNIX` socket (`103`-byte `sun_path` limit).
    Darwin,
}

impl DesktopControlPlatform {
    /// Returns the compile-time host [`DesktopControlPlatform`].
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Win32
        } else if cfg!(target_os = "macos") {
            Self::Darwin
        } else {
            Self::Linux
        }
    }
}

/// Resolved desktop control endpoint address and optional Unix socket parent directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppControlAddress {
    /// Windows Named Pipe path (`\\.\pipe\...`) or POSIX `AF_UNIX` socket path.
    pub address: String,
    /// Private socket directory on POSIX platforms (`None` on Windows).
    pub directory: Option<String>,
}

/// Derives the deterministic control address matching `@legioncode/shared/desktopAppControl.ts`
/// routed through `legion_contracts::brand::ACTIVE_BRAND`.
pub fn resolve_desktop_app_control_address(
    state_dir: &str,
    platform: DesktopControlPlatform,
    temp_dir: &str,
    user_id: Option<u32>,
) -> DesktopAppControlAddress {
    let shared_platform = match platform {
        DesktopControlPlatform::Win32 => {
            legion_shared::desktop_app_control::DesktopControlPlatform::Win32
        }
        DesktopControlPlatform::Linux => {
            legion_shared::desktop_app_control::DesktopControlPlatform::Linux
        }
        DesktopControlPlatform::Darwin => {
            legion_shared::desktop_app_control::DesktopControlPlatform::Darwin
        }
    };
    let resolved = legion_shared::desktop_app_control::resolve_desktop_app_control_address(
        state_dir,
        shared_platform,
        temp_dir,
        user_id,
    );
    DesktopAppControlAddress {
        address: resolved.address,
        directory: resolved.directory,
    }
}

/// Incoming activation JSON request over the desktop control socket / named pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopControlRequest {
    /// Protocol version (must equal [`DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION`]).
    pub version: u32,
    /// Caller-generated request identifier echoed back in [`DesktopControlResponse`].
    pub request_id: String,
    /// Workspace directory path to focus or open in the desktop host.
    pub workspace_root: String,
    /// Optional message discriminator (`"open-workspace"` when sent by `legioncode app`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,
    /// Optional caller platform (`"win32"`, `"linux"`, or `"darwin"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Activation outcome tag returned in [`DesktopControlResponse::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopControlStatus {
    /// Workspace activation succeeded.
    Activated,
    /// Request validation or activation failed.
    Error,
}

/// Reply sent back to `legioncode app` or activation tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopControlResponse {
    /// Protocol version (`1`).
    pub version: u32,
    /// Echoed correlation ID.
    pub request_id: String,
    /// `"activated"` or `"error"`.
    pub status: DesktopControlStatus,
    /// `true` when `status == DesktopControlStatus::Activated`.
    pub ok: bool,
    /// Error code (`"invalid-request"`, `"internal-error"`, etc.) when `status == Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional human-readable error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Activated workspace root echoed on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
}

impl DesktopControlResponse {
    /// Builds a successful `"activated"` response for `request_id` and `workspace_root`.
    pub fn activated(request_id: impl Into<String>, workspace_root: impl Into<String>) -> Self {
        Self {
            version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
            request_id: request_id.into(),
            status: DesktopControlStatus::Activated,
            ok: true,
            code: None,
            message: None,
            workspace_root: Some(workspace_root.into()),
        }
    }

    /// Builds an `"error"` response for `request_id` with `code` and `message`.
    pub fn error(
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
            request_id: request_id.into(),
            status: DesktopControlStatus::Error,
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            workspace_root: None,
        }
    }
}

/// Callback invoked for every validated [`DesktopControlRequest`].
pub type ActivationHandler =
    Arc<dyn Fn(DesktopControlRequest) -> DesktopControlResponse + Send + Sync>;

/// Parses and validates one newline-delimited JSON activation request line.
pub fn handle_activation_line(line: &str, handler: &ActivationHandler) -> DesktopControlResponse {
    if line.len() > MAX_CONTROL_REQUEST_BYTES {
        return DesktopControlResponse::error(
            "invalid-request",
            "invalid-request",
            "The desktop app request is too large.",
        );
    }

    let trimmed = line.trim_end_matches(['\r', '\n']);
    let raw_val: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(val) => val,
        Err(_) => {
            return DesktopControlResponse::error(
                "invalid-request",
                "invalid-request",
                "The desktop app request is not valid JSON.",
            );
        }
    };

    let fallback_id = raw_val
        .get("requestId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("invalid-request")
        .to_owned();

    let req: DesktopControlRequest = match serde_json::from_value(raw_val) {
        Ok(parsed) => parsed,
        Err(_) => {
            return DesktopControlResponse::error(
                fallback_id,
                "invalid-request",
                "The desktop app request is invalid.",
            );
        }
    };

    if req.version != DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION {
        return DesktopControlResponse::error(
            req.request_id,
            "invalid-request",
            format!(
                "Unsupported desktop activation protocol version {}; expected {DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION}.",
                req.version
            ),
        );
    }
    if req.request_id.trim().is_empty() || req.workspace_root.trim().is_empty() {
        return DesktopControlResponse::error(
            fallback_id,
            "invalid-request",
            "requestId and workspaceRoot must be non-empty.",
        );
    }
    if req
        .request_type
        .as_deref()
        .is_some_and(|kind| kind != "open-workspace")
    {
        return DesktopControlResponse::error(
            req.request_id,
            "invalid-request",
            "Unsupported activation request type.",
        );
    }

    handler(req)
}
