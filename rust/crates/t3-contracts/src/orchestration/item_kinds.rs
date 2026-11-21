//! Variant payloads for the 25 known [`TurnItemKind`] kinds (`orchestrationV2.ts:1750-1979`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    CheckpointId, CheckpointScopeId, ContextHandoffId, EnvironmentId, MessageId, NodeId, PlanId,
    ProviderDriverKind, ProviderInstanceId, ProviderThreadId, RunId, RuntimeRequestId,
    ScheduledTaskId, ThreadId,
};

use super::command_payloads_extra::ProviderApprovalDecision;
use super::common::{CreationSource, McpClientAccess, OrchestrationActor};
use super::items::{CheckpointFileSummary, CheckpointFileTotals, PlanStep};
use super::model_selection::{
    ModelSelection, OrchestrationNotification, ProviderFailure, ProviderRetry,
};

/// All 25 known `TurnItem` variants (`orchestrationV2.ts:1750-1979`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnItemKind {
    /// Background wake notification (`notification`).
    Notification {
        #[serde(flatten)]
        notification: OrchestrationNotification,
    },
    /// User prompt message (`user_message`).
    #[serde(rename_all = "camelCase")]
    UserMessage {
        created_by: OrchestrationActor,
        creation_source: CreationSource,
        message_id: MessageId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheduled_task_id: Option<ScheduledTaskId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_thread_id: Option<ThreadId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_sender: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idle_compaction: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_script: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fresh_context: Option<bool>,
        input_intent: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_omitted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Value>,
        attachments: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_switch: Option<Value>,
    },
    /// Assistant response message (`assistant_message`).
    #[serde(rename_all = "camelCase")]
    AssistantMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        message_id: MessageId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_omitted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Value>>,
        streaming: bool,
    },
    /// Model reasoning trace (`reasoning`).
    Reasoning { text: String, streaming: bool },
    /// Proposed markdown plan (`proposed_plan`).
    #[serde(rename_all = "camelCase")]
    ProposedPlan {
        plan_id: PlanId,
        markdown: String,
        streaming: bool,
    },
    /// Structured task checklist (`todo_list`).
    #[serde(rename_all = "camelCase")]
    TodoList {
        plan_id: PlanId,
        steps: Vec<PlanStep>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explanation: Option<String>,
    },
    /// Structured question prompt (`user_input_request`).
    #[serde(rename_all = "camelCase")]
    UserInputRequest {
        request_id: RuntimeRequestId,
        questions: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question_answer: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_mode: Option<String>,
    },
    /// File modification summary (`file_change`).
    #[serde(rename_all = "camelCase")]
    FileChange {
        file_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additions: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deletions: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_str: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_str: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_str: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<Vec<Value>>,
    },
    /// Shell command execution (`command_execution`).
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        input: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_omitted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_indicates_failure: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completion_wake: Option<String>,
    },
    /// Workspace file search (`file_search`).
    FileSearch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        results: Option<Vec<Value>>,
    },
    /// Web search call (`web_search`).
    WebSearch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        patterns: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        results: Option<Vec<Value>>,
    },
    /// Interactive approval prompt (`approval_request`).
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        request_id: RuntimeRequestId,
        request_kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full_access_override: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<Value>>,
    },
    /// Captured git checkpoint (`checkpoint`).
    #[serde(rename_all = "camelCase")]
    Checkpoint {
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
        files: Vec<CheckpointFileSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_total: Option<CheckpointFileTotals>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_omitted: Option<bool>,
    },
    /// In-flight interrupt notice (`run_interrupt_request`).
    #[serde(rename_all = "camelCase")]
    RunInterruptRequest {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unresponsive_at: Option<String>,
    },
    /// Completed interrupt notice (`run_interrupt_result`).
    RunInterruptResult { message: String },
    /// Informational/warning/error system notice (`system_notice`).
    SystemNotice {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level: Option<String>,
    },
    /// Provider or runtime error (`error`).
    Error {
        failure: ProviderFailure,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<ProviderRetry>,
    },
    /// Context compaction summary (`compaction`).
    #[serde(rename_all = "camelCase")]
    Compaction {
        driver: Option<ProviderDriverKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_token_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_token_count: Option<u64>,
    },
    /// Cross-provider handoff (`handoff`).
    #[serde(rename_all = "camelCase")]
    Handoff {
        context_handoff_id: ContextHandoffId,
        from_provider_thread_ids: Vec<ProviderThreadId>,
        to_provider_thread_id: ProviderThreadId,
        from_provider_instance_ids: Vec<ProviderInstanceId>,
        to_provider_instance_id: ProviderInstanceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_model_selections: Option<Vec<ModelSelection>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_model: Option<String>,
        strategy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Thread fork marker (`fork`).
    #[serde(rename_all = "camelCase")]
    Fork {
        source: Value,
        target_thread_id: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_thread_id: Option<ProviderThreadId>,
    },
    /// Child thread creation notice (`thread_created`).
    #[serde(rename_all = "camelCase")]
    ThreadCreated {
        target_thread_id: ThreadId,
        target_run_id: Option<RunId>,
        target_provider_instance_id: ProviderInstanceId,
        target_model: String,
    },
    /// Agent secret entry request card (`secret_request`).
    #[serde(rename_all = "camelCase")]
    SecretRequest {
        label: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
        secret_status: String,
    },
    /// Peer environment link request card (`link_request`).
    #[serde(rename_all = "camelCase")]
    LinkRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment_id: Option<EnvironmentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested_access: Option<McpClientAccess>,
        link_status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_environment_id: Option<EnvironmentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_access: Option<McpClientAccess>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
    /// Delegated or native subagent card (`subagent`).
    #[serde(rename_all = "camelCase")]
    Subagent {
        subagent_id: NodeId,
        origin: String,
        driver: ProviderDriverKind,
        provider_instance_id: ProviderInstanceId,
        child_thread_id: Option<ThreadId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_child: Option<Value>,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_omitted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_omitted: Option<bool>,
    },
    /// Provider-invoked dynamic tool call (`dynamic_tool`).
    #[serde(rename_all = "camelCase")]
    DynamicTool {
        tool_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        viewed_image_path: Option<String>,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_omitted: Option<bool>,
    },
}

/// Unused reference preserving import parity when checking approval decisions.
pub type ApprovalDecisionMarker = ProviderApprovalDecision;
