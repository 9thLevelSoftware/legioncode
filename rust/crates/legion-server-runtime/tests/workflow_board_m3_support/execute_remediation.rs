#![forbid(unsafe_code)]
//! M3 remediation regression tests for `workflow_board_turn_observer` settlement catch-up,
//! review severity normalization & plain-prose rejection, and `deliverable_stale` recovery.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use legion_contracts::orchestration::{PatchValue, WorkspaceStrategy};
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput,
    WorkflowBoardEvidenceMethod, WorkflowBoardGetCardInput, WorkflowBoardReviewSeverity,
    WorkflowBoardReviewVerdict, WorkflowBoardStageRunStage, WorkflowBoardStageRunStatus,
    WorkflowBoardStartStageInput, WorkflowBoardTaskStatus, WorkflowBoardUpdatePlanInput,
    WorkflowBoardUpdateSpecInput,
};
use legion_orchestrator::outbox::OutboxWorkerOptions;
use legion_persistence::workflow_board_repo::link_stage_run_thread;
use legion_persistence::workflow_board_repo::types::LinkStageRunThreadInput;
use legion_server_runtime::RuntimeOutboxExecutor;
use serde_json::json;

use super::m3_support::{m3_task, setup_approved_m3_card};
use super::workflow_board_m2_support::{TestResult, setup_m2_board};

fn inject_assistant_review_reply(
    orch: &legion_orchestrator::Orchestrator,
    thread_id: &str,
    reply_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
    let mut stmt = conn.prepare(
        "SELECT message_id, payload_json FROM orchestration_v2_projection_messages WHERE thread_id = ?1",
    )?;
    let rows: Vec<(String, String)> = stmt
        .query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (msg_id, raw) in rows {
        let mut val: serde_json::Value = serde_json::from_str(&raw)?;
        if val.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            val["text"] = json!(reply_text);
            conn.execute(
                "UPDATE orchestration_v2_projection_messages SET payload_json = ?2 WHERE message_id = ?1",
                rusqlite::params![msg_id, val.to_string()],
            )?;
        }
    }
    Ok(())
}

async fn assert_post_claim_execute_catchup(
    orch: &Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    worker: &legion_orchestrator::outbox::EffectOutboxWorker,
    cid: &legion_contracts::ids::WorkflowCardId,
    rev: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let exec_start = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: rev,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: Some("TSK-1".to_owned()),
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "UPDATE workflow_board_stage_runs SET thread_id = NULL WHERE id = ?1",
            [exec_start.stage_run_id.as_str()],
        )?;
    }
    let _ = worker.drain().await?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        link_stage_run_thread(
            &conn,
            &LinkStageRunThreadInput {
                id: exec_start.stage_run_id.as_str(),
                status: "running",
                thread_id: Some(exec_start.thread_id.as_str()),
                workflow_run_id: exec_start
                    .run_id
                    .as_ref()
                    .map(legion_contracts::ids::RunId::as_str),
                worktree_path: None,
                branch: None,
                last_error: None,
                ended_at: None,
            },
            "2026-11-25T05:00:00Z",
        )?;
    }
    board.process_settled_turns().await;
    let after_exec = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(
        after_exec
            .task_executions
            .iter()
            .find(|t| t.task_id == "TSK-1")
            .map(|t| t.status),
        Some(WorkflowBoardTaskStatus::Completed)
    );
    Ok(after_exec.summary.revision)
}

async fn assert_plain_prose_review_fails(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    worker: &legion_orchestrator::outbox::EffectOutboxWorker,
    repo_dir: &Path,
    cid: &legion_contracts::ids::WorkflowCardId,
    rev: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    fs::create_dir_all(repo_dir.join("src"))?;
    fs::write(repo_dir.join("src/a.txt"), "impl\n")?;
    let rev_plain = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: rev,
            stage: WorkflowBoardStageRunStage::Review,
            task_id: None,
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    let _ = worker.drain().await?;
    board.process_settled_turns().await;
    let after_plain = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let plain_sr = after_plain
        .stage_runs
        .iter()
        .find(|sr| sr.id == rev_plain.stage_run_id)
        .ok_or("missing plain review stage run")?;
    assert_eq!(plain_sr.status, WorkflowBoardStageRunStatus::Failed);
    assert!(
        plain_sr
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("legion_workflow_board_propose"))
    );
    assert_eq!(
        after_plain.review_report.as_ref().and_then(|r| r.verdict),
        Some(WorkflowBoardReviewVerdict::Failed)
    );
    Ok(after_plain.summary.revision)
}

