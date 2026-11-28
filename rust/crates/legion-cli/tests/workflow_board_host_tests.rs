//! Host restart & live WS/RPC integration tests for Project Workflow Board (M1.3).

#[path = "support/ws_client.rs"]
mod ws_client;

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::pairing_tokens::{
    IssuePairingTokenInput, STANDARD_CLIENT_SCOPES, issue_pairing_token_with_conn,
};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcCauseReason, RpcExit, RpcId, ServerRpcFrame, decode_server_frames,
    encode_client_frame,
};
use serde_json::{Value, json};
use ws_client::WsTestClient;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let st = StdCommand::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .status()
            .expect("git exec");
        assert!(st.success(), "git {args:?} failed");
    };
    run(&["init"]);
    run(&["config", "user.email", "test@legion.local"]);
    run(&["config", "user.name", "Legion Test"]);
    fs::write(dir.join("README.md"), "# Host Test Repo\n").expect("write readme");
    run(&["add", "."]);
    run(&["commit", "-m", "initial commit"]);
}

async fn spawn_srv(base: &Path, cwd: &Path) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server(ServeConfig {
        base_dir: base.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: cwd.to_path_buf(),
    })
    .await
    .map_err(Into::into)
}

async fn call_unary(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
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
    ws.send_text(&encode_client_frame(&frame)?).await?;
    loop {
        for resp in decode_server_frames(&ws.recv_text().await?)? {
            if let ServerRpcFrame::Exit { request_id, exit } = resp
                && request_id == RpcId::from(id)
            {
                return match exit {
                    RpcExit::Success { value } => Ok(value),
                    RpcExit::Failure { cause } => Err(format!("{tag} failed: {cause:?}").into()),
                };
            }
        }
    }
}

async fn next_chunk_for(
    ws: &mut WsTestClient,
    id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        for resp in decode_server_frames(&ws.recv_text().await?)? {
            if let ServerRpcFrame::Chunk { request_id, values } = resp
                && request_id == RpcId::from(id)
                && let Some(first) = values.into_iter().next()
            {
                return Ok(first);
            }
        }
    }
}

async fn pair_client_token(
    server: &RunningServer,
    label: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let credential = {
        let store = SqliteStore::open(&server.paths().db_path)?;
        issue_pairing_token_with_conn(
            store.connection(),
            &IssuePairingTokenInput {
                scopes: STANDARD_CLIENT_SCOPES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                ..IssuePairingTokenInput::default()
            },
            now_ms,
        )?
        .credential
    };
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": credential, "clientLabel": label }))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    Ok(body["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned())
}

#[tokio::test]
async fn host_restart_preserves_approved_spec_and_streams_board_snapshots() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let base_dir = tmp.path().join("host-state");
    let ws_dir = tmp.path().join("host-repo");
    fs::create_dir_all(&ws_dir)?;
    init_git_repo(&ws_dir);

    let (card_id, approved_rev, approved_digest) =
        phase1_create_subscribe_approve(&base_dir, &ws_dir).await?;
    let server2 = spawn_srv(&base_dir, &ws_dir).await?;
    phase2_restart_start_and_check_auth(&server2, &card_id, approved_rev, &approved_digest).await?;
    server2.shutdown().await?;
    Ok(())
}

async fn phase1_create_subscribe_approve(
    base_dir: &Path,
    ws_dir: &Path,
) -> Result<(String, u64, String), Box<dyn std::error::Error>> {
    let server = spawn_srv(base_dir, ws_dir).await?;
    let store = SqliteStore::open(&server.paths().db_path)?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(ws_dir.to_string_lossy().as_ref())
            .with_project_id("proj-host-1")
            .with_title("Host Repo"),
    )?;
    let tok1 = pair_client_token(&server, "client-1").await?;
    let mut ws1 = WsTestClient::connect(
        server.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={tok1}"),
    )
    .await?;

    ws1.send_text(&encode_client_frame(&ClientRpcFrame::Request {
        id: RpcId::from("sub-1"),
        tag: "workflowBoard.subscribe".to_owned(),
        payload: json!({ "projectId": "proj-host-1" }),
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    })?)
    .await?;
    let init_snap = next_chunk_for(&mut ws1, "sub-1").await?;
    assert_eq!(init_snap["cards"].as_array().map(Vec::len), Some(0));

    let created = call_unary(
        &mut ws1,
        "c-1",
        "workflowBoard.createCard",
        json!({
            "projectId": "proj-host-1",
            "title": "Host Restart Card",
            "brief": "Initial brief",
            "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
            "runtimeMode": "full-access",
            "interactionMode": "default"
        }),
    )
    .await?;
    let cid = created["cardId"]
        .as_str()
        .ok_or("missing cardId")?
        .to_owned();
    let after_create_snap = next_chunk_for(&mut ws1, "sub-1").await?;
    assert_eq!(after_create_snap["cards"].as_array().map(Vec::len), Some(1));

    let approved = call_unary(
        &mut ws1,
        "a-1",
        "workflowBoard.approveSpec",
        json!({ "cardId": cid, "expectedRevision": created["revision"] }),
    )
    .await?;
    let rev = approved["revision"].as_u64().ok_or("missing revision")?;
    let digest = approved["specApproval"]["specDigest"]
        .as_str()
        .ok_or("missing specDigest")?
        .to_owned();
    server.shutdown().await?;
    Ok((cid, rev, digest))
}

