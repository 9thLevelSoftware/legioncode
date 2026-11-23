//! Tests for [`legion_wasm_ffi::bridge::HookInboxDurableObjectBridge`] backing
//! `WasmHookInboxObject`.

use legion_wasm_ffi::bridge::HookInboxDurableObjectBridge;
use serde_json::Value;

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn hook_inbox_actor_bridge_preserves_bug29_fencing_alarms_and_json_hydration() -> Result<(), String>
{
    let mut actor = HookInboxDurableObjectBridge::new("env-do-1");
    assert_eq!(actor.target_generation(), 0);
    assert_eq!(actor.pending_count(), 0);
    assert_eq!(actor.alarm_at_ms(), None);

    let held = parse_json(&actor.hold_json(
        r#"{
            "id": "deliv-1",
            "hookKey": "hook-gh",
            "baseUrl": "https://old.tunnel.test",
            "method": "POST",
            "body": {"event": "push"},
            "nowMs": 1000
        }"#,
    )?)?;
    assert_eq!(held["held"], true);
    assert_eq!(held["targetGeneration"], 1);
    assert_eq!(held["nextAttemptAtMs"], 11_000);
    assert_eq!(actor.alarm_at_ms(), Some(11_000));
    assert!(!actor.is_alarm_due(10_999));
    assert!(actor.is_alarm_due(11_000));

    // Hydrate a second Durable Object instance from exported JSON state.
    let saved_state = actor.export_state_json()?;
    let mut restored = HookInboxDurableObjectBridge::from_state_json(&saved_state)?;
    assert_eq!(restored.target_generation(), 1);
    assert_eq!(restored.pending_count(), 1);

    // Wake rotates generation to 2 and schedules an immediate alarm at 1_200.
    let woken = parse_json(&restored.wake("https://new.tunnel.test", 1_200)?)?;
    assert_eq!(woken["pending"], true);
    assert_eq!(woken["targetGeneration"], 2);
    assert_eq!(woken["alarmAtMs"], 1_200);

    // Alarm step dispatches batch at 1_200 and clears consumed alarm slot.
    let step = parse_json(&restored.alarm_step_json(1_200)?)?;
    assert_eq!(step["due"], true);
    assert_eq!(step["alarmAtMs"], Value::Null);
    assert_eq!(step["batch"]["targetGeneration"], 2);
    assert_eq!(step["batch"]["hooks"][0]["id"], "deliv-1");

    // Stale generation 1 unreachable result MUST be ignored (BUG-29).
    let stale = parse_json(&restored.record_outcome_json(
        r#"{"snapshotGeneration": 1, "hookId": "deliv-1", "status": 503, "nowMs": 9000}"#,
    )?)?;
    assert_eq!(stale["ignoredStale"], true);
    assert_eq!(stale["failures"], 0);
    assert_eq!(stale["currentGeneration"], 2);
    assert_eq!(stale["pendingCount"], 1);

    // Current generation 2 delivery succeeds and clears inbox + alarm.
    let delivered = parse_json(&restored.record_outcome_json(
        r#"{"snapshotGeneration": 2, "hookId": "deliv-1", "status": 200, "nowMs": 9100}"#,
    )?)?;
    assert_eq!(delivered["outcomeKind"], "delivered");
    assert_eq!(delivered["ignoredStale"], false);
    assert_eq!(delivered["pendingCount"], 0);
    assert_eq!(delivered["alarmAtMs"], Value::Null);
    Ok(())
}

#[test]
fn hook_inbox_actor_bridge_fetch_and_failure_retry_and_hydrate() -> Result<(), String> {
    let mut actor = HookInboxDurableObjectBridge::new("env-do-2");
    let hold_resp = parse_json(&actor.fetch_json(
        r#"{
            "method": "POST",
            "path": "/hold",
            "headers": {"x-legioncode-target-url": "https://t1.example.test", "x-legioncode-delivery-id": "d-fetch"},
            "body": {"ok": true},
            "nowMs": 500
        }"#,
    )?)?;
    assert_eq!(hold_resp["status"], 202);
    assert_eq!(hold_resp["body"]["deliveryId"], "d-fetch");

    let dup_resp = parse_json(&actor.fetch_json(
        r#"{
            "method": "POST",
            "path": "/hold",
            "headers": {"x-legioncode-target-url": "https://t1.example.test", "x-legioncode-delivery-id": "d-fetch"},
            "nowMs": 501
        }"#,
    )?)?;
    assert_eq!(dup_resp["status"], 409);

    let batch_resp = parse_json(
        &actor.fetch_json(r#"{"method": "POST", "path": "/deliver-batch", "nowMs": 600}"#)?,
    )?;
    assert_eq!(batch_resp["status"], 200);
    assert_eq!(batch_resp["body"]["hooks"][0]["id"], "d-fetch");

    let run_fail = parse_json(&actor.record_run_failure(700)?)?;
    assert_eq!(run_fail["retryAtMs"], 60_700);

    let fin = parse_json(&actor.finish_alarm_batch(0, 1, 800)?)?;
    assert_eq!(fin["nextAlarmAtMs"], 30_800);
    assert_eq!(actor.alarm_at_ms(), Some(30_800));

    let summary = parse_json(&actor.fetch_json(r#"{"path": "/summary"}"#)?)?;
    assert_eq!(summary["status"], 200);
    assert_eq!(summary["body"]["pendingCount"], 1);

    let mut fresh = HookInboxDurableObjectBridge::new("other");
    let hydrated = parse_json(&fresh.hydrate_state_json(&actor.export_state_json()?)?)?;
    assert_eq!(hydrated["endpointKey"], "env-do-2");
    assert_eq!(hydrated["pendingCount"], 1);

    fresh.clear();
    assert_eq!(fresh.pending_count(), 0);
    assert_eq!(fresh.alarm_at_ms(), None);
    Ok(())
}
