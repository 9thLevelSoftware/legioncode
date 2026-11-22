//! Desktop auto-update state machine (`BUG-28`), HTTP `Range` download resume planner,
//! SHA-512 verifier, and Linux AUR launcher generator (`BUG-31`).
//!
//! Mirrors `apps/desktop/src/updates/{DesktopUpdates.ts,updateMachine.ts}`,
//! `patches/electron-updater@6.8.9.patch`, and `packaging/aur/{legioncode-bin,legioncode-nightly-bin}/PKGBUILD`.
//!
//! # Examples
//!
//! ```
//! use legion_edge::desktop_updates::{
//!     DesktopUpdateChannel, DesktopUpdateMachine, DownloadFailureKind, DownloadResumePlanner,
//!     HttpDownloadResponse, PendingUpdate, ResponseAction, RetryPlan, UpdateStatus,
//!     VerifiedArtifact, compute_sha512_base64, render_aur_launcher, verify_sha512,
//! };
//!
//! // BUG-28: retained V1 download survives a failed V2 check or download.
//! let mut machine = DesktopUpdateMachine::new(DesktopUpdateChannel::Latest, "1.0.0");
//! let payload = b"legioncode-v1-installer";
//! let digest = compute_sha512_base64(payload);
//! assert!(verify_sha512(payload, &digest).is_ok());
//! machine.on_download_complete(VerifiedArtifact::new("1.1.0", digest, payload.len() as u64));
//!
//! machine.begin_check("2026-11-20T00:00:00Z");
//! machine.on_update_available(PendingUpdate::new("1.2.0", "2026-11-20T00:00:01Z"));
//! assert!(machine.begin_download().is_ok());
//! machine.on_download_failure("net::ERR_TIMED_OUT");
//! assert_eq!(machine.status(), UpdateStatus::Downloaded);
//! assert_eq!(machine.installable_artifact().map(|a| a.version.as_str()), Some("1.1.0"));
//!
//! // Range resume planner resumes stalled downloads up to 3 times.
//! let mut planner = DownloadResumePlanner::new();
//! planner.record_bytes_written(1024);
//! assert_eq!(planner.range_header().as_deref(), Some("bytes=1024-"));
//! assert_eq!(
//!     planner.plan_failure(DownloadFailureKind::StallOrTimeout),
//!     RetryPlan::Retry { retry_index: 1, backoff_ms: 2_000, range_offset: 1024 }
//! );
//! assert_eq!(
//!     planner.plan_response(HttpDownloadResponse::PartialContent206 {
//!         content_range: Some("bytes 1024-2047/2048".to_owned()),
//!     }),
//!     Ok(ResponseAction::AppendAndVerifyWholeFile { offset: 1024 })
//! );
//!
//! // BUG-31: AUR launchers emit LEGIONCODE_DESKTOP_LAUNCHER_PATH and distinct Nightly desktop ID.
//! let nightly = render_aur_launcher(DesktopUpdateChannel::Nightly);
//! assert_eq!(nightly.desktop_entry_id, "dev.legioncode.LegionCodeNightly.desktop");
//! assert!(nightly.launcher_script.contains("LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode-nightly"));
//! ```

use legion_contracts::policy::{ManagedUpdatesMode, ManagedUpdatesPolicy};
use legion_shared::search::normalize_whitespace;
use serde::{Deserialize, Serialize};

pub use crate::desktop_updates_aur::{
    AurLauncherBundle, DesktopUpdateChannel, render_aur_launcher, render_aur_launcher_for_brand,
};
pub use crate::desktop_updates_resume::{
    DOWNLOAD_STALL_RETRIES, DigestError, DownloadFailureKind, DownloadResumePlanner,
    HttpDownloadResponse, RETRY_BASE_BACKOFF_MS, ResponseAction, ResumeError, RetryPlan,
    Sha512Verifier, compute_sha512_base64, compute_sha512_hex, verify_sha512,
};
pub use crate::desktop_updates_types::{
    DesktopUpdateError, PendingUpdate, TrackMismatch, UpdateErrorContext, UpdateStatus,
    VerifiedArtifact, desktop_update_channel_of, track_mismatch_message,
};

/// Desktop auto-update state machine keeping `retained_download` separate from `pending_check`
/// (`BUG-28`).
///
/// When `V1` is already verified in `retained_download` and an install-time refresh check finds
/// `V2`, `V2` lives solely in `pending_check`. If checking or downloading `V2` fails, `pending_check`
/// is cleared and the state restores `UpdateStatus::Downloaded` pointing at `retained_download` (`V1`),
/// never stranding a ready installer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopUpdateMachine {
    channel: DesktopUpdateChannel,
    current_version: String,
    status: UpdateStatus,
    retained_download: Option<VerifiedArtifact>,
    pending_check: Option<PendingUpdate>,
    checked_at: Option<String>,
    last_error: Option<String>,
    error_context: Option<UpdateErrorContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    track_mismatch: Option<TrackMismatch>,
}

