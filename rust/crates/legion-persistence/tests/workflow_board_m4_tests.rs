//! Integration tests for Migration 114 (`WorkflowBoardCliImport`), idempotent `.legion-cli`
//! upserts, ownership preservation, policy acknowledgement, and historical Ship blockers.

use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore};
use legion_persistence::workflow_board_repo::{
    ApplyCliImportInput, CliImportAcEvidenceSeed, CliImportApprovalSeed, CliImportCheckReceiptSeed,
    CliImportDecisionSeed, CliImportPlannedCheckSeed, CliImportTaskSeed, UpdateSpecInput,
    acknowledge_card_policies, apply_cli_import_tx, get_card_execution_detail, update_spec,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_policy_and_prov(digest: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let policies = serde_json::json!([{
        "key": "sandbox.requireHardened",
        "category": "sandbox",
        "summary": "Hardened sandbox",
        "configuredValue": "true",
        "status": "unsupported",
        "blocksExecution": true,
        "sourcePath": ".legion-cli/config.yaml"
    }])
    .to_string();
    let prov = serde_json::json!({
        "sourceSystem": "legion-cli",
        "sourceCliDir": ".legion-cli",
        "sourceSpecId": "spec-t3-code",
        "owner": "legioncode",
        "importedAt": "2026-11-26T00:00:00Z",
        "lastSyncedAt": "2026-11-26T00:00:00Z",
        "sourceDigest": digest,
        "decisionIds": ["D-001"],
        "assumptionIds": ["ASM-0001"],
        "acceptanceCriterionIds": ["AC-P0-01"],
        "historicalReceipts": [],
        "unsupportedPolicies": serde_json::from_str::<serde_json::Value>(&policies)?,
        "acknowledgedPolicies": [],
        "missingLogNotes": [],
        "preservedArtifactPaths": [".legion-cli/specs/spec-t3-code/SPEC.md"],
        "assurancePolicySummary": null
    })
    .to_string();
    Ok((policies, prov))
}

const SAMPLE_DECISIONS: [CliImportDecisionSeed<'static>; 1] = [CliImportDecisionSeed {
    decision_id: "D-001",
    prompt: "Provide setup",
    status: "accepted",
    answer: Some("Accepted"),
    rationale: None,
}];

const SAMPLE_TASKS: [CliImportTaskSeed<'static>; 1] = [CliImportTaskSeed {
    task_id: "TSK-0002",
    ordinal: 0,
    title: "Qualify MiMo",
    status: "blocked",
    priority: "P0",
    parent_id: None,
    blocked_by_json: "[]",
    blocks_json: "[\"TSK-0001\"]",
    files_allowed_json: "[\"packages/provider-mimo/qualification/cli.test.mjs\"]",
    files_forbidden_json: "[\".legion-cli/config.yaml\"]",
    expected_artifacts_json: "[\"packages/provider-mimo/qualification/cli.test.mjs\"]",
    verification_commands_json: "[[\"node\",\"--test\",\"packages/provider-mimo/qualification/cli.test.mjs\"]]",
    testing_method: "test-first",
    acceptance_ids_json: "[\"AC-P0-01\"]",
    blocker_reason: Some("Missing verify-1.log"),
    notes: None,
    max_files_touched: Some(2),
    contract_digest: Some("sha256:tsk2"),
    last_error: Some("verification command failed"),
}];

const SAMPLE_CHECKS: [CliImportPlannedCheckSeed<'static>; 1] = [CliImportPlannedCheckSeed {
    check_id: "chk-tsk-0002",
    owner_task_id: Some("TSK-0002"),
    argv_json: "[\"node\",\"--test\",\"packages/provider-mimo/qualification/cli.test.mjs\"]",
    label: Some("Q-CLI"),
    last_exit_code: Some(1),
    last_log_ref: Some(".legion-cli/cache/runs/execute-mviwmzlq-24861406/verify-1.log"),
    stale_reason: Some("imported_historical_cli_receipt"),
    last_receipt_id: Some("hist-chk-cli-spec-t3-code"),
    last_run_at: Some("2026-10-22T02:28:41.128Z"),
    last_output_excerpt: Some("missing log"),
}];

