#![forbid(unsafe_code)]
//! M3 Review, AC observations, Ship readiness, Recovery, and Amendment hydration helpers.

use legion_contracts::ids::{ThreadId, WorkflowBoardApprovalId};
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceStatus, WorkflowBoardAcObservationRecord, WorkflowBoardDeliveryReceipt,
    WorkflowBoardDeliveryStatus, WorkflowBoardExternalEditRecord, WorkflowBoardExternalEditStatus,
    WorkflowBoardFindingDisposition, WorkflowBoardFixPointer, WorkflowBoardPlanAmendmentRecord,
    WorkflowBoardRecoveryEntry, WorkflowBoardRecoveryState, WorkflowBoardRequestError,
    WorkflowBoardReviewFinding, WorkflowBoardReviewReport, WorkflowBoardShipBlocker,
    WorkflowBoardShipReadiness,
};
use legion_persistence::workflow_board_repo::types::WriteJournalRow;
use legion_persistence::workflow_board_repo::types_m3::{
    CardExecutionDetail, DeliveryReceiptRow, ShipBlocker,
};
use serde_json::Value;

use crate::workflow_board_types::{internal, parse_enum};

pub(crate) fn hydrate_review_report(
    exec: &CardExecutionDetail,
) -> Result<Option<WorkflowBoardReviewReport>, WorkflowBoardRequestError> {
    let Some(rev) = exec.review_runs.first() else {
        return Ok(None);
    };
    let findings = exec
        .review_findings
        .iter()
        .filter(|f| f.review_run_id == rev.id)
        .map(|f| {
            Ok(WorkflowBoardReviewFinding {
                id: f.id.clone(),
                severity: parse_enum(&f.severity, "finding.severity")?,
                status: Some(parse_enum(&f.status, "finding.status")?),
                criterion_id: f.criterion_id.clone(),
                file: f.file_path.clone(),
                line: f
                    .line
                    .and_then(|l| u64::try_from(l).ok())
                    .filter(|l| *l >= 1),
                message: f.message.clone(),
                disposition: parse_enum::<WorkflowBoardFindingDisposition>(
                    &f.disposition,
                    "finding.disposition",
                )?,
                resolution_note: f.resolution_note.clone(),
                resolved_by: f.resolved_by.clone(),
                resolved_at: f.resolved_at.clone(),
            })
        })
        .collect::<Result<Vec<_>, WorkflowBoardRequestError>>()?;
    Ok(Some(WorkflowBoardReviewReport {
        review_id: rev.id.clone(),
        stage_run_id: rev.stage_run_id.clone(),
        thread_id: rev
            .thread_id
            .as_deref()
            .map(ThreadId::new)
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        status: parse_enum(&rev.status, "review.status")?,
        verdict: Some(parse_enum(&rev.verdict, "review.verdict")?),
        summary: rev.summary.clone(),
        product_snapshot_digest: rev.snapshot_digest.clone(),
        stale_reason: rev.stale_reason.clone(),
        findings,
        created_at: rev.created_at.clone(),
        updated_at: rev.updated_at.clone(),
        completed_at: rev.completed_at.clone(),
    }))
}

pub(crate) fn hydrate_ac_observations(
    exec: &CardExecutionDetail,
) -> Result<Vec<WorkflowBoardAcObservationRecord>, WorkflowBoardRequestError> {
    exec.planning
        .ac_evidence
        .iter()
        .map(|ac| {
            Ok(WorkflowBoardAcObservationRecord {
                criterion_id: ac.criterion_id.clone(),
                method: parse_enum(&ac.method, "ac.method")?,
                status: parse_enum::<WorkflowBoardAcEvidenceStatus>(&ac.status, "ac.status")?,
                check_ids: serde_json::from_str(&ac.check_ids_json).unwrap_or_default(),
                procedure: ac.procedure.clone(),
                observation_note: ac.observation_note.clone(),
                evidence_ref: ac.evidence_ref.clone(),
                evidence_digest: ac.evidence_digest.clone(),
                product_snapshot_digest: ac.product_snapshot_digest.clone(),
                external_source: ac.external_source.clone(),
                stale_reason: ac.stale_reason.clone(),
                observed_by: ac.observed_by.clone(),
                observed_at: ac.observed_at.clone(),
                updated_at: ac.updated_at.clone(),
            })
        })
        .collect()
}

pub(crate) fn to_delivery_receipt(
    row: &DeliveryReceiptRow,
) -> Result<WorkflowBoardDeliveryReceipt, WorkflowBoardRequestError> {
    Ok(WorkflowBoardDeliveryReceipt {
        id: row.id.clone(),
        approval_id: row
            .approval_id
            .as_deref()
            .map(WorkflowBoardApprovalId::new)
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        card_revision: u64::try_from(row.card_revision.max(1)).unwrap_or(1),
        product_snapshot_digest: row.product_snapshot_digest.clone(),
        commit_sha: row.commit_sha.clone(),
        spec_digest: row.spec_digest.clone(),
        plan_digest: row.plan_digest.clone(),
        tasks_digest: row.tasks_digest.clone(),
        checks_digest: row.checks_digest.clone(),
        review_id: row.review_id.clone(),
        ac_verdicts_summary: row.ac_verdicts_summary_json.clone(),
        approved_by: row.approved_by.clone(),
        approved_at: row.approved_at.clone(),
        status: parse_enum(&row.status, "delivery_receipt.status")?,
    })
}

