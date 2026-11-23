//! Multi-step durable workflow status, definitions, questions, and RPC payloads (`workflow.ts`).

mod definitions;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::issue::{IssueRef, IssueRefError};

pub use definitions::{
    WorkflowDefinitionChangeResult, WorkflowDefinitionDeleteInput, WorkflowDefinitionFile,
    WorkflowDefinitionFiles, WorkflowDefinitionGetInput, WorkflowDefinitionProblem,
    WorkflowDefinitionSaveInput, WorkflowDefinitionState,
};

/// Known `WorkflowRunReference.kind` constant for issues (`WorkflowReferenceKind.issue`).
pub const WORKFLOW_REFERENCE_KIND_ISSUE: &str = "issue";
/// Known `WorkflowRunReference.kind` constant for pull requests (`WorkflowReferenceKind.pullRequest`).
pub const WORKFLOW_REFERENCE_KIND_PULL_REQUEST: &str = "pullRequest";

/// Typed workflow address aliases (`WorkflowIssueAddress` / `WorkflowPullRequestAddress`).
pub type WorkflowIssueAddress = IssueRef;

/// Typed `WorkflowPullRequestAddress` mirror (`PullRequestRef` in `pullRequest.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPullRequestAddress {
    /// Normalized forge host name.
    pub host: String,
    /// Canonical `owner/repo`.
    pub repository: String,
    /// Positive PR number.
    pub number: u32,
}

/// Unused compile-time sanity check re-exporting `IssueRefError` next to workflow addresses.
pub type WorkflowAddressValidationError = IssueRefError;

/// Execution lifecycle state of a [`WorkflowRun`] (`WorkflowRunStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowRunStatus {
    /// Actively advancing.
    Running,
    /// Waiting on a sleep, event, or agent turn.
    Waiting,
    /// Paused after a failing step or waiting on human action.
    Paused,
    /// Finished successfully.
    Completed,
    /// Stopped before finishing.
    Stopped,
    /// Ended intentionally because the trigger did not apply.
    Skipped,
    /// Cancelled explicitly.
    Cancelled,
}

/// Summary and live state of a durable workflow execution (`WorkflowRun`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// Unique workflow run ID.
    pub run_id: String,
    /// Registered workflow name.
    pub workflow: String,
    /// Workflow definition version number.
    pub version: i64,
    /// Human-readable run title.
    pub title: String,
    /// Current run lifecycle status.
    pub status: WorkflowRunStatus,
    /// Pause reason, wait subject, or terminal detail.
    pub detail: Option<String>,
    /// Final workflow result JSON, or `None`.
    pub result: Option<Value>,
    /// Consecutive step-failure pause count.
    pub pauses: i64,
    /// True if `version` is no longer loaded on disk.
    pub retired: bool,
    /// Automatic resume timestamp when paused with backoff, or `None`.
    pub resumes_at: Option<String>,
    /// Invoking parent workflow run ID, or `None`.
    pub parent_run_id: Option<String>,
    /// Parent step name that invoked this run, or `None`.
    pub parent_step: Option<String>,
    /// ISO-8601 start time.
    pub started_at: String,
    /// ISO-8601 last update time.
    pub updated_at: String,
    /// ISO-8601 completion time, or `None`.
    pub ended_at: Option<String>,
}

/// Kind of recorded step inside a [`WorkflowRun`] (`WorkflowStep.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepKind {
    /// Synchronous or async script step.
    Step,
    /// External wait condition or child workflow wait.
    Wait,
    /// Agent thread step.
    Agent,
    /// Human decision prompt (`ask`).
    Ask,
    /// Timed sleep step.
    Sleep,
}

/// Status of an individual [`WorkflowStep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepStatus {
    /// Step currently active.
    Running,
    /// Step completed cleanly.
    Completed,
    /// Step failed.
    Failed,
}

/// One execution step within a workflow run (`WorkflowStep`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    /// Step name.
    pub name: String,
    /// Step execution kind.
    pub kind: WorkflowStepKind,
    /// Step execution status.
    pub status: WorkflowStepStatus,
    /// Attempt count.
    pub attempts: i64,
    /// Optional detail or sleep wakeup timestamp.
    pub detail: Option<String>,
    /// Thread ID for agent steps, or `None`.
    pub thread_id: Option<String>,
    /// Linked environment ID hosting `thread_id`, or `None` when local.
    pub environment_id: Option<String>,
    /// Step return value JSON, or `None`.
    pub result: Option<Value>,
    /// Closing text reply from an agent step returning structured data.
    pub said: Option<String>,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// ISO-8601 end timestamp, or `None`.
    pub ended_at: Option<String>,
}

