//! Regression tests for CLI-imported `"historical"` plan approval semantics in `claim_or_start_task`.

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AlternativeSelectionUpdate, ApplyCliImportInput, ApproveSpecInput, ClaimOrStartTaskInput,
    CliImportApprovalSeed, CliImportTaskSeed, CreateCardInput, ReturnForRevisionInput,
    UpdatePlanInput, UpdateSpecInput, WorkflowBoardRepoError, apply_cli_import_tx, approve_spec,
    claim_or_start_task, create_card, invalidate_plan_approval, invalidate_spec_approval,
    return_for_revision, update_plan, update_spec,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const READY_TASKS: [CliImportTaskSeed<'static>; 1] = [CliImportTaskSeed {
    task_id: "TSK-0002",
    ordinal: 0,
    title: "Qualify MiMo",
    status: "todo",
    priority: "P0",
    parent_id: None,
    blocked_by_json: "[]",
    blocks_json: "[]",
    files_allowed_json: "[\"src/lib.rs\"]",
    files_forbidden_json: "[]",
    expected_artifacts_json: "[]",
    verification_commands_json: "[]",
    testing_method: "test-first",
    acceptance_ids_json: "[\"AC-P0-01\"]",
    blocker_reason: None,
    notes: None,
    max_files_touched: Some(1),
    contract_digest: Some("sha256:tsk2"),
    last_error: None,
}];

const HIST_APPROVALS: [CliImportApprovalSeed<'static>; 2] = [
    CliImportApprovalSeed {
        id: "apr-spec",
        gate: "spec",
        artifact_digest: "sha256:spec",
        spec_digest: Some("sha256:spec"),
        plan_digest: None,
        tasks_digest: None,
        approved_by: "legion-cli:user",
        approved_at: "2026-10-22T01:00:00Z",
        metadata_json: "{}",
    },
    CliImportApprovalSeed {
        id: "apr-plan",
        gate: "plan",
        artifact_digest: "sha256:plan",
        spec_digest: Some("sha256:spec"),
        plan_digest: Some("sha256:plan"),
        tasks_digest: Some("sha256:tasks"),
        approved_by: "legion-cli:user",
        approved_at: "2026-10-22T01:10:00Z",
        metadata_json: "{}",
    },
];

fn seed_cli_card(
    conn: &rusqlite::Connection,
    candidate_card_id: &str,
    spec_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let prov = serde_json::json!({
        "sourceSystem": "legion-cli",
        "sourceCliDir": ".legion-cli",
        "sourceSpecId": spec_id,
        "owner": "legioncode",
        "importedAt": "2026-11-26T00:00:00Z",
        "lastSyncedAt": "2026-11-26T00:00:00Z",
        "sourceDigest": "sha256:claim",
        "decisionIds": [],
        "assumptionIds": [],
        "acceptanceCriterionIds": ["AC-P0-01"],
        "historicalReceipts": [],
        "unsupportedPolicies": [],
        "acknowledgedPolicies": [],
        "missingLogNotes": [],
        "preservedArtifactPaths": [],
        "assurancePolicySummary": null
    })
    .to_string();
    let out = apply_cli_import_tx(
        conn,
        &ApplyCliImportInput {
            candidate_card_id,
            revision_id: &format!("rev-{candidate_card_id}"),
            project_id: "proj-m4-claim",
            source_cli_spec_id: spec_id,
            source_cli_dir: ".legion-cli",
            source_digest: "sha256:claim",
            title: "CLI Claim Card",
            stage: "execute",
            status: "active",
            brief: "Brief",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_digest: "sha256:spec",
            design_digest: None,
            plan_digest: Some("sha256:plan"),
            tasks_digest: Some("sha256:tasks"),
            current_task_id: None,
            blocker_reason: None,
            next_action: Some("start_execute"),
            spec_json: "{}",
            plan_json: None,
            unsupported_policies_json: "[]",
            acknowledged_policies_json: "[]",
            policies_acknowledged_at: None,
            assurance_policy_json: None,
            import_provenance_json: &prov,
            decisions: &[],
            tasks: &READY_TASKS,
            planned_checks: &[],
            check_receipts: &[],
            ac_evidence: &[],
            historical_approvals: &HIST_APPROVALS,
            historical_review_summary: None,
            historical_review_at: None,
        },
        "2026-11-26T00:00:00Z",
    )?;
    Ok(out.card_id)
}

