//! Snapshot, signal, reference, store entry, and domain error workflow contracts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::types::{
    WorkflowDefinitionState, WorkflowQuestion, WorkflowRun, WorkflowRunStatus, WorkflowStep,
    WorkflowSummary,
};

/// Snapshot pushed to `workflows.subscribe` watchers (`WorkflowRunsSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsSnapshot {
    /// Registered workflows.
    pub workflows: Vec<WorkflowSummary>,
    /// Recent runs (up to 500 non-skipped + 500 top-level skipped).
    pub runs: Vec<WorkflowRun>,
    /// Open questions waiting on user input.
    pub questions: Vec<WorkflowQuestion>,
    /// Optional definition reload state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkflowDefinitionState>,
}

/// Lightweight badge counters pushed to `workflows.counts` (`WorkflowCounts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCounts {
    /// Open human questions plus paused runs.
    pub needs_you: i64,
    /// Runs that have not yet ended.
    pub active: i64,
    /// Total workflow runs stored in SQLite.
    pub total: i64,
}

/// Origin that first attached a reference to a workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowReferenceOrigin {
    /// Noted automatically when a trigger started the run.
    Trigger,
    /// Noted while executing a step.
    Step,
}

impl WorkflowReferenceOrigin {
    /// Returns the canonical SQL/wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Step => "step",
        }
    }

    /// Parses a SQL/wire origin string.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "trigger" => Some(Self::Trigger),
            "step" => Some(Self::Step),
            _ => None,
        }
    }
}

/// External entity touched by a workflow run (`WorkflowRunReference`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunReference {
    /// Reference kind (e.g. `"issue"` or `"pullRequest"`).
    pub kind: String,
    /// Normalized identity key within `kind`.
    pub key: String,
    /// Kind-specific JSON address.
    pub address: Value,
    /// Display title (empty until known).
    pub title: String,
    /// Optional web URL.
    pub url: Option<String>,
    /// Optional state label (`open`, `merged`, `closed`, ...).
    pub state: Option<String>,
    /// Step names that touched this entity in first-seen order.
    pub steps: Vec<String>,
    /// Whether noted by trigger or step.
    pub origin: WorkflowReferenceOrigin,
}

/// Input payload for noting a run reference (`ReferenceInput`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowReferenceInput {
    /// Reference kind (`"issue"`, `"pullRequest"`, ...).
    pub kind: String,
    /// Normalized identity key within `kind`.
    pub key: String,
    /// Kind-specific JSON address.
    pub address: Value,
    /// Title (empty string preserves existing title).
    pub title: String,
    /// Optional web URL.
    pub url: Option<String>,
    /// Optional state label.
    pub state: Option<String>,
}

/// Full inspection payload for one workflow run (`WorkflowRunDetail`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunDetail {
    /// Summary of the run.
    pub run: WorkflowRun,
    /// Steps in execution order.
    pub steps: Vec<WorkflowStep>,
    /// External entities touched by this run.
    pub references: Vec<WorkflowRunReference>,
}

/// User answer to a pending `ask` question (`WorkflowAnswerInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAnswerInput {
    /// Target run ID.
    pub run_id: String,
    /// Question step name.
    pub name: String,
    /// Selected choice string.
    pub choice: String,
    /// Optional free-text reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Target address for sending a workflow signal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SignalAddress {
    /// Specific wait step inside one run.
    Run {
        #[serde(rename = "runId")]
        run_id: String,
        wait: String,
    },
    /// Channel shared by runs of one workflow.
    Channel { workflow: String, channel: String },
}

/// Target address for broadcasting a signal without buffering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EveryoneAt {
    /// Channel key.
    pub channel: String,
    /// Optional workflow restriction.
    pub workflow: Option<String>,
}

/// Parent link connecting an invoked child run to its parent wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunParent {
    /// Parent run ID.
    pub run_id: String,
    /// Wait step name on the parent run.
    pub wait: String,
}

/// Filter accepted by `runs(...)` inside workflow steps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFilter {
    /// Filter by workflow name.
    pub workflow: Option<String>,
    /// Filter by run status.
    pub status: Option<WorkflowRunStatus>,
    /// Filter by idempotency key prefix.
    pub key_prefix: Option<String>,
}

/// Summary row returned to steps calling `runs(...)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    /// Run ID.
    pub id: String,
    /// Workflow name.
    pub workflow: String,
    /// Idempotency key.
    pub key: String,
    /// Run title.
    pub title: String,
    /// Current status.
    pub status: WorkflowRunStatus,
    /// Status detail.
    pub detail: Option<String>,
    /// Start timestamp.
    pub started_at: String,
    /// End timestamp, if ended.
    pub ended_at: Option<String>,
}

/// Outcome of delivering a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalDelivery {
    /// Immediately woke an open wait.
    Delivered,
    /// Stored in `workflow_mail` for a future wait.
    Buffered,
}

/// Stored key-value entry returned by `WorkflowEngine::stored`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEntry {
    /// Key in `workflow_store`.
    pub key: String,
    /// Parsed JSON value.
    pub value: Value,
}

/// Typed workflow engine and store errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkflowError {
    /// Requested workflow name or version is not registered.
    #[error("{}", format_not_registered(.workflow, *.version))]
    NotRegistered {
        /// Workflow name or `name@version` tag.
        workflow: String,
        /// Optional version number.
        version: Option<i64>,
    },
    /// Requested workflow run ID does not exist.
    #[error("Workflow run {run_id} was not found.")]
    RunNotFound {
        /// Missing run ID.
        run_id: String,
    },
    /// Invalid input, schema rejection, or disabled/unconfigured workflows.
    #[error("{problem}")]
    Input {
        /// Actionable error message.
        problem: String,
    },
    /// Storage or SQLite failure.
    #[error("Workflow storage error: {message}")]
    Storage {
        /// Underlying storage error message.
        message: String,
    },
}

fn format_not_registered(workflow: &str, version: Option<i64>) -> String {
    match version {
        Some(v) => format!("Version {v} of workflow {workflow} is not registered."),
        None => format!("No workflow named {workflow} is registered."),
    }
}

/// Kind of entry stored in `workflow_engine_journal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEntryKind {
    /// Durable activity (`step/*`, `agent/*/start`, `agent/*/result/*`, `title/*`, etc.).
    Activity,
    /// Durable clock sleep (`sleep/<name>`).
    Sleep,
    /// Durable deferred wait (`wait/<name>`).
    WaitDeferred,
}

impl JournalEntryKind {
    /// Returns the SQL string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::Sleep => "sleep",
            Self::WaitDeferred => "wait_deferred",
        }
    }

    /// Parses a SQL string representation.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "activity" => Some(Self::Activity),
            "sleep" => Some(Self::Sleep),
            "wait_deferred" => Some(Self::WaitDeferred),
            _ => None,
        }
    }
}

/// Execution state of a journal entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEntryStatus {
    /// Finished and cached for replay.
    Completed,
    /// Suspended on step failure until resumed.
    Failed,
    /// Waiting on timer or external signal.
    Waiting,
}

impl JournalEntryStatus {
    /// Returns the SQL string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Waiting => "waiting",
        }
    }

    /// Parses a SQL string representation.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "waiting" => Some(Self::Waiting),
            _ => None,
        }
    }
}
