//! Integration tests for M2 `workflow_board_repo` guided exploration & bounded challenge guards.

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ChallengeConcernInput, ChallengeStepInput, CreateCardInput, ExploreAnswerInput,
    ExploreStepInput, UpdateSpecInput, WorkflowBoardRepoError, challenge_step, create_card,
    explore_step, update_spec, validate_spec_approval_m2_readiness,
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

fn ch_in<'a>(
    card_id: &'a str,
    revision_id: &'a str,
    expected_revision: i64,
    action: &'a str,
    concerns: &'a [ChallengeConcernInput<'a>],
) -> ChallengeStepInput<'a> {
    ChallengeStepInput {
        card_id,
        revision_id,
        challenge_id: "ch-1",
        expected_revision,
        action,
        concerns,
        acknowledgement_token: None,
        unresolved_concern_ids: &[],
        manual_answers: None,
        spec_digest: None,
        interview_digest: None,
        source_refs_digest: None,
        synthesis_diff_json: None,
        approved: None,
    }
}

#[test]
fn explore_pause_resume_preserves_pending_continue_and_round1_decisions() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-pr")?;

    let r1 = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-pr",
            revision_id: "rev-2",
            expected_revision: 1,
            action: None,
            continue_round: None,
            answers: &[
                ans("D1", "edit", Some("A")),
                ans("D2", "edit", Some("B")),
                ans("D3", "edit", Some("C")),
            ],
        },
        "2026-11-24T00:01:00Z",
    )?;
    assert!(r1.exploration_rounds[0].pending_continue_prompt);

    let paused = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-pr",
            revision_id: "rev-3",
            expected_revision: 2,
            action: Some("pause"),
            continue_round: None,
            answers: &[],
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert!(paused.exploration_rounds[0].pending_continue_prompt);

    let resumed = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-pr",
            revision_id: "rev-4",
            expected_revision: 3,
            action: Some("resume"),
            continue_round: None,
            answers: &[],
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert!(resumed.exploration_rounds[0].pending_continue_prompt);

    for act in [Some("resume"), None] {
        assert!(matches!(
            explore_step(
                conn,
                &ExploreStepInput {
                    card_id: "card-pr",
                    revision_id: "rev-bypass",
                    expected_revision: 4,
                    action: act,
                    continue_round: None,
                    answers: &[ans("D4", "edit", Some("Bypass"))],
                },
                "2026-11-24T00:04:00Z",
            ),
            Err(WorkflowBoardRepoError::PreconditionFailed(_))
        ));
    }

    let r2 = explore_step(
        conn,
        &ExploreStepInput {
            card_id: "card-pr",
            revision_id: "rev-5",
            expected_revision: 4,
            action: Some("resume"),
            continue_round: Some(true),
            answers: &[ans("D4", "edit", Some("Next"))],
        },
        "2026-11-24T00:05:00Z",
    )?;
    assert_eq!(r2.exploration_rounds.len(), 2);
    assert_eq!(
        r2.exploration_rounds[0].decision_ids_json,
        "[\"D1\",\"D2\",\"D3\"]"
    );
    assert_eq!(r2.exploration_rounds[1].decision_ids_json, "[\"D4\"]");
    Ok(())
}

