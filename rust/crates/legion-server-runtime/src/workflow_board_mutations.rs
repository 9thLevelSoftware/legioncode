#![forbid(unsafe_code)]
//! Spec update, spec approval, and return-for-revision mutations with write-journaled deliverables.

use std::path::Path;
use std::sync::Arc;

use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardSourceInputDigest, WorkflowBoardSpecDiff,
    WorkflowBoardUpdateSpecInput, WorkflowBoardUpdateSpecResult,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types::{CardDetail, UpdateSpecInput};
use legion_persistence::workflow_board_repo::{get_card_planning_detail, update_spec};
use legion_workflows::sha256_prefixed;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, read_confined_input_file,
    resolve_card_workspace_root, validate_artifact_key_strict, validate_safe_card_id,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_journal::{
    JournaledWriteSpec, execute_journaled_writes, read_confined_utf8_if_file,
};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_render::{
    StoredSpecFields, WireframeEditCtx, apply_wireframe_edit, compute_spec_diff,
    parse_stored_spec_fields, render_card_json, render_spec_if_changed,
};
use crate::workflow_board_snapshot::clear_deliverable_stale_if_clean;
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, internal, map_repo_err, parse_expected_rev,
};

struct PreparedSpecUpdate {
    next_title: String,
    next_brief: String,
    next_intake_goal: Option<String>,
    next_input_paths_json: Option<String>,
    next_spec_json: String,
    spec_digest: String,
    next_wireframes_digest: Option<String>,
    clear_wireframes_digest: bool,
    diff: Option<WorkflowBoardSpecDiff>,
    writes: Vec<JournaledWriteSpec>,
}

pub(crate) async fn update_spec_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardUpdateSpecInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardUpdateSpecResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    if let PatchValue::Set(ref wf_key) = input.wireframes_index {
        let _ = validate_artifact_key_strict(wf_key)?;
    }
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

    let ws_root = resolve_card_workspace_root(&conn, &current.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let prep = prepare_spec_update(current, &ws_root, &card_dir, input, card_id, &now_iso)?;
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());

    let mut updated =
        execute_journaled_writes(&conn, card_id, &card_dir, &prep.writes, &now_iso, |tx| {
            update_spec(
                tx,
                &UpdateSpecInput {
                    card_id,
                    revision_id: &rev_id,
                    expected_revision: expected_rev_i64,
                    title: Some(&prep.next_title),
                    brief: Some(&prep.next_brief),
                    intake_goal: prep.next_intake_goal.as_deref(),
                    input_paths_json: prep.next_input_paths_json.as_deref(),
                    spec_json: Some(&prep.next_spec_json),
                    spec_digest: Some(&prep.spec_digest),
                    wireframes_digest: prep.next_wireframes_digest.as_deref(),
                    clear_wireframes_digest: prep.clear_wireframes_digest,
                },
                &now_iso,
            )
            .map_err(map_repo_err)?;
            get_card_planning_detail(tx, card_id).map_err(map_repo_err)
        })?;
    if clear_deliverable_stale_if_clean(&conn, card_id, &now_iso)? {
        updated = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    }

    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    Ok(WorkflowBoardUpdateSpecResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        spec_digest: detail.spec.spec_digest.clone(),
        spec_approval_status: detail.summary.spec_approval_status,
        wireframes_index: detail.wireframes_index.clone(),
        spec_approval: detail.spec_approval.clone(),
        plan_approval: detail.plan_approval.clone(),
        diff: prep.diff,
        card: detail,
    })
}

struct AppliedSpecEdits {
    fields: StoredSpecFields,
    changed_fields: Vec<String>,
    next_input_paths_json: Option<String>,
    clear_wireframes_digest: bool,
    next_wireframes_digest: Option<String>,
    extra_wf_write: Option<JournaledWriteSpec>,
}

fn apply_spec_field_edits(
    current: &CardDetail,
    ws_root: &Path,
    card_dir: &Path,
    input: &WorkflowBoardUpdateSpecInput,
    card_id: &str,
) -> Result<AppliedSpecEdits, WorkflowBoardRequestError> {
    let mut fields: StoredSpecFields = parse_stored_spec_fields(&current.card.spec_json);
    let mut changed_fields = Vec::new();
    if let Some(ref t) = input.title
        && t != &current.card.title
    {
        changed_fields.push("title".to_owned());
    }
    if let Some(ref b) = input.brief
        && b != &current.card.brief
    {
        changed_fields.push("brief".to_owned());
    }
    if let Some(ref ac) = input.acceptance_criteria {
        fields.acceptance_criteria.clone_from(ac);
        changed_fields.push("acceptanceCriteria".to_owned());
    }
    if let Some(ref cons) = input.constraints {
        fields.constraints.clone_from(cons);
        changed_fields.push("constraints".to_owned());
    }
    if let Some(ref refs) = input.source_refs {
        fields.source_refs.clone_from(refs);
        changed_fields.push("sourceRefs".to_owned());
    }
    if let Some(ref paths) = input.input_paths {
        let mut digests = Vec::new();
        for p in paths {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let (_c, d) = read_confined_input_file(ws_root, trimmed)?;
                digests.push(WorkflowBoardSourceInputDigest {
                    path: trimmed.to_owned(),
                    digest: d,
                });
                if !fields.source_refs.iter().any(|r| r == trimmed) {
                    fields.source_refs.push(trimmed.to_owned());
                }
            }
        }
        fields.source_digest = (!digests.is_empty()).then(|| {
            let joined = digests
                .iter()
                .map(|d| format!("{}:{}", d.path, d.digest))
                .collect::<Vec<_>>()
                .join("\n");
            sha256_prefixed(joined.as_bytes())
        });
        fields.source_input_digests = digests;
        changed_fields.push("inputPaths".to_owned());
    }
    let next_input_paths_json = input
        .input_paths
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;

    let mut clear_wireframes_digest = false;
    let mut next_wireframes_digest = None;
    let mut extra_wf_write: Option<JournaledWriteSpec> = None;
    apply_wireframe_edit(&mut WireframeEditCtx {
        current,
        card_dir,
        input,
        card_id,
        fields: &mut fields,
        changed_fields: &mut changed_fields,
        clear_wireframes_digest: &mut clear_wireframes_digest,
        next_wireframes_digest: &mut next_wireframes_digest,
        extra_wf_write: &mut extra_wf_write,
    })?;

    Ok(AppliedSpecEdits {
        fields,
        changed_fields,
        next_input_paths_json,
        clear_wireframes_digest,
        next_wireframes_digest,
        extra_wf_write,
    })
}

