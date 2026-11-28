#![forbid(unsafe_code)]
//! M2 Plan authoring (`updatePlan`) and content-bound approval (`approvePlan`) service handlers.

use std::path::Path;
use std::sync::Arc;

use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardApprovePlanInput, WorkflowBoardApprovePlanResult, WorkflowBoardPlanDocument,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardUpdatePlanInput,
    WorkflowBoardUpdatePlanResult,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m2::ApprovePlanInput;
use legion_persistence::workflow_board_repo::{
    approve_plan, get_card_planning_detail, preview_plan_approval_card_state,
};
use legion_workflows::sha256_prefixed;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, resolve_card_workspace_root,
    validate_safe_card_id,
};
use crate::workflow_board_hydration::to_approval_receipt;
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_journal::{
    JournaledWriteSpec, execute_journaled_writes, read_confined_digest_if_file,
};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_plan_inputs::{cleanup_obsolete_task_files, persist_plan_with_journal};
use crate::workflow_board_recovery::{reconcile_card_external_edits, verify_disk_task_contracts};
use crate::workflow_board_render::{parse_stored_spec_fields, render_card_json};
use crate::workflow_board_render_plan::{
    render_design_markdown, render_plan_markdown, render_task_contract_markdown,
};
use crate::workflow_board_snapshot::clear_deliverable_stale_if_clean_and_reload;
use crate::workflow_board_types::{
    check_revision, db_lock_err, internal, map_repo_err, parse_expected_rev,
};

pub(crate) async fn update_plan_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardUpdatePlanInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardUpdatePlanResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    if invalidated {
        notify_project(&planning.card_detail.card.project_id);
    }
    check_revision(
        card_id,
        planning.card_detail.card.revision,
        input.expected_revision,
    )?;

    let ws_root = resolve_card_workspace_root(&conn, &planning.card_detail.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let existing_keys = enumerate_artifact_keys(&ws_root, card_id);
    let existing_detail = to_card_detail_from_planning(&planning, existing_keys)?;
    let merged_doc = build_merged_plan_doc(existing_detail.plan.as_ref(), input, &now_iso);

    let (plan_json, plan_digest, design_digest, writes) =
        build_plan_writes(card_id, &planning.card_detail.card, &merged_doc, &now_iso)?;

    let updated = persist_plan_with_journal(
        &conn,
        card_id,
        &card_dir,
        &writes,
        expected_rev_i64,
        input,
        &plan_json,
        &plan_digest,
        &design_digest,
        &now_iso,
    )?;
    let updated = clear_deliverable_stale_if_clean_and_reload(&conn, card_id, updated, &now_iso)?;

    cleanup_obsolete_task_files(&card_dir, &updated.tasks);
    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    Ok(WorkflowBoardUpdatePlanResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        plan_digest: detail.plan.as_ref().and_then(|p| p.plan_digest.clone()),
        spec_approval: detail.spec_approval.clone(),
        plan_approval: detail.plan_approval.clone(),
        lenses: detail.lenses.clone(),
        alternatives: detail.alternatives.clone(),
        selected_alternative_id: detail.selected_alternative_id.clone(),
        selection_rationale: detail.selection_rationale.clone(),
        strategy: detail.strategy,
        strategy_metadata: detail.strategy_metadata.clone(),
        granularity: detail.granularity,
        tasks: detail.tasks.clone(),
        ac_mappings: detail.ac_mappings.clone(),
        card: detail,
    })
}