#[test]
fn challenge_rejects_persisted_unresolved_manual_fallback_and_answer_erasure() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-cg")?;

    let c_unres = ChallengeConcernInput {
        id: "C1",
        summary: "Unresolved risk",
        why_matters: None,
        evidence: None,
        assumption: None,
        disposition: "unresolved",
        answer: None,
        reason: None,
        mutated_answer: None,
    };
    let c_ans = ChallengeConcernInput {
        id: "C2",
        summary: "WAL mode",
        why_matters: Some("Concurrency"),
        evidence: Some("sqlite.rs"),
        assumption: Some("Single writer"),
        disposition: "answer",
        answer: Some("Keep WAL"),
        reason: None,
        mutated_answer: None,
    };
    challenge_step(
        conn,
        &ch_in(
            "card-cg",
            "rev-2",
            1,
            "start",
            &[c_unres.clone(), c_ans.clone()],
        ),
        "2026-11-24T00:01:00Z",
    )?;

    assert!(matches!(
        challenge_step(
            conn,
            &ChallengeStepInput {
                acknowledgement_token: Some("I acknowledge"),
                manual_answers: Some(&["a", "b", "c"]),
                ..ch_in("card-cg", "rev-fb-bad", 2, "manualFallback", &[])
            },
            "2026-11-24T00:02:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let c_ans_none = ChallengeConcernInput {
        disposition: "dismiss",
        answer: None,
        reason: Some("Erased"),
        ..c_ans.clone()
    };
    let c_ans_mutated = ChallengeConcernInput {
        answer: Some("Mutated"),
        ..c_ans.clone()
    };
    for bad in [
        &[c_unres][..],
        &[c_ans_none][..],
        &[c_ans_mutated, c_ans.clone()][..],
    ] {
        assert!(matches!(
            challenge_step(
                conn,
                &ch_in("card-cg", "rev-s-bad", 2, "synthesize", bad),
                "2026-11-24T00:02:00Z"
            ),
            Err(WorkflowBoardRepoError::InvalidInput(_))
        ));
    }

    let c1_acc = ChallengeConcernInput {
        id: "C1",
        summary: "Unresolved risk",
        why_matters: None,
        evidence: None,
        assumption: None,
        disposition: "accept",
        answer: None,
        reason: None,
        mutated_answer: None,
    };
    let syn = challenge_step(
        conn,
        &ch_in("card-cg", "rev-3", 2, "synthesize", &[c1_acc, c_ans]),
        "2026-11-24T00:03:00Z",
    )?;
    assert_eq!(syn.challenges[0].status, "synthesized");
    Ok(())
}

#[test]
fn challenge_synthesize_with_empty_concerns_preserves_stored_answers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-cg-empty")?;
    challenge_step(
        conn,
        &ChallengeStepInput {
            card_id: "card-cg-empty",
            revision_id: "rev-e2",
            challenge_id: "ch-empty-1",
            expected_revision: 1,
            action: "start",
            concerns: &[ChallengeConcernInput {
                id: "C2",
                summary: "WAL mode",
                why_matters: Some("Concurrency"),
                evidence: None,
                assumption: None,
                disposition: "answer",
                answer: Some("Keep WAL"),
                reason: None,
                mutated_answer: None,
            }],
            acknowledgement_token: None,
            unresolved_concern_ids: &[],
            manual_answers: None,
            spec_digest: None,
            interview_digest: None,
            source_refs_digest: None,
            synthesis_diff_json: None,
            approved: None,
        },
        "2026-11-24T00:01:00Z",
    )?;
    let syn_empty = challenge_step(
        conn,
        &ChallengeStepInput {
            challenge_id: "ch-empty-1",
            ..ch_in("card-cg-empty", "rev-e3", 2, "synthesize", &[])
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert_eq!(syn_empty.challenges[0].status, "synthesized");
    assert!(syn_empty.challenges[0].concerns_json.contains("Keep WAL"));
    Ok(())
}

#[test]
fn challenge_starts_fresh_round_after_spec_edit_stales_prior_challenge() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-st")?;

    challenge_step(
        conn,
        &ch_in("card-st", "rev-2", 1, "synthesize", &[]),
        "2026-11-24T00:01:00Z",
    )?;
    let comp = challenge_step(
        conn,
        &ch_in("card-st", "rev-3", 2, "complete", &[]),
        "2026-11-24T00:02:00Z",
    )?;
    assert!(comp.challenges[0].completed_at.is_some());

    let sp = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-st",
            revision_id: "rev-4",
            expected_revision: 3,
            title: None,
            brief: Some("Updated brief"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert!(validate_spec_approval_m2_readiness(conn, "card-st", &sp.card.spec_json).is_err());

    let r2 = challenge_step(
        conn,
        &ch_in("card-st", "rev-5", 4, "synthesize", &[]),
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(r2.challenges.len(), 2);
    assert_eq!(r2.challenges[0].round, 2);
    assert_eq!(r2.challenges[0].synthesis_count, 1);
    assert!(r2.challenges[0].completed_at.is_none());
    validate_spec_approval_m2_readiness(conn, "card-st", &r2.card_detail.card.spec_json)?;

    let r2_done = challenge_step(
        conn,
        &ch_in("card-st", "rev-6", 5, "complete", &[]),
        "2026-11-24T00:05:00Z",
    )?;
    assert!(r2_done.challenges[0].completed_at.is_some());
    let r2_restarted = challenge_step(
        conn,
        &ch_in("card-st", "rev-7", 6, "start", &[]),
        "2026-11-24T00:06:00Z",
    )?;
    assert!(r2_restarted.challenges[0].completed_at.is_none());
    Ok(())
}
