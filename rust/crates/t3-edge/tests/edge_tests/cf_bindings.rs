//! Integration and property tests for `t3_edge::cf_bindings`.

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::json;
use t3_edge::cf_bindings::{
    CLOUDFLARE_UPSTREAM_API_KEY, CronScheduleKind, D1BindingPlan, D1DatabaseBinding,
    GC_CLAIM_LEASE_SECONDS, HookInboxDurableActor, HyperdriveRelayPlan, HyperdriveTable,
    MAX_GC_BATCH_SIZE, R2BindingOp, R2BucketBinding, SAVINGS_RETENTION_MS,
    apply_upstream_pause_query_result, run_scheduled_cron,
};
use t3_edge::relay_hooks::{
    BUSY_RETRY_MS, CloudflareApiPauseGate, DeliveryOutcome, DeliveryRecordOutcome, HeldHook,
    RUN_FAILURE_RETRY_MS,
};
use t3_edge::task_cache::{
    CompletedPart, MAX_MULTIPART_PARTS, PartValidationError, SummaryRecord, TaskSavingsEntry,
    TaskSavingsSource,
};

#[test]
fn d1_binding_plans_cover_index_queries() {
    let sel = D1BindingPlan::select_artifact_entry("t3code", "aa11", "bb22", 1_700_000_000);
    assert_eq!(sel.database, D1DatabaseBinding::Index);
    assert_eq!(sel.database.as_str(), "INDEX");
    assert!(sel.sql.contains("SELECT"));
    assert!(sel.sql.contains("UNION ALL"));
    assert_eq!(
        sel.params,
        vec![
            json!("t3code"),
            json!("aa11"),
            json!("bb22"),
            json!(1_700_000_000)
        ]
    );

    let upsert = D1BindingPlan::upsert_artifact_entry("t3code", "aa11", "gen-1");
    assert_eq!(upsert.database, D1DatabaseBinding::Index);
    assert!(upsert.sql.starts_with("INSERT OR REPLACE INTO entries"));
    assert_eq!(
        upsert.params,
        vec![json!("t3code"), json!("aa11"), json!("gen-1")]
    );

    let claim = D1BindingPlan::claim_expired_gc_batch("claim-1", 10_000, 9_999);
    assert_eq!(claim.database, D1DatabaseBinding::Index);
    assert_eq!(
        claim.params,
        vec![
            json!("claim-1"),
            json!(10_000 + GC_CLAIM_LEASE_SECONDS),
            json!(10_000),
            json!(MAX_GC_BATCH_SIZE),
        ]
    );

    let gc_del = D1BindingPlan::delete_expired_gc_batch(10_000, 0);
    assert_eq!(gc_del.database, D1DatabaseBinding::Index);
    assert!(gc_del.sql.starts_with("DELETE FROM generations"));
    assert_eq!(gc_del.params, vec![json!(10_000), json!(1)]);
}

#[test]
fn d1_binding_plans_cover_savings_queries() {
    let summary = SummaryRecord {
        id: "r1".to_owned(),
        recorded_at_ms: 50_000,
        developer: "dev-a".to_owned(),
        host: "mbp".to_owned(),
        platform: "darwin".to_owned(),
        arch: "arm64".to_owned(),
        ci: true,
        tasks: vec![
            TaskSavingsEntry {
                package: "web".to_owned(),
                task: "build".to_owned(),
                source: TaskSavingsSource::Local,
                saved_ms: 120,
            },
            TaskSavingsEntry {
                package: "server".to_owned(),
                task: "check".to_owned(),
                source: TaskSavingsSource::Remote,
                saved_ms: 300,
            },
            TaskSavingsEntry {
                package: "shared".to_owned(),
                task: "lint".to_owned(),
                source: TaskSavingsSource::Unspecified,
                saved_ms: 45,
            },
            TaskSavingsEntry {
                package: "cli".to_owned(),
                task: "test".to_owned(),
                source: TaskSavingsSource::Executed,
                saved_ms: 0,
            },
            TaskSavingsEntry {
                package: "mobile".to_owned(),
                task: "typecheck".to_owned(),
                source: TaskSavingsSource::Failed,
                saved_ms: 0,
            },
        ],
    };
    let rollup = D1BindingPlan::upsert_savings_rollup(&summary, "dev-a");
    assert_eq!(rollup.database, D1DatabaseBinding::Savings);
    assert_eq!(rollup.database.as_str(), "SAVINGS");
    assert!(rollup.sql.contains("INSERT INTO savings"));
    assert_eq!(
        rollup.params,
        vec![
            json!("dev-a"),
            json!("mbp"),
            json!("darwin"),
            json!("arm64"),
            json!(1),
            json!(120),
            json!(300),
            json!(1),
            json!(1),
            json!(1),
            json!(1),
            json!(45),
            json!(1),
        ]
    );

    let prune = D1BindingPlan::delete_expired_savings_receipts(SAVINGS_RETENTION_MS + 999);
    assert_eq!(prune.database, D1DatabaseBinding::Savings);
    assert_eq!(prune.params, vec![json!(999)]);

    let sum_all = D1BindingPlan::select_savings_sum(None);
    assert_eq!(sum_all.database, D1DatabaseBinding::Savings);
    assert!(sum_all.sql.contains("SUM(local_ms)"));
    assert_eq!(sum_all.params, Vec::<serde_json::Value>::new());

    let sum_dev = D1BindingPlan::select_savings_sum(Some("dev-a"));
    assert_eq!(sum_dev.params, vec![json!("dev-a")]);
}

