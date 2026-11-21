//! Tests for [`t3_wasm_ffi::bridge::EdgeWorkerBridge`] backing `WasmEdgeWorker`.

use serde_json::Value;
use t3_wasm_ffi::bridge::EdgeWorkerBridge;

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn edge_worker_handles_hooks_wake_and_stale_outcome_via_shorthand_and_canonical_paths()
-> Result<(), String> {
    let mut worker = EdgeWorkerBridge::new();

    let health = parse_json(&worker.handle_request_json(r#"{"path":"/health"}"#)?)?;
    assert_eq!(health["status"], 200);
    assert_eq!(health["body"]["ok"], true);

    // Hold a hook via canonical path, then wake + outcome via shorthand /hooks/wake & /hooks/outcome.
    let held = parse_json(&worker.handle_request_json(
        r#"{
            "method": "POST",
            "path": "/v1/hooks/env-1/hook-1/tok?src=gh",
            "headers": {
                "x-t3-target-url": "https://t1.trycloudflare.com",
                "x-t3-delivery-id": "deliv-1"
            },
            "body": {"event": "push"},
            "nowMs": 1000
        }"#,
    )?)?;
    assert_eq!(held["status"], 202);
    assert_eq!(held["body"]["targetGeneration"], 1);

    let wake = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/hooks/wake",
            "endpointKey": "env-1",
            "body": {"baseUrl": "https://t2.trycloudflare.com"},
            "nowMs": 1050
        }"#,
    )?)?;
    assert_eq!(wake["status"], 200);
    assert_eq!(wake["body"]["pending"], true);
    assert_eq!(wake["body"]["targetGeneration"], 2);

    // Stale generation 1 outcome must be ignored (BUG-29).
    let stale_outcome = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/hooks/outcome",
            "endpointKey": "env-1",
            "body": {"snapshotGeneration": 1, "hookId": "deliv-1", "status": null},
            "nowMs": 1070
        }"#,
    )?)?;
    assert_eq!(stale_outcome["status"], 200);
    assert_eq!(stale_outcome["body"]["ignoredStale"], true);
    assert_eq!(stale_outcome["body"]["failures"], 0);

    // Current generation 2 delivery succeeds and drains inbox.
    let ok_outcome = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/hooks/outcome",
            "endpointKey": "env-1",
            "body": {"snapshotGeneration": 2, "hookId": "deliv-1", "status": 200},
            "nowMs": 1080
        }"#,
    )?)?;
    assert_eq!(ok_outcome["status"], 200);
    assert_eq!(ok_outcome["body"]["ignoredStale"], false);
    assert_eq!(ok_outcome["body"]["pendingCount"], 0);
    Ok(())
}

