#![forbid(unsafe_code)]
//! `previewCliImport` and idempotent `applyCliImport` service implementations.

use std::path::PathBuf;

use legion_contracts::ids::{ProjectId, WorkflowCardId};
use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApplyCliImportResult,
    WorkflowBoardCliImportPolicyStatus, WorkflowBoardCliImportProvenance,
    WorkflowBoardCliImportTaskPreview, WorkflowBoardPreviewCliImportInput,
    WorkflowBoardPreviewCliImportResult, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::project_store::find_project_by_id_or_workspace;
use legion_persistence::workflow_board_repo::{
    ApplyCliImportInput, CliImportApprovalSeed, apply_cli_import_tx, find_cli_import_by_spec,
    get_card_planning_detail, pick_available_card_id,
};
use rusqlite::Connection;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_cli_import_seeds::OwnedCliImportSeeds;
use crate::workflow_board_cli_render::{
    RenderedCliImportArtifacts, blocked_task_id, build_cli_import_artifacts,
    build_cli_import_journal_writes,
};
use crate::workflow_board_cli_restore::has_missing_cli_deliverable;
use crate::workflow_board_cli_scan::{ScannedCliProject, scan_cli_project};
use crate::workflow_board_create::CreateTxGuard;
use crate::workflow_board_fs::{ensure_confined_card_dir, validate_safe_card_id};
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_journal::{
    finalize_staged_files, remove_confined_staging_file, stage_and_write_temp_files,
};
use crate::workflow_board_types::{db_lock_err, enum_as_str, internal, map_repo_err};

fn resolve_project_root(
    conn: &Connection,
    project_id: &ProjectId,
) -> Result<(String, PathBuf), WorkflowBoardRequestError> {
    let proj = find_project_by_id_or_workspace(conn, project_id.as_str())
        .map_err(|e| {
            internal(format!(
                "Failed to resolve project `{}`: {e}",
                project_id.as_str()
            ))
        })?
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ProjectNotFound,
                format!("Project `{}` was not found", project_id.as_str()),
            )
        })?;
    Ok((proj.id, PathBuf::from(proj.workspace_root)))
}

fn derive_candidate_card_id(spec_id: &str) -> String {
    let mut slug = String::from("cli-");
    for ch in spec_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
        } else {
            slug.push('-');
        }
        if slug.len() >= 56 {
            break;
        }
    }
    if slug.len() <= 4 {
        slug.push_str("import");
    }
    slug
}

pub(crate) fn preview_cli_import_impl(
    orchestrator: &Orchestrator,
    input: &WorkflowBoardPreviewCliImportInput,
) -> Result<WorkflowBoardPreviewCliImportResult, WorkflowBoardRequestError> {
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let (resolved_project_id, ws_root) = resolve_project_root(&conn, &input.project_id)?;
    drop(conn);

    let scan = scan_cli_project(&ws_root, input.cli_dir.as_deref())?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let existing = find_cli_import_by_spec(&conn, &resolved_project_id, &scan.spec_id)
        .map_err(map_repo_err)?;
    drop(conn);

    let can_apply = scan.active_locks.is_empty();
    let refusal_reason = if can_apply {
        None
    } else {
        let paths: Vec<&str> = scan
            .active_locks
            .iter()
            .map(|l| l.relative_path.as_str())
            .collect();
        Some(format!(
            "Active CLI run lock conflict detected ({}); refusing takeover while CLI run is active",
            paths.join(", ")
        ))
    };

    let tasks = scan
        .tasks
        .iter()
        .map(|t| WorkflowBoardCliImportTaskPreview {
            id: t.id.clone(),
            task_id: t.id.clone(),
            title: t.title.clone(),
            status: t.status,
            blocker_summary: t.blocker_summary.clone(),
            verification_commands: t.verification_commands.clone(),
            missing_log_notes: t.missing_log_notes.clone(),
        })
        .collect();

    Ok(WorkflowBoardPreviewCliImportResult {
        project_id: ProjectId::new(&resolved_project_id).map_err(|e| internal(e.to_string()))?,
        source_path: scan.source_cli_dir,
        spec_id: scan.spec_id,
        title: scan.title,
        stage: scan.stage,
        status: scan.status,
        existing_card_id: existing
            .as_ref()
            .map(|row| WorkflowCardId::new(&row.card_id))
            .transpose()
            .map_err(|e| internal(e.to_string()))?,
        already_imported: existing.is_some(),
        active_locks: scan.active_locks,
        stale_locks: scan.stale_locks,
        can_apply,
        refusal_reason,
        tasks,
        decision_ids: scan.decisions.iter().map(|d| d.id.clone()).collect(),
        assumption_ids: scan.assumption_ids,
        acceptance_criterion_ids: scan.acceptance_criterion_ids,
        historical_receipts: scan.historical_receipts,
        unsupported_policies: scan.unsupported_policies,
        missing_log_notes: scan.missing_log_notes,
        warnings: scan.warnings,
    })
}

