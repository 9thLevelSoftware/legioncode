//! E2E-M5 Tier 4 scenarios S7b, S10, and S12 (interrupted resume, CLI locks/policies, auth/capabilities).

use std::fs;

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::{
    CliLockKind, CopyLegionCliOpts, copy_disposable_legion_cli, create_worktree,
    seed_active_cli_lock,
};
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

use super::helpers::{
    assert_cli_import_card_fields, assert_domain_failure, connect_ws_at,
    create_and_approve_m1_spec, create_project, mint_scoped_session, probe_m4_cli_import,
    require_m1_board, ws_call, ws_ok,
};
use super::helpers_m3::{
    complete_m2_plan, run_check, start_crash_m3_if_ready, start_stage_settled,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s07b_interrupted_external_action_reconciles_before_retry() -> TestResult<()> {
    let (mut h, mut ws) = start_crash_m3_if_ready(&[], "t4-s07b")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let wt = h.temp_dir.path().join("wt-t4-s07b");
    create_worktree(&h.workspace_dir, &wt, "feat/t4-s07b")?;
    create_project(&mut ws, "proj-t4-s07b", "Scenario 7b", &h.workspace_dir).await?;
    let (cid, spec_rev, _) =
        create_and_approve_m1_spec(&mut ws, "proj-t4-s07b", &wt, "Interrupted check").await?;
    let plan_rev = complete_m2_plan(&mut ws, &cid, spec_rev, false, "outcomes", None).await?;
    let (_, executed) =
        start_stage_settled(&mut ws, &cid, &json!(plan_rev), "execute", None).await?;
    let paused = ws_ok(
        &mut ws,
        "workflowBoard.pauseCard",
        json!({ "cardId": cid, "expectedRevision": executed["revision"] }),
    )
    .await?;
    assert_eq!(paused["status"], "paused");

    let edit = wt.join("external-edit.txt");
    fs::write(&edit, "dirty external edit during pause\n")?;
    drop(ws);
    h.inject_interrupted_check_and_restart(&cid)?;
    let mut ws = h.pair_and_connect("t4-s07b-re").await?;
    let unreconciled = ws_call(
        &mut ws,
        "workflowBoard.resumeCard",
        json!({ "cardId": cid, "expectedRevision": paused["revision"] }),
    )
    .await?;
    assert_domain_failure(
        &unreconciled,
        &["interrupted", "unreconciled"],
        "resume without reconciliation",
    )?;
    let resumed = ws_ok(
        &mut ws, "workflowBoard.resumeCard",
        json!({ "cardId": cid, "expectedRevision": paused["revision"], "reconcileInterrupted": true }),
    ).await?;
    assert_eq!(resumed["cardId"], cid);
    assert_ne!(resumed["status"], "paused");
    assert_eq!(resumed["reconciledCount"], 1);
    let checked = run_check(&mut ws, &resumed["card"]).await?;
    assert_eq!(checked["checkReceipts"][0]["status"], "passed");
    assert_eq!(
        fs::read_to_string(edit)?,
        "dirty external edit during pause\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s10_cli_import_safety_guards_active_locks_and_policy_gate() -> TestResult<()> {
    let h = OpaqueBoxHarness::start(true)?;
    assert!(
        require_m1_board(&h, "t4_s10").await?,
        "required workflow board capability"
    );
    let tok = h.pair_client("t4-s10", None).await?;
    let mut ws = connect_ws_at(h.addr()?, &tok).await?;
    assert!(
        probe_m4_cli_import(&mut ws, "t4_s10").await?,
        "required workflow board capability"
    );
    create_project(&mut ws, "proj-t4-s10", "Scenario 10", h.workspace_dir()).await?;
    let _ = copy_disposable_legion_cli(h.workspace_dir(), CopyLegionCliOpts::default())?;
    assert!(
        !h.workspace_dir().join(".legion-cli").join("cache").exists(),
        "missing gitignored cache/ directory must be tolerated"
    );

    let mut lock_paths = Vec::new();
    for kind in [
        CliLockKind::EngineLock,
        CliLockKind::LiveSpawn,
        CliLockKind::ResumeJson,
    ] {
        let lock = seed_active_cli_lock(h.workspace_dir(), kind)?;
        let refused = ws_call(
            &mut ws,
            "workflowBoard.applyCliImport",
            json!({ "projectId": "proj-t4-s10" }),
        )
        .await?;
        assert_domain_failure(
            &refused,
            &["active", "lock", "running", "cli", "refus"],
            "t4_s10 active lock refusal",
        )?;
        lock_paths.push(lock);
    }
    for lock in lock_paths {
        fs::remove_file(lock)?;
    }

    let imported = ws_ok(
        &mut ws,
        "workflowBoard.applyCliImport",
        json!({ "projectId": "proj-t4-s10" }),
    )
    .await?;
    let cid = imported["cardId"].as_str().ok_or("missing cardId")?;
    let detail = ws_ok(&mut ws, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_cli_import_card_fields(&detail)?;
    let rev = detail["revision"].as_i64().ok_or("missing rev")?;

    let blocked = ws_call(
        &mut ws,
        "workflowBoard.startStage",
        json!({ "cardId": cid, "expectedRevision": rev, "stage": "execute" }),
    )
    .await?;
    assert_domain_failure(
        &blocked,
        &[
            "polic",
            "acknowledg",
            "requirehardened",
            "passscore",
            "trustkeys",
            "adopted",
        ],
        "t4_s10 unsupported policy gate",
    )?;
    Ok(())
}

async fn verify_origin_and_ticket_replay(h: &OpaqueBoxHarness, admin_tok: &str) -> TestResult<()> {
    let addr = h.addr()?;
    let mut bad_origin = TcpStream::connect(addr).await?;
    let bad_req = format!(
        "GET /ws?orchestrationProtocol=2 HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://evil.example\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    bad_origin.write_all(bad_req.as_bytes()).await?;
    let mut buf = [0_u8; 512];
    let n = bad_origin.read(&mut buf).await?;
    let head = std::str::from_utf8(&buf[..n])?;
    assert!(head.starts_with("HTTP/1.1 403"), "got {head}");

    let ticket_resp = reqwest::Client::new()
        .post(format!("{}/api/auth/websocket-ticket/rpc", h.origin()?))
        .header("authorization", format!("Bearer {admin_tok}"))
        .header(CONTENT_TYPE, "application/json")
        .send()
        .await?;
    assert_eq!(ticket_resp.status(), 200);
    let ticket_body: Value = ticket_resp.json().await?;
    let ticket = ticket_body["ticket"].as_str().ok_or("missing ticket")?;
    let path = format!("/ws?orchestrationProtocol=2&wsTicket={ticket}");
    let ws_once = OpaqueWsClient::connect(addr, &path, &[]).await?;
    drop(ws_once);

    let mut replay_stream = TcpStream::connect(addr).await?;
    let replay_req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    replay_stream.write_all(replay_req.as_bytes()).await?;
    let rn = replay_stream.read(&mut buf).await?;
    let rhead = std::str::from_utf8(&buf[..rn])?;
    assert!(rhead.starts_with("HTTP/1.1 401"), "got {rhead}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s12_capability_negotiation_auth_scopes_origin_and_ticket_replay() -> TestResult<()> {
    let h = OpaqueBoxHarness::start(true)?;
    let env_desc = h.probe_environment().await?;
    assert_eq!(env_desc["orchestrationProtocolVersion"], 2);
    assert_eq!(env_desc["capabilities"]["projectWorkflowBoard"], true);

    let admin_tok = h.pair_client("t4-s12-admin", None).await?;
    let read_tok = h
        .pair_client("t4-s12-read", Some(&["orchestration:read"]))
        .await?;
    let mut admin_ws = connect_ws_at(h.addr()?, &admin_tok).await?;
    let mut read_ws = connect_ws_at(h.addr()?, &read_tok).await?;

    create_project(
        &mut admin_ws,
        "proj-t4-s12",
        "Scenario 12",
        h.workspace_dir(),
    )
    .await?;
    let created = ws_ok(
        &mut admin_ws,
        "workflowBoard.createCard",
        json!({ "projectId": "proj-t4-s12", "title": "Auth Guard Card", "brief": "Guard approvals." }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("missing cardId")?;
    let rev = created["revision"].as_i64().ok_or("missing rev")?;

    let read_create = ws_call(
        &mut read_ws,
        "workflowBoard.createCard",
        json!({ "projectId": "proj-t4-s12", "title": "Denied" }),
    )
    .await?;
    assert!(matches!(read_create, RpcExit::Failure { .. }));

    for subject in ["thread:thr-s12", "mcp-oauth:oauth-s12"] {
        let agent_tok = mint_scoped_session(
            h.origin()?,
            &admin_tok,
            subject,
            &["orchestration:read", "orchestration:operate"],
        )
        .await?;
        let mut agent_ws = connect_ws_at(h.addr()?, &agent_tok).await?;
        let exit = ws_call(
            &mut agent_ws,
            "workflowBoard.approveSpec",
            json!({ "cardId": cid, "expectedRevision": rev }),
        )
        .await?;
        match exit {
            RpcExit::Failure { cause } => {
                let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                    return Err(format!("expected Fail for {subject}").into());
                };
                assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
                assert_eq!(error["requiredPermission"], "client:interactive");
            }
            RpcExit::Success { .. } => {
                return Err(format!("{subject} unexpectedly passed approveSpec").into());
            }
        }
    }

    let mcp_client_tok = mint_scoped_session(
        h.origin()?,
        &admin_tok,
        "mcp-client:cli-s12",
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;
    assert!(
        connect_ws_at(h.addr()?, &mcp_client_tok).await.is_err(),
        "mcp-client session must be rejected on /ws upgrade"
    );

    verify_origin_and_ticket_replay(&h, &admin_tok).await
}
