#![forbid(unsafe_code)]
//! Deterministic deliverable rendering & contract mapping for `.legion-cli/` imports.

use std::collections::HashSet;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAssistanceMode, WorkflowBoardCliImportOwner,
    WorkflowBoardCliImportPolicyStatus, WorkflowBoardCliImportProvenance,
    WorkflowBoardCliImportSourceSystem, WorkflowBoardCliImportUnsupportedPolicy,
    WorkflowBoardEvidenceMethod, WorkflowBoardPlanDocument, WorkflowBoardPlanLens,
    WorkflowBoardPlanLensEntry, WorkflowBoardPlannedCheck, WorkflowBoardPlanningStrategy,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType,
    WorkflowBoardTaskContract, WorkflowBoardTaskGranularity, WorkflowBoardTaskStatus,
};
use legion_persistence::workflow_board_repo::ApplyCliImportOutcome;
use legion_persistence::workflow_board_repo::types::CardRow;
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_workflows::sha256_prefixed;
use serde_json::Value;

use crate::workflow_board_cli_restore::restore_missing_persisted_deliverables;
use crate::workflow_board_cli_scan::ScannedCliProject;
use crate::workflow_board_journal::JournaledWriteSpec;
use crate::workflow_board_render::{
    StoredSpecFields, parse_stored_spec_fields, render_card_json, render_spec_markdown,
};
use crate::workflow_board_render_plan::{
    render_design_markdown, render_plan_markdown, render_task_contract_markdown,
};

pub(crate) struct RenderedCliImportArtifacts {
    pub spec_fields: StoredSpecFields,
    pub spec_json: String,
    pub spec_md: String,
    pub spec_digest: String,
    pub plan_doc: WorkflowBoardPlanDocument,
    pub plan_json: String,
    pub design_md: String,
    pub design_digest: String,
    pub plan_md: String,
    pub plan_digest: String,
    pub tasks_digest: String,
    pub task_files: Vec<(String, String)>,
    pub provenance: WorkflowBoardCliImportProvenance,
    pub provenance_json: String,
    pub unsupported_policies_json: String,
    pub acknowledged_policies_json: String,
}

pub(crate) fn build_cli_import_artifacts(
    card_id: &str,
    scan: &ScannedCliProject,
    ack_all: bool,
    ack_keys: Option<&[String]>,
    existing_acked: &[String],
    now_iso: &str,
) -> Result<RenderedCliImportArtifacts, WorkflowBoardRequestError> {
    let spec_fields = StoredSpecFields {
        acceptance_criteria: scan.acceptance_criteria.clone(),
        constraints: scan.constraints.clone(),
        source_refs: scan.source_refs.clone(),
        wireframes_index: None,
        source_digest: Some(scan.source_digest.clone()),
        source_input_digests: Vec::new(),
        intake_analysis: None,
    };
    let spec_json = serde_json::to_string(&spec_fields).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to serialize spec_json: {e}"),
        )
    })?;
    let spec_md = render_spec_markdown(
        card_id,
        &scan.title,
        &scan.brief,
        WorkflowBoardSpecSourceType::Codebase,
        WorkflowBoardAssistanceMode::Balanced,
        &spec_fields,
        now_iso,
    );
    let spec_digest = sha256_prefixed(spec_md.as_bytes());

    let tasks = build_task_contracts(scan);
    let (planned_checks, ac_mappings) = build_checks_and_ac_mappings(scan, &tasks);
    let (task_files, tasks_digest) = build_task_markdowns(card_id, &tasks);
    let lenses = build_default_lenses(scan);
    let mut plan_doc = build_plan_doc(
        lenses,
        tasks,
        planned_checks,
        ac_mappings,
        &tasks_digest,
        now_iso,
    );

    let design_md = render_design_markdown(card_id, &scan.title, &plan_doc);
    let design_digest = sha256_prefixed(design_md.as_bytes());
    let plan_md = render_plan_markdown(card_id, &scan.title, &plan_doc);
    let plan_digest = sha256_prefixed(plan_md.as_bytes());
    plan_doc.plan_digest = Some(plan_digest.clone());
    let plan_json = serde_json::to_string(&plan_doc).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to serialize plan_json: {e}"),
        )
    })?;

    let (unsupported_policies, acknowledged_policies) =
        resolve_policy_acknowledgements(scan, ack_all, ack_keys, existing_acked);
    let provenance = build_provenance_doc(
        scan,
        unsupported_policies.clone(),
        acknowledged_policies.clone(),
        now_iso,
    );
    let provenance_json = serde_json::to_string_pretty(&provenance).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to serialize import-provenance.json: {e}"),
        )
    })?;
    let unsupported_policies_json =
        serde_json::to_string(&unsupported_policies).unwrap_or_else(|_| "[]".to_owned());
    let acknowledged_policies_json =
        serde_json::to_string(&acknowledged_policies).unwrap_or_else(|_| "[]".to_owned());

    Ok(RenderedCliImportArtifacts {
        spec_fields,
        spec_json,
        spec_md,
        spec_digest,
        plan_doc,
        plan_json,
        design_md,
        design_digest,
        plan_md,
        plan_digest,
        tasks_digest,
        task_files,
        provenance,
        provenance_json,
        unsupported_policies_json,
        acknowledged_policies_json,
    })
}

