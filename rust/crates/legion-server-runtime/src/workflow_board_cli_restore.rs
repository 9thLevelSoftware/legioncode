#![forbid(unsafe_code)]
//! Persisted-state deliverable restoration when `preserve_existing_deliverables` is active.

use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardAssistanceMode, WorkflowBoardPlanDocument, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType, WorkflowBoardTaskContract,
};
use legion_persistence::workflow_board_repo::ApplyCliImportOutcome;
use legion_persistence::workflow_board_repo::types::CardRow;
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_workflows::sha256_prefixed;
use serde_json::Value;

use crate::workflow_board_hydration_plan::hydrate_plan_document;
use crate::workflow_board_journal::JournaledWriteSpec;
use crate::workflow_board_render::{parse_stored_spec_fields, render_spec_markdown};
use crate::workflow_board_render_plan::{
    render_design_markdown, render_plan_markdown, render_task_contract_markdown,
};

fn persisted_plan_doc(
    card: &CardRow,
    planning: Option<&CardPlanningDetail>,
) -> Option<WorkflowBoardPlanDocument> {
    card.plan_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<WorkflowBoardPlanDocument>(raw).ok())
        .or_else(|| planning.and_then(|p| hydrate_plan_document(p).ok().flatten()))
}

fn persisted_tasks<'a>(
    plan_doc: Option<&'a WorkflowBoardPlanDocument>,
    fallback_tasks: &'a [WorkflowBoardTaskContract],
) -> &'a [WorkflowBoardTaskContract] {
    plan_doc
        .filter(|d| !d.tasks.is_empty())
        .map_or(fallback_tasks, |d| d.tasks.as_slice())
}

pub(crate) fn has_missing_cli_deliverable(
    card_dir: &Path,
    card: &CardRow,
    planning: Option<&CardPlanningDetail>,
    fallback_tasks: &[WorkflowBoardTaskContract],
) -> bool {
    if !card_dir.join("card.json").is_file()
        || !card_dir.join("import-provenance.json").is_file()
        || !card_dir.join("spec.md").is_file()
    {
        return true;
    }
    let plan_doc = persisted_plan_doc(card, planning);
    if (card.design_digest.is_some() || card.plan_json.is_some())
        && !card_dir.join("design.md").is_file()
    {
        return true;
    }
    if (card.plan_digest.is_some() || card.plan_json.is_some())
        && !card_dir.join("plan.md").is_file()
    {
        return true;
    }
    persisted_tasks(plan_doc.as_ref(), fallback_tasks)
        .iter()
        .any(|t| {
            !card_dir
                .join("tasks")
                .join(format!("{}.md", t.task_id))
                .is_file()
        })
}

pub(crate) fn restore_missing_persisted_deliverables(
    card_dir: &Path,
    outcome: &ApplyCliImportOutcome,
    planning: Option<&CardPlanningDetail>,
    fallback_title: &str,
    fallback_tasks: &[WorkflowBoardTaskContract],
    revision: i64,
    writes: &mut Vec<JournaledWriteSpec>,
) -> Result<(), WorkflowBoardRequestError> {
    let card = &outcome.card_detail.card;
    let plan_doc = persisted_plan_doc(card, planning);

    if !card_dir.join("spec.md").is_file() {
        let (bytes, expected_digest) = restore_persisted_spec_md(outcome, plan_doc.as_ref())?;
        writes.push(JournaledWriteSpec {
            key: "spec.md".to_owned(),
            bytes,
            expected_digest,
            revision: Some(revision),
        });
    }

    if let Some(ref doc) = plan_doc {
        let mut titles: Vec<&str> = Vec::with_capacity(2);
        titles.push(card.title.as_str());
        if fallback_title != card.title.as_str() {
            titles.push(fallback_title);
        }

        if (card.design_digest.is_some() || card.plan_json.is_some())
            && !card_dir.join("design.md").is_file()
        {
            let (bytes, expected_digest) = restore_plan_artifact(
                "design.md",
                card.design_digest.as_deref(),
                &titles,
                |title| render_design_markdown(&card.card_id, title, doc),
            )?;
            writes.push(JournaledWriteSpec {
                key: "design.md".to_owned(),
                bytes,
                expected_digest,
                revision: Some(revision),
            });
        }

        if (card.plan_digest.is_some() || card.plan_json.is_some())
            && !card_dir.join("plan.md").is_file()
        {
            let (bytes, expected_digest) =
                restore_plan_artifact("plan.md", card.plan_digest.as_deref(), &titles, |title| {
                    render_plan_markdown(&card.card_id, title, doc)
                })?;
            writes.push(JournaledWriteSpec {
                key: "plan.md".to_owned(),
                bytes,
                expected_digest,
                revision: Some(revision),
            });
        }
    }

    for task in persisted_tasks(plan_doc.as_ref(), fallback_tasks) {
        let key = format!("tasks/{}.md", task.task_id);
        if !card_dir
            .join("tasks")
            .join(format!("{}.md", task.task_id))
            .is_file()
        {
            let md = render_task_contract_markdown(&card.card_id, task);
            let bytes = md.into_bytes();
            let expected_digest = sha256_prefixed(&bytes);
            writes.push(JournaledWriteSpec {
                key,
                bytes,
                expected_digest,
                revision: Some(revision),
            });
        }
    }
    Ok(())
}

