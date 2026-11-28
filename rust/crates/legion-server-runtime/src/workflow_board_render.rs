#![forbid(unsafe_code)]
//! Deterministic `spec.md`, `card.json`, and spec diff rendering for `.legion/workflows/<card-id>/`.

use std::fmt::Write as _;
use std::path::Path;

use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardAssistanceMode, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardSourceInputDigest, WorkflowBoardSpecDiff,
    WorkflowBoardSpecIntakeAnalysis, WorkflowBoardSpecSourceType, WorkflowBoardUpdateSpecInput,
};
use legion_persistence::workflow_board_repo::types::{CardDetail, CardRow};
use legion_workflows::sha256_prefixed;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::workflow_board_journal::{JournaledWriteSpec, read_confined_digest_if_file};
use crate::workflow_board_render_plan::render_wireframe_starter_html;
use crate::workflow_board_types::{enum_as_str, parse_workspace_strategy};

/// Structured fields persisted in `workflow_board_cards.spec_json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredSpecFields {
    #[serde(default)]
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub wireframes_index: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_input_digests: Vec<WorkflowBoardSourceInputDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_analysis: Option<WorkflowBoardSpecIntakeAnalysis>,
}

pub(crate) fn parse_stored_spec_fields(spec_json: &str) -> StoredSpecFields {
    serde_json::from_str::<StoredSpecFields>(spec_json).unwrap_or_default()
}

#[expect(
    clippy::too_many_arguments,
    reason = "Renders spec.md only when spec content actually changed"
)]
pub(crate) fn render_spec_if_changed(
    current: &CardDetail,
    card_id: &str,
    next_title: &str,
    next_brief: &str,
    fields: &StoredSpecFields,
    prev_md: Option<&str>,
    content_unchanged: bool,
    now_iso: &str,
) -> (String, String) {
    if content_unchanged
        && let (Some(existing_md), Some(existing_dg)) = (prev_md, current.card.spec_digest.as_ref())
    {
        return (existing_md.to_owned(), existing_dg.clone());
    }
    let src_type: WorkflowBoardSpecSourceType =
        serde_json::from_value(Value::String(current.card.source_type.clone()))
            .unwrap_or(WorkflowBoardSpecSourceType::Idea);
    let assist: WorkflowBoardAssistanceMode =
        serde_json::from_value(Value::String(current.card.assistance_mode.clone()))
            .unwrap_or(WorkflowBoardAssistanceMode::Balanced);
    let render_ts = if content_unchanged {
        current.card.updated_at.as_str()
    } else {
        now_iso
    };
    let md = render_spec_markdown(
        card_id, next_title, next_brief, src_type, assist, fields, render_ts,
    );
    let dg = sha256_prefixed(md.as_bytes());
    (md, dg)
}

