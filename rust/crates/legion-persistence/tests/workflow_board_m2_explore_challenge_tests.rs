//! Integration tests for M2 `workflow_board_repo` guided exploration & bounded challenge.

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ChallengeConcernInput, ChallengeStepInput, CreateCardInput, ExploreAnswerInput,
    ExploreStepInput, WorkflowBoardRepoError, challenge_step, create_card, explore_step,
    validate_spec_approval_m2_readiness,
};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn seed_card(conn: &Connection, card_id: &str) -> TestResult {
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
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Persists\"}]}",
            spec_digest: Some("sha256:spec-1"),
            wireframes_digest: None,
        },
        "2026-11-24T00:00:00Z",
    )?;
    Ok(())
}

fn ans(
    decision_id: &'static str,
    action: &'static str,
    answer: Option<&'static str>,
) -> ExploreAnswerInput<'static> {
    ExploreAnswerInput {
        decision_id,
        action,
        answer,
        prompt: None,
        recommendation: None,
        rationale: None,
    }
}

#[test]
fn guided_exploration_round_cap_continue_gate_pause_and_cas_race() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-exp")?;

    assert!(matches!(
        explore_step(
            conn,
            &ExploreStepInput {
                card_id: "card-exp",
                revision_id: "rev-bad",
                expected_revision: 1,
                action: None,
                continue_round: None,
                answers: &[
                    ans("D1", "edit", Some("A")),
                    ans("D2", "edit", Some("B")),
                    ans("D3", "edit", Some("C")),
                    ans("D4", "edit", Some("D")),
                ],
            },
            "2026-11-24T00:01:00Z",
        ),
        Err(WorkflowBoardRepoError::InvalidInput(_))
    ));

    let paused = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-exp",
            revision_id: "rev-2",
            expected_revision: 1,
            action: Some("pause"),
            continue_round: None,
            answers: &[ans("D1", "edit", Some("Use WAL")), ans("D2", "pause", None)],
        },
        "2026-11-24T00:01:00Z",
    )?;
    assert_eq!(paused.card_detail.card.revision, 2);
    assert_eq!(paused.card_detail.card.status, "paused");
    assert_eq!(paused.card_detail.approvals.len(), 0);
    assert!(
        validate_spec_approval_m2_readiness(conn, "card-exp", &paused.card_detail.card.spec_json)
            .is_err()
    );

    let resolved = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-exp",
            revision_id: "rev-3",
            expected_revision: 2,
            action: Some("resume"),
            continue_round: None,
            answers: &[ans("D2", "edit", Some("In-memory LRU"))],
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert_eq!(resolved.card_detail.card.status, "active");

    assert!(matches!(
        explore_step(
            conn,
            &ExploreStepInput {
                card_id: "card-exp",
                revision_id: "rev-gate",
                expected_revision: 3,
                action: None,
                continue_round: None,
                answers: &[ans("D3", "edit", Some("X"))],
            },
            "2026-11-24T00:03:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let r2 = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-exp",
            revision_id: "rev-4",
            expected_revision: 3,
            action: None,
            continue_round: Some(true),
            answers: &[ans("D3", "edit", Some("X"))],
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(r2.exploration_rounds.len(), 2);
    assert!(matches!(
        explore_step(
            conn,
            &ExploreStepInput {
                card_id: "card-exp",
                revision_id: "rev-stale",
                expected_revision: 3,
                action: None,
                continue_round: Some(true),
                answers: &[ans("D4", "edit", Some("Y"))],
            },
            "2026-11-24T00:05:00Z",
        ),
        Err(WorkflowBoardRepoError::RevisionConflict { .. })
    ));
    validate_spec_approval_m2_readiness(conn, "card-exp", &r2.card_detail.card.spec_json)?;
    Ok(())
}

fn seed_synthesized_challenge(conn: &rusqlite::Connection, card_id: &str) -> TestResult {
    seed_card(conn, card_id)?;
    let first = challenge_step(
        conn,
        &ChallengeStepInput {
            card_id,
            revision_id: "rev-2",
            challenge_id: "ch-1",
            expected_revision: 1,
            action: "synthesize",
            concerns: &[ChallengeConcernInput {
                id: "C1",
                summary: "Preserve FK",
                why_matters: None,
                evidence: None,
                assumption: None,
                disposition: "answer",
                answer: Some("Verbatim human constraint"),
                reason: None,
                mutated_answer: None,
            }],
            acknowledgement_token: None,
            unresolved_concern_ids: &[],
            manual_answers: None,
            spec_digest: Some("sha256:spec-1"),
            interview_digest: Some("sha256:int-1"),
            source_refs_digest: None,
            synthesis_diff_json: None,
            approved: None,
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert_eq!(first.challenges[0].synthesis_count, 1);
    Ok(())
}

#[test]
fn challenge_blocks_self_approval_and_second_synthesis_pass() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_synthesized_challenge(conn, "card-ch")?;

    assert!(
        challenge_step(
            conn,
            &ChallengeStepInput {
                card_id: "card-ch",
                revision_id: "r-self",
                challenge_id: "ch-1",
                expected_revision: 2,
                action: "complete",
                concerns: &[],
                acknowledgement_token: None,
                unresolved_concern_ids: &[],
                manual_answers: None,
                spec_digest: None,
                interview_digest: None,
                source_refs_digest: None,
                synthesis_diff_json: None,
                approved: Some(true),
            },
            "2026-11-24T00:02:30Z",
        )
        .is_err()
    );

    assert!(matches!(
        challenge_step(
            conn,
            &ChallengeStepInput {
                card_id: "card-ch",
                revision_id: "rev-3",
                challenge_id: "ch-1",
                expected_revision: 2,
                action: "synthesize",
                concerns: &[],
                acknowledgement_token: None,
                unresolved_concern_ids: &[],
                manual_answers: None,
                spec_digest: None,
                interview_digest: None,
                source_refs_digest: None,
                synthesis_diff_json: None,
                approved: None,
            },
            "2026-11-24T00:03:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    Ok(())
}

#[test]
fn challenge_manual_fallback_requires_zero_unresolved_and_three_answers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_synthesized_challenge(conn, "card-fb")?;

    assert!(
        challenge_step(
            conn,
            &ChallengeStepInput {
                card_id: "card-fb",
                revision_id: "rev-3b",
                challenge_id: "ch-1",
                expected_revision: 2,
                action: "manualFallback",
                concerns: &[],
                acknowledgement_token: Some("I acknowledge"),
                unresolved_concern_ids: &["C99"],
                manual_answers: Some(&["a", "b", "c"]),
                spec_digest: None,
                interview_digest: None,
                source_refs_digest: None,
                synthesis_diff_json: None,
                approved: None,
            },
            "2026-11-24T00:03:00Z",
        )
        .is_err()
    );

    let fb = challenge_step(
        conn,
        &ChallengeStepInput {
            card_id: "card-fb",
            revision_id: "rev-4",
            challenge_id: "ch-1",
            expected_revision: 2,
            action: "manualFallback",
            concerns: &[],
            acknowledgement_token: Some("I acknowledge"),
            unresolved_concern_ids: &[],
            manual_answers: Some(&["Measurable", "Rollback", "Compatible"]),
            spec_digest: None,
            interview_digest: None,
            source_refs_digest: None,
            synthesis_diff_json: None,
            approved: None,
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(fb.challenges[0].status, "manual_fallback");
    Ok(())
}
