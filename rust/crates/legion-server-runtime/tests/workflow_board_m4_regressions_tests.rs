#![forbid(unsafe_code)]
//! Regression tests for M4.1 Iteration 2 fixes:
//! - `startStage(Execute)` OCC & precondition ordering + immediate disk deliverable sync
//! - Case A / Case B / Case C re-import policy preservation and result parity
//! - Hydrating `"accepted"` CLI decisions as `Resolved`
//! - Scanner & renderer edge cases (`mcpHttpToolAllowlist` maps, multi-checks, quoted commands, `prd.md`/`plans/<id>.md`, `config.yaml` ref)

#[path = "workflow_board_m2_support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::ids::{ProjectId, WorkflowCardId};
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApproveSpecInput,
    WorkflowBoardCliImportPolicyStatus, WorkflowBoardExplorationDecisionStatus,
    WorkflowBoardGetCardInput, WorkflowBoardPauseCardInput, WorkflowBoardPreviewCliImportInput,
    WorkflowBoardRequestErrorCode, WorkflowBoardResumeCardInput, WorkflowBoardShipBlockerKind,
    WorkflowBoardStage, WorkflowBoardStageRunStage, WorkflowBoardStartStageInput,
    WorkflowBoardUpdateSpecInput,
};
use legion_server_runtime::ProjectWorkflowBoardService;
use legion_workflows::sha256_prefixed;
use serde_json::Value;
use support::{TestResult, replay_model, setup_m2_board};

fn copy_fixture_cli(repo_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.legion-cli");
    let dst = repo_path.join(".legion-cli");
    copy_dir_recursive(&src, &dst)?;
    Ok(dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn execute_input(
    card_id: &WorkflowCardId,
    expected_revision: u64,
    ack: Option<bool>,
    ack_keys: Option<Vec<String>>,
) -> WorkflowBoardStartStageInput {
    WorkflowBoardStartStageInput {
        card_id: card_id.clone(),
        expected_revision,
        stage: WorkflowBoardStageRunStage::Execute,
        task_id: None,
        stable_launch_key: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
        acknowledge_unsupported_policies: ack,
        acknowledged_policies: ack_keys,
    }
}

fn apply_input(
    project_id: &ProjectId,
    cli_dir: Option<&str>,
    ack: Option<bool>,
) -> WorkflowBoardApplyCliImportInput {
    WorkflowBoardApplyCliImportInput {
        project_id: project_id.clone(),
        cli_dir: cli_dir.map(str::to_owned),
        acknowledge_unsupported_policies: ack,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    }
}

async fn assert_failed_start_preserves_state(
    board: &ProjectWorkflowBoardService,
    card_id: &WorkflowCardId,
    expected_rev: u64,
    ack_keys: Option<Vec<String>>,
    expected_code: WorkflowBoardRequestErrorCode,
    actual_rev: u64,
) -> TestResult {
    let err = board
        .start_stage(&execute_input(card_id, expected_rev, Some(true), ack_keys))
        .await
        .expect_err("startStage(Execute) should fail precondition");
    assert_eq!(err.code, expected_code);
    let detail = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card_id.clone(),
        project_id: None,
    })?;
    assert_eq!(detail.summary.revision, actual_rev);
    assert_eq!(
        detail
            .cli_import
            .as_ref()
            .expect("cli_import")
            .acknowledged_policies,
        Vec::<String>::new()
    );
    Ok(())
}

#[tokio::test]
async fn start_stage_execute_enforces_occ_preconditions_before_policy_ack_and_syncs_disk()
-> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    copy_fixture_cli(repo_path)?;
    let project_id = ProjectId::new("proj-board-1")?;
    let applied = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    let card_id = applied.card_id.clone();

    let paused = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: card_id.clone(),
            expected_revision: 1,
            reason: Some("Paused before execute".to_owned()),
        })
        .await?;
    assert_eq!(paused.revision, 2);
    assert_failed_start_preserves_state(
        &board,
        &card_id,
        2,
        None,
        WorkflowBoardRequestErrorCode::InvalidStageTransition,
        2,
    )
    .await?;

    let resumed = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: card_id.clone(),
            expected_revision: 2,
            reconcile_interrupted: None,
            note: None,
        })
        .await?;
    assert_eq!(resumed.revision, 3);
    assert_failed_start_preserves_state(
        &board,
        &card_id,
        1,
        None,
        WorkflowBoardRequestErrorCode::RevisionConflict,
        3,
    )
    .await?;
    assert_failed_start_preserves_state(
        &board,
        &card_id,
        3,
        Some(vec!["sandbox.requireHardened".to_owned()]),
        WorkflowBoardRequestErrorCode::InvalidStageTransition,
        3,
    )
    .await?;

    let tsk2_err = board
        .start_stage(&execute_input(&card_id, 3, Some(true), None))
        .await
        .expect_err("TSK-0002 blocked must surface");
    assert_eq!(
        tsk2_err.code,
        WorkflowBoardRequestErrorCode::ContractConflict
    );
    assert!(tsk2_err.message.contains("TSK-0002"));

    let card_dir = repo_path.join(".legion/workflows").join(card_id.as_str());
    let disk_card: Value = serde_json::from_str(&fs::read_to_string(card_dir.join("card.json"))?)?;
    assert_eq!(disk_card["revision"], 4);
    assert!(
        disk_card["unsupportedPolicies"]
            .as_array()
            .expect("array")
            .iter()
            .all(|p| p["status"] == "acknowledged")
    );
    let disk_prov: Value = serde_json::from_str(&fs::read_to_string(
        card_dir.join("import-provenance.json"),
    )?)?;
    assert!(
        disk_prov["unsupportedPolicies"]
            .as_array()
            .expect("array")
            .iter()
            .all(|p| p["status"] == "acknowledged")
    );
    Ok(())
}

