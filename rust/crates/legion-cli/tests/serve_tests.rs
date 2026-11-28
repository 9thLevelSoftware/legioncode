//! Integration tests for `legioncode serve` HTTP/WebSocket/MCP endpoints, `?frameParts=1`, and locks.

mod support;
#[path = "support/ws_client.rs"]
mod ws_client;

use std::time::Duration;

use legion_cli::serve::{RunningServer, ServeConfig, ServeError, ServeMode, spawn_server};
use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DispatchMode, MessageDispatchCommand, ModelSelection, OrchestrationActor,
    OrchestrationCommand, ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::projections::WireShellHttpSnapshot;
use legion_persistence::ownership::{OwnershipError, SERVER_EXIT_CODE_STATE_DIR_OWNED};
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use legion_shared::frame_parts::{
    FrameOutput, FramePartAssembler, PART_PREFIX, PART_RECEIVED_FRAME, SOCKET_FRAME_PART_BYTES,
    split_socket_frame,
};
use serde_json::{Value, json};

use support::{TestResult, isolated_env, run_legioncode};
use ws_client::WsTestClient;

fn make_thread_create(
    cmd_id: &str,
    thread_id: &str,
    title: &str,
) -> Result<OrchestrationCommand, Box<dyn std::error::Error>> {
    Ok(OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new(cmd_id)?,
        thread_id: ThreadId::new(thread_id)?,
        project_id: ProjectId::new("proj-1")?,
        title: title.to_owned(),
        generate_title: None,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: None,
        title_renamed: None,
        workflow_run: None,
    }))
}

fn make_msg_dispatch(
    cmd_id: &str,
    thread_id: &str,
    msg_id: &str,
    text: &str,
) -> Result<OrchestrationCommand, Box<dyn std::error::Error>> {
    Ok(OrchestrationCommand::MessageDispatch(
        MessageDispatchCommand {
            notification: None,
            guest_message_id: None,
            created_by: OrchestrationActor::User,
            creation_source: CreationSource::Web,
            scheduled_task_id: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            fresh_context: None,
            output_schema: None,
            command_id: CommandId::new(cmd_id)?,
            thread_id: ThreadId::new(thread_id)?,
            message_id: MessageId::new(msg_id)?,
            text: text.to_owned(),
            context: None,
            attachments: Vec::new(),
            title_seed: None,
            model_selection: None,
            source_plan_ref: None,
            restart_continuation_of_run_id: None,
            usage_limit_continuation_of_run_id: None,
            manual_continuation_of_run_id: None,
            steer_follow_up_of_run_id: None,
            steer_follow_up_after_run_ordinal: None,
            usage_limit_recovery_request_id: None,
            delivery_intent: None,
            delegated_completion: None,
            dispatch_mode: DispatchMode::StartImmediately,
        },
    ))
}

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

