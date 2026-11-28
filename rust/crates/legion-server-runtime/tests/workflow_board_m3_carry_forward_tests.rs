#![forbid(unsafe_code)]
//! M3.1 carry-forward regression tests:
//! 1. `approve_plan` syncs `card.json` `status` / `nextAction` when an orthogonal blocker exists.
//! 2. `explore_step` in non-`spec` stage with already-`stale` spec approval regresses `card.json`
//!    to `spec`, while non-regressing `explore_step` rejects without seeding decisions.

#[path = "workflow_board_m2_support/plan_helpers.rs"]
mod plan_helpers;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardApprovePlanInput, WorkflowBoardCardStatus, WorkflowBoardExploreAction,
    WorkflowBoardExploreAnswerInput, WorkflowBoardExploreStepAction, WorkflowBoardExploreStepInput,
    WorkflowBoardGetCardInput, WorkflowBoardRequestErrorCode, WorkflowBoardReturnForRevisionInput,
    WorkflowBoardReturnTargetStage, WorkflowBoardStage,
};
use legion_persistence::workflow_board_repo::types_m3::RecordExternalActionInput;
use legion_persistence::workflow_board_repo::{get_card_planning_detail, record_external_action};
use plan_helpers::{make_plan_input, make_task, setup_spec_approved_card};
use serde_json::Value;
use workflow_board_m2_support::{TestResult, setup_m2_board};

#[tokio::test]
async fn carry_forward_approve_plan_syncs_card_json_and_explore_regression_guards() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, spec_rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;

    // Non-regressing `explore_step` in `plan` stage must reject WITHOUT seeding decisions.
    let err_non_reg = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: spec_rev,
            action: Some(WorkflowBoardExploreStepAction::Pause),
            continue_round: None,
            answers: None,
        })
        .await
        .expect_err("pause exploreStep in plan stage must reject");
    assert_eq!(
        err_non_reg.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let planning = get_card_planning_detail(&conn, cid.as_str())?;
        assert!(
            planning.decisions.is_empty(),
            "decisions must NOT be seeded on rejected non-regressing explore_step"
        );
    }

    let upd_plan = board
        .update_plan(&make_plan_input(
            cid.clone(),
            spec_rev,
            vec![make_task("TSK-1", "README.md")],
        ))
        .await?;

    // Record an interrupted external action so `approve_plan` yields blocked status + `reconcile_external_action`.
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        record_external_action(
            &conn,
            &RecordExternalActionInput {
                card_id: cid.as_str(),
                id: "ext-act-1",
                action_kind: "git_push",
                target_id: None,
                stage_run_id: None,
                idempotency_key: "idem-cf-1",
                status: "interrupted",
                expected_snapshot_digest: None,
                observed_snapshot_digest: None,
                details_json: "{\"summary\":\"Interrupted push\"}",
            },
            "2026-11-24T12:00:00Z",
        )?;
    }

    let app_plan = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: upd_plan.revision,
                expected_spec_digest: None,
                expected_plan_digest: upd_plan.plan_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(
        app_plan.card.summary.status,
        WorkflowBoardCardStatus::Blocked
    );
    assert_eq!(
        app_plan.card.summary.next_action.as_deref(),
        Some("reconcile_external_action")
    );

    let card_json_path = repo_dir
        .join(".legion/workflows")
        .join(cid.as_str())
        .join("card.json");
    let card_json: Value = serde_json::from_str(&fs::read_to_string(&card_json_path)?)?;
    assert_eq!(card_json["status"], "blocked");
    assert_eq!(card_json["nextAction"], "reconcile_external_action");

    verify_already_stale_spec_explore_regression(
        &orch,
        &board,
        &cid,
        app_plan.revision,
        &card_json_path,
    )
    .await
}

async fn verify_already_stale_spec_explore_regression(
    orch: &std::sync::Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    plan_rev: u64,
    card_json_path: &Path,
) -> TestResult {
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "UPDATE workflow_board_approvals SET status = 'stale' WHERE card_id = ?1 AND gate = 'spec'",
            [cid.as_str()],
        )?;
    }
    let regressed = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: plan_rev,
            action: Some(WorkflowBoardExploreStepAction::Step),
            continue_round: None,
            answers: Some(vec![WorkflowBoardExploreAnswerInput {
                decision_id: "DEC-CF-1".to_owned(),
                action: Some(WorkflowBoardExploreAction::Recommend),
                choice: None,
                answer: None,
                note: None,
            }]),
        })
        .await?;
    assert_eq!(regressed.card.summary.stage, WorkflowBoardStage::Spec);
    let card_json_after: Value = serde_json::from_str(&fs::read_to_string(card_json_path)?)?;
    assert_eq!(card_json_after["stage"], "spec");
    assert_eq!(card_json_after["status"], "blocked");

    let fetched = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let _ = WorkflowBoardReturnTargetStage::Spec;
    let _ = WorkflowBoardReturnForRevisionInput {
        card_id: fetched.summary.card_id,
        expected_revision: fetched.summary.revision,
        target_stage: WorkflowBoardReturnTargetStage::Spec,
        reason: "noop".to_owned(),
    };
    Ok(())
}
