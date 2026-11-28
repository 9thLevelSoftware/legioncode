#![forbid(unsafe_code)]
//! Read-only `.legion-cli/` directory scanner for `previewCliImport` and `applyCliImport`.

use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardCardStatus,
    WorkflowBoardCliImportHistoricalReceipt, WorkflowBoardCliImportHistoricalReceiptKind,
    WorkflowBoardCliImportHistoricalStatus, WorkflowBoardCliImportLockEntry,
    WorkflowBoardCliImportMissingLogNote, WorkflowBoardCliImportUnsupportedPolicy,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStage,
    WorkflowBoardTaskStatus, WorkflowBoardTaskTestingMethod, is_valid_input_path,
};
use legion_workflows::sha256_prefixed;
use serde_json::Value;

use crate::workflow_board_cli_locks::scan_cli_locks;
use crate::workflow_board_cli_scan_meta::{
    ScannedDecision, ScannedExecutionState, parse_execution_state, read_markdown_file,
    read_yaml_file, scan_decisions_and_assumptions,
};
use crate::workflow_board_cli_scan_tasks::{
    build_historical_approvals, discover_first_spec_dir, extract_spec_content, scan_tasks,
    str_field,
};
use crate::workflow_board_fs::is_symlink_or_reparse_point;

#[derive(Debug, Clone)]
pub(crate) struct ScannedTask {
    pub id: String,
    pub title: String,
    pub status: WorkflowBoardTaskStatus,
    pub priority: String,
    pub parent_id: Option<String>,
    pub blocked_by: Vec<String>,
    pub blocks: Vec<String>,
    pub files_allowed: Vec<String>,
    pub files_forbidden: Vec<String>,
    pub expected_artifacts: Vec<String>,
    pub verification_commands: Vec<Vec<String>>,
    pub testing_method: WorkflowBoardTaskTestingMethod,
    pub acceptance_ids: Vec<String>,
    pub blocker_summary: Option<String>,
    pub notes: Option<String>,
    pub max_files_touched: Option<i64>,
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScannedApproval {
    pub id: String,
    pub gate: String,
    pub artifact_digest: String,
    pub spec_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub tasks_digest: Option<String>,
    pub approved_by: String,
    pub approved_at: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ScannedCliProject {
    pub source_cli_dir: String,
    pub source_digest: String,
    pub spec_id: String,
    pub title: String,
    pub brief: String,
    pub stage: WorkflowBoardStage,
    pub status: WorkflowBoardCardStatus,
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    pub constraints: Vec<String>,
    pub source_refs: Vec<String>,
    pub decisions: Vec<ScannedDecision>,
    pub assumption_ids: Vec<String>,
    pub acceptance_criterion_ids: Vec<String>,
    pub tasks: Vec<ScannedTask>,
    pub approvals: Vec<ScannedApproval>,
    pub historical_receipts: Vec<WorkflowBoardCliImportHistoricalReceipt>,
    pub unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    pub assurance_policy_summary: Option<String>,
    pub assurance_policy_json: Option<String>,
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
    pub active_locks: Vec<WorkflowBoardCliImportLockEntry>,
    pub stale_locks: Vec<WorkflowBoardCliImportLockEntry>,
    pub preserved_artifact_paths: Vec<String>,
    pub execution_state: ScannedExecutionState,
    pub plan_design_markdown: Option<String>,
    pub plan_review_markdown: Option<String>,
    pub warnings: Vec<String>,
}

pub(crate) fn scan_cli_project(
    ws_root: &Path,
    raw_cli_dir: Option<&str>,
) -> Result<ScannedCliProject, WorkflowBoardRequestError> {
    let (cli_rel, cli_root) = resolve_cli_root(ws_root, raw_cli_dir)?;
    let (active_locks, stale_locks) = scan_cli_locks(&cli_root, &cli_rel)?;
    let mut digest_parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut preserved = Vec::new();
    let mut warnings = Vec::new();
    if !stale_locks.is_empty() {
        warnings.push(format!(
            "Detected {} stale dead-PID lock(s)",
            stale_locks.len()
        ));
    }

    let config_val = read_yaml_file(
        &cli_root.join("config.yaml"),
        &format!("{cli_rel}/config.yaml"),
        &mut digest_parts,
    )?;
    if config_val.is_some() {
        preserved.push(format!("{cli_rel}/config.yaml"));
    }
    let (unsupported_policies, assurance_policy_summary, assurance_policy_json) =
        crate::workflow_board_cli_scan_meta::scan_policies(
            &cli_root,
            &cli_rel,
            config_val.as_ref(),
            &mut digest_parts,
        )?;

    let exec_rel = format!("{cli_rel}/workflow/execution.yaml");
    let exec_val = read_yaml_file(
        &cli_root.join("workflow").join("execution.yaml"),
        &exec_rel,
        &mut digest_parts,
    )?;
    if exec_val.is_some() {
        preserved.push(exec_rel);
    }
    let (execution_state, exec_missing_note, exec_receipt) =
        parse_execution_state(ws_root, &cli_rel, exec_val.as_ref());

    let spec_apr_rel = format!("{cli_rel}/workflow/spec-approval.yaml");
    let spec_apr_val = read_yaml_file(
        &cli_root.join("workflow").join("spec-approval.yaml"),
        &spec_apr_rel,
        &mut digest_parts,
    )?;
    if spec_apr_val.is_some() {
        preserved.push(spec_apr_rel.clone());
    }

    let plan_apr_rel = format!("{cli_rel}/workflow/plan-approval.yaml");
    let plan_apr_val = read_yaml_file(
        &cli_root.join("workflow").join("plan-approval.yaml"),
        &plan_apr_rel,
        &mut digest_parts,
    )?;
    if plan_apr_val.is_some() {
        preserved.push(plan_apr_rel.clone());
    }

    let spec_id = execution_state
        .spec_id
        .clone()
        .or_else(|| str_field(plan_apr_val.as_ref(), "specId"))
        .or_else(|| str_field(spec_apr_val.as_ref(), "specId"))
        .or_else(|| discover_first_spec_dir(&cli_root))
        .unwrap_or_else(|| "legion-cli-project".to_owned());

    finish_scan_cli_project(ScanContext {
        cli_root: &cli_root,
        cli_rel,
        spec_id,
        execution_state,
        exec_missing_note,
        exec_receipt,
        spec_apr_rel,
        spec_apr_val,
        plan_apr_rel,
        plan_apr_val,
        unsupported_policies,
        assurance_policy_summary,
        assurance_policy_json,
        active_locks,
        stale_locks,
        preserved,
        warnings,
        digest_parts,
    })
}

fn resolve_cli_root(
    ws_root: &Path,
    raw_cli_dir: Option<&str>,
) -> Result<(String, std::path::PathBuf), WorkflowBoardRequestError> {
    let cli_rel = raw_cli_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(".legion-cli")
        .replace('\\', "/");
    if !is_valid_input_path(&cli_rel) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            format!("Invalid cliDir path `{cli_rel}`"),
        ));
    }
    let mut cli_root = ws_root.to_path_buf();
    for segment in cli_rel.split('/') {
        cli_root.push(segment);
        let meta = fs::symlink_metadata(&cli_root).map_err(|_| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ArtifactNotFound,
                format!("CLI project directory `{cli_rel}` was not found (missing or absent under workspace root)"),
            )
        })?;
        if is_symlink_or_reparse_point(&meta) || !meta.is_dir() {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                format!("CLI project path `{cli_rel}` is not a valid directory"),
            ));
        }
    }
    Ok((cli_rel, cli_root))
}

