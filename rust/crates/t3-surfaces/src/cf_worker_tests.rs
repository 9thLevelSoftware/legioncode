//! Unit tests for `CloudflareWorkerRouter::dispatch_with_bindings`, host effects,
//! preloaded bindings, and scheduled cron dispatching.

use t3_edge::cf_bindings::{
    CLOUDFLARE_UPSTREAM_API_KEY, CronScheduleKind, D1DatabaseBinding, HookInboxDurableActor,
    HyperdriveTable, MAX_GC_BATCH_SIZE, R2BindingOp, R2BucketBinding, SAVINGS_RETENTION_MS,
};
use t3_edge::relay_hooks::HeldHook;
use t3_edge::task_cache::{
    CompletedPart, SummaryRecord, TaskSavingsEntry, TaskSavingsSource, validate_complete_parts,
};

use super::{
    BindingReadResults, CacheFetchPayload, CacheStorePayload, CloudflareWorkerRouter,
    EdgeHostEffect, HookOutcomePayload, PreloadedCacheHit, WakePayload, WorkerHttpRequest,
};

fn store_payload(key: &str) -> CacheStorePayload {
    CacheStorePayload {
        key: key.to_owned(),
        key_is_new: true,
        association_is_new: false,
        actual_bytes: 256,
        reserved_bytes: 256,
        retired_bytes: None,
    }
}

#[test]
fn dispatch_with_bindings_emits_d1_r2_rate_limit_and_metric_effects_for_cache_flow()
-> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();

    // 1. Preloaded rate limit rejection blocks request with 429 and RateLimitCheck effect.
    let limited = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/projects/t3code/store", 1_000)
            .with_json(&store_payload("cache-k1"))
            .map_err(|e| e.to_string())?,
        Some(&BindingReadResults {
            rate_limit_allowed: Some(false),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(limited.response.status, 429);
    assert!(limited.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::RateLimitCheck { binding, .. } if binding == "STORE_LIMITER"
    )));

    // 2. Store emits RateLimitCheck + R2Op::Put(ARTIFACTS) + D1Query(INDEX upsert) + EmitMetric.
    let stored = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/projects/t3code/store", 1_050)
            .with_json(&store_payload("cache-k1"))
            .map_err(|e| e.to_string())?,
        None,
    );
    assert_eq!(stored.response.status, 200);
    assert!(stored.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::Put {
                bucket: R2BucketBinding::Artifacts,
                ..
            }
        }
    )));
    assert!(stored.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::D1Query { plan }
            if plan.database == D1DatabaseBinding::Index && plan.sql.contains("INSERT OR REPLACE")
    )));
    assert!(stored.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::EmitMetric { name, .. } if name == "task_cache.store"
    )));

    // 3. Fetch with preloaded D1 hit emits D1Query(INDEX) + R2Op::Get(ARTIFACTS) + EmitMetric.
    let fetched = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/projects/t3code/fetch", 2_000)
            .with_json(&CacheFetchPayload {
                key: "preloaded-hit".to_owned(),
                rows_read: 2,
                fallback: false,
            })
            .map_err(|e| e.to_string())?,
        Some(&BindingReadResults {
            d1_index_hit: Some(Ok(Some(PreloadedCacheHit {
                bytes: 4096,
                fallback: false,
            }))),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(fetched.response.status, 200);
    let body: serde_json::Value = fetched.response.json_body().map_err(|e| e.to_string())?;
    assert_eq!(body["hit"].as_bool(), Some(true));
    assert_eq!(body["bytes"].as_u64(), Some(4096));
    assert!(fetched.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::Get {
                bucket: R2BucketBinding::Artifacts,
                ..
            }
        }
    )));
    Ok(())
}

#[test]
fn dispatch_with_bindings_emits_savings_d1_effects_on_telemetry() -> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();
    let tel = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/projects/t3code/telemetry", 3_000)
            .with_json(&SummaryRecord {
                id: "s".repeat(64),
                recorded_at_ms: 3_000,
                developer: "dev-1".to_owned(),
                host: "h1".to_owned(),
                platform: "linux".to_owned(),
                arch: "x64".to_owned(),
                ci: false,
                tasks: vec![TaskSavingsEntry {
                    package: "web".to_owned(),
                    task: "build".to_owned(),
                    source: TaskSavingsSource::Remote,
                    saved_ms: 500,
                }],
            })
            .map_err(|e| e.to_string())?,
        None,
    );
    assert_eq!(tel.response.status, 204);
    let savings_queries = tel
        .effects
        .iter()
        .filter(|e| matches!(e, EdgeHostEffect::D1Query { plan } if plan.database == D1DatabaseBinding::Savings))
        .count();
    assert_eq!(savings_queries, 2);
    Ok(())
}

