//! Tests for `legion_edge::desktop_updates` (`BUG-28`, `BUG-31`, `Range` resume, and SHA-512).

use legion_contracts::brand::LEGION_CODE;
use legion_contracts::policy::{ManagedUpdatesMode, ManagedUpdatesPolicy};
use legion_edge::desktop_updates::{
    DOWNLOAD_STALL_RETRIES, DesktopUpdateChannel, DesktopUpdateError, DesktopUpdateMachine,
    DigestError, DownloadFailureKind, DownloadResumePlanner, HttpDownloadResponse, PendingUpdate,
    ResponseAction, ResumeError, RetryPlan, Sha512Verifier, TrackMismatch, UpdateErrorContext,
    UpdateStatus, VerifiedArtifact, compute_sha512_base64, compute_sha512_hex,
    desktop_update_channel_of, render_aur_launcher, render_aur_launcher_for_brand,
    track_mismatch_message, verify_sha512,
};
use proptest::prelude::*;

#[test]
fn bug28_failed_v2_check_or_download_restores_retained_v1() -> Result<(), String> {
    let mut machine = DesktopUpdateMachine::new(DesktopUpdateChannel::Latest, "1.0.0");
    let v1 = VerifiedArtifact::new("1.1.0", "sha-v1", 2048);
    machine.on_download_complete(v1.clone());
    assert_eq!(machine.status(), UpdateStatus::Downloaded);
    assert_eq!(machine.installable_artifact(), Some(&v1));

    // Install-time refresh check discovers V2, then a subsequent check fails -> V1 restored!
    machine.begin_check("2026-11-20T00:00:00Z");
    assert_eq!(machine.retained_download(), Some(&v1));
    machine.on_update_available(PendingUpdate::new("1.2.0", "2026-11-20T00:00:01Z"));
    assert_eq!(machine.status(), UpdateStatus::Available);
    assert_eq!(machine.retained_download(), Some(&v1));
    machine.on_check_failure("feed offline", "2026-11-20T00:00:02Z");
    assert_eq!(machine.status(), UpdateStatus::Downloaded);
    assert_eq!(machine.installable_artifact(), Some(&v1));

    // Discover V2 again, start downloading V2, and fail download -> V1 restored (BUG-28)!
    machine.on_update_available(PendingUpdate::new("1.2.0", "2026-11-20T00:01:00Z"));
    machine.begin_download().map_err(|e| e.to_string())?;
    machine
        .on_download_progress(55)
        .map_err(|e| e.to_string())?;
    assert_eq!(
        machine.pending_check().and_then(|p| p.download_percent),
        Some(55)
    );
    machine.on_download_failure("net::ERR_TIMED_OUT");
    assert_eq!(machine.status(), UpdateStatus::Downloaded);
    assert_eq!(machine.error_context(), Some(UpdateErrorContext::Download));
    assert_eq!(machine.installable_artifact(), Some(&v1));
    assert_eq!(machine.pending_check(), None);

    // Managed policy checks via legion_contracts::policy.
    let deny_nightly = ManagedUpdatesPolicy {
        mode: ManagedUpdatesMode::Auto,
        feed_url: None,
        allow_nightly: Some(false),
    };
    let mut nightly = DesktopUpdateMachine::new(DesktopUpdateChannel::Nightly, "1.2.0-nightly.1");
    assert!(!nightly.apply_managed_policy(Some(&deny_nightly)));
    assert_eq!(nightly.status(), UpdateStatus::Disabled);
    assert_eq!(
        nightly.begin_download(),
        Err(DesktopUpdateError::UpdatesDisabled)
    );
    Ok(())
}

