#![forbid(unsafe_code)]
//! Persistence tests for M2 challenge synthesis fields, task limitations/status, and plan approval metadata.

use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::types::{ApproveSpecInput, CreateCardInput};
use legion_persistence::workflow_board_repo::types_m2::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, ApprovePlanInput,
    ChallengeConcernInput, ChallengeStepInput, ExploreAnswerInput, ExploreStepInput, LensInput,
    PlannedCheckInput, SeedDecisionInput, TaskContractInput, UpdatePlanInput,
};
use legion_persistence::workflow_board_repo::{
    approve_plan, approve_spec, challenge_step, create_card, explore_step, get_board_snapshot,
    get_card_planning_detail, seed_exploration_decisions, update_plan,
};
use rusqlite::Connection;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const TS: &str = "2026-11-24T12:00:00Z";

fn seed_card_and_explore(conn: &Connection) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-f2",
            project_id: "proj-m2-pf",
            revision_id: "rev-f2-1",
            title: "Plan Metadata Card",
            brief: "Brief",
            source_type: "idea",
            assistance_mode: "guided",
            intake_goal: Some("greenfield"),
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            wireframes_digest: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Persist metadata\"}]}",
            spec_digest: Some("sha256:spec-f2"),
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
        },
        TS,
    )?;
    seed_exploration_decisions(
        conn,
        "card-f2",
        &[SeedDecisionInput {
            decision_id: "dec-1",
            prompt: "Confirm scope",
            depends_on: &[],
            recommendation: None,
            rationale: None,
        }],
        TS,
    )?;
    explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-f2",
            revision_id: "rev-f2-2",
            expected_revision: 1,
            action: Some("step"),
            continue_round: Some(false),
            answers: &[ExploreAnswerInput {
                decision_id: "dec-1",
                action: "edit",
                answer: Some("Confirmed"),
                prompt: Some("Confirm scope"),
                recommendation: None,
                rationale: None,
            }],
        },
        TS,
    )?;
    Ok(())
}

fn seed_challenge_synthesis(conn: &Connection) -> TestResult {
    let concern = |disp: &'static str, ans: Option<&'static str>| ChallengeConcernInput {
        id: "c-1",
        summary: "Check evidence boundary",
        why_matters: Some("Why matters text"),
        evidence: Some("README.md:1"),
        assumption: Some("No side effects"),
        disposition: disp,
        answer: ans,
        reason: None,
        mutated_answer: None,
    };
    for (rev, ch_id, act, concerns, diff) in [
        (
            2,
            "ch-f2-1",
            "start",
            vec![concern("unresolved", None)],
            None,
        ),
        (
            3,
            "ch-f2-2",
            "resolve",
            vec![concern("answer", Some("Exact human answer"))],
            None,
        ),
        (
            4,
            "ch-f2-3",
            "synthesize",
            Vec::new(),
            Some("{\"diffSummary\":\"Synthesized\"}"),
        ),
    ] {
        challenge_step(
            conn,
            &ChallengeStepInput {
                card_id: "card-f2",
                revision_id: &format!("rev-f2-ch-{rev}"),
                challenge_id: ch_id,
                expected_revision: rev,
                action: act,
                concerns: &concerns,
                acknowledgement_token: None,
                unresolved_concern_ids: &[],
                manual_answers: None,
                spec_digest: Some("sha256:spec-f2"),
                interview_digest: Some("sha256:int-f2"),
                source_refs_digest: None,
                synthesis_diff_json: diff,
                approved: None,
            },
            TS,
        )?;
    }
    Ok(())
}

