#![forbid(unsafe_code)]
//! Startup write-journal recovery and external disk edit reconciliation.

use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::workflow_board::{
    WorkflowBoardPlanDocument, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_persistence::workflow_board_repo::types::WriteJournalRow;
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_persistence::workflow_board_repo::{
    commit_journal, get_card_detail, get_card_planning_detail, get_cli_import_by_card,
    invalidate_plan_approval, invalidate_spec_approval, list_incomplete_journal_entries,
    mark_file_written, rollback_journal, update_card_external_edit_state,
};
use legion_workflows::sha256_prefixed;
use rusqlite::Connection;

use crate::workflow_board_fs::{
    resolve_card_workspace_root, resolve_confined_card_dir_if_exists, validate_safe_card_id,
};
use crate::workflow_board_hydration_plan::hydrate_plan_document;
use crate::workflow_board_journal::{
    ensure_confined_deliverable_target, read_confined_digest_if_file, remove_confined_staging_file,
    sweep_tmp_files,
};
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_render_plan::render_task_contract_markdown;
use crate::workflow_board_types::{internal, map_repo_err};

/// Summary of startup write-journal reconciliation actions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteJournalRecoveryReport {
    /// Journal entries forward-completed to `'committed'`.
    pub committed: usize,
    /// Incomplete or torn journal entries rolled back to `'rolled_back'`.
    pub rolled_back: usize,
    /// Orphaned `.tmp` files removed from `.legion/workflows/<card-id>/`.
    pub temp_files_cleaned: usize,
}