#[test]
fn range_resume_planner_and_sha512_verifier() -> Result<(), String> {
    let mut planner = DownloadResumePlanner::new();
    assert_eq!(planner.max_retries(), DOWNLOAD_STALL_RETRIES);
    assert_eq!(planner.range_header(), None);

    // Pre-existing bytes are ignored until this session writes bytes.
    planner.record_bytes_written(4096);
    assert_eq!(planner.range_header().as_deref(), Some("bytes=4096-"));

    // 3 stall retries with 2s / 4s / 8s backoff, 4th aborts.
    assert_eq!(
        planner.plan_failure(DownloadFailureKind::StallOrTimeout),
        RetryPlan::Retry {
            retry_index: 1,
            backoff_ms: 2_000,
            range_offset: 4096,
        }
    );
    assert_eq!(
        planner.plan_failure(DownloadFailureKind::StallOrTimeout),
        RetryPlan::Retry {
            retry_index: 2,
            backoff_ms: 4_000,
            range_offset: 4096,
        }
    );
    assert_eq!(
        planner.plan_failure(DownloadFailureKind::StallOrTimeout),
        RetryPlan::Retry {
            retry_index: 3,
            backoff_ms: 8_000,
            range_offset: 4096,
        }
    );
    assert_eq!(
        planner.plan_failure(DownloadFailureKind::StallOrTimeout),
        RetryPlan::Abort
    );

    // Non-stall error aborts immediately even with retries remaining.
    let mut immediate = DownloadResumePlanner::new();
    assert_eq!(
        immediate.plan_failure(DownloadFailureKind::Other),
        RetryPlan::Abort
    );

    // 206 validates Content-Range prefix, 416 triggers verify-existing, 200 resets offset.
    assert_eq!(
        planner.plan_response(HttpDownloadResponse::PartialContent206 {
            content_range: Some("bytes 0-100/5000".to_owned()),
        }),
        Err(ResumeError::InvalidContentRange {
            offset: 4096,
            content_range: Some("bytes 0-100/5000".to_owned()),
        })
    );
    assert_eq!(
        planner.plan_response(HttpDownloadResponse::PartialContent206 {
            content_range: Some("bytes 4096-4999/5000".to_owned()),
        }),
        Ok(ResponseAction::AppendAndVerifyWholeFile { offset: 4096 })
    );
    assert_eq!(
        planner.plan_response(HttpDownloadResponse::RangeNotSatisfiable416),
        Ok(ResponseAction::VerifyExistingFile)
    );
    assert_eq!(
        planner.plan_response(HttpDownloadResponse::Ok200),
        Ok(ResponseAction::RestartFromZero)
    );
    assert_eq!(planner.bytes_written(), 0);

    // SHA-512 supports both base64 and 128-char hex digests.
    let data = b"hello LegionCode desktop release";
    let b64 = compute_sha512_base64(data);
    let hex_digest = compute_sha512_hex(data);
    verify_sha512(data, &b64).map_err(|e| e.to_string())?;
    verify_sha512(data, &hex_digest).map_err(|e| e.to_string())?;

    let mut streaming = Sha512Verifier::new();
    streaming.update(b"hello ");
    streaming.update(b"LegionCode desktop release");
    streaming.finish(&b64).map_err(|e| e.to_string())?;

    assert!(matches!(
        verify_sha512(b"corrupted", &b64),
        Err(DigestError::ChecksumMismatch { .. })
    ));
    assert_eq!(
        verify_sha512(data, "not-valid-sha512"),
        Err(DigestError::InvalidExpectedFormat)
    );
    Ok(())
}

#[test]
fn bug31_aur_launcher_sets_env_and_distinct_nightly_desktop_id() {
    let stable = render_aur_launcher(DesktopUpdateChannel::Latest);
    assert_eq!(stable.package_name, "legioncode-bin");
    assert_eq!(stable.launcher_path, "/usr/bin/legioncode");
    assert_eq!(stable.desktop_entry_id, "dev.legioncode.LegionCode.desktop");
    assert_eq!(stable.startup_wm_class, "dev.legioncode.LegionCode");
    assert!(stable.launcher_script.contains(
        "APPDIR=/opt/legioncode-bin LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode"
    ));
    assert!(
        stable
            .desktop_entry
            .contains("StartupWMClass=dev.legioncode.LegionCode\n")
    );

    let nightly = render_aur_launcher(DesktopUpdateChannel::Nightly);
    assert_eq!(nightly.package_name, "legioncode-nightly-bin");
    assert_eq!(nightly.launcher_path, "/usr/bin/legioncode-nightly");
    assert_eq!(
        nightly.desktop_entry_id,
        "dev.legioncode.LegionCodeNightly.desktop"
    );
    assert_eq!(nightly.startup_wm_class, "dev.legioncode.LegionCodeNightly");
    assert!(nightly.launcher_script.contains(
        "APPDIR=/opt/legioncode-nightly-bin LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode-nightly"
    ));
    assert!(
        nightly
            .desktop_entry
            .contains("Exec=legioncode-nightly %U\n")
    );
    assert!(
        nightly
            .desktop_entry
            .contains("StartupWMClass=dev.legioncode.LegionCodeNightly\n")
    );
    assert_ne!(stable.desktop_entry_id, nightly.desktop_entry_id);

    let legion = render_aur_launcher_for_brand(DesktopUpdateChannel::Latest, &LEGION_CODE);
    assert_eq!(legion, stable);
    assert!(
        legion
            .desktop_entry
            .contains("MimeType=x-scheme-handler/legioncode;\n")
    );
}

