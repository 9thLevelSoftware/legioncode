//! Tests for [`t3_wasm_ffi::bridge::ClientCoreBridge`] backing `WasmClientCore`.

use serde_json::Value;
use t3_wasm_ffi::bridge::{ClientCoreBridge, js_ms_to_u64, u64_to_f64_exact};

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn timestamp_conversion_validates_js_safe_integers() {
    assert_eq!(js_ms_to_u64(0.0), Ok(0));
    assert_eq!(js_ms_to_u64(1_732_118_400_987.0), Ok(1_732_118_400_987));
    assert_eq!(u64_to_f64_exact(1_732_118_400_987), 1_732_118_400_987.0);
    assert!(js_ms_to_u64(-0.1).is_err());
    assert!(js_ms_to_u64(f64::NAN).is_err());
    assert!(js_ms_to_u64(f64::INFINITY).is_err());
    assert!(js_ms_to_u64(9_007_199_254_740_992.0).is_err());
}

#[test]
fn steady_shell_holds_until_peer_live_or_2s_deadline_and_isolates_instances() -> Result<(), String>
{
    let mut core_a = ClientCoreBridge::new(0);
    let core_b = ClientCoreBridge::new(0);

    let init = parse_json(&core_a.enqueue_steady_shell(
        r#"{
            "inputs": [
                {"environmentId":"env-a","snapshot":"cached-a","serverSequence":1,"status":"synchronizing","generation":0,"connecting":true},
                {"environmentId":"env-b","snapshot":"cached-b","serverSequence":2,"status":"synchronizing","generation":0,"connecting":true}
            ],
            "nowMs": 0
        }"#,
    )?)?;
    assert_eq!(init["ready"], true);

    let held = parse_json(&core_a.enqueue_steady_shell(
        r#"{"environmentId":"env-a","snapshot":"live-a","serverSequence":3,"status":"live","nowMs":500}"#,
    )?)?;
    assert_eq!(held["holding"], true);
    assert_eq!(held["ready"], false);
    assert_eq!(held["releaseAtMs"], 2_500);
    assert_eq!(held["shells"]["env-a"]["shown"], "cached-a");

    let early = parse_json(&core_a.poll_steady_ready(1_500)?)?;
    assert_eq!(early["ready"], false);
    assert_eq!(early["shells"]["env-a"]["shown"], "cached-a");

    let released = parse_json(&core_a.poll_steady_ready(2_500)?)?;
    assert_eq!(released["ready"], true);
    assert_eq!(released["holding"], false);
    assert_eq!(released["releaseAtMs"], Value::Null);
    assert_eq!(released["shells"]["env-a"]["shown"], "live-a");

    // Separate instance never inherits closure state from core_a (BUG-21).
    let mut isolated = core_b;
    let isolated_poll = parse_json(&isolated.poll_steady_ready(500)?)?;
    assert_eq!(isolated_poll["ready"], true);
    assert_eq!(isolated_poll["shells"], serde_json::json!({}));
    Ok(())
}