pub(crate) fn hydrate_ship_readiness(
    exec: &CardExecutionDetail,
    delivery_receipt: Option<WorkflowBoardDeliveryReceipt>,
) -> Result<WorkflowBoardShipReadiness, WorkflowBoardRequestError> {
    let card = &exec.planning.card_detail.card;
    let blockers: Vec<WorkflowBoardShipBlocker> = exec
        .ship_blockers
        .iter()
        .map(to_ship_blocker)
        .collect::<Result<Vec<_>, _>>()?;
    let ready = blockers.is_empty();
    let status = if card.status == "shipped" {
        WorkflowBoardDeliveryStatus::Shipped
    } else if !ready {
        WorkflowBoardDeliveryStatus::Blocked
    } else if matches!(card.stage.as_str(), "execute" | "ship") {
        WorkflowBoardDeliveryStatus::Ready
    } else {
        WorkflowBoardDeliveryStatus::Pending
    };
    Ok(WorkflowBoardShipReadiness {
        status,
        ready,
        product_snapshot_digest: card.product_snapshot_digest.clone(),
        blockers,
        delivery_receipt,
    })
}

fn to_ship_blocker(b: &ShipBlocker) -> Result<WorkflowBoardShipBlocker, WorkflowBoardRequestError> {
    Ok(WorkflowBoardShipBlocker {
        id: b.id.clone(),
        kind: parse_enum(&b.kind, "ship_blocker.kind")?,
        message: b.message.clone(),
        pointer: WorkflowBoardFixPointer {
            stage: parse_enum(&b.stage, "ship_blocker.stage")?,
            tab: b.tab.clone(),
            target_id: b.target_id.clone(),
        },
        blocking: true,
    })
}

pub(crate) fn hydrate_recovery_state(
    exec: &CardExecutionDetail,
    pending_journal: &[WriteJournalRow],
) -> Result<WorkflowBoardRecoveryState, WorkflowBoardRequestError> {
    let card = &exec.planning.card_detail.card;
    let incomplete_journal_count =
        u64::try_from(exec.planning.card_detail.incomplete_journal_count.max(0)).unwrap_or(0);
    let mut pending_entries = Vec::with_capacity(pending_journal.len());
    for row in pending_journal {
        let norm_path = row.target_path.replace('\\', "/");
        let rel_key = norm_path
            .split(&format!("/{}/", card.card_id))
            .nth(1)
            .unwrap_or(&norm_path)
            .to_owned();
        pending_entries.push(WorkflowBoardRecoveryEntry {
            id: row.id.clone(),
            target_path: rel_key,
            operation: parse_enum(&row.operation, "journal.operation")?,
            status: parse_enum(&row.status, "journal.status")?,
            expected_digest: row.expected_digest.clone(),
            previous_digest: row.previous_digest.clone(),
            revision: row
                .revision
                .and_then(|r| u64::try_from(r).ok())
                .filter(|r| *r >= 1),
            last_error: row.last_error.clone(),
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
            completed_at: row.completed_at.clone(),
        });
    }
    let mut external_edits: Vec<WorkflowBoardExternalEditRecord> = card
        .external_edit_detail_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<WorkflowBoardExternalEditRecord>>(raw).ok())
        .unwrap_or_default();
    if external_edits.is_empty()
        && let Some(ref ext_state) = card.external_edit_state
        && ext_state != "clean"
    {
        external_edits.push(WorkflowBoardExternalEditRecord {
            id: format!("ext-{}", card.card_id),
            relative_path: "card.json".to_owned(),
            artifact_kind: None,
            expected_digest: None,
            actual_digest: None,
            status: WorkflowBoardExternalEditStatus::Detected,
            affected_gates: Vec::new(),
            detected_at: card.updated_at.clone(),
            reconciled_at: None,
            summary: format!("External edit detected ({ext_state})"),
        });
    }
    for act in exec
        .external_actions
        .iter()
        .filter(|a| a.status == "interrupted")
    {
        let note = serde_json::from_str::<Value>(&act.details_json)
            .ok()
            .and_then(|v| v.get("summary").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| format!("Interrupted external action `{}`", act.action_kind));
        external_edits.push(WorkflowBoardExternalEditRecord {
            id: act.id.clone(),
            relative_path: "card.json".to_owned(),
            artifact_kind: None,
            expected_digest: act.expected_snapshot_digest.clone(),
            actual_digest: act.observed_snapshot_digest.clone(),
            status: WorkflowBoardExternalEditStatus::Detected,
            affected_gates: Vec::new(),
            detected_at: act.created_at.clone(),
            reconciled_at: act.reconciled_at.clone(),
            summary: note,
        });
    }
    let has_interrupted = exec
        .external_actions
        .iter()
        .any(|a| a.status == "interrupted");
    let has_ext_state = card
        .external_edit_state
        .as_deref()
        .is_some_and(|s| s != "clean");
    let requires_reconciliation = incomplete_journal_count > 0 || has_interrupted || has_ext_state;
    Ok(WorkflowBoardRecoveryState {
        incomplete_journal_count,
        pending_journal_entries: pending_entries,
        external_edits,
        requires_reconciliation,
        last_recovered_at: None,
    })
}

pub(crate) fn hydrate_amendments(
    exec: &CardExecutionDetail,
) -> Result<Vec<WorkflowBoardPlanAmendmentRecord>, WorkflowBoardRequestError> {
    exec.amendments
        .iter()
        .map(|a| {
            Ok(WorkflowBoardPlanAmendmentRecord {
                id: a.id.clone(),
                card_revision: u64::try_from(a.card_revision.max(1)).unwrap_or(1),
                target_stage: Some(parse_enum(&a.target_stage, "amendment.target_stage")?),
                status: Some(parse_enum(&a.status, "amendment.status")?),
                reason: a.reason.clone(),
                summary: a.summary.clone(),
                source_task_id: a.source_task_id.clone(),
                unexpected_files: serde_json::from_str(&a.unexpected_files_json)
                    .unwrap_or_default(),
                created_at: a.created_at.clone(),
            })
        })
        .collect()
}
