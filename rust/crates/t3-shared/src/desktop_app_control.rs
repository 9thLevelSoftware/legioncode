//! Deterministic desktop app activation IPC pipe / Unix socket address derivation
//! matching `packages/shared/src/desktopAppControl.ts` bit-for-bit.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const UNIX_SUN_PATH_LIMIT_LINUX: usize = 107;
const UNIX_SUN_PATH_LIMIT_DARWIN: usize = 103;

/// Target platform kind for [`resolve_desktop_app_control_address`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopControlPlatform {
    /// Windows named pipe (`\\.\pipe\t3code-app-<24hex>`).
    Win32,
    /// Linux Unix domain socket (`107`-byte `sun_path` limit).
    Linux,
    /// macOS Darwin Unix domain socket (`103`-byte `sun_path` limit).
    Darwin,
}

impl DesktopControlPlatform {
    /// Returns the compile-time host platform variant.
    pub const fn current() -> Self {
        #[cfg(windows)]
        {
            Self::Win32
        }
        #[cfg(target_os = "macos")]
        {
            Self::Darwin
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Self::Linux
        }
    }
}

/// Resolved named pipe or Unix domain socket address for desktop window activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppControlAddress {
    /// Named pipe path (`\\.\pipe\...`) on Windows or socket path on Unix.
    pub address: String,
    /// Parent socket directory to create/secure on Unix, or `None` on Windows.
    pub directory: Option<String>,
}

/// Computes the 24-character lowercase hex SHA-256 prefix matching `shortHash` in
/// `packages/shared/src/desktopAppControl.ts`.
pub fn desktop_control_state_hash(state_dir: &str) -> String {
    let digest = Sha256::digest(state_dir.as_bytes());
    let mut out = String::with_capacity(24);
    for &byte in &digest[..12] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Resolves the deterministic desktop activation control address for `state_dir`, hashing
/// state directories to stay within Unix socket path limits.
///
/// # Examples
///
/// ```
/// use t3_shared::desktop_app_control::{
///     DesktopAppControlAddress, DesktopControlPlatform, resolve_desktop_app_control_address,
/// };
///
/// let win = resolve_desktop_app_control_address(
///     r"C:\Users\alice\.t3\userdata",
///     DesktopControlPlatform::Win32,
///     r"C:\Temp",
///     Some(1000),
/// );
/// assert_eq!(
///     win,
///     DesktopAppControlAddress {
///         address: r"\\.\pipe\t3code-app-7f7a9e0de59c51e4f7ba644b".to_owned(),
///         directory: None,
///     }
/// );
/// ```
pub fn resolve_desktop_app_control_address(
    state_dir: &str,
    platform: DesktopControlPlatform,
    temp_dir: &str,
    user_id: Option<u32>,
) -> DesktopAppControlAddress {
    let state_hash = desktop_control_state_hash(state_dir);
    if platform == DesktopControlPlatform::Win32 {
        return DesktopAppControlAddress {
            address: format!(r"\\.\pipe\t3code-app-{state_hash}"),
            directory: None,
        };
    }

    let user_key = match user_id {
        Some(uid) => uid.to_string(),
        None => state_hash[..12].to_owned(),
    };
    let folder = format!("t3code-{user_key}");
    let socket_name = format!("{state_hash}.sock");
    let max_path_bytes = match platform {
        DesktopControlPlatform::Linux | DesktopControlPlatform::Win32 => UNIX_SUN_PATH_LIMIT_LINUX,
        DesktopControlPlatform::Darwin => UNIX_SUN_PATH_LIMIT_DARWIN,
    };

    let mut directory = posix_join(temp_dir, &folder);
    if posix_join(&directory, &socket_name).len() > max_path_bytes {
        directory = posix_join("/tmp", &folder);
    }

    DesktopAppControlAddress {
        address: posix_join(&directory, &socket_name),
        directory: Some(directory),
    }
}

fn posix_join(base: &str, segment: &str) -> String {
    if base.is_empty() {
        return segment.to_owned();
    }
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        format!("/{segment}")
    } else {
        format!("{trimmed}/{segment}")
    }
}
