//! E2E-M5 Tier 3 pairwise Rust CLI tests (`t3_r01`..`t3_r06`).

use std::fs;

use legion_protocol::rpc_codec::RpcExit;
use serde_json::json;

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

use super::helpers::{
    assert_auth_error, connect_ws_at, create_and_approve_m1_spec, create_project, err_tag_and_code,
    mint_scoped_session, next_id, probe_m2_planning, start_m1_ws, ws_call, ws_ok,
};
use super::helpers_m3::{CrashServer, setup_m3_card};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r01_f13_f22_launch_strictness_and_ws_scope_guards() -> TestResult<()> {
    let h = OpaqueBoxHarness::start(true)?;
    let read_tok = h
        .pair_client("t3-r01-read", Some(&["orchestration:read"]))
        .await?;
    let mut ws = connect_ws_at(h.addr()?, &read_tok).await?;
    let bad = ws_call(
        &mut ws,
        "orchestration.launchThread",
        json!({
            "commandId": next_id("cmd"),
            "projectId": "missing-proj",
            "title": "No Fallback",
            "modelSelection": { "instanceId": "devReplay", "model": "" },
            "runtimeMode": "approval-required",
            "interactionMode": "default",
            "workspaceStrategy": { "type": "root" },
        }),
    )
    .await?;
    assert!(matches!(bad, RpcExit::Failure { .. }));
    let c_den = ws_call(
        &mut ws,
        "workflowBoard.createCard",
        json!({ "projectId": "proj-1", "title": "Denied" }),
    )
    .await?;
    assert_auth_error(
        &c_den,
        "orchestration:operate",
        "orchestration:operate",
        "createCard",
    )?;
    let a_den = ws_call(
        &mut ws,
        "workflowBoard.approveSpec",
        json!({ "cardId": "card-1", "expectedRevision": 1 }),
    )
    .await?;
    assert_auth_error(
        &a_den,
        "orchestration:operate",
        "orchestration:operate",
        "approveSpec",
    )?;
    let n: i64 = h.open_db_readonly()?.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r02_f4_f20_brief_path_traversal_and_atomic_sqlite_rejection() -> TestResult<()> {
    let (h, _, mut ws) = start_m1_ws("t3_r02").await?.ok_or("required workflow board scenario setup")?;
    fs::write(h.workspace_dir().join(".env"), "SECRET=123\n")?;
    fs::write(h.workspace_dir().join("bin.md"), b"bad\0null\xfe")?;
    create_project(&mut ws, "proj-t3-r02", "T3 R02", h.workspace_dir()).await?;
    for bad in [".env", "../../etc/passwd", "bin.md"] {
        let exit = ws_call(
            &mut ws,
            "workflowBoard.createCard",
            json!({
                "projectId": "proj-t3-r02",
                "title": "Bad",
                "briefPath": bad,
                "sourceType": "file",
            }),
        )
        .await?;
        assert!(matches!(exit, RpcExit::Failure { .. }), "path={bad}");
    }
    let cnt: i64 =
        h.open_db_readonly()?
            .query_row("SELECT COUNT(*) FROM workflow_board_cards", [], |r| {
                r.get(0)
            })?;
    assert_eq!(cnt, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r03_f8_f22_concurrent_update_spec_vs_approve_spec_cas() -> TestResult<()> {
    let (h, tok, mut ws1) = start_m1_ws("t3_r03").await?.ok_or("required workflow board scenario setup")?;
    let mut ws2 = connect_ws_at(h.addr()?, &tok).await?;
    create_project(&mut ws1, "proj-t3-r03", "T3 R03", h.workspace_dir()).await?;
    let created = ws_ok(
        &mut ws1,
        "workflowBoard.createCard",
        json!({ "projectId": "proj-t3-r03", "title": "CAS", "brief": "Initial" }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("missing cardId")?;
    let rev = created["revision"].as_i64().ok_or("missing rev")?;
    let (r1, r2) = tokio::join!(
        ws_call(
            &mut ws1,
            "workflowBoard.updateSpec",
            json!({ "cardId": cid, "expectedRevision": rev, "brief": "Updated" })
        ),
        ws_call(
            &mut ws2,
            "workflowBoard.approveSpec",
            json!({ "cardId": cid, "expectedRevision": rev })
        )
    );
    let (e1, e2) = (r1?, r2?);
    assert_eq!(
        usize::from(matches!(e1, RpcExit::Success { .. }))
            + usize::from(matches!(e2, RpcExit::Success { .. })),
        1
    );
    let loser = if matches!(e1, RpcExit::Failure { .. }) {
        &e1
    } else {
        &e2
    };
    assert_eq!(err_tag_and_code(loser).1, Some("revision_conflict"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r04_f20_f21_torn_write_journal_recovery_and_plan_brief() -> TestResult<()> {
    let mut h = CrashServer::start()?;
    let wt = h.temp_dir.path().join("wt-t3-r04");
    create_worktree(&h.workspace_dir, &wt, "feat/t3-r04")?;
    let mut ws = h.pair_and_connect("t3-r04").await?;
    let _ = ws_ok(
        &mut ws,
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;
    create_project(&mut ws, "proj-t3-r04", "T3 R04", &h.workspace_dir).await?;
    let (cid, rev, _) = create_and_approve_m1_spec(&mut ws, "proj-t3-r04", &wt, "Journal").await?;
    drop(ws);

    let card_dir = wt.join(".legion").join("workflows").join(&cid);
    h.inject_torn_journal_and_restart(&card_dir, &cid, "pending")?;

    let mut ws2 = h.pair_and_connect("t3-r04-b").await?;
    let started = ws_ok(
        &mut ws2,
        "workflowBoard.startStage",
        json!({ "cardId": cid, "expectedRevision": rev, "stage": "plan" }),
    )
    .await?;
    assert_eq!(started["stage"], "plan");
    let brief = ws_ok(
        &mut ws2,
        "workflowBoard.getArtifact",
        json!({ "cardId": cid, "key": "plan-brief.md" }),
    )
    .await?;
    assert!(
        brief["content"]
            .as_str()
            .is_some_and(|s| s.contains("AC-01"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r05_f2_f22_artifact_traversal_and_large_frame_parts_roundtrip() -> TestResult<()> {
    let (h, tok, mut ws) = start_m1_ws("t3_r05").await?.ok_or("required workflow board scenario setup")?;
    create_project(&mut ws, "proj-t3-r05", "T3 R05", h.workspace_dir()).await?;
    let created = ws_ok(
        &mut ws,
        "workflowBoard.createCard",
        json!({ "projectId": "proj-t3-r05", "title": "FrameParts", "brief": "Small" }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("missing cardId")?;
    let rev = created["revision"].as_i64().ok_or("missing rev")?;
    for key in ["../.env", ".git/config"] {
        let exit = ws_call(
            &mut ws,
            "workflowBoard.getArtifact",
            json!({ "cardId": cid, "key": key }),
        )
        .await?;
        assert!(matches!(exit, RpcExit::Failure { .. }), "key={key}");
    }
    let hdr = format!("Bearer {tok}");
    let mut no_fp = OpaqueWsClient::connect(
        h.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &hdr)],
    )
    .await?;
    let large = "FRAME_PARTS_SENTINEL_".repeat(3600);
    let up_body = json!({ "cardId": cid, "expectedRevision": rev, "brief": large });
    assert!(
        ws_ok(&mut no_fp, "workflowBoard.updateSpec", up_body).await?["revision"]
            .as_i64()
            .unwrap_or(0)
            > rev
    );
    let art_body = json!({ "cardId": cid, "kind": "spec" });
    let art = ws_ok(&mut no_fp, "workflowBoard.getArtifact", art_body).await?;
    assert!(
        art["content"]
            .as_str()
            .is_some_and(|s| s.contains("FRAME_PARTS_SENTINEL_"))
    );
    assert!(ws_ok(&mut no_fp, "server.probe", json!({})).await?["serverTime"].is_string());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r06_f3_f8_return_for_revision_history_and_human_only_guard() -> TestResult<()> {
    let (h, tok, mut ws) = start_m1_ws("t3_r06").await?.ok_or("required workflow board scenario setup")?;
    assert!(probe_m2_planning(&mut ws, "t3_r06").await?, "required workflow board capability");
    let (_wt, cid, plan_rev) = setup_m3_card(&h, &mut ws, "t3-r06", "outcomes").await?;
    let agent_tok = mint_scoped_session(
        h.origin()?,
        &tok,
        "thread:thr-agent-r06",
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;
    let mut agent_ws = connect_ws_at(h.addr()?, &agent_tok).await?;
    let denied = ws_call(
        &mut agent_ws,
        "workflowBoard.returnForRevision",
        json!({
            "cardId": cid,
            "expectedRevision": plan_rev,
            "targetStage": "spec",
            "reason": "Blocked",
        }),
    )
    .await?;
    assert_auth_error(
        &denied,
        "orchestration:operate",
        "client:interactive",
        "returnForRevision",
    )?;
    let ret = ws_ok(
        &mut ws,
        "workflowBoard.returnForRevision",
        json!({
            "cardId": cid,
            "expectedRevision": plan_rev,
            "targetStage": "spec",
            "reason": "Revisit AC-02",
        }),
    )
    .await?;
    assert_eq!(ret["stage"], "spec");
    assert_eq!(ret["planApproval"]["status"], "stale");
    Ok(())
}