impl DesktopUpdateMachine {
    /// Creates an enabled update state machine in [`UpdateStatus::Idle`].
    pub fn new(channel: DesktopUpdateChannel, current_version: impl Into<String>) -> Self {
        Self {
            channel,
            current_version: current_version.into(),
            status: UpdateStatus::Idle,
            retained_download: None,
            pending_check: None,
            checked_at: None,
            last_error: None,
            error_context: None,
            track_mismatch: None,
        }
    }

    /// Active release channel.
    pub fn channel(&self) -> DesktopUpdateChannel {
        self.channel
    }

    /// Currently running application version.
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// Current lifecycle status.
    pub fn status(&self) -> UpdateStatus {
        self.status
    }

    /// Verified installer retained on disk (`V1`), independent of `pending_check` (`V2`).
    pub fn retained_download(&self) -> Option<&VerifiedArtifact> {
        self.retained_download.as_ref()
    }

    /// Candidate update currently offered or downloading (`V2`).
    pub fn pending_check(&self) -> Option<&PendingUpdate> {
        self.pending_check.as_ref()
    }

    /// Normalized error summary from the latest failed operation, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Context of `last_error`, if any.
    pub fn error_context(&self) -> Option<UpdateErrorContext> {
        self.error_context
    }

    /// Track mismatch attached when the last check failed solely due to a build vs saved
    /// channel mismatch (`#38506`).
    pub fn track_mismatch(&self) -> Option<TrackMismatch> {
        self.track_mismatch
    }

    /// Applies a [`ManagedUpdatesPolicy`], returning `true` when updates remain enabled.
    pub fn apply_managed_policy(&mut self, policy: Option<&ManagedUpdatesPolicy>) -> bool {
        self.track_mismatch = None;
        let blocked = policy.is_some_and(|p| {
            p.mode == ManagedUpdatesMode::Off
                || (self.channel == DesktopUpdateChannel::Nightly && p.allow_nightly == Some(false))
        });
        if blocked {
            self.status = UpdateStatus::Disabled;
            return false;
        }
        if self.status == UpdateStatus::Disabled {
            self.status = if self.retained_download.is_some() {
                UpdateStatus::Downloaded
            } else {
                UpdateStatus::Idle
            };
        }
        true
    }

