#![forbid(unsafe_code)]
//! M3 Execute and Independent Review brief preparation helpers.

use std::collections::BTreeMap;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardPlannedCheck, WorkflowBoardRequestError, WorkflowBoardTaskContract,
};
use legion_persistence::workflow_board_repo::types::CardDetail;
use legion_persistence::workflow_board_repo::types_m2::TaskContractRow;
use legion_persistence::workflow_board_repo::types_m3::CardExecutionDetail;
use legion_persistence::workflow_board_repo::{
    get_card_execution_detail, update_card_product_snapshot_digest,
};
use legion_workflows::{
    ExecuteBriefInput, ReviewBriefInput, build_execute_brief, build_review_brief,
};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::workflow_board_briefs::PreparedBriefFiles;
use crate::workflow_board_diff::{WorktreeBaseline, get_card_diff_impl, save_task_baseline};
use crate::workflow_board_fs::{ensure_confined_card_dir, resolve_card_workspace_root};
use crate::workflow_board_hydration_m3::to_card_detail_from_execution;
use crate::workflow_board_journal::ensure_confined_deliverable_target;
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_snapshot::{compute_product_snapshot_digest, resolve_head_commit_sha};
use crate::workflow_board_stage_prep::{ResolvedLaunchConfig, StageLaunchEnv};
use crate::workflow_board_types::db_lock_err;

pub(crate) fn prepare_execute_brief_inputs(
    env: &StageLaunchEnv<'_>,
    exec: &CardExecutionDetail,
    cfg: &ResolvedLaunchConfig,
    task_id: &str,
    snapshot_digest: Option<&str>,
) -> Result<PreparedBriefFiles, WorkflowBoardRequestError> {
    let conn = env
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| db_lock_err())?;
    let ws_root = resolve_card_workspace_root(&conn, &exec.planning.card_detail.card)?;
    drop(conn);
    prepare_execute_brief_for_workspace(ws_root, exec, cfg, task_id, snapshot_digest)
}

pub(crate) fn prepare_execute_brief_for_workspace(
    ws_root: std::path::PathBuf,
    exec: &CardExecutionDetail,
    cfg: &ResolvedLaunchConfig,
    task_id: &str,
    snapshot_digest: Option<&str>,
) -> Result<PreparedBriefFiles, WorkflowBoardRequestError> {
    let planning = &exec.planning;
    let detail = &planning.card_detail;
    let card_dir = ensure_confined_card_dir(&ws_root, &detail.card.card_id)?;
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let hydrated = to_card_detail_from_execution(exec, &[], Vec::new())?;

    let task: WorkflowBoardTaskContract = hydrated
        .tasks
        .into_iter()
        .find(|t| t.task_id == task_id)
        .ok_or_else(|| {
            crate::workflow_board_types::internal(format!(
                "Task `{task_id}` missing from hydrated plan"
            ))
        })?;
    let completed_task_ids: Vec<String> = planning
        .tasks
        .iter()
        .filter(|t| matches!(t.status.as_str(), "completed" | "skipped"))
        .map(|t| t.task_id.clone())
        .collect();
    let planned_checks: Vec<WorkflowBoardPlannedCheck> = hydrated.checks;

    let assembled = build_execute_brief(&ExecuteBriefInput {
        card_id: detail.card.card_id.clone(),
        card_title: detail.card.title.clone(),
        project_id: detail.card.project_id.clone(),
        workspace_root: cfg.project.workspace_root.clone(),
        worktree_path: detail.card.worktree_path.clone(),
        branch: detail.card.branch.clone(),
        card_revision: u64::try_from(detail.card.revision.max(1)).unwrap_or(1),
        spec_digest: detail.card.spec_digest.clone().unwrap_or_default(),
        plan_digest: detail.card.plan_digest.clone().unwrap_or_default(),
        product_snapshot_digest: snapshot_digest.map(str::to_owned),
        task,
        completed_task_ids,
        planned_checks,
        acceptance_criteria: fields.acceptance_criteria,
        constraints: fields.constraints,
        source_refs: fields.source_refs,
        expected_output_contract: String::new(),
    });

    Ok(PreparedBriefFiles {
        ws_root,
        card_dir: card_dir.clone(),
        brief_filename: "execute-brief.md".to_owned(),
        markdown: assembled.markdown,
        brief_digest: assembled.brief_digest,
        brief_path: card_dir
            .join("execute-brief.md")
            .to_string_lossy()
            .into_owned(),
        spec_path: card_dir.join("spec.md").to_string_lossy().into_owned(),
    })
}

