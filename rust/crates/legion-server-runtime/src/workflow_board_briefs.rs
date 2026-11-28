#![forbid(unsafe_code)]
//! M2 Spec & Plan stage brief assembly and write-journaled brief staging.

use std::path::{Path, PathBuf};

use legion_contracts::workflow_board::{
    WorkflowBoardAssistanceMode, WorkflowBoardIntakeGoal, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType,
};
use legion_persistence::workflow_board_repo::types::{
    CardDetail, StageRunRow, UpsertStageRunInput, UpsertStageRunOutcome,
};
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_persistence::workflow_board_repo::upsert_or_get_stage_run_by_launch_key;
use legion_workflows::{
    BriefChallengeConcern, PlanBriefInput, SpecBriefInput, build_plan_brief, build_spec_brief,
    sha256_prefixed,
};
use serde_json::Value;
use uuid::Uuid;

use crate::workflow_board_fs::{ensure_confined_card_dir, resolve_card_workspace_root};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_journal::{
    JournaledWriteSpec, execute_journaled_writes, read_confined_utf8_if_file,
};
use crate::workflow_board_render::{
    parse_stored_spec_fields, render_card_json, render_spec_markdown,
};
use crate::workflow_board_stage_prep::{ResolvedLaunchConfig, StageLaunchEnv};
use crate::workflow_board_types::{enum_as_str, map_repo_err, parse_opt_enum};

pub(crate) struct PreparedBriefFiles {
    pub ws_root: PathBuf,
    pub card_dir: PathBuf,
    pub brief_filename: String,
    pub markdown: String,
    pub brief_digest: String,
    pub brief_path: String,
    pub spec_path: String,
}

pub(crate) fn prepare_spec_brief_inputs(
    env: &StageLaunchEnv<'_>,
    planning: &CardPlanningDetail,
    cfg: &ResolvedLaunchConfig,
) -> Result<PreparedBriefFiles, WorkflowBoardRequestError> {
    let detail = &planning.card_detail;
    let conn = env.orchestrator.sink().db().lock().map_err(|_| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Database lock poisoned",
        )
    })?;
    let ws_root = resolve_card_workspace_root(&conn, &detail.card)?;
    drop(conn);
    let card_dir = ensure_confined_card_dir(&ws_root, &detail.card.card_id)?;
    let spec_path = card_dir.join("spec.md");
    let current_spec_markdown = read_confined_utf8_if_file(&card_dir, &spec_path)?;
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let assist: WorkflowBoardAssistanceMode =
        parse_opt_enum(Some(&detail.card.assistance_mode), "assistance_mode")?
            .unwrap_or(WorkflowBoardAssistanceMode::Balanced);
    let intake_goal: Option<WorkflowBoardIntakeGoal> =
        parse_opt_enum(detail.card.intake_goal.as_deref(), "intake_goal")?;
    let unresolved_decisions: Vec<String> = planning
        .decisions
        .iter()
        .filter(|d| d.status == "unresolved")
        .map(|d| format!("{}: {}", d.decision_id, d.prompt))
        .collect();

    let assembled = build_spec_brief(&SpecBriefInput {
        card_id: detail.card.card_id.clone(),
        card_title: detail.card.title.clone(),
        card_description: detail.card.brief.clone(),
        project_id: detail.card.project_id.clone(),
        workspace_root: cfg.project.workspace_root.clone(),
        worktree_path: detail.card.worktree_path.clone(),
        branch: detail.card.branch.clone(),
        card_revision: u64::try_from(detail.card.revision.max(1)).unwrap_or(1),
        assistance_mode: assist,
        intake_goal,
        current_spec_markdown,
        acceptance_criteria: fields.acceptance_criteria,
        constraints: fields.constraints,
        source_refs: fields.source_refs,
        wireframes_index: fields.wireframes_index,
        unresolved_decisions,
        expected_output_contract: String::new(),
    });
    let brief_path = card_dir
        .join("spec-brief.md")
        .to_string_lossy()
        .into_owned();
    let spec_path_str = card_dir.join("spec.md").to_string_lossy().into_owned();
    Ok(PreparedBriefFiles {
        ws_root,
        card_dir,
        brief_filename: "spec-brief.md".to_owned(),
        markdown: assembled.markdown,
        brief_digest: assembled.brief_digest,
        brief_path,
        spec_path: spec_path_str,
    })
}

