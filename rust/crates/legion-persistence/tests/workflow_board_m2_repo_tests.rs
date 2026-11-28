//! Integration tests for M2 `workflow_board_repo`: plan authoring, approval binding,
//! return for revision, and targeted invalidation.

mod workflow_board_m2_support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AcMappingInput, ApprovePlanInput, ApproveSpecInput, ReturnForRevisionInput, UpdatePlanInput,
    UpdateSpecInput, WorkflowBoardRepoError, approve_plan, approve_spec, return_for_revision,
    update_plan, update_spec,
};
use workflow_board_m2_support::{
    SIX_LENSES, TWO_ALTS, TestResult, empty_plan_update, seed_m2_card, seed_ready_plan,
};

#[test]
fn plan_rejects_unapproved_spec_and_shell_wrapper_commands() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_m2_card(conn, "card-guard")?;

    assert!(matches!(
        approve_plan(
            conn,
            &ApprovePlanInput {
                card_id: "card-guard",
                revision_id: "rev-no-spec",
                approval_id: "apr-no-spec",
                expected_revision: 1,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            "2026-11-24T00:01:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let sh_wrap = vec!["sh".to_owned(), "-c".to_owned(), "echo bad".to_owned()];
    assert!(
        update_plan(
            conn,
            &UpdatePlanInput {
                card_id: "card-guard",
                revision_id: "rev-bad-sh",
                expected_revision: 1,
                lenses: Some(SIX_LENSES),
                alternatives: Some(TWO_ALTS),
                ac_mappings: Some(&[AcMappingInput {
                    criterion_id: "AC-01",
                    method: "automated",
                    check_ids: &[],
                    command_argv: Some(&sh_wrap),
                    procedure: None,
                    evidence_ref: None,
                    external_source: None,
                }]),
                ..empty_plan_update("card-guard", "rev-bad-sh", 1)
            },
            "2026-11-24T00:02:00Z",
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn plan_approval_binds_digests_and_update_plan_stales_only_downstream() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-plan")?;

    let approved_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-plan",
            revision_id: "rev-4",
            approval_id: "apr-plan-1",
            expected_revision: 3,
            expected_spec_digest: Some("sha256:spec-1"),
            expected_plan_digest: Some("sha256:plan-v1"),
            approved_by: "client",
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(approved_plan.card_detail.card.stage, "execute");
    let plan_apr = approved_plan
        .card_detail
        .approvals
        .iter()
        .find(|a| a.gate == "plan" && a.status == "valid")
        .ok_or("missing valid plan approval")?;
    assert!(plan_apr.metadata_json.contains("TSK-0001"));
    assert!(plan_apr.metadata_json.contains("AC-02"));

    conn.execute(
        "UPDATE workflow_board_planned_checks SET status = 'passed' WHERE card_id = 'card-plan'",
        [],
    )?;
    conn.execute(
        "UPDATE workflow_board_ac_evidence SET status = 'pass' WHERE card_id = 'card-plan'",
        [],
    )?;

    let plan_edit = update_plan(
        conn,
        &UpdatePlanInput {
            granularity: Some("fine"),
            plan_digest: Some("sha256:plan-v2"),
            ..empty_plan_update("card-plan", "rev-5", 4)
        },
        "2026-11-24T00:05:00Z",
    )?;
    assert!(
        plan_edit
            .card_detail
            .approvals
            .iter()
            .any(|a| a.gate == "spec" && a.status == "valid")
    );
    assert!(
        plan_edit
            .card_detail
            .approvals
            .iter()
            .any(|a| a.gate == "plan" && a.status == "stale")
    );
    assert_eq!(plan_edit.planned_checks[0].status, "stale");
    assert_eq!(plan_edit.ac_evidence[0].status, "stale");
    Ok(())
}

#[test]
fn return_for_revision_preserves_history_and_stales_targeted_approvals() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-ret")?;

    approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-ret",
            revision_id: "rev-4",
            approval_id: "apr-plan-1",
            expected_revision: 3,
            expected_spec_digest: Some("sha256:spec-1"),
            expected_plan_digest: Some("sha256:plan-v1"),
            approved_by: "client",
        },
        "2026-11-24T00:04:00Z",
    )?;

    let returned = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-ret",
            revision_id: "rev-5",
            expected_revision: 4,
            target_stage: "spec",
            reason: "Clarify AC-02 boundary",
        },
        "2026-11-24T00:05:00Z",
    )?;
    assert_eq!(returned.card_detail.card.stage, "spec");
    assert_eq!(returned.card_detail.revisions.len(), 5);
    assert!(
        returned
            .card_detail
            .approvals
            .iter()
            .all(|a| a.status == "stale")
    );

    let spec_edit = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-ret",
            revision_id: "rev-6",
            expected_revision: 5,
            title: None,
            brief: Some("Revised spec brief"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:06:00Z",
    )?;
    assert_eq!(spec_edit.card.revision, 6);
    Ok(())
}

#[test]
fn update_spec_in_execute_regresses_to_spec_and_recovers_to_execute() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-spec-regress")?;

    let in_execute = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-spec-regress",
            revision_id: "rev-4",
            approval_id: "apr-plan-1",
            expected_revision: 3,
            expected_spec_digest: Some("sha256:spec-1"),
            expected_plan_digest: Some("sha256:plan-v1"),
            approved_by: "client",
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(in_execute.card_detail.card.stage, "execute");

    let regressed = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-spec-regress",
            revision_id: "rev-5",
            expected_revision: 4,
            title: None,
            brief: Some("Updated brief from execute stage"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:05:00Z",
    )?;
    assert_eq!(regressed.card.stage, "spec");
    assert_eq!(regressed.card.next_action.as_deref(), Some("revise_spec"));
    assert!(regressed.approvals.iter().all(|a| a.status == "stale"));

    let blocked_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-spec-regress",
            revision_id: "rev-blocked",
            approval_id: "apr-blocked",
            expected_revision: 5,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:05:30Z",
    );
    assert!(matches!(
        blocked_plan,
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let reapproved_spec = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-spec-regress",
            revision_id: "rev-6",
            approval_id: "apr-spec-2",
            expected_revision: 5,
            expected_spec_digest: Some("sha256:spec-v2"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:06:00Z",
    )?;
    assert_eq!(reapproved_spec.card.stage, "plan");

    let reapproved_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-spec-regress",
            revision_id: "rev-7",
            approval_id: "apr-plan-2",
            expected_revision: 6,
            expected_spec_digest: Some("sha256:spec-v2"),
            expected_plan_digest: Some("sha256:plan-v1"),
            approved_by: "client",
        },
        "2026-11-24T00:07:00Z",
    )?;
    assert_eq!(reapproved_plan.card_detail.card.stage, "execute");
    assert!(
        reapproved_plan
            .card_detail
            .approvals
            .iter()
            .any(|a| a.gate == "spec" && a.status == "valid")
    );
    assert!(
        reapproved_plan
            .card_detail
            .approvals
            .iter()
            .any(|a| a.gate == "plan" && a.status == "valid")
    );
    Ok(())
}