#[test]
fn dispatch_with_bindings_covers_hyperdrive_bug32_and_hook_durable_object_bug29()
-> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();

    // Preloaded Hyperdrive transient error keeps checked_at_ms unset (BUG-32) + emits query.
    let err_eval = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/v1/internal/cloudflare-pause/evaluate", 10_000),
        Some(&BindingReadResults {
            hyperdrive_paused_until_ms: Some(Err("pg timeout".to_owned())),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(err_eval.response.status, 200);
    assert_eq!(router.pause_gate().checked_at_ms(), None);
    assert!(err_eval.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::HyperdriveQuery { plan }
            if plan.table == HyperdriveTable::UpstreamApiPauses
                && plan.params == vec![serde_json::json!(CLOUDFLARE_UPSTREAM_API_KEY)]
    )));

    // Preloaded Hyperdrive pause sets 503 + Retry-After and records checked_at_ms.
    let paused_eval = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/v1/internal/cloudflare-pause/evaluate", 10_500),
        Some(&BindingReadResults {
            hyperdrive_paused_until_ms: Some(Ok(Some(25_500))),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(paused_eval.response.status, 503);
    assert_eq!(router.pause_gate().checked_at_ms(), Some(10_500));

    // Preloaded Durable Object actor + BUG-29 wake & stale outcome fencing.
    let mut seeded = HookInboxDurableActor::new("env-seeded");
    seeded
        .hold(
            HeldHook::new("d1", "h1", 500, "POST", b"{}".to_vec()),
            "https://t1.invalid",
            500,
        )
        .map_err(|e| e.to_string())?;
    let gen1 = seeded.target_generation();

    let wake = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/v1/internal/hooks/env-seeded/wake", 1_200)
            .with_json(&WakePayload {
                base_url: "https://t2.invalid".to_owned(),
            })
            .map_err(|e| e.to_string())?,
        Some(&BindingReadResults {
            hook_actor: Some(seeded),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(wake.response.status, 200);
    assert!(wake.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::DurableObjectCall {
            method,
            actor: Some(actor),
            ..
        } if method == "wake" && actor.alarm_at_ms() == Some(1_200) && actor.target_generation() == gen1 + 1
    )));

    let stale = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("POST", "/v1/internal/hooks/env-seeded/outcome", 1_300)
            .with_json(&HookOutcomePayload {
                snapshot_generation: gen1,
                hook_id: "d1".to_owned(),
                status: Some(503),
            })
            .map_err(|e| e.to_string())?,
        None,
    );
    let stale_json: serde_json::Value = stale.response.json_body().map_err(|e| e.to_string())?;
    assert_eq!(stale_json["ignoredStale"].as_bool(), Some(true));
    assert_eq!(
        router
            .hook_actor("env-seeded")
            .map(|a| a.state().target().map_or(99, |t| t.failures)),
        Some(0)
    );
    Ok(())
}

#[test]
fn dispatch_with_bindings_covers_native_client_r2_ops() -> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new().with_gc_batch_size(128);
    let create = router.dispatch_with_bindings(
        &WorkerHttpRequest::new(
            "POST",
            "/projects/t3code/native-clients/ios/dev/0123456789abcdef0123456789abcdef01234567/uploads",
            1_000,
        )
        .with_header("x-t3-digest", "a".repeat(64)),
        None,
    );
    assert_eq!(create.response.status, 200);
    assert!(create.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::CreateMultipartUpload {
                bucket: R2BucketBinding::Clients,
                ..
            }
        }
    )));

    let put = router.dispatch_with_bindings(
        &WorkerHttpRequest::new(
            "PUT",
            "/projects/t3code/native-clients/ios/dev/fp1/uploads/up1/1",
            1_010,
        )
        .with_body(vec![7, 8, 9]),
        None,
    );
    assert_eq!(put.response.status, 200);
    assert!(put.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::UploadPart {
                bucket: R2BucketBinding::Clients,
                part_number: 1,
                ..
            }
        }
    )));

    let part = CompletedPart {
        part_number: 1,
        etag: "etag-1-3".to_owned(),
    };
    validate_complete_parts(std::slice::from_ref(&part)).map_err(|e| e.to_string())?;
    let complete = router.dispatch_with_bindings(
        &WorkerHttpRequest::new(
            "POST",
            "/projects/t3code/native-clients/ios/dev/fp1/uploads/up1/complete",
            1_020,
        )
        .with_json(&serde_json::json!({ "parts": [part] }))
        .map_err(|e| e.to_string())?,
        None,
    );
    assert_eq!(complete.response.status, 204);
    assert!(complete.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::CompleteMultipartUpload {
                bucket: R2BucketBinding::Clients,
                ..
            }
        }
    )));

    let abort = router.dispatch_with_bindings(
        &WorkerHttpRequest::new(
            "DELETE",
            "/projects/t3code/native-clients/ios/dev/fp1/uploads/up2",
            1_030,
        ),
        None,
    );
    assert_eq!(abort.response.status, 204);
    assert!(abort.effects.iter().any(|e| matches!(
        e,
        EdgeHostEffect::R2Op {
            op: R2BindingOp::AbortMultipartUpload {
                bucket: R2BucketBinding::Clients,
                ..
            }
        }
    )));

    let dl = router.dispatch_with_bindings(
        &WorkerHttpRequest::new("GET", "/projects/t3code/native-clients/ios/dev/fp1", 1_040),
        Some(&BindingReadResults {
            r2_object_bytes: Some(vec![4, 5, 6]),
            ..BindingReadResults::default()
        }),
    );
    assert_eq!(dl.response.status, 200);
    assert_eq!(dl.response.body, vec![4, 5, 6]);
    Ok(())
}

#[test]
fn dispatch_scheduled_runs_minutely_gc_and_hourly_savings() {
    let mut router = CloudflareWorkerRouter::new().with_gc_batch_size(128);
    let gc = router.dispatch_scheduled("* * * * *", 60_000);
    assert!(gc.recognized);
    assert_eq!(gc.schedule_kind, CronScheduleKind::EveryMinuteCacheGc);
    assert_eq!(gc.gc_batch_size, 128.min(MAX_GC_BATCH_SIZE));

    let hourly = router.dispatch_scheduled("0 * * * *", SAVINGS_RETENTION_MS + 12_000);
    assert!(hourly.recognized);
    assert_eq!(
        hourly.schedule_kind,
        CronScheduleKind::HourlySavingsMaintenance
    );
    assert_eq!(hourly.retention_cutoff_ms, Some(12_000));
}
