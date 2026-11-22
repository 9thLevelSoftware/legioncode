//! Host tests verifying Wave 5 `legion-wasm-ffi` pure helpers and `bridge` helpers.

use legion_wasm_ffi::bridge::{
    active_brand_json, allows_preview_tunnel_json, highlight_hdl_json, link_batch_plan_json,
    normalize_nfd_filename, resolve_linked_thread_open_json, strip_osc4_queries,
};
use serde_json::Value;

#[test]
fn wave5_pure_helpers_normalize_strip_policy_and_brand() -> Result<(), String> {
    assert_eq!(
        normalize_nfd_filename("re\u{0301}sume\u{0301}.txt"),
        "résumé.txt"
    );
    assert_eq!(
        normalize_nfd_filename("/Users/dev/Cafe\u{0301} notes/"),
        "Café notes"
    );

    let dirty = "hello\x1b]4;1;?\x07world";
    assert_eq!(strip_osc4_queries(dirty), "hello\x1b\\world");
    let mixed = "\x1b]4;1;red;2;?\x07keep";
    assert!(strip_osc4_queries(mixed).contains("red;2;?"));

    let allowed: Value = serde_json::from_str(&allows_preview_tunnel_json(
        r#"{"enablePreviewTunnel":true,"managedPolicy":null}"#,
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(allowed["allowed"].as_bool(), Some(true));

    let blocked: Value = serde_json::from_str(&allows_preview_tunnel_json(
        r#"{"enablePreviewTunnel":true,"managedPolicy":{"organization":"Acme","lockedSettings":[],"limits":{"previewTunnel":false}}}"#,
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(blocked["allowed"].as_bool(), Some(false));

    let brand: Value = serde_json::from_str(&active_brand_json()?).map_err(|e| e.to_string())?;
    assert_eq!(brand["productName"].as_str(), Some("LegionCode"));
    assert_eq!(brand["cliBinName"].as_str(), Some("legioncode"));
    Ok(())
}

#[test]
fn wave5_client_core_and_web_app_bridge_helpers() -> Result<(), String> {
    let strategy: Value = serde_json::from_str(&resolve_linked_thread_open_json(
        r#"{
            "target": {
                "viaEnvironmentId": "hub",
                "environmentId": "leaf",
                "threadId": "th-9",
                "label": "Leaf",
                "title": null
            },
            "ownConnected": null,
            "peerLinks": [{ "environmentId": "hub", "label": "Hub" }]
        }"#,
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(strategy.as_str(), Some("linked"));

    let batch: Value = serde_json::from_str(&link_batch_plan_json(
        r#"{
            "machines": [
                { "environmentId": "e1", "label": "E1", "machine": "desktop", "connected": true, "connectOnly": false, "linked": false, "urls": ["http://e1"] },
                { "environmentId": "e2", "label": "E2", "machine": "server", "connected": true, "connectOnly": false, "linked": false, "urls": ["http://e2"] },
                { "environmentId": "e3", "label": "E3", "machine": "server", "connected": true, "connectOnly": false, "linked": false, "urls": ["http://e3"] },
                { "environmentId": "e4", "label": "E4", "machine": "server", "connected": true, "connectOnly": false, "linked": false, "urls": ["http://e4"] },
                { "environmentId": "e5", "label": "E5", "machine": "server", "connected": true, "connectOnly": false, "linked": false, "urls": ["http://e5"] }
            ],
            "writableEnvironmentIds": ["e1", "e2", "e3", "e4", "e5"]
        }"#,
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(batch["concurrency"].as_u64(), Some(4));
    assert_eq!(batch["startedIds"].as_array().map(Vec::len), Some(4));

    let hdl: Value = serde_json::from_str(&highlight_hdl_json(
        "alu.sv",
        "always_ff @(posedge clk) q <= 1;",
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(hdl["language"].as_str(), Some("system-verilog"));

    let unknown_hdl: Value = serde_json::from_str(&highlight_hdl_json("notes.txt", "plain text")?)
        .map_err(|e| e.to_string())?;
    assert_eq!(unknown_hdl["language"], Value::Null);
    Ok(())
}