/// Reconciles all incomplete `workflow_board_write_journal` rows (`pending`, `staged`, `file_written`, `failed`)
/// and sweeps orphaned `*.tmp` files inside card deliverable directories.
pub fn reconcile_incomplete_write_journal(
    conn: &Connection,
    now_iso: &str,
) -> Result<WriteJournalRecoveryReport, WorkflowBoardRequestError> {
    let mut report = WriteJournalRecoveryReport::default();
    let entries = list_incomplete_journal_entries(conn, None).map_err(map_repo_err)?;
    for entry in entries {
        reconcile_single_entry(conn, &entry, now_iso, &mut report)?;
    }

    let mut stmt = conn
        .prepare("SELECT card_id, worktree_path FROM workflow_board_cards")
        .map_err(|e| internal(format!("Failed to query workflow board cards: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| internal(format!("Failed to iterate workflow board cards: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(format!("Failed to collect workflow board cards: {e}")))?;

    for (card_id, worktree_path) in rows {
        if validate_safe_card_id(&card_id).is_err() {
            continue;
        }
        let ws_root = match worktree_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(wt) => PathBuf::from(wt),
            None => match get_card_detail(conn, &card_id)
                .ok()
                .and_then(|d| resolve_card_workspace_root(conn, &d.card).ok())
            {
                Some(p) => p,
                None => continue,
            },
        };
        if let Ok(Some(card_dir)) = resolve_confined_card_dir_if_exists(&ws_root, &card_id) {
            report.temp_files_cleaned += sweep_tmp_files(&card_dir, 0);
        }
    }

    Ok(report)
}

fn reconcile_single_entry(
    conn: &Connection,
    entry: &WriteJournalRow,
    now_iso: &str,
    report: &mut WriteJournalRecoveryReport,
) -> Result<(), WorkflowBoardRequestError> {
    let target_path = PathBuf::from(&entry.target_path);
    let staging_path = entry
        .staging_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    let planning = get_card_planning_detail(conn, &entry.card_id).ok();
    let confined_card_dir = planning
        .as_ref()
        .and_then(|p| resolve_card_workspace_root(conn, &p.card_detail.card).ok())
        .and_then(|ws| resolve_confined_card_dir_if_exists(&ws, &entry.card_id).ok())
        .flatten();
    let expected_digest = entry
        .expected_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(exp_digest) =
        expected_digest.filter(|_| !matches!(entry.status.as_str(), "pending" | "failed"))
    else {
        return rollback_entry(
            conn,
            &entry.id,
            confined_card_dir.as_deref(),
            staging_path.as_deref(),
            "rolled back incomplete write-journal entry on startup",
            now_iso,
            report,
        );
    };
    let db_committed = planning.as_ref().is_some_and(|p| {
        is_journal_entry_committed_in_db(&entry.target_path, exp_digest, entry.revision, p)
    });

    if db_committed && let Some(ref card_dir) = confined_card_dir {
        if read_confined_digest_if_file(card_dir, &target_path)
            .ok()
            .flatten()
            .as_deref()
            == Some(exp_digest)
        {
            if let Some(ref st) = staging_path {
                remove_confined_staging_file(card_dir, st);
            }
            commit_journal(conn, &entry.id, now_iso).map_err(map_repo_err)?;
            report.committed += 1;
            return Ok(());
        }
        if entry.status == "staged"
            && let Some(ref st) = staging_path
            && read_confined_digest_if_file(card_dir, st)
                .ok()
                .flatten()
                .as_deref()
                == Some(exp_digest)
            && let Ok(safe_st) = ensure_confined_deliverable_target(card_dir, st, false)
            && let Ok(safe_target) =
                ensure_confined_deliverable_target(card_dir, &target_path, true)
            && fs::rename(&safe_st, &safe_target).is_ok()
        {
            mark_file_written(conn, &entry.id, now_iso).map_err(map_repo_err)?;
            commit_journal(conn, &entry.id, now_iso).map_err(map_repo_err)?;
            report.committed += 1;
            return Ok(());
        }
    }

    rollback_entry(
        conn,
        &entry.id,
        confined_card_dir.as_deref(),
        staging_path.as_deref(),
        "rolled back uncommitted write-journal entry on startup",
        now_iso,
        report,
    )
}

fn rollback_entry(
    conn: &Connection,
    journal_id: &str,
    card_dir: Option<&Path>,
    staging_path: Option<&Path>,
    reason: &str,
    now_iso: &str,
    report: &mut WriteJournalRecoveryReport,
) -> Result<(), WorkflowBoardRequestError> {
    if let (Some(dir), Some(st)) = (card_dir, staging_path) {
        remove_confined_staging_file(dir, st);
    }
    rollback_journal(conn, journal_id, Some(reason), now_iso).map_err(map_repo_err)?;
    report.rolled_back += 1;
    Ok(())
}

fn is_journal_entry_committed_in_db(
    target_path: &str,
    expected_digest: &str,
    entry_revision: Option<i64>,
    planning: &CardPlanningDetail,
) -> bool {
    let detail = &planning.card_detail;
    let normalized = target_path.replace('\\', "/");
    if normalized.ends_with("/spec.md") {
        return detail.card.spec_digest.as_deref() == Some(expected_digest)
            || detail
                .revisions
                .iter()
                .any(|r| r.spec_digest.as_deref() == Some(expected_digest));
    }
    if normalized.ends_with("/spec-brief.md") || normalized.ends_with("/plan-brief.md") {
        return detail
            .stage_runs
            .iter()
            .any(|sr| sr.brief_digest.as_deref() == Some(expected_digest));
    }
    if normalized.ends_with("/design.md") {
        return detail.card.design_digest.as_deref() == Some(expected_digest)
            || planning
                .plan_design
                .as_ref()
                .and_then(|d| d.design_digest.as_deref())
                == Some(expected_digest);
    }
    if normalized.ends_with("/plan.md") {
        return detail.card.plan_digest.as_deref() == Some(expected_digest)
            || detail
                .revisions
                .iter()
                .any(|r| r.plan_digest.as_deref() == Some(expected_digest));
    }
    if normalized.contains("/tasks/")
        && std::path::Path::new(&normalized)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        return entry_revision.is_none_or(|rev| detail.card.revision >= rev)
            && planning
                .tasks
                .iter()
                .any(|t| normalized.ends_with(&format!("/tasks/{}.md", t.task_id)));
    }
    let custom_wf_match = parse_stored_spec_fields(&detail.card.spec_json)
        .wireframes_index
        .as_deref()
        .map(|idx| idx.trim().replace('\\', "/"))
        .is_some_and(|idx| {
            !idx.is_empty() && normalized.ends_with(&format!("/{}/{idx}", detail.card.card_id))
        });
    if normalized.contains("/wireframes/") || custom_wf_match {
        return detail.card.wireframes_digest.as_deref() == Some(expected_digest)
            || detail
                .approvals
                .iter()
                .any(|a| a.wireframes_digest.as_deref() == Some(expected_digest));
    }
    if normalized.ends_with("/card.json") {
        return entry_revision.is_none_or(|rev| detail.card.revision >= rev);
    } else if normalized.ends_with("/import-provenance.json") {
        return detail.card.import_provenance_json.is_some()
            && entry_revision.is_none_or(|rev| detail.card.revision >= rev);
    }
    false
}