/// Renders deterministic `spec.md` bytes from spec inputs.
#[must_use]
pub(crate) fn render_spec_markdown(
    card_id: &str,
    title: &str,
    brief: &str,
    source_type: WorkflowBoardSpecSourceType,
    assistance_mode: WorkflowBoardAssistanceMode,
    fields: &StoredSpecFields,
    updated_at: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", title.trim());
    let _ = writeln!(out, "- Card ID: `{card_id}`");
    let _ = writeln!(
        out,
        "- Source Type: `{}`",
        enum_as_str(&source_type).unwrap_or_else(|| "idea".to_owned())
    );
    let _ = writeln!(
        out,
        "- Assistance Mode: `{}`",
        enum_as_str(&assistance_mode).unwrap_or_else(|| "balanced".to_owned())
    );
    let _ = writeln!(out, "- Updated At: `{updated_at}`\n");

    let _ = writeln!(out, "## Brief");
    if brief.trim().is_empty() {
        let _ = writeln!(out, "_No brief provided._\n");
    } else {
        let _ = writeln!(out, "{}\n", brief.trim());
    }

    let _ = writeln!(out, "## Acceptance Criteria");
    if fields.acceptance_criteria.is_empty() {
        let _ = writeln!(out, "- None yet.\n");
    } else {
        for ac in &fields.acceptance_criteria {
            let _ = writeln!(out, "- **{}**: {}", ac.id.trim(), ac.text.trim());
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Constraints");
    if fields.constraints.is_empty() {
        let _ = writeln!(out, "- None.\n");
    } else {
        for item in &fields.constraints {
            let _ = writeln!(out, "- {}", item.trim());
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Source References");
    if fields.source_refs.is_empty() {
        let _ = writeln!(out, "- None.\n");
    } else {
        for path in &fields.source_refs {
            let _ = writeln!(out, "- `{}`", path.trim());
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Wireframes");
    match fields
        .wireframes_index
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(idx) => {
            let _ = writeln!(out, "- Index: `{idx}`");
        }
        None => {
            let _ = writeln!(out, "- None");
        }
    }
    out
}

/// Renders deterministic `card.json` payload for `.legion/workflows/<card-id>/card.json`.
pub(crate) fn render_card_json(
    card: &CardRow,
    fields: &StoredSpecFields,
) -> Result<String, WorkflowBoardRequestError> {
    let val = json!({
        "cardId": card.card_id,
        "projectId": card.project_id,
        "title": card.title,
        "stage": card.stage,
        "status": card.status,
        "revision": card.revision,
        "intakeGoal": card.intake_goal,
        "briefPath": card.brief_path,
        "briefDigest": card.brief_digest,
        "sourceType": card.source_type,
        "assistanceMode": card.assistance_mode,
        "workspaceStrategy": parse_workspace_strategy(&card.workspace_strategy_json)?,
        "worktreePath": card.worktree_path,
        "branch": card.branch,
        "specDigest": card.spec_digest,
        "wireframesDigest": card.wireframes_digest,
        "designDigest": card.design_digest,
        "planDigest": card.plan_digest,
        "tasksDigest": card.tasks_digest,
        "nextAction": card.next_action,
        "spec": fields,
        "createdAt": card.created_at,
        "updatedAt": card.updated_at,
    });
    serde_json::to_string_pretty(&val)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!("Failed to serialize card.json: {e}"),
            )
        })
}

/// Computes a deterministic unified-style diff summary for `workflowBoard.updateSpec`.
pub(crate) fn compute_spec_diff(
    previous_md: Option<&str>,
    previous_digest: Option<&str>,
    next_md: &str,
    next_digest: &str,
    changed_fields: Vec<String>,
) -> Option<WorkflowBoardSpecDiff> {
    if changed_fields.is_empty() && previous_digest == Some(next_digest) {
        return None;
    }
    let prev_lines: Vec<&str> = previous_md.unwrap_or("").lines().collect();
    let next_lines: Vec<&str> = next_md.lines().collect();
    let mut unified = String::from("--- a/spec.md\n+++ b/spec.md\n");
    for line in &prev_lines {
        if !next_lines.contains(line) {
            let _ = writeln!(unified, "-{line}");
        }
    }
    for line in &next_lines {
        if !prev_lines.contains(line) {
            let _ = writeln!(unified, "+{line}");
        }
    }
    Some(WorkflowBoardSpecDiff {
        previous_digest: previous_digest.map(str::to_owned),
        next_digest: next_digest.to_owned(),
        changed_fields,
        unified_diff: unified,
    })
}

pub(crate) struct WireframeEditCtx<'a> {
    pub current: &'a CardDetail,
    pub card_dir: &'a Path,
    pub input: &'a WorkflowBoardUpdateSpecInput,
    pub card_id: &'a str,
    pub fields: &'a mut StoredSpecFields,
    pub changed_fields: &'a mut Vec<String>,
    pub clear_wireframes_digest: &'a mut bool,
    pub next_wireframes_digest: &'a mut Option<String>,
    pub extra_wf_write: &'a mut Option<JournaledWriteSpec>,
}

pub(crate) fn apply_wireframe_edit(
    ctx: &mut WireframeEditCtx<'_>,
) -> Result<(), WorkflowBoardRequestError> {
    match &ctx.input.wireframes_index {
        PatchValue::Absent => {}
        PatchValue::Null => {
            if ctx.fields.wireframes_index.is_some() || ctx.current.card.wireframes_digest.is_some()
            {
                ctx.changed_fields.push("wireframesIndex".to_owned());
                *ctx.clear_wireframes_digest = true;
            }
            ctx.fields.wireframes_index = None;
        }
        PatchValue::Set(v) => {
            let trimmed = v.trim();
            let target = trimmed
                .split('/')
                .fold(ctx.card_dir.to_path_buf(), |p, s| p.join(s));
            if let Some(existing_dg) = read_confined_digest_if_file(ctx.card_dir, &target)? {
                if ctx.fields.wireframes_index.as_deref() != Some(trimmed)
                    || ctx.current.card.wireframes_digest.as_deref() != Some(existing_dg.as_str())
                {
                    ctx.changed_fields.push("wireframesIndex".to_owned());
                    *ctx.next_wireframes_digest = Some(existing_dg);
                }
            } else {
                ctx.changed_fields.push("wireframesIndex".to_owned());
                let html = render_wireframe_starter_html(
                    ctx.card_id,
                    ctx.input
                        .title
                        .as_deref()
                        .unwrap_or(&ctx.current.card.title),
                    ctx.input
                        .brief
                        .as_deref()
                        .unwrap_or(&ctx.current.card.brief),
                );
                let dg = sha256_prefixed(html.as_bytes());
                *ctx.next_wireframes_digest = Some(dg.clone());
                *ctx.extra_wf_write = Some(JournaledWriteSpec {
                    key: trimmed.to_owned(),
                    bytes: html.into_bytes(),
                    expected_digest: dg,
                    revision: Some(ctx.current.card.revision + 1),
                });
            }
            ctx.fields.wireframes_index = Some(trimmed.to_owned());
        }
    }
    Ok(())
}
