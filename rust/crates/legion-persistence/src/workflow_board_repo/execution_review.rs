//! M3 fresh-thread independent review recording and human finding disposition.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::blocker_status::recompute_card_blocker_status;
use super::cards::load_card_row;
use super::execution_queries::load_card_execution_detail_inner;
pub use super::execution_review_resolve::resolve_finding;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m3::{CardExecutionDetail, RecordReviewRunInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Records a fresh-thread independent review run and its structured findings inside a CAS `WriteTx`.
///
/// Rejects `thread_id` reuse if that thread was previously attached to any non-`'review'` stage run
/// on `card_id`.
pub fn record_review_run(
    conn: &Connection,
    input: &RecordReviewRunInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _review_id = require_non_empty(input.review_id, "review_id")?;
    let verdict = require_non_empty(input.verdict, "verdict")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(verdict, "pass" | "findings" | "failed") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported review verdict `{verdict}`"
        )));
    }
    if verdict == "pass" && !input.findings.is_empty() {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "review verdict `pass` cannot carry findings".to_owned(),
        ));
    }

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if matches!(existing.status.as_str(), "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot record review run while card status is `{}`",
            existing.status
        )));
    }
    let trimmed_input_snap = input
        .snapshot_digest
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(current_snap) = existing
        .product_snapshot_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && trimmed_input_snap != Some(current_snap)
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "digest mismatch: review snapshot_digest {trimmed_input_snap:?} does not match current product snapshot `{current_snap}`"
        )));
    }

    if let Some(tid) = input.thread_id.map(str::trim).filter(|s| !s.is_empty()) {
        let non_review_reuse: Option<String> = conn
            .query_row(
                "SELECT stage FROM workflow_board_stage_runs
                 WHERE card_id = ?1 AND thread_id = ?2 AND stage != 'review'
                 LIMIT 1",
                params![card_id, tid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "check_fresh_review_thread",
                source,
            })?;
        if let Some(prior_stage) = non_review_reuse {
            return Err(WorkflowBoardRepoError::ReuseConflict(format!(
                "independent review requires a fresh thread; `{tid}` was already used in stage `{prior_stage}`"
            )));
        }
    }

    persist_review_run_and_findings(conn, &existing, input, now)?;
    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn persist_review_run_and_findings(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordReviewRunInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    archive_and_insert_review_run(conn, existing, input, now)?;
    insert_review_findings_and_finalize_card(conn, existing, input, now)
}

fn archive_and_insert_review_run(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordReviewRunInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let review_id = input.review_id.trim();
    let verdict = input.verdict.trim();
    conn.execute(
        "UPDATE workflow_board_review_runs
         SET status = 'historical',
             updated_at = ?2
         WHERE card_id = ?1 AND status IN ('pending', 'running', 'completed', 'failed', 'stale')",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "archive_prior_review_runs",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_review_findings
         SET status = 'historical',
             updated_at = ?2
         WHERE card_id = ?1 AND status IN ('open', 'resolved', 'stale')",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "archive_prior_review_findings",
        source,
    })?;

    let review_status = if verdict == "failed" {
        "failed"
    } else {
        "completed"
    };
    let snap = input
        .snapshot_digest
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(existing.product_snapshot_digest.as_deref());
    conn.execute(
        "INSERT INTO workflow_board_review_runs (
           id, card_id, stage_run_id, thread_id, card_revision, snapshot_digest,
           status, verdict, summary, created_at, completed_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)",
        params![
            review_id,
            card_id,
            input.stage_run_id,
            input.thread_id,
            existing.revision + 1,
            snap,
            review_status,
            verdict,
            input.summary,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_review_run",
        source,
    })?;
    Ok(())
}

fn insert_review_findings_and_finalize_card(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordReviewRunInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let revision_id = input.revision_id.trim();
    let review_id = input.review_id.trim();
    let verdict = input.verdict.trim();
    let mut used_ids: HashSet<String> = HashSet::new();
    for f in input.findings {
        let raw_fid = require_non_empty(f.id, "finding.id")?;
        let sev = require_non_empty(f.severity, "finding.severity")?;
        let msg = require_non_empty(f.message, "finding.message")?;
        if !matches!(sev, "critical" | "major" | "minor" | "info") {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsupported finding severity `{sev}`"
            )));
        }
        let fid = resolve_unique_finding_id(conn, review_id, raw_fid, &mut used_ids)?;
        conn.execute(
            "INSERT INTO workflow_board_review_findings (
               id, card_id, review_run_id, severity, criterion_id, file_path, line,
               message, status, disposition, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', 'unresolved', ?9, ?9)",
            params![
                fid,
                card_id,
                review_id,
                sev,
                f.criterion_id,
                f.file_path,
                f.line,
                msg,
                now
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "insert_review_finding",
            source,
        })?;
    }

    let next_revision = existing.revision + 1;
    let has_open = !input.findings.is_empty();
    let blocker = if verdict == "failed" {
        Some("Independent review failed".to_owned())
    } else if has_open {
        Some(format!(
            "{} unresolved review finding(s)",
            input.findings.len()
        ))
    } else {
        None
    };
    let blocked_next_action = if verdict == "failed" || has_open {
        Some("resolve_finding")
    } else {
        None
    };
    recompute_card_blocker_status(
        conn,
        existing,
        next_revision,
        "ready_for_ship",
        blocker.as_deref(),
        blocked_next_action,
        None,
        now,
    )?;

    let payload_json = json!({
        "reviewId": review_id,
        "verdict": verdict,
        "findingsCount": input.findings.len(),
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "review_recorded",
            summary: &format!("Independent review {review_id} -> {verdict}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}

fn finding_id_exists(conn: &Connection, fid: &str) -> Result<bool, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT 1 FROM workflow_board_review_findings WHERE id = ?1 LIMIT 1",
        params![fid],
        |_| Ok(true),
    )
    .optional()
    .map(|v| v.unwrap_or(false))
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "check_review_finding_id_collision",
        source,
    })
}

fn resolve_unique_finding_id(
    conn: &Connection,
    review_id: &str,
    raw_fid: &str,
    used_ids: &mut HashSet<String>,
) -> Result<String, WorkflowBoardRepoError> {
    if !used_ids.contains(raw_fid) && !finding_id_exists(conn, raw_fid)? {
        used_ids.insert(raw_fid.to_owned());
        return Ok(raw_fid.to_owned());
    }
    let prefixed = format!("{review_id}-{raw_fid}");
    if !used_ids.contains(&prefixed) && !finding_id_exists(conn, &prefixed)? {
        used_ids.insert(prefixed.clone());
        return Ok(prefixed);
    }
    let mut suffix = 2_usize;
    loop {
        let candidate = format!("{prefixed}-{suffix}");
        if !used_ids.contains(&candidate) && !finding_id_exists(conn, &candidate)? {
            used_ids.insert(candidate.clone());
            return Ok(candidate);
        }
        suffix += 1;
    }
}
