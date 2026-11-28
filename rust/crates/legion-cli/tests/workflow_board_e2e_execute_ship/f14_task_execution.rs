//! F14: Dependency-ready task execution & single mutating task (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, await_thread_settled, create_project, expect_rpc_err_tag, get_card,
    require_m3_execute_ship, require_m3_update_plan, setup_approved_plan_card,
    setup_spec_approved_card, start_stage_settled,
};
use super::support_board_ship::{
    await_card_stage_settled, setup_worktree_plan_card as f14_board_card,
    setup_worktree_plan_card_t1 as f14_board_card_t1,
};
use super::support_m3_mocks::{
    HttpGate, install_mock_providers, valid_plan_update_payload, valid_single_ready_tasks,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t1_1_dependency_ready_task_selection_blocks_dependent_task() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, _, card_id, rev) =
        f14_board_card_t1(&h, "f14-t1-1", "proj-f14-1", "F14 DAG Card").await?;
    let before = get_card(&mut ws, &card_id).await?;
    let plan_tasks = before["plan"]["tasks"]
        .as_array()
        .ok_or("missing plan.tasks")?;
    assert!(plan_tasks.iter().any(|t| t["taskId"] == "TSK-1"));
    let exec_before = before["taskExecutions"]
        .as_array()
        .ok_or("missing taskExecutions")?;
    assert_eq!(
        exec_before
            .iter()
            .find(|t| t["taskId"] == "TSK-1")
            .and_then(|t| t["status"].as_str()),
        Some("todo")
    );

    let err = expect_rpc_err_tag(&mut ws, "workflowBoard.startStage", json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute", "taskId": "TSK-2" }), "WorkflowBoardRequestError").await?;
    assert_eq!(err["code"], "contract_conflict");

    let _ = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let after = get_card(&mut ws, &card_id).await?;
    let exec_after = after["taskExecutions"]
        .as_array()
        .ok_or("missing taskExecutions after")?;
    assert_eq!(
        exec_after
            .iter()
            .find(|t| t["taskId"] == "TSK-1")
            .and_then(|t| t["status"].as_str()),
        Some("completed")
    );
    let _ = ws.call_ok(&next_id("s2"), "workflowBoard.startStage", json!({ "cardId": card_id, "expectedRevision": after["revision"], "stage": "execute", "taskId": "TSK-2" })).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t1_2_single_mutating_task_cap_per_card_rejects_second() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f14_t1_2_single_mutating_task_cap_per_card_rejects_second",
    )
    .await?;
    let wt = h.temp_root().join("wt-f14-cap");
    create_worktree(h.workspace_dir(), &wt, "feat/f14-cap")?;
    let tok = h.pair_client("f14-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f14-2", "F14 Cap", h.workspace_dir()).await?;
    let mut gate = HttpGate::bind().await?;
    install_mock_providers(&mut ws, Some(gate.port())).await?;
    let (card_id, rev_spec) =
        setup_spec_approved_card(&mut ws, "proj-f14-2", &wt, "F14 Cap Card", vec![], vec![])
            .await?;
    let plan_payload = valid_plan_update_payload(
        &card_id,
        rev_spec,
        valid_single_ready_tasks(),
        json!([{ "id": "CHK-1", "criterionId": "AC-1", "argv": ["git", "status", "--porcelain"] }]),
        json!([
            { "criterionId": "AC-1", "method": "automated", "checkIds": ["CHK-1"] },
            { "criterionId": "AC-2", "method": "manual", "procedure": "Verify manually in worktree." }
        ]),
    );
    let plan_up = ws
        .call_ok(&next_id("plan"), "workflowBoard.updatePlan", plan_payload)
        .await?;
    let app = ws
        .call_ok(
            &next_id("app"),
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": plan_up["revision"] }),
        )
        .await?;
    let _ = ws
        .call_ok(
            &next_id("s1"),
            "workflowBoard.startStage",
            json!({
                "cardId": card_id,
                "expectedRevision": app["revision"],
                "stage": "execute",
                "taskId": "TSK-1",
                "modelSelection": { "instanceId": "mock-gate", "model": "claude-sonnet-4-5" }
            }),
        )
        .await?;
    gate.wait_entered().await?;
    let mid = get_card(&mut ws, &card_id).await?;
    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.startStage",
        json!({ "cardId": card_id, "expectedRevision": mid["revision"], "stage": "execute", "taskId": "TSK-2" }),
        "WorkflowBoardRequestError",
    )
    .await?;
    assert_eq!(err["code"], "contract_conflict");
    gate.release();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t1_3_in_contract_edits_pass_validation_and_unlock_next_task() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, wt, card_id, rev) =
        f14_board_card_t1(&h, "f14-t1-3", "proj-f14-3", "F14 OK Card").await?;
    fs::create_dir_all(wt.join("src"))?;
    fs::write(wt.join("src").join("ok.ts"), "export const ok = 1;\n")?;
    let _ = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let detail = get_card(&mut ws, &card_id).await?;
    let blockers = detail["shipReadiness"]["blockers"]
        .as_array()
        .ok_or("missing shipReadiness.blockers")?;
    assert!(!blockers.iter().any(|b| b["kind"] == "contract_violation"));
    assert_eq!(detail["taskExecutions"][0]["status"], "completed");
    let s2 = ws
        .call_ok(
            &next_id("s2-ok"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": detail["revision"], "stage": "execute", "taskId": "TSK-2" }),
        )
        .await?;
    assert_eq!(s2["taskId"], "TSK-2");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t1_4_get_card_diff_isolates_card_worktree_from_main_repo() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, wt, card_id, _) =
        f14_board_card_t1(&h, "f14-t1-4", "proj-f14-4", "F14 Diff Card").await?;
    fs::write(h.workspace_dir().join("main-only.txt"), "main\n")?;
    fs::create_dir_all(wt.join("src"))?;
    fs::write(wt.join("src").join("ok.ts"), "export const x = 2;\n")?;
    let diff = ws
        .call_ok(
            &next_id("diff"),
            "workflowBoard.getCardDiff",
            json!({ "cardId": card_id }),
        )
        .await?;
    let text = diff.to_string();
    assert!(text.contains("src/ok.ts"));
    assert!(!text.contains("main-only.txt"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t1_5_task_thread_streams_and_updates_card_task_results() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, _, card_id, rev) =
        f14_board_card_t1(&h, "f14-t1-5", "proj-f14-5", "F14 Stream Card").await?;
    let started = ws.call_ok(&next_id("s1"), "workflowBoard.startStage", json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute", "taskId": "TSK-1" })).await?;
    let settled = await_thread_settled(
        &mut ws,
        started["threadId"].as_str().ok_or("missing threadId")?,
    )
    .await?;
    assert!(settled["runs"].as_array().is_some_and(|r| !r.is_empty()));
    let card = await_card_stage_settled(
        &mut ws,
        &card_id,
        started["stageRunId"].as_str().ok_or("missing stageRunId")?,
    )
    .await?;
    assert_eq!(card["taskExecutions"][0]["taskId"], "TSK-1");
    assert_eq!(card["taskExecutions"][0]["status"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t2_1_unbuilt_board_start_stage_rejected_cleanly_when_disabled() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(&h, "f14_t2_1").await?;
    let (mut ws, wt, card_id, rev) =
        f14_board_card_t1(&h, "f14-t2-1", "proj-f14-forb", "Forbidden Card").await?;
    let mut gate = HttpGate::bind().await?;
    install_mock_providers(&mut ws, Some(gate.port())).await?;
    let started = ws
        .call_ok(
            &next_id("forb"),
            "workflowBoard.startStage",
            json!({
                "cardId": card_id, "expectedRevision": rev, "stage": "execute", "taskId": "TSK-1",
                "modelSelection": { "instanceId": "mock-gate", "model": "claude-sonnet-4-5" },
            }),
        )
        .await?;
    gate.wait_entered().await?;
    fs::create_dir_all(wt.join("src"))?;
    let secret = wt.join("src").join("secret.ts");
    fs::write(&secret, "export const secret = 42;\n")?;
    gate.release();
    let card = await_card_stage_settled(
        &mut ws,
        &card_id,
        started["stageRunId"].as_str().ok_or("missing stageRunId")?,
    )
    .await?;
    assert!(
        card["fileContractReports"]
            .as_array()
            .is_some_and(|reports| reports.iter().any(|report| {
                report["status"] == "violation_pending"
                    && report["forbiddenFiles"] == json!(["src/secret.ts"])
            }))
    );
    assert!(secret.is_file());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t2_2_out_of_contract_edits_preserved_on_disk_for_review() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, wt, card_id, rev) = f14_board_card(&h, "f14-t2-2", "proj-f14-6", "F14 Unexp Card")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let unexpected = wt.join("src").join("unexpected.ts");
    fs::create_dir_all(wt.join("src"))?;
    fs::write(&unexpected, "export const keepMe = true;\n")?;
    let _ = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    assert!(unexpected.is_file());
    assert_eq!(
        fs::read_to_string(&unexpected)?,
        "export const keepMe = true;\n"
    );
    let diff = ws
        .call_ok(
            &next_id("diffu"),
            "workflowBoard.getCardDiff",
            json!({ "cardId": card_id, "taskId": "TSK-1" }),
        )
        .await?;
    assert!(
        diff["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|entry| {
                entry["path"] == "src/unexpected.ts" && entry["contractStatus"] == "unexpected"
            }))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t2_3_concurrent_cards_in_separate_worktrees_do_not_block() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f14_t2_3_concurrent_cards_in_separate_worktrees_do_not_block",
        )
        .await?,
        "required workflow board capability"
    );
    let wt_a = h.temp_root().join("wt-f14-ca");
    let wt_b = h.temp_root().join("wt-f14-cb");
    create_worktree(h.workspace_dir(), &wt_a, "feat/f14-ca")?;
    create_worktree(h.workspace_dir(), &wt_b, "feat/f14-cb")?;
    let tok = h.pair_client("f14-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f14-7", "F14 Multi", h.workspace_dir()).await?;
    let (card_a, rev_a) =
        setup_approved_plan_card(&mut ws, "proj-f14-7", &wt_a, "Card A", None).await?;
    let (card_b, rev_b) =
        setup_approved_plan_card(&mut ws, "proj-f14-7", &wt_b, "Card B", None).await?;
    let s_a = ws.call_ok(&next_id("sa"), "workflowBoard.startStage", json!({ "cardId": card_a, "expectedRevision": rev_a, "stage": "execute", "taskId": "TSK-1" })).await?;
    let s_b = ws.call_ok(&next_id("sb"), "workflowBoard.startStage", json!({ "cardId": card_b, "expectedRevision": rev_b, "stage": "execute", "taskId": "TSK-1" })).await?;
    assert_ne!(s_a["threadId"], s_b["threadId"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t2_4_empty_diff_turn_handled_deterministically() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (mut ws, _, card_id, rev) = f14_board_card(&h, "f14-t2-4", "proj-f14-8", "Empty Card")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let _ = start_stage_settled(&mut ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let detail = get_card(&mut ws, &card_id).await?;
    let diff = ws
        .call_ok(
            &next_id("diffe"),
            "workflowBoard.getCardDiff",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(diff["files"], json!([]));
    assert!(
        !detail["shipReadiness"]["blockers"]
            .as_array()
            .is_some_and(|blockers| {
                blockers
                    .iter()
                    .any(|blocker| blocker["kind"] == "contract_violation")
            })
    );
    assert!(detail["taskExecutions"].as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .any(|task| task["taskId"] == "TSK-1" && task["status"] == "completed")
    }));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f14_t2_5_circular_dependency_dag_rejected_before_execute() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_update_plan(
            &h,
            "f14_t2_5_circular_dependency_dag_rejected_before_execute",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f14-cyc");
    create_worktree(h.workspace_dir(), &wt, "feat/f14-cyc")?;
    let tok = h.pair_client("f14-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f14-9", "F14 Cycle", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_spec_approved_card(&mut ws, "proj-f14-9", &wt, "Cycle Card", vec![], vec![]).await?;
    let err = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.updatePlan",
        json!({
            "cardId": card_id,
            "expectedRevision": rev,
            "tasks": [
                { "taskId": "TSK-1", "title": "A", "dependsOn": ["TSK-2"], "filesAllowed": ["src/a.ts"], "testingMethod": "regression-first" },
                { "taskId": "TSK-2", "title": "B", "dependsOn": ["TSK-1"], "filesAllowed": ["src/b.ts"], "testingMethod": "regression-first" },
            ],
        }),
        "ParseError",
    )
    .await?;
    assert_eq!(err["message"], "task dependency graph contains a cycle");
    assert_eq!(get_card(&mut ws, &card_id).await?["revision"], rev);
    Ok(())
}