    /// Begins an update check while keeping any `retained_download` intact.
    pub fn begin_check(&mut self, checked_at: impl Into<String>) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        self.status = UpdateStatus::Checking;
        self.checked_at = Some(checked_at.into());
        self.last_error = None;
        self.error_context = None;
        self.track_mismatch = None;
    }

    /// Records a newly offered update (`V2`) in `pending_check` without clearing `retained_download` (`V1`).
    pub fn on_update_available(&mut self, pending: PendingUpdate) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        let matches_retained = self
            .retained_download
            .as_ref()
            .is_some_and(|retained| retained.version == pending.version);
        self.checked_at = Some(pending.checked_at.clone());
        self.last_error = None;
        self.error_context = None;
        self.track_mismatch = None;
        if matches_retained {
            self.pending_check = None;
            self.status = UpdateStatus::Downloaded;
        } else {
            self.pending_check = Some(pending);
            self.status = UpdateStatus::Available;
        }
    }

    /// Records that the feed offered no newer release; restores `Downloaded` if `retained_download` exists.
    pub fn on_no_update(&mut self, checked_at: impl Into<String>) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        self.pending_check = None;
        self.checked_at = Some(checked_at.into());
        self.last_error = None;
        self.error_context = None;
        self.track_mismatch = None;
        self.status = if self.retained_download.is_some() {
            UpdateStatus::Downloaded
        } else {
            UpdateStatus::UpToDate
        };
    }

    /// Records that an update check offered nothing usable; surfaces a track mismatch when a build
    /// on one track checks a feed for another unblocked saved track without a release floor (`#38506`).
    pub fn on_no_offer(
        &mut self,
        checked_at: impl Into<String>,
        has_release_floor: bool,
        policy: Option<&ManagedUpdatesPolicy>,
    ) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        let build_channel = desktop_update_channel_of(&self.current_version);
        if has_release_floor || build_channel == self.channel {
            self.on_no_update(checked_at);
            return;
        }
        let blocked_reason = policy.and_then(|p| {
            if p.mode == ManagedUpdatesMode::Off {
                Some("Automatic updates are disabled by policy.".to_owned())
            } else if build_channel == DesktopUpdateChannel::Nightly
                && p.allow_nightly == Some(false)
            {
                Some(
                    "Nightly updates are turned off on this machine. Switch to the stable channel to keep getting updates."
                        .to_owned(),
                )
            } else {
                None
            }
        });
        if let Some(reason) = blocked_reason {
            self.on_check_failure_with_track_mismatch(&reason, checked_at, None);
            return;
        }
        let message = track_mismatch_message(build_channel, self.channel);
        self.on_check_failure_with_track_mismatch(
            &message,
            checked_at,
            Some(TrackMismatch { build_channel }),
        );
    }

    /// Handles a failed update check; restores `retained_download` (`V1`) instead of stranding it (`BUG-28`).
    pub fn on_check_failure(&mut self, message: &str, checked_at: impl Into<String>) {
        self.on_check_failure_with_track_mismatch(message, checked_at, None);
    }

    /// Handles a failed update check, optionally recording a [`TrackMismatch`] when no installer
    /// is retained on disk (`#38506`).
    pub fn on_check_failure_with_track_mismatch(
        &mut self,
        message: &str,
        checked_at: impl Into<String>,
        track_mismatch: Option<TrackMismatch>,
    ) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        self.checked_at = Some(checked_at.into());
        if self.retained_download.is_some() {
            self.pending_check = None;
            self.status = UpdateStatus::Downloaded;
            self.last_error = None;
            self.error_context = None;
            self.track_mismatch = None;
        } else {
            self.status = UpdateStatus::Error;
            self.last_error = Some(normalize_whitespace(message).into_owned());
            self.error_context = Some(UpdateErrorContext::Check);
            self.track_mismatch = track_mismatch;
        }
    }

    /// Transitions `pending_check` (`V2`) into [`UpdateStatus::Downloading`] without touching `retained_download`.
    pub fn begin_download(&mut self) -> Result<(), DesktopUpdateError> {
        if self.status == UpdateStatus::Disabled {
            return Err(DesktopUpdateError::UpdatesDisabled);
        }
        let Some(pending) = self.pending_check.as_mut() else {
            return Err(DesktopUpdateError::NoPendingUpdate);
        };
        pending.download_percent = Some(0);
        self.status = UpdateStatus::Downloading;
        self.last_error = None;
        self.error_context = None;
        self.track_mismatch = None;
        Ok(())
    }

    /// Updates download percentage (`0..=100`) on `pending_check`.
    pub fn on_download_progress(&mut self, percent: u8) -> Result<(), DesktopUpdateError> {
        if self.status != UpdateStatus::Downloading {
            return Err(DesktopUpdateError::NotDownloading {
                status: self.status,
            });
        }
        if let Some(pending) = self.pending_check.as_mut() {
            pending.download_percent = Some(percent.min(100));
        }
        Ok(())
    }

    /// Handles a failed `V2` download; restores `retained_download` (`V1`) if present (`BUG-28`).
    pub fn on_download_failure(&mut self, message: &str) {
        if self.status == UpdateStatus::Disabled {
            return;
        }
        self.last_error = Some(normalize_whitespace(message).into_owned());
        self.error_context = Some(UpdateErrorContext::Download);
        self.track_mismatch = None;
        if self.retained_download.is_some() {
            self.pending_check = None;
            self.status = UpdateStatus::Downloaded;
        } else if let Some(pending) = self.pending_check.as_mut() {
            pending.download_percent = None;
            self.status = UpdateStatus::Available;
        } else {
            self.status = UpdateStatus::Error;
        }
    }

    /// Promotes a verified downloaded artifact into `retained_download` and clears `pending_check`.
    pub fn on_download_complete(&mut self, artifact: VerifiedArtifact) {
        self.retained_download = Some(artifact);
        self.pending_check = None;
        self.status = UpdateStatus::Downloaded;
        self.last_error = None;
        self.error_context = None;
        self.track_mismatch = None;
    }

    /// Records an installer launch failure while retaining the verified download for retry.
    pub fn on_install_failure(&mut self, message: &str) {
        if self.retained_download.is_some() {
            self.status = UpdateStatus::Downloaded;
        } else {
            self.status = UpdateStatus::Error;
        }
        self.last_error = Some(normalize_whitespace(message).into_owned());
        self.error_context = Some(UpdateErrorContext::Install);
        self.track_mismatch = None;
    }

    /// Returns the verified installer ready for installation (`V1` fallback or newly downloaded `V2`).
    pub fn installable_artifact(&self) -> Option<&VerifiedArtifact> {
        match self.status {
            UpdateStatus::Downloaded => self.retained_download.as_ref(),
            UpdateStatus::Error
                if matches!(self.error_context, None | Some(UpdateErrorContext::Install)) =>
            {
                self.retained_download.as_ref()
            }
            _ => None,
        }
    }
}