fn confined_digest_matches(
    card_dir: Option<&Path>,
    rel_key: &str,
    expected: &str,
    propagate_err: bool,
) -> Result<bool, WorkflowBoardRequestError> {
    let Some(dir) = card_dir else {
        return Ok(false);
    };
    let target = rel_key.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
    let actual = if propagate_err {
        read_confined_digest_if_file(dir, &target)?
    } else {
        read_confined_digest_if_file(dir, &target).ok().flatten()
    };
    Ok(actual.as_deref() == Some(expected))
}

struct ActivePlanExpected<'a> {
    plan_digest: Option<&'a str>,
}

fn active_spec_expected_digests(
    detail: &legion_persistence::workflow_board_repo::types::CardDetail,
    is_cli_imported: bool,
) -> Option<(Option<&str>, Option<&str>)> {
    if let Some(valid_spec) = detail
        .approvals
        .iter()
        .find(|a| a.gate == "spec" && a.status == "valid")
    {
        let expected_spec = valid_spec
            .spec_digest
            .as_deref()
            .unwrap_or(valid_spec.artifact_digest.as_str());
        return Some((Some(expected_spec), valid_spec.wireframes_digest.as_deref()));
    }
    if !is_cli_imported
        || detail
            .approvals
            .iter()
            .any(|a| a.gate == "spec" && matches!(a.status.as_str(), "stale" | "superseded"))
    {
        return None;
    }
    detail
        .approvals
        .iter()
        .any(|a| a.gate == "spec" && a.status == "historical")
        .then_some((
            detail.card.spec_digest.as_deref(),
            detail.card.wireframes_digest.as_deref(),
        ))
}

fn active_plan_expected_digest(
    detail: &legion_persistence::workflow_board_repo::types::CardDetail,
    is_cli_imported: bool,
) -> Option<ActivePlanExpected<'_>> {
    if let Some(valid_plan) = detail
        .approvals
        .iter()
        .find(|a| a.gate == "plan" && a.status == "valid")
    {
        let expected_plan = valid_plan
            .plan_digest
            .as_deref()
            .unwrap_or(valid_plan.artifact_digest.as_str());
        return Some(ActivePlanExpected {
            plan_digest: Some(expected_plan),
        });
    }
    if !is_cli_imported
        || detail
            .approvals
            .iter()
            .any(|a| a.gate == "plan" && matches!(a.status.as_str(), "stale" | "superseded"))
    {
        return None;
    }
    detail
        .approvals
        .iter()
        .any(|a| a.gate == "plan" && a.status == "historical")
        .then_some(ActivePlanExpected {
            plan_digest: detail.card.plan_digest.as_deref(),
        })
}

