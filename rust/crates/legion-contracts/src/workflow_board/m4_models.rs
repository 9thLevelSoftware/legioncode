//! M4 `.legion-cli` import models, lock entries, unsupported policy descriptors, and provenance.

use serde::{Deserialize, Deserializer, Serialize, de};

use super::m2_task_models::WorkflowBoardTaskStatus;
use super::m2_validators::is_valid_argv_command;
use super::validators::{
    deserialize_artifact_key, deserialize_artifact_key_vec, deserialize_opt_positive_revision,
    deserialize_opt_trimmed_non_empty, deserialize_trimmed_non_empty,
    deserialize_trimmed_non_empty_vec,
};

fn deserialize_argv_command_vec<'de, D>(deserializer: D) -> Result<Vec<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let cmds = Vec::<Vec<String>>::deserialize(deserializer)?;
    cmds.into_iter()
        .map(|argv| {
            let trimmed: Vec<String> = argv.into_iter().map(|s| s.trim().to_owned()).collect();
            if is_valid_argv_command(&trimmed) {
                Ok(trimmed)
            } else {
                Err(de::Error::custom(
                    "expected a valid argv-only command without shell wrappers or control operators",
                ))
            }
        })
        .collect()
}

/// Lock source kind detected under `.legion-cli/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportLockKind {
    /// `.legion-cli/index/engine.lock` (+ `.steal`).
    EngineLock,
    /// `.legion-cli/cache/live-spawn/*.json` or `live-spawn.json`.
    LiveSpawn,
    /// `.legion-cli/cache/runs/<runId>/resume.json`.
    ResumeJson,
}

/// Lock liveness classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportLockStatus {
    /// Active CLI run or uncertain termination — refuses takeover.
    Active,
    /// Dead-PID stale lock — surfaced as informational warning only.
    Stale,
}

/// Detected CLI lock entry during `.legion-cli/` scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportLockEntry {
    /// Lock file kind.
    pub kind: WorkflowBoardCliImportLockKind,
    /// Relative workspace path to the lock file.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub relative_path: String,
    /// Active or stale status.
    pub status: WorkflowBoardCliImportLockStatus,
    /// OS process ID if recorded.
    #[serde(deserialize_with = "deserialize_opt_positive_revision")]
    pub pid: Option<u64>,
    /// Run identifier if recorded.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub run_id: Option<String>,
    /// Start/acquired timestamp if recorded.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub started_at: Option<String>,
    /// Human-readable explanation.
    pub message: String,
}

/// Policy acknowledgement state for an imported unsupported CLI policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportPolicyStatus {
    /// Not yet acknowledged by operator; blocks Execute.
    Unsupported,
    /// Explicitly acknowledged by operator.
    Acknowledged,
}

/// Adopted CLI policy not natively enforced by `LegionCode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportUnsupportedPolicy {
    /// Policy key (e.g. `sandbox.requireHardened`, `qa.passScore`, `skills.trustKeys`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub key: String,
    /// Policy category (`sandbox`, `qa`, `skills`, `mcp`, `dashboard`, `assurance`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub category: String,
    /// Human-readable description of the policy and native difference.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub summary: String,
    /// Configured value from `.legion-cli` serialized concisely.
    pub configured_value: Option<String>,
    /// Acknowledgement status.
    pub status: WorkflowBoardCliImportPolicyStatus,
    /// Whether this policy blocks Execute until acknowledged.
    pub blocks_execution: bool,
    /// Relative path where the policy was declared.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub source_path: String,
}

/// Kind of historical CLI evidence receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportHistoricalReceiptKind {
    /// `workflow/spec-approval.yaml`.
    SpecApproval,
    /// `workflow/plan-approval.yaml`.
    PlanApproval,
    /// `workflow/execution.yaml`.
    Execution,
    /// `plans/*/review.md` or execution review summary.
    Review,
    /// Imported verification check / failure outcome.
    Check,
}

/// Fixed `"historical"` status tag for CLI receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportHistoricalStatus {
    /// Imported historical evidence awaiting native revalidation.
    Historical,
}