struct ScanContext<'a> {
    cli_root: &'a Path,
    cli_rel: String,
    spec_id: String,
    execution_state: ScannedExecutionState,
    exec_missing_note: Option<WorkflowBoardCliImportMissingLogNote>,
    exec_receipt: Option<WorkflowBoardCliImportHistoricalReceipt>,
    spec_apr_rel: String,
    spec_apr_val: Option<Value>,
    plan_apr_rel: String,
    plan_apr_val: Option<Value>,
    unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    assurance_policy_summary: Option<String>,
    assurance_policy_json: Option<String>,
    active_locks: Vec<WorkflowBoardCliImportLockEntry>,
    stale_locks: Vec<WorkflowBoardCliImportLockEntry>,
    preserved: Vec<String>,
    warnings: Vec<String>,
    digest_parts: Vec<(String, Vec<u8>)>,
}

fn read_spec_markdown_with_fallback(
    ctx: &mut ScanContext<'_>,
) -> Result<Option<(Option<Value>, String)>, WorkflowBoardRequestError> {
    let cli_rel = &ctx.cli_rel;
    let spec_id = &ctx.spec_id;
    let spec_primary_path = ctx.cli_root.join("specs").join(spec_id).join("SPEC.md");
    let spec_primary_rel = format!("{cli_rel}/specs/{spec_id}/SPEC.md");
    if let Some(found) =
        read_markdown_file(&spec_primary_path, &spec_primary_rel, &mut ctx.digest_parts)?
    {
        ctx.preserved.push(spec_primary_rel);
        return Ok(Some(found));
    }
    let prd_path = ctx.cli_root.join("specs").join(spec_id).join("prd.md");
    let prd_rel = format!("{cli_rel}/specs/{spec_id}/prd.md");
    let prd = read_markdown_file(&prd_path, &prd_rel, &mut ctx.digest_parts)?;
    if prd.is_some() {
        ctx.preserved.push(prd_rel);
    }
    Ok(prd)
}