const SAMPLE_RECEIPTS: [CliImportCheckReceiptSeed<'static>; 1] = [CliImportCheckReceiptSeed {
    id: "chk-1",
    check_id: "chk-tsk-0002",
    criterion_id: Some("AC-P0-01"),
    task_id: Some("TSK-0002"),
    argv_json: "[\"node\",\"--test\",\"packages/provider-mimo/qualification/cli.test.mjs\"]",
    exit_code: Some(1),
    log_ref: Some(".legion-cli/cache/runs/execute-mviwmzlq-24861406/verify-1.log"),
    stderr_excerpt: Some("missing log"),
    stale_reason: Some("imported_historical_cli_receipt"),
    executed_at: Some("2026-10-22T02:28:41.128Z"),
}];

const SAMPLE_ACS: [CliImportAcEvidenceSeed<'static>; 1] = [CliImportAcEvidenceSeed {
    criterion_id: "AC-P0-01",
    method: "automated",
    check_ids_json: "[\"chk-tsk-0002\"]",
    command_argv_json: None,
    procedure: None,
    observation_note: Some("Historical CLI AC"),
    external_source: Some(".legion-cli/workflow/plan-approval.yaml"),
}];

const SAMPLE_APPROVALS: [CliImportApprovalSeed<'static>; 1] = [CliImportApprovalSeed {
    id: "apr-spec",
    gate: "spec",
    artifact_digest: "sha256:spec",
    spec_digest: Some("sha256:spec"),
    plan_digest: None,
    tasks_digest: None,
    approved_by: "legion-cli:user",
    approved_at: "2026-10-22T01:00:17.476Z",
    metadata_json: "{}",
}];

fn run_import<'a>(
    conn: &rusqlite::Connection,
    rev_id: &'a str,
    digest: &'a str,
    title: &'a str,
) -> Result<
    legion_persistence::workflow_board_repo::ApplyCliImportOutcome,
    Box<dyn std::error::Error>,
> {
    let (policies, prov) = sample_policy_and_prov(digest)?;
    let out = apply_cli_import_tx(
        conn,
        &ApplyCliImportInput {
            candidate_card_id: "cli-spec-t3-code",
            revision_id: rev_id,
            project_id: "proj-m4",
            source_cli_spec_id: "spec-t3-code",
            source_cli_dir: ".legion-cli",
            source_digest: digest,
            title,
            stage: "execute",
            status: "blocked",
            brief: "Imported brief",
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
            current_task_id: Some("TSK-0002"),
            blocker_reason: Some("TSK-0002 is blocked"),
            next_action: Some("acknowledge_policies"),
            spec_json: "{}",
            plan_json: None,
            unsupported_policies_json: &policies,
            acknowledged_policies_json: "[]",
            policies_acknowledged_at: None,
            assurance_policy_json: None,
            import_provenance_json: &prov,
            decisions: &SAMPLE_DECISIONS,
            tasks: &SAMPLE_TASKS,
            planned_checks: &SAMPLE_CHECKS,
            check_receipts: &SAMPLE_RECEIPTS,
            ac_evidence: &SAMPLE_ACS,
            historical_approvals: &SAMPLE_APPROVALS,
            historical_review_summary: Some("Historical review"),
            historical_review_at: Some("2026-10-22T01:20:00Z"),
        },
        "2026-11-26T00:00:00Z",
    )?;
    Ok(out)
}

