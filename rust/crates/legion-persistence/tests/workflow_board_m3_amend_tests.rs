//! Integration tests for M3 plan/spec amendments (`propose_amendment`), contract-violation
//! staling, and pause-metadata cleanup.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeSelectionUpdate, ApprovePlanInput, ClaimOrStartTaskInput,
    CompleteTaskInput, PauseCardInput, PlannedCheckInput, ProposeAmendmentInput,
    RecordCheckReceiptInput, RecordExternalActionInput, ReturnForRevisionInput, TaskContractInput,
    UpdatePlanInput, WorkflowBoardRepoError, approve_plan, claim_or_start_task, complete_task,
    pause_card, propose_amendment, record_check_receipt, record_external_action,
    return_for_revision, update_plan,
};
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn amendment_stales_contract_violations_and_clears_pause_metadata() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-amend")?;
    let t = "2026-11-24T01:45:00Z";

    let s1 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-amend",
            revision_id: "r-am-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let viol = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-amend",
            revision_id: "r-am-v1",
            expected_revision: s1.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "completed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: None,
            product_snapshot_digest: Some("sha256:snap-am"),
            error_message: None,
            violation_id: Some("viol-am"),
            changed_files: &["src/a.rs", "src/extra.rs"],
            unexpected_files: &["src/extra.rs"],
            forbidden_files: &[],
        },
        t,
    )?;
    // Probe 7.5: pause card then propose_amendment clears pause metadata
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-amend",
            revision_id: "r-am-pause",
            expected_revision: viol.planning.card_detail.card.revision,
            reason: Some("Paused before amend"),
        },
        t,
    )?;
    let amended = propose_amendment(
        conn,
        &ProposeAmendmentInput {
            card_id: "card-amend",
            revision_id: "r-am-prop",
            amendment_id: "am-viol-1",
            expected_revision: paused.planning.card_detail.card.revision,
            target_stage: "plan",
            reason: "Accept extra file",
            summary: None,
            source_task_id: Some("TSK-1"),
            unexpected_files: &["src/extra.rs"],
            proposed_changes_json: "{}",
            proposed_by: "client",
        },
        t,
    )?;
    let card = &amended.planning.card_detail.card;
    assert_eq!(card.status, "active");
    assert_eq!(card.paused_from_status, None);
    assert_eq!(card.pause_reason, None);
    assert_eq!(card.paused_at, None);
    assert_eq!(amended.contract_violations[0].status, "amended");
    replan_and_claim_new_task(conn, card.revision, t)
}

fn replan_and_claim_new_task(
    conn: &rusqlite::Connection,
    card_revision: i64,
    t: &str,
) -> TestResult {
    // Probe 7.2: update_plan + approve_plan allows claiming new task TSK-NEW
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let new_tasks = [TaskContractInput {
        task_id: "TSK-NEW",
        title: "Amended Task",
        depends_on: &[],
        blocks: &[],
        files_allowed: &["src/extra.rs"],
        files_forbidden: &[],
        expected_artifacts: &[],
        verification_commands: &[],
        testing_method: "test-first",
        testing_limitations: None,
        status: None,
    }];
    let new_checks = [PlannedCheckInput {
        check_id: "chk-1",
        owner_task_id: Some("TSK-NEW"),
        argv: &cmd,
        label: Some("Run tests"),
    }];
    let new_acs = [
        AcMappingInput {
            criterion_id: "AC-01",
            method: "automated",
            check_ids: &["chk-1"],
            command_argv: None,
            procedure: None,
            evidence_ref: None,
            external_source: None,
        },
        AcMappingInput {
            criterion_id: "AC-02",
            method: "manual",
            check_ids: &[],
            command_argv: None,
            procedure: Some("Inspect UI"),
            evidence_ref: None,
            external_source: None,
        },
    ];
    let updated = update_plan(
        conn,
        &UpdatePlanInput {
            card_id: "card-amend",
            revision_id: "r-am-upd",
            expected_revision: card_revision,
            lenses: None,
            alternatives: None,
            selected_alternative_id: AlternativeSelectionUpdate::Keep,
            selection_rationale: None,
            revise_alternatives_feedback: None,
            strategy: None,
            strategy_rationale: None,
            strategy_metadata_json: None,
            granularity: None,
            tasks: Some(&new_tasks),
            checks: Some(&new_checks),
            ac_mappings: Some(&new_acs),
            plan_json: None,
            plan_digest: Some("sha256:plan-am-2"),
            design_digest: None,
            tasks_digest: None,
        },
        t,
    )?;
    let reapproved = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-amend",
            revision_id: "r-am-app",
            approval_id: "apr-plan-am-2",
            expected_revision: updated.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    let claimed = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-amend",
            revision_id: "r-am-claim-new",
            expected_revision: reapproved.card_detail.card.revision,
            task_id: "TSK-NEW",
            stage_run_id: None,
        },
        t,
    )?;
    assert_eq!(
        claimed.planning.card_detail.card.current_task_id.as_deref(),
        Some("TSK-NEW")
    );
    Ok(())
}

