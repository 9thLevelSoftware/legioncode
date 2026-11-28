#![forbid(unsafe_code)]
//! Spec exploration, challenge synthesis, and assertion helpers for M2 service tests.

use legion_contracts::ids::WorkflowCardId;
use legion_contracts::workflow_board::{
    WorkflowBoardChallengeConcernInput, WorkflowBoardChallengeConcernResponse,
    WorkflowBoardChallengeDisposition, WorkflowBoardChallengeStatus,
    WorkflowBoardChallengeStepAction, WorkflowBoardChallengeStepAnswers,
    WorkflowBoardChallengeStepInput, WorkflowBoardChallengeStepResult, WorkflowBoardExploreAction,
    WorkflowBoardExploreAnswerInput, WorkflowBoardExploreStepAction, WorkflowBoardExploreStepInput,
    WorkflowBoardRequestError,
};
use legion_server_runtime::ProjectWorkflowBoardService;

pub async fn run_explore_pause_resume(
    board: &ProjectWorkflowBoardService,
    cid: &WorkflowCardId,
    rev: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let seeded = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: rev,
            action: Some(WorkflowBoardExploreStepAction::Step),
            continue_round: None,
            answers: None,
        })
        .await?;
    let all_answers: Vec<WorkflowBoardExploreAnswerInput> = seeded
        .decisions
        .into_iter()
        .take(3)
        .map(|d| WorkflowBoardExploreAnswerInput {
            decision_id: d.id,
            action: Some(WorkflowBoardExploreAction::Recommend),
            choice: None,
            answer: None,
            note: None,
        })
        .collect();
    if all_answers.is_empty() {
        return Err("missing seeded decisions".into());
    }

    let paused = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: seeded.revision,
            action: Some(WorkflowBoardExploreStepAction::Pause),
            continue_round: None,
            answers: None,
        })
        .await?;
    assert!(paused.paused);

    let resumed = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: paused.revision,
            action: Some(WorkflowBoardExploreStepAction::Resume),
            continue_round: None,
            answers: None,
        })
        .await?;
    assert!(!resumed.paused);

    let answered = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid.clone(),
            expected_revision: resumed.revision,
            action: Some(WorkflowBoardExploreStepAction::Step),
            continue_round: Some(false),
            answers: Some(all_answers),
        })
        .await?;
    Ok(answered.revision)
}

pub fn concern(
    id: &str,
    summary: &str,
    why_matters: Option<&str>,
    evidence: Option<&str>,
    assumption: Option<&str>,
    disposition: WorkflowBoardChallengeDisposition,
    answer: Option<&str>,
) -> WorkflowBoardChallengeConcernInput {
    WorkflowBoardChallengeConcernInput {
        id: id.to_owned(),
        summary: summary.to_owned(),
        why_matters: why_matters.map(str::to_owned),
        evidence: evidence.map(str::to_owned),
        assumption: assumption.map(str::to_owned),
        disposition,
        answer: answer.map(str::to_owned),
        reason: None,
        mutated_answer: None,
    }
}

pub async fn ch_step_req(
    board: &ProjectWorkflowBoardService,
    cid: &WorkflowCardId,
    rev: u64,
    action: WorkflowBoardChallengeStepAction,
    concerns: Option<Vec<WorkflowBoardChallengeConcernInput>>,
    answers: Option<WorkflowBoardChallengeStepAnswers>,
) -> Result<WorkflowBoardChallengeStepResult, WorkflowBoardRequestError> {
    board
        .challenge_step(&WorkflowBoardChallengeStepInput {
            card_id: cid.clone(),
            expected_revision: rev,
            action,
            concerns,
            acknowledgement_token: None,
            unresolved_concern_ids: None,
            answers,
            approved: None,
        })
        .await
}

pub async fn run_challenge_synthesis(
    board: &ProjectWorkflowBoardService,
    cid: &WorkflowCardId,
    rev: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let started = ch_step_req(
        board,
        cid,
        rev,
        WorkflowBoardChallengeStepAction::Start,
        Some(vec![concern(
            "CON-1",
            "Confirm digest scope",
            Some("Avoid missing source inputs"),
            Some("docs/input.md"),
            None,
            WorkflowBoardChallengeDisposition::Unresolved,
            None,
        )]),
        None,
    )
    .await?;
    let synthesized = ch_step_req(
        board,
        cid,
        started.revision,
        WorkflowBoardChallengeStepAction::Synthesize,
        None,
        Some(WorkflowBoardChallengeStepAnswers::Responses(vec![
            WorkflowBoardChallengeConcernResponse {
                concern_id: "CON-1".to_owned(),
                disposition: WorkflowBoardChallengeDisposition::Accept,
                response: Some("Confirmed".to_owned()),
                reason: None,
            },
        ])),
    )
    .await?;
    let completed = ch_step_req(
        board,
        cid,
        synthesized.revision,
        WorkflowBoardChallengeStepAction::Complete,
        None,
        None,
    )
    .await?;
    assert_eq!(
        completed.challenge_receipt.status,
        WorkflowBoardChallengeStatus::Complete
    );
    Ok(completed.revision)
}
