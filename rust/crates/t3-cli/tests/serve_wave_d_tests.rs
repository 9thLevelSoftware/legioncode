//! Integration tests for Wave D capabilities wired into `t3 serve /ws` RPC dispatch.

mod support;
#[path = "support/ws_client.rs"]
mod ws_client;

use std::fs;

use serde_json::{Value, json};
use t3_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use t3_contracts::ids::ThreadId;
use t3_persistence::sqlite::SqliteStore;
use t3_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use t3_remote::theme::{publish_theme_file, set_default_theme};
use t3_remote::trace::TraceSummary;
use t3_scm::checkpoints::store::{CheckpointDiffBlob, CheckpointStore};

use support::{TestResult, isolated_env, run_t3};
use ws_client::WsTestClient;

fn rpc_request(id: &str, tag: &str, payload: Value) -> ClientRpcFrame {
    ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    }
}

async fn call_ws_rpc(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<RpcExit, Box<dyn std::error::Error>> {
    ws.send_text(&encode_client_frame(&rpc_request(id, tag, payload))?)
        .await?;
    let frames = decode_server_frames(&ws.recv_text().await?)?;
    match frames.into_iter().next() {
        Some(ServerRpcFrame::Exit { exit, .. }) => Ok(exit),
        other => Err(format!("expected Exit frame, got {other:?}").into()),
    }
}

async fn verify_providers_list_drivers(ws: &mut WsTestClient) -> TestResult {
    let RpcExit::Success { value: drivers_val } =
        call_ws_rpc(ws, "rpc-1", "providers.listDrivers", json!({})).await?
    else {
        return Err("expected Success from providers.listDrivers".into());
    };
    let all_drivers = drivers_val["drivers"]
        .as_array()
        .ok_or("missing drivers array")?;
    assert!(all_drivers.len() >= 12);
    assert!(all_drivers.iter().any(|d| d["slug"] == "antigravity"));
    assert!(all_drivers.iter().any(|d| d["slug"] == "devReplay"));

    let RpcExit::Success { value: acp_val } = call_ws_rpc(
        ws,
        "rpc-2",
        "providers.listDrivers",
        json!({ "acpOnly": true }),
    )
    .await?
    else {
        return Err("expected Success from filtered providers.listDrivers".into());
    };
    let acp_drivers = acp_val["drivers"]
        .as_array()
        .ok_or("missing filtered drivers")?;
    assert_ne!(acp_drivers.len(), 0);
    assert!(acp_drivers.iter().any(|d| d["slug"] == "antigravity"));
    assert!(acp_drivers.iter().all(|d| d["supportsAcp"] == true));
    Ok(())
}

async fn verify_checkpoints_get_diff(ws: &mut WsTestClient, server: &RunningServer) -> TestResult {
    let RpcExit::Success { value: inline_diff } = call_ws_rpc(
        ws,
        "rpc-3",
        "checkpoints.getDiff",
        json!({
            "files": [{
                "oldPath": "  spaced dir/file .txt ",
                "newPath": "  spaced dir/file .txt ",
                "oldContent": "alpha\nbeta\n",
                "newContent": "alpha\ngamma\n"
            }]
        }),
    )
    .await?
    else {
        return Err("expected Success from inline checkpoints.getDiff".into());
    };
    assert_eq!(inline_diff["filesChanged"], 1);
    assert_eq!(inline_diff["insertions"], 1);
    assert_eq!(inline_diff["deletions"], 1);
    let diff_str = inline_diff["diff"].as_str().ok_or("missing diff str")?;
    assert!(diff_str.contains("--- a/  spaced dir/file .txt "));
    assert!(diff_str.contains("+gamma"));

    let store = SqliteStore::open(&server.paths().db_path)?;
    CheckpointStore::new(store.connection()).save_diff_blob(&CheckpointDiffBlob {
        thread_id: ThreadId::new("thread-diff-1")?,
        from_turn_count: 0,
        to_turn_count: 1,
        diff: "diff --git a/a.rs b/a.rs\n+hello\n".to_owned(),
        created_at: "2026-11-20T00:00:00Z".to_owned(),
    })?;
    drop(store);

    let RpcExit::Success { value: stored_diff } = call_ws_rpc(
        ws,
        "rpc-4",
        "checkpoints.getDiff",
        json!({
            "threadId": "thread-diff-1",
            "fromTurnCount": 0,
            "toTurnCount": 1
        }),
    )
    .await?
    else {
        return Err("expected Success from stored checkpoints.getDiff".into());
    };
    assert_eq!(stored_diff["diff"], "diff --git a/a.rs b/a.rs\n+hello\n");

    let missing_diff = call_ws_rpc(
        ws,
        "rpc-5",
        "checkpoints.getDiff",
        json!({
            "threadId": "thread-diff-1",
            "fromTurnCount": 0,
            "toTurnCount": 9
        }),
    )
    .await?;
    assert!(matches!(missing_diff, RpcExit::Failure { .. }));
    Ok(())
}

async fn verify_diagnostics_and_themes(
    ws: &mut WsTestClient,
    server: &RunningServer,
) -> TestResult {
    fs::write(
        &server.paths().server_trace_path,
        "bad-json\n\
         {\"name\":\"rpc.test\",\"durationMs\":42,\"endTimeUnixNano\":\"1763600000000000000\",\"exit\":{\"_tag\":\"Success\"}}\n\
         {\"name\":\"rpc.test\",\"durationMs\":1500,\"endTimeUnixNano\":\"1763600060000000000\",\"status\":{\"code\":\"2\"}}\n",
    )?;
    let RpcExit::Success { value: trace_val } =
        call_ws_rpc(ws, "rpc-6", "diagnostics.traceSummary", json!({})).await?
    else {
        return Err("expected Success from diagnostics.traceSummary".into());
    };
    let parsed_summary: TraceSummary = serde_json::from_value(trace_val["summary"].clone())?;
    assert_eq!(parsed_summary.span_count, 2);
    assert_eq!(parsed_summary.skipped_line_count, 1);
    assert!(
        trace_val["formatted"]
            .as_str()
            .is_some_and(|text| text.contains("rpc.test"))
    );

    let raw_theme = br##"{"version":1,"name":"Aurora","appearance":"dark","canvas":"#101318","accent":"#38bdf8"}"##;
    let handle = publish_theme_file(&server.paths().environment_themes_dir, "aurora", raw_theme)?;
    set_default_theme(&server.paths().settings_path, "aurora", 1_763_600_000_000)?;
    handle.cleanup()?;

    let RpcExit::Success { value: themes_val } =
        call_ws_rpc(ws, "rpc-7", "themes.read", json!({ "themeId": "aurora" })).await?
    else {
        return Err("expected Success from themes.read".into());
    };
    assert_eq!(themes_val["current"], "aurora");
    assert_eq!(themes_val["theme"]["id"], "aurora");
    assert_eq!(themes_val["theme"]["theme"]["name"], "Aurora");

    let missing_theme = call_ws_rpc(
        ws,
        "rpc-8",
        "themes.read",
        json!({ "themeId": "not-published" }),
    )
    .await?;
    assert!(matches!(missing_theme, RpcExit::Failure { .. }));
    Ok(())
}

#[tokio::test]
async fn serve_ws_exposes_providers_checkpoints_diagnostics_and_themes_rpc() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("serve-wave-d");
    let server = spawn_server(ServeConfig {
        base_dir: base_dir.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    })
    .await?;

    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    verify_providers_list_drivers(&mut ws).await?;
    verify_checkpoints_get_diff(&mut ws, &server).await?;
    verify_diagnostics_and_themes(&mut ws, &server).await?;

    // Verify synchronous read-only CLI commands also coexist with live server state.
    let cli_env = isolated_env(temp.path(), &[]);
    let out_theme = run_t3(
        &["theme", "show", "--base-dir", &base_dir.to_string_lossy()],
        &cli_env,
    )?;
    assert!(out_theme.status.success());
    assert!(String::from_utf8(out_theme.stdout)?.contains("Environment theme: \"aurora\"."));

    server.shutdown().await?;
    Ok(())
}
