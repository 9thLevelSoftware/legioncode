#![forbid(unsafe_code)]
//! Owned seed struct builders for `.legion-cli/` persistence upserts.

use legion_persistence::workflow_board_repo::{
    CliImportAcEvidenceSeed, CliImportCheckReceiptSeed, CliImportDecisionSeed,
    CliImportPlannedCheckSeed, CliImportTaskSeed,
};

use crate::workflow_board_cli_render::{RenderedCliImportArtifacts, blocked_task_id};
use crate::workflow_board_cli_scan::ScannedCliProject;
use crate::workflow_board_types::enum_as_str;

pub(crate) struct OwnedDecision {
    pub id: String,
    pub prompt: String,
    pub status: String,
    pub answer: Option<String>,
    pub rationale: Option<String>,
}

pub(crate) struct OwnedTask {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub parent_id: Option<String>,
    pub blocked_by_json: String,
    pub blocks_json: String,
    pub files_allowed_json: String,
    pub files_forbidden_json: String,
    pub expected_artifacts_json: String,
    pub verification_commands_json: String,
    pub testing_method: String,
    pub acceptance_ids_json: String,
    pub blocker_reason: Option<String>,
    pub notes: Option<String>,
    pub max_files_touched: Option<i64>,
}

pub(crate) struct OwnedPlannedCheck {
    pub check_id: String,
    pub owner_task_id: Option<String>,
    pub argv_json: String,
    pub label: Option<String>,
    pub last_exit_code: Option<i64>,
    pub last_log_ref: Option<String>,
    pub stale_reason: Option<String>,
    pub last_receipt_id: Option<String>,
    pub last_run_at: Option<String>,
    pub last_output_excerpt: Option<String>,
}

pub(crate) struct OwnedReceipt {
    pub id: String,
    pub check_id: String,
    pub criterion_id: Option<String>,
    pub task_id: Option<String>,
    pub argv_json: String,
    pub exit_code: Option<i64>,
    pub log_ref: Option<String>,
    pub stderr_excerpt: Option<String>,
    pub stale_reason: Option<String>,
    pub executed_at: Option<String>,
}

pub(crate) struct OwnedAc {
    pub criterion_id: String,
    pub method: String,
    pub check_ids_json: String,
    pub command_argv_json: Option<String>,
    pub procedure: Option<String>,
    pub observation_note: Option<String>,
    pub external_source: Option<String>,
}

pub(crate) struct OwnedCliImportSeeds {
    pub decisions: Vec<OwnedDecision>,
    pub tasks: Vec<OwnedTask>,
    pub planned_checks: Vec<OwnedPlannedCheck>,
    pub receipts: Vec<OwnedReceipt>,
    pub acs: Vec<OwnedAc>,
}

impl OwnedCliImportSeeds {
    pub(crate) fn build(
        scan: &ScannedCliProject,
        rendered: &RenderedCliImportArtifacts,
        current_task_id: Option<&str>,
    ) -> Self {
        let decisions = scan
            .decisions
            .iter()
            .map(|d| OwnedDecision {
                id: d.id.clone(),
                prompt: d.prompt.clone(),
                status: d.status.clone(),
                answer: d.answer.clone(),
                rationale: d.rationale.clone(),
            })
            .collect();
        let tasks = build_owned_tasks(scan, rendered);
        let planned_checks = build_owned_checks(scan, rendered, current_task_id);
        let receipts = build_owned_receipts(scan, &planned_checks);
        let acs = build_owned_acs(scan, rendered);
        Self {
            decisions,
            tasks,
            planned_checks,
            receipts,
            acs,
        }
    }

