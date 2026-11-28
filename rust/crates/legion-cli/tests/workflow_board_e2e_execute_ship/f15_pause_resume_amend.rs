//! F15: Pause, resume, retry & plan amendment (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, expect_rpc_err_any_tag, expect_rpc_err_tag, get_card,
    require_m3_execute_ship, retry_check, setup_approved_plan_card, start_stage_settled,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t1_1_pause_card_persists_across_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(&h, "f15_t1_1_pause_card_persists_across_restart").await?;
    let wt = h.temp_root().join("wt-f15-pause");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-pause")?;
    let tok = h.pair_client("f15-t1-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-1", "F15 Pause", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-1", &wt, "Pause Card", None).await?;

    let paused = ws
        .call_ok(
            &next_id("pause"),
            "workflowBoard.pauseCard",
            json!({ "cardId": card_id, "expectedRevision": rev }),
        )
        .await?;
    assert_eq!(paused["status"], "paused");
    drop(ws);

    h.restart_server()?;
    let tok2 = h.pair_client("f15-t1-1b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    assert_eq!(card["status"], "paused");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t1_2_resume_card_continues_without_duplicating_completed_tasks() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f15_t1_2_resume_card_continues_without_duplicating_completed_tasks",
    )
    .await?;
    let wt = h.temp_root().join("wt-f15-res");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-res")?;
    let tok = h.pair_client("f15-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-2", "F15 Resume", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-2", &wt, "Resume Card", None).await?;

    let started = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let rev_after_exec = started["revision"].as_i64().unwrap_or(rev + 1);
    let paused = ws
        .call_ok(
            &next_id("pause"),
            "workflowBoard.pauseCard",
            json!({ "cardId": card_id, "expectedRevision": rev_after_exec }),
        )
        .await?;
    let rev2 = paused["revision"].as_i64().unwrap_or(rev_after_exec + 1);
    let resumed = ws
        .call_ok(
            &next_id("res"),
            "workflowBoard.resumeCard",
            json!({ "cardId": card_id, "expectedRevision": rev2 }),
        )
        .await?;
    assert_ne!(resumed["status"], "paused");
    let rev3 = resumed["revision"].as_i64().unwrap_or(rev2 + 1);
    let _ = start_stage_settled(&mut ws, &card_id, rev3, "execute", Some("TSK-2")).await?;
    let detail = get_card(&mut ws, &card_id).await?;
    let tasks = detail["taskExecutions"]
        .as_array()
        .ok_or("missing taskExecutions")?;
    assert_eq!(
        tasks
            .iter()
            .filter(|x| x["taskId"] == "TSK-1" && x["status"] == "completed")
            .count(),
        1
    );
    assert!(
        tasks
            .iter()
            .any(|x| x["taskId"] == "TSK-2" && x["status"] == "completed")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t1_3_retry_step_recovers_after_workspace_fix() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(&h, "f15_t1_3_retry_step_recovers_after_workspace_fix").await?;
    let wt = h.temp_root().join("wt-f15-retry");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-retry")?;
    let tok = h.pair_client("f15-t1-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-3", "F15 Retry", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f15-3",
        &wt,
        "Retry Card",
        Some(vec![
            "node",
            "-e",
            "process.exit(require('node:fs').existsSync('ready.txt') ? 0 : 1)",
        ]),
    )
    .await?;

    let exec = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let exec_rev = exec["revision"].as_i64().ok_or("missing exec rev")?;
    let _ = retry_check(&mut ws, &card_id, exec_rev, "CHK-1").await?;
    let pre = get_card(&mut ws, &card_id).await?;
    assert_eq!(pre["checkReceipts"][0]["status"], "failed");
    assert!(
        pre["shipReadiness"]["blockers"]
            .as_array()
            .is_some_and(|b| b.iter().any(|x| x["kind"] == "check_failed"))
    );

    fs::write(wt.join("ready.txt"), "ok\n")?;
    let pre_rev = pre["revision"].as_i64().ok_or("missing pre-retry rev")?;
    let retried = retry_check(&mut ws, &card_id, pre_rev, "CHK-1").await?;
    assert_eq!(retried["stepKind"], "check");
    assert_eq!(retried["targetId"], "CHK-1");

    let post = get_card(&mut ws, &card_id).await?;
    assert_eq!(post["checkReceipts"][0]["status"], "passed");
    assert!(
        !post["shipReadiness"]["blockers"]
            .as_array()
            .is_some_and(|b| b.iter().any(|x| x["kind"] == "check_failed"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t1_4_propose_amendment_and_approve_stales_affected_evidence() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f15_t1_4_propose_amendment_and_approve_stales_affected_evidence",
    )
    .await?;
    let wt = h.temp_root().join("wt-f15-amend");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-amend")?;
    let tok = h.pair_client("f15-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-4", "F15 Amend", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-4", &wt, "Amend Card", None).await?;

    let exec = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let chk = retry_check(
        &mut ws,
        &card_id,
        exec["revision"].as_i64().unwrap_or(rev + 1),
        "CHK-1",
    )
    .await?;
    let rev_e = chk["revision"].as_i64().unwrap_or(rev + 2);
    let amended = ws.call_ok(&next_id("amend"), "workflowBoard.proposeAmendment", json!({
        "cardId": card_id,
        "expectedRevision": rev_e,
        "targetStage": "plan",
        "reason": "Widen filesAllowed",
        "tasks": [
            { "taskId": "TSK-1", "title": "Implement core task", "dependsOn": [], "filesAllowed": ["src/ok.ts", "src/extra.ts"], "filesForbidden": ["src/secret.ts"], "testingMethod": "regression-first" },
            { "taskId": "TSK-2", "title": "Dependent follow-up task", "dependsOn": ["TSK-1"], "filesAllowed": ["src/followup.ts"], "filesForbidden": ["src/secret.ts"], "testingMethod": "regression-first" }
        ]
    })).await?;
    let approved = ws.call_ok(&next_id("app"), "workflowBoard.approvePlan", json!({ "cardId": card_id, "expectedRevision": amended["revision"].as_i64().unwrap_or(rev_e + 1) })).await?;
    assert_eq!(approved["card"]["specApprovalStatus"], "valid");
    let detail = get_card(&mut ws, &card_id).await?;
    assert!(
        detail["plan"]["tasks"][0]["filesAllowed"]
            .to_string()
            .contains("src/extra.ts")
    );
    assert_eq!(detail["checkReceipts"][0]["status"], "stale");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t1_5_return_for_revision_preserves_history_and_stales_approvals() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f15_t1_5_return_for_revision_preserves_history_and_stales_approvals",
    )
    .await?;
    let wt = h.temp_root().join("wt-f15-ret");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-ret")?;
    let tok = h.pair_client("f15-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-5", "F15 Return", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-5", &wt, "Return Card", None).await?;
    let exec = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let chk = retry_check(
        &mut ws,
        &card_id,
        exec["revision"].as_i64().unwrap_or(rev + 1),
        "CHK-1",
    )
    .await?;
    let rev_e = chk["revision"].as_i64().unwrap_or(rev + 2);
    let returned = ws.call_ok(&next_id("ret"), "workflowBoard.returnForRevision", json!({ "cardId": card_id, "expectedRevision": rev_e, "targetStage": "plan", "reason": "Adjust plan" })).await?;
    assert_eq!(returned["stage"], "plan");
    assert_eq!(returned["planApproval"]["status"], "stale");
    assert_eq!(returned["specApproval"]["status"], "valid");
    assert!(
        returned["history"]
            .as_array()
            .is_some_and(|h| !h.is_empty())
    );
    let after_ret = get_card(&mut ws, &card_id).await?;
    assert_eq!(after_ret["checkReceipts"][0]["status"], "stale");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t2_1_agent_token_cannot_call_return_for_revision() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let agent_tok = h.mint_agent_thread_token("thr-agent-15").await?;
    let mut agent_ws = h.connect_ws(&agent_tok).await?;

    let err = expect_rpc_err_tag(
        &mut agent_ws,
        "workflowBoard.returnForRevision",
        json!({ "cardId": "card-15", "expectedRevision": 1, "targetStage": "spec" }),
        "EnvironmentAuthorizationError",
    )
    .await?;
    assert_eq!(err["requiredPermission"], "client:interactive");

    let client_tok = h.pair_client("f15-t2-1-client", None).await?;
    let mut client_ws = h.connect_ws(&client_tok).await?;
    let env_desc = h.probe_environment().await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() != Some(true) {
        let _ = expect_rpc_err_tag(
            &mut client_ws,
            "workflowBoard.returnForRevision",
            json!({ "cardId": "card-15", "expectedRevision": 1, "targetStage": "spec" }),
            "UnsupportedRpcMethod",
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t2_2_stale_expected_revision_rejected_without_mutation() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f15_t2_2_stale_expected_revision_rejected_without_mutation",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f15-stale");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-stale")?;
    let tok = h.pair_client("f15-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-6", "F15 Stale", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-6", &wt, "Stale Rev", None).await?;

    for (method, payload) in [
        (
            "workflowBoard.pauseCard",
            json!({ "cardId": card_id, "expectedRevision": rev - 1 }),
        ),
        (
            "workflowBoard.resumeCard",
            json!({ "cardId": card_id, "expectedRevision": rev - 1 }),
        ),
        (
            "workflowBoard.retryStep",
            json!({ "cardId": card_id, "expectedRevision": rev - 1, "stepKind": "check", "targetId": "CHK-1" }),
        ),
        (
            "workflowBoard.proposeAmendment",
            json!({ "cardId": card_id, "expectedRevision": rev - 1, "reason": "stale", "patch": {} }),
        ),
    ] {
        let err = expect_rpc_err_any_tag(&mut ws, method, payload, &["WorkflowBoardRequestError"])
            .await?;
        assert_eq!(err["code"], "revision_conflict");
    }
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["revision"], rev);
    assert_ne!(card["status"], "paused");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t2_3_resume_after_interrupted_external_action_reconciles_first() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f15_t2_3_resume_after_interrupted_external_action_reconciles_first",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f15-rec");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-rec")?;
    let tok = h.pair_client("f15-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-7", "F15 Rec", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-7", &wt, "Reconcile Card", None).await?;
    let paused = ws
        .call_ok(
            &next_id("p"),
            "workflowBoard.pauseCard",
            json!({ "cardId": card_id, "expectedRevision": rev }),
        )
        .await?;
    let rev2 = paused["revision"].as_i64().unwrap_or(rev + 1);
    drop(ws);

    h.stop_server();
    h.open_db_rw_while_stopped()?.execute(
        "INSERT INTO workflow_board_external_actions
          (id, card_id, action_kind, idempotency_key, status, created_at, updated_at)
         VALUES ('f15-interrupted', ?1, 'planned_check', 'f15-interrupted', 'interrupted',
                 '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z')",
        [&card_id],
    )?;
    h.restart_server()?;
    let tok2 = h.pair_client("f15-t2-3b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let res = ws2
        .call_ok(
            &next_id("res"),
            "workflowBoard.resumeCard",
            json!({ "cardId": card_id, "expectedRevision": rev2, "reconcileInterrupted": true }),
        )
        .await?;
    assert_ne!(res["status"], "paused");
    assert_eq!(res["reconciledCount"], 1);
    let status: String = h.open_db_readonly()?.query_row(
        "SELECT status FROM workflow_board_external_actions WHERE id = 'f15-interrupted'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(status, "reconciled");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t2_4_agent_token_cannot_self_approve_plan_amendment() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let agent_tok = h.mint_agent_thread_token("thr-agent-15b").await?;
    let mut agent_ws = h.connect_ws(&agent_tok).await?;
    let err = expect_rpc_err_tag(
        &mut agent_ws,
        "workflowBoard.approvePlan",
        json!({ "cardId": "card-15", "expectedRevision": 1 }),
        "EnvironmentAuthorizationError",
    )
    .await?;
    assert_eq!(err["requiredPermission"], "client:interactive");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f15_t2_5_retry_non_failed_step_rejected_cleanly() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(&h, "f15_t2_5_retry_non_failed_step_rejected_cleanly").await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f15-nonfail");
    create_worktree(h.workspace_dir(), &wt, "feat/f15-nonfail")?;
    let tok = h.pair_client("f15-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f15-8", "F15 NonFail", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f15-8", &wt, "NonFail Card", None).await?;

    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.retryStep",
        json!({ "cardId": card_id, "expectedRevision": rev, "stepKind": "task", "targetId": "TSK-1" }),
        "WorkflowBoardRequestError",
    )
    .await?;
    assert_eq!(err["code"], "invalid_stage_transition");
    assert!(
        err["message"]
            .as_str()
            .is_some_and(|msg| msg.contains("must be `failed` or `blocked`"))
    );
    Ok(())
}