/// Optional linked thread attached to an open [`WorkflowQuestion`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowQuestionThread {
    /// Thread identifier.
    pub thread_id: String,
    /// Linked environment identifier when remote, or `None`.
    pub environment_id: Option<String>,
}

/// Open human-in-the-loop question raised by a workflow run (`WorkflowQuestion`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowQuestion {
    /// Run ID awaiting an answer.
    pub run_id: String,
    /// Step/question signal name.
    pub name: String,
    /// Prompt payload (`title`, choices, context JSON).
    pub request: Value,
    /// ISO-8601 ask timestamp.
    pub asked_at: String,
    /// Run's latest agent thread for interactive follow-up, if any.
    pub thread: Option<WorkflowQuestionThread>,
}

/// Webhook endpoint registered for a workflow (`WorkflowHook`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHook {
    /// Registered workflow name.
    pub workflow: String,
    /// Hook name.
    pub name: String,
    /// Secret-carrying webhook route path.
    pub path: String,
}

/// Summary of one registered workflow (`WorkflowSummary`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    /// Workflow identifier.
    pub name: String,
    /// Latest registered version.
    pub version: i64,
    /// Human-readable trigger descriptions.
    pub triggers: Vec<String>,
}

/// Initial/updated stream payload for `workflows.subscribe` (`WorkflowRunsSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsSnapshot {
    /// Registered workflow summaries.
    pub workflows: Vec<WorkflowSummary>,
    /// Recent workflow runs, newest first.
    pub runs: Vec<WorkflowRun>,
    /// Open workflow questions.
    pub questions: Vec<WorkflowQuestion>,
    /// Optional definition loader state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkflowDefinitionState>,
}

/// Lightweight badge counters returned by `workflows.counts` (`WorkflowCounts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCounts {
    /// Open questions and paused runs awaiting a person.
    pub needs_you: i64,
    /// Non-terminal active runs.
    pub active: i64,
    /// Total runs ever recorded.
    pub total: i64,
}

/// Empty subscription payload for `workflows.subscribe` (`WorkflowRunsSubscribeInput`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunsSubscribeInput {}

/// Input payload for `workflows.get` (`WorkflowRunGetInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunGetInput {
    /// Workflow run identifier.
    pub run_id: String,
}

/// How an external reference entered a workflow run (`WorkflowRunReference.origin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowReferenceOrigin {
    /// Supplied by the triggering webhook/event.
    Trigger,
    /// Recorded while executing a step.
    Step,
}

/// External entity reference touched by a workflow run (`WorkflowRunReference`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunReference {
    /// Open entity kind (`"issue"`, `"pullRequest"`, etc.).
    pub kind: String,
    /// Stable identity key within `kind`.
    pub key: String,
    /// Kind-specific address JSON (`IssueRef` or `PullRequestRef`).
    pub address: Value,
    /// Known display title (may be empty before first resolution).
    pub title: String,
    /// Web URL, or `None`.
    pub url: Option<String>,
    /// Normalized entity state (`open`, `merged`, `closed`, etc.), or `None`.
    pub state: Option<String>,
    /// Ordered step names that first touched this reference.
    pub steps: Vec<String>,
    /// Whether the trigger or a step introduced this reference.
    pub origin: WorkflowReferenceOrigin,
}

/// Full detail view for `workflows.get` (`WorkflowRunDetail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunDetail {
    /// Run metadata and state.
    pub run: WorkflowRun,
    /// Ordered steps in start order.
    pub steps: Vec<WorkflowStep>,
    /// External references touched by this run.
    pub references: Vec<WorkflowRunReference>,
}

/// Input payload for `workflows.answer` (`WorkflowAnswerInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAnswerInput {
    /// Workflow run identifier.
    pub run_id: String,
    /// Step/question signal name.
    pub name: String,
    /// Chosen option value.
    pub choice: String,
    /// Optional user rationale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Input payload for `workflows.resume` (`WorkflowRunResumeInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunResumeInput {
    /// Paused workflow run identifier.
    pub run_id: String,
}

/// Input payload for `workflows.cancel` (`WorkflowRunCancelInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunCancelInput {
    /// Workflow run identifier.
    pub run_id: String,
    /// Optional cancellation reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Tagged error returned by workflow RPC endpoints (`WorkflowRequestError`).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct WorkflowRequestError {
    /// User-facing error message.
    pub message: String,
}