#[test]
fn cli_import_idempotency_ownership_policy_ack_and_ship_blockers() -> TestResult {
    assert_eq!(BUILD_MIGRATION_HEAD, 114);
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let first = run_import(conn, "rev-1", "sha256:src-1", "T3 Code")?;
    assert!(first.created);
    assert!(!first.reused);
    assert_eq!(first.card_id, "cli-spec-t3-code");
    assert_eq!(first.revision, 1);

    // Case A: identical source digest is a strict no-op
    let repeat_a = run_import(conn, "rev-2", "sha256:src-1", "T3 Code")?;
    assert!(!repeat_a.created);
    assert!(repeat_a.reused);
    assert!(!repeat_a.ownership_preserved);
    assert_eq!(repeat_a.card_id, first.card_id);
    assert_eq!(repeat_a.revision, 1);

    // Case B: changed source digest with matching revision updates in-place
    let repeat_b = run_import(conn, "rev-3", "sha256:src-2", "T3 Code Updated")?;
    assert!(!repeat_b.created);
    assert!(!repeat_b.reused);
    assert_eq!(repeat_b.revision, 2);
    assert_eq!(repeat_b.card_detail.card.title, "T3 Code Updated");

    // Historical evidence + unacknowledged policy blocks Ship
    let exec = get_card_execution_detail(conn, &first.card_id)?;
    assert!(
        exec.ship_blockers
            .iter()
            .any(|b| b.kind == "policy_unacknowledged")
    );
    assert!(exec.ship_blockers.iter().any(|b| b.kind == "check_stale"));
    assert!(exec.ship_blockers.iter().any(|b| b.kind == "spec_stale"));

    // Acknowledge policies
    let ack_keys = vec!["sandbox.requireHardened".to_owned()];
    let (acked_row, ack_rev) = acknowledge_card_policies(
        conn,
        &first.card_id,
        2,
        "rev-ack",
        Some(&ack_keys),
        "2026-11-26T00:01:00Z",
    )?;
    assert_eq!(ack_rev, 3);
    assert!(acked_row.policies_acknowledged_at.is_some());

    // LegionCode mutation bumps revision beyond last_imported_revision
    let updated = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: &first.card_id,
            revision_id: "rev-lc",
            expected_revision: 3,
            title: Some("LegionCode Owned Title"),
            brief: None,
            intake_goal: None,
            input_paths_json: None,
            wireframes_digest: None,
            clear_wireframes_digest: false,
            spec_digest: Some("sha256:spec-lc"),
            spec_json: Some("{}"),
        },
        "2026-11-26T00:02:00Z",
    )?;
    assert_eq!(updated.card.revision, 4);

    // Case C: repeat import preserves LegionCode-owned edits
    let repeat_c = run_import(conn, "rev-4", "sha256:src-3", "CLI Should Not Overwrite")?;
    assert!(repeat_c.reused);
    assert!(repeat_c.ownership_preserved);
    assert_eq!(repeat_c.card_detail.card.title, "LegionCode Owned Title");
    assert_eq!(repeat_c.revision, 4);

    Ok(())
}

struct PolImportParams<'a> {
    rev_id: &'a str,
    digest: &'a str,
    title: &'a str,
    policies: &'a str,
    acked_json: &'a str,
    acked_at: Option<&'a str>,
    now: &'a str,
}

fn run_pol_import(
    conn: &rusqlite::Connection,
    p: &PolImportParams<'_>,
) -> Result<
    legion_persistence::workflow_board_repo::ApplyCliImportOutcome,
    Box<dyn std::error::Error>,
> {
    let (_, prov) = sample_policy_and_prov(p.digest)?;
    Ok(apply_cli_import_tx(
        conn,
        &ApplyCliImportInput {
            candidate_card_id: "cli-pol-card",
            revision_id: p.rev_id,
            project_id: "proj-m4-pol",
            source_cli_spec_id: "spec-pol",
            source_cli_dir: ".legion-cli",
            source_digest: p.digest,
            title: p.title,
            stage: "execute",
            status: "blocked",
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
            current_task_id: Some("TSK-0002"),
            blocker_reason: Some("TSK-0002 blocked"),
            next_action: Some("acknowledge_policies"),
            spec_json: "{}",
            plan_json: None,
            unsupported_policies_json: p.policies,
            acknowledged_policies_json: p.acked_json,
            policies_acknowledged_at: p.acked_at,
            assurance_policy_json: None,
            import_provenance_json: &prov,
            decisions: &SAMPLE_DECISIONS,
            tasks: &SAMPLE_TASKS,
            planned_checks: &SAMPLE_CHECKS,
            check_receipts: &SAMPLE_RECEIPTS,
            ac_evidence: &SAMPLE_ACS,
            historical_approvals: &SAMPLE_APPROVALS,
            historical_review_summary: None,
            historical_review_at: None,
        },
        p.now,
    )?)
}

