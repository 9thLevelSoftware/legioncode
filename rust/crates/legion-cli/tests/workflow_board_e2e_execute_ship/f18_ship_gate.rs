//! F18: Ship delivery gate & criterion observations (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, execute_and_review_for_ship, expect_rpc_err_any_tag,
    expect_rpc_err_tag, get_card, require_m3_execute_ship, retry_check, setup_approved_plan_card,
    start_stage_settled,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t1_1_approve_delivery_succeeds_when_all_gates_and_observations_pass() -> TestResult<()>
{
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f18_t1_1_approve_delivery_succeeds_when_all_gates_and_observations_pass",
    )
    .await?;
    let wt = h.temp_root().join("wt-f18-ship");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-ship")?;
    let tok = h.pair_client("f18-t1-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-1", "F18 Ship", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-1", &wt, "F18 Ship Card", None).await?;
    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;

    let obs = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({
                "cardId": card_id,
                "expectedRevision": rev_ready,
                "criterionId": "AC-2",
                "verdict": "pass",
                "note": "Verified manually",
            }),
        )
        .await?;
    let rev2 = obs["revision"].as_i64().unwrap_or(rev + 1);
    let shipped = ws
        .call_ok(
            &next_id("ship"),
            "workflowBoard.approveDelivery",
            json!({ "cardId": card_id, "expectedRevision": rev2 }),
        )
        .await?;
    assert_eq!(shipped["card"]["stage"], "ship");
    assert_eq!(shipped["status"], "completed");
    assert_eq!(shipped["deliveryStatus"], "shipped");
    assert!(shipped["deliveryReceipt"]["productSnapshotDigest"].is_string());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t1_2_record_manual_acceptance_persists_across_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f18_t1_2_record_manual_acceptance_persists_across_restart",
    )
    .await?;
    let wt = h.temp_root().join("wt-f18-obs");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-obs")?;
    let tok = h.pair_client("f18-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-2", "F18 Obs", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-2", &wt, "F18 Obs Card", None).await?;

    let _ = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "criterionId": "AC-2",
                "verdict": "not-applicable",
                "note": "API-only card",
            }),
        )
        .await?;
    drop(ws);
    h.restart_server()?;
    let tok2 = h.pair_client("f18-t1-2b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    let ac2 = card["acObservations"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["criterionId"] == "AC-2"))
        .ok_or("missing AC-2 acObservation")?;
    assert_eq!(ac2["status"], "not_applicable");
    assert_eq!(ac2["observationNote"], "API-only card");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t1_3_failed_check_blocks_approve_delivery_with_fix_target() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f18_t1_3_failed_check_blocks_approve_delivery_with_fix_target",
    )
    .await?;
    let wt = h.temp_root().join("wt-f18-blk");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-blk")?;
    let tok = h.pair_client("f18-t1-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-3", "F18 Block", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f18-3",
        &wt,
        "F18 Block Card",
        Some(vec!["git", "rev-parse", "--verify", "refs/heads/missing"]),
    )
    .await?;
    let t1 = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let t2 = start_stage_settled(
        &mut ws,
        &card_id,
        t1["revision"].as_i64().unwrap_or(rev + 1),
        "execute",
        Some("TSK-2"),
    )
    .await?;
    let _ = retry_check(
        &mut ws,
        &card_id,
        t2["revision"].as_i64().unwrap_or(rev + 2),
        "CHK-1",
    )
    .await?;
    let detail = get_card(&mut ws, &card_id).await?;
    assert_eq!(detail["checkReceipts"][0]["status"], "failed");
    let chk_blocker = detail["shipReadiness"]["blockers"]
        .as_array()
        .and_then(|b| b.iter().find(|x| x["kind"] == "check_failed"))
        .ok_or("missing check_failed blocker")?;
    assert_eq!(chk_blocker["pointer"]["stage"], "execute");
    assert_eq!(chk_blocker["pointer"]["targetId"], "CHK-1");
    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": detail["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    let err_lower = err.to_string().to_lowercase();
    assert!(err_lower.contains("check") || err_lower.contains("chk-1"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t1_4_stale_evidence_refuses_approve_delivery() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(&h, "f18_t1_4_stale_evidence_refuses_approve_delivery").await?;
    let wt = h.temp_root().join("wt-f18-stale");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-stale")?;
    let tok = h.pair_client("f18-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-4", "F18 Stale", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-4", &wt, "F18 Stale Card", None).await?;
    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let _ = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": card_id, "expectedRevision": rev_ready, "criterionId": "AC-2", "verdict": "pass", "note": "Verified" }),
        )
        .await?;
    fs::write(wt.join("README.md"), "# Mutated before ship\n")?;
    let stale_card = get_card(&mut ws, &card_id).await?;
    assert!(
        stale_card["checkReceipts"][0]["status"] == "stale"
            || stale_card["reviewReport"]["status"] == "stale"
    );
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
async fn f18_t1_5_shipped_card_remains_readable_with_history() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(&h, "f18_t1_5_shipped_card_remains_readable_with_history").await?;
    let wt = h.temp_root().join("wt-f18-hist");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-hist")?;
    let tok = h.pair_client("f18-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-5", "F18 Hist", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-5", &wt, "F18 Hist Card", None).await?;
    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let obs = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({
                "cardId": card_id,
                "expectedRevision": rev_ready,
                "criterionId": "AC-2",
                "verdict": "pass",
                "note": "Verified",
            }),
        )
        .await?;
    let _ = ws
        .call_ok(
            &next_id("ship"),
            "workflowBoard.approveDelivery",
            json!({ "cardId": card_id, "expectedRevision": obs["revision"] }),
        )
        .await?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["stage"], "ship");
    assert_eq!(card["status"], "completed");
    assert!(card["deliveryReceipt"].is_object());
    assert_eq!(card["specApprovalStatus"], "valid");
    assert_eq!(card["planApprovalStatus"], "valid");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t2_1_agent_token_rejected_on_ship_and_manual_acceptance() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let agent_tok = h.mint_agent_thread_token("thr-agent-18").await?;
    let mut agent_ws = h.connect_ws(&agent_tok).await?;

    for method in [
        "workflowBoard.recordManualAcceptance",
        "workflowBoard.approveDelivery",
    ] {
        let err = expect_rpc_err_tag(
            &mut agent_ws,
            method,
            json!({ "cardId": "card-18", "expectedRevision": 1 }),
            "EnvironmentAuthorizationError",
        )
        .await?;
        assert_eq!(err["requiredPermission"], "client:interactive");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t2_2_readonly_token_rejected_on_approve_delivery() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let ro_tok = h
        .pair_client("f18-t2-2-ro", Some(&["orchestration:read"]))
        .await?;
    let mut ro_ws = h.connect_ws(&ro_tok).await?;

    let err = expect_rpc_err_tag(
        &mut ro_ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": "card-18", "expectedRevision": 1 }),
        "EnvironmentAuthorizationError",
    )
    .await?;
    assert_eq!(err["requiredPermission"], "orchestration:operate");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t2_3_missing_manual_observation_blocks_approve_delivery() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f18_t2_3_missing_manual_observation_blocks_approve_delivery",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f18-miss");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-miss")?;
    let tok = h.pair_client("f18-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-6", "F18 Miss", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-6", &wt, "F18 Miss Card", None).await?;
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": rev }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t2_4_failing_criterion_observation_blocks_approve_delivery() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f18_t2_4_failing_criterion_observation_blocks_approve_delivery",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f18-failac");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-failac")?;
    let tok = h.pair_client("f18-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-7", "F18 FailAC", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-7", &wt, "F18 FailAC Card", None).await?;
    let obs = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "criterionId": "AC-2",
                "verdict": "fail",
                "note": "Broken flow",
            }),
        )
        .await?;
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": obs["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f18_t2_5_stale_revision_or_duplicate_post_ship_rejected() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f18_t2_5_stale_revision_or_duplicate_post_ship_rejected",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f18-dup");
    create_worktree(h.workspace_dir(), &wt, "feat/f18-dup")?;
    let tok = h.pair_client("f18-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f18-8", "F18 Dup", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f18-8", &wt, "F18 Dup Card", None).await?;
    let err = expect_rpc_err_any_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": rev - 1 }),
        &["WorkflowBoardRequestError"],
    )
    .await?;
    assert_eq!(err["code"], "revision_conflict");

    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let obs = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": card_id, "expectedRevision": rev_ready, "criterionId": "AC-2", "verdict": "pass", "note": "OK" }),
        )
        .await?;
    let shipped = ws
        .call_ok(
            &next_id("ship"),
            "workflowBoard.approveDelivery",
            json!({ "cardId": card_id, "expectedRevision": obs["revision"] }),
        )
        .await?;
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": card_id, "expectedRevision": shipped["revision"] }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}