#[test]
fn r2_binding_ops_enforce_multipart_limits_for_artifacts_and_clients() -> Result<(), String> {
    let get = R2BindingOp::get(R2BucketBinding::Artifacts, "t3code/g1/blob");
    assert_eq!(get.bucket(), R2BucketBinding::Artifacts);
    assert_eq!(get.bucket().as_str(), "ARTIFACTS");
    get.validate().map_err(|e| e.to_string())?;

    let mut meta = BTreeMap::new();
    meta.insert("digest".to_owned(), "a".repeat(64));
    let create = R2BindingOp::create_multipart_upload(R2BucketBinding::Clients, "ios/dev/fp", meta);
    assert_eq!(create.bucket().as_str(), "CLIENTS");

    for bad in [0, 65, 99, u32::MAX] {
        let err =
            R2BindingOp::upload_part(R2BucketBinding::Clients, "ios/dev/fp", "u1", bad, vec![9])
                .expect_err("invalid part rejected");
        assert!(matches!(err, PartValidationError::InvalidPartNumber { part, .. } if part == bad));
        assert_eq!(err.http_status(), 400);
        assert_eq!(err.error_code(), "invalid_part_number");
    }

    let ok_part = R2BindingOp::upload_part(
        R2BucketBinding::Clients,
        "ios/dev/fp",
        "u1",
        MAX_MULTIPART_PARTS,
        vec![1, 2, 3],
    )
    .map_err(|e| e.to_string())?;
    ok_part.validate().map_err(|e| e.to_string())?;

    let complete = R2BindingOp::complete_multipart_upload(
        R2BucketBinding::Clients,
        "ios/dev/fp",
        "u1",
        vec![CompletedPart {
            part_number: 64,
            etag: "etag-64".to_owned(),
        }],
    )
    .map_err(|e| e.to_string())?;
    assert!(complete.validate().is_ok());

    let abort = R2BindingOp::abort_multipart_upload(R2BucketBinding::Clients, "ios/dev/fp", "u1");
    assert!(abort.validate().is_ok());
    Ok(())
}

#[test]
fn hyperdrive_relay_plans_and_pause_gate_error_handling_preserve_bug32() -> Result<(), String> {
    let link = HyperdriveRelayPlan::get_environment_link("user-1", "env-1");
    assert_eq!(link.table, HyperdriveTable::EnvironmentLinks);
    assert_eq!(link.table.table_name(), "relay_environment_links");
    assert!(link.sql.contains("$1"));

    let alloc = HyperdriveRelayPlan::claim_managed_endpoint_deprovision("user-1", "env-1", 4);
    assert_eq!(alloc.table, HyperdriveTable::ManagedEndpointAllocations);
    assert_eq!(
        alloc.params,
        vec![json!("user-1"), json!("env-1"), json!(4)]
    );

    let pause_read = HyperdriveRelayPlan::read_upstream_api_pause(CLOUDFLARE_UPSTREAM_API_KEY);
    assert_eq!(pause_read.table, HyperdriveTable::UpstreamApiPauses);
    assert_eq!(pause_read.params, vec![json!("cloudflare")]);

    let pause_write =
        HyperdriveRelayPlan::record_upstream_api_pause(CLOUDFLARE_UPSTREAM_API_KEY, 42_000);
    assert!(pause_write.sql.contains("GREATEST"));

    let mut gate = CloudflareApiPauseGate::new();
    assert!(apply_upstream_pause_query_result(&mut gate, 10_000, Err("pg timeout")).is_err());
    assert_eq!(gate.checked_at_ms(), None);
    assert!(gate.should_refresh(10_001));

    let applied = apply_upstream_pause_query_result::<&str>(&mut gate, 10_050, Ok(Some(20_000)))
        .map_err(str::to_owned)?;
    assert_eq!(applied, 20_000);
    assert_eq!(gate.checked_at_ms(), Some(10_050));
    assert!(!gate.should_refresh(12_000));
    Ok(())
}