#[test]
fn propose_amendment_rejects_interrupted_external_actions_probe_812() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-amend-812")?;
    let t = "2026-11-24T02:00:00Z";

    let _act = record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-812",
            card_id: "card-amend-812",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "push-812",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    assert!(matches!(
        propose_amendment(
            conn,
            &ProposeAmendmentInput {
                card_id: "card-amend-812",
                revision_id: "r-am-812",
                amendment_id: "am-812",
                expected_revision: rev,
                target_stage: "plan",
                reason: "Blocked by interrupted action",
                summary: None,
                source_task_id: None,
                unexpected_files: &[],
                proposed_changes_json: "{}",
                proposed_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    Ok(())
}

#[test]
fn return_for_revision_clears_ghost_blocker_and_pause_state_probe_813() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev_blocked = setup_approved_execute_card(conn, "card-ret-blocked")?;
    let t = "2026-11-24T02:10:00Z";
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let failed = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id: "card-ret-blocked",
            revision_id: "r-chk-fail-813",
            receipt_id: "rcpt-fail-813",
            expected_revision: rev_blocked,
            check_id: "chk-1",
            argv: &cmd,
            status: "failed",
            exit_code: Some(1),
            log_ref: None,
            stdout_excerpt: None,
            stderr_excerpt: Some("failed"),
            duration_ms: Some(5),
            snapshot_digest: "sha256:snap-813",
            commit_sha: None,
        },
        t,
    )?;
    assert_eq!(failed.planning.card_detail.card.status, "blocked");
    let ret = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-ret-blocked",
            revision_id: "r-ret-813",
            expected_revision: failed.planning.card_detail.card.revision,
            target_stage: "plan",
            reason: "Revise plan after check failure",
        },
        t,
    )?;
    assert_eq!(ret.card_detail.card.status, "active");
    assert_eq!(ret.card_detail.card.blocker_reason, None);
    assert_eq!(
        ret.card_detail.card.next_action.as_deref(),
        Some("revise_plan")
    );
    let updated = update_plan(
        conn,
        &UpdatePlanInput {
            card_id: "card-ret-blocked",
            revision_id: "r-upd-813",
            expected_revision: ret.card_detail.card.revision,
            lenses: None,
            alternatives: None,
            selected_alternative_id: AlternativeSelectionUpdate::Keep,
            selection_rationale: None,
            revise_alternatives_feedback: None,
            strategy: None,
            strategy_rationale: None,
            strategy_metadata_json: None,
            granularity: None,
            tasks: None,
            checks: None,
            ac_mappings: None,
            plan_json: None,
            plan_digest: Some("sha256:plan-813-v2"),
            design_digest: None,
            tasks_digest: None,
        },
        t,
    )?;
    let approved = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-ret-blocked",
            revision_id: "r-app-813",
            approval_id: "apr-plan-813-v2",
            expected_revision: updated.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(approved.card_detail.card.status, "active");
    assert_eq!(approved.card_detail.card.blocker_reason, None);
    verify_return_paused_card_clears_pause_state(conn, t)
}

fn verify_return_paused_card_clears_pause_state(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    let rev_paused = setup_approved_execute_card(conn, "card-ret-paused")?;
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-ret-paused",
            revision_id: "r-pause-813",
            expected_revision: rev_paused,
            reason: Some("Pause before return"),
        },
        t,
    )?;
    let returned_paused = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-ret-paused",
            revision_id: "r-ret-paused-813",
            expected_revision: paused.planning.card_detail.card.revision,
            target_stage: "spec",
            reason: "Return paused card to spec",
        },
        t,
    )?;
    assert_eq!(returned_paused.card_detail.card.status, "active");
    assert_eq!(returned_paused.card_detail.card.paused_from_status, None);
    assert_eq!(returned_paused.card_detail.card.pause_reason, None);
    assert_eq!(returned_paused.card_detail.card.paused_at, None);
    assert_eq!(
        returned_paused.card_detail.card.next_action.as_deref(),
        Some("revise_spec")
    );
    Ok(())
}