#[test]
fn edge_worker_handles_pause_evaluate_and_cache_store_fetch_and_multipart_bounds()
-> Result<(), String> {
    let mut worker = EdgeWorkerBridge::new();

    // BUG-32: shared read error fails open WITHOUT advancing checkedAtMs.
    let fail_open = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/pause/evaluate",
            "body": {"sharedPausedUntilMs": null, "sharedReadError": "kv timeout"},
            "nowMs": 10000
        }"#,
    )?)?;
    assert_eq!(fail_open["status"], 200);
    assert_eq!(fail_open["body"]["checkedAtMs"], Value::Null);

    let rate_limit = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/pause/rate-limit",
            "body": {"retryAfterMs": 15000},
            "nowMs": 10050
        }"#,
    )?)?;
    assert_eq!(rate_limit["status"], 200);

    let paused = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/pause/evaluate",
            "body": {"sharedPausedUntilMs": 25050, "sharedReadError": null},
            "nowMs": 12000
        }"#,
    )?)?;
    assert_eq!(paused["status"], 503);
    assert_eq!(paused["headers"]["retry-after"], "14");

    // Task cache store & fetch via shorthand /cache/store & /cache/fetch.
    let store = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/cache/store",
            "body": {
                "key": "artifact-1",
                "keyIsNew": true,
                "associationIsNew": false,
                "actualBytes": 512,
                "reservedBytes": 600,
                "retiredBytes": null
            },
            "nowMs": 2000
        }"#,
    )?)?;
    assert_eq!(store["status"], 200);
    assert_eq!(store["body"]["chargedBytes"], 512);

    let hit = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/cache/fetch",
            "body": {"key": "artifact-1", "rowsRead": 1, "fallback": false},
            "nowMs": 2010
        }"#,
    )?)?;
    assert_eq!(hit["status"], 200);
    assert_eq!(hit["body"]["hit"], true);
    assert_eq!(hit["body"]["bytes"], 512);
    assert_eq!(hit["body"]["hits"], 1);

    let miss = parse_json(&worker.handle_request_json(
        r#"{
            "path": "/cache/fetch",
            "body": {"key": "missing", "rowsRead": 1, "fallback": false},
            "nowMs": 2020
        }"#,
    )?)?;
    assert_eq!(miss["status"], 200);
    assert_eq!(miss["body"]["hit"], false);
    assert_eq!(miss["body"]["misses"], 1);

    // BUG-30: multipart part 65 is rejected immediately at PUT.
    let bad_part = parse_json(&worker.handle_request_json(
        r#"{
            "method": "PUT",
            "path": "/projects/t3code/native-clients/ios/dev/fp1/uploads/up1/65",
            "nowMs": 2030
        }"#,
    )?)?;
    assert_eq!(bad_part["status"], 400);
    assert_eq!(bad_part["body"]["error"], "invalid_part_number");

    assert!(worker.handle_request_json(r#"{"path":""}"#).is_err());
    Ok(())
}

#[test]
fn edge_worker_dispatch_with_bindings_plans_d1_r2_hyperdrive_and_scheduled_cron()
-> Result<(), String> {
    let mut worker = EdgeWorkerBridge::new();

    // Preloaded rate limit rejection returns 429 and RateLimitCheck effect.
    let limited = parse_json(&worker.dispatch_with_bindings_json(
        r#"{
            "path": "/cache/store",
            "body": {
                "key": "k-rate",
                "keyIsNew": true,
                "associationIsNew": false,
                "actualBytes": 256,
                "reservedBytes": 256,
                "retiredBytes": null
            },
            "preloaded": { "rateLimitAllowed": false },
            "nowMs": 1000
        }"#,
    )?)?;
    assert_eq!(limited["response"]["status"], 429);
    assert_eq!(limited["effects"][0]["kind"], "rateLimitCheck");
    assert_eq!(limited["effects"][0]["binding"], "STORE_LIMITER");

    // Preloaded D1 index hit on /cache/fetch emits D1Query + R2Op::Get + EmitMetric.
    let fetched = parse_json(&worker.dispatch_with_bindings_json(
        r#"{
            "path": "/cache/fetch",
            "body": { "key": "preloaded-k", "rowsRead": 2, "fallback": false },
            "preloaded": { "d1IndexHit": { "bytes": 4096, "fallback": false } },
            "nowMs": 2000
        }"#,
    )?)?;
    assert_eq!(fetched["response"]["status"], 200);
    assert_eq!(fetched["response"]["body"]["hit"], true);
    assert_eq!(fetched["response"]["body"]["bytes"], 4096);
    let effects = fetched["effects"].as_array().ok_or("missing effects")?;
    assert!(effects.iter().any(|e| e["kind"] == "d1Query"));
    assert!(effects.iter().any(|e| e["kind"] == "r2Op"));

    // Preloaded Hyperdrive transient error fails open without advancing checkedAtMs (BUG-32).
    let pause_err = parse_json(&worker.dispatch_with_bindings_json(
        r#"{
            "path": "/pause/evaluate",
            "preloaded": { "hyperdriveReadError": "pg timeout" },
            "nowMs": 10000
        }"#,
    )?)?;
    assert_eq!(pause_err["response"]["status"], 200);
    assert_eq!(pause_err["response"]["body"]["checkedAtMs"], Value::Null);
    assert_eq!(pause_err["effects"][0]["kind"], "hyperdriveQuery");

    // Preloaded Hyperdrive pause sets 503 + Retry-After header.
    let paused = parse_json(&worker.dispatch_with_bindings_json(
        r#"{
            "path": "/pause/evaluate",
            "preloaded": { "hyperdrivePausedUntilMs": 25000 },
            "nowMs": 11000
        }"#,
    )?)?;
    assert_eq!(paused["response"]["status"], 503);
    assert_eq!(paused["response"]["headers"]["retry-after"], "14");

    // Scheduled cron triggers ("* * * * *" and "0 * * * *").
    let minutely = parse_json(&worker.handle_scheduled_json("* * * * *", 60_000)?)?;
    assert_eq!(minutely["recognized"], true);
    assert_eq!(minutely["scheduleKind"], "every_minute_cache_gc");
    assert_eq!(minutely["d1Plans"].as_array().map(Vec::len), Some(6));

    let hourly = parse_json(&worker.handle_scheduled_json("0 * * * *", 2_678_400_000 + 5_000)?)?;
    assert_eq!(hourly["recognized"], true);
    assert_eq!(hourly["scheduleKind"], "hourly_savings_maintenance");
    assert_eq!(hourly["retentionCutoffMs"], 5_000);
    Ok(())
}