#[test]
fn hook_inbox_durable_actor_tracks_alarms_serialization_and_bug29_fencing() -> Result<(), String> {
    let mut actor = HookInboxDurableActor::new("env-abc");
    let hook = HeldHook::new("del-1", "hook-1", 1_000, "POST", b"hello".to_vec());
    assert_eq!(
        actor.hold(hook, "https://old.tunnel.test", 1_000),
        Ok(11_000)
    );
    assert_eq!(actor.alarm_at_ms(), Some(11_000));
    assert!(!actor.is_alarm_due(10_999));
    assert!(actor.is_alarm_due(11_000));

    let batch = actor.begin_alarm_batch(11_000).ok_or("expected batch")?;
    assert!(actor.wake("https://new.tunnel.test", 11_100));
    assert_eq!(actor.alarm_at_ms(), Some(11_100));

    let json = serde_json::to_string(&actor).map_err(|e| e.to_string())?;
    let mut restored: HookInboxDurableActor =
        serde_json::from_str(&json).map_err(|e| e.to_string())?;
    assert_eq!(restored.target_generation(), 2);

    let stale = restored.record_delivery_outcome(
        batch.target_generation,
        "del-1",
        DeliveryOutcome::Unreachable,
        12_000,
    );
    assert!(matches!(stale, DeliveryRecordOutcome::IgnoredStale { .. }));
    assert_eq!(restored.alarm_at_ms(), Some(11_100));

    // A failure recovery backoff must not delay the earlier concurrent wake alarm at 11_100.
    assert_eq!(restored.record_run_failure(20_000), 11_100);
    // Consuming the wake alarm clears the slot so retry/run-failure schedules fresh timestamps.
    assert!(restored.begin_alarm_batch(11_100).is_some());
    assert_eq!(restored.alarm_at_ms(), None);
    assert_eq!(
        restored.record_run_failure(20_000),
        20_000 + RUN_FAILURE_RETRY_MS
    );
    assert!(restored.begin_alarm_batch(80_000).is_some());
    assert_eq!(
        restored.finish_alarm_batch(0, 1, 21_000),
        Some(21_000 + BUSY_RETRY_MS)
    );
    assert_eq!(restored.alarm_at_ms(), Some(21_000 + BUSY_RETRY_MS));

    let delivered = restored.record_delivery_outcome(
        restored.target_generation(),
        "del-1",
        DeliveryOutcome::Delivered,
        22_000,
    );
    assert_eq!(
        delivered,
        DeliveryRecordOutcome::Delivered {
            remaining_pending: false
        }
    );
    assert_eq!(restored.alarm_at_ms(), None);
    assert_eq!(restored.pending_count(), 0);
    Ok(())
}

#[test]
fn scheduled_cron_reports_cover_minutely_gc_hourly_savings_and_unknown() {
    let minutely = run_scheduled_cron("* * * * *", 120_000, 1_000);
    assert!(minutely.recognized);
    assert_eq!(minutely.schedule_kind, CronScheduleKind::EveryMinuteCacheGc);
    assert_eq!(minutely.gc_batch_size, MAX_GC_BATCH_SIZE);
    assert_eq!(minutely.d1_plans.len(), 6);
    assert!(
        minutely
            .d1_plans
            .iter()
            .all(|p| p.database == D1DatabaseBinding::Index)
    );

    let hourly = run_scheduled_cron("0 * * * *", SAVINGS_RETENTION_MS + 5_000, 64);
    assert!(hourly.recognized);
    assert_eq!(
        hourly.schedule_kind,
        CronScheduleKind::HourlySavingsMaintenance
    );
    assert_eq!(hourly.retention_cutoff_ms, Some(5_000));
    assert_eq!(hourly.d1_plans.len(), 2);
    assert!(
        hourly
            .d1_plans
            .iter()
            .all(|p| p.database == D1DatabaseBinding::Savings)
    );
    assert_eq!(hourly.d1_plans[0].params, vec![json!(5_000)]);
    assert!(hourly.d1_plans[1].sql.contains("SUM("));

    let unknown = run_scheduled_cron("*/15 * * * *", 50_000, 10);
    assert!(!unknown.recognized);
    assert_eq!(unknown.schedule_kind, CronScheduleKind::Unknown);
    assert_eq!(unknown.d1_plans.len(), 0);
}

proptest! {
    #[test]
    fn proptest_cron_batch_size_always_clamped_and_r2_part_validated(
        raw_batch in any::<u32>(),
        part in any::<u32>(),
    ) {
        let report = run_scheduled_cron("* * * * *", 90_000, raw_batch);
        prop_assert!((1..=MAX_GC_BATCH_SIZE).contains(&report.gc_batch_size));

        let op = R2BindingOp::upload_part(R2BucketBinding::Clients, "k", "u", part, vec![]);
        if (1..=MAX_MULTIPART_PARTS).contains(&part) {
            prop_assert!(op.is_ok());
        } else {
            prop_assert!(op.is_err());
        }
    }
}
