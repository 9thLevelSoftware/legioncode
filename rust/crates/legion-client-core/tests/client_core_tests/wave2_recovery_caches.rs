use std::error::Error;

use legion_client_core::backoff::{FailureReason, RETRY_MAX_DELAY_MS};
use legion_client_core::client_caches::{
    PatchReadSnapshot, PreviewRevisionState, ProviderOptionChoice, ProviderOptionDescriptor,
    VcsRefsDigestCache, format_model_selection_effort, read_selected_effort_value,
};
use legion_client_core::steady_shells_resume::{
    ThreadDetailDataStatus, ThreadResumeRecovery, ThreadStreamResumeState, ThreadSyncPhase,
    resolve_thread_sync_phase,
};
use legion_client_core::supervisor_recovery::{
    EnvironmentSupervisorState, LISTENER_RESTART_DELAY_MS, SupervisorDefectKind,
    SupervisorListenerKind, SupervisorPhase,
};
use legion_contracts::ids::{EnvironmentId, ProviderInstanceId};
use legion_contracts::orchestration::{
    ModelSelection, ProviderOptionSelection, ProviderOptionValue,
};

#[test]
fn supervisor_recovers_from_attempt_release_loop_and_listener_defects() {
    let mut sup = EnvironmentSupervisorState::new();
    sup.start_connecting();
    assert_eq!(sup.on_connected(), 1);
    assert_eq!(sup.phase, SupervisorPhase::Connected);

    // #39420: Live session release defect -> fast retry keeping monotonic generation = 1 -> reconnects gen 2.
    sup.on_defect(SupervisorDefectKind::SessionReleaseDefect, 10_000, 0.5);
    assert_eq!(sup.phase, SupervisorPhase::Backoff);
    assert_eq!(sup.generation, 1);
    assert!(sup.retry_at_ms.is_some_and(|t| t <= 10_250));
    assert_eq!(sup.on_connected(), 2);

    // Outer loop defect schedules max-backoff recovery rather than dying permanently (#39420).
    sup.on_defect(SupervisorDefectKind::LoopDefect, 20_000, 0.5);
    assert_eq!(sup.phase, SupervisorPhase::Backoff);
    assert_eq!(sup.last_failure, Some(FailureReason::Transient));
    assert_eq!(sup.retry_at_ms, Some(20_000 + RETRY_MAX_DELAY_MS));

    // Offline connectivity transition wakes when host returns online (#39431).
    sup.set_network_offline(true);
    assert_eq!(sup.phase, SupervisorPhase::Offline);
    let restart_at = sup.on_listener_defect(SupervisorListenerKind::Connectivity, 30_000);
    assert_eq!(restart_at, 30_000 + LISTENER_RESTART_DELAY_MS);
    assert!(sup.consume_due_listener_restart(SupervisorListenerKind::Connectivity, restart_at));
    sup.set_network_offline(false);
    assert_eq!(sup.phase, SupervisorPhase::Connecting);
    assert_eq!(sup.on_connected(), 3);
}

#[test]
fn thread_stream_resume_falls_back_to_full_snapshot_on_replay_defect() {
    assert_eq!(
        resolve_thread_sync_phase(ThreadDetailDataStatus::Cached, true),
        ThreadSyncPhase::Failed
    );
    assert_eq!(
        resolve_thread_sync_phase(ThreadDetailDataStatus::Cached, false),
        ThreadSyncPhase::Syncing
    );

    let mut stream = ThreadStreamResumeState::new(Some(42));
    assert_eq!(stream.begin_subscription(), Some(42));

    // #24583: Incremental replay dies -> silently force full snapshot without error.
    assert_eq!(
        stream.on_stream_defect("unknown event delta"),
        ThreadResumeRecovery::ImmediateFullSnapshotReload
    );
    assert!(stream.force_snapshot());
    assert_eq!(stream.error(), None);
    assert_eq!(stream.begin_subscription(), None);

    // Full reload fails once -> backs off and surfaces error, still forcing snapshot on retry.
    assert_eq!(
        stream.on_stream_defect("network closed"),
        ThreadResumeRecovery::BackoffFullSnapshotReload
    );
    assert_eq!(stream.error(), Some("network closed"));
    assert_eq!(stream.begin_subscription(), None);

    // Snapshot arrives -> clears force_snapshot & future reconnects resume from seq 50.
    stream.on_snapshot_received(50);
    assert!(!stream.force_snapshot());
    assert_eq!(stream.error(), None);
    assert_eq!(stream.begin_subscription(), Some(50));
}

