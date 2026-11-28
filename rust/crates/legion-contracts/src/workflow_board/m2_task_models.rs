//! M2 task contracts, planned checks, AC evidence mappings, and Plan document models.

use serde::{Deserialize, Deserializer, Serialize, de};

use super::m2_plan_models::{
    WorkflowBoardPlanAlternative, WorkflowBoardPlanLensEntry, WorkflowBoardPlanningStrategy,
    WorkflowBoardStrategyMetadata, WorkflowBoardTaskGranularity,
};
use super::m2_validators::{
    deserialize_argv_command, deserialize_non_empty_artifact_key_vec, validate_ac_mappings_partial,
    validate_single_task_contract,
};
use super::m3_validators::deserialize_present_artifact_key_vec;
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_present_argv_command,
    deserialize_present_argv_command_vec, deserialize_present_non_git_artifact_key_vec,
    deserialize_present_some, deserialize_present_trimmed_non_empty,
    deserialize_present_trimmed_non_empty_vec, deserialize_trimmed_non_empty,
};

/// Testing discipline assigned to a task contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowBoardTaskTestingMethod {
    /// Write failing tests before implementation.
    TestFirst,
    /// Reproduce defect with regression test first.
    RegressionFirst,
    /// Rely on existing verification checks.
    ExistingChecks,
}

/// Task lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardTaskStatus {
    /// Queued and pending.
    Pending,
    /// Initial imported/created todo status.
    Todo,
    /// Currently executing.
    InProgress,
    /// Completed successfully.
    Completed,
    /// Failed verification or execution.
    Failed,
    /// Blocked on dependency or failure.
    Blocked,
    /// Intentionally skipped.
    Skipped,
}

/// Task contract inside a card's execution DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardTaskContract {
    /// Optional legacy/import alias for `taskId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Unique task identifier within the card (e.g. `TSK-0001`).
    pub task_id: String,
    /// Human-readable task title.
    pub title: String,
    /// Upstream dependency task IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    /// Alias dependency list (`blockedBy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    /// Downstream task IDs (`blocks`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<String>>,
    /// Exclusive relative workspace paths this task may modify.
    pub files_allowed: Vec<String>,
    /// Relative workspace paths forbidden for this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_forbidden: Option<Vec<String>>,
    /// Expected deliverable artifact keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_artifacts: Option<Vec<String>>,
    /// Argv-only verification commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_commands: Option<Vec<Vec<String>>>,
    /// Assigned testing method.
    pub testing_method: WorkflowBoardTaskTestingMethod,
    /// Optional testing limitation note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing_limitations: Option<String>,
    /// Current task status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkflowBoardTaskStatus>,
}

impl<'de> Deserialize<'de> for WorkflowBoardTaskContract {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            id: Option<String>,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            task_id: String,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            title: String,
            #[serde(
                default,
                deserialize_with = "deserialize_present_trimmed_non_empty_vec"
            )]
            depends_on: Option<Vec<String>>,
            #[serde(
                default,
                deserialize_with = "deserialize_present_trimmed_non_empty_vec"
            )]
            blocked_by: Option<Vec<String>>,
            #[serde(
                default,
                deserialize_with = "deserialize_present_trimmed_non_empty_vec"
            )]
            blocks: Option<Vec<String>>,
            #[serde(deserialize_with = "deserialize_non_empty_artifact_key_vec")]
            files_allowed: Vec<String>,
            #[serde(
                default,
                deserialize_with = "deserialize_present_non_git_artifact_key_vec"
            )]
            files_forbidden: Option<Vec<String>>,
            #[serde(default, deserialize_with = "deserialize_present_artifact_key_vec")]
            expected_artifacts: Option<Vec<String>>,
            #[serde(default, deserialize_with = "deserialize_present_argv_command_vec")]
            verification_commands: Option<Vec<Vec<String>>>,
            testing_method: WorkflowBoardTaskTestingMethod,
            #[serde(default)]
            testing_limitations: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_some")]
            status: Option<WorkflowBoardTaskStatus>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let task = Self {
            id: raw.id,
            task_id: raw.task_id,
            title: raw.title,
            depends_on: raw.depends_on,
            blocked_by: raw.blocked_by,
            blocks: raw.blocks,
            files_allowed: raw.files_allowed,
            files_forbidden: raw.files_forbidden,
            expected_artifacts: raw.expected_artifacts,
            verification_commands: raw.verification_commands,
            testing_method: raw.testing_method,
            testing_limitations: raw.testing_limitations,
            status: raw.status,
        };
        validate_single_task_contract(&task).map_err(de::Error::custom)?;
        Ok(task)
    }
}