fn build_task_contracts(scan: &ScannedCliProject) -> Vec<WorkflowBoardTaskContract> {
    scan.tasks
        .iter()
        .map(|t| WorkflowBoardTaskContract {
            id: Some(t.id.clone()),
            task_id: t.id.clone(),
            title: t.title.clone(),
            depends_on: None,
            blocked_by: Some(t.blocked_by.clone()),
            blocks: Some(t.blocks.clone()),
            files_allowed: t.files_allowed.clone(),
            files_forbidden: Some(t.files_forbidden.clone()),
            expected_artifacts: Some(t.expected_artifacts.clone()),
            verification_commands: Some(t.verification_commands.clone()),
            testing_method: t.testing_method,
            testing_limitations: t.blocker_summary.clone(),
            status: Some(t.status),
        })
        .collect()
}

fn build_checks_and_ac_mappings(
    scan: &ScannedCliProject,
    _tasks: &[WorkflowBoardTaskContract],
) -> (
    Vec<WorkflowBoardPlannedCheck>,
    Vec<WorkflowBoardAcEvidenceMapping>,
) {
    let mut planned_checks = Vec::new();
    let mut task_to_checks: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for t in &scan.tasks {
        let total = t.verification_commands.len();
        let mut ids = Vec::with_capacity(total);
        for (idx, argv) in t.verification_commands.iter().enumerate() {
            let check_id = if total == 1 {
                format!("chk-{}", t.id.to_ascii_lowercase())
            } else {
                format!("chk-{}-{}", t.id.to_ascii_lowercase(), idx + 1)
            };
            ids.push(check_id.clone());
            planned_checks.push(WorkflowBoardPlannedCheck {
                id: check_id,
                criterion_id: t.acceptance_ids.first().cloned(),
                argv: argv.clone(),
                description: Some(format!("Imported CLI check for {}", t.id)),
            });
        }
        if !ids.is_empty() {
            task_to_checks.insert(t.id.clone(), ids);
        }
    }
    let mut ac_mappings = Vec::new();
    for ac_id in &scan.acceptance_criterion_ids {
        let linked_checks: Vec<String> = scan
            .tasks
            .iter()
            .filter(|t| t.acceptance_ids.iter().any(|id| id == ac_id))
            .flat_map(|t| task_to_checks.get(&t.id).into_iter().flatten().cloned())
            .collect();
        if linked_checks.is_empty() {
            ac_mappings.push(WorkflowBoardAcEvidenceMapping {
                criterion_id: ac_id.clone(),
                method: WorkflowBoardEvidenceMethod::Manual,
                check_ids: None,
                command: None,
                procedure: Some(
                    "Imported from .legion-cli; native verification required before Ship."
                        .to_owned(),
                ),
                external_ref: None,
                external_source: None,
                limitations: None,
            });
        } else {
            ac_mappings.push(WorkflowBoardAcEvidenceMapping {
                criterion_id: ac_id.clone(),
                method: WorkflowBoardEvidenceMethod::Automated,
                check_ids: Some(linked_checks),
                command: None,
                procedure: None,
                external_ref: None,
                external_source: None,
                limitations: None,
            });
        }
    }
    (planned_checks, ac_mappings)
}