    pub(crate) fn decision_seeds(&self) -> Vec<CliImportDecisionSeed<'_>> {
        self.decisions
            .iter()
            .map(|d| CliImportDecisionSeed {
                decision_id: &d.id,
                prompt: &d.prompt,
                status: &d.status,
                answer: d.answer.as_deref(),
                rationale: d.rationale.as_deref(),
            })
            .collect()
    }

    pub(crate) fn task_seeds<'a>(&'a self, tasks_digest: &'a str) -> Vec<CliImportTaskSeed<'a>> {
        self.tasks
            .iter()
            .enumerate()
            .map(|(idx, t)| CliImportTaskSeed {
                task_id: &t.task_id,
                ordinal: i64::try_from(idx).unwrap_or(0),
                title: &t.title,
                status: &t.status,
                priority: &t.priority,
                parent_id: t.parent_id.as_deref(),
                blocked_by_json: &t.blocked_by_json,
                blocks_json: &t.blocks_json,
                files_allowed_json: &t.files_allowed_json,
                files_forbidden_json: &t.files_forbidden_json,
                expected_artifacts_json: &t.expected_artifacts_json,
                verification_commands_json: &t.verification_commands_json,
                testing_method: &t.testing_method,
                acceptance_ids_json: &t.acceptance_ids_json,
                blocker_reason: t.blocker_reason.as_deref(),
                notes: t.notes.as_deref(),
                max_files_touched: t.max_files_touched,
                contract_digest: Some(tasks_digest),
                last_error: t.blocker_reason.as_deref(),
            })
            .collect()
    }

    pub(crate) fn planned_check_seeds(&self) -> Vec<CliImportPlannedCheckSeed<'_>> {
        self.planned_checks
            .iter()
            .map(|c| CliImportPlannedCheckSeed {
                check_id: &c.check_id,
                owner_task_id: c.owner_task_id.as_deref(),
                argv_json: &c.argv_json,
                label: c.label.as_deref(),
                last_exit_code: c.last_exit_code,
                last_log_ref: c.last_log_ref.as_deref(),
                stale_reason: c.stale_reason.as_deref(),
                last_receipt_id: c.last_receipt_id.as_deref(),
                last_run_at: c.last_run_at.as_deref(),
                last_output_excerpt: c.last_output_excerpt.as_deref(),
            })
            .collect()
    }

    pub(crate) fn check_receipt_seeds(&self) -> Vec<CliImportCheckReceiptSeed<'_>> {
        self.receipts
            .iter()
            .map(|r| CliImportCheckReceiptSeed {
                id: &r.id,
                check_id: &r.check_id,
                criterion_id: r.criterion_id.as_deref(),
                task_id: r.task_id.as_deref(),
                argv_json: &r.argv_json,
                exit_code: r.exit_code,
                log_ref: r.log_ref.as_deref(),
                stderr_excerpt: r.stderr_excerpt.as_deref(),
                stale_reason: r.stale_reason.as_deref(),
                executed_at: r.executed_at.as_deref(),
            })
            .collect()
    }

    pub(crate) fn ac_evidence_seeds(&self) -> Vec<CliImportAcEvidenceSeed<'_>> {
        self.acs
            .iter()
            .map(|a| CliImportAcEvidenceSeed {
                criterion_id: &a.criterion_id,
                method: &a.method,
                check_ids_json: &a.check_ids_json,
                command_argv_json: a.command_argv_json.as_deref(),
                procedure: a.procedure.as_deref(),
                observation_note: a.observation_note.as_deref(),
                external_source: a.external_source.as_deref(),
            })
            .collect()
    }
}

fn build_owned_tasks(
    scan: &ScannedCliProject,
    rendered: &RenderedCliImportArtifacts,
) -> Vec<OwnedTask> {
    rendered
        .plan_doc
        .tasks
        .iter()
        .zip(scan.tasks.iter())
        .map(|(contract, raw)| OwnedTask {
            task_id: contract.task_id.clone(),
            title: contract.title.clone(),
            status: enum_as_str(&raw.status).unwrap_or_else(|| "todo".to_owned()),
            priority: raw.priority.clone(),
            parent_id: raw.parent_id.clone(),
            blocked_by_json: serde_json::to_string(&raw.blocked_by)
                .unwrap_or_else(|_| "[]".to_owned()),
            blocks_json: serde_json::to_string(&raw.blocks).unwrap_or_else(|_| "[]".to_owned()),
            files_allowed_json: serde_json::to_string(&contract.files_allowed)
                .unwrap_or_else(|_| "[]".to_owned()),
            files_forbidden_json: serde_json::to_string(
                contract.files_forbidden.as_deref().unwrap_or(&[]),
            )
            .unwrap_or_else(|_| "[]".to_owned()),
            expected_artifacts_json: serde_json::to_string(
                contract.expected_artifacts.as_deref().unwrap_or(&[]),
            )
            .unwrap_or_else(|_| "[]".to_owned()),
            verification_commands_json: serde_json::to_string(
                contract.verification_commands.as_deref().unwrap_or(&[]),
            )
            .unwrap_or_else(|_| "[]".to_owned()),
            testing_method: enum_as_str(&raw.testing_method)
                .unwrap_or_else(|| "test-first".to_owned()),
            acceptance_ids_json: serde_json::to_string(&raw.acceptance_ids)
                .unwrap_or_else(|_| "[]".to_owned()),
            blocker_reason: raw.blocker_summary.clone(),
            notes: raw.notes.clone(),
            max_files_touched: raw.max_files_touched,
        })
        .collect()
}

