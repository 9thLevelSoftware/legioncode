#![forbid(unsafe_code)]
//! Integration tests for M4 `.legion-cli/` preview, active lock refusal, idempotent import, policy gates, and historical Ship blocks.

#[path = "workflow_board_m2_support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use legion_contracts::ids::ProjectId;
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApproveDeliveryInput,
    WorkflowBoardCliImportHistoricalStatus, WorkflowBoardCliImportLockStatus,
    WorkflowBoardCliImportOwner, WorkflowBoardCliImportPolicyStatus,
    WorkflowBoardCliImportSourceSystem, WorkflowBoardGetCardInput,
    WorkflowBoardPreviewCliImportInput, WorkflowBoardRequestErrorCode,
    WorkflowBoardShipBlockerKind, WorkflowBoardStage, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardTaskStatus, WorkflowBoardUpdateSpecInput,
};
use rusqlite::params;
use support::{TestResult, replay_model, setup_m2_board};

fn git_commit_count(dir: &Path) -> String {
    let out = StdCommand::new("git")
        .current_dir(dir)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn repo_legion_cli_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(".legion-cli")
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn copy_fixture_cli(repo_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dst = repo_path.join(".legion-cli");
    copy_dir_recursive(&repo_legion_cli_dir(), &dst)?;
    Ok(dst)
}

#[test]
fn preview_cli_import_reads_real_fixture_and_enforces_lock_and_error_rules() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let project_id = ProjectId::new("proj-board-1")?;

    // Missing `.legion-cli` directory fails with ArtifactNotFound.
    let missing_err = board
        .preview_cli_import(&WorkflowBoardPreviewCliImportInput {
            project_id: project_id.clone(),
            cli_dir: None,
        })
        .expect_err("missing .legion-cli must fail");
    assert_eq!(
        missing_err.code,
        WorkflowBoardRequestErrorCode::ArtifactNotFound
    );

    // Copy real `.legion-cli` fixture into disposable temp workspace.
    let cli_dir = copy_fixture_cli(repo_path)?;
    // Add an assurance policy sidecar and a dead-PID engine.lock (`999999`) to verify stale lock + sidecar detection.
    fs::write(
        cli_dir.join("workflow/assurance.yaml"),
        "mode: strict\nmaxRetries: 2\n",
    )?;
    fs::create_dir_all(cli_dir.join("index"))?;
    fs::write(
        cli_dir.join("index/engine.lock"),
        "{\"pid\": 999999, \"runId\": \"dead-run\", \"startedAt\": \"2026-01-01T00:00:00Z\"}\n",
    )?;

    let preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
    })?;
    assert_preview_fields(&preview);

    let card_count: i64 = {
        let conn = orch.sink().db().lock().expect("db lock");
        conn.query_row("SELECT COUNT(*) FROM workflow_board_cards", [], |r| {
            r.get(0)
        })?
    };
    assert_eq!(card_count, 0);
    assert!(!repo_path.join(".legion/workflows").exists());

    assert_active_lock_refusals(&orch, &board, &project_id, &cli_dir)?;

    // Malformed `execution.yaml` fails with InvalidInput.
    fs::write(
        cli_dir.join("workflow/execution.yaml"),
        "specId: [unterminated\n",
    )?;
    let malformed_err = board
        .preview_cli_import(&WorkflowBoardPreviewCliImportInput {
            project_id,
            cli_dir: None,
        })
        .expect_err("malformed execution.yaml must fail");
    assert_eq!(
        malformed_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );
    Ok(())
}

fn assert_preview_fields(
    preview: &legion_contracts::workflow_board::WorkflowBoardPreviewCliImportResult,
) {
    assert_eq!(preview.spec_id, "spec-t3-code");
    assert!(!preview.already_imported);
    assert!(preview.can_apply);
    assert_eq!(preview.active_locks.len(), 0);
    assert_eq!(preview.stale_locks.len(), 1);
    assert_eq!(
        preview.stale_locks[0].status,
        WorkflowBoardCliImportLockStatus::Stale
    );
    assert_eq!(preview.tasks.len(), 8);
    let tsk2 = preview
        .tasks
        .iter()
        .find(|t| t.task_id == "TSK-0002")
        .expect("TSK-0002");
    assert_eq!(tsk2.id, "TSK-0002");
    assert_eq!(tsk2.status, WorkflowBoardTaskStatus::Blocked);
    assert!(preview.decision_ids.iter().any(|d| d == "D-001"));
    assert!(preview.decision_ids.iter().any(|d| d == "D-008"));
    assert!(preview.assumption_ids.iter().any(|a| a == "ASM-0001"));
    assert!(
        preview
            .acceptance_criterion_ids
            .iter()
            .any(|ac| ac == "AC-P0-01")
    );
    assert!(
        preview
            .acceptance_criterion_ids
            .iter()
            .any(|ac| ac == "AC-CH-01")
    );
    assert!(
        preview
            .missing_log_notes
            .iter()
            .any(|n| n.relative_path.contains("verify-1.log")
                && n.command.as_deref()
                    == Some("node --test packages/provider-mimo/qualification/cli.test.mjs"))
    );
    assert!(
        preview
            .unsupported_policies
            .iter()
            .any(|p| p.key == "sandbox.requireHardened" && p.blocks_execution)
    );
    assert!(
        preview
            .unsupported_policies
            .iter()
            .any(|p| p.key == "qa.passScore" && p.blocks_execution)
    );
    assert!(
        preview
            .unsupported_policies
            .iter()
            .any(|p| p.key == "workflow.assurance" && p.blocks_execution)
    );
}

