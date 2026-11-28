//! Card creation, listing, snapshot, and detail queries for `workflow_board_repo`.

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use super::stage_runs::list_stage_runs_for_card;
use super::{
    ApprovalRow, CardDetail, CardRow, CreateCardInput, RevisionRow, WorkflowBoardRepoError,
    WriteTx, require_non_empty,
};

pub(crate) fn validate_intake_goal(
    raw: Option<&str>,
) -> Result<Option<String>, WorkflowBoardRepoError> {
    match raw {
        Some(g) => {
            let trimmed = require_non_empty(g, "intake_goal")?;
            if !matches!(trimmed, "greenfield" | "change" | "audit") {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "unsupported intake_goal '{trimmed}'"
                )));
            }
            Ok(Some(trimmed.to_owned()))
        }
        None => Ok(None),
    }
}

pub(crate) fn validate_input_paths_json(
    raw: Option<&str>,
) -> Result<String, WorkflowBoardRepoError> {
    match raw {
        Some(json_str) => {
            let paths: Vec<String> =
                serde_json::from_str(json_str).map_err(|source| WorkflowBoardRepoError::Json {
                    operation: "parse_input_paths_json",
                    source,
                })?;
            let mut normalized = Vec::with_capacity(paths.len());
            for p in paths {
                let trimmed = p.trim();
                if !legion_contracts::workflow_board::is_valid_input_path(trimmed) {
                    return Err(WorkflowBoardRepoError::InvalidInput(format!(
                        "invalid input_path '{trimmed}'"
                    )));
                }
                normalized.push(trimmed.to_owned());
            }
            serde_json::to_string(&normalized).map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_input_paths_json",
                source,
            })
        }
        None => Ok("[]".to_owned()),
    }
}

pub(crate) fn map_card_row(row: &Row<'_>) -> rusqlite::Result<CardRow> {
    Ok(CardRow {
        card_id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        stage: row.get(3)?,
        status: row.get(4)?,
        revision: row.get(5)?,
        brief: row.get(6)?,
        intake_goal: row.get(7)?,
        brief_path: row.get(8)?,
        brief_digest: row.get(9)?,
        input_paths_json: row.get(10)?,
        source_type: row.get(11)?,
        assistance_mode: row.get(12)?,
        workspace_strategy_json: row.get(13)?,
        worktree_path: row.get(14)?,
        branch: row.get(15)?,
        model_selection_json: row.get(16)?,
        runtime_mode: row.get(17)?,
        interaction_mode: row.get(18)?,
        spec_digest: row.get(19)?,
        wireframes_digest: row.get(20)?,
        design_digest: row.get(21)?,
        plan_digest: row.get(22)?,
        tasks_digest: row.get(23)?,
        current_task_id: row.get(24)?,
        blocker_reason: row.get(25)?,
        next_action: row.get(26)?,
        spec_json: row.get(27)?,
        plan_json: row.get(28)?,
        import_provenance_json: row.get(29)?,
        metadata_json: row.get(30)?,
        product_snapshot_digest: row.get(31)?,
        paused_from_status: row.get(32)?,
        pause_reason: row.get(33)?,
        paused_at: row.get(34)?,
        external_edit_state: row.get(35)?,
        external_edit_detail_json: row.get(36)?,
        created_at: row.get(37)?,
        updated_at: row.get(38)?,
        completed_at: row.get(39)?,
        archived_at: row.get(40)?,
    })
}

fn map_revision_row(row: &Row<'_>) -> rusqlite::Result<RevisionRow> {
    Ok(RevisionRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        revision: row.get(2)?,
        stage: row.get(3)?,
        kind: row.get(4)?,
        actor_kind: row.get(5)?,
        summary: row.get(6)?,
        spec_digest: row.get(7)?,
        plan_digest: row.get(8)?,
        payload_json: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub(crate) fn map_approval_row(row: &Row<'_>) -> rusqlite::Result<ApprovalRow> {
    Ok(ApprovalRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        gate: row.get(2)?,
        status: row.get(3)?,
        revision: row.get(4)?,
        artifact_digest: row.get(5)?,
        spec_digest: row.get(6)?,
        plan_digest: row.get(7)?,
        tasks_digest: row.get(8)?,
        wireframes_digest: row.get(9)?,
        product_snapshot_digest: row.get(10)?,
        approved_by: row.get(11)?,
        approved_at: row.get(12)?,
        invalidated_at: row.get(13)?,
        invalidation_reason: row.get(14)?,
        metadata_json: row.get(15)?,
    })
}

pub(crate) fn load_card_row(
    conn: &Connection,
    card_id: &str,
) -> Result<CardRow, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT card_id, project_id, title, stage, status, revision, brief,
                intake_goal, brief_path, brief_digest, input_paths_json,
                source_type, assistance_mode, workspace_strategy_json, worktree_path, branch,
                model_selection_json, runtime_mode, interaction_mode,
                spec_digest, wireframes_digest, design_digest, plan_digest, tasks_digest,
                current_task_id, blocker_reason, next_action, spec_json, plan_json,
                import_provenance_json, metadata_json, product_snapshot_digest,
                paused_from_status, pause_reason, paused_at, external_edit_state,
                external_edit_detail_json, created_at, updated_at, completed_at, archived_at
         FROM workflow_board_cards
         WHERE card_id = ?1",
        params![card_id],
        map_card_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "load_card_row",
        source,
    })?
    .ok_or_else(|| WorkflowBoardRepoError::CardNotFound {
        card_id: card_id.to_owned(),
    })
}