fn prepare_spec_update(
    current: &CardDetail,
    ws_root: &Path,
    card_dir: &Path,
    input: &WorkflowBoardUpdateSpecInput,
    card_id: &str,
    now_iso: &str,
) -> Result<PreparedSpecUpdate, WorkflowBoardRequestError> {
    let AppliedSpecEdits {
        fields,
        changed_fields,
        next_input_paths_json,
        clear_wireframes_digest,
        next_wireframes_digest,
        extra_wf_write,
    } = apply_spec_field_edits(current, ws_root, card_dir, input, card_id)?;

    let next_title = input
        .title
        .as_deref()
        .unwrap_or(&current.card.title)
        .to_owned();
    let next_brief = input
        .brief
        .as_deref()
        .unwrap_or(&current.card.brief)
        .to_owned();
    let next_intake_goal = input.intake_goal.and_then(|g| enum_as_str(&g));
    let next_spec_json = serde_json::to_string(&fields).map_err(|e| internal(e.to_string()))?;
    let prev_md = read_confined_utf8_if_file(card_dir, &card_dir.join("spec.md"))?;
    let content_unchanged = next_title == current.card.title
        && next_brief == current.card.brief
        && next_spec_json == current.card.spec_json
        && next_intake_goal
            .as_deref()
            .is_none_or(|g| Some(g) == current.card.intake_goal.as_deref())
        && next_input_paths_json.is_none()
        && !clear_wireframes_digest
        && next_wireframes_digest.is_none();
    let (spec_md, spec_digest) = render_spec_if_changed(
        current,
        card_id,
        &next_title,
        &next_brief,
        &fields,
        prev_md.as_deref(),
        content_unchanged,
        now_iso,
    );
    let diff = compute_spec_diff(
        prev_md.as_deref(),
        current.card.spec_digest.as_deref(),
        &spec_md,
        &spec_digest,
        changed_fields,
    );
    let writes = build_spec_writes(
        current,
        &fields,
        &next_title,
        &next_brief,
        next_intake_goal.as_deref(),
        next_input_paths_json.as_deref(),
        clear_wireframes_digest,
        next_wireframes_digest.as_deref(),
        &next_spec_json,
        spec_md,
        &spec_digest,
        content_unchanged,
        extra_wf_write,
        now_iso,
    )?;
    Ok(PreparedSpecUpdate {
        next_title,
        next_brief,
        next_intake_goal,
        next_input_paths_json,
        next_spec_json,
        spec_digest,
        next_wireframes_digest,
        clear_wireframes_digest,
        diff,
        writes,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Synchronizes temp CardRow fields and builds journaled spec/card/wireframe writes"
)]
fn build_spec_writes(
    current: &CardDetail,
    fields: &StoredSpecFields,
    next_title: &str,
    next_brief: &str,
    next_intake_goal: Option<&str>,
    next_input_paths_json: Option<&str>,
    clear_wireframes_digest: bool,
    next_wireframes_digest: Option<&str>,
    next_spec_json: &str,
    spec_md: String,
    spec_digest: &str,
    content_unchanged: bool,
    extra_wf_write: Option<JournaledWriteSpec>,
    now_iso: &str,
) -> Result<Vec<JournaledWriteSpec>, WorkflowBoardRequestError> {
    let mut temp_card = current.card.clone();
    next_title.clone_into(&mut temp_card.title);
    next_brief.clone_into(&mut temp_card.brief);
    if let Some(goal) = next_intake_goal {
        temp_card.intake_goal = Some(goal.to_owned());
    }
    if let Some(paths_json) = next_input_paths_json {
        paths_json.clone_into(&mut temp_card.input_paths_json);
    }
    if clear_wireframes_digest {
        temp_card.wireframes_digest = None;
    } else if let Some(wf_dg) = next_wireframes_digest {
        temp_card.wireframes_digest = Some(wf_dg.to_owned());
    }
    next_spec_json.clone_into(&mut temp_card.spec_json);
    temp_card.spec_digest = Some(spec_digest.to_owned());
    if !content_unchanged || current.card.spec_digest.as_deref() != Some(spec_digest) {
        temp_card.revision += 1;
        if temp_card.stage != "spec" {
            "spec".clone_into(&mut temp_card.stage);
            temp_card.next_action = Some("revise_spec".to_owned());
        }
        now_iso.clone_into(&mut temp_card.updated_at);
    }
    let card_json = render_card_json(&temp_card, fields)?;
    let mut writes = vec![
        JournaledWriteSpec {
            key: "spec.md".to_owned(),
            bytes: spec_md.into_bytes(),
            expected_digest: spec_digest.to_owned(),
            revision: Some(temp_card.revision),
        },
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: Some(temp_card.revision),
        },
    ];
    writes.extend(extra_wf_write);
    Ok(writes)
}