#[test]
fn claim_or_start_task_allows_cli_historical_plan_only_when_not_stale_and_blocks_native()
-> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let card_id = seed_cli_card(conn, "cli-claim-ok", "spec-ok")?;
    let claimed = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: &card_id,
            revision_id: "rev-claim-ok-2",
            expected_revision: 1,
            task_id: "TSK-0002",
            stage_run_id: None,
        },
        "2026-11-26T00:01:00Z",
    )?;
    assert_eq!(claimed.planning.tasks[0].status, "in_progress");

    // Stale CLI plan approval must fail.
    let stale_id = seed_cli_card(conn, "cli-claim-stale", "spec-stale")?;
    conn.execute(
        "UPDATE workflow_board_approvals SET status = 'stale' WHERE card_id = ?1 AND gate = 'plan'",
        [stale_id.as_str()],
    )?;
    let stale_err = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: &stale_id,
            revision_id: "rev-claim-stale-2",
            expected_revision: 1,
            task_id: "TSK-0002",
            stage_run_id: None,
        },
        "2026-11-26T00:01:00Z",
    )
    .expect_err("stale CLI plan approval must fail");
    assert!(matches!(
        stale_err,
        WorkflowBoardRepoError::PreconditionFailed(_)
    ));

    // Non-CLI native card without valid Plan approval must fail even if a historical row exists.
    create_card(
        conn,
        &CreateCardInput {
            card_id: "native-no-plan",
            revision_id: "rev-nat-1",
            project_id: "proj-m4-claim",
            title: "Native",
            brief: "Brief",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "balanced",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{}",
            spec_digest: Some("sha256:nat"),
            wireframes_digest: None,
        },
        "2026-11-26T00:00:00Z",
    )?;
    conn.execute(
        "UPDATE workflow_board_cards SET stage = 'execute' WHERE card_id = 'native-no-plan'",
        [],
    )?;
    let nat_err = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "native-no-plan",
            revision_id: "rev-nat-2",
            expected_revision: 1,
            task_id: "TSK-0001",
            stage_run_id: None,
        },
        "2026-11-26T00:01:00Z",
    )
    .expect_err("native card without valid plan approval must fail");
    assert!(matches!(
        nat_err,
        WorkflowBoardRepoError::PreconditionFailed(_)
    ));
    Ok(())
}

fn gate_status(
    conn: &rusqlite::Connection,
    card_id: &str,
    gate: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    Ok(conn.query_row(
        "SELECT status FROM workflow_board_approvals
         WHERE card_id = ?1 AND gate = ?2
         ORDER BY CASE WHEN status = 'valid' THEN 0 ELSE 1 END, approved_at DESC, id DESC
         LIMIT 1",
        [card_id, gate],
        |r| r.get(0),
    )?)
}

fn assert_invalidation_and_updates_stale_historical(conn: &rusqlite::Connection) -> TestResult {
    let c2 = seed_cli_card(conn, "cli-inv-2", "spec-inv-2")?;
    assert!(invalidate_plan_approval(
        conn,
        &c2,
        "external_plan_edit",
        "2026-11-26T00:01:00Z",
    )?);
    assert_eq!(gate_status(conn, &c2, "plan")?, "stale");
    assert_eq!(gate_status(conn, &c2, "spec")?, "historical");
    assert!(invalidate_spec_approval(
        conn,
        &c2,
        "external_spec_edit",
        "2026-11-26T00:02:00Z",
    )?);
    assert_eq!(gate_status(conn, &c2, "spec")?, "stale");

    let c3 = seed_cli_card(conn, "cli-upd-3", "spec-upd-3")?;
    update_plan(
        conn,
        &UpdatePlanInput {
            card_id: &c3,
            revision_id: "rev-upd-plan",
            expected_revision: 1,
            lenses: None,
            alternatives: None,
            selected_alternative_id: AlternativeSelectionUpdate::Keep,
            selection_rationale: None,
            revise_alternatives_feedback: None,
            strategy: None,
            strategy_rationale: Some("Updated strategy"),
            strategy_metadata_json: None,
            granularity: None,
            tasks: None,
            checks: None,
            ac_mappings: None,
            plan_json: None,
            plan_digest: None,
            design_digest: None,
            tasks_digest: None,
        },
        "2026-11-26T00:01:00Z",
    )?;
    assert_eq!(gate_status(conn, &c3, "plan")?, "stale");
    assert_eq!(gate_status(conn, &c3, "spec")?, "historical");

    update_spec(
        conn,
        &UpdateSpecInput {
            card_id: &c3,
            revision_id: "rev-upd-spec",
            expected_revision: 2,
            title: Some("Revised Title"),
            brief: None,
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: None,
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-26T00:02:00Z",
    )?;
    assert_eq!(gate_status(conn, &c3, "spec")?, "stale");
    assert_eq!(gate_status(conn, &c3, "plan")?, "stale");
    Ok(())
}

#[test]
fn return_for_revision_and_invalidation_mark_cli_historical_approvals_stale() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let c1 = seed_cli_card(conn, "cli-ret-1", "spec-ret-1")?;
    assert_eq!(gate_status(conn, &c1, "spec")?, "historical");
    assert_eq!(gate_status(conn, &c1, "plan")?, "historical");

    let ret_plan = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: &c1,
            revision_id: "rev-ret-plan",
            expected_revision: 1,
            target_stage: "plan",
            reason: "Revise plan",
        },
        "2026-11-26T00:01:00Z",
    )?;
    assert_eq!(ret_plan.card_detail.card.stage, "plan");
    assert_eq!(gate_status(conn, &c1, "plan")?, "stale");
    assert_eq!(gate_status(conn, &c1, "spec")?, "historical");

    let ret_spec = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: &c1,
            revision_id: "rev-ret-spec",
            expected_revision: 2,
            target_stage: "spec",
            reason: "Revise spec",
        },
        "2026-11-26T00:02:00Z",
    )?;
    assert_eq!(ret_spec.card_detail.card.stage, "spec");
    assert_eq!(gate_status(conn, &c1, "spec")?, "stale");
    assert_eq!(gate_status(conn, &c1, "plan")?, "stale");

    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: &c1,
            revision_id: "rev-apr-spec",
            approval_id: "apr-spec-fresh",
            expected_revision: 3,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-26T00:03:00Z",
    )?;
    assert_eq!(gate_status(conn, &c1, "spec")?, "valid");
    assert_invalidation_and_updates_stale_historical(conn)
}