pub(crate) fn load_card_detail_inner(
    conn: &Connection,
    card_id: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card = load_card_row(conn, card_id)?;

    let mut rev_stmt = conn
        .prepare(
            "SELECT id, card_id, revision, stage, kind, actor_kind, summary, spec_digest,
                    plan_digest, payload_json, created_at
             FROM workflow_board_revisions
             WHERE card_id = ?1
             ORDER BY revision ASC",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "prepare_list_revisions",
            source,
        })?;
    let revisions = rev_stmt
        .query_map(params![card_id], map_revision_row)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_list_revisions",
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_revisions",
            source,
        })?;

    let mut app_stmt = conn
        .prepare(
            "SELECT id, card_id, gate, status, revision, artifact_digest, spec_digest,
                    plan_digest, tasks_digest, wireframes_digest, product_snapshot_digest,
                    approved_by, approved_at, invalidated_at, invalidation_reason, metadata_json
             FROM workflow_board_approvals
             WHERE card_id = ?1
             ORDER BY approved_at DESC, id DESC",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "prepare_list_approvals",
            source,
        })?;
    let approvals = app_stmt
        .query_map(params![card_id], map_approval_row)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_list_approvals",
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_approvals",
            source,
        })?;

    let stage_runs = list_stage_runs_for_card(conn, card_id)?;

    let incomplete_journal_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_write_journal
             WHERE card_id = ?1 AND status NOT IN ('committed', 'rolled_back')",
            params![card_id],
            |row| row.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_incomplete_journal",
            source,
        })?;

    Ok(CardDetail {
        card,
        revisions,
        approvals,
        stage_runs,
        incomplete_journal_count,
    })
}

struct ValidatedCreateFields<'a> {
    source_type: &'a str,
    assistance_mode: &'a str,
    intake_goal: Option<String>,
    input_paths_json: Option<String>,
    brief_digest: Option<String>,
    wireframes_digest: Option<String>,
    brief_path: Option<&'a str>,
}

fn validate_create_card_fields<'a>(
    input: &'a CreateCardInput<'a>,
) -> Result<ValidatedCreateFields<'a>, WorkflowBoardRepoError> {
    let source_type = require_non_empty(input.source_type, "source_type")?;
    if !matches!(source_type, "idea" | "file" | "codebase") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported source_type '{source_type}'"
        )));
    }
    let assistance_mode = require_non_empty(input.assistance_mode, "assistance_mode")?;
    if !matches!(assistance_mode, "guided" | "balanced" | "direct") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported assistance_mode '{assistance_mode}'"
        )));
    }
    let intake_goal = validate_intake_goal(input.intake_goal)?;
    let input_paths_json = validate_input_paths_json(input.input_paths_json)?;
    let brief_digest = input
        .brief_digest
        .map(|d| require_non_empty(d, "brief_digest"))
        .transpose()?
        .map(str::to_owned);
    let wireframes_digest = input
        .wireframes_digest
        .map(|d| require_non_empty(d, "wireframes_digest"))
        .transpose()?
        .map(str::to_owned);
    let brief_path = match input.brief_path {
        Some(raw) => {
            let trimmed = raw.trim();
            if !legion_contracts::workflow_board::is_valid_input_path(trimmed) {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "invalid brief_path '{trimmed}'"
                )));
            }
            Some(trimmed)
        }
        None => None,
    };
    Ok(ValidatedCreateFields {
        source_type,
        assistance_mode,
        intake_goal,
        input_paths_json: Some(input_paths_json),
        brief_digest,
        wireframes_digest,
        brief_path,
    })
}

/// Creates a new workflow board card at `revision = 1` in stage `'spec'` with an initial revision row.
pub fn create_card(
    conn: &Connection,
    input: &CreateCardInput<'_>,
    now_iso: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let project_id = require_non_empty(input.project_id, "project_id")?;
    let title = require_non_empty(input.title, "title")?;
    let fields = validate_create_card_fields(input)?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    conn.execute(
        "INSERT INTO workflow_board_cards (
           card_id, project_id, title, stage, status, revision, brief,
           intake_goal, brief_path, brief_digest, input_paths_json,
           source_type, assistance_mode, workspace_strategy_json, worktree_path, branch,
           model_selection_json, runtime_mode, interaction_mode, spec_digest, wireframes_digest,
           next_action, spec_json, created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, 'spec', 'active', 1, ?4,
           ?5, ?6, ?7, ?8,
           ?9, ?10, ?11, ?12, ?13,
           ?14, ?15, ?16, ?17, ?18,
           'draft_spec', ?19, ?20, ?20
         )",
        params![
            card_id,
            project_id,
            title,
            input.brief,
            fields.intake_goal,
            fields.brief_path,
            fields.brief_digest,
            fields.input_paths_json,
            fields.source_type,
            fields.assistance_mode,
            input.workspace_strategy_json,
            input.worktree_path,
            input.branch,
            input.model_selection_json,
            input.runtime_mode,
            input.interaction_mode,
            input.spec_digest,
            fields.wireframes_digest,
            input.spec_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_card",
        source,
    })?;

    conn.execute(
        "INSERT INTO workflow_board_revisions (
           id, card_id, revision, stage, kind, actor_kind, summary, spec_digest,
           payload_json, created_at
         ) VALUES (?1, ?2, 1, 'spec', 'card_created', 'client', ?3, ?4, ?5, ?6)",
        params![
            revision_id,
            card_id,
            format!("Created card '{title}'"),
            input.spec_digest,
            input.spec_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_initial_revision",
        source,
    })?;

    let detail = load_card_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Loads full card detail, revisions, approvals, stage runs, and incomplete journal count.
pub fn get_card_detail(
    conn: &Connection,
    card_id: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    load_card_detail_inner(conn, card_id)
}