pub(crate) fn prepare_plan_brief_inputs(
    env: &StageLaunchEnv<'_>,
    planning: &CardPlanningDetail,
    cfg: &ResolvedLaunchConfig,
    spec_digest: &str,
) -> Result<PreparedBriefFiles, WorkflowBoardRequestError> {
    let detail = &planning.card_detail;
    let conn = env.orchestrator.sink().db().lock().map_err(|_| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Database lock poisoned",
        )
    })?;
    let ws_root = resolve_card_workspace_root(&conn, &detail.card)?;
    drop(conn);
    let card_dir = ensure_confined_card_dir(&ws_root, &detail.card.card_id)?;
    let spec_path = card_dir.join("spec.md");
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let spec_md = read_confined_utf8_if_file(&card_dir, &spec_path)?.unwrap_or_else(|| {
        let src_type: WorkflowBoardSpecSourceType =
            serde_json::from_value(Value::String(detail.card.source_type.clone()))
                .unwrap_or(WorkflowBoardSpecSourceType::Idea);
        let assist: WorkflowBoardAssistanceMode =
            serde_json::from_value(Value::String(detail.card.assistance_mode.clone()))
                .unwrap_or(WorkflowBoardAssistanceMode::Balanced);
        render_spec_markdown(
            &detail.card.card_id,
            &detail.card.title,
            &detail.card.brief,
            src_type,
            assist,
            &fields,
            &detail.card.updated_at,
        )
    });
    let spec_rev = detail
        .approvals
        .iter()
        .find(|a| a.gate == "spec" && a.status == "valid")
        .and_then(|a| u64::try_from(a.revision).ok())
        .unwrap_or(1);

    let hydrated = to_card_detail_from_planning(planning, Vec::new())?;
    let m2_ctx = build_plan_m2_brief_fields(planning, &hydrated);

    let assembled = build_plan_brief(&PlanBriefInput {
        card_id: detail.card.card_id.clone(),
        card_title: detail.card.title.clone(),
        project_id: detail.card.project_id.clone(),
        workspace_root: cfg.project.workspace_root.clone(),
        worktree_path: detail.card.worktree_path.clone(),
        branch: detail.card.branch.clone(),
        spec_revision: spec_rev,
        spec_digest: spec_digest.to_owned(),
        spec_brief: detail.card.brief.clone(),
        spec_markdown: spec_md,
        acceptance_criteria: fields.acceptance_criteria,
        constraints: fields.constraints,
        unresolved_decisions: m2_ctx.unresolved_decisions,
        source_refs: fields.source_refs,
        wireframes_index: fields.wireframes_index,
        assistance_mode: hydrated.spec.assistance_mode,
        planning_strategy: hydrated.strategy,
        strategy_rationale: hydrated.strategy_rationale,
        task_granularity: hydrated.granularity,
        exploration_summary: m2_ctx.exploration_summary,
        resolved_decisions: m2_ctx.resolved_decisions,
        challenge_receipt_summary: m2_ctx.challenge_receipt_summary,
        challenge_concerns: m2_ctx.challenge_concerns,
        design_lenses: hydrated.lenses,
        design_alternatives: hydrated.alternatives,
        selected_alternative_id: hydrated.selected_alternative_id,
        selection_rationale: hydrated.selection_rationale,
        existing_tasks_summary: m2_ctx.existing_tasks_summary,
        expected_output_contract: String::new(),
    });
    let brief_path = card_dir
        .join("plan-brief.md")
        .to_string_lossy()
        .into_owned();
    let spec_path_str = card_dir.join("spec.md").to_string_lossy().into_owned();
    Ok(PreparedBriefFiles {
        ws_root,
        card_dir,
        brief_filename: "plan-brief.md".to_owned(),
        markdown: assembled.markdown,
        brief_digest: assembled.brief_digest,
        brief_path,
        spec_path: spec_path_str,
    })
}

struct PlanBriefM2Context {
    unresolved_decisions: Vec<String>,
    resolved_decisions: Vec<String>,
    exploration_summary: Option<String>,
    challenge_receipt_summary: Option<String>,
    challenge_concerns: Vec<BriefChallengeConcern>,
    existing_tasks_summary: Vec<String>,
}

