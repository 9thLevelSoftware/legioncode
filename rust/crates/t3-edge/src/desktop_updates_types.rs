//! Core data types and errors for [`crate::desktop_updates`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle status of `DesktopUpdateMachine` (`packages/contracts/src/ipc.ts:76-84`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateStatus {
    /// Automatic updates disabled by environment or managed policy.
    Disabled,
    /// Enabled and awaiting the next update check.
    Idle,
    /// Currently querying the release feed.
    Checking,
    /// Running build matches the latest release on the active track.
    UpToDate,
    /// A newer update in `pending_check` is ready to download.
    Available,
    /// Downloading the candidate in `pending_check`.
    Downloading,
    /// A verified artifact in `retained_download` is ready to install.
    Downloaded,
    /// Last operation failed without a retained download to fall back to.
    Error,
}

/// Operation phase associated with the latest update error (`ipc.ts:290`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateErrorContext {
    /// Failure occurred while checking for updates.
    Check,
    /// Failure occurred while downloading an update package.
    Download,
    /// Failure occurred while quitting and launching the installer.
    Install,
}

/// Fully downloaded and SHA-512 verified installer ready to install (`BUG-28`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedArtifact {
    /// Semantic version of the verified installer (`V1`).
    pub version: String,
    /// Expected SHA-512 digest verified against disk contents.
    pub sha512: String,
    /// Total artifact size in bytes.
    pub size_bytes: u64,
}

impl VerifiedArtifact {
    /// Constructs a verified update artifact descriptor.
    pub fn new(version: impl Into<String>, sha512: impl Into<String>, size_bytes: u64) -> Self {
        Self {
            version: version.into(),
            sha512: sha512.into(),
            size_bytes,
        }
    }
}

/// Candidate release discovered on the update feed (`V2`), kept separate from `retained_download`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdate {
    /// Offered release version (`V2`).
    pub version: String,
    /// ISO-8601 timestamp when the release was discovered.
    pub checked_at: String,
    /// Release note items extracted from the feed.
    pub release_notes: Vec<String>,
    /// Current download progress percentage (`0..=100`) once downloading starts.
    pub download_percent: Option<u8>,
}

impl PendingUpdate {
    /// Constructs a pending update offer with empty notes and no download progress yet.
    pub fn new(version: impl Into<String>, checked_at: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            checked_at: checked_at.into(),
            release_notes: Vec::new(),
            download_percent: None,
        }
    }
}

/// Errors emitted by invalid `DesktopUpdateMachine` state transitions.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DesktopUpdateError {
    /// Download was requested when no `pending_check` candidate was available.
    #[error("cannot start download without an available pending update")]
    NoPendingUpdate,
    /// Progress update arrived while not in [`UpdateStatus::Downloading`].
    #[error("cannot record download progress while status is {status:?}")]
    NotDownloading {
        /// Current machine status.
        status: UpdateStatus,
    },
    /// Machine is disabled by policy or environment.
    #[error("desktop updates are disabled")]
    UpdatesDisabled,
}