fn restore_persisted_spec_md(
    outcome: &ApplyCliImportOutcome,
    plan_doc: Option<&WorkflowBoardPlanDocument>,
) -> Result<(Vec<u8>, String), WorkflowBoardRequestError> {
    let card = &outcome.card_detail.card;
    let spec_fields = parse_stored_spec_fields(&card.spec_json);
    let src_type: WorkflowBoardSpecSourceType =
        serde_json::from_value(Value::String(card.source_type.clone()))
            .unwrap_or(WorkflowBoardSpecSourceType::Codebase);
    let assist: WorkflowBoardAssistanceMode =
        serde_json::from_value(Value::String(card.assistance_mode.clone()))
            .unwrap_or(WorkflowBoardAssistanceMode::Balanced);

    let mut timestamps: Vec<&str> = Vec::new();
    for rev in outcome.card_detail.revisions.iter().rev() {
        if rev.spec_digest.as_deref() == card.spec_digest.as_deref()
            && !timestamps.contains(&rev.created_at.as_str())
        {
            timestamps.push(rev.created_at.as_str());
        }
    }
    for ts in [
        outcome.cli_import.imported_at.as_str(),
        outcome.cli_import.last_synced_at.as_str(),
        card.created_at.as_str(),
        card.updated_at.as_str(),
    ] {
        if !timestamps.contains(&ts) {
            timestamps.push(ts);
        }
    }
    if let Some(doc) = plan_doc
        && !timestamps.contains(&doc.updated_at.as_str())
    {
        timestamps.push(doc.updated_at.as_str());
    }

    for ts in timestamps {
        let md = render_spec_markdown(
            &card.card_id,
            &card.title,
            &card.brief,
            src_type,
            assist,
            &spec_fields,
            ts,
        );
        let bytes = md.into_bytes();
        let digest = sha256_prefixed(&bytes);
        if card.spec_digest.as_deref().is_none_or(|exp| exp == digest) {
            return Ok((bytes, digest));
        }
    }

    Err(WorkflowBoardRequestError::new(
        WorkflowBoardRequestErrorCode::DigestMismatch,
        "Unable to restore missing spec.md matching persisted spec digest",
    ))
}

fn restore_plan_artifact(
    artifact_name: &str,
    expected_digest: Option<&str>,
    titles: &[&str],
    render: impl Fn(&str) -> String,
) -> Result<(Vec<u8>, String), WorkflowBoardRequestError> {
    for title in titles {
        let bytes = render(title).into_bytes();
        let digest = sha256_prefixed(&bytes);
        if expected_digest.is_none_or(|exp| exp == digest) {
            return Ok((bytes, digest));
        }
    }
    Err(WorkflowBoardRequestError::new(
        WorkflowBoardRequestErrorCode::DigestMismatch,
        format!("Unable to restore missing {artifact_name} matching persisted digest"),
    ))
}