/// Historical CLI approval/execution/check receipt surfaced on preview and card detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportHistoricalReceipt {
    /// Deterministic receipt identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Receipt classification.
    pub kind: WorkflowBoardCliImportHistoricalReceiptKind,
    /// Always `"historical"`.
    pub status: WorkflowBoardCliImportHistoricalStatus,
    /// Relative source file path under `.legion-cli/`.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub source_path: String,
    /// Timestamp recorded in the CLI file, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub recorded_at: Option<String>,
    /// Principal/actor recorded in the CLI file, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub recorded_by: Option<String>,
    /// Human-readable summary of the historical receipt.
    pub summary: String,
    /// Always `true` until natively revalidated.
    pub requires_native_revalidation: bool,
}

/// Explicit note for a gitignored `.legion-cli/cache/**` log file that is absent on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportMissingLogNote {
    /// Related task ID (`TSK-*`), if known.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub task_id: Option<String>,
    /// Related check ID, if known.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub check_id: Option<String>,
    /// Relative path to the missing log file.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub relative_path: String,
    /// Failed or referenced command string, if recorded.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub command: Option<String>,
    /// Explanatory note surfaced to the user.
    pub note: String,
}

/// Preview summary for an imported CLI task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportTaskPreview {
    /// Preserved task ID alias (`TSK-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Preserved task ID (`TSK-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub task_id: String,
    /// Task title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Preserved task status (`todo`, `blocked`, `completed`, etc.).
    pub status: WorkflowBoardTaskStatus,
    /// Explicit blocker summary if blocked/failed.
    pub blocker_summary: Option<String>,
    /// Argv-only verification commands from task contract.
    #[serde(deserialize_with = "deserialize_argv_command_vec")]
    pub verification_commands: Vec<Vec<String>>,
    /// Missing cache log notes associated with this task.
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
}

/// Fixed `"legion-cli"` source system marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkflowBoardCliImportSourceSystem {
    /// Imported from `.legion-cli/`.
    #[serde(rename = "legion-cli")]
    LegionCli,
}

/// Fixed `"legioncode"` subsequent-mutation owner marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardCliImportOwner {
    /// `LegionCode` owns all subsequent board mutations.
    Legioncode,
}

/// Provenance, policy, and historical receipt metadata attached to `WorkflowCardDetail.cliImport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCliImportProvenance {
    /// Source system (`"legion-cli"`).
    pub source_system: WorkflowBoardCliImportSourceSystem,
    /// Source CLI directory relative path (e.g. `".legion-cli"`).
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub source_cli_dir: String,
    /// Preserved CLI `specId` (e.g. `"spec-t3-code"`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub source_spec_id: String,
    /// Ownership marker (`"legioncode"`).
    pub owner: WorkflowBoardCliImportOwner,
    /// Initial import ISO-8601 timestamp.
    pub imported_at: String,
    /// Most recent sync ISO-8601 timestamp.
    pub last_synced_at: String,
    /// Canonical SHA-256 digest of scanned `.legion-cli` inputs.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub source_digest: Option<String>,
    /// Preserved CLI decision IDs (`D-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub decision_ids: Vec<String>,
    /// Preserved CLI assumption IDs (`ASM-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub assumption_ids: Vec<String>,
    /// Preserved acceptance criterion IDs (`AC-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub acceptance_criterion_ids: Vec<String>,
    /// Historical receipts imported from `.legion-cli`.
    pub historical_receipts: Vec<WorkflowBoardCliImportHistoricalReceipt>,
    /// Unsupported adopted policies and their acknowledgement statuses.
    pub unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    /// Policy keys explicitly acknowledged by the operator.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub acknowledged_policies: Vec<String>,
    /// Notes for missing gitignored cache logs.
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
    /// Relative paths of `.legion-cli` artifacts scanned into provenance.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub preserved_artifact_paths: Vec<String>,
    /// Optional summary of `workflow/assurance*.yaml` sidecars, if present.
    pub assurance_policy_summary: Option<String>,
}
