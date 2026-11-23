//! Additional RPC parity tests: assets, attachments, browse, scratch, terminal, providers, ACP, telemetry, preview, device.

use std::fs;

use serde_json::json;

use super::dispatch;
use super::tests_core::{assert_err_tag, assert_ok, build_test_state, init_git_repo};

#[tokio::test]
async fn assets_attachments_browse_scratch_and_terminal_work() {
    let (tmp, state) = build_test_state();
    let repo_dir = tmp.path().join("workspace-assets");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();

    let asset = assert_ok(
        dispatch(
            &state,
            "assets.createUrl",
            json!({
                "resource": {
                    "kind": "workspace-file",
                    "workspaceRoot": repo_str,
                    "baseRelativePath": "README.md"
                }
            }),
        )
        .await,
        "assets.createUrl",
    );
    assert!(
        asset["relativeUrl"]
            .as_str()
            .expect("relativeUrl")
            .starts_with("/api/assets/")
    );

    let upload = assert_ok(
        dispatch(
            &state,
            "attachments.createUploadUrl",
            json!({ "type": "image", "name": "shot.png", "mimeType": "image/png", "sizeBytes": 128 }),
        )
        .await,
        "attachments.createUploadUrl",
    );
    assert!(
        upload["relativeUrl"]
            .as_str()
            .expect("upload url")
            .starts_with("/api/attachments/upload/")
    );

    let meta = assert_ok(
        dispatch(
            &state,
            "filesystem.getMetadata",
            json!({ "paths": [repo_dir.join("README.md").to_string_lossy()] }),
        )
        .await,
        "filesystem.getMetadata",
    );
    assert_eq!(meta["entries"][0]["kind"], "file");

    let browse = assert_ok(
        dispatch(
            &state,
            "filesystem.browse",
            json!({ "partialPath": format!("{}/", repo_dir.to_string_lossy()) }),
        )
        .await,
        "filesystem.browse",
    );
    assert!(browse["entries"].is_array());

    let scratch = assert_ok(
        dispatch(&state, "projects.ensureScratch", json!({})).await,
        "projects.ensureScratch",
    );
    assert!(scratch["projectId"].is_string());

    let term_open = assert_ok(
        dispatch(
            &state,
            "terminal.open",
            json!({ "threadId": "th-term", "terminalId": "default", "cols": 80, "rows": 24 }),
        )
        .await,
        "terminal.open",
    );
    assert_eq!(term_open["threadId"], "th-term");
    let _ = assert_ok(
        dispatch(
            &state,
            "terminal.write",
            json!({ "threadId": "th-term", "data": "ping" }),
        )
        .await,
        "terminal.write",
    );
    let _ = assert_ok(
        dispatch(&state, "terminal.clear", json!({ "threadId": "th-term" })).await,
        "terminal.clear",
    );
    let _ = assert_ok(
        dispatch(&state, "terminal.close", json!({ "threadId": "th-term" })).await,
        "terminal.close",
    );
}

#[tokio::test]
async fn providers_acp_telemetry_preview_and_device_rpcs_work() {
    let (_tmp, state) = build_test_state();

    let refreshed = assert_ok(
        dispatch(&state, "server.refreshProviders", json!({})).await,
        "server.refreshProviders",
    );
    assert!(refreshed["providers"].is_array());

    let updated = assert_ok(
        dispatch(
            &state,
            "server.updateProvider",
            json!({ "instanceId": "devReplay", "patch": { "enabled": true } }),
        )
        .await,
        "server.updateProvider",
    );
    assert!(
        updated["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .any(|p| p["driver"] == "devReplay")
    );

    let text_models = assert_ok(
        dispatch(&state, "server.listTextGeneratorModels", json!({})).await,
        "server.listTextGeneratorModels",
    );
    assert!(text_models["models"].is_array());

    let triage = assert_ok(
        dispatch(&state, "server.prepareTriage", json!({})).await,
        "server.prepareTriage",
    );
    assert!(triage["prompt"].is_string());

    for tag in [
        "server.getTraceDiagnostics",
        "server.getProcessDiagnostics",
        "server.getHostResources",
        "server.getProcessResourceHistory",
        "server.getResourceTelemetryHistory",
        "server.retryResourceTelemetry",
    ] {
        let _ = assert_ok(dispatch(&state, tag, json!({})).await, tag);
    }
    let otlp = assert_ok(
        dispatch(
            &state,
            "server.checkOtlpEndpoint",
            json!({ "endpoint": "invalid://otlp" }),
        )
        .await,
        "server.checkOtlpEndpoint",
    );
    assert_ne!(otlp["outcome"], "Accepted");

    let _ = assert_ok(
        dispatch(
            &state,
            "server.setAcpRegistryProvider",
            json!({ "id": "acp-demo", "label": "Demo" }),
        )
        .await,
        "setAcpRegistryProvider",
    );
    let acp_list = assert_ok(
        dispatch(&state, "server.listAcpRegistryProviders", json!({})).await,
        "listAcpRegistryProviders",
    );
    assert_eq!(acp_list["providers"].as_array().map(Vec::len), Some(1));
    let _ = assert_ok(
        dispatch(
            &state,
            "server.disableAcpRegistryProvider",
            json!({ "id": "acp-demo" }),
        )
        .await,
        "disableAcpRegistryProvider",
    );
}

#[tokio::test]
async fn preview_device_and_unsupported_rpcs_work() {
    let (_tmp, state) = build_test_state();

    let tab = assert_ok(
        dispatch(
            &state,
            "preview.open",
            json!({ "threadId": "thread-p1", "url": "http://localhost:3000" }),
        )
        .await,
        "preview.open",
    );
    let tab_id = tab["tabId"].as_str().expect("tabId").to_owned();
    let _ = assert_ok(
        dispatch(
            &state,
            "preview.resize",
            json!({ "threadId": "thread-p1", "tabId": tab_id, "viewport": { "_tag": "fill" } }),
        )
        .await,
        "preview.resize",
    );
    let _ = assert_ok(
        dispatch(
            &state,
            "preview.close",
            json!({ "threadId": "thread-p1", "tabId": tab_id }),
        )
        .await,
        "preview.close",
    );
    let dev_cfg = assert_ok(
        dispatch(
            &state,
            "device.configure",
            json!({ "onboardingCompleted": true }),
        )
        .await,
        "device.configure",
    );
    assert_eq!(dev_cfg["onboardingCompleted"], true);

    let _ = assert_err_tag(
        dispatch(&state, "unknown.method", json!({})).await,
        "UnsupportedRpcMethod",
        "unknown method",
    );
}