/// Planned argv-only verification check entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlannedCheck {
    /// Check identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Linked acceptance criterion ID, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub criterion_id: Option<String>,
    /// Argv-only command tokens.
    #[serde(deserialize_with = "deserialize_argv_command")]
    pub argv: Vec<String>,
    /// Optional human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Evidence method mapped to an acceptance criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardEvidenceMethod {
    /// Automated argv check command.
    Automated,
    /// Human manual verification procedure.
    Manual,
    /// External evidence reference.
    External,
}

/// Mapping from an acceptance criterion (`AC-*`) to its verification evidence method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardAcEvidenceMapping {
    /// Target acceptance criterion ID.
    pub criterion_id: String,
    /// Evidence method.
    pub method: WorkflowBoardEvidenceMethod,
    /// Linked planned check IDs (`automated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_ids: Option<Vec<String>>,
    /// Direct argv-only command (`automated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Concrete verification procedure (`manual` / `external`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    /// External evidence reference (`external`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    /// External source alias (`external`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_source: Option<String>,
    /// Optional limitations note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitations: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardAcEvidenceMapping {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            criterion_id: String,
            method: WorkflowBoardEvidenceMethod,
            #[serde(
                default,
                deserialize_with = "deserialize_present_trimmed_non_empty_vec"
            )]
            check_ids: Option<Vec<String>>,
            #[serde(default, deserialize_with = "deserialize_present_argv_command")]
            command: Option<Vec<String>>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            procedure: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            external_ref: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            external_source: Option<String>,
            #[serde(default)]
            limitations: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let mapping = Self {
            criterion_id: raw.criterion_id,
            method: raw.method,
            check_ids: raw.check_ids,
            command: raw.command,
            procedure: raw.procedure,
            external_ref: raw.external_ref,
            external_source: raw.external_source,
            limitations: raw.limitations,
        };
        validate_ac_mappings_partial(std::slice::from_ref(&mapping)).map_err(de::Error::custom)?;
        Ok(mapping)
    }
}

/// Structured M2 planning document persisted on a workflow board card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlanDocument {
    /// Design lens entries.
    pub lenses: Vec<WorkflowBoardPlanLensEntry>,
    /// Compared architectural alternatives.
    pub alternatives: Vec<WorkflowBoardPlanAlternative>,
    /// Number of alternative-comparison revisions used (`0..=1`).
    pub alternatives_revision_count: u64,
    /// Selected alternative ID, if chosen.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub selected_alternative_id: Option<String>,
    /// Selection rationale.
    pub selection_rationale: Option<String>,
    /// Selected planning strategy.
    pub strategy: Option<WorkflowBoardPlanningStrategy>,
    /// Strategy rationale (`custom`).
    pub strategy_rationale: Option<String>,
    /// Strategy metadata.
    pub strategy_metadata: Option<WorkflowBoardStrategyMetadata>,
    /// Task granularity.
    pub granularity: Option<WorkflowBoardTaskGranularity>,
    /// Task contracts.
    pub tasks: Vec<WorkflowBoardTaskContract>,
    /// Planned checks.
    pub checks: Vec<WorkflowBoardPlannedCheck>,
    /// AC evidence mappings.
    pub ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
    /// Plan content SHA-256 digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub plan_digest: Option<String>,
    /// Task contracts SHA-256 digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub tasks_digest: Option<String>,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
}

/// Valid earlier target stages for `workflowBoard.returnForRevision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardReturnTargetStage {
    /// Return to Spec stage.
    Spec,
    /// Return to Plan stage.
    Plan,
    /// Return to Execute stage.
    Execute,
}