pub(crate) fn apply_cli_import_impl(
    orchestrator: &Orchestrator,
    input: &WorkflowBoardApplyCliImportInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardApplyCliImportResult, WorkflowBoardRequestError> {
    let now_iso = utc_now_iso();
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let (resolved_project_id, ws_root) = resolve_project_root(&conn, &input.project_id)?;
    drop(conn);

    let scan = scan_cli_project(&ws_root, input.cli_dir.as_deref())?;
    if !scan.active_locks.is_empty() {
        let lock_desc: Vec<String> = scan
            .active_locks
            .iter()
            .map(|l| match l.pid {
                Some(p) => format!("{} (pid {p})", l.relative_path),
                None => l.relative_path.clone(),
            })
            .collect();
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            format!(
                "Refusing CLI import takeover while active CLI run lock is held: {}",
                lock_desc.join(", ")
            ),
        ));
    }

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let tx = CreateTxGuard::begin(&conn)?;
    let existing = find_cli_import_by_spec(&conn, &resolved_project_id, &scan.spec_id)
        .map_err(map_repo_err)?;
    let authoritative_card_id = if let Some(ref row) = existing {
        validate_safe_card_id(&row.card_id)?.to_owned()
    } else {
        let base_candidate = derive_candidate_card_id(&scan.spec_id);
        let safe_base = validate_safe_card_id(&base_candidate)?;
        let resolved = pick_available_card_id(&conn, safe_base).map_err(map_repo_err)?;
        validate_safe_card_id(&resolved)?.to_owned()
    };

    let ack_all = input.acknowledge_unsupported_policies == Some(true);
    let ack_keys = if input.acknowledge_unsupported_policies == Some(false) {
        None
    } else {
        input.acknowledged_policies.as_deref()
    };
    let rendered = build_cli_import_artifacts(
        &authoritative_card_id,
        &scan,
        ack_all,
        ack_keys,
        &[],
        &now_iso,
    )?;

    let (outcome, staged) = commit_cli_import_and_files(
        &conn,
        &ws_root,
        tx,
        &resolved_project_id,
        &authoritative_card_id,
        input,
        &scan,
        &rendered,
        &now_iso,
    )?;
    if !staged.is_empty() {
        finalize_staged_files(&conn, &staged, &now_iso)?;
    }

    let card = load_hydrated_card_detail(&conn, &ws_root, &outcome.card_id)?;
    drop(conn);

    if outcome.created || !outcome.reused {
        notify_project(&resolved_project_id);
    }

    let revision = u64::try_from(outcome.revision.max(1)).unwrap_or(1);
    let (source_spec_id, historical_receipts, unsupported_policies, missing_log_notes) =
        match card.cli_import.as_ref() {
            Some(prov) => (
                prov.source_spec_id.clone(),
                prov.historical_receipts.clone(),
                prov.unsupported_policies.clone(),
                prov.missing_log_notes.clone(),
            ),
            None => (
                scan.spec_id,
                rendered.provenance.historical_receipts,
                rendered.provenance.unsupported_policies,
                rendered.provenance.missing_log_notes,
            ),
        };
    Ok(WorkflowBoardApplyCliImportResult {
        card_id: card.summary.card_id.clone(),
        project_id: card.summary.project_id.clone(),
        revision,
        created: outcome.created,
        source_spec_id,
        tasks: card.tasks.clone(),
        historical_receipts,
        unsupported_policies,
        missing_log_notes,
        card,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates transactional SQLite CLI import upsert and write-journaled deliverable staging"
)]
fn commit_cli_import_and_files(
    conn: &Connection,
    ws_root: &std::path::Path,
    tx: CreateTxGuard<'_>,
    project_id: &str,
    authoritative_card_id: &str,
    input: &WorkflowBoardApplyCliImportInput,
    scan: &ScannedCliProject,
    rendered: &RenderedCliImportArtifacts,
    now_iso: &str,
) -> Result<
    (
        legion_persistence::workflow_board_repo::ApplyCliImportOutcome,
        Vec<crate::workflow_board_journal::StagedEntry>,
    ),
    WorkflowBoardRequestError,