async fn verify_ws_rpc_roundtrip(server: &RunningServer) -> TestResult {
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    ws.send_text(&encode_client_frame(&ClientRpcFrame::Ping)?)
        .await?;
    assert_eq!(
        decode_server_frames(&ws.recv_text().await?)?,
        vec![ServerRpcFrame::Pong]
    );

    let create_cmd = make_thread_create("cmd-ws-1", "thread-ws-1", "Hello WS")?;
    let create_req = rpc_request(
        "req-create",
        "orchestration.v2.dispatchCommand",
        serde_json::to_value(&create_cmd)?,
    );
    ws.send_text(&encode_client_frame(&create_req)?).await?;
    let create_exit = decode_server_frames(&ws.recv_text().await?)?;
    assert!(matches!(
        create_exit.as_slice(),
        [ServerRpcFrame::Exit {
            request_id: RpcId::String(id),
            exit: RpcExit::Success { value },
        }] if id == "req-create" && value["sequence"].as_i64().is_some_and(|s| s >= 1)
    ));

    let msg_cmd = make_msg_dispatch("cmd-ws-2", "thread-ws-1", "msg-ws-1", "First prompt")?;
    let msg_req = rpc_request(
        "req-msg",
        "orchestration.v2.dispatchCommand",
        serde_json::to_value(&msg_cmd)?,
    );
    ws.send_text(&encode_client_frame(&msg_req)?).await?;
    let msg_exit = decode_server_frames(&ws.recv_text().await?)?;
    assert!(matches!(
        msg_exit.as_slice(),
        [ServerRpcFrame::Exit {
            exit: RpcExit::Success { .. },
            ..
        }]
    ));

    ws.send_text(&encode_client_frame(&ClientRpcFrame::Ack {
        request_id: RpcId::from("req-msg"),
    })?)
    .await?;
    let snap_req = rpc_request("req-snap", "orchestration.v2.getShellSnapshot", json!({}));
    ws.send_text(&encode_client_frame(&snap_req)?).await?;
    let snap_frames = decode_server_frames(&ws.recv_text().await?)?;
    let ServerRpcFrame::Exit {
        exit: RpcExit::Success { value: snap_val },
        ..
    } = snap_frames
        .first()
        .ok_or("missing shell snapshot Exit frame")?
    else {
        return Err("expected Success Exit for getShellSnapshot".into());
    };
    let snap: WireShellHttpSnapshot = serde_json::from_value(snap_val.clone())?;
    assert_eq!(snap.threads.len(), 1);
    assert_eq!(snap.threads[0].id, "thread-ws-1");
    Ok(())
}

async fn verify_ws_frame_parts(server: &RunningServer) -> TestResult {
    let huge_title = "Legion-FramePart-".repeat(4_500);
    assert!(huge_title.len() > SOCKET_FRAME_PART_BYTES);
    let huge_cmd = make_thread_create("cmd-huge-1", "thread-huge-1", &huge_title)?;
    let huge_req = rpc_request(
        "req-huge",
        "orchestration.v2.dispatchCommand",
        serde_json::to_value(&huge_cmd)?,
    );
    let huge_wire = encode_client_frame(&huge_req)?;
    let inbound_parts = split_socket_frame(&huge_wire);
    assert!(inbound_parts.len() > 1);

    let mut ws_fp =
        WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2&frameParts=1").await?;
    for (idx, part) in inbound_parts.iter().enumerate() {
        ws_fp.send_text(part).await?;
        let reply = ws_fp.recv_text().await?;
        if idx + 1 < inbound_parts.len() {
            assert_eq!(reply, PART_RECEIVED_FRAME);
        } else {
            let frames = decode_server_frames(&reply)?;
            assert!(matches!(
                frames.as_slice(),
                [ServerRpcFrame::Exit {
                    request_id: RpcId::String(id),
                    exit: RpcExit::Success { .. },
                }] if id == "req-huge"
            ));
        }
    }

    let snap_req = rpc_request(
        "req-snap-fp",
        "orchestration.v2.getShellSnapshot",
        json!({}),
    );
    ws_fp.send_text(&encode_client_frame(&snap_req)?).await?;
    let mut assembler = FramePartAssembler::new();
    let mut part_count = 0usize;
    let reassembled_snap = loop {
        let frame_str = ws_fp.recv_text().await?;
        part_count += 1;
        if let FrameOutput::Complete(whole) = assembler.push(&frame_str) {
            break whole.into_owned();
        }
        assert!(frame_str.starts_with(PART_PREFIX));
    };
    assert!(part_count >= 2);
    let fp_frames = decode_server_frames(&reassembled_snap)?;
    let ServerRpcFrame::Exit {
        exit: RpcExit::Success { value: fp_val },
        ..
    } = fp_frames.first().ok_or("missing reassembled Exit")?
    else {
        return Err("expected Success Exit from reassembled frameParts".into());
    };
    let fp_snap: WireShellHttpSnapshot = serde_json::from_value(fp_val.clone())?;
    assert_eq!(fp_snap.threads.len(), 2);
    Ok(())
}

