//! Regression tests for live `subscribeShell` project mutations and implicit dev state dir isolation.
#![forbid(unsafe_code)]

#[path = "support/ws_client.rs"]
mod ws_client;

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server_with_dev_url};
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use serde_json::{Value, json};
use ws_client::WsTestClient;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn spawn_dev_server(
    base: &std::path::Path,
    base_dir_is_explicit: bool,
    dev_url: Option<String>,
) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server_with_dev_url(
        ServeConfig {
            base_dir: base.to_path_buf(),
            base_dir_is_explicit,
            port: 0,
            host: Some("127.0.0.1".to_owned()),
            mode: ServeMode::Web,
            no_browser: true,
            lan: false,
            static_dir: None,
            cwd: base.to_path_buf(),
        },
        dev_url,
    )
    .await
    .map_err(Into::into)
}

async fn send_req(ws: &mut WsTestClient, id: &str, tag: &str, payload: Value) -> TestResult {
    let frame = ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    };
    ws.send_text(&encode_client_frame(&frame)?).await
}

async fn recv_frames(
    ws: &mut WsTestClient,
) -> Result<Vec<ServerRpcFrame>, Box<dyn std::error::Error>> {
    Ok(decode_server_frames(&ws.recv_text().await?)?)
}

async fn wait_for_exit_buffering(
    ws: &mut WsTestClient,
    id: &str,
    sub_id: &str,
    buffered: &mut Vec<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        for frame in recv_frames(ws).await? {
            match frame {
                ServerRpcFrame::Chunk { request_id, values }
                    if request_id == RpcId::from(sub_id) =>
                {
                    buffered.extend(values);
                }
                ServerRpcFrame::Exit { request_id, exit } if request_id == RpcId::from(id) => {
                    return match exit {
                        RpcExit::Success { value } => Ok(value),
                        RpcExit::Failure { cause } => Err(format!("{id} failed: {cause:?}").into()),
                    };
                }
                _ => {}
            }
        }
    }
}

async fn wait_for_shell_event<F>(
    ws: &mut WsTestClient,
    sub_id: &str,
    buffered: &mut Vec<Value>,
    predicate: F,
) -> Result<Value, Box<dyn std::error::Error>>
where
    F: Fn(&Value) -> bool,
{
    if let Some(pos) = buffered.iter().position(&predicate) {
        return Ok(buffered.remove(pos));
    }
    loop {
        for frame in recv_frames(ws).await? {
            if let ServerRpcFrame::Chunk { request_id, values } = frame
                && request_id == RpcId::from(sub_id)
            {
                buffered.extend(values);
                if let Some(pos) = buffered.iter().position(&predicate) {
                    return Ok(buffered.remove(pos));
                }
            }
        }
    }
}

#[tokio::test]
async fn projects_mutate_and_create_new_stream_live_shell_updates() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base = temp.path().join("home");
    let server = spawn_dev_server(&base, true, None).await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    let mut buffered = Vec::new();

    send_req(
        &mut ws,
        "sub-shell",
        "orchestration.v2.subscribeShell",
        json!({}),
    )
    .await?;
    let initial_snap = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "snapshot"
    })
    .await?;
    let start_seq = initial_snap["snapshot"]["snapshotSequence"]
        .as_i64()
        .ok_or("missing snapshotSequence")?;
    let _ = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "synchronized"
    })
    .await?;

    let created_dir = temp.path().join("missing-on-disk-project");
    send_req(
        &mut ws,
        "m-create",
        "projects.mutate",
        json!({
            "type": "project.create",
            "commandId": "cmd-p-1",
            "projectId": "proj-live-1",
            "title": "LiveProject",
            "workspaceRoot": created_dir,
            "createWorkspaceRootIfMissing": true
        }),
    )
    .await?;
    let _ = wait_for_exit_buffering(&mut ws, "m-create", "sub-shell", &mut buffered).await?;
    let created_ev = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "project.updated" && v["project"]["id"] == "proj-live-1"
    })
    .await?;
    assert_eq!(created_ev["project"]["title"], "LiveProject");
    assert!(created_dir.is_dir());

    send_req(&mut ws, "m-scratch", "projects.ensureScratch", json!({})).await?;
    let _ = wait_for_exit_buffering(&mut ws, "m-scratch", "sub-shell", &mut buffered).await?;
    let scratch_ev = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "project.updated" && v["project"]["id"] == "project-scratch"
    })
    .await?;
    assert_eq!(scratch_ev["project"]["title"], "Scratch");

    send_req(
        &mut ws,
        "m-new",
        "projects.createNew",
        json!({ "name": "Brand New", "directory": temp.path() }),
    )
    .await?;
    let new_res = wait_for_exit_buffering(&mut ws, "m-new", "sub-shell", &mut buffered).await?;
    let new_id = new_res["projectId"]
        .as_str()
        .ok_or("missing projectId")?
        .to_owned();
    let new_ev = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "project.updated" && v["project"]["id"] == new_id
    })
    .await?;
    assert_eq!(new_ev["project"]["title"], "Brand New");

    // Incremental resume from before the mutations must also replay the created projects.
    let mut ws2 = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    let mut resumed_buf = Vec::new();
    send_req(
        &mut ws2,
        "sub-resume",
        "orchestration.v2.subscribeShell",
        json!({ "afterSequence": start_seq }),
    )
    .await?;
    let resumed_ev = wait_for_shell_event(&mut ws2, "sub-resume", &mut resumed_buf, |v| {
        v["kind"] == "project.updated" && v["project"]["id"] == "proj-live-1"
    })
    .await?;
    assert_eq!(resumed_ev["project"]["title"], "LiveProject");
    drop(ws2);

    send_req(
        &mut ws,
        "m-del",
        "projects.mutate",
        json!({ "type": "project.delete", "commandId": "cmd-p-del", "projectId": "proj-live-1", "force": true }),
    )
    .await?;
    let _ = wait_for_exit_buffering(&mut ws, "m-del", "sub-shell", &mut buffered).await?;
    let removed_ev = wait_for_shell_event(&mut ws, "sub-shell", &mut buffered, |v| {
        v["kind"] == "project.removed" && v["projectId"] == "proj-live-1"
    })
    .await?;
    assert_eq!(removed_ev["projectId"], "proj-live-1");

    drop(ws);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn implicit_dev_url_uses_dev_state_dir_instead_of_userdata() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base = temp.path().join("legion-home");

    let dev_server =
        spawn_dev_server(&base, false, Some("http://localhost:5733".to_owned())).await?;
    assert_eq!(dev_server.paths().state_dir, base.join("dev"));
    dev_server.shutdown().await?;

    let prod_server = spawn_dev_server(&base, false, None).await?;
    assert_eq!(prod_server.paths().state_dir, base.join("userdata"));
    prod_server.shutdown().await?;
    Ok(())
}
