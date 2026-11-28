//! F17: Independent review in fresh thread (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, execute_and_review_for_ship, expect_rpc_err_any_tag,
    expect_rpc_err_tag, get_card, require_m3_execute_ship, setup_approved_plan_card,
    start_stage_settled, start_stage_settled_with_model,
};
use super::support_m3_mocks::install_mock_providers;
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

async fn f17_setup_card(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<(OpaqueWsClient, String, i64)> {
    assert_m3_execute_ship(h, tag).await?;
    let wt = h.temp_root().join(format!("wt-{tag}"));
    create_worktree(h.workspace_dir(), &wt, &format!("feat/{tag}"))?;
    let tok = h.pair_client(tag, None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, proj, title, h.workspace_dir()).await?;
    install_mock_providers(&mut ws, None).await?;
    let (card_id, rev) = setup_approved_plan_card(&mut ws, proj, &wt, title, None).await?;
    Ok((ws, card_id, rev))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t1_1_review_launches_in_fresh_thread_distinct_from_execute() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f17_setup_card(&h, "f17-t1-1", "proj-f17-1", "F17 Fresh Card").await?;

    let exec_res = start_stage_settled(&mut ws, &card_id, rev, "execute", None).await?;
    let exec_tid = exec_res["threadId"].as_str().ok_or("missing exec tid")?;
    let rev2 = exec_res["revision"].as_i64().unwrap_or(rev + 1);
    let rev_res = ws
        .call_ok(
            &next_id("rev"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": rev2, "stage": "review" }),
        )
        .await?;
    assert_ne!(
        rev_res["threadId"].as_str().ok_or("missing rev tid")?,
        exec_tid
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t1_2_structured_pass_review_records_snapshot_bound_receipt() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f17_setup_card(&h, "f17-t1-2", "proj-f17-2", "F17 Pass Card").await?;
    let _ = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        rev,
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-pass", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["reviewReport"]["verdict"], "pass");
    assert_eq!(card["reviewReport"]["status"], "completed");
    assert!(card["reviewReport"]["productSnapshotDigest"].is_string());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t1_3_review_findings_block_ship_until_human_disposition() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) = f17_setup_card(
        &h,
        "f17-t1-3",
        "proj-f17-3",
        "F17 Find Card [REVIEW_FINDING]",
    )
    .await?;
    let exec = start_stage_settled(&mut ws, &card_id, rev, "execute", None).await?;
    let rev_e = exec["revision"].as_i64().unwrap_or(rev + 1);
    let _ = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        rev_e,
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-findings", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    let card = get_card(&mut ws, &card_id).await?;
    let findings = card["reviewReport"]["findings"]
        .as_array()
        .ok_or("missing reviewReport.findings")?;
    let first = findings.first().ok_or("expected non-empty findings")?;
    assert_eq!(first["status"], "open");
    assert_eq!(first["disposition"], "unresolved");
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": card["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    let fid = findings[0]["id"].as_str().ok_or("missing finding id")?;
    let resolved = ws
        .call_ok(
            &next_id("rf"),
            "workflowBoard.resolveFinding",
            json!({
                "cardId": card_id, "expectedRevision": card["revision"], "findingId": fid,
                "disposition": "accept_risk", "reason": "Accepted"
            }),
        )
        .await?;
    assert!(resolved["revision"].as_i64().unwrap_or(0) > card["revision"].as_i64().unwrap_or(0));
    assert_eq!(resolved["finding"]["status"], "resolved");
    assert_eq!(resolved["finding"]["disposition"], "accept_risk");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t1_4_resolve_finding_disposition_persists_across_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) = f17_setup_card(
        &h,
        "f17-t1-4",
        "proj-f17-4",
        "F17 Disp Card [REVIEW_FINDING]",
    )
    .await?;
    let _ = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        rev,
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-findings", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    let before = get_card(&mut ws, &card_id).await?;
    let fid = before["reviewReport"]["findings"][0]["id"]
        .as_str()
        .ok_or("missing finding id")?;
    let _ = ws
        .call_ok(
            &next_id("res"),
            "workflowBoard.resolveFinding",
            json!({
                "cardId": card_id, "expectedRevision": before["revision"], "findingId": fid,
                "disposition": "accept_risk", "reason": "Persisted across restart"
            }),
        )
        .await?;
    drop(ws);
    h.restart_server()?;
    let tok2 = h.pair_client("f17-t1-4b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    assert_eq!(card["reviewReport"]["findings"][0]["status"], "resolved");
    assert_eq!(
        card["reviewReport"]["findings"][0]["disposition"],
        "accept_risk"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t1_5_rereview_after_fix_launches_another_fresh_thread() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f17_setup_card(&h, "f17-t1-5", "proj-f17-5", "F17 Re Card [REVIEW_FINDING]").await?;
    let r1 = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        rev,
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-findings", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    let mid = get_card(&mut ws, &card_id).await?;
    let fid = mid["reviewReport"]["findings"][0]["id"]
        .as_str()
        .ok_or("missing finding id")?;
    let resolved = ws
        .call_ok(
            &next_id("rf"),
            "workflowBoard.resolveFinding",
            json!({
                "cardId": card_id, "expectedRevision": mid["revision"], "findingId": fid,
                "disposition": "fix", "reason": "Fixed"
            }),
        )
        .await?;
    let r2 = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        resolved["revision"].as_i64().unwrap_or(rev + 2),
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-pass", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    assert_ne!(r1["threadId"], r2["threadId"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t2_1_agent_token_rejected_on_resolve_finding() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let agent_tok = h.mint_agent_thread_token("thr-agent-17").await?;
    let mut agent_ws = h.connect_ws(&agent_tok).await?;

    let err = expect_rpc_err_tag(
        &mut agent_ws,
        "workflowBoard.resolveFinding",
        json!({
            "cardId": "card-17",
            "expectedRevision": 1,
            "findingId": "fnd-1",
            "disposition": "accept-risk",
        }),
        "EnvironmentAuthorizationError",
    )
    .await?;
    assert_eq!(err["requiredPermission"], "client:interactive");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t2_2_snapshot_mutation_after_pass_review_stales_review_and_blocks_ship()
-> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f17_t2_2_snapshot_mutation_after_pass_review_stales_review_and_blocks_ship",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f17-stale");
    create_worktree(h.workspace_dir(), &wt, "feat/f17-stale")?;
    let tok = h.pair_client("f17-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f17-6", "F17 Stale", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f17-6", &wt, "F17 Stale Card", None).await?;
    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let _ = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": card_id, "expectedRevision": rev_ready, "criterionId": "AC-2", "verdict": "pass", "note": "OK" }),
        )
        .await?;
    fs::write(wt.join("README.md"), "# Mutated after review\n")?;
    let stale_card = get_card(&mut ws, &card_id).await?;
    assert_eq!(stale_card["reviewReport"]["status"], "stale");
    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": stale_card["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    assert!(err.to_string().to_lowercase().contains("stale"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t2_3_resolve_finding_with_stale_revision_or_unknown_id_rejected() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f17_t2_3_resolve_finding_with_stale_revision_or_unknown_id_rejected",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f17-badid");
    create_worktree(h.workspace_dir(), &wt, "feat/f17-badid")?;
    let tok = h.pair_client("f17-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f17-7", "F17 BadId", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f17-7", &wt, "F17 BadId Card", None).await?;
    let err_rev = expect_rpc_err_any_tag(&mut ws, "workflowBoard.resolveFinding", json!({ "cardId": card_id, "expectedRevision": rev - 1, "findingId": "FND-1", "disposition": "fix" }), &["WorkflowBoardRequestError"]).await?;
    assert_eq!(err_rev["code"], "revision_conflict");
    let _ = expect_rpc_err_any_tag(&mut ws, "workflowBoard.resolveFinding", json!({ "cardId": card_id, "expectedRevision": rev, "findingId": "unknown-finding", "disposition": "fix" }), &["WorkflowBoardRequestError"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t2_4_malformed_review_output_never_silently_marks_pass() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f17_t2_4_malformed_review_output_never_silently_marks_pass",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f17-mal");
    create_worktree(h.workspace_dir(), &wt, "feat/f17-mal")?;
    let tok = h.pair_client("f17-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f17-8", "F17 Mal", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f17-8",
        &wt,
        "F17 Mal Card [MALFORMED_REVIEW]",
        None,
    )
    .await?;
    let _ = start_stage_settled(&mut ws, &card_id, rev, "review", None).await?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["reviewReport"]["status"], "failed");
    assert_ne!(card["reviewReport"]["verdict"], "pass");
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": card["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f17_t2_5_return_from_review_to_execute_preserves_findings_history() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f17_t2_5_return_from_review_to_execute_preserves_findings_history",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f17-ret");
    create_worktree(h.workspace_dir(), &wt, "feat/f17-ret")?;
    let tok = h.pair_client("f17-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f17-9", "F17 Ret", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f17-9",
        &wt,
        "F17 Ret Card [REVIEW_FINDING]",
        None,
    )
    .await?;
    let _ = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let before = get_card(&mut ws, &card_id).await?;
    assert!(
        before["reviewReport"]["findings"]
            .as_array()
            .is_some_and(|f| !f.is_empty())
    );
    let same_stage = expect_rpc_err_tag(&mut ws, "workflowBoard.returnForRevision", json!({
        "cardId": card_id, "expectedRevision": before["revision"], "targetStage": "execute", "reason": "Fix review item",
    }), "WorkflowBoardRequestError").await?;
    assert_eq!(same_stage["code"], "invalid_stage_transition");
    assert_eq!(
        same_stage["message"],
        "target_stage `execute` must be strictly earlier than current stage `execute`"
    );
    let ret = ws.call_ok(&next_id("ret"), "workflowBoard.returnForRevision", json!({
        "cardId": card_id, "expectedRevision": before["revision"], "targetStage": "plan", "reason": "Revise plan for the review finding",
    })).await?;
    assert_eq!(ret["stage"], "plan");
    let finding_id = before["reviewReport"]["findings"][0]["id"]
        .as_str()
        .ok_or("missing prior finding id")?;
    let db = h.open_db_readonly()?;
    let status: String = db.query_row(
        "SELECT status FROM workflow_board_review_findings WHERE id = ?1 AND card_id = ?2",
        [finding_id, &card_id],
        |row| row.get(0),
    )?;
    assert_eq!(status, "stale");
    assert!(
        ret["history"]
            .as_array()
            .is_some_and(|history| !history.is_empty())
    );
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": ret["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}
