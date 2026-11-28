//! Integration tests for M2 `update_plan` validation & `approve_plan` readiness guards.

mod workflow_board_m2_support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, ApprovePlanInput,
    ApproveSpecInput, CreateCardInput, PlannedCheckInput, TaskContractInput, UpdatePlanInput,
    WorkflowBoardRepoError, approve_plan, approve_spec, create_card, update_plan,
};
use workflow_board_m2_support::{
    SIX_LENSES, TWO_ALTS, TestResult, empty_plan_update, seed_m2_card, seed_ready_plan,
};

#[test]
fn update_plan_rejects_cyclic_blocks_dup_checks_unknown_check_ref_and_invalid_strategy()
-> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-val")?;
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];

    assert!(matches!(
        update_plan(
            conn,
            &UpdatePlanInput {
                tasks: Some(&[
                    TaskContractInput {
                        task_id: "T1",
                        title: "A",
                        depends_on: &[],
                        blocks: &["T2"],
                        files_allowed: &["a.rs"],
                        files_forbidden: &[],
                        expected_artifacts: &[],
                        verification_commands: &[],
                        testing_method: "test-first",
                        testing_limitations: None,
                        status: None,
                    },
                    TaskContractInput {
                        task_id: "T2",
                        title: "B",
                        depends_on: &[],
                        blocks: &["T1"],
                        files_allowed: &["b.rs"],
                        files_forbidden: &[],
                        expected_artifacts: &[],
                        verification_commands: &[],
                        testing_method: "test-first",
                        testing_limitations: None,
                        status: None,
                    },
                ]),
                ..empty_plan_update("card-val", "rev-cyc", 3)
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));

    assert!(matches!(
        update_plan(
            conn,
            &UpdatePlanInput {
                checks: Some(&[
                    PlannedCheckInput {
                        check_id: "CHK-DUP",
                        owner_task_id: None,
                        argv: &cmd,
                        label: None
                    },
                    PlannedCheckInput {
                        check_id: "CHK-DUP",
                        owner_task_id: None,
                        argv: &cmd,
                        label: None
                    },
                ]),
                ..empty_plan_update("card-val", "rev-dup", 3)
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));

    assert!(matches!(
        update_plan(
            conn,
            &UpdatePlanInput {
                ac_mappings: Some(&[AcMappingInput {
                    criterion_id: "AC-01",
                    method: "automated",
                    check_ids: &["CHK-NOPE"],
                    command_argv: None,
                    procedure: None,
                    evidence_ref: None,
                    external_source: None,
                }]),
                ..empty_plan_update("card-val", "rev-chk", 3)
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));

    assert!(matches!(
        update_plan(
            conn,
            &UpdatePlanInput {
                strategy: Some("risk-first"),
                ..empty_plan_update("card-val", "rev-strat", 3)
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));
    Ok(())
}

#[test]
fn update_plan_rejects_invalid_granularity_with_invalid_input() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-gran")?;
    assert!(matches!(
        update_plan(
            conn,
            &UpdatePlanInput {
                granularity: Some("invalid-granularity"),
                ..empty_plan_update("card-gran", "rev-gran", 3)
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));
    Ok(())
}

#[test]
fn update_plan_clears_stale_selection_and_approve_plan_requires_plan_stage() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-stg")?;

    let replaced = update_plan(
        conn,
        &UpdatePlanInput {
            alternatives: Some(&[
                AlternativeInput {
                    id: "alt-c",
                    title: "C",
                    summary: "New C",
                    tradeoffs: "TC",
                },
                AlternativeInput {
                    id: "alt-d",
                    title: "D",
                    summary: "New D",
                    tradeoffs: "TD",
                },
            ]),
            ..empty_plan_update("card-stg", "rev-alt", 3)
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(
        replaced
            .plan_design
            .as_ref()
            .and_then(|d| d.selected_alternative_id.as_deref()),
        None
    );

    update_plan(
        conn,
        &UpdatePlanInput {
            selected_alternative_id: AlternativeSelectionUpdate::Set("alt-c"),
            selection_rationale: Some("Pick C"),
            ..empty_plan_update("card-stg", "rev-sel", 4)
        },
        "2026-11-24T00:05:00Z",
    )?;
    approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-stg",
            revision_id: "rev-apr1",
            approval_id: "apr-1",
            expected_revision: 5,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:06:00Z",
    )?;
    assert!(matches!(
        approve_plan(
            conn,
            &ApprovePlanInput {
                card_id: "card-stg",
                revision_id: "rev-apr2",
                approval_id: "apr-2",
                expected_revision: 6,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            "2026-11-24T00:07:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}

#[test]
fn approve_plan_rejects_empty_spec_ac_ids_and_invalid_persisted_strategy() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_m2_card(conn, "card-noac-seed")?;
    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-noac",
            revision_id: "rev-noac-1",
            project_id: "proj-1",
            title: "No AC",
            brief: "Brief",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "direct",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{}",
            spec_digest: Some("sha256:noac"),
            wireframes_digest: None,
        },
        "2026-11-24T00:00:00Z",
    )?;
    let approve_input = ApproveSpecInput {
        card_id: "card-noac",
        revision_id: "rev-noac-2",
        approval_id: "apr-noac",
        expected_revision: 1,
        expected_spec_digest: None,
        wireframes_digest: None,
        approved_by: "client",
    };
    assert!(matches!(
        approve_spec(conn, &approve_input, "2026-11-24T00:01:00Z"),
        Err(WorkflowBoardRepoError::PreconditionFailed(message))
            if message == "spec approval requires at least one acceptance criterion"
    ));
    let unchanged = legion_persistence::workflow_board_repo::get_card_detail(conn, "card-noac")?;
    assert_eq!(unchanged.card.revision, 1);
    assert!(unchanged.approvals.is_empty());
    conn.execute(
        "UPDATE workflow_board_cards SET spec_json = ?1 WHERE card_id = 'card-noac'",
        [r#"{"acceptanceCriteria":[{"id":"AC-1","text":"Original criterion"}]}"#],
    )?;
    approve_spec(conn, &approve_input, "2026-11-24T00:01:00Z")?;
    conn.execute(
        "UPDATE workflow_board_cards SET spec_json = '{}' WHERE card_id = 'card-noac'",
        [],
    )?;
    update_plan(
        conn,
        &UpdatePlanInput {
            lenses: Some(SIX_LENSES),
            alternatives: Some(TWO_ALTS),
            selected_alternative_id: AlternativeSelectionUpdate::Set("alt-a"),
            selection_rationale: Some("Pick A"),
            tasks: Some(&[TaskContractInput {
                task_id: "TSK-1",
                title: "T",
                depends_on: &[],
                blocks: &[],
                files_allowed: &["src/x.rs"],
                files_forbidden: &[],
                expected_artifacts: &[],
                verification_commands: &[],
                testing_method: "test-first",
                testing_limitations: None,
                status: None,
            }]),
            ..empty_plan_update("card-noac", "rev-noac-3", 2)
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert!(matches!(
        approve_plan(
            conn,
            &ApprovePlanInput {
                card_id: "card-noac",
                revision_id: "rev-noac-4",
                approval_id: "apr-plan-noac",
                expected_revision: 3,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            "2026-11-24T00:03:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    Ok(())
}

#[test]
fn approve_plan_rejects_invalid_persisted_strategy() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_ready_plan(conn, "card-badstrat")?;
    conn.execute(
        "UPDATE workflow_board_plan_designs SET strategy = 'custom', strategy_rationale = NULL WHERE card_id = 'card-badstrat'",
        [],
    )?;
    assert!(matches!(
        approve_plan(
            conn,
            &ApprovePlanInput {
                card_id: "card-badstrat",
                revision_id: "rev-badstrat",
                approval_id: "apr-badstrat",
                expected_revision: 3,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            "2026-11-24T00:04:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    Ok(())
}