fn build_task_markdowns(
    card_id: &str,
    tasks: &[WorkflowBoardTaskContract],
) -> (Vec<(String, String)>, String) {
    let mut task_files = Vec::with_capacity(tasks.len());
    let mut tasks_digest_buf = String::new();
    for t in tasks {
        let md = render_task_contract_markdown(card_id, t);
        let dg = sha256_prefixed(md.as_bytes());
        tasks_digest_buf.push_str(&t.task_id);
        tasks_digest_buf.push(':');
        tasks_digest_buf.push_str(&dg);
        tasks_digest_buf.push('\n');
        task_files.push((format!("tasks/{}.md", t.task_id), md));
    }
    (task_files, sha256_prefixed(tasks_digest_buf.as_bytes()))
}

fn build_default_lenses(scan: &ScannedCliProject) -> Vec<WorkflowBoardPlanLensEntry> {
    vec![WorkflowBoardPlanLensEntry {
        lens: WorkflowBoardPlanLens::Architecture,
        applicable: Some(true),
        summary: Some(
            scan.plan_design_markdown
                .as_deref()
                .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("Imported CLI architecture design")
                .trim()
                .to_owned(),
        ),
        reason: None,
    }]
}

fn build_plan_doc(
    lenses: Vec<WorkflowBoardPlanLensEntry>,
    tasks: Vec<WorkflowBoardTaskContract>,
    checks: Vec<WorkflowBoardPlannedCheck>,
    ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
    tasks_digest: &str,
    now_iso: &str,
) -> WorkflowBoardPlanDocument {
    WorkflowBoardPlanDocument {
        lenses,
        alternatives: Vec::new(),
        alternatives_revision_count: 0,
        selected_alternative_id: None,
        selection_rationale: None,
        strategy: Some(WorkflowBoardPlanningStrategy::RiskFirst),
        strategy_rationale: None,
        strategy_metadata: None,
        granularity: Some(WorkflowBoardTaskGranularity::Balanced),
        tasks,
        checks,
        ac_mappings,
        plan_digest: None,
        tasks_digest: Some(tasks_digest.to_owned()),
        updated_at: now_iso.to_owned(),
    }
}

fn resolve_policy_acknowledgements(
    scan: &ScannedCliProject,
    ack_all: bool,
    ack_keys: Option<&[String]>,
    existing_acked: &[String],
) -> (Vec<WorkflowBoardCliImportUnsupportedPolicy>, Vec<String>) {
    let present_keys: HashSet<&str> = scan
        .unsupported_policies
        .iter()
        .map(|p| p.key.as_str())
        .collect();
    let mut acked_set: HashSet<String> = existing_acked
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty() && present_keys.contains(k))
        .map(str::to_owned)
        .collect();
    if let Some(keys) = ack_keys {
        for k in keys {
            let t = k.trim();
            if !t.is_empty() && present_keys.contains(t) {
                acked_set.insert(t.to_owned());
            }
        }
    } else if ack_all {
        for p in &scan.unsupported_policies {
            acked_set.insert(p.key.clone());
        }
    }
    let mut unsupported_policies = scan.unsupported_policies.clone();
    for p in &mut unsupported_policies {
        if acked_set.contains(&p.key) {
            p.status = WorkflowBoardCliImportPolicyStatus::Acknowledged;
        }
    }
    let mut acknowledged_policies: Vec<String> = acked_set.into_iter().collect();
    acknowledged_policies.sort();
    (unsupported_policies, acknowledged_policies)
}