> {
    let ws_strategy = input
        .workspace_strategy
        .clone()
        .unwrap_or(WorkspaceStrategy::Root { branch: None });
    let ws_json = serde_json::to_string(&ws_strategy).map_err(|e| internal(e.to_string()))?;
    let model_json = input
        .model_selection
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;
    let rt_str = input.runtime_mode.and_then(|r| enum_as_str(&r));
    let im_str = input.interaction_mode.and_then(|m| enum_as_str(&m));
    let stage_str = enum_as_str(&scan.stage).unwrap_or_else(|| "execute".to_owned());
    let status_str = enum_as_str(&scan.status).unwrap_or_else(|| "blocked".to_owned());
    let current_task_id =
        blocked_task_id(scan).or_else(|| scan.tasks.first().map(|t| t.id.clone()));
    let blocker_reason = scan
        .execution_state
        .blocker
        .clone()
        .or_else(|| scan.tasks.iter().find_map(|t| t.blocker_summary.clone()));
    let acked_at = blocking_policies_acked_at(rendered, now_iso);
    let owned = OwnedCliImportSeeds::build(scan, rendered, current_task_id.as_deref());
    let decisions = owned.decision_seeds();
    let tasks = owned.task_seeds(&rendered.tasks_digest);
    let planned_checks = owned.planned_check_seeds();
    let check_receipts = owned.check_receipt_seeds();
    let ac_evidence = owned.ac_evidence_seeds();
    let historical_approvals = historical_approval_seeds(scan);
    let rev_id = format!("rev-cli-{}", Uuid::new_v4().simple());
    let next_action = derive_cli_next_action(rendered, current_task_id.as_deref());

    let outcome = apply_cli_import_tx(
        conn,
        &ApplyCliImportInput {
            candidate_card_id: authoritative_card_id,
            revision_id: &rev_id,
            project_id,
            source_cli_spec_id: &scan.spec_id,
            source_cli_dir: &scan.source_cli_dir,
            source_digest: &scan.source_digest,
            title: &scan.title,
            stage: &stage_str,
            status: &status_str,
            brief: &scan.brief,
            workspace_strategy_json: &ws_json,
            worktree_path: None,
            branch: None,
            model_selection_json: model_json.as_deref(),
            runtime_mode: rt_str.as_deref(),
            interaction_mode: im_str.as_deref(),
            spec_digest: &rendered.spec_digest,
            design_digest: Some(&rendered.design_digest),
            plan_digest: Some(&rendered.plan_digest),
            tasks_digest: Some(&rendered.tasks_digest),
            current_task_id: current_task_id.as_deref(),
            blocker_reason: blocker_reason.as_deref(),
            next_action: Some(next_action),
            spec_json: &rendered.spec_json,
            plan_json: Some(&rendered.plan_json),
            unsupported_policies_json: &rendered.unsupported_policies_json,
            acknowledged_policies_json: &rendered.acknowledged_policies_json,
            policies_acknowledged_at: acked_at,
            assurance_policy_json: scan.assurance_policy_json.as_deref(),
            import_provenance_json: &rendered.provenance_json,
            decisions: &decisions,
            tasks: &tasks,
            planned_checks: &planned_checks,
            check_receipts: &check_receipts,
            ac_evidence: &ac_evidence,
            historical_approvals: &historical_approvals,
            historical_review_summary: scan.plan_review_markdown.as_deref(),
            historical_review_at: scan.execution_state.updated_at.as_deref(),
        },
        now_iso,
    )
    .map_err(map_repo_err)?;
    if outcome.card_id != authoritative_card_id {
        return Err(internal(format!(
            "CLI import allocated unexpected card_id `{}` (expected `{authoritative_card_id}`)",
            outcome.card_id
        )));
    }

    let staged = stage_cli_deliverables_and_commit(
        conn,
        ws_root,
        tx,
        &outcome,
        &scan.title,
        rendered,
        now_iso,
    )?;

    Ok((outcome, staged))
}

