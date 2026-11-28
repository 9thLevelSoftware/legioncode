#![forbid(unsafe_code)]
//! Enum/JSON parsing helpers, revision CAS helpers, and error mapping for [`crate::workflow_board`].

use legion_contracts::ids::ProviderInstanceId;
use legion_contracts::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardArtifactKind, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_persistence::workflow_board_repo::WorkflowBoardRepoError;
use legion_persistence::workflow_board_repo::types::StageRunRow;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::thread_launcher::ThreadLaunchError;

pub(crate) fn parse_workspace_strategy(
    raw: &str,
) -> Result<WorkspaceStrategy, WorkflowBoardRequestError> {
    serde_json::from_str::<WorkspaceStrategy>(raw)
        .map_err(|e| internal(format!("Invalid persisted workspace_strategy JSON: {e}")))
}

pub(crate) fn parse_stage_run_model_selection(
    row: &StageRunRow,
) -> Result<ModelSelection, WorkflowBoardRequestError> {
    let model_options = row
        .model_options_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| internal(format!("Invalid stage run model_options_json: {e}")))?;
    Ok(ModelSelection {
        instance_id: ProviderInstanceId::new(&row.provider_instance_id)
            .map_err(|e| internal(e.to_string()))?,
        model: row.model.clone(),
        options: model_options,
    })
}

pub(crate) fn parse_opt_model_selection(
    raw: Option<&str>,
) -> Result<Option<ModelSelection>, WorkflowBoardRequestError> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|text| {
            serde_json::from_str::<ModelSelection>(text)
                .map_err(|e| internal(format!("Invalid persisted model_selection JSON: {e}")))
        })
        .transpose()
}

pub(crate) fn parse_opt_enum<T: for<'de> Deserialize<'de>>(
    raw: Option<&str>,
    label: &str,
) -> Result<Option<T>, WorkflowBoardRequestError> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parse_enum(s, label))
        .transpose()
}

pub(crate) fn parse_opt_runtime_mode(
    raw: Option<&str>,
) -> Result<Option<RuntimeMode>, WorkflowBoardRequestError> {
    parse_opt_enum(raw, "runtime_mode")
}

pub(crate) fn parse_opt_interaction_mode(
    raw: Option<&str>,
) -> Result<Option<ProviderInteractionMode>, WorkflowBoardRequestError> {
    parse_opt_enum(raw, "interaction_mode")
}

pub(crate) fn enum_as_str<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

pub(crate) fn parse_enum<T: for<'de> Deserialize<'de>>(
    raw: &str,
    label: &str,
) -> Result<T, WorkflowBoardRequestError> {
    serde_json::from_value::<T>(Value::String(raw.to_owned()))
        .map_err(|e| internal(format!("Invalid persisted {label} `{raw}`: {e}")))
}

pub(crate) fn infer_artifact_kind(key: &str) -> WorkflowBoardArtifactKind {
    match key {
        "card.json" => WorkflowBoardArtifactKind::Card,
        "spec.md" | "spec-brief.md" => WorkflowBoardArtifactKind::Spec,
        "plan.md" | "plan-brief.md" => WorkflowBoardArtifactKind::Plan,
        "design.md" => WorkflowBoardArtifactKind::Design,
        _ if key.starts_with("wireframes/")
            || std::path::Path::new(key)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html")) =>
        {
            WorkflowBoardArtifactKind::Wireframes
        }
        _ => WorkflowBoardArtifactKind::Plan,
    }
}

pub(crate) fn wb_err(
    code: WorkflowBoardRequestErrorCode,
    msg: impl Into<String>,
) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(code, msg)
}

pub(crate) fn map_repo_err(err: WorkflowBoardRepoError) -> WorkflowBoardRequestError {
    use WorkflowBoardRequestErrorCode as C;
    match err {
        WorkflowBoardRepoError::CardNotFound { .. } => wb_err(C::CardNotFound, err.to_string()),
        WorkflowBoardRepoError::RevisionConflict {
            actual_revision, ..
        } => {
            let rev = u64::try_from(actual_revision.max(1)).unwrap_or(1);
            wb_err(C::RevisionConflict, err.to_string()).with_current_revision(rev)
        }
        WorkflowBoardRepoError::PreconditionFailed(msg) => {
            let lower = msg.to_ascii_lowercase();
            let code = if lower.contains("exploration")
                || lower.contains("decision")
                || lower.contains("continue_round")
            {
                C::ExplorationUnresolved
            } else if lower.contains("challenge")
                || lower.contains("manualfallback")
                || lower.contains("synthesis")
            {
                C::ChallengeUnresolved
            } else if lower.contains("digest mismatch") {
                C::DigestMismatch
            } else if lower.contains("delivery blocked")
                || lower.contains("unsupported adopted cli policies")
            {
                C::InvalidStageTransition
            } else if lower.contains("interrupted external action")
                || lower.contains("incomplete write-journal")
                || lower.contains("unreconciled")
            {
                C::ExternalEditDetected
            } else if lower.contains("depends on unknown task")
                || lower.contains("blocked by incomplete dependency")
                || lower.contains("argv mismatch")
                || lower.contains("conflict")
            {
                C::ContractConflict
            } else if lower.contains("plan approval")
                || lower.contains("valid plan approval")
                || lower.contains("plan_digest")
                || lower.contains("plan design")
                || lower.contains("selected alternative")
                || lower.contains("automated ac")
                || lower.contains("strategy")
                || lower.contains("alternatives comparison")
            {
                C::PlanNotApproved
            } else if lower.contains("not found")
                || lower.contains("does not exist")
                || lower.contains("external action missing")
            {
                C::InvalidInput
            } else if lower.contains("plan") && !lower.contains("spec approval") {
                C::PlanNotApproved
            } else {
                C::SpecNotApproved
            };
            wb_err(code, msg)
        }
        WorkflowBoardRepoError::InvalidInput(msg) => wb_err(C::InvalidInput, msg),
        WorkflowBoardRepoError::ReuseConflict(msg) => wb_err(C::LaunchFailed, msg),
        WorkflowBoardRepoError::InvalidStateTransition(msg) => {
            wb_err(C::InvalidStageTransition, msg)
        }
        other => wb_err(C::InternalError, other.to_string()),
    }
}

