//! Pure Safe Rust Tauri v2 desktop host library crate for T3 Code (`t3-desktop`).
//!
//! Coordinates desktop custom-scheme routing and window bounds clamping ([`t3_surfaces::desktop`]),
//! auto-update state transitions and cross-platform boot-service units ([`t3_edge`]),
//! pre-start SQLite/protocol compatibility checks ([`t3_persistence::service_preflight`]),
//! and `BUG-09` child environment sanitization & Win32 Job supervision ([`t3_process_ffi`]).
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_desktop_shell::bundle::{DesktopBundleConfig, DesktopBundleManifest};
//! use t3_desktop_shell::ipc::{DesktopIpcRequest, DesktopIpcRouter};
//!
//! let mut router = DesktopIpcRouter::default();
//! let resp = router.dispatch(DesktopIpcRequest {
//!     id: Some(serde_json::json!(1)),
//!     method: "desktop.resolveScheme".to_owned(),
//!     params: serde_json::json!({
//!         "method": "GET",
//!         "url": "t3code://app/%2e%2e%2fetc/passwd"
//!     }),
//! });
//! assert!(resp.ok);
//! assert_eq!(
//!     resp.result.as_ref().and_then(|v| v.get("status")).and_then(|v| v.as_u64()),
//!     Some(404)
//! );
//!
//! let (manifest, artifacts) = DesktopBundleManifest::generate(&DesktopBundleConfig::default())?;
//! assert_eq!(manifest.identifier, "com.t3tools.t3code");
//! assert!(artifacts.windows_task_utf16le_bom.starts_with(&[0xff, 0xfe]));
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod bundle;
mod bundle_templates;
pub mod control_server;
mod control_server_protocol;
pub mod ipc;
mod ipc_handlers;
mod ipc_types;
mod ipc_update_types;
pub mod self_check;
mod self_check_checks;
pub mod sidecar;
mod sidecar_discovery;
pub mod window_host;
pub mod window_host_protocol;
mod window_host_smoke;
mod window_host_types;

/// Returns the crate version string for `t3-desktop-shell`.
///
/// # Examples
///
/// ```
/// assert!(!t3_desktop_shell::desktop_shell_version().is_empty());
/// ```
pub const fn desktop_shell_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::desktop_shell_version;

    #[test]
    fn exposes_non_empty_package_version() {
        assert_eq!(desktop_shell_version(), env!("CARGO_PKG_VERSION"));
    }
}