async fn assert_structured_json_review_normalizes(
    orch: &Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    worker: &legion_orchestrator::outbox::EffectOutboxWorker,
    cid: &legion_contracts::ids::WorkflowCardId,
    rev: u64,
) -> TestResult {
    let rev_json = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: rev,
            stage: WorkflowBoardStageRunStage::Review,
            task_id: None,
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    let _ = worker.drain().await?;
    inject_assistant_review_reply(
        orch,
        rev_json.thread_id.as_str(),
        r#"{"verdict":"findings","summary":"Normalized","findings":[{"severity":"blocking","message":"Must fix"},{"severity":"warning","message":"Check warning"}]}"#,
    )?;
    board.process_settled_turns().await;
    let after_json = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let json_sr = after_json
        .stage_runs
        .iter()
        .find(|sr| sr.id == rev_json.stage_run_id)
        .ok_or("missing json review stage run")?;
    assert_eq!(json_sr.status, WorkflowBoardStageRunStatus::Completed);
    let report = after_json.review_report.ok_or("missing review report")?;
    assert_eq!(report.findings.len(), 2);
    let f_crit = report
        .findings
        .iter()
        .find(|f| f.message == "Must fix")
        .ok_or("missing critical finding")?;
    let f_maj = report
        .findings
        .iter()
        .find(|f| f.message == "Check warning")
        .ok_or("missing major finding")?;
    assert_eq!(f_crit.severity, WorkflowBoardReviewSeverity::Critical);
    assert_eq!(f_maj.severity, WorkflowBoardReviewSeverity::Major);
    assert!(f_crit.id.starts_with("fnd-"));
    assert!(f_maj.id.starts_with("fnd-"));
    Ok(())
}

#[tokio::test]
async fn turn_observer_catches_up_post_claim_and_hardens_review_settlement() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let worker = orch.manual_outbox_worker(
        RuntimeOutboxExecutor::new(Arc::clone(&orch)),
        OutboxWorkerOptions {
            worker_id: "m3-obs-test".to_owned(),
            max_attempts: 1,
            retry_delay_ms: 0,
        },
    );
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Manual,
        check_ids: None,
        command: None,
        procedure: Some("Manual check".to_owned()),
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Root { branch: None }),
        vec![m3_task("TSK-1", vec!["src/a.txt"], vec![], &[])],
        Vec::new(),
        acs,
    ))
    .await?;

    let rev = Box::pin(assert_post_claim_execute_catchup(
        &orch, &board, &worker, &cid, rev,
    ))
    .await?;
    let rev = Box::pin(assert_plain_prose_review_fails(
        &board, &worker, repo_dir, &cid, rev,
    ))
    .await?;
    Box::pin(assert_structured_json_review_normalizes(
        &orch, &board, &worker, &cid, rev,
    ))
    .await
}

#[tokio::test]
async fn deliverable_stale_clears_to_clean_on_reauthor_and_reapprove() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Manual,
        check_ids: None,
        command: None,
        procedure: Some("Manual check".to_owned()),
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Root { branch: None }),
        vec![m3_task("TSK-1", vec!["src/a.txt"], vec![], &[])],
        Vec::new(),
        acs,
    ))
    .await?;

    // Externally edit plan.md -> get_card_detail detects deliverable_stale.
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    fs::write(card_dir.join("plan.md"), "# Externally edited plan\n")?;
    let stale = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(stale.summary.has_unreconciled_edits, Some(true));
    assert_eq!(stale.summary.revision, rev);

    // Re-saving via update_plan rewrites plan.md cleanly and clears deliverable_stale!
    let reauthored = board
        .update_plan(&WorkflowBoardUpdatePlanInput {
            card_id: cid.clone(),
            expected_revision: stale.summary.revision,
            lenses: None,
            alternatives: None,
            selected_alternative_id: PatchValue::Absent,
            selection_rationale: None,
            revise_alternatives_feedback: None,
            strategy: None,
            strategy_rationale: Some("Re-authored after external edit".to_owned()),
            strategy_metadata: None,
            granularity: None,
            tasks: None,
            checks: None,
            ac_mappings: None,
        })
        .await?;
    assert_eq!(reauthored.card.summary.has_unreconciled_edits, Some(false));
    let reapproved = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: reauthored.revision,
                expected_spec_digest: None,
                expected_plan_digest: reauthored.plan_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(reapproved.card.summary.has_unreconciled_edits, Some(false));

    // Externally edit spec.md -> update_spec + approve_spec clears deliverable_stale too.
    fs::write(card_dir.join("spec.md"), "# Externally edited spec\n")?;
    let spec_stale = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(spec_stale.summary.has_unreconciled_edits, Some(true));
    let spec_saved = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: spec_stale.summary.revision,
            title: None,
            brief: Some("Cleanly re-saved spec".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await?;
    assert_eq!(spec_saved.card.summary.has_unreconciled_edits, Some(false));
    let spec_app = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid,
                expected_revision: spec_saved.revision,
                expected_digest: spec_saved.spec_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(spec_app.card.summary.has_unreconciled_edits, Some(false));
    Ok(())
}