#[test]
fn cli_import_case_a_policy_ack_and_case_b_policy_preservation() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let (policies_unacked, _) = sample_policy_and_prov("sha256:s1")?;

    let first = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-pol-1",
            digest: "sha256:s1",
            title: "Policy Card",
            policies: &policies_unacked,
            acked_json: "[]",
            acked_at: None,
            now: "2026-11-26T00:00:00Z",
        },
    )?;
    assert_eq!(first.revision, 1);
    assert!(first.cli_import.policies_acknowledged_at.is_none());

    let case_a_ack = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-pol-2",
            digest: "sha256:s1",
            title: "Policy Card",
            policies: &policies_unacked,
            acked_json: "[\"sandbox.requireHardened\"]",
            acked_at: Some("2026-11-26T00:01:00Z"),
            now: "2026-11-26T00:01:00Z",
        },
    )?;
    assert!(!case_a_ack.created && !case_a_ack.reused);
    assert_eq!(case_a_ack.revision, 2);
    assert_eq!(case_a_ack.cli_import.last_imported_revision, 2);
    assert_eq!(
        case_a_ack.cli_import.acknowledged_policies_json,
        "[\"sandbox.requireHardened\"]"
    );
    assert_eq!(
        case_a_ack.cli_import.policies_acknowledged_at.as_deref(),
        Some("2026-11-26T00:01:00Z")
    );

    let (policies_s2, _) = sample_policy_and_prov("sha256:s2")?;
    let case_b_preserve = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-pol-3",
            digest: "sha256:s2",
            title: "Policy Card v2",
            policies: &policies_s2,
            acked_json: "[]",
            acked_at: None,
            now: "2026-11-26T00:02:00Z",
        },
    )?;
    assert_eq!(case_b_preserve.revision, 3);
    assert_eq!(
        case_b_preserve.cli_import.acknowledged_policies_json,
        "[\"sandbox.requireHardened\"]"
    );
    assert_eq!(
        case_b_preserve
            .cli_import
            .policies_acknowledged_at
            .as_deref(),
        Some("2026-11-26T00:01:00Z")
    );

    let case_b_cleared = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-pol-4",
            digest: "sha256:s3",
            title: "Policy Card v3",
            policies: "[]",
            acked_json: "[]",
            acked_at: None,
            now: "2026-11-26T00:03:00Z",
        },
    )?;
    assert_eq!(case_b_cleared.cli_import.acknowledged_policies_json, "[]");
    assert!(case_b_cleared.cli_import.policies_acknowledged_at.is_none());
    Ok(())
}

fn escalation_policies_json(configured_value: &str, blocks: bool) -> String {
    serde_json::json!([
        {
            "key": "sandbox.requireHardened",
            "category": "sandbox",
            "summary": "Hardened sandbox",
            "configuredValue": "true",
            "status": "unsupported",
            "blocksExecution": true,
            "sourcePath": ".legion-cli/config.yaml"
        },
        {
            "key": "mcpHttpToolAllowlist",
            "category": "mcp",
            "summary": "MCP HTTP allowlist",
            "configuredValue": configured_value,
            "status": "unsupported",
            "blocksExecution": blocks,
            "sourcePath": ".legion-cli/config.yaml"
        }
    ])
    .to_string()
}

#[test]
fn cli_import_case_b_rejects_carried_ack_on_policy_escalation_or_value_change() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let policies_non_blocking = escalation_policies_json("[]", false);

    let first = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-esc-1",
            digest: "sha256:esc-1",
            title: "Escalation Card",
            policies: &policies_non_blocking,
            acked_json: "[\"mcpHttpToolAllowlist\",\"sandbox.requireHardened\"]",
            acked_at: Some("2026-11-26T00:00:00Z"),
            now: "2026-11-26T00:00:00Z",
        },
    )?;
    assert_eq!(
        first.cli_import.policies_acknowledged_at.as_deref(),
        Some("2026-11-26T00:00:00Z")
    );

    let policies_escalated = escalation_policies_json("{\"serverA\":[\"read\"]}", true);
    let escalated = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-esc-2",
            digest: "sha256:esc-2",
            title: "Escalation Card",
            policies: &policies_escalated,
            acked_json: "[]",
            acked_at: None,
            now: "2026-11-26T00:01:00Z",
        },
    )?;
    assert_eq!(
        escalated.cli_import.acknowledged_policies_json,
        "[\"sandbox.requireHardened\"]"
    );
    assert!(escalated.cli_import.policies_acknowledged_at.is_none());
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&escalated.cli_import.unsupported_policies_json)?;
    let mcp = parsed
        .iter()
        .find(|p| p["key"] == "mcpHttpToolAllowlist")
        .ok_or("missing mcpHttpToolAllowlist")?;
    assert_eq!(mcp["status"], "unsupported");

    let reacked = run_pol_import(
        conn,
        &PolImportParams {
            rev_id: "rev-esc-3",
            digest: "sha256:esc-3",
            title: "Escalation Card",
            policies: &policies_escalated,
            acked_json: "[\"mcpHttpToolAllowlist\"]",
            acked_at: None,
            now: "2026-11-26T00:02:00Z",
        },
    )?;
    assert_eq!(
        reacked.cli_import.policies_acknowledged_at.as_deref(),
        Some("2026-11-26T00:02:00Z")
    );
    Ok(())
}