pub(crate) async fn approve_plan_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardApprovePlanInput,
    approved_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardApprovePlanResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let current = &planning.card_detail;
    if invalidated {
        notify_project(&current.card.project_id);
    }
    check_revision(card_id, current.card.revision, input.expected_revision)?;
    validate_approve_plan_preconditions(current)?;

    let ws_root = resolve_card_workspace_root(&conn, &current.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    verify_plan_artifacts_on_disk(&card_dir, card_id, &planning)?;

    let preview = preview_plan_approval_card_state(&conn, &planning).map_err(map_repo_err)?;
    let fields = parse_stored_spec_fields(&current.card.spec_json);
    let mut next_card = current.card.clone();
    next_card.revision += 1;
    "execute".clone_into(&mut next_card.stage);
    next_card.status = preview.status;
    next_card.blocker_reason = preview.blocker_reason;
    next_card.tasks_digest = Some(preview.tasks_digest);
    next_card.next_action = Some(preview.next_action);
    now_iso.clone_into(&mut next_card.updated_at);
    let card_json = render_card_json(&next_card, &fields)?;
    let writes = vec![JournaledWriteSpec {
        key: "card.json".to_owned(),
        bytes: card_json.as_bytes().to_vec(),
        expected_digest: sha256_prefixed(card_json.as_bytes()),
        revision: Some(next_card.revision),
    }];

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let app_id = format!("apr-{}", Uuid::new_v4().simple());
    let updated = execute_journaled_writes(&conn, card_id, &card_dir, &writes, &now_iso, |tx| {
        approve_plan(
            tx,
            &ApprovePlanInput {
                card_id,
                revision_id: &rev_id,
                approval_id: &app_id,
                expected_revision: expected_rev_i64,
                expected_spec_digest: input.expected_spec_digest.as_deref(),
                expected_plan_digest: input.expected_plan_digest.as_deref(),
                approved_by,
            },
            &now_iso,
        )
        .map_err(map_repo_err)
    })?;
    let updated = clear_deliverable_stale_if_clean_and_reload(&conn, card_id, updated, &now_iso)?;

    let receipt = updated
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == app_id)
        .map(to_approval_receipt)
        .transpose()?
        .ok_or_else(|| internal("Newly recorded plan approval receipt missing"))?;

    cleanup_obsolete_task_files(&card_dir, &updated.tasks);
    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    Ok(WorkflowBoardApprovePlanResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        plan_approval: receipt,
        card: detail,
    })
}

pub(crate) fn build_merged_plan_doc(
    prior: Option<&WorkflowBoardPlanDocument>,
    input: &WorkflowBoardUpdatePlanInput,
    now_iso: &str,
) -> WorkflowBoardPlanDocument {
    let mut doc = prior.cloned().unwrap_or_else(|| WorkflowBoardPlanDocument {
        lenses: Vec::new(),
        alternatives: Vec::new(),
        alternatives_revision_count: 0,
        selected_alternative_id: None,
        selection_rationale: None,
        strategy: None,
        strategy_rationale: None,
        strategy_metadata: None,
        granularity: None,
        tasks: Vec::new(),
        checks: Vec::new(),
        ac_mappings: Vec::new(),
        plan_digest: None,
        tasks_digest: None,
        updated_at: now_iso.to_owned(),
    });
    if let Some(ref lenses) = input.lenses {
        doc.lenses.clone_from(lenses);
    }
    if let Some(ref alts) = input.alternatives {
        doc.alternatives.clone_from(alts);
        if let Some(ref sel) = doc.selected_alternative_id
            && !alts.iter().any(|a| a.id.trim() == sel.trim())
        {
            doc.selected_alternative_id = None;
            doc.selection_rationale = None;
        }
    }
    match &input.selected_alternative_id {
        PatchValue::Absent => {}
        PatchValue::Null => {
            doc.selected_alternative_id = None;
            doc.selection_rationale = None;
        }
        PatchValue::Set(id) => doc.selected_alternative_id = Some(id.clone()),
    }
    if let Some(ref rat) = input.selection_rationale {
        doc.selection_rationale = Some(rat.clone());
    }
    if input.revise_alternatives_feedback.is_some() {
        doc.alternatives_revision_count = (doc.alternatives_revision_count + 1).min(1);
    }
    if input.strategy.is_some() {
        doc.strategy = input.strategy;
    }
    if let Some(ref srat) = input.strategy_rationale {
        doc.strategy_rationale = Some(srat.clone());
    }
    if let Some(ref smeta) = input.strategy_metadata {
        doc.strategy_metadata = Some(smeta.clone());
    }
    if input.granularity.is_some() {
        doc.granularity = input.granularity;
    }
    if let Some(ref tasks) = input.tasks {
        doc.tasks.clone_from(tasks);
    }
    if let Some(ref checks) = input.checks {
        doc.checks.clone_from(checks);
    }
    if let Some(ref mappings) = input.ac_mappings {
        doc.ac_mappings.clone_from(mappings);
    }
    now_iso.clone_into(&mut doc.updated_at);
    doc
}