pub(crate) fn prepare_review_brief_inputs(
    env: &StageLaunchEnv<'_>,
    exec: &CardExecutionDetail,
    cfg: &ResolvedLaunchConfig,
    snapshot_digest: &str,
) -> Result<PreparedBriefFiles, WorkflowBoardRequestError> {
    let planning = &exec.planning;
    let detail = &planning.card_detail;
    let conn = env
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| db_lock_err())?;
    let ws_root = resolve_card_workspace_root(&conn, &detail.card)?;
    drop(conn);
    let card_dir = ensure_confined_card_dir(&ws_root, &detail.card.card_id)?;
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let hydrated = to_card_detail_from_execution(exec, &[], Vec::new())?;

    let task_summaries: Vec<String> = planning
        .tasks
        .iter()
        .map(|t| format!("`{}` ({}): {}", t.task_id, t.status, t.title))
        .collect();
    let check_receipt_summaries: Vec<String> = hydrated
        .check_receipts
        .iter()
        .map(|r| {
            format!(
                "`{}` status={:?} exit={:?} snapshot={}",
                r.check_id,
                r.status,
                r.exit_code,
                r.product_snapshot_digest.as_deref().unwrap_or("(none)")
            )
        })
        .collect();

    let diff_res = get_card_diff_impl(
        env.orchestrator,
        &legion_contracts::workflow_board::WorkflowBoardGetCardDiffInput {
            card_id: hydrated.summary.card_id.clone(),
            task_id: None,
            include_patches: Some(true),
        },
    )
    .ok();
    let changed_files_summary: Vec<String> = diff_res
        .as_ref()
        .map_or_default(|d| d.files.iter().map(|f| f.path.clone()).collect());
    let mut diff_excerpt = diff_res.as_ref().map_or_default(|d| {
        d.files
            .iter()
            .filter_map(|f| f.patch.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    });
    if diff_excerpt.chars().count() > 8000 {
        diff_excerpt = diff_excerpt.chars().take(8000).collect();
        diff_excerpt.push_str("\n...[truncated]");
    }

    let assembled = build_review_brief(&ReviewBriefInput {
        card_id: detail.card.card_id.clone(),
        card_title: detail.card.title.clone(),
        project_id: detail.card.project_id.clone(),
        workspace_root: cfg.project.workspace_root.clone(),
        worktree_path: detail.card.worktree_path.clone(),
        branch: detail.card.branch.clone(),
        card_revision: u64::try_from(detail.card.revision.max(1)).unwrap_or(1),
        spec_digest: detail.card.spec_digest.clone().unwrap_or_default(),
        plan_digest: detail.card.plan_digest.clone().unwrap_or_default(),
        product_snapshot_digest: snapshot_digest.to_owned(),
        spec_summary: detail.card.brief.clone(),
        acceptance_criteria: fields.acceptance_criteria,
        ac_mappings: hydrated.ac_mappings,
        task_summaries,
        check_receipt_summaries,
        changed_files_summary,
        diff_excerpt,
        expected_output_contract: String::new(),
    });

    Ok(PreparedBriefFiles {
        ws_root,
        card_dir: card_dir.clone(),
        brief_filename: "review-brief.md".to_owned(),
        markdown: assembled.markdown,
        brief_digest: assembled.brief_digest,
        brief_path: card_dir
            .join("review-brief.md")
            .to_string_lossy()
            .into_owned(),
        spec_path: card_dir.join("spec.md").to_string_lossy().into_owned(),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Initializes clean worktree baseline, snapshot, and execute-brief.md after materialization"
)]
pub(crate) fn sync_materialized_worktree_baseline_and_brief(
    conn: &Connection,
    active_ws: &Path,
    active_dir: &Path,
    card_id: &str,
    stage_run_id: &str,
    task_id: &str,
    cfg: &ResolvedLaunchConfig,
    now_iso: &str,
) -> Result<(), WorkflowBoardRequestError> {
    let wt_head = resolve_head_commit_sha(active_ws).unwrap_or_else(|| "UNBORN".to_owned());
    let wt_baseline = WorktreeBaseline {
        head_oid: wt_head,
        dirty_digests: BTreeMap::new(),
    };
    save_task_baseline(active_dir, stage_run_id, &wt_baseline)?;
    let wt_snap = compute_product_snapshot_digest(active_ws);
    let _ = update_card_product_snapshot_digest(conn, card_id, &wt_snap, now_iso);
    if let Ok(refreshed_exec) = get_card_execution_detail(conn, card_id)
        && let Ok(wt_prep) = prepare_execute_brief_for_workspace(
            active_ws.to_path_buf(),
            &refreshed_exec,
            cfg,
            task_id,
            Some(&wt_snap),
        )
        && let Ok(safe_brief) = ensure_confined_deliverable_target(
            active_dir,
            &active_dir.join(&wt_prep.brief_filename),
            true,
        )
    {
        let _ = std::fs::write(&safe_brief, wt_prep.markdown.as_bytes());
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Assembles typed Execute workflow payload from card, task, and launch config"
)]
pub(crate) fn build_execute_stage_payload(
    env: &StageLaunchEnv<'_>,
    detail: &CardDetail,
    cfg: &ResolvedLaunchConfig,
    snapshot_digest: &str,
    card_id: &str,
    task_id: &str,
    task_row: &TaskContractRow,
    prep: &PreparedBriefFiles,
    stable_launch_key: &str,
) -> Value {
    json!({
        "cardId": card_id,
        "projectId": detail.card.project_id,
        "title": detail.card.title,
        "briefMarkdown": prep.markdown,
        "briefPath": prep.brief_path,
        "specPath": prep.spec_path,
        "planPath": prep.card_dir.join("plan.md").to_string_lossy().into_owned(),
        "workspaceRoot": prep.ws_root.to_string_lossy().into_owned(),
        "worktreePath": detail.card.worktree_path,
        "modelSelection": cfg.model_selection,
        "runtimeMode": cfg.runtime_mode,
        "interactionMode": cfg.interaction_mode,
        "workspaceStrategy": cfg.workspace_strategy,
        "environmentId": env.environment_id,
        "stableLaunchKey": stable_launch_key,
        "taskId": task_id,
        "taskTitle": task_row.title,
        "filesAllowed": serde_json::from_str::<Value>(&task_row.files_allowed_json).unwrap_or_else(|_| json!([])),
        "filesForbidden": serde_json::from_str::<Value>(&task_row.files_forbidden_json).unwrap_or_else(|_| json!([])),
        "expectedArtifacts": serde_json::from_str::<Value>(&task_row.expected_artifacts_json).unwrap_or_else(|_| json!([])),
        "verificationCommands": serde_json::from_str::<Value>(&task_row.verification_commands_json).unwrap_or_else(|_| json!([])),
        "productSnapshotDigest": snapshot_digest,
    })
}