#[tokio::test]
async fn cli_import_case_a_b_c_and_accepted_decisions() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let cli_dir = copy_fixture_cli(repo_path)?;
    let project_id = ProjectId::new("proj-board-1")?;

    let applied = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    let card_id = applied.card_id.clone();

    let exploration = applied.card.exploration.as_ref().expect("exploration");
    assert_eq!(exploration.unresolved_ids, Vec::<String>::new());
    assert_eq!(exploration.decisions.len(), 8);
    assert!(
        exploration
            .decisions
            .iter()
            .all(|d| d.status == WorkflowBoardExplorationDecisionStatus::Resolved)
    );
    assert_eq!(
        applied
            .card
            .checks
            .iter()
            .filter(|c| c.id.starts_with("chk-tsk-0008"))
            .count(),
        3
    );

    let card_dir = repo_path.join(".legion/workflows").join(card_id.as_str());
    let spec_t1 = fs::read(card_dir.join("spec.md"))?;
    let design_t1 = fs::read(card_dir.join("design.md"))?;
    let plan_t1 = fs::read(card_dir.join("plan.md"))?;
    std::thread::sleep(std::time::Duration::from_millis(5));

    let case_a_ack = board.apply_cli_import(&apply_input(&project_id, None, Some(true)))?;
    assert!(!case_a_ack.created);
    assert_eq!(case_a_ack.revision, 2);
    assert_eq!(fs::read(card_dir.join("spec.md"))?, spec_t1);
    assert_eq!(fs::read(card_dir.join("design.md"))?, design_t1);
    assert_eq!(fs::read(card_dir.join("plan.md"))?, plan_t1);
    let disk_card_a: Value =
        serde_json::from_str(&fs::read_to_string(card_dir.join("card.json"))?)?;
    assert_eq!(
        Some(sha256_prefixed(&spec_t1).as_str()),
        case_a_ack.card.spec.spec_digest.as_deref()
    );
    assert_eq!(
        sha256_prefixed(&design_t1),
        disk_card_a["designDigest"].as_str().unwrap_or("")
    );
    assert_eq!(
        sha256_prefixed(&plan_t1),
        disk_card_a["planDigest"].as_str().unwrap_or("")
    );
    let disk_prov: Value = serde_json::from_str(&fs::read_to_string(
        card_dir.join("import-provenance.json"),
    )?)?;
    assert!(
        disk_prov["unsupportedPolicies"]
            .as_array()
            .expect("array")
            .iter()
            .filter(|p| p["blocksExecution"] == true)
            .all(|p| p["status"] == "acknowledged")
    );

    fs::write(
        cli_dir.join("assumptions/ASM-0019.md"),
        "---\nid: ASM-0019\n---\nExtra assumption\n",
    )?;
    let case_b = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    assert_eq!(case_b.revision, 3);
    assert!(
        case_b
            .unsupported_policies
            .iter()
            .filter(|p| p.blocks_execution)
            .all(|p| p.status == WorkflowBoardCliImportPolicyStatus::Acknowledged)
    );

    board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id,
            expected_revision: 3,
            title: Some("Owned By LegionCode".to_owned()),
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: legion_contracts::orchestration::PatchValue::Absent,
        })
        .await?;
    fs::write(cli_dir.join("workflow/assurance.yaml"), "mode: strict\n")?;
    let case_c = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    assert_eq!(case_c.revision, 4);
    let c_prov = case_c.card.cli_import.as_ref().expect("cli_import");
    assert_eq!(case_c.unsupported_policies, c_prov.unsupported_policies);
    assert_eq!(case_c.historical_receipts, c_prov.historical_receipts);
    assert_eq!(case_c.missing_log_notes, c_prov.missing_log_notes);
    Ok(())
}

