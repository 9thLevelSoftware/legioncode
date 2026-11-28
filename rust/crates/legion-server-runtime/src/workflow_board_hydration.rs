#![forbid(unsafe_code)]
//! M1 summary, snapshot, approval, stage-run, and revision hydration helpers.

use legion_contracts::ids::{
    ProjectId, RunId, ThreadId, WorkflowBoardApprovalId, WorkflowBoardStageRunId, WorkflowCardId,
};
use legion_contracts::workflow_board::{
    ProjectWorkflowBoardSnapshot, WorkflowBoardApprovalReceipt, WorkflowBoardCardStatus,
    WorkflowBoardCardSummary, WorkflowBoardDeliveryStatus, WorkflowBoardRequestError,
    WorkflowBoardRevisionEntry, WorkflowBoardStageRunLink,
};
use legion_persistence::workflow_board_repo::types::{
    ApprovalRow, BoardSnapshot, CardDetail, CardSummary, RevisionRow, StageRunRow,
};
use serde_json::Value;

use crate::workflow_board_types::{
    internal, parse_enum, parse_opt_enum, parse_stage_run_model_selection, parse_workspace_strategy,
};

fn hydrate_card_status(
    raw_status: &str,
) -> Result<(WorkflowBoardCardStatus, Option<WorkflowBoardDeliveryStatus>), WorkflowBoardRequestError>
{
    if raw_status == "shipped" {
        return Ok((
            WorkflowBoardCardStatus::Completed,
            Some(WorkflowBoardDeliveryStatus::Shipped),
        ));
    }
    Ok((parse_enum(raw_status, "status")?, None))
}

pub(crate) fn to_card_summary(
    row: &CardSummary,
) -> Result<WorkflowBoardCardSummary, WorkflowBoardRequestError> {
    let (status, delivery_status) = hydrate_card_status(&row.status)?;
    Ok(WorkflowBoardCardSummary {
        card_id: WorkflowCardId::new(&row.card_id).map_err(|e| internal(e.to_string()))?,
        project_id: ProjectId::new(&row.project_id).map_err(|e| internal(e.to_string()))?,
        title: row.title.clone(),
        stage: parse_enum(&row.stage, "stage")?,
        status,
        revision: u64::try_from(row.revision.max(1)).unwrap_or(1),
        current_task: row.current_task_id.clone(),
        blocker_summary: row.blocker_reason.clone(),
        next_action: row.next_action.clone(),
        spec_approved_revision: row
            .spec_approved_revision
            .and_then(|r| u64::try_from(r).ok())
            .filter(|r| *r >= 1),
        spec_approval_status: parse_opt_enum(
            row.spec_approval_status.as_deref(),
            "spec_approval_status",
        )?,
        active_thread_id: row
            .active_thread_id
            .as_deref()
            .map(ThreadId::new)
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        plan_approved_revision: row
            .plan_approved_revision
            .and_then(|r| u64::try_from(r).ok())
            .filter(|r| *r >= 1),
        plan_approval_status: parse_opt_enum(
            row.plan_approval_status.as_deref(),
            "plan_approval_status",
        )?,
        incomplete_journal_count: u64::try_from(row.incomplete_journal_count.max(0)).ok(),
        has_unreconciled_edits: row.external_edit_state.as_deref().map(|s| s != "clean"),
        delivery_status,
    })
}

pub(crate) fn to_board_snapshot(
    snap: &BoardSnapshot,
) -> Result<ProjectWorkflowBoardSnapshot, WorkflowBoardRequestError> {
    let cards = snap
        .cards
        .iter()
        .map(to_card_summary)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectWorkflowBoardSnapshot {
        project_id: ProjectId::new(&snap.project_id).map_err(|e| internal(e.to_string()))?,
        revision: u64::try_from(snap.revision.max(0)).unwrap_or(0),
        cards,
        generated_at: snap.generated_at.clone(),
    })
}