fn build_plan_m2_brief_fields(
    planning: &CardPlanningDetail,
    hydrated: &legion_contracts::workflow_board::WorkflowCardDetail,
) -> PlanBriefM2Context {
    let unresolved_decisions: Vec<String> = planning
        .decisions
        .iter()
        .filter(|d| d.status == "unresolved")
        .map(|d| format!("{}: {}", d.decision_id, d.prompt))
        .collect();
    let resolved_decisions: Vec<String> = planning
        .decisions
        .iter()
        .filter(|d| d.status == "resolved")
        .map(|d| {
            format!(
                "{}: {}",
                d.decision_id,
                d.answer.as_deref().unwrap_or("(resolved)")
            )
        })
        .collect();
    let exploration_summary = hydrated.exploration.as_ref().map(|e| {
        format!(
            "round={}, paused={}, resolved={}, unresolved={}",
            e.round_index,
            e.paused,
            resolved_decisions.len(),
            e.unresolved_ids.len()
        )
    });
    let challenge_receipt_summary = hydrated.challenge_receipt.as_ref().map(|c| {
        format!(
            "status={:?}, synthesisCount={}, concerns={}",
            c.status,
            c.synthesis_count,
            c.concerns.len()
        )
    });
    let challenge_concerns: Vec<BriefChallengeConcern> = hydrated
        .challenge_receipt
        .as_ref()
        .map(|c| {
            c.concerns
                .iter()
                .map(|con| BriefChallengeConcern {
                    id: con.id.clone(),
                    summary: con.summary.clone(),
                    disposition: enum_as_str(&con.disposition)
                        .unwrap_or_else(|| "unresolved".to_owned()),
                    operator_note: con.answer.clone().or_else(|| con.reason.clone()),
                })
                .collect()
        })
        .unwrap_or_default();
    let existing_tasks_summary: Vec<String> = hydrated
        .tasks
        .iter()
        .map(|t| format!("{}: {}", t.task_id, t.title))
        .collect();
    PlanBriefM2Context {
        unresolved_decisions,
        resolved_decisions,
        exploration_summary,
        challenge_receipt_summary,
        challenge_concerns,
        existing_tasks_summary,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates write-journaled stage brief persistence with task-scoped stage run upsert"
)]
pub(crate) fn stage_brief_and_upsert_run(
    env: &StageLaunchEnv<'_>,
    detail: &CardDetail,
    cfg: &ResolvedLaunchConfig,
    stage: &str,
    task_id: Option<&str>,
    brief_filename: &str,
    card_dir: &Path,
    brief_markdown: &str,
    brief_digest: &str,
    stable_launch_key: &str,
    attempt: i64,
    now_iso: &str,
) -> Result<StageRunRow, WorkflowBoardRequestError> {
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let card_json = render_card_json(&detail.card, &fields)?;
    let writes = vec![
        JournaledWriteSpec {
            key: brief_filename.to_owned(),
            bytes: brief_markdown.as_bytes().to_vec(),
            expected_digest: brief_digest.to_owned(),
            revision: Some(detail.card.revision),
        },
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: Some(detail.card.revision),
        },
    ];
    let sr_id = format!("sr-{}", Uuid::new_v4().simple());
    let model_opts_json = cfg
        .model_selection
        .options
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                e.to_string(),
            )
        })?;
    let rt_str = enum_as_str(&cfg.runtime_mode).unwrap_or_else(|| "approval-required".to_owned());
    let im_str = enum_as_str(&cfg.interaction_mode).unwrap_or_else(|| "default".to_owned());
    let ws_json = serde_json::to_string(&cfg.workspace_strategy).map_err(|e| {
        WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::InternalError, e.to_string())
    })?;

    let conn = env.orchestrator.sink().db().lock().map_err(|_| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Database lock poisoned",
        )
    })?;
    execute_journaled_writes(
        &conn,
        &detail.card.card_id,
        card_dir,
        &writes,
        now_iso,
        |tx_conn| {
            let outcome = upsert_or_get_stage_run_by_launch_key(
                tx_conn,
                &UpsertStageRunInput {
                    id: &sr_id,
                    card_id: &detail.card.card_id,
                    stage,
                    task_id,
                    attempt,
                    stable_launch_key,
                    card_revision: detail.card.revision,
                    provider_instance_id: cfg.model_selection.instance_id.as_str(),
                    model: &cfg.model_selection.model,
                    model_options_json: model_opts_json.as_deref(),
                    runtime_mode: &rt_str,
                    interaction_mode: &im_str,
                    workspace_strategy_json: &ws_json,
                    worktree_path: detail.card.worktree_path.as_deref(),
                    branch: detail.card.branch.as_deref(),
                    brief_digest: Some(brief_digest),
                },
                now_iso,
            )
            .map_err(map_repo_err)?;
            Ok(match outcome {
                UpsertStageRunOutcome::Created(row) | UpsertStageRunOutcome::Existing(row) => row,
            })
        },
    )
}