pub(crate) fn map_launch_err(err: ThreadLaunchError) -> WorkflowBoardRequestError {
    use WorkflowBoardRequestErrorCode as C;
    match err {
        ThreadLaunchError::MissingProject | ThreadLaunchError::ProjectNotFound => {
            wb_err(C::ProjectNotFound, err.to_string())
        }
        ThreadLaunchError::InvalidModelSelection(_)
        | ThreadLaunchError::InvalidRuntimeMode(_)
        | ThreadLaunchError::InvalidInteractionMode(_)
        | ThreadLaunchError::InvalidWorkspaceStrategy(_) => {
            wb_err(C::InvalidInput, err.to_string())
        }
        other => wb_err(C::LaunchFailed, other.to_string()),
    }
}

pub(crate) fn parse_expected_rev(rev: u64) -> Result<i64, WorkflowBoardRequestError> {
    i64::try_from(rev).map_err(|_| {
        wb_err(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "expectedRevision out of range",
        )
    })
}

pub(crate) fn check_revision(
    card_id: &str,
    actual: i64,
    expected: u64,
) -> Result<(), WorkflowBoardRequestError> {
    let expected_i64 = parse_expected_rev(expected)?;
    if actual != expected_i64 {
        let cur = u64::try_from(actual.max(1)).unwrap_or(1);
        return Err(wb_err(
            WorkflowBoardRequestErrorCode::RevisionConflict,
            format!("Revision conflict on card `{card_id}`: expected {expected}, current {cur}"),
        )
        .with_current_revision(cur));
    }
    Ok(())
}

pub(crate) fn db_lock_err() -> WorkflowBoardRequestError {
    internal("Database lock poisoned")
}

pub(crate) fn internal(msg: impl Into<String>) -> WorkflowBoardRequestError {
    wb_err(WorkflowBoardRequestErrorCode::InternalError, msg)
}

#[cfg(test)]
mod tests {
    use legion_contracts::workflow_board::WorkflowBoardRequestErrorCode as C;
    use legion_persistence::workflow_board_repo::WorkflowBoardRepoError;

    use super::map_repo_err;

    #[test]
    fn map_repo_err_classifies_all_precondition_failed_categories() {
        let cases = [
            (
                "cannot proceed while interrupted external actions remain unreconciled",
                C::ExternalEditDetected,
            ),
            (
                "incomplete write-journal entries must be reconciled before resume",
                C::ExternalEditDetected,
            ),
            (
                "interrupted external action must be reconciled before starting a task",
                C::ExternalEditDetected,
            ),
            (
                "delivery blocked by 1 issue(s), first: missing check",
                C::InvalidStageTransition,
            ),
            ("planned check `chk-1` not found", C::InvalidInput),
            (
                "planned check `chk-1` does not exist on card `c-1`",
                C::InvalidInput,
            ),
            ("task `TSK-99` not found", C::InvalidInput),
            ("task `TSK-99` does not exist on card", C::InvalidInput),
            (
                "acceptance criterion `AC-99` not found in AC evidence mappings",
                C::InvalidInput,
            ),
            (
                "review finding `f-99` not found on card `c-1`",
                C::InvalidInput,
            ),
            (
                "contract violation `v-99` not found on card `c-1`",
                C::InvalidInput,
            ),
            ("external action missing", C::InvalidInput),
            (
                "external action `ext-99` not found on card `c-1`",
                C::InvalidInput,
            ),
            (
                "task `TSK-2` depends on unknown task `TSK-99`",
                C::ContractConflict,
            ),
            (
                "task `TSK-2` is blocked by incomplete dependency `TSK-1` (`todo`)",
                C::ContractConflict,
            ),
            (
                "planned check `chk-1` argv mismatch against approved plan",
                C::ContractConflict,
            ),
            (
                "contract conflict: unresolved file-contract violation exists",
                C::ContractConflict,
            ),
            (
                "plan design (lenses, alternatives, strategy) is missing",
                C::PlanNotApproved,
            ),
            (
                "plan approval missing AC evidence mapping for `AC-01`",
                C::PlanNotApproved,
            ),
            (
                "cannot execute tasks without a valid Plan approval",
                C::PlanNotApproved,
            ),
            (
                "cannot approve plan without non-empty plan_digest",
                C::PlanNotApproved,
            ),
            (
                "custom strategy requires non-empty strategy_rationale",
                C::PlanNotApproved,
            ),
            (
                "cannot approve plan without a valid spec approval",
                C::SpecNotApproved,
            ),
            (
                "cannot approve spec without a non-empty spec_digest",
                C::SpecNotApproved,
            ),
        ];

        for (msg, expected) in cases {
            let err = map_repo_err(WorkflowBoardRepoError::PreconditionFailed(msg.to_owned()));
            assert_eq!(err.code, expected, "mismatch for message: {msg}");
        }
    }
}