pub(crate) fn to_approval_receipt(
    row: &ApprovalRow,
) -> Result<WorkflowBoardApprovalReceipt, WorkflowBoardRequestError> {
    let meta: Value = serde_json::from_str(&row.metadata_json).unwrap_or(Value::Null);
    let checks_digest = meta
        .get("checksDigest")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let acceptance_criteria_ids =
        meta.get("acceptanceIds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
    Ok(WorkflowBoardApprovalReceipt {
        id: WorkflowBoardApprovalId::new(&row.id).map_err(|e| internal(e.to_string()))?,
        gate: parse_enum(&row.gate, "gate")?,
        status: parse_enum(&row.status, "approval_status")?,
        revision: u64::try_from(row.revision.max(1)).unwrap_or(1),
        artifact_digest: row.artifact_digest.clone(),
        spec_digest: row.spec_digest.clone(),
        wireframes_digest: row.wireframes_digest.clone(),
        approved_at: row.approved_at.clone(),
        approved_by: row.approved_by.clone(),
        plan_digest: row.plan_digest.clone(),
        tasks_digest: row.tasks_digest.clone(),
        checks_digest,
        acceptance_criteria_ids,
        product_snapshot_digest: row.product_snapshot_digest.clone(),
    })
}

pub(crate) fn to_stage_run_link(
    row: &StageRunRow,
    project_id: &ProjectId,
) -> Result<WorkflowBoardStageRunLink, WorkflowBoardRequestError> {
    let model_selection = parse_stage_run_model_selection(row)?;
    Ok(WorkflowBoardStageRunLink {
        id: WorkflowBoardStageRunId::new(&row.id).map_err(|e| internal(e.to_string()))?,
        stage: parse_enum(&row.stage, "stage_run.stage")?,
        task_id: row.task_id.clone(),
        attempt: u64::try_from(row.attempt).ok().filter(|a| *a >= 1),
        stable_launch_key: row.stable_launch_key.clone(),
        status: parse_enum(&row.status, "stage_run.status")?,
        card_revision: u64::try_from(row.card_revision.max(1)).unwrap_or(1),
        run_id: row
            .workflow_run_id
            .as_deref()
            .map(RunId::new)
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        thread_id: row
            .thread_id
            .as_deref()
            .map(ThreadId::new)
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        project_id: project_id.clone(),
        workspace_strategy: parse_workspace_strategy(&row.workspace_strategy_json)?,
        worktree_path: row.worktree_path.clone(),
        branch: row.branch.clone(),
        model_selection,
        runtime_mode: parse_enum(&row.runtime_mode, "stage_run.runtime_mode")?,
        interaction_mode: parse_enum(&row.interaction_mode, "stage_run.interaction_mode")?,
        last_error: row.last_error.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        ended_at: row.ended_at.clone(),
    })
}

pub(crate) fn to_revision_entry(
    row: &RevisionRow,
) -> Result<WorkflowBoardRevisionEntry, WorkflowBoardRequestError> {
    Ok(WorkflowBoardRevisionEntry {
        revision: u64::try_from(row.revision.max(1)).unwrap_or(1),
        stage: parse_enum(&row.stage, "revision.stage")?,
        kind: row.kind.clone(),
        summary: row.summary.clone(),
        spec_digest: row.spec_digest.clone(),
        created_at: row.created_at.clone(),
        plan_digest: row.plan_digest.clone(),
    })
}

pub(crate) fn build_card_summary(
    detail: &CardDetail,
    project_id: &ProjectId,
    spec_approval: Option<&WorkflowBoardApprovalReceipt>,
    plan_approval: Option<&WorkflowBoardApprovalReceipt>,
) -> Result<WorkflowBoardCardSummary, WorkflowBoardRequestError> {
    let card = &detail.card;
    let active_thread_id = detail
        .stage_runs
        .iter()
        .filter(|sr| sr.thread_id.is_some())
        .max_by(|a, b| {
            (a.updated_at.as_str(), a.id.as_str()).cmp(&(b.updated_at.as_str(), b.id.as_str()))
        })
        .and_then(|sr| sr.thread_id.as_deref())
        .map(ThreadId::new)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;
    let (status, delivery_status) = hydrate_card_status(&card.status)?;
    Ok(WorkflowBoardCardSummary {
        card_id: WorkflowCardId::new(&card.card_id).map_err(|e| internal(e.to_string()))?,
        project_id: project_id.clone(),
        title: card.title.clone(),
        stage: parse_enum(&card.stage, "stage")?,
        status,
        revision: u64::try_from(card.revision.max(1)).unwrap_or(1),
        current_task: card.current_task_id.clone(),
        blocker_summary: card.blocker_reason.clone(),
        next_action: card.next_action.clone(),
        spec_approved_revision: spec_approval.map(|a| a.revision),
        spec_approval_status: spec_approval.map(|a| a.status),
        active_thread_id,
        created_at: card.created_at.clone(),
        updated_at: card.updated_at.clone(),
        plan_approved_revision: plan_approval.map(|a| a.revision),
        plan_approval_status: plan_approval.map(|a| a.status),
        incomplete_journal_count: u64::try_from(detail.incomplete_journal_count.max(0)).ok(),
        has_unreconciled_edits: card.external_edit_state.as_deref().map(|s| s != "clean"),
        delivery_status,
    })
}
