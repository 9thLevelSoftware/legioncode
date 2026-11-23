//! Wire-compatible workflow contracts, addresses, filters, and domain errors.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle state of a durable workflow run (`packages/contracts/src/workflow.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowRunStatus {
    /// Actively executing or replaying steps.
    Running,
    /// Sleeping on a wait, question, agent turn, sleep timer, or pull-request watch.
    Waiting,
    /// Suspended because a step or agent failed; resumable automatically or by a person.
    Paused,
    /// Finished successfully with an optional JSON result.
    Completed,
    /// Ended intentionally by the workflow via `stop(reason)`.
    Stopped,
    /// Ended by the workflow via `skip(reason)` because the trigger event was not applicable.
    Skipped,
    /// Cancelled externally by a user, parent run, or step.
    Cancelled,
}

impl WorkflowRunStatus {
    /// Returns the canonical lowercase SQL/wire string for this status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses a SQL/wire status string.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "running" => Some(Self::Running),
            "waiting" => Some(Self::Waiting),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "stopped" => Some(Self::Stopped),
            "skipped" => Some(Self::Skipped),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Returns `true` when the run has reached a terminal state.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Stopped | Self::Skipped | Self::Cancelled
        )
    }
}

/// Wire projection of a workflow run (`WorkflowRun` in `workflow.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// Unique execution identifier.
    pub run_id: String,
    /// Registered workflow name.
    pub workflow: String,
    /// Workflow definition version this run started against.
    pub version: i64,
    /// Human-readable run title.
    pub title: String,
    /// Current lifecycle status.
    pub status: WorkflowRunStatus,
    /// Pause reason, wait target, or terminal reason.
    pub detail: Option<String>,
    /// Terminal JSON result when completed.
    pub result: Option<Value>,
    /// Consecutive step-failure pauses since the last completed step.
    pub pauses: i64,
    /// True when the run has not ended and its version is no longer registered.
    pub retired: bool,
    /// ISO-8601 timestamp when automatic resume is scheduled, or `null`.
    pub resumes_at: Option<String>,
    /// Invoking parent workflow run ID, if any.
    pub parent_run_id: Option<String>,
    /// Step/wait name in the parent run that invoked this run.
    pub parent_step: Option<String>,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// ISO-8601 last update timestamp.
    pub updated_at: String,
    /// ISO-8601 completion timestamp, or `null`.
    pub ended_at: Option<String>,
}

/// Kind of recorded step within a workflow run (`WorkflowStep.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepKind {
    /// Pure or side-effecting `step` / `onEnd`.
    Step,
    /// External signal or invoked-child `wait`.
    Wait,
    /// Agent thread execution `agent`.
    Agent,
    /// Human-in-the-loop question `ask`.
    Ask,
    /// Durable timer `sleep`.
    Sleep,
}

impl WorkflowStepKind {
    /// Returns the canonical SQL/wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Wait => "wait",
            Self::Agent => "agent",
            Self::Ask => "ask",
            Self::Sleep => "sleep",
        }
    }

    /// Parses a SQL/wire step kind string.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "step" => Some(Self::Step),
            "wait" => Some(Self::Wait),
            "agent" => Some(Self::Agent),
            "ask" => Some(Self::Ask),
            "sleep" => Some(Self::Sleep),
            _ => None,
        }
    }
}

/// Execution status of a workflow step (`WorkflowStep.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepStatus {
    /// Currently active or waiting.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed or cancelled.
    Failed,
}

impl WorkflowStepStatus {
    /// Returns the canonical SQL/wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parses a SQL/wire step status string.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Wire projection of one workflow step (`WorkflowStep` in `workflow.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    /// Unique step name within the run.
    pub name: String,
    /// Step primitive kind.
    pub kind: WorkflowStepKind,
    /// Step status.
    pub status: WorkflowStepStatus,
    /// Total execution attempts.
    pub attempts: i64,
    /// Optional status detail, wake ISO timestamp, or failure reason.
    pub detail: Option<String>,
    /// Thread ID associated with an agent step.
    pub thread_id: Option<String>,
    /// Linked environment ID hosting the thread, or `null` for local.
    pub environment_id: Option<String>,
    /// Stored step output JSON (truncated for display if large).
    pub result: Option<Value>,
    /// Closing text produced by an agent when returning structured output.
    pub said: Option<String>,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// ISO-8601 completion timestamp, or `null`.
    pub ended_at: Option<String>,
}

/// Latest agent thread attached to a pending human question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowQuestionThread {
    /// Thread identifier.
    pub thread_id: String,
    /// Linked environment identifier, or `null` when local.
    pub environment_id: Option<String>,
}

/// Open question waiting on human input (`WorkflowQuestion` in `workflow.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowQuestion {
    /// Owning run ID.
    pub run_id: String,
    /// Wait/ask step name.
    pub name: String,
    /// Question prompt payload (`title`, `choices`, optional `data`).
    pub request: Value,
    /// ISO-8601 timestamp when asked.
    pub asked_at: String,
    /// Latest agent thread of this run, if any.
    pub thread: Option<WorkflowQuestionThread>,
}

/// Registered webhook route path for a workflow trigger (`WorkflowHook`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHook {
    /// Registered workflow name.
    pub workflow: String,
    /// Hook trigger name.
    pub name: String,
    /// Relative HTTP path including the secret hook token.
    pub path: String,
}

/// Summary of the latest registered version of a workflow (`WorkflowSummary`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    /// Workflow name.
    pub name: String,
    /// Latest registered version.
    pub version: i64,
    /// Human-readable trigger descriptions.
    pub triggers: Vec<String>,
}

/// Diagnostic explaining why a workflow definition file or directory change did not load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionProblem {
    /// Human-readable error explanation.
    pub message: String,
    /// Originating definition file name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line number inside `file`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
}

/// Current definition reload state (`WorkflowDefinitionState`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionState {
    /// Monotonic counter incremented on each reload or problem update.
    pub revision: u64,
    /// Aggregated problem string when any file/directory error exists.
    pub problem: Option<String>,
}

/// Single workflow definition file snapshot (`WorkflowDefinitionFile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionFile {
    /// File basename inside the workflows folder.
    pub name: String,
    /// Raw file source currently on disk.
    pub source: String,
    /// Last-known-good `name@version` tags active from this file.
    pub workflows: Vec<String>,
    /// Active parse/load/conflict problems on this file.
    pub problems: Vec<WorkflowDefinitionProblem>,
}

/// Full directory view of workflow definitions (`WorkflowDefinitionFiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionFiles {
    /// Directory containing workflow definition files.
    pub directory: String,
    /// True when overridden to a single definition file path.
    pub single: bool,
    /// Per-file details sorted by file name.
    pub files: Vec<WorkflowDefinitionFile>,
    /// Cross-file or folder-level problems.
    pub problems: Vec<WorkflowDefinitionProblem>,
    /// Current revision counter.
    pub revision: u64,
    /// Aggregated problem summary, or `null`.
    pub problem: Option<String>,
}

/// Result of saving or removing a workflow definition file (`WorkflowDefinitionChangeResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionChangeResult {
    /// True when disk state changed.
    pub changed: bool,
    /// Validation or runtime reload problems.
    pub problems: Vec<WorkflowDefinitionProblem>,
}

pub use crate::types_extra::*;