pub(crate) fn build_plan_writes(
    card_id: &str,
    current_card: &legion_persistence::workflow_board_repo::types::CardRow,
    merged_doc: &WorkflowBoardPlanDocument,
    now_iso: &str,
) -> Result<(String, String, String, Vec<JournaledWriteSpec>), WorkflowBoardRequestError> {
    let design_md = render_design_markdown(card_id, &current_card.title, merged_doc);
    let design_digest = sha256_prefixed(design_md.as_bytes());
    let plan_md = render_plan_markdown(card_id, &current_card.title, merged_doc);
    let plan_digest = sha256_prefixed(plan_md.as_bytes());
    let mut final_doc = merged_doc.clone();
    final_doc.plan_digest = Some(plan_digest.clone());
    let plan_json = serde_json::to_string(&final_doc).map_err(|e| internal(e.to_string()))?;

    let mut next_card = current_card.clone();
    next_card.revision += 1;
    if matches!(next_card.stage.as_str(), "execute" | "ship") {
        "plan".clone_into(&mut next_card.stage);
        next_card.next_action = Some("revise_plan".to_owned());
    }
    next_card.design_digest = Some(design_digest.clone());
    next_card.plan_digest = Some(plan_digest.clone());
    next_card.plan_json = Some(plan_json.clone());
    now_iso.clone_into(&mut next_card.updated_at);
    let fields = parse_stored_spec_fields(&next_card.spec_json);
    let card_json = render_card_json(&next_card, &fields)?;

    let mut writes = vec![
        JournaledWriteSpec {
            key: "design.md".to_owned(),
            bytes: design_md.into_bytes(),
            expected_digest: design_digest.clone(),
            revision: Some(next_card.revision),
        },
        JournaledWriteSpec {
            key: "plan.md".to_owned(),
            bytes: plan_md.into_bytes(),
            expected_digest: plan_digest.clone(),
            revision: Some(next_card.revision),
        },
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: Some(next_card.revision),
        },
    ];
    for task in &merged_doc.tasks {
        let task_md = render_task_contract_markdown(card_id, task);
        let task_digest = sha256_prefixed(task_md.as_bytes());
        writes.push(JournaledWriteSpec {
            key: format!("tasks/{}.md", task.task_id),
            bytes: task_md.into_bytes(),
            expected_digest: task_digest,
            revision: Some(next_card.revision),
        });
    }
    Ok((plan_json, plan_digest, design_digest, writes))
}

fn verify_plan_artifacts_on_disk(
    card_dir: &Path,
    card_id: &str,
    planning: &legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail,
) -> Result<(), WorkflowBoardRequestError> {
    let card = &planning.card_detail.card;
    for (file, expected) in [
        ("plan.md", card.plan_digest.as_deref()),
        ("design.md", card.design_digest.as_deref()),
    ] {
        if let Some(dg) = expected
            && read_confined_digest_if_file(card_dir, &card_dir.join(file))?.as_deref() != Some(dg)
        {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::DigestMismatch,
                format!("{file} on disk does not match persisted digest"),
            ));
        }
    }
    if !verify_disk_task_contracts(card_dir, card_id, planning) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::DigestMismatch,
            "Task contract markdown under tasks/*.md on disk does not match persisted plan",
        ));
    }
    Ok(())
}

fn validate_approve_plan_preconditions(
    current: &legion_persistence::workflow_board_repo::types::CardDetail,
) -> Result<(), WorkflowBoardRequestError> {
    if matches!(
        current.card.status.as_str(),
        "paused" | "completed" | "shipped"
    ) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "Cannot approve plan while card status is `{}`",
                current.card.status
            ),
        ));
    }
    if !current
        .approvals
        .iter()
        .any(|a| a.gate == "spec" && a.status == "valid")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::SpecNotApproved,
            "cannot approve plan without a valid spec approval",
        ));
    }
    if current.card.stage != "plan" {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "Cannot approve plan in stage `{}` with status `{}`",
                current.card.stage, current.card.status
            ),
        ));
    }
    Ok(())
}