fn assert_active_lock_refusals(
    orch: &legion_orchestrator::Orchestrator,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    project_id: &ProjectId,
    cli_dir: &Path,
) -> TestResult {
    let current_pid = std::process::id();
    fs::write(
        cli_dir.join("index/engine.lock"),
        format!(
            "{{\"pid\": {current_pid}, \"runId\": \"live-run\", \"startedAt\": \"2026-11-26T00:00:00Z\"}}\n"
        ),
    )?;
    let blocked_preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
    })?;
    assert!(!blocked_preview.can_apply);
    assert_eq!(blocked_preview.active_locks.len(), 1);
    assert_eq!(
        blocked_preview.active_locks[0].status,
        WorkflowBoardCliImportLockStatus::Active
    );
    assert!(blocked_preview.refusal_reason.is_some());
    let apply_refused = board
        .apply_cli_import(&WorkflowBoardApplyCliImportInput {
            project_id: project_id.clone(),
            cli_dir: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
            model_selection: Some(replay_model()),
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
        })
        .expect_err("live lock must fail closed");
    assert_eq!(
        apply_refused.code,
        WorkflowBoardRequestErrorCode::ContractConflict
    );
    let card_count_after: i64 = {
        let conn = orch.sink().db().lock().expect("db lock");
        conn.query_row("SELECT COUNT(*) FROM workflow_board_cards", [], |r| {
            r.get(0)
        })?
    };
    assert_eq!(card_count_after, 0);

    fs::remove_file(cli_dir.join("index/engine.lock"))?;
    fs::create_dir_all(cli_dir.join("cache/live-spawn"))?;
    fs::write(
        cli_dir.join("cache/live-spawn/agent.json"),
        format!("{{\"pid\": {current_pid}, \"runId\": \"spawn-1\"}}\n"),
    )?;
    let spawn_preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
    })?;
    assert!(!spawn_preview.can_apply);
    assert!(
        spawn_preview
            .active_locks
            .iter()
            .any(|l| l.relative_path.contains("live-spawn"))
    );
    fs::remove_dir_all(cli_dir.join("cache/live-spawn"))?;

    fs::create_dir_all(cli_dir.join("cache/runs/run-1"))?;
    fs::write(
        cli_dir.join("cache/runs/run-1/resume.json"),
        "{\"status\": \"running\"}\n",
    )?;
    let resume_preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
    })?;
    assert!(!resume_preview.can_apply);
    assert!(
        resume_preview
            .active_locks
            .iter()
            .any(|l| l.relative_path.contains("resume.json"))
    );
    fs::remove_dir_all(cli_dir.join("cache"))?;
    Ok(())
}

#[tokio::test]
async fn apply_cli_import_is_idempotent_writes_deliverables_and_enforces_execute_and_ship_gates()
-> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let cli_dir = copy_fixture_cli(repo_path)?;
    let commits_before = git_commit_count(repo_path);
    let project_id = ProjectId::new("proj-board-1")?;

    // Case 1: Initial apply without policy acknowledgement.
    let applied = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    assert!(applied.created);
    assert_eq!(applied.revision, 1);
    assert_eq!(applied.source_spec_id, "spec-t3-code");
    assert_eq!(applied.tasks.len(), 8);
    assert!(applied.historical_receipts.iter().all(|r| r.status
        == WorkflowBoardCliImportHistoricalStatus::Historical
        && r.requires_native_revalidation));
    let prov = applied
        .card
        .cli_import
        .as_ref()
        .expect("cli_import populated");
    assert_eq!(
        prov.source_system,
        WorkflowBoardCliImportSourceSystem::LegionCli
    );
    assert_eq!(prov.owner, WorkflowBoardCliImportOwner::Legioncode);

    // Deliverables on disk and zero git commits created.
    let card_dir = repo_path
        .join(".legion/workflows")
        .join(applied.card_id.as_str());
    assert!(card_dir.join("card.json").is_file());
    assert!(card_dir.join("spec.md").is_file());
    assert!(card_dir.join("design.md").is_file());
    assert!(card_dir.join("plan.md").is_file());
    assert!(card_dir.join("tasks/TSK-0002.md").is_file());
    assert!(card_dir.join("import-provenance.json").is_file());
    assert_eq!(git_commit_count(repo_path), commits_before);

    // `getCard` reloads full `cliImport` provenance and task aliases.
    let detail = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: applied.card_id.clone(),
        project_id: None,
    })?;
    assert_eq!(
        detail
            .cli_import
            .as_ref()
            .map(|c| c.source_spec_id.as_str()),
        Some("spec-t3-code")
    );
    assert_eq!(
        detail
            .tasks
            .iter()
            .find(|t| t.task_id == "TSK-0002")
            .and_then(|t| t.id.as_deref()),
        Some("TSK-0002")
    );

    // Case A: Re-importing unchanged `.legion-cli` is a strict no-op (`created: false`, same revision).
    let re_applied = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    assert!(!re_applied.created);
    assert_eq!(re_applied.card_id, applied.card_id);
    assert_eq!(re_applied.revision, 1);

    let ack_rev = assert_execute_and_ship_gates(&board, &applied.card_id, &detail).await?;
    assert_native_edit_preservation(
        &orch,
        &board,
        &project_id,
        &cli_dir,
        &applied.card_id,
        ack_rev,
    )
    .await
}