fn build_provenance_doc(
    scan: &ScannedCliProject,
    unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    acknowledged_policies: Vec<String>,
    now_iso: &str,
) -> WorkflowBoardCliImportProvenance {
    WorkflowBoardCliImportProvenance {
        source_system: WorkflowBoardCliImportSourceSystem::LegionCli,
        source_cli_dir: scan.source_cli_dir.clone(),
        source_spec_id: scan.spec_id.clone(),
        owner: WorkflowBoardCliImportOwner::Legioncode,
        imported_at: now_iso.to_owned(),
        last_synced_at: now_iso.to_owned(),
        source_digest: Some(scan.source_digest.clone()),
        decision_ids: scan.decisions.iter().map(|d| d.id.clone()).collect(),
        assumption_ids: scan.assumption_ids.clone(),
        acceptance_criterion_ids: scan.acceptance_criterion_ids.clone(),
        historical_receipts: scan.historical_receipts.clone(),
        unsupported_policies,
        acknowledged_policies,
        missing_log_notes: scan.missing_log_notes.clone(),
        preserved_artifact_paths: scan.preserved_artifact_paths.clone(),
        assurance_policy_summary: scan.assurance_policy_summary.clone(),
    }
}

pub(crate) fn render_imported_card_json(
    card: &CardRow,
    fields: &StoredSpecFields,
    provenance: &WorkflowBoardCliImportProvenance,
) -> Result<String, WorkflowBoardRequestError> {
    let base = render_card_json(card, fields)?;
    let mut val: Value = serde_json::from_str(&base).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to parse rendered card.json: {e}"),
        )
    })?;
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "importProvenance".to_owned(),
            serde_json::to_value(provenance).unwrap_or(Value::Null),
        );
        obj.insert(
            "unsupportedPolicies".to_owned(),
            serde_json::to_value(&provenance.unsupported_policies).unwrap_or(Value::Null),
        );
    }
    serde_json::to_string_pretty(&val)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!("Failed to serialize imported card.json: {e}"),
            )
        })
}

pub(crate) fn blocked_task_id(scan: &ScannedCliProject) -> Option<String> {
    scan.tasks
        .iter()
        .find(|t| t.status == WorkflowBoardTaskStatus::Blocked)
        .map(|t| t.id.clone())
}

pub(crate) fn build_cli_import_journal_writes(
    outcome: &ApplyCliImportOutcome,
    planning: Option<&CardPlanningDetail>,
    fallback_title: &str,
    rendered: &RenderedCliImportArtifacts,
    authoritative_prov: Option<&WorkflowBoardCliImportProvenance>,
    card_dir: &Path,
    preserve_existing_deliverables: bool,
) -> Result<Vec<JournaledWriteSpec>, WorkflowBoardRequestError> {
    let card = &outcome.card_detail.card;
    let revision = outcome.revision;
    let prov = authoritative_prov.unwrap_or(&rendered.provenance);
    let spec_fields = if preserve_existing_deliverables {
        parse_stored_spec_fields(&card.spec_json)
    } else {
        rendered.spec_fields.clone()
    };
    let card_json = render_imported_card_json(card, &spec_fields, prov)?;
    let prov_json = serde_json::to_string_pretty(prov).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to serialize import-provenance.json: {e}"),
        )
    })?;
    let rev = Some(revision);
    let mut writes = vec![
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: rev,
        },
        JournaledWriteSpec {
            key: "import-provenance.json".to_owned(),
            bytes: prov_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(prov_json.as_bytes()),
            revision: rev,
        },
    ];
    if preserve_existing_deliverables {
        restore_missing_persisted_deliverables(
            card_dir,
            outcome,
            planning,
            fallback_title,
            &rendered.plan_doc.tasks,
            revision,
            &mut writes,
        )?;
        return Ok(writes);
    }

    let deliverables = [
        (
            "spec.md",
            rendered.spec_md.as_bytes(),
            &rendered.spec_digest,
        ),
        (
            "design.md",
            rendered.design_md.as_bytes(),
            &rendered.design_digest,
        ),
        (
            "plan.md",
            rendered.plan_md.as_bytes(),
            &rendered.plan_digest,
        ),
    ];
    for (key, bytes, digest) in deliverables {
        writes.push(JournaledWriteSpec {
            key: key.to_owned(),
            bytes: bytes.to_vec(),
            expected_digest: digest.clone(),
            revision: rev,
        });
    }
    for (key, md) in &rendered.task_files {
        writes.push(JournaledWriteSpec {
            key: key.clone(),
            bytes: md.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(md.as_bytes()),
            revision: rev,
        });
    }
    Ok(writes)
}
