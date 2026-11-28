//! Trait abstractions for agent execution, issue/PR forges, and milestone publication.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Boxed `Send` future alias used by async driver traits without extra proc-macro deps.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Error returned by an agent thread driver (`WorkflowAgentError`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{reason}")]
pub struct WorkflowAgentError {
    /// Human-readable error message.
    pub reason: String,
}

impl WorkflowAgentError {
    /// Creates an agent error from any displayable message.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Error returned by an issue tracker or pull-request forge driver (`WorkflowForgeError`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{reason}")]
pub struct WorkflowForgeError {
    /// Human-readable error message.
    pub reason: String,
}

impl WorkflowForgeError {
    /// Creates a forge error from any displayable message.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Model selection attached to a workflow agent step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModel {
    /// Provider instance ID.
    pub instance_id: String,
    /// Model identifier.
    pub model: String,
    /// Optional provider-specific model options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

/// Launch mode overrides for a new workflow agent thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLaunch {
    /// Workspace strategy (`{ "type": "root" }` or `{ "type": "worktree" }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Value>,
    /// Runtime mode (`approval-required`, `full-access`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<String>,
    /// Interaction mode (`default`, `plan`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_mode: Option<String>,
    /// Optional stable launch identity shared across retries and crash recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_launch_key: Option<String>,
}

/// Reference to an issue passed into an agent step or forge API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IssueAddress {
    /// Web URL scoped to a project ID or workspace path.
    Url { project: String, url: String },
    /// Structured issue reference from `issues.list` or trigger payload.
    Ref {
        #[serde(rename = "projectId")]
        project_id: String,
        tracker: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        scope: String,
        key: String,
    },
}

/// Reference to a pull request passed into a forge API or wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PullRequestAddress {
    /// Web URL scoped to a project ID or workspace path.
    Url { project: String, url: String },
    /// Structured PR reference from `pullRequests.list` or trigger payload.
    Ref {
        #[serde(rename = "projectId")]
        project_id: String,
        host: String,
        repository: String,
        number: i64,
    },
}

/// Origin metadata linking an agent thread back to its workflow step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOrigin {
    /// Workflow run ID.
    pub run_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Step name.
    pub step: String,
}

/// Input passed to [`WorkflowAgentDriver::start`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStartInput {
    /// Idempotency key (`<runId>/<step>/<subkey>`).
    pub key: String,
    /// Workflow lineage attached to newly launched threads.
    pub origin: AgentOrigin,
    /// Thread title.
    pub title: String,
    /// Turn prompt text.
    pub prompt: String,
    /// Optional structured JSON Schema expected via `legion_submit_output` / `t3_submit_output`.
    pub output_schema: Option<serde_json::Map<String, Value>>,
    /// Optional explicit model override.
    pub model: Option<AgentModel>,
    /// Project ID or absolute directory path when starting a new thread.
    pub project: Option<String>,
    /// Linked environment ID when running remotely.
    pub environment: Option<String>,
    /// Existing thread ID when continuing a thread.
    pub thread: Option<String>,
    /// Optional issue linked to the agent thread.
    pub issue: Option<IssueAddress>,
    /// Launch options for new threads.
    pub launch: Option<AgentLaunch>,
    /// Optional stable launch identity overriding `launch.stable_launch_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_launch_key: Option<String>,
}

/// Handle to a started or attempted agent turn (`AgentTurn`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    /// Thread ID (empty string when `failed` is set).
    pub thread_id: String,
    /// Turn/run ID (empty string when `failed` is set).
    pub turn_id: String,
    /// Linked environment ID, if remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Epoch milliseconds when the step's first turn started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Start failure message when `agent.try` captures start failures as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// Terminal outcome of an agent turn (`AgentResult.ending`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentEnding {
    /// Turn finished normally.
    Completed,
    /// Turn failed underneath the agent.
    Failed,
    /// Turn was interrupted or stopped intentionally.
    Stopped,
}

/// Result collected when an agent turn completes (`AgentResult`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResult {
    /// How the turn ended.
    pub ending: AgentEnding,
    /// Human-readable status detail.
    pub detail: String,
    /// Closing assistant message text.
    pub reply: String,
    /// Structured JSON submitted against `output_schema`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

/// Target for turn-scoped interruption on cancel or timeout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterruptTarget {
    /// Thread ID.
    pub thread_id: String,
    /// Linked environment ID, if remote.
    pub environment: Option<String>,
    /// Specific turn ID so concurrent user turns on the same thread are left alone.
    pub turn_id: Option<String>,
}

/// Result of interrupting an agent turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentInterruptOutcome {
    /// Interrupt request was dispatched to an active turn.
    Stopped,
    /// Turn had already ended before interrupt arrived.
    Ended,
}

/// Trait abstracting thread launch, turn completion wait (`untilOutput` + `quiet`), and cancel.
pub trait WorkflowAgentDriver: Send + Sync {
    /// Starts or continues a turn on a local or linked-environment thread.
    fn start(&self, input: AgentStartInput)
    -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>>;

    /// Waits for `turn` to finish (and if `quiet_ms` is set with an `outputSchema`, waits for
    /// a submitted output across woken turns until the thread stays quiet for `quiet_ms`).
    fn result(
        &self,
        turn: AgentTurn,
        quiet_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<AgentResult, WorkflowAgentError>>;

    /// Interrupts `target`'s turn without disturbing unrelated turns on the same thread.
    fn interrupt(
        &self,
        target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>>;
}

/// Default agent driver used when no runtime thread driver is attached.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableAgentDriver;

impl WorkflowAgentDriver for UnavailableAgentDriver {
    fn start(
        &self,
        _input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async {
            Err(WorkflowAgentError::new(
                "Agent threads are not available in this workflow runtime.",
            ))
        })
    }

    fn result(
        &self,
        _turn: AgentTurn,
        _quiet_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<AgentResult, WorkflowAgentError>> {
        Box::pin(async {
            Err(WorkflowAgentError::new(
                "Agent threads are not available in this workflow runtime.",
            ))
        })
    }

    fn interrupt(
        &self,
        _target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async { Ok(AgentInterruptOutcome::Ended) })
    }
}

pub use crate::traits_forge::*;

/// Workflow milestone transition emitted by [`WorkflowStore`](crate::store::WorkflowStore).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMilestoneEvent {
    /// Stable deduplication identifier (`<type>:<key>`).
    pub id: String,
    /// Event kind (`workflow.run_started`, `workflow.needs_input`, etc.).
    #[serde(rename = "type")]
    pub event_type: String,
    /// ISO-8601 occurrence timestamp.
    pub occurred_at: String,
    /// True when invoked as a child workflow run.
    pub is_subagent_child: bool,
    /// Workflow run ID.
    pub workflow_run_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Run title.
    pub run_title: String,
    /// Parent run ID if invoked.
    pub parent_workflow_run_id: Option<String>,
    /// Question step name for `needs_input` / `input_resolved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// Question title for `needs_input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// Resolution for `input_resolved` (`answered`, `expired`, `cancelled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// Pause reason for `run_paused`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// End outcome for `run_ended`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Optional detail for `run_ended`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Receiver for workflow milestone transitions (`MilestoneHub`).
pub trait WorkflowMilestoneSink: Send + Sync {
    /// Publishes one workflow milestone transition.
    fn publish(&self, event: WorkflowMilestoneEvent);
}

/// No-op milestone sink used when milestone broadcasting is disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMilestoneSink;

impl WorkflowMilestoneSink for NoopMilestoneSink {
    fn publish(&self, _event: WorkflowMilestoneEvent) {}
}
