//! M3 deserialization and domain validators mirroring `workflowBoard.ts` / `workflowBoardM3.ts`.

use serde::Deserializer;

use super::m2_models::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardPlannedCheck, WorkflowBoardTaskContract,
};
use super::m2_plan_validators::{validate_ac_mappings_partial, validate_tasks_dag_and_contracts};
use super::m2_validators::is_valid_argv_command;
use super::m3_models::{
    WorkflowBoardFindingResolveDisposition, WorkflowBoardManualAcceptanceVerdict,
};
use super::m3_ship_models::WorkflowBoardRetryStepKind;
use super::validators::{deserialize_artifact_key_vec, is_valid_artifact_key};

/// Deserializes an array of safe relative artifact keys, rejecting explicit JSON `null`.
pub(crate) fn deserialize_present_artifact_key_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_artifact_key_vec(deserializer).map(Some)
}

/// Validates `WorkflowBoardRetryStepInput` invariants (`task` / `check` require non-empty `targetId`).
pub(crate) fn validate_retry_step(
    step_kind: WorkflowBoardRetryStepKind,
    target_id: Option<&str>,
) -> Result<(), String> {
    if matches!(
        step_kind,
        WorkflowBoardRetryStepKind::Task | WorkflowBoardRetryStepKind::Check
    ) && target_id.map(str::trim).is_none_or(str::is_empty)
    {
        return Err("retryStep requires non-empty `targetId` for `task` or `check`".to_owned());
    }
    Ok(())
}

/// Validates `WorkflowBoardResolveFindingInput` invariants (`dismiss` / `accept_risk` require `reason`).
pub(crate) fn validate_resolve_finding(
    disposition: WorkflowBoardFindingResolveDisposition,
    reason: Option<&str>,
) -> Result<(), String> {
    if matches!(
        disposition,
        WorkflowBoardFindingResolveDisposition::Dismiss
            | WorkflowBoardFindingResolveDisposition::AcceptRisk
    ) && reason.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(
            "resolveFinding requires non-empty `reason` for `dismiss` or `accept_risk`".to_owned(),
        );
    }
    Ok(())
}

/// Validates `WorkflowBoardRecordManualAcceptanceInput` invariants (`fail` / `not-applicable` require `note`).
pub(crate) fn validate_manual_acceptance(
    verdict: WorkflowBoardManualAcceptanceVerdict,
    note: Option<&str>,
) -> Result<(), String> {
    if matches!(
        verdict,
        WorkflowBoardManualAcceptanceVerdict::Fail
            | WorkflowBoardManualAcceptanceVerdict::NotApplicable
    ) && note.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(
            "recordManualAcceptance requires non-empty `note` for `fail` or `not-applicable`"
                .to_owned(),
        );
    }
    Ok(())
}

/// Normalizes review finding severity aliases (`blocking|critical -> critical`, `warning|major -> major`,
/// `note|info -> info`, `minor -> minor`, fallback `major`).
#[must_use]
pub fn normalize_review_finding_severity(raw: Option<&str>) -> &'static str {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("critical" | "blocking") => "critical",
        Some("minor") => "minor",
        Some("info" | "note") => "info",
        _ => "major",
    }
}

/// Validates `WorkflowBoardProposeAmendmentInput` tasks, planned checks, AC mappings, and deliverables.
pub(crate) fn validate_propose_amendment(
    tasks: Option<&[WorkflowBoardTaskContract]>,
    planned_checks: Option<&[WorkflowBoardPlannedCheck]>,
    ac_mappings: Option<&[WorkflowBoardAcEvidenceMapping]>,
    deliverables: Option<&[String]>,
) -> Result<(), String> {
    if let Some(tasks) = tasks {
        validate_tasks_dag_and_contracts(tasks)?;
    }
    if let Some(checks) = planned_checks {
        for ch in checks {
            if !is_valid_argv_command(&ch.argv) {
                return Err(format!(
                    "plannedCheck `{}` must use a safe argv-only command",
                    ch.id
                ));
            }
        }
    }
    if let Some(mappings) = ac_mappings {
        validate_ac_mappings_partial(mappings)?;
    }
    if let Some(deliverables) = deliverables {
        for key in deliverables {
            if !is_valid_artifact_key(key) {
                return Err(format!("invalid deliverable key `{key}`"));
            }
        }
    }
    Ok(())
}