async fn verify_mcp_http_gates(http: &reqwest::Client, base_url: &str) -> TestResult {
    let mcp_url = format!("{base_url}/mcp");
    let mcp_origin = http
        .post(&mcp_url)
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
        .send()
        .await?;
    assert_eq!(mcp_origin.status(), 403);

    let mcp_get = http.get(&mcp_url).send().await?;
    assert_eq!(mcp_get.status(), 405);
    assert_eq!(
        mcp_get.headers().get("allow").and_then(|v| v.to_str().ok()),
        Some("POST, DELETE")
    );

    let mcp_415 = http
        .post(&mcp_url)
        .header("content-type", "text/plain")
        .header("accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await?;
    assert_eq!(mcp_415.status(), 415);

    let mcp_406 = http
        .post(&mcp_url)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body("{}")
        .send()
        .await?;
    assert_eq!(mcp_406.status(), 406);

    let mcp_ping = http
        .post(&mcp_url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":7,"method":"ping","params":{}}"#)
        .send()
        .await?;
    assert_eq!(mcp_ping.status(), 200);
    let ping_json: Value = mcp_ping.json().await?;
    assert_eq!(ping_json["jsonrpc"], "2.0");
    assert_eq!(ping_json["id"], 7);
    assert_eq!(ping_json["result"], json!({}));

    let mcp_del = http.delete(&mcp_url).send().await?;
    assert_eq!(mcp_del.status(), 204);
    Ok(())
}

#[tokio::test]
async fn serve_exposes_environment_ws_rpc_frame_parts_mcp_and_owner_lock() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("serve-home");
    let config = ServeConfig {
        base_dir: base_dir.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    };
    let server = spawn_server(config.clone()).await?;
    let base_url = server.origin().to_owned();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let env_resp = http
        .get(format!("{base_url}/.well-known/legioncode/environment"))
        .send()
        .await?;
    assert_eq!(env_resp.status(), 200);
    let descriptor: ExecutionEnvironmentDescriptor = env_resp.json().await?;
    assert_eq!(&descriptor.environment_id, server.environment_id());
    assert_eq!(descriptor.orchestration_protocol_version, Some(2));
    assert_eq!(
        descriptor.database_migration,
        u32::try_from(legion_persistence::sqlite::BUILD_MIGRATION_HEAD).ok()
    );

    let ws_no_proto = http.get(format!("{base_url}/ws")).send().await?;
    assert_eq!(ws_no_proto.status(), 426);
    let ws_bad_proto = http
        .get(format!("{base_url}/ws?orchestrationProtocol=1"))
        .send()
        .await?;
    assert_eq!(ws_bad_proto.status(), 426);

    verify_ws_rpc_roundtrip(&server).await?;
    verify_ws_frame_parts(&server).await?;
    verify_mcp_http_gates(&http, &base_url).await?;

    let Err(second_err) = spawn_server(config).await else {
        return Err("expected second spawn_server on owned base_dir to fail".into());
    };
    assert_eq!(second_err.exit_code(), SERVER_EXIT_CODE_STATE_DIR_OWNED);
    assert!(matches!(
        second_err,
        ServeError::Ownership(OwnershipError::StateDirOwned { .. })
    ));

    let cli_env = isolated_env(temp.path(), &[]);
    let second_cli = run_legioncode(
        &[
            "serve",
            "--base-dir",
            &base_dir.to_string_lossy(),
            "--port",
            "0",
        ],
        &cli_env,
    )?;
    assert_eq!(second_cli.status.code(), Some(78));

    let runtime_path = server.paths().server_runtime_state_path.clone();
    assert!(runtime_path.exists());
    server.shutdown().await?;
    assert!(!runtime_path.exists());
    Ok(())
}