#[test]
fn track_mismatch_reported_and_cleared_across_subsequent_transitions() {
    assert_eq!(
        desktop_update_channel_of("1.2.4"),
        DesktopUpdateChannel::Latest
    );
    assert_eq!(
        desktop_update_channel_of("1.2.4-nightly.20261027.1"),
        DesktopUpdateChannel::Nightly
    );

    let mut machine = DesktopUpdateMachine::new(DesktopUpdateChannel::Nightly, "1.2.4");
    machine.begin_check("2026-11-20T00:00:00Z");
    machine.on_no_offer("2026-11-20T00:00:01Z", false, None);
    assert_eq!(machine.status(), UpdateStatus::Error);
    assert_eq!(machine.error_context(), Some(UpdateErrorContext::Check));
    assert_eq!(
        machine.track_mismatch(),
        Some(TrackMismatch {
            build_channel: DesktopUpdateChannel::Latest
        })
    );
    assert_eq!(
        machine.last_error(),
        Some(
            track_mismatch_message(DesktopUpdateChannel::Latest, DesktopUpdateChannel::Nightly)
                .as_str()
        )
    );

    // Later unrelated failures or new checks clear stale track mismatch.
    machine.on_check_failure("background updater failure", "2026-11-20T00:00:02Z");
    assert_eq!(machine.track_mismatch(), None);

    machine.on_no_offer("2026-11-20T00:00:03Z", false, None);
    assert!(machine.track_mismatch().is_some());
    machine.begin_check("2026-11-20T00:00:04Z");
    assert_eq!(machine.status(), UpdateStatus::Checking);
    assert_eq!(machine.track_mismatch(), None);

    // A release floor disables mismatch because cross-track downgrade is permitted intentionally.
    machine.on_no_offer("2026-11-20T00:00:05Z", true, None);
    assert_eq!(machine.status(), UpdateStatus::UpToDate);
    assert_eq!(machine.track_mismatch(), None);

    // Policy blocking nightly refuses without naming nightly as a switch-back target.
    let deny_nightly = ManagedUpdatesPolicy {
        mode: ManagedUpdatesMode::Auto,
        feed_url: Some("https://updates.example.com".to_owned()),
        allow_nightly: Some(false),
    };
    let mut nightly_on_latest =
        DesktopUpdateMachine::new(DesktopUpdateChannel::Latest, "1.2.4-nightly.20261027.1");
    nightly_on_latest.on_no_offer("2026-11-20T00:00:06Z", false, Some(&deny_nightly));
    assert_eq!(nightly_on_latest.status(), UpdateStatus::Error);
    assert_eq!(
        nightly_on_latest.error_context(),
        Some(UpdateErrorContext::Check)
    );
    assert_eq!(nightly_on_latest.track_mismatch(), None);
}

proptest! {
    #[test]
    fn proptest_retained_v1_never_wiped_by_failed_v2_transitions(
        ops in prop::collection::vec(0_u8..7, 1..40),
    ) {
        let mut machine = DesktopUpdateMachine::new(DesktopUpdateChannel::Latest, "1.0.0");
        let v1 = VerifiedArtifact::new("1.1.0", "digest-v1", 4096);
        machine.on_download_complete(v1.clone());

        for op in ops {
            match op {
                0 => machine.begin_check("2026-11-20T00:00:00Z"),
                1 => machine.on_update_available(PendingUpdate::new("2.0.0", "2026-11-20T00:01:00Z")),
                2 => {
                    let _ = machine.begin_download();
                }
                3 => {
                    let _ = machine.on_download_progress(50);
                }
                4 => machine.on_check_failure("check timeout", "2026-11-20T00:02:00Z"),
                5 => machine.on_download_failure("download timeout"),
                _ => machine.on_no_update("2026-11-20T00:03:00Z"),
            }
            prop_assert_eq!(machine.retained_download(), Some(&v1));
            if !matches!(
                machine.status(),
                UpdateStatus::Checking | UpdateStatus::Available | UpdateStatus::Downloading
            ) {
                prop_assert_eq!(machine.status(), UpdateStatus::Downloaded);
                prop_assert_eq!(machine.installable_artifact(), Some(&v1));
            }
        }
    }

    #[test]
    fn proptest_sha512_roundtrip_for_arbitrary_payloads(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let b64 = compute_sha512_base64(&bytes);
        let hex_digest = compute_sha512_hex(&bytes);
        prop_assert!(verify_sha512(&bytes, &b64).is_ok());
        prop_assert!(verify_sha512(&bytes, &hex_digest).is_ok());
    }
}