async fn phase2_restart_start_and_check_auth(
    server2: &RunningServer,
    card_id: &str,
    approved_rev: u64,
    approved_digest: &str,
) -> TestResult {
    let tok2 = pair_client_token(server2, "client-2").await?;
    let mut ws2 = WsTestClient::connect(
        server2.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={tok2}"),
    )
    .await?;

    let card2 = call_unary(
        &mut ws2,
        "g-2",
        "workflowBoard.getCard",
        json!({ "cardId": card_id, "projectId": "proj-host-1" }),
    )
    .await?;
    assert_eq!(card2["revision"], json!(approved_rev));
    assert_eq!(card2["specApproval"]["status"], json!("valid"));
    assert_eq!(card2["specApproval"]["specDigest"], json!(approved_digest));

    let _ = call_unary(
        &mut ws2,
        "set-wf",
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;
    let start1 = call_unary(
        &mut ws2,
        "s-1",
        "workflowBoard.startStage",
        json!({
            "cardId": card_id,
            "expectedRevision": approved_rev,
            "stage": "plan"
        }),
    )
    .await?;
    assert_eq!(start1["resumed"], json!(false));
    let tid = start1["threadId"]
        .as_str()
        .ok_or("missing threadId")?
        .to_owned();

    let start2 = call_unary(
        &mut ws2,
        "s-2",
        "workflowBoard.startStage",
        json!({
            "cardId": card_id,
            "expectedRevision": start1["revision"],
            "stage": "plan"
        }),
    )
    .await?;
    assert_eq!(start2["resumed"], json!(true));
    assert_eq!(start2["threadId"], json!(tid));

    let art = call_unary(
        &mut ws2,
        "art-1",
        "workflowBoard.getArtifact",
        json!({ "cardId": card_id, "key": "plan-brief.md" }),
    )
    .await?;
    assert_eq!(art["key"], json!("plan-brief.md"));

    verify_read_only_approve_denied(server2, card_id).await
}

async fn verify_read_only_approve_denied(server2: &RunningServer, card_id: &str) -> TestResult {
    let ro_tok = {
        let store = SqliteStore::open(&server2.paths().db_path)?;
        issue_scoped_session(
            store.connection(),
            IssueSessionInput {
                subject: Some("one-time-token".to_owned()),
                scopes: vec!["orchestration:read".to_owned()],
                ..IssueSessionInput::default()
            },
        )?
        .token
    };
    let mut ws_ro = WsTestClient::connect(
        server2.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={ro_tok}"),
    )
    .await?;
    ws_ro
        .send_text(&encode_client_frame(&ClientRpcFrame::Request {
            id: RpcId::from("ro-1"),
            tag: "workflowBoard.approveSpec".to_owned(),
            payload: json!({ "cardId": card_id, "expectedRevision": 1 }),
            headers: Vec::new(),
            is_notification: false,
            discard: false,
            trace_id: None,
            span_id: None,
            sampled: None,
        })?)
        .await?;
    let mut denied = false;
    while !denied {
        for resp in decode_server_frames(&ws_ro.recv_text().await?)? {
            if let ServerRpcFrame::Exit {
                request_id,
                exit: RpcExit::Failure { cause },
            } = resp
                && request_id == RpcId::from("ro-1")
                && let Some(RpcCauseReason::Fail { error }) = cause.first()
            {
                assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
                assert_eq!(error["requiredScope"], "orchestration:operate");
                denied = true;
            }
        }
    }
    Ok(())
}

#[path = "workflow_board_host/m2_cases.rs"]
mod m2_cases;