#[test]
fn touch_family_manages_generations_mounts_merges_and_eviction() -> Result<(), String> {
    let mut core = ClientCoreBridge::new(0);

    let initial = parse_json(
        &core.touch_family(r#"{"environmentId":"env-1","threadId":"parent","nowMs":0}"#)?,
    )?;
    assert_eq!(initial["shellGeneration"], 0);
    assert_eq!(initial["readState"], "pending");
    assert_eq!(initial["mountRefCount"], 1);

    let set_gen = parse_json(&core.touch_family(
        r#"{"action":"setGeneration","environmentId":"env-1","generation":1,"nowMs":10}"#,
    )?)?;
    assert_eq!(set_gen["shellGeneration"], 1);

    let began = parse_json(&core.touch_family(
        r#"{"action":"beginFetch","environmentId":"env-1","threadId":"parent","nowMs":15}"#,
    )?)?;
    assert_eq!(began["fetchGeneration"], 1);
    assert_eq!(began["readState"], "pending");

    let completed = parse_json(&core.touch_family(
        r#"{
            "action":"completeFetch",
            "environmentId":"env-1",
            "threadId":"parent",
            "generation":1,
            "snapshotSequence":3,
            "threads":[{"id":"child-1","payload":{"title":"Child"}}],
            "nowMs":20
        }"#,
    )?)?;
    assert_eq!(completed["readState"], "settled");
    assert_eq!(completed["cachedEntryCount"], 1);

    let merged = parse_json(&core.touch_family(
        r#"{
            "action":"merge",
            "environmentId":"env-1",
            "shellThreads":[{"id":"parent","payload":{"title":"Parent"}}]
        }"#,
    )?)?;
    let merged_rows = merged["mergedThreads"]
        .as_array()
        .ok_or("missing mergedThreads")?;
    assert_eq!(merged_rows.len(), 2);
    assert_eq!(merged_rows[1]["id"], "child-1");

    let released = parse_json(&core.touch_family(
        r#"{"action":"release","environmentId":"env-1","threadId":"parent","nowMs":30}"#,
    )?)?;
    assert_eq!(released["released"], true);
    assert_eq!(released["mountRefCount"], 0);

    let evicted = parse_json(&core.touch_family(r#"{"action":"evict","nowMs":60030}"#)?)?;
    assert_eq!(evicted["evictedCount"], 1);
    assert_eq!(evicted["cachedEntryCount"], 0);
    Ok(())
}

#[test]
fn reconnect_backoff_and_silent_socket_probe_advance_deterministically() -> Result<(), String> {
    let mut core = ClientCoreBridge::new(1_000);
    assert_eq!(core.next_backoff_ms(0), 1_500);
    assert_eq!(core.next_backoff_ms(1), 3_000);
    assert_eq!(core.next_backoff_ms(2), 6_000);
    assert_eq!(core.next_backoff_ms(8), 225_000);

    let d1 = parse_json(&core.record_disconnect(1_000)?)?;
    assert_eq!(d1["attempt"], 1);
    assert_eq!(d1["delayMs"], 1_500);
    assert_eq!(d1["nextRetryAtMs"], 2_500);

    let d2 = parse_json(&core.record_disconnect(2_500)?)?;
    assert_eq!(d2["attempt"], 2);
    assert_eq!(d2["delayMs"], 3_000);

    let reset = parse_json(&core.probe_step_json(r#"{"kind":"reset","routeKind":"relay"}"#)?)?;
    assert_eq!(reset["probing"], false);

    let sig = parse_json(&core.probe_step_json(
        r#"{"kind":"signal","signal":{"Wakeup":"application-active-probe"},"nowMs":0}"#,
    )?)?;
    assert_eq!(sig["probing"], true);
    assert_eq!(sig["actions"][0], "StartProbe");
    assert_eq!(sig["nextDeadlineMs"], 500);

    let tick500 =
        parse_json(&core.probe_step_json(r#"{"kind":"tick","nowMs":500,"currentFrames":0}"#)?)?;
    assert_eq!(tick500["actions"][0], "LoadInitialConfig");

    let cfg = parse_json(&core.probe_step_json(
        r#"{"kind":"configLoaded","loaded":true,"currentFrames":0,"nowMs":500}"#,
    )?)?;
    assert_eq!(cfg["actions"][0], "CheckHost");

    let host_ok = parse_json(&core.probe_step_json(
        r#"{"kind":"hostCheckResult","answered":true,"currentFrames":0,"nowMs":500}"#,
    )?)?;
    assert_eq!(host_ok["nextDeadlineMs"], 600);

    let poll2499 =
        parse_json(&core.probe_step_json(r#"{"kind":"tick","nowMs":2499,"currentFrames":0}"#)?)?;
    assert_eq!(poll2499["actions"][0], "PollInboundFrames");
    assert_eq!(poll2499["nextDeadlineMs"], 2_500);

    let silent =
        parse_json(&core.probe_step_json(r#"{"kind":"tick","nowMs":2500,"currentFrames":0}"#)?)?;
    assert_eq!(
        silent["actions"][0]["ReplaceSocket"]["reason"],
        "SilentSocket"
    );
    assert_eq!(silent["probing"], false);
    Ok(())
}