#[test]
fn vcs_refs_digest_and_diff_patch_dedup() -> Result<(), Box<dyn Error>> {
    let env = EnvironmentId::new("env-a")?;
    let mut cache = VcsRefsDigestCache::new();
    assert!(cache.on_status_refresh_settled(&env, "/repo", Some("refs-a")));
    // Unchanged refs digest retains cached branch list without invalidating (#40385).
    assert!(!cache.on_status_refresh_settled(&env, "/repo", Some("refs-a")));
    // Moved ref OID invalidates (#40385).
    assert!(cache.on_status_refresh_settled(&env, "/repo", Some("refs-b")));
    // Unreadable refs invalidates cleanly.
    assert!(cache.on_status_refresh_settled(&env, "/repo", None));

    // #40217: Diff panel single-flight deduplication keeps prior revision while patch overlaps preview.
    let mut rev = PreviewRevisionState::default();
    assert_eq!(
        rev.step(
            false,
            Some("rev-1"),
            100,
            PatchReadSnapshot {
                waiting: false,
                success_timestamp_ms: Some(90),
            }
        ),
        Some("rev-1")
    );
    assert_eq!(
        rev.step(
            true,
            Some("rev-1"),
            200,
            PatchReadSnapshot {
                waiting: true,
                success_timestamp_ms: None,
            }
        ),
        Some("rev-1")
    );
    assert_eq!(
        rev.step(
            false,
            Some("rev-2"),
            250,
            PatchReadSnapshot {
                waiting: false,
                success_timestamp_ms: Some(210),
            }
        ),
        Some("rev-1")
    );
    Ok(())
}

#[test]
fn acp_effort_metadata_selection() -> Result<(), Box<dyn Error>> {
    // #40136: Recognize `thought_level` and `role: "reasoning"` descriptors.
    let descriptors = vec![
        ProviderOptionDescriptor {
            id: "thought_level".to_owned(),
            option_type: "select".to_owned(),
            role: Some("reasoning".to_owned()),
            options: vec![
                ProviderOptionChoice {
                    id: "medium".to_owned(),
                    label: "Medium".to_owned(),
                    is_default: true,
                },
                ProviderOptionChoice {
                    id: "high".to_owned(),
                    label: "High".to_owned(),
                    is_default: false,
                },
            ],
        },
        ProviderOptionDescriptor {
            id: "custom_thinking_mode".to_owned(),
            option_type: "select".to_owned(),
            role: Some("reasoning".to_owned()),
            options: vec![ProviderOptionChoice {
                id: "deep".to_owned(),
                label: "Deep".to_owned(),
                is_default: false,
            }],
        },
    ];
    let selection = ModelSelection {
        instance_id: ProviderInstanceId::new("antigravity")?,
        model: "gemini-4-neon".to_owned(),
        options: Some(vec![
            ProviderOptionSelection {
                id: "thought_level".to_owned(),
                value: ProviderOptionValue::Text("high".to_owned()),
            },
            ProviderOptionSelection {
                id: "custom_thinking_mode".to_owned(),
                value: ProviderOptionValue::Text("deep".to_owned()),
            },
        ]),
    };
    assert_eq!(
        read_selected_effort_value(Some(&selection), &descriptors).as_deref(),
        Some("high")
    );
    assert_eq!(
        format_model_selection_effort(Some(&selection), &descriptors).as_deref(),
        Some("High")
    );
    let custom_only = ModelSelection {
        instance_id: ProviderInstanceId::new("acpRegistry")?,
        model: "acp-custom".to_owned(),
        options: Some(vec![ProviderOptionSelection {
            id: "custom_thinking_mode".to_owned(),
            value: ProviderOptionValue::Text("deep".to_owned()),
        }]),
    };
    assert_eq!(
        format_model_selection_effort(Some(&custom_only), &descriptors).as_deref(),
        Some("Deep")
    );
    Ok(())
}
