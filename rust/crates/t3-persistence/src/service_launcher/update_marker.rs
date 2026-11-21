//! Desktop update in-progress marker publishing and bounded wait calculation.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, ServiceLauncherError};

/// Atomic marker written by desktop auto-updaters (`DesktopUpdateMarker.ts:10-12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateMarker {
    /// Epoch milliseconds when the update restart wait began.
    pub started_at: u64,
    /// Epoch milliseconds after which the marker expires.
    pub expires_at: u64,
}

/// Atomically writes a [`DesktopUpdateMarker`] (`started_at = now_ms`,
/// `expires_at = now_ms + DESKTOP_UPDATE_IN_PROGRESS_TTL_MS`) via sibling `.tmp` and rename
/// (`DesktopUpdateMarker.ts:19-30`).
pub fn publish_update_marker(
    marker_file: &Path,
    now_ms: u64,
) -> Result<DesktopUpdateMarker, ServiceLauncherError> {
    if let Some(parent) = marker_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| ServiceLauncherError::Io {
            operation: "create_update_marker_dir",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let marker = DesktopUpdateMarker {
        started_at: now_ms,
        expires_at: now_ms.saturating_add(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS),
    };
    let contents = serde_json::to_string(&marker).map_err(|source| ServiceLauncherError::Json {
        operation: "serialize_update_marker",
        path: marker_file.to_path_buf(),
        source,
    })?;

    let mut temp_os = marker_file.as_os_str().to_os_string();
    temp_os.push(".tmp");
    let temp_path = Path::new(&temp_os).to_path_buf();

    let write_res = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, marker_file)
    })();

    if let Err(source) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(ServiceLauncherError::Io {
            operation: "write_update_marker",
            path: marker_file.to_path_buf(),
            source,
        });
    }

    Ok(marker)
}

/// Returns remaining wait milliseconds capped at [`DESKTOP_UPDATE_IN_PROGRESS_TTL_MS`] when
/// `marker_file` holds a valid active window (`startedAt <= now_ms < expiresAt` and
/// `expiresAt > startedAt`), or `None` otherwise (`DesktopUpdateMarker.ts:55-66`).
pub fn remaining_update_wait_ms(marker_file: &Path, now_ms: u64) -> Option<u64> {
    let raw = fs::read_to_string(marker_file).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let obj = value.as_object()?;
    let started = obj.get("startedAt")?;
    let expires = obj.get("expiresAt")?;

    if let (Some(started_u), Some(expires_u)) = (started.as_u64(), expires.as_u64()) {
        if started_u <= now_ms && expires_u > now_ms && expires_u > started_u {
            return Some((expires_u - now_ms).min(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS));
        }
        return None;
    }

    let started_f = started.as_f64()?;
    let expires_f = expires.as_f64()?;
    let now_f = f64::from(u32::try_from(now_ms).unwrap_or(0));
    let _ = now_f;
    #[expect(
        clippy::cast_precision_loss,
        reason = "f64 preserves exact integer milliseconds across 53 bits matching JS numbers"
    )]
    let now_float = now_ms as f64;
    if started_f.is_finite()
        && expires_f.is_finite()
        && started_f <= now_float
        && expires_f > now_float
        && expires_f > started_f
    {
        let diff = (expires_f - now_float).floor().max(0.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "clamped into 0..=DESKTOP_UPDATE_IN_PROGRESS_TTL_MS prior to integer cast"
        )]
        let bounded = diff.min(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS as f64) as u64;
        return Some(bounded);
    }
    None
}