fn read_plan_design_with_fallback(
    ctx: &mut ScanContext<'_>,
) -> Result<Option<String>, WorkflowBoardRequestError> {
    let cli_rel = &ctx.cli_rel;
    let spec_id = &ctx.spec_id;
    let design_primary_rel = format!("{cli_rel}/plans/{spec_id}/design.md");
    if let Some((_, body)) = read_markdown_file(
        &ctx.cli_root.join("plans").join(spec_id).join("design.md"),
        &design_primary_rel,
        &mut ctx.digest_parts,
    )? {
        ctx.preserved.push(design_primary_rel);
        return Ok(Some(body));
    }
    let fallback_rel = format!("{cli_rel}/plans/{spec_id}.md");
    if let Some((_, body)) = read_markdown_file(
        &ctx.cli_root.join("plans").join(format!("{spec_id}.md")),
        &fallback_rel,
        &mut ctx.digest_parts,
    )? {
        ctx.preserved.push(fallback_rel);
        return Ok(Some(body));
    }
    Ok(None)
}

fn finish_scan_cli_project(
    mut ctx: ScanContext<'_>,
) -> Result<ScannedCliProject, WorkflowBoardRequestError> {
    let spec_file = read_spec_markdown_with_fallback(&mut ctx)?;
    let cli_rel = &ctx.cli_rel;
    let spec_id = &ctx.spec_id;

    let (title, brief, acceptance_criteria, constraints) = extract_spec_content(
        spec_id,
        spec_file.as_ref().and_then(|(fm, _)| fm.as_ref()),
        spec_file.as_ref().map(|(_, b)| b.as_str()),
    );

    let (decisions, assumption_ids, mut dec_paths) =
        scan_decisions_and_assumptions(ctx.cli_root, cli_rel, &mut ctx.digest_parts)?;
    ctx.preserved.append(&mut dec_paths);

    let (mut tasks, mut task_paths) = scan_tasks(
        ctx.cli_root,
        cli_rel,
        &ctx.execution_state,
        ctx.exec_missing_note.as_ref(),
        &mut ctx.digest_parts,
    )?;
    ctx.preserved.append(&mut task_paths);

    let missing_log_notes = collect_missing_log_notes(&mut tasks, ctx.exec_missing_note.take());
    let acceptance_criterion_ids = extract_ac_ids(ctx.plan_apr_val.as_ref(), &acceptance_criteria);

    let (approvals, mut historical_receipts) = build_historical_approvals(
        &ctx.spec_apr_rel,
        ctx.spec_apr_val.as_ref(),
        &ctx.plan_apr_rel,
        ctx.plan_apr_val.as_ref(),
    );
    if let Some(er) = ctx.exec_receipt.take() {
        historical_receipts.push(er);
    }

    let plan_design_markdown = read_plan_design_with_fallback(&mut ctx)?;
    let cli_rel = &ctx.cli_rel;
    let spec_id = &ctx.spec_id;
    let plan_review_markdown = read_markdown_file(
        &ctx.cli_root.join("plans").join(spec_id).join("review.md"),
        &format!("{cli_rel}/plans/{spec_id}/review.md"),
        &mut ctx.digest_parts,
    )?
    .map(|(_, b)| b);
    if plan_review_markdown.is_some() {
        historical_receipts.push(WorkflowBoardCliImportHistoricalReceipt {
            id: "hist-review-1".to_owned(),
            kind: WorkflowBoardCliImportHistoricalReceiptKind::Review,
            status: WorkflowBoardCliImportHistoricalStatus::Historical,
            source_path: format!("{cli_rel}/plans/{spec_id}/review.md"),
            recorded_at: ctx.execution_state.updated_at.clone(),
            recorded_by: Some("legion-cli".to_owned()),
            summary: "Historical plan/task review note imported from .legion-cli".to_owned(),
            requires_native_revalidation: true,
        });
    }

    let (stage, status) = derive_stage_and_status(&ctx, &tasks, plan_design_markdown.as_deref());
    let source_digest = compute_source_digest(&mut ctx.digest_parts);
    let cfg_rel = format!("{cli_rel}/config.yaml");
    let mut source_refs = Vec::new();
    if ctx.preserved.iter().any(|p| p == &cfg_rel) {
        source_refs.push(cfg_rel);
    }
    Ok(ScannedCliProject {
        source_cli_dir: ctx.cli_rel,
        source_digest,
        spec_id: ctx.spec_id,
        title,
        brief,
        stage,
        status,
        acceptance_criteria,
        constraints,
        source_refs,
        decisions,
        assumption_ids,
        acceptance_criterion_ids,
        tasks,
        approvals,
        historical_receipts,
        unsupported_policies: ctx.unsupported_policies,
        assurance_policy_summary: ctx.assurance_policy_summary,
        assurance_policy_json: ctx.assurance_policy_json,
        missing_log_notes,
        active_locks: ctx.active_locks,
        stale_locks: ctx.stale_locks,
        preserved_artifact_paths: ctx.preserved,
        execution_state: ctx.execution_state,
        plan_design_markdown,
        plan_review_markdown,
        warnings: ctx.warnings,
    })
}