fn build_owned_checks(
    scan: &ScannedCliProject,
    rendered: &RenderedCliImportArtifacts,
    current_task_id: Option<&str>,
) -> Vec<OwnedPlannedCheck> {
    rendered
        .plan_doc
        .checks
        .iter()
        .map(|c| {
            let argv_json = serde_json::to_string(&c.argv).unwrap_or_else(|_| "[]".to_owned());
            let owner_task_id = scan
                .tasks
                .iter()
                .find(|t| check_belongs_to_task(&c.id, &t.id))
                .map(|t| t.id.clone());
            let is_blocked_task = owner_task_id.as_deref() == current_task_id
                && scan.execution_state.missing_log_relpath.is_some();
            OwnedPlannedCheck {
                check_id: c.id.clone(),
                owner_task_id,
                argv_json,
                label: c.description.clone(),
                last_exit_code: if is_blocked_task { Some(1_i64) } else { None },
                last_log_ref: if is_blocked_task {
                    scan.execution_state.missing_log_relpath.clone()
                } else {
                    None
                },
                stale_reason: Some("imported_historical_cli_receipt".to_owned()),
                last_receipt_id: if is_blocked_task {
                    Some("hist-chk-1".to_owned())
                } else {
                    None
                },
                last_run_at: scan.execution_state.updated_at.clone(),
                last_output_excerpt: if is_blocked_task {
                    scan.execution_state.blocker.clone()
                } else {
                    None
                },
            }
        })
        .collect()
}

fn check_belongs_to_task(check_id: &str, task_id: &str) -> bool {
    let base = format!("chk-{}", task_id.to_ascii_lowercase());
    if check_id == base {
        return true;
    }
    check_id
        .strip_prefix(&format!("{base}-"))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn build_owned_receipts(
    scan: &ScannedCliProject,
    planned_checks: &[OwnedPlannedCheck],
) -> Vec<OwnedReceipt> {
    let mut receipts = Vec::new();
    if let Some(ref blocker) = scan.execution_state.blocker {
        let failed_task = blocked_task_id(scan);
        let failed_check = planned_checks
            .iter()
            .find(|c| c.owner_task_id == failed_task)
            .map_or_else(
                || {
                    (
                        "chk-cli-exec-1".to_owned(),
                        "[\"node\",\"--test\"]".to_owned(),
                    )
                },
                |c| (c.check_id.clone(), c.argv_json.clone()),
            );
        receipts.push(OwnedReceipt {
            id: "chk-exec-1".to_owned(),
            check_id: failed_check.0,
            criterion_id: scan.acceptance_criterion_ids.first().cloned(),
            task_id: failed_task,
            argv_json: failed_check.1,
            exit_code: Some(1_i64),
            log_ref: scan.execution_state.missing_log_relpath.clone(),
            stderr_excerpt: Some(blocker.clone()),
            stale_reason: Some("imported_historical_cli_receipt".to_owned()),
            executed_at: scan.execution_state.updated_at.clone(),
        });
    }
    receipts
}

fn build_owned_acs(
    scan: &ScannedCliProject,
    rendered: &RenderedCliImportArtifacts,
) -> Vec<OwnedAc> {
    rendered
        .plan_doc
        .ac_mappings
        .iter()
        .map(|m| OwnedAc {
            criterion_id: m.criterion_id.clone(),
            method: enum_as_str(&m.method).unwrap_or_else(|| "manual".to_owned()),
            check_ids_json: serde_json::to_string(m.check_ids.as_deref().unwrap_or(&[]))
                .unwrap_or_else(|_| "[]".to_owned()),
            command_argv_json: m
                .command
                .as_ref()
                .and_then(|c| serde_json::to_string(c).ok()),
            procedure: m.procedure.clone(),
            observation_note: Some(
                "Imported from .legion-cli; native verification required".to_owned(),
            ),
            external_source: Some(format!(
                "{}/workflow/plan-approval.yaml",
                scan.source_cli_dir
            )),
        })
        .collect()
}
