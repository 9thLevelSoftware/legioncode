//! Read queries hydrating full M2 planning state (`CardPlanningDetail`) for a card.

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use super::cards::load_card_detail_inner;
use super::types_m2::{
    AcEvidenceRow, CardPlanningDetail, ChallengeRow, DecisionRow, ExplorationRoundRow,
    PlanDesignRow, PlannedCheckRow, TaskContractRow,
};
use super::{WorkflowBoardRepoError, require_non_empty};

fn map_exploration_round(row: &Row<'_>) -> rusqlite::Result<ExplorationRoundRow> {
    Ok(ExplorationRoundRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        round_index: row.get(2)?,
        status: row.get(3)?,
        decision_ids_json: row.get(4)?,
        pending_continue_prompt: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_decision(row: &Row<'_>) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        card_id: row.get(0)?,
        decision_id: row.get(1)?,
        round_index: row.get(2)?,
        prompt: row.get(3)?,
        recommendation: row.get(4)?,
        depends_on_json: row.get(5)?,
        last_action: row.get(6)?,
        status: row.get(7)?,
        answer: row.get(8)?,
        rationale: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn map_challenge(row: &Row<'_>) -> rusqlite::Result<ChallengeRow> {
    Ok(ChallengeRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        round: row.get(2)?,
        status: row.get(3)?,
        spec_digest: row.get(4)?,
        interview_digest: row.get(5)?,
        source_refs_digest: row.get(6)?,
        final_spec_digest: row.get(7)?,
        concerns_json: row.get(8)?,
        synthesis_count: row.get(9)?,
        synthesis_diff_json: row.get(10)?,
        manual_fallback_json: row.get(11)?,
        generation_error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        completed_at: row.get(15)?,
    })
}

fn map_plan_design(row: &Row<'_>) -> rusqlite::Result<PlanDesignRow> {
    Ok(PlanDesignRow {
        card_id: row.get(0)?,
        lenses_json: row.get(1)?,
        alternatives_json: row.get(2)?,
        selected_alternative_id: row.get(3)?,
        selection_rationale: row.get(4)?,
        comparison_revision_count: row.get(5)?,
        strategy: row.get(6)?,
        strategy_rationale: row.get(7)?,
        strategy_metadata_json: row.get(8)?,
        granularity: row.get(9)?,
        design_digest: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn map_task(row: &Row<'_>) -> rusqlite::Result<TaskContractRow> {
    Ok(TaskContractRow {
        card_id: row.get(0)?,
        task_id: row.get(1)?,
        ordinal: row.get(2)?,
        title: row.get(3)?,
        status: row.get(4)?,
        priority: row.get(5)?,
        parent_id: row.get(6)?,
        blocked_by_json: row.get(7)?,
        blocks_json: row.get(8)?,
        files_allowed_json: row.get(9)?,
        files_forbidden_json: row.get(10)?,
        expected_artifacts_json: row.get(11)?,
        verification_commands_json: row.get(12)?,
        testing_method: row.get(13)?,
        testing_boundary: row.get(14)?,
        acceptance_ids_json: row.get(15)?,
        blocker_reason: row.get(16)?,
        notes: row.get(17)?,
        max_files_touched: row.get(18)?,
        contract_digest: row.get(19)?,
        active_stage_run_id: row.get(20)?,
        started_at: row.get(21)?,
        completed_at: row.get(22)?,
        attempt_count: row.get(23)?,
        last_error: row.get(24)?,
        last_diff_digest: row.get(25)?,
        created_at: row.get(26)?,
        updated_at: row.get(27)?,
    })
}

fn map_planned_check(row: &Row<'_>) -> rusqlite::Result<PlannedCheckRow> {
    Ok(PlannedCheckRow {
        card_id: row.get(0)?,
        check_id: row.get(1)?,
        owner_task_id: row.get(2)?,
        argv_json: row.get(3)?,
        label: row.get(4)?,
        status: row.get(5)?,
        last_exit_code: row.get(6)?,
        last_log_ref: row.get(7)?,
        product_snapshot_digest: row.get(8)?,
        stale_reason: row.get(9)?,
        last_receipt_id: row.get(10)?,
        last_run_at: row.get(11)?,
        last_duration_ms: row.get(12)?,
        last_output_excerpt: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn map_ac_evidence(row: &Row<'_>) -> rusqlite::Result<AcEvidenceRow> {
    Ok(AcEvidenceRow {
        card_id: row.get(0)?,
        criterion_id: row.get(1)?,
        method: row.get(2)?,
        check_ids_json: row.get(3)?,
        command_argv_json: row.get(4)?,
        procedure: row.get(5)?,
        status: row.get(6)?,
        observation_note: row.get(7)?,
        evidence_ref: row.get(8)?,
        evidence_digest: row.get(9)?,
        product_snapshot_digest: row.get(10)?,
        stale_reason: row.get(11)?,
        external_source: row.get(12)?,
        observed_by: row.get(13)?,
        observed_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn query_vec<T>(
    conn: &Connection,
    sql: &str,
    card_id: &str,
    op: &'static str,
    mapper: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, WorkflowBoardRepoError> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })?;
    stmt.query_map(params![card_id], mapper)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })
}

pub(crate) fn load_card_planning_detail_inner(
    conn: &Connection,
    card_id: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let card_detail = load_card_detail_inner(conn, card_id)?;
    let exploration_rounds = query_vec(
        conn,
        "SELECT id, card_id, round_index, status, decision_ids_json, pending_continue_prompt,
                created_at, updated_at
         FROM workflow_board_exploration_rounds
         WHERE card_id = ?1
         ORDER BY round_index ASC",
        card_id,
        "load_exploration_rounds",
        map_exploration_round,
    )?;
    let decisions = query_vec(
        conn,
        "SELECT card_id, decision_id, round_index, prompt, recommendation, depends_on_json,
                last_action, status, answer, rationale, created_at, updated_at
         FROM workflow_board_decisions
         WHERE card_id = ?1
         ORDER BY COALESCE(round_index, 0) ASC, decision_id ASC",
        card_id,
        "load_decisions",
        map_decision,
    )?;
    let challenges = query_vec(
        conn,
        "SELECT id, card_id, round, status, spec_digest, interview_digest, source_refs_digest,
                final_spec_digest, concerns_json, synthesis_count, synthesis_diff_json,
                manual_fallback_json, generation_error, created_at, updated_at, completed_at
         FROM workflow_board_challenges
         WHERE card_id = ?1
         ORDER BY round DESC",
        card_id,
        "load_challenges",
        map_challenge,
    )?;
    let plan_design = conn
        .query_row(
            "SELECT card_id, lenses_json, alternatives_json, selected_alternative_id,
                    selection_rationale, comparison_revision_count, strategy, strategy_rationale,
                    strategy_metadata_json, granularity, design_digest, updated_at
             FROM workflow_board_plan_designs
             WHERE card_id = ?1",
            params![card_id],
            map_plan_design,
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_plan_design",
            source,
        })?;
    let tasks = query_vec(
        conn,
        "SELECT card_id, task_id, ordinal, title, status, priority, parent_id, blocked_by_json,
                blocks_json, files_allowed_json, files_forbidden_json, expected_artifacts_json,
                verification_commands_json, testing_method, testing_boundary, acceptance_ids_json,
                blocker_reason, notes, max_files_touched, contract_digest, active_stage_run_id,
                started_at, completed_at, attempt_count, last_error, last_diff_digest,
                created_at, updated_at
         FROM workflow_board_tasks
         WHERE card_id = ?1
         ORDER BY ordinal ASC, task_id ASC",
        card_id,
        "load_tasks",
        map_task,
    )?;
    let planned_checks = query_vec(
        conn,
        "SELECT card_id, check_id, owner_task_id, argv_json, label, status, last_exit_code,
                last_log_ref, product_snapshot_digest, stale_reason, last_receipt_id,
                last_run_at, last_duration_ms, last_output_excerpt, updated_at
         FROM workflow_board_planned_checks
         WHERE card_id = ?1
         ORDER BY check_id ASC",
        card_id,
        "load_planned_checks",
        map_planned_check,
    )?;
    let ac_evidence = query_vec(
        conn,
        "SELECT card_id, criterion_id, method, check_ids_json, command_argv_json, procedure,
                status, observation_note, evidence_ref, evidence_digest, product_snapshot_digest,
                stale_reason, external_source, observed_by, observed_at, updated_at
         FROM workflow_board_ac_evidence
         WHERE card_id = ?1
         ORDER BY criterion_id ASC",
        card_id,
        "load_ac_evidence",
        map_ac_evidence,
    )?;

    Ok(CardPlanningDetail {
        card_detail,
        exploration_rounds,
        decisions,
        challenges,
        plan_design,
        tasks,
        planned_checks,
        ac_evidence,
    })
}

/// Loads full M1 + M2 planning aggregate for `card_id`.
pub fn get_card_planning_detail(
    conn: &Connection,
    card_id: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    load_card_planning_detail_inner(conn, card_id)
}
