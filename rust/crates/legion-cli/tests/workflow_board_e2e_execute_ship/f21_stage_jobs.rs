//! F21: Versioned built-in stage jobs (`legion-workflows`) (5 Tier 1 + 5 Tier 2).

use serde_json::{Value, json};

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, expect_rpc_err_tag, get_card, probe_m3_execute_ship_ws,
    require_board, require_m3_execute_ship, setup_approved_plan_card, setup_spec_approved_card,
    start_stage_settled, start_stage_settled_with_model,
};
use super::support_board_ship::{setup_worktree_plan_card, setup_worktree_plan_card_t1};
use super::support_m3_mocks::{
    HttpGate, install_mock_providers, valid_plan_tasks, valid_plan_update_payload,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

async fn f21_board_card_t1(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<(OpaqueWsClient, String, i64)> {
    let (ws, _, card_id, rev) = setup_worktree_plan_card_t1(h, tag, proj, title).await?;
    Ok((ws, card_id, rev))
}

async fn f21_board_card(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<Option<(OpaqueWsClient, String, i64)>> {
    Ok(setup_worktree_plan_card(h, tag, proj, title)
        .await?
        .map(|(ws, _, card_id, rev)| (ws, card_id, rev)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t1_1_start_stage_creates_versioned_workflow_run_in_inspector() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f21_board_card_t1(&h, "f21-t1-1", "proj-f21-1", "F21 Run Card").await?;
    let started = ws
        .call_ok(
            &next_id("s1"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute" }),
        )
        .await?;
    let run_id = started["runId"].as_str().ok_or("missing runId")?;
    let detail = ws
        .call_ok(&next_id("wf"), "workflows.get", json!({ "runId": run_id }))
        .await?;
    assert_eq!(detail["run"]["runId"], run_id);
    assert!(detail["run"]["version"].as_i64().unwrap_or(0) >= 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t1_2_stage_brief_carries_approved_artifacts_decisions_and_refs() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f21_t1_2_stage_brief_carries_approved_artifacts_decisions_and_refs",
    )
    .await?;
    let wt = h.temp_root().join("wt-f21-brief");
    create_worktree(h.workspace_dir(), &wt, "feat/f21-brief")?;
    let tok = h.pair_client("f21-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f21-2", "F21 Brief", h.workspace_dir()).await?;
    let (card_id, rev_spec) = setup_spec_approved_card(
        &mut ws,
        "proj-f21-2",
        &wt,
        "F21 Brief Card",
        vec!["Must stay under 50ms"],
        vec!["README.md"],
    )
    .await?;
    let plan_payload = valid_plan_update_payload(
        &card_id,
        rev_spec,
        valid_plan_tasks(),
        json!([{ "id": "CHK-1", "criterionId": "AC-1", "argv": ["git", "status", "--porcelain"] }]),
        json!([
            { "criterionId": "AC-1", "method": "automated", "checkIds": ["CHK-1"] },
            { "criterionId": "AC-2", "method": "manual", "procedure": "Verify manually in worktree." }
        ]),
    );
    let plan_up = ws
        .call_ok(&next_id("plan"), "workflowBoard.updatePlan", plan_payload)
        .await?;
    let plan_ok = ws
        .call_ok(
            &next_id("app"),
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": plan_up["revision"] }),
        )
        .await?;
    let started = ws.call_ok(&next_id("s1"), "workflowBoard.startStage", json!({ "cardId": card_id, "expectedRevision": plan_ok["revision"], "stage": "execute" })).await?;
    let tid = started["threadId"].as_str().ok_or("missing tid")?;
    let proj = ws
        .call_ok(
            &next_id("proj"),
            "orchestration.getThreadProjection",
            json!({ "threadId": tid }),
        )
        .await?;
    let body = proj["messages"].to_string();
    assert!(
        body.contains("AC-1")
            && body.contains("Must stay under 50ms")
            && body.contains("README.md")
            && body.contains("Approved Spec Digest: `sha256:")
            && body.contains("Approved Plan Digest: `sha256:")
            && body.contains("Expected Execute Output Contract")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t1_3_provider_switch_between_stage_attempts_preserves_card_identity() -> TestResult<()>
{
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f21_board_card_t1(&h, "f21-t1-3", "proj-f21-3", "F21 Switch Card").await?;
    install_mock_providers(&mut ws, None).await?;
    let s1 = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        rev,
        "execute",
        Some("TSK-1"),
        Some(json!({ "instanceId": "devReplay", "model": "dev-replay" })),
    )
    .await?;
    let executed = get_card(&mut ws, &card_id).await?;
    assert!(executed["taskExecutions"].as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .any(|task| task["taskId"] == "TSK-1" && task["status"] == "completed")
    }));
    let s2 = start_stage_settled_with_model(
        &mut ws,
        &card_id,
        executed["revision"].as_i64().ok_or("missing revision")?,
        "review",
        None,
        Some(json!({ "instanceId": "mock-review-pass", "model": "claude-sonnet-4-5" })),
    )
    .await?;
    let run1 = s1["runId"].as_str().ok_or("missing run1")?;
    let run2 = s2["runId"].as_str().ok_or("missing run2")?;
    assert_ne!(run1, run2);
    let _ = ws
        .call_ok(&next_id("g1"), "workflows.get", json!({ "runId": run1 }))
        .await?;
    let _ = ws
        .call_ok(&next_id("g2"), "workflows.get", json!({ "runId": run2 }))
        .await?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["cardId"], card_id);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t1_4_return_and_readvance_creates_new_run_preserving_history() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f21_board_card_t1(&h, "f21-t1-4", "proj-f21-4", "F21 Hist Card").await?;
    let s1 = start_stage_settled(&mut ws, &card_id, rev, "execute", None).await?;
    let ret = ws.call_ok(&next_id("ret"), "workflowBoard.returnForRevision", json!({ "cardId": card_id, "expectedRevision": s1["revision"], "targetStage": "plan", "reason": "Refine plan" })).await?;
    assert_eq!(ret["stage"], "plan");
    let app = ws
        .call_ok(
            &next_id("app"),
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": ret["revision"] }),
        )
        .await?;
    let s2 = ws
        .call_ok(
            &next_id("s2"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": app["revision"], "stage": "execute" }),
        )
        .await?;
    assert_ne!(s1["runId"], s2["runId"]);
    let card = get_card(&mut ws, &card_id).await?;
    assert!(card["revisions"].as_array().is_some_and(|h| !h.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t1_5_durable_workflow_journal_prevents_duplicate_step_launch_on_restart()
-> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) =
        f21_board_card_t1(&h, "f21-t1-5", "proj-f21-5", "F21 Jrn Card").await?;
    let mut gate = HttpGate::bind().await?;
    install_mock_providers(&mut ws, Some(gate.port())).await?;
    let s1 = ws
        .call_ok(
            &next_id("s1"),
            "workflowBoard.startStage",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "stage": "execute",
                "modelSelection": { "instanceId": "mock-gate", "model": "claude-sonnet-4-5" }
            }),
        )
        .await?;
    gate.wait_entered().await?;
    drop(ws);
    h.restart_server()?;
    let tok2 = h.pair_client("f21-t1-5b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let s2 = ws2
        .call_ok(
            &next_id("s2"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": s1["revision"], "stage": "execute" }),
        )
        .await?;
    assert_eq!(s1["threadId"], s2["threadId"]);
    assert_eq!(s2["resumed"], true);
    gate.release();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t2_1_thread_scoped_mcp_hides_and_blocks_legion_workflow_tool() -> TestResult<()> {
    let h = CrashableHarness::start(false)?;
    let tok = h.pair_client("f21-t2-1", None).await?;
    let thread_headers = [("x-legioncode-thread-id", "thr-scoped-21")];

    let list_resp: Value = h
        .post_mcp(
            Some(&tok),
            &thread_headers,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .await?
        .json()
        .await?;
    let tools = list_resp["result"]["tools"]
        .as_array()
        .ok_or("missing tools array")?;
    assert!(!tools.iter().any(|t| t["name"] == "legion_workflow"));

    let call_resp: Value = h.post_mcp(
        Some(&tok),
        &thread_headers,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "legion_workflow", "arguments": { "action": "list" } } }),
    ).await?.json().await?;
    assert_eq!(call_resp["result"]["isError"], true);
    let msg_text = call_resp["result"]["content"][0]["text"]
        .as_str()
        .ok_or("missing error text")?;
    assert!(msg_text.contains("Workflow tool is hidden inside thread scopes."));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t2_2_workflows_get_missing_run_returns_workflow_request_error() -> TestResult<()> {
    let h = CrashableHarness::start(false)?;
    let tok = h.pair_client("f21-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;

    let err = expect_rpc_err_tag(
        &mut ws,
        "workflows.get",
        json!({ "runId": "nonexistent-run-f21" }),
        "WorkflowRequestError",
    )
    .await?;
    assert!(err["message"].as_str().is_some_and(|m| !m.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t2_3_labs_workflows_off_gates_stage_job_execution_cleanly() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_board(
            &h,
            "f21_t2_3_labs_workflows_off_gates_stage_job_execution_cleanly",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f21-off");
    create_worktree(h.workspace_dir(), &wt, "feat/f21-off")?;
    let tok = h.pair_client("f21-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f21-off", "F21 Off", h.workspace_dir()).await?;
    let m3_ready = probe_m3_execute_ship_ws(&mut ws).await?;
    let ((card_id, rev), stage) = if m3_ready {
        (
            setup_approved_plan_card(&mut ws, "proj-f21-off", &wt, "Off Card", None).await?,
            "execute",
        )
    } else {
        (
            setup_spec_approved_card(&mut ws, "proj-f21-off", &wt, "Off Card", vec![], vec![])
                .await?,
            "plan",
        )
    };
    let _ = ws
        .call_ok(
            &next_id("cfg"),
            "server.updateSettings",
            json!({ "patch": { "workflowsEnabled": false } }),
        )
        .await?;
    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.startStage",
        json!({ "cardId": card_id, "expectedRevision": rev, "stage": stage }),
        "WorkflowBoardRequestError",
    )
    .await?;
    assert_eq!(err["code"], "launch_failed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t2_4_malformed_stage_output_never_silently_passes() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(&h, "f21_t2_4_malformed_stage_output_never_silently_passes")
            .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f21-mal");
    create_worktree(h.workspace_dir(), &wt, "feat/f21-mal")?;
    let tok = h.pair_client("f21-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f21-6", "F21 Mal", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f21-6",
        &wt,
        "F21 Mal Card [MALFORMED_STAGE_OUTPUT]",
        None,
    )
    .await?;
    let _ = start_stage_settled(&mut ws, &card_id, rev, "review", None).await?;
    let card = get_card(&mut ws, &card_id).await?;
    let has_pass = card["reviewReceipts"]
        .as_array()
        .is_some_and(|r| r.iter().any(|x| x["verdict"] == "PASS"));
    assert!(!has_pass);
    assert_ne!(card["status"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f21_t2_5_cancelling_stage_job_keeps_card_resumable() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, card_id, rev) = f21_board_card(&h, "f21-t2-5", "proj-f21-7", "F21 Can Card")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let s1 = ws
        .call_ok(
            &next_id("s1"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute" }),
        )
        .await?;
    let run_1 = s1["runId"].as_str().ok_or("missing run_1")?;
    let _ = ws
        .call_ok(
            &next_id("can"),
            "workflows.cancel",
            json!({ "runId": run_1, "reason": "abort" }),
        )
        .await?;
    let wf = ws
        .call_ok(&next_id("g"), "workflows.get", json!({ "runId": run_1 }))
        .await?;
    assert_eq!(wf["run"]["status"], "cancelled");
    let card = get_card(&mut ws, &card_id).await?;
    let s2 = ws
        .call_ok(
            &next_id("s2"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": card["revision"], "stage": "execute" }),
        )
        .await?;
    assert_ne!(s2["runId"], s1["runId"]);
    Ok(())
}