async fn assert_execute_and_ship_gates(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    card_id: &legion_contracts::ids::WorkflowCardId,
    detail: &legion_contracts::workflow_board::WorkflowCardDetail,
) -> Result<u64, Box<dyn std::error::Error>> {
    let policy_err = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: card_id.clone(),
            expected_revision: 1,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: None,
            stable_launch_key: None,
            model_selection: Some(replay_model()),
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await
        .expect_err("Execute must block on unacknowledged unsupported policies");
    assert_eq!(
        policy_err.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );
    assert!(policy_err.message.contains("sandbox.requireHardened"));

    assert_ship_blocked(board, card_id, detail).await?;

    let tsk2_err = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: card_id.clone(),
            expected_revision: 1,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: None,
            stable_launch_key: None,
            model_selection: Some(replay_model()),
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: Some(true),
            acknowledged_policies: Some(vec![
                "sandbox.requireHardened".to_owned(),
                "qa.passScore".to_owned(),
                "skills.trustKeys".to_owned(),
            ]),
        })
        .await
        .expect_err("Execute must surface blocked TSK-0002 explicitly");
    assert_eq!(
        tsk2_err.code,
        WorkflowBoardRequestErrorCode::ContractConflict
    );
    assert!(tsk2_err.message.contains("TSK-0002"));

    let after_ack = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card_id.clone(),
        project_id: None,
    })?;
    assert_eq!(after_ack.summary.revision, 2);
    assert!(
        after_ack
            .cli_import
            .as_ref()
            .expect("cli_import")
            .unsupported_policies
            .iter()
            .filter(|p| p.blocks_execution)
            .all(|p| p.status == WorkflowBoardCliImportPolicyStatus::Acknowledged)
    );
    Ok(after_ack.summary.revision)
}

async fn assert_ship_blocked(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    card_id: &legion_contracts::ids::WorkflowCardId,
    detail: &legion_contracts::workflow_board::WorkflowCardDetail,
) -> TestResult {
    let ship_readiness = detail
        .ship_readiness
        .as_ref()
        .expect("ship_readiness populated");
    assert!(!ship_readiness.ready);
    assert!(
        ship_readiness
            .blockers
            .iter()
            .any(|b| b.kind == WorkflowBoardShipBlockerKind::PolicyUnacknowledged)
    );
    assert!(
        ship_readiness
            .blockers
            .iter()
            .any(|b| b.kind == WorkflowBoardShipBlockerKind::SpecStale)
    );
    assert!(
        ship_readiness
            .blockers
            .iter()
            .any(|b| b.kind == WorkflowBoardShipBlockerKind::PlanStale)
    );
    let ship_err = board
        .approve_delivery(
            &WorkflowBoardApproveDeliveryInput {
                card_id: card_id.clone(),
                expected_revision: 1,
                expected_product_snapshot_digest: None,
                note: None,
            },
            "client",
        )
        .await
        .expect_err("Ship must remain blocked on historical evidence");
    assert_eq!(
        ship_err.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );
    Ok(())
}

async fn assert_native_edit_preservation(
    orch: &legion_orchestrator::Orchestrator,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    project_id: &ProjectId,
    cli_dir: &Path,
    card_id: &legion_contracts::ids::WorkflowCardId,
    ack_rev: u64,
) -> TestResult {
    let updated = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: card_id.clone(),
            expected_revision: ack_rev,
            title: Some("Native Edited Title".to_owned()),
            brief: Some("Native brief preserved".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: legion_contracts::orchestration::PatchValue::Absent,
        })
        .await?;
    assert_eq!(updated.card.summary.title, "Native Edited Title");
    fs::write(
        cli_dir.join("assumptions/ASM-0019.md"),
        "---\nid: ASM-0019\n---\nNew note\n",
    )?;
    let sync_after_edit = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: None,
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    assert!(!sync_after_edit.created);
    assert_eq!(sync_after_edit.card.summary.title, "Native Edited Title");
    assert_eq!(sync_after_edit.card.summary.stage, WorkflowBoardStage::Spec);
    let conn = orch.sink().db().lock().expect("db lock");
    let last_rev: i64 = conn.query_row(
        "SELECT last_imported_revision FROM workflow_board_cli_imports WHERE card_id = ?1",
        params![card_id.as_str()],
        |r| r.get(0),
    )?;
    assert_eq!(last_rev, 1);
    Ok(())
}