/// Verifies `spec.md`, bound wireframes, `plan.md`, `design.md`, and `tasks/*.md` on disk against
/// active (`'valid'`, or CLI-imported `'historical'`) approvals, marking affected approvals `'stale'`
/// if missing or externally modified.
pub(crate) fn reconcile_card_external_edits(
    conn: &Connection,
    card_id: &str,
    now_iso: &str,
) -> Result<bool, WorkflowBoardRequestError> {
    let planning = get_card_planning_detail(conn, card_id).map_err(map_repo_err)?;
    let detail = &planning.card_detail;
    let is_cli_imported = get_cli_import_by_card(conn, card_id)
        .map_err(map_repo_err)?
        .is_some();
    let ws_root = resolve_card_workspace_root(conn, &detail.card)?;
    let card_dir = resolve_confined_card_dir_if_exists(&ws_root, card_id)?;
    let dir_ref = card_dir.as_deref();
    let mut invalidated = false;

    if let Some((expected_spec, expected_wf)) =
        active_spec_expected_digests(detail, is_cli_imported)
    {
        let spec_matches = match expected_spec {
            Some(dg) => confined_digest_matches(dir_ref, "spec.md", dg, true)?,
            None => true,
        };
        let wf_matches = match expected_wf {
            Some(wf_digest) => {
                let fields = parse_stored_spec_fields(&detail.card.spec_json);
                let wf_key = fields
                    .wireframes_index
                    .as_deref()
                    .unwrap_or("wireframes/index.html");
                confined_digest_matches(dir_ref, wf_key, wf_digest, false)?
            }
            None => true,
        };
        if !spec_matches || !wf_matches {
            invalidated |= invalidate_spec_approval(conn, card_id, "external_spec_edit", now_iso)
                .map_err(map_repo_err)?;
            if invalidated {
                let _ = update_card_external_edit_state(
                    conn,
                    card_id,
                    "deliverable_stale",
                    None,
                    now_iso,
                );
            }
            return Ok(invalidated);
        }
    }

    if let Some(expected_plan) = active_plan_expected_digest(detail, is_cli_imported) {
        let plan_matches = match expected_plan.plan_digest {
            Some(dg) => confined_digest_matches(dir_ref, "plan.md", dg, true)?,
            None => true,
        };
        let design_matches = match detail.card.design_digest.as_deref() {
            Some(dg) => confined_digest_matches(dir_ref, "design.md", dg, true)?,
            None => true,
        };
        let tasks_match =
            dir_ref.is_some_and(|d| verify_disk_task_contracts(d, card_id, &planning));
        if !plan_matches || !design_matches || !tasks_match {
            invalidated |= invalidate_plan_approval(conn, card_id, "external_plan_edit", now_iso)
                .map_err(map_repo_err)?;
        }
    }

    if invalidated {
        let _ = update_card_external_edit_state(conn, card_id, "deliverable_stale", None, now_iso);
    }
    Ok(invalidated)
}

pub(crate) fn verify_disk_task_contracts(
    card_dir: &Path,
    card_id: &str,
    planning: &CardPlanningDetail,
) -> bool {
    let stored: Option<WorkflowBoardPlanDocument> = planning
        .card_detail
        .card
        .plan_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let tasks = match stored {
        Some(doc) if !doc.tasks.is_empty() => doc.tasks,
        _ => hydrate_plan_document(planning)
            .ok()
            .flatten()
            .map_or_else(Vec::new, |d| d.tasks),
    };
    tasks.iter().all(|task| {
        let expected_md = render_task_contract_markdown(card_id, task);
        let expected_dg = sha256_prefixed(expected_md.as_bytes());
        let task_path = card_dir.join("tasks").join(format!("{}.md", task.task_id));
        read_confined_digest_if_file(card_dir, &task_path)
            .ok()
            .flatten()
            .as_deref()
            == Some(expected_dg.as_str())
    })
}

/// Verifies `spec.md` on disk matches `expected_digest` before `approveSpec` proceeds.
pub(crate) fn ensure_disk_spec_matches(
    conn: &Connection,
    card_id: &str,
    expected_digest: &str,
    now_iso: &str,
) -> Result<(), WorkflowBoardRequestError> {
    let detail = get_card_detail(conn, card_id).map_err(map_repo_err)?;
    let ws_root = resolve_card_workspace_root(conn, &detail.card)?;
    let confined_card_dir = resolve_confined_card_dir_if_exists(&ws_root, card_id)?;
    let disk_digest = match confined_card_dir.as_deref() {
        Some(card_dir) => read_confined_digest_if_file(card_dir, &card_dir.join("spec.md"))?,
        None => None,
    };
    if disk_digest.as_deref() != Some(expected_digest) {
        let _ = invalidate_spec_approval(conn, card_id, "external_spec_edit", now_iso);
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::SpecNotApproved,
            "spec.md on disk does not match the persisted spec digest; save updated spec before approving",
        ));
    }
    Ok(())
}