fn blocking_policies_acked_at<'a>(
    rendered: &RenderedCliImportArtifacts,
    now_iso: &'a str,
) -> Option<&'a str> {
    let has_blocking = rendered
        .provenance
        .unsupported_policies
        .iter()
        .any(|p| p.blocks_execution);
    let all_blocking_acked = rendered
        .provenance
        .unsupported_policies
        .iter()
        .filter(|p| p.blocks_execution)
        .all(|p| p.status == WorkflowBoardCliImportPolicyStatus::Acknowledged);
    (has_blocking && all_blocking_acked).then_some(now_iso)
}

fn historical_approval_seeds(scan: &ScannedCliProject) -> Vec<CliImportApprovalSeed<'_>> {
    scan.approvals
        .iter()
        .map(|a| CliImportApprovalSeed {
            id: &a.id,
            gate: &a.gate,
            artifact_digest: &a.artifact_digest,
            spec_digest: a.spec_digest.as_deref(),
            plan_digest: a.plan_digest.as_deref(),
            tasks_digest: a.tasks_digest.as_deref(),
            approved_by: &a.approved_by,
            approved_at: &a.approved_at,
            metadata_json: &a.metadata_json,
        })
        .collect()
}

fn derive_cli_next_action(
    rendered: &RenderedCliImportArtifacts,
    current_task_id: Option<&str>,
) -> &'static str {
    if rendered.provenance.unsupported_policies.iter().any(|p| {
        p.blocks_execution
            && p.status
                == legion_contracts::workflow_board::WorkflowBoardCliImportPolicyStatus::Unsupported
    }) {
        "acknowledge_policies"
    } else if current_task_id.is_some() {
        "resolve_blocked_task"
    } else {
        "start_execute"
    }
}

fn stage_cli_deliverables_and_commit(
    conn: &Connection,
    ws_root: &std::path::Path,
    tx: CreateTxGuard<'_>,
    outcome: &legion_persistence::workflow_board_repo::ApplyCliImportOutcome,
    fallback_title: &str,
    rendered: &RenderedCliImportArtifacts,
    now_iso: &str,
) -> Result<Vec<crate::workflow_board_journal::StagedEntry>, WorkflowBoardRequestError> {
    let card_dir = ensure_confined_card_dir(ws_root, &outcome.card_id)?;
    let preserve_existing_deliverables =
        outcome.policy_ack_only || outcome.reused || outcome.ownership_preserved;
    let planning = if preserve_existing_deliverables {
        get_card_planning_detail(conn, &outcome.card_id).ok()
    } else {
        None
    };
    let missing_deliverable = outcome.card_detail.card.external_edit_state.as_deref()
        != Some("deliverable_stale")
        && has_missing_cli_deliverable(
            &card_dir,
            &outcome.card_detail.card,
            planning.as_ref(),
            &rendered.plan_doc.tasks,
        );
    let needs_disk_write = (!outcome.reused && !outcome.ownership_preserved)
        || !card_dir.join("card.json").is_file()
        || !card_dir.join("import-provenance.json").is_file()
        || missing_deliverable;

    let staged = if needs_disk_write {
        let authoritative_prov: Option<WorkflowBoardCliImportProvenance> =
            serde_json::from_str(&outcome.cli_import.import_provenance_json).ok();
        let writes = build_cli_import_journal_writes(
            outcome,
            planning.as_ref(),
            fallback_title,
            rendered,
            authoritative_prov.as_ref(),
            &card_dir,
            preserve_existing_deliverables,
        )?;
        stage_and_write_temp_files(conn, &outcome.card_id, &card_dir, &writes, now_iso)?
    } else {
        Vec::new()
    };

    if let Err(err) = tx.commit() {
        for entry in &staged {
            remove_confined_staging_file(&entry.card_dir, &entry.staging_path);
        }
        return Err(err);
    }
    Ok(staged)
}
