//! `CloudflareWorkerRouter` integration tests wiring `legion_edge` `relay_hooks` & `task_cache`.

use legion_edge::task_cache::{
    CompletedPart, D1BatchLedger, MAX_COMPLETE_BODY_BYTES, OIDC_JWKS_COOLDOWN_MS, SummaryRecord,
    TaskSavingsEntry, TaskSavingsSource,
};
use legion_surfaces::cf_worker::{
    CacheFetchPayload, CacheStorePayload, CloudflareWorkerRouter, HookOutcomePayload,
    PauseEvaluatePayload, RateLimitPayload, WakePayload, WorkerHttpRequest,
};

#[test]
fn worker_health_and_relay_hook_wake_and_stale_generation_flow() -> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();
    let health = router.dispatch(&WorkerHttpRequest::new("GET", "/health", 100));
    assert_eq!(health.status, 200);

    // Path traversal or > 1 MiB hook bodies are rejected at the boundary.
    assert_eq!(
        router
            .dispatch(&WorkerHttpRequest::new("POST", "/v1/hooks/env/./tok", 100))
            .status,
        404
    );
    assert_eq!(
        router
            .dispatch(
                &WorkerHttpRequest::new("POST", "/v1/hooks/env/h1/tok", 100)
                    .with_body(vec![b'x'; 1_048_577])
            )
            .status,
        413
    );

    let held = router.dispatch(
        &WorkerHttpRequest::new("POST", "/v1/hooks/env/hook-1/tok?foo=bar", 1_000)
            .with_header("x-legioncode-target-url", "https://t1.trycloudflare.com")
            .with_header("x-legioncode-delivery-id", "deliv-1")
            .with_body(b"{}".to_vec()),
    );
    assert_eq!(held.status, 202);
    let inbox = router.hook_inbox("env").ok_or("missing inbox")?;
    let gen1 = inbox.target().ok_or("missing target")?.target_generation;

    // Wake rotates to generation 2; delivery outcome stamped with stale gen1 is ignored (BUG-29).
    let wake_req = WorkerHttpRequest::new("POST", "/v1/internal/hooks/env/wake", 1_050)
        .with_json(&WakePayload {
            base_url: "https://t2.trycloudflare.com".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(router.dispatch(&wake_req).status, 200);

    let batch_resp = router.dispatch(&WorkerHttpRequest::new(
        "POST",
        "/v1/internal/hooks/env/deliver-batch",
        1_060,
    ));
    assert_eq!(batch_resp.status, 200);
    let batch_json: serde_json::Value = batch_resp.json_body().map_err(|e| e.to_string())?;
    assert_eq!(batch_json["targetGeneration"].as_u64(), Some(gen1 + 1));

    let stale_req = WorkerHttpRequest::new("POST", "/v1/internal/hooks/env/outcome", 1_070)
        .with_json(&HookOutcomePayload {
            snapshot_generation: gen1,
            hook_id: "deliv-1".to_owned(),
            status: None,
        })
        .map_err(|e| e.to_string())?;
    let stale_json: serde_json::Value = router
        .dispatch(&stale_req)
        .json_body()
        .map_err(|e| e.to_string())?;
    assert_eq!(stale_json["ignoredStale"].as_bool(), Some(true));
    assert_eq!(stale_json["failures"].as_u64(), Some(0));

    let current_ok = WorkerHttpRequest::new("POST", "/v1/internal/hooks/env/outcome", 1_080)
        .with_json(&HookOutcomePayload {
            snapshot_generation: gen1 + 1,
            hook_id: "deliv-1".to_owned(),
            status: Some(200),
        })
        .map_err(|e| e.to_string())?;
    let ok_json: serde_json::Value = router
        .dispatch(&current_ok)
        .json_body()
        .map_err(|e| e.to_string())?;
    assert_eq!(ok_json["ignoredStale"].as_bool(), Some(false));
    assert_eq!(ok_json["pendingCount"].as_u64(), Some(0));
    Ok(())
}

#[test]
fn worker_cloudflare_pause_gate_fails_open_and_enforces_pause_with_retry_after()
-> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();

    // Shared read error fails open WITHOUT advancing checked_at_ms (BUG-32).
    let fail_open_req =
        WorkerHttpRequest::new("POST", "/v1/internal/cloudflare-pause/evaluate", 10_000)
            .with_json(&PauseEvaluatePayload {
                shared_paused_until_ms: None,
                shared_read_error: Some("kv timeout".to_owned()),
            })
            .map_err(|e| e.to_string())?;
    let fail_open_resp = router.dispatch(&fail_open_req);
    assert_eq!(fail_open_resp.status, 200);
    assert_eq!(router.pause_gate().checked_at_ms(), None);

    let rl_req = WorkerHttpRequest::new("POST", "/v1/internal/cloudflare-pause/rate-limit", 10_050)
        .with_json(&RateLimitPayload {
            retry_after_ms: Some(15_000),
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(router.dispatch(&rl_req).status, 200);

    let blocked_req =
        WorkerHttpRequest::new("POST", "/v1/internal/cloudflare-pause/evaluate", 12_000)
            .with_json(&PauseEvaluatePayload {
                shared_paused_until_ms: Some(25_050),
                shared_read_error: None,
            })
            .map_err(|e| e.to_string())?;
    let blocked = router.dispatch(&blocked_req);
    assert_eq!(blocked.status, 503);
    assert_eq!(
        blocked.headers.get("retry-after").map(String::as_str),
        Some("14")
    );
    Ok(())
}

#[test]
fn worker_native_clients_validate_parts_put_and_complete() -> Result<(), String> {
    let mut router = CloudflareWorkerRouter::new();

    // BUG-30: multipart PUT immediately rejects 0, 65, 99 with 400 invalid_part_number.
    for bad in ["0", "65", "99", "abc"] {
        let resp = router.dispatch(&WorkerHttpRequest::new(
            "PUT",
            format!("/projects/legioncode/native-clients/ios/dev/fp1/uploads/up1/{bad}"),
            1_000,
        ));
        assert_eq!(resp.status, 400);
        let body: serde_json::Value = resp.json_body().map_err(|e| e.to_string())?;
        assert_eq!(body["error"].as_str(), Some("invalid_part_number"));
    }

    let put1 = router.dispatch(
        &WorkerHttpRequest::new(
            "PUT",
            "/projects/legioncode/native-clients/ios/dev/fp1/uploads/up1/1",
            1_000,
        )
        .with_body(vec![1, 2, 3]),
    );
    assert_eq!(put1.status, 200);
    let put_json: serde_json::Value = put1.json_body().map_err(|e| e.to_string())?;
    let etag = put_json["etag"].as_str().ok_or("missing etag")?.to_owned();

    // Complete rejects > MAX_COMPLETE_BODY_BYTES and completes cleanly with valid part.
    assert_eq!(
        router
            .dispatch(
                &WorkerHttpRequest::new(
                    "POST",
                    "/projects/legioncode/native-clients/ios/dev/fp1/uploads/up1/complete",
                    1_010
                )
                .with_body(vec![b' '; MAX_COMPLETE_BODY_BYTES + 1])
            )
            .status,
        413
    );
    let complete_req = WorkerHttpRequest::new(
        "POST",
        "/projects/legioncode/native-clients/ios/dev/fp1/uploads/up1/complete",
        1_020,
    )
    .with_json(&serde_json::json!({
        "parts": [CompletedPart { part_number: 1, etag }]
    }))
    .map_err(|e| e.to_string())?;
    assert_eq!(router.dispatch(&complete_req).status, 204);
    Ok(())
}

#[test]
fn worker_task_cache_store_fetch_telemetry_and_oidc_cooldown() -> Result<(), String> {
    let mut router =
        CloudflareWorkerRouter::new().with_ledger(D1BatchLedger::new(10_000, 100, 100));

    // Store + fetch + telemetry D1 accounting.
    let store_req = WorkerHttpRequest::new("POST", "/projects/legioncode/store", 2_000)
        .with_json(&CacheStorePayload {
            key: "k1".to_owned(),
            key_is_new: true,
            association_is_new: false,
            actual_bytes: 512,
            reserved_bytes: 600,
            retired_bytes: None,
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(router.dispatch(&store_req).status, 200);
    assert_eq!(router.ledger().charged_bytes(), 512);

    let fetch_req = WorkerHttpRequest::new("POST", "/projects/legioncode/fetch", 2_010)
        .with_json(&CacheFetchPayload {
            key: "k1".to_owned(),
            rows_read: 1,
            fallback: false,
        })
        .map_err(|e| e.to_string())?;
    let fetch_json: serde_json::Value = router
        .dispatch(&fetch_req)
        .json_body()
        .map_err(|e| e.to_string())?;
    assert_eq!(fetch_json["hit"].as_bool(), Some(true));
    assert_eq!(fetch_json["bytes"].as_u64(), Some(512));

    let tel_req = WorkerHttpRequest::new("POST", "/projects/legioncode/telemetry", 2_020)
        .with_json(&SummaryRecord {
            id: "a".repeat(64),
            recorded_at_ms: 2_020,
            developer: "dev-1".to_owned(),
            host: "host-1".to_owned(),
            platform: "darwin".to_owned(),
            arch: "arm64".to_owned(),
            ci: false,
            tasks: vec![TaskSavingsEntry {
                package: "web".to_owned(),
                task: "build".to_owned(),
                source: TaskSavingsSource::Remote,
                saved_ms: 420,
            }],
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(router.dispatch(&tel_req).status, 204);

    // OIDC cooldown returns 503 with retry-after during the 30s window.
    assert_eq!(
        router
            .dispatch(&WorkerHttpRequest::new(
                "POST",
                "/projects/legioncode/oidc/refresh",
                10_000
            ))
            .status,
        200
    );
    let cooled = router.dispatch(&WorkerHttpRequest::new(
        "POST",
        "/projects/legioncode/oidc/refresh",
        10_000 + OIDC_JWKS_COOLDOWN_MS - 1,
    ));
    assert_eq!(cooled.status, 503);
    assert_eq!(
        cooled.headers.get("retry-after").map(String::as_str),
        Some("1")
    );
    Ok(())
}