fn write_m2_plan(conn: &Connection) -> TestResult {
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-f2",
            revision_id: "rev-f2-6",
            approval_id: "apr-spec-f2",
            expected_revision: 5,
            expected_spec_digest: Some("sha256:spec-f2"),
            wireframes_digest: None,
            approved_by: "client",
        },
        TS,
    )?;
    let lenses = [
        ("ux", Some(false), None, Some("Backend only")),
        ("functional", Some(true), Some("Covers AC-01"), None),
        ("architecture", Some(true), Some("Modular"), None),
        ("quality", Some(true), Some("Tested"), None),
        ("infrastructure", Some(false), None, Some("Local only")),
        ("delivery", Some(true), Some("Single PR"), None),
    ]
    .into_iter()
    .map(|(lens, applicable, summary, reason)| LensInput {
        lens,
        applicable,
        summary,
        reason,
    })
    .collect::<Vec<_>>();
    let alts = [
        AlternativeInput {
            id: "a1",
            title: "Alt 1",
            summary: "S1",
            tradeoffs: "T1",
        },
        AlternativeInput {
            id: "a2",
            title: "Alt 2",
            summary: "S2",
            tradeoffs: "T2",
        },
    ];
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    update_plan(
        conn,
        &UpdatePlanInput {
            card_id: "card-f2",
            revision_id: "rev-f2-7",
            expected_revision: 6,
            lenses: Some(&lenses),
            alternatives: Some(&alts),
            selected_alternative_id: AlternativeSelectionUpdate::Set("a1"),
            selection_rationale: Some("Best fit"),
            revise_alternatives_feedback: None,
            strategy: Some("outcomes"),
            strategy_rationale: Some("Deliver AC-01"),
            strategy_metadata_json: None,
            granularity: Some("balanced"),
            tasks: Some(&[TaskContractInput {
                task_id: "TSK-01",
                title: "Task 1",
                depends_on: &[],
                blocks: &[],
                files_allowed: &["src/lib.rs"],
                files_forbidden: &[".env"],
                expected_artifacts: &["src/lib.rs"],
                verification_commands: std::slice::from_ref(&cmd),
                testing_method: "regression-first",
                testing_limitations: Some("No network"),
                status: Some("todo"),
            }]),
            checks: Some(&[PlannedCheckInput {
                check_id: "CHK-01",
                owner_task_id: Some("TSK-01"),
                argv: &cmd,
                label: Some("Run tests"),
            }]),
            ac_mappings: Some(&[AcMappingInput {
                criterion_id: "AC-01",
                method: "automated",
                check_ids: &["CHK-01"],
                command_argv: None,
                procedure: None,
                evidence_ref: None,
                external_source: None,
            }]),
            plan_json: Some("{}"),
            plan_digest: Some("sha256:plan-f2"),
            design_digest: Some("sha256:design-f2"),
            tasks_digest: Some("sha256:tasks-f2"),
        },
        TS,
    )?;
    Ok(())
}

#[test]
fn explore_challenge_synthesis_and_plan_approval_metadata_round_trip() -> TestResult {
    let tmp = TempDir::new()?;
    let store = SqliteStore::open(tmp.path().join("state.sqlite"))?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(tmp.path().to_string_lossy().as_ref())
            .with_project_id("proj-m2-pf")
            .with_title("M2 Plan Fields"),
    )?;
    let conn = store.connection();
    seed_card_and_explore(conn)?;
    seed_challenge_synthesis(conn)?;

    let mid = get_card_planning_detail(conn, "card-f2")?;
    assert_eq!(mid.challenges[0].synthesis_count, 1);
    assert_eq!(
        mid.challenges[0].synthesis_diff_json.as_deref(),
        Some("{\"diffSummary\":\"Synthesized\"}")
    );
    assert!(
        mid.challenges[0]
            .concerns_json
            .contains("Exact human answer")
    );
    assert!(mid.challenges[0].concerns_json.contains("Why matters text"));

    write_m2_plan(conn)?;
    approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-f2",
            revision_id: "rev-f2-8",
            approval_id: "apr-plan-f2",
            expected_revision: 7,
            expected_spec_digest: Some("sha256:spec-f2"),
            expected_plan_digest: Some("sha256:plan-f2"),
            approved_by: "client",
        },
        TS,
    )?;
    let detail = get_card_planning_detail(conn, "card-f2")?;
    assert_eq!(
        detail.card_detail.card.design_digest.as_deref(),
        Some("sha256:design-f2")
    );
    assert!(
        detail
            .card_detail
            .card
            .tasks_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("sha256:"))
    );
    assert_eq!(
        detail.tasks[0].testing_boundary.as_deref(),
        Some("No network")
    );
    assert_eq!(detail.tasks[0].status, "todo");
    let plan_apr = detail
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == "apr-plan-f2")
        .ok_or("missing plan approval")?;
    assert!(plan_apr.metadata_json.contains("checksDigest"));

    let snap = get_board_snapshot(conn, "proj-m2-pf", TS)?;
    assert_eq!(snap.cards[0].plan_approved_revision, Some(8));
    Ok(())
}
