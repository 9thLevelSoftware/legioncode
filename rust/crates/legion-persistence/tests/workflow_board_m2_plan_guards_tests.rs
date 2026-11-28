//! Integration tests for M2 `tasks_digest` aggregation and contract binding.

mod workflow_board_m2_support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeSelectionUpdate, ApprovePlanInput, ApproveSpecInput,
    PlannedCheckInput, ReturnForRevisionInput, TaskContractInput, UpdatePlanInput, approve_plan,
    approve_spec, return_for_revision, update_plan,
};
use workflow_board_m2_support::{
    SIX_LENSES, TWO_ALTS, TestResult, empty_plan_update, seed_m2_card, seed_ready_plan,
};

fn two_task_contracts<'a>(
    first_allowed: &'a [&'a str],
    cmd: &'a [Vec<String>],
) -> [TaskContractInput<'a>; 2] {
    [
        TaskContractInput {
            task_id: "TSK-0001",
            title: "One",
            depends_on: &[],
            blocks: &["TSK-0002"],
            files_allowed: first_allowed,
            files_forbidden: &[],
            expected_artifacts: &[],
            verification_commands: cmd,
            testing_method: "test-first",
            testing_limitations: None,
            status: None,
        },
        TaskContractInput {
            task_id: "TSK-0002",
            title: "Two",
            depends_on: &["TSK-0001"],
            blocks: &[],
            files_allowed: &["src/b.rs"],
            files_forbidden: &[],
            expected_artifacts: &[],
            verification_commands: cmd,
            testing_method: "regression-first",
            testing_limitations: None,
            status: None,
        },
    ]
}

fn seed_two_task_plan(
    conn: &rusqlite::Connection,
    card_id: &str,
    cmd: &[Vec<String>],
) -> TestResult {
    seed_m2_card(conn, card_id)?;
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id,
            revision_id: "rev-2",
            approval_id: "apr-spec-1",
            expected_revision: 1,
            expected_spec_digest: Some("sha256:spec-1"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:01:00Z",
    )?;
    let tasks = two_task_contracts(&["src/a.rs"], cmd);
    update_plan(
        conn,
        &UpdatePlanInput {
            lenses: Some(SIX_LENSES),
            alternatives: Some(TWO_ALTS),
            selected_alternative_id: AlternativeSelectionUpdate::Set("alt-a"),
            selection_rationale: Some("Normalized"),
            strategy: Some("outcomes"),
            granularity: Some("balanced"),
            tasks: Some(&tasks),
            checks: Some(&[PlannedCheckInput {
                check_id: "CHK-01",
                owner_task_id: Some("TSK-0001"),
                argv: &cmd[0],
                label: None,
            }]),
            ac_mappings: Some(&[
                AcMappingInput {
                    criterion_id: "AC-01",
                    method: "automated",
                    check_ids: &["CHK-01"],
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
                    procedure: Some("Manual check"),
                    evidence_ref: None,
                    external_source: None,
                },
            ]),
            ..empty_plan_update(card_id, "rev-3", 2)
        },
        "2026-11-24T00:02:00Z",
    )?;
    Ok(())
}

#[test]
fn partial_update_plan_preserves_aggregate_tasks_digest() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let cmds = vec![vec!["cargo".to_owned(), "test".to_owned()]];
    seed_two_task_plan(conn, "card-dig", &cmds)?;

    let card_tasks_digest = |cid: &str| -> Result<Option<String>, rusqlite::Error> {
        conn.query_row(
            "SELECT tasks_digest FROM workflow_board_cards WHERE card_id = ?1",
            [cid],
            |r| r.get(0),
        )
    };
    let agg1 = card_tasks_digest("card-dig")?.ok_or("tasks_digest")?;
    let first_contract: Option<String> = conn.query_row(
        "SELECT contract_digest FROM workflow_board_tasks WHERE card_id = 'card-dig' AND task_id = 'TSK-0001'",
        [],
        |r| r.get(0),
    )?;
    assert_ne!(Some(agg1.as_str()), first_contract.as_deref());

    let p2 = update_plan(
        conn,
        &UpdatePlanInput {
            granularity: Some("fine"),
            ..empty_plan_update("card-dig", "rev-4", 3)
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert_eq!(p2.card_detail.card.revision, 4);
    assert_eq!(
        card_tasks_digest("card-dig")?.as_deref(),
        Some(agg1.as_str())
    );
    Ok(())
}

#[test]
fn approve_plan_tasks_digest_changes_when_files_allowed_changes() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let cmds = vec![vec!["cargo".to_owned(), "test".to_owned()]];
    seed_two_task_plan(conn, "card-dig2", &cmds)?;

    let a1 = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-dig2",
            revision_id: "rev-4",
            approval_id: "apr-plan-1",
            expected_revision: 3,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:03:00Z",
    )?;
    let d_apr1 = a1
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == "apr-plan-1")
        .ok_or("apr1")?
        .tasks_digest
        .clone();

    return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-dig2",
            revision_id: "rev-5",
            expected_revision: 4,
            target_stage: "plan",
            reason: "Adjust scope",
        },
        "2026-11-24T00:04:00Z",
    )?;
    let edited_tasks = two_task_contracts(&["src/renamed.rs"], &cmds);
    update_plan(
        conn,
        &UpdatePlanInput {
            tasks: Some(&edited_tasks),
            ..empty_plan_update("card-dig2", "rev-6", 5)
        },
        "2026-11-24T00:05:00Z",
    )?;
    let a2 = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-dig2",
            revision_id: "rev-7",
            approval_id: "apr-plan-2",
            expected_revision: 6,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:06:00Z",
    )?;
    let d_apr2 = a2
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == "apr-plan-2")
        .ok_or("apr2")?
        .tasks_digest
        .clone();
    assert_ne!(d_apr1, d_apr2);
    let store2 = SqliteStore::open_in_memory()?;
    seed_ready_plan(store2.connection(), "card-ready-smoke")?;
    Ok(())
}
