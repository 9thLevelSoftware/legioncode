//! Deterministic Ship gate blocker evaluator with UI fix pointers (`stage`, `tab`, `target_id`).

use rusqlite::Connection;

use super::execution_queries::load_card_execution_detail_inner;
use super::types_m3::{CardExecutionDetail, ShipBlocker};
use super::{WorkflowBoardRepoError, require_non_empty};

fn push_blocker(
    out: &mut Vec<ShipBlocker>,
    id: impl Into<String>,
    kind: &str,
    message: impl Into<String>,
    stage: &str,
    tab: &str,
    target_id: Option<&str>,
) {
    out.push(ShipBlocker {
        id: id.into(),
        kind: kind.to_owned(),
        message: message.into(),
        stage: stage.to_owned(),
        tab: tab.to_owned(),
        target_id: target_id.map(str::to_owned),
    });
}

/// Evaluates all Ship gate blockers deterministically with UI fix pointers (`stage`, `tab`, `target_id`).
pub(crate) fn compute_ship_blockers_from_detail(detail: &CardExecutionDetail) -> Vec<ShipBlocker> {
    let mut blockers = Vec::new();
    append_gate_and_task_blockers(detail, &mut blockers);
    append_check_blockers(detail, &mut blockers);
    append_review_and_ac_blockers(detail, &mut blockers);
    blockers
}

fn append_gate_and_task_blockers(detail: &CardExecutionDetail, blockers: &mut Vec<ShipBlocker>) {
    let cd = &detail.planning.card_detail;
    if !cd
        .approvals
        .iter()
        .any(|a| a.gate == "spec" && a.status == "valid")
    {
        push_blocker(
            blockers,
            "blocker-spec-approval",
            "spec_stale",
            "Spec approval is missing or stale",
            "spec",
            "spec",
            None,
        );
    }
    if !cd
        .approvals
        .iter()
        .any(|a| a.gate == "plan" && a.status == "valid")
    {
        push_blocker(
            blockers,
            "blocker-plan-approval",
            "plan_stale",
            "Plan approval is missing or stale",
            "plan",
            "plan",
            None,
        );
    }
    if cd.incomplete_journal_count > 0 {
        push_blocker(
            blockers,
            "blocker-journal",
            "journal_incomplete",
            "Incomplete write-journal entries require recovery",
            "execute",
            "recovery",
            None,
        );
    }
    for act in detail
        .external_actions
        .iter()
        .filter(|a| a.status == "interrupted")
    {
        push_blocker(
            blockers,
            format!("blocker-action-{}", act.id),
            "external_edit",
            format!(
                "Interrupted external action `{}` ({}) requires reconciliation",
                act.id, act.action_kind
            ),
            "execute",
            "recovery",
            Some(&act.id),
        );
    }
    if let Some(ref ext_state) = cd.card.external_edit_state
        && ext_state != "clean"
    {
        push_blocker(
            blockers,
            "blocker-external-edit",
            "external_edit",
            format!("External edits require reconciliation ({ext_state})"),
            "execute",
            "recovery",
            None,
        );
    }
    append_policy_blockers(detail, blockers);
    append_violation_and_task_blockers(detail, blockers);
}

fn append_policy_blockers(detail: &CardExecutionDetail, blockers: &mut Vec<ShipBlocker>) {
    let Some(ref cli) = detail.cli_import else {
        return;
    };
    let policies: Vec<serde_json::Value> =
        serde_json::from_str(&cli.unsupported_policies_json).unwrap_or_default();
    for p in policies {
        let blocks = p
            .get("blocksExecution")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let status = p
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unsupported");
        if blocks
            && status != "acknowledged"
            && let Some(key) = p.get("key").and_then(serde_json::Value::as_str)
        {
            push_blocker(
                blockers,
                format!("blocker-policy-{key}"),
                "policy_unacknowledged",
                format!("Unsupported adopted CLI policy `{key}` requires acknowledgement"),
                "execute",
                "policies",
                Some(key),
            );
        }
    }
}

fn append_violation_and_task_blockers(
    detail: &CardExecutionDetail,
    blockers: &mut Vec<ShipBlocker>,
) {
    for viol in detail
        .contract_violations
        .iter()
        .filter(|v| v.status == "violation_pending")
    {
        push_blocker(
            blockers,
            format!("blocker-viol-{}", viol.id),
            "contract_violation",
            format!(
                "Unresolved file-contract violation on task {}",
                viol.task_id
            ),
            "execute",
            "tasks",
            Some(&viol.task_id),
        );
    }
    for task in detail
        .planning
        .tasks
        .iter()
        .filter(|t| !matches!(t.status.as_str(), "completed" | "skipped"))
    {
        push_blocker(
            blockers,
            format!("blocker-task-{}", task.task_id),
            "task_incomplete",
            format!("Task {} is `{}`", task.task_id, task.status),
            "execute",
            "tasks",
            Some(&task.task_id),
        );
    }
}