#[tokio::test]
async fn cli_import_scanner_fallbacks_and_edge_cases() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let project_id = ProjectId::new("proj-board-1")?;
    let alt_dir = repo_path.join(".legion-cli-alt");
    fs::create_dir_all(alt_dir.join("specs/spec-alt"))?;
    fs::create_dir_all(alt_dir.join("plans"))?;
    fs::create_dir_all(alt_dir.join("tasks"))?;
    fs::write(
        alt_dir.join("specs/spec-alt/prd.md"),
        "---\ntitle: Alt Spec\n---\nFallback PRD brief\n",
    )?;
    fs::write(alt_dir.join("plans/spec-alt.md"), "Fallback plan design\n")?;
    fs::write(
        alt_dir.join("tasks/TSK-0001.md"),
        "---\nid: TSK-0001\ntitle: Alt Task\nstatus: todo\ncontract:\n  filesAllowed:\n    - .git/config\n    - ../escape\n    - src/lib.rs\n  verificationCommands:\n    - node -e \"console.log('a b')\"\n---\nBody\n",
    )?;
    let alt_preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id: project_id.clone(),
        cli_dir: Some(".legion-cli-alt".to_owned()),
    })?;
    assert_eq!(alt_preview.spec_id, "spec-alt");
    assert_eq!(
        alt_preview.tasks[0].verification_commands,
        vec![vec![
            "node".to_owned(),
            "-e".to_owned(),
            "console.log('a b')".to_owned()
        ]]
    );
    let alt_apply =
        board.apply_cli_import(&apply_input(&project_id, Some(".legion-cli-alt"), None))?;
    assert!(
        !alt_apply
            .card
            .spec
            .source_refs
            .iter()
            .any(|r| r.ends_with("config.yaml"))
    );
    assert_eq!(
        alt_apply.tasks[0].files_allowed,
        vec!["src/lib.rs".to_owned()]
    );

    fs::write(
        alt_dir.join("config.yaml"),
        "mcpHttpToolAllowlist: { serverA: [tool_a] }\n",
    )?;
    let map_preview = board.preview_cli_import(&WorkflowBoardPreviewCliImportInput {
        project_id,
        cli_dir: Some(".legion-cli-alt".to_owned()),
    })?;
    assert!(
        map_preview
            .unsupported_policies
            .iter()
            .any(|p| p.key == "mcpHttpToolAllowlist" && p.blocks_execution)
    );
    Ok(())
}

#[tokio::test]
async fn spec_stage_cli_card_rejects_start_execute_preserves_digests_and_approves_spec()
-> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let cli_dir = copy_fixture_cli(repo_path)?;
    for rel in ["workflow/plan-approval.yaml", "workflow/execution.yaml"] {
        let p = cli_dir.join(rel);
        if p.exists() {
            fs::remove_file(p)?;
        }
    }
    for dir in ["tasks", "plans"] {
        let p = cli_dir.join(dir);
        if p.exists() {
            fs::remove_dir_all(p)?;
        }
    }
    let project_id = ProjectId::new("proj-board-1")?;
    let applied = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    assert_eq!(applied.card.summary.stage, WorkflowBoardStage::Spec);
    let card_id = applied.card_id.clone();

    assert_failed_start_preserves_state(
        &board,
        &card_id,
        1,
        None,
        WorkflowBoardRequestErrorCode::InvalidStageTransition,
        1,
    )
    .await?;

    let card_dir = repo_path.join(".legion/workflows").join(card_id.as_str());
    let spec_t1 = fs::read(card_dir.join("spec.md"))?;
    let design_t1 = fs::read(card_dir.join("design.md"))?;
    let plan_t1 = fs::read(card_dir.join("plan.md"))?;
    std::thread::sleep(std::time::Duration::from_millis(5));

    let acked = board.apply_cli_import(&apply_input(&project_id, None, Some(true)))?;
    assert_eq!(acked.revision, 2);
    assert_eq!(fs::read(card_dir.join("spec.md"))?, spec_t1);
    assert_eq!(fs::read(card_dir.join("design.md"))?, design_t1);
    assert_eq!(fs::read(card_dir.join("plan.md"))?, plan_t1);

    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id,
                expected_revision: 2,
                expected_digest: acked.card.spec.spec_digest.clone(),
            },
            "client",
        )
        .await?;
    assert_eq!(approved.revision, 3);
    assert_eq!(approved.card.summary.stage, WorkflowBoardStage::Plan);
    Ok(())
}