fn collect_missing_log_notes(
    tasks: &mut [ScannedTask],
    mut exec_missing_note: Option<WorkflowBoardCliImportMissingLogNote>,
) -> Vec<WorkflowBoardCliImportMissingLogNote> {
    if let Some(ref mut note) = exec_missing_note
        && let Some(blocked_task) = tasks
            .iter()
            .find(|t| t.status == WorkflowBoardTaskStatus::Blocked)
    {
        note.task_id = Some(blocked_task.id.clone());
    }
    let mut missing_log_notes = Vec::new();
    for t in tasks {
        if t.status == WorkflowBoardTaskStatus::Blocked
            && let Some(ref note) = exec_missing_note
            && t.missing_log_notes.is_empty()
        {
            t.missing_log_notes.push(note.clone());
        }
        missing_log_notes.extend(t.missing_log_notes.iter().cloned());
    }
    if missing_log_notes.is_empty()
        && let Some(note) = exec_missing_note
    {
        missing_log_notes.push(note);
    }
    missing_log_notes
}

fn derive_stage_and_status(
    ctx: &ScanContext<'_>,
    tasks: &[ScannedTask],
    plan_design_markdown: Option<&str>,
) -> (WorkflowBoardStage, WorkflowBoardCardStatus) {
    let stage = if ctx.plan_apr_val.is_some() || ctx.execution_state.status.is_some() {
        WorkflowBoardStage::Execute
    } else if !tasks.is_empty() || plan_design_markdown.is_some() {
        WorkflowBoardStage::Plan
    } else {
        WorkflowBoardStage::Spec
    };
    let has_blocked_task = tasks.iter().any(|t| {
        matches!(
            t.status,
            WorkflowBoardTaskStatus::Blocked | WorkflowBoardTaskStatus::Failed
        )
    });
    let status = if has_blocked_task || ctx.execution_state.status.as_deref() == Some("blocked") {
        WorkflowBoardCardStatus::Blocked
    } else {
        WorkflowBoardCardStatus::Active
    };
    (stage, status)
}

fn compute_source_digest(digest_parts: &mut [(String, Vec<u8>)]) -> String {
    digest_parts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut digest_buf = Vec::new();
    for (p, bytes) in digest_parts {
        digest_buf.extend_from_slice(p.as_bytes());
        digest_buf.push(0);
        digest_buf.extend_from_slice(bytes);
        digest_buf.push(0xff);
    }
    sha256_prefixed(&digest_buf)
}

fn extract_ac_ids(
    plan_apr_val: Option<&Value>,
    acceptance_criteria: &[WorkflowBoardAcceptanceCriterion],
) -> Vec<String> {
    let mut ids: Vec<String> = plan_apr_val
        .and_then(|v| v.get("acceptanceIds"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| {
                    x.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        ids = acceptance_criteria.iter().map(|c| c.id.clone()).collect();
    }
    ids
}