fn append_check_blockers(detail: &CardExecutionDetail, blockers: &mut Vec<ShipBlocker>) {
    let current_snap = detail
        .planning
        .card_detail
        .card
        .product_snapshot_digest
        .as_deref();
    for ch in &detail.planning.planned_checks {
        let snap_mismatch = current_snap.is_some()
            && ch.product_snapshot_digest.as_deref() != current_snap
            && ch.status == "passed";
        match ch.status.as_str() {
            "passed" if !snap_mismatch => {}
            "failed" => push_blocker(
                blockers,
                format!("blocker-check-{}", ch.check_id),
                "check_failed",
                format!("Planned check {} failed", ch.check_id),
                "execute",
                "checks",
                Some(&ch.check_id),
            ),
            "stale" | "historical" => push_blocker(
                blockers,
                format!("blocker-check-{}", ch.check_id),
                "check_stale",
                format!("Planned check {} is stale", ch.check_id),
                "execute",
                "checks",
                Some(&ch.check_id),
            ),
            _ if snap_mismatch => push_blocker(
                blockers,
                format!("blocker-check-{}", ch.check_id),
                "check_stale",
                format!(
                    "Planned check {} is bound to an older snapshot",
                    ch.check_id
                ),
                "execute",
                "checks",
                Some(&ch.check_id),
            ),
            _ => push_blocker(
                blockers,
                format!("blocker-check-{}", ch.check_id),
                "check_missing",
                format!("Planned check {} has not passed", ch.check_id),
                "execute",
                "checks",
                Some(&ch.check_id),
            ),
        }
    }
}

fn append_review_and_ac_blockers(detail: &CardExecutionDetail, blockers: &mut Vec<ShipBlocker>) {
    let current_snap = detail
        .planning
        .card_detail
        .card
        .product_snapshot_digest
        .as_deref();
    match detail.review_runs.first() {
        None => push_blocker(
            blockers,
            "blocker-review-missing",
            "review_stale",
            "Independent fresh-thread review has not been completed",
            "execute",
            "review",
            None,
        ),
        Some(rev)
            if matches!(
                rev.status.as_str(),
                "stale" | "historical" | "pending" | "running"
            ) =>
        {
            push_blocker(
                blockers,
                format!("blocker-review-{}", rev.id),
                "review_stale",
                format!("Independent review {} is `{}`", rev.id, rev.status),
                "execute",
                "review",
                Some(&rev.id),
            );
        }
        Some(rev) if rev.status == "failed" || rev.verdict == "failed" => push_blocker(
            blockers,
            format!("blocker-review-{}", rev.id),
            "review_unresolved",
            format!("Independent review {} failed", rev.id),
            "execute",
            "review",
            Some(&rev.id),
        ),
        Some(rev) if current_snap.is_some() && rev.snapshot_digest.as_deref() != current_snap => {
            push_blocker(
                blockers,
                format!("blocker-review-{}", rev.id),
                "review_stale",
                format!(
                    "Independent review {} is bound to an older snapshot",
                    rev.id
                ),
                "execute",
                "review",
                Some(&rev.id),
            );
        }
        Some(rev) => {
            for f in detail.review_findings.iter().filter(|f| {
                f.review_run_id == rev.id
                    && (f.status == "open" || (f.status == "resolved" && f.disposition == "fix"))
            }) {
                push_blocker(
                    blockers,
                    format!("blocker-finding-{}", f.id),
                    "review_unresolved",
                    format!("Review finding {} requires resolution", f.id),
                    "execute",
                    "review",
                    Some(&f.id),
                );
            }
        }
    }

    for ac in &detail.planning.ac_evidence {
        let snap_mismatch =
            current_snap.is_some() && ac.product_snapshot_digest.as_deref() != current_snap;
        if !matches!(ac.status.as_str(), "pass" | "not_applicable") || snap_mismatch {
            let msg = if snap_mismatch {
                format!(
                    "Acceptance criterion {} is bound to an older snapshot",
                    ac.criterion_id
                )
            } else {
                format!(
                    "Acceptance criterion {} is `{}`",
                    ac.criterion_id, ac.status
                )
            };
            push_blocker(
                blockers,
                format!("blocker-ac-{}", ac.criterion_id),
                "ac_unverified",
                msg,
                "ship",
                "acceptance",
                Some(&ac.criterion_id),
            );
        }
    }
}

/// Computes deterministic Ship blockers for `card_id`.
pub fn compute_ship_blockers(
    conn: &Connection,
    card_id: &str,
) -> Result<Vec<ShipBlocker>, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let detail = load_card_execution_detail_inner(conn, card_id)?;
    Ok(detail.ship_blockers)
}