#[tokio::test]
async fn cli_import_case_b_requires_reack_when_policy_escalates_to_blocking() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let cli_dir = repo_path.join(".legion-cli-esc");
    fs::create_dir_all(cli_dir.join("specs/spec-esc"))?;
    fs::write(
        cli_dir.join("specs/spec-esc/SPEC.md"),
        "---\ntitle: Escalation Test\n---\nBody\n",
    )?;
    fs::write(
        cli_dir.join("config.yaml"),
        "sandbox:\n  requireHardened: true\nmcpHttpToolAllowlist: []\n",
    )?;
    let project_id = ProjectId::new("proj-board-1")?;
    let first = board.apply_cli_import(&apply_input(
        &project_id,
        Some(".legion-cli-esc"),
        Some(true),
    ))?;
    assert_eq!(first.revision, 1);
    assert!(
        first
            .unsupported_policies
            .iter()
            .all(|p| p.status == WorkflowBoardCliImportPolicyStatus::Acknowledged)
    );

    fs::write(
        cli_dir.join("config.yaml"),
        "sandbox:\n  requireHardened: true\nmcpHttpToolAllowlist:\n  serverA: [read]\n",
    )?;
    let escalated =
        board.apply_cli_import(&apply_input(&project_id, Some(".legion-cli-esc"), None))?;
    assert_eq!(escalated.revision, 2);
    let mcp = escalated
        .unsupported_policies
        .iter()
        .find(|p| p.key == "mcpHttpToolAllowlist")
        .ok_or("missing mcpHttpToolAllowlist")?;
    assert!(mcp.blocks_execution);
    assert_eq!(mcp.status, WorkflowBoardCliImportPolicyStatus::Unsupported);
    assert!(escalated.card.ship_readiness.as_ref().is_some_and(|sr| {
        sr.blockers
            .iter()
            .any(|b| b.kind == WorkflowBoardShipBlockerKind::PolicyUnacknowledged)
    }));
    let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
    let acked_at: Option<String> = conn.query_row(
        "SELECT policies_acknowledged_at FROM workflow_board_cli_imports WHERE card_id = ?1",
        [escalated.card_id.as_str()],
        |r| r.get(0),
    )?;
    assert!(acked_at.is_none());
    Ok(())
}

#[tokio::test]
async fn missing_deliverables_restored_on_policy_ack_match_sqlite_digests() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    let cli_dir = copy_fixture_cli(repo_path)?;
    for rel in ["workflow/plan-approval.yaml", "workflow/execution.yaml"] {
        let p = cli_dir.join(rel);
        if p.exists() {
            fs::remove_file(p)?;
        }
    }
    for dir in ["tasks", "plans"] {
        let p = cli_dir.join(dir);
        if p.exists() {
            fs::remove_dir_all(p)?;
        }
    }
    let project_id = ProjectId::new("proj-board-1")?;
    let applied = board.apply_cli_import(&apply_input(&project_id, None, None))?;
    assert_eq!(applied.card.summary.stage, WorkflowBoardStage::Spec);
    let card_id = applied.card_id.clone();

    let (db_spec, db_design, db_plan): (Option<String>, Option<String>, Option<String>) = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.query_row(
            "SELECT spec_digest, design_digest, plan_digest FROM workflow_board_cards WHERE card_id = ?1",
            [card_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    };
    let card_dir = repo_path.join(".legion/workflows").join(card_id.as_str());
    fs::remove_file(card_dir.join("spec.md"))?;
    fs::remove_file(card_dir.join("design.md"))?;
    fs::remove_file(card_dir.join("plan.md"))?;
    std::thread::sleep(std::time::Duration::from_millis(20));

    let acked = board.apply_cli_import(&apply_input(&project_id, None, Some(true)))?;
    assert_eq!(acked.revision, 2);
    assert_eq!(
        Some(sha256_prefixed(&fs::read(card_dir.join("spec.md"))?)),
        db_spec
    );
    assert_eq!(
        Some(sha256_prefixed(&fs::read(card_dir.join("design.md"))?)),
        db_design
    );
    assert_eq!(
        Some(sha256_prefixed(&fs::read(card_dir.join("plan.md"))?)),
        db_plan
    );

    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id,
                expected_revision: 2,
                expected_digest: acked.card.spec.spec_digest.clone(),
            },
            "client",
        )
        .await?;
    assert_eq!(approved.revision, 3);
    assert_eq!(approved.card.summary.stage, WorkflowBoardStage::Plan);
    Ok(())
}
