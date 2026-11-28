//! Shared M2 integration test fixtures for `workflow_board_m2_repo_tests`.

use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, ApproveSpecInput,
    CreateCardInput, LensInput, PlannedCheckInput, TaskContractInput, UpdatePlanInput,
    approve_spec, create_card, update_plan,
};
use rusqlite::Connection;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub const SIX_LENSES: &[LensInput<'static>] = &[
    LensInput {
        lens: "ux",
        applicable: Some(false),
        summary: None,
        reason: Some("Backend only"),
    },
    LensInput {
        lens: "functional",
        applicable: Some(true),
        summary: Some("CRUD + CAS"),
        reason: None,
    },
    LensInput {
        lens: "architecture",
        applicable: Some(true),
        summary: Some("SQLite v112"),
        reason: None,
    },
    LensInput {
        lens: "quality",
        applicable: Some(true),
        summary: Some("Unit + prop tests"),
        reason: None,
    },
    LensInput {
        lens: "infrastructure",
        applicable: Some(true),
        summary: Some("Local WAL DB"),
        reason: None,
    },
    LensInput {
        lens: "delivery",
        applicable: Some(true),
        summary: Some("Phased rollout"),
        reason: None,
    },
];

pub const TWO_ALTS: &[AlternativeInput<'static>] = &[
    AlternativeInput {
        id: "alt-a",
        title: "Normalized",
        summary: "7 tables",
        tradeoffs: "More DDL",
    },
    AlternativeInput {
        id: "alt-b",
        title: "Blob",
        summary: "1 JSON col",
        tradeoffs: "Coarse updates",
    },
];

pub fn seed_m2_card(conn: &Connection, card_id: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: Box::leak(format!("rev-{card_id}-1").into_boxed_str()),
            project_id: "proj-1",
            title: "M2 Feature",
            brief: "Initial brief",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "guided",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Persists\"},{\"id\":\"AC-02\",\"text\":\"Validates\"}]}",
            spec_digest: Some("sha256:spec-1"),
            wireframes_digest: None,
        },
        "2026-11-24T00:00:00Z",
    )?;
    Ok(())
}

pub fn seed_ready_plan(conn: &Connection, card_id: &str) -> TestResult {
    seed_m2_card(conn, card_id)?;
    let rev2 = format!("rev-{card_id}-2");
    let apr_spec = format!("apr-spec-{card_id}");
    let rev3 = format!("rev-{card_id}-3");
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id,
            revision_id: &rev2,
            approval_id: &apr_spec,
            expected_revision: 1,
            expected_spec_digest: Some("sha256:spec-1"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:02:00Z",
    )?;

    let cmd1 = vec!["cargo".to_owned(), "test".to_owned()];
    let partial = update_plan(
        conn,
        &UpdatePlanInput {
            card_id,
            revision_id: &rev3,
            expected_revision: 2,
            lenses: Some(SIX_LENSES),
            alternatives: Some(TWO_ALTS),
            selected_alternative_id: AlternativeSelectionUpdate::Set("alt-a"),
            selection_rationale: Some("Normalized tables fit M3"),
            revise_alternatives_feedback: None,
            strategy: Some("outcomes"),
            strategy_rationale: None,
            strategy_metadata_json: None,
            granularity: Some("balanced"),
            tasks: Some(&[TaskContractInput {
                task_id: "TSK-0001",
                title: "Implement",
                depends_on: &[],
                blocks: &[],
                files_allowed: &["src/lib.rs"],
                files_forbidden: &[".env"],
                expected_artifacts: &[],
                verification_commands: std::slice::from_ref(&cmd1),
                testing_method: "test-first",
                testing_limitations: None,
                status: None,
            }]),
            checks: Some(&[PlannedCheckInput {
                check_id: "CHK-01",
                owner_task_id: Some("TSK-0001"),
                argv: &cmd1,
                label: Some("Cargo test"),
            }]),
            ac_mappings: Some(&[
                AcMappingInput {
                    criterion_id: "AC-01",
                    method: "automated",
                    check_ids: &["CHK-01"],
                    command_argv: Some(&cmd1),
                    procedure: None,
                    evidence_ref: None,
                    external_source: None,
                },
                AcMappingInput {
                    criterion_id: "AC-02",
                    method: "manual",
                    check_ids: &[],
                    command_argv: None,
                    procedure: Some("Check UI"),
                    evidence_ref: None,
                    external_source: None,
                },
            ]),
            plan_json: Some("{\"ready\":true}"),
            plan_digest: Some("sha256:plan-v1"),
            design_digest: None,
            tasks_digest: None,
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert_eq!(partial.card_detail.card.revision, 3);
    assert!(
        partial
            .card_detail
            .approvals
            .iter()
            .any(|a| a.gate == "spec" && a.status == "valid")
    );
    Ok(())
}

pub fn empty_plan_update<'a>(
    card_id: &'a str,
    revision_id: &'a str,
    expected_revision: i64,
) -> UpdatePlanInput<'a> {
    UpdatePlanInput {
        card_id,
        revision_id,
        expected_revision,
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
        plan_digest: None,
        design_digest: None,
        tasks_digest: None,
    }
}
