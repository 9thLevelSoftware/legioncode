//! Orchestration v2 commands, stored domain events, and forward-compatible stream items.
//!
//! Mirrors `packages/contracts/src/orchestrationV2.ts` while enforcing strict `type`
//! tag validation (fixing BUG-18):
//! - [`OrchestrationCommand`] accepts all 58 client-dispatchable commands and rejects
//!   the 17 server-only `OrchestrationV2InternalCommand` variants.
//! - [`StoredDomainEvent`] wraps persisted sequence numbers, nullable command IDs, and
//!   all 49 domain event types.
//! - [`StreamItem`] maps unknown *string* event/turn-item types to [`StreamItem::UnknownEvent`]
//!   while rejecting missing or non-string `"type"` tags (such as numbers or `null`) with `Err`.
//!
//! # Examples
//!
//! ```
//! use legion_contracts::orchestration::{OrchestrationCommand, StreamItem};
//!
//! let cmd: OrchestrationCommand = serde_json::from_str(
//!     r#"{"type":"thread.archive","commandId":"cmd-1","threadId":"thread-1"}"#,
//! )?;
//! assert_eq!(cmd.command_type(), "thread.archive");
//!
//! // Server-only internal commands are rejected on client command deserializers.
//! let internal = r#"{"type":"thread.guest-share.open","commandId":"cmd-2","threadId":"t1","shareId":"s1","host":"h","expiresAt":"2026-01-01T00:00:00Z"}"#;
//! assert!(serde_json::from_str::<OrchestrationCommand>(internal).is_err());
//!
//! // Unknown string event types map to StreamItem::UnknownEvent, while non-string `type` fails.
//! let unknown: StreamItem = serde_json::from_str(
//!     r#"{"kind":"event","sequence":2,"event":{"id":"e2","type":"future.event","threadId":"t1","occurredAt":"2026-01-01T00:00:00Z","payload":{}}}"#,
//! )?;
//! assert!(matches!(unknown, StreamItem::UnknownEvent { sequence: 2, .. }));
//!
//! let bad_tag = r#"{"kind":"event","sequence":3,"event":{"id":"e3","type":"turn-item.updated","threadId":"t1","occurredAt":"2026-01-01T00:00:00Z","payload":{"type":123}}}"#;
//! assert!(serde_json::from_str::<StreamItem>(bad_tag).is_err());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod checkpoint_models;
mod command_meta;
mod command_payloads;
mod command_payloads_extra;
mod command_payloads_pr;
mod commands;
mod common;
mod event_payloads;
mod events;
mod item_kinds;
mod items;
mod model_selection;
mod run_models;
mod run_output;
mod runtime_models;
mod state_models;
mod stream;
mod wire_command;

pub use checkpoint_models::{
    Checkpoint, CheckpointRollbackRequest, CheckpointScope, ContextHandoff, ContextTransfer,
    PlanArtifact, repository_rewind_checkpoint,
};
pub use command_meta::{INTERNAL_COMMAND_TYPES, is_internal_command_type};
pub use command_payloads::{
    DispatchMode, ImportedNativeThreadPayload, ImportedNativeThreadRef, InitialThreadMessage,
    MessageDispatchCommand, MetadataContinuation, ProviderInteractionMode, ThreadCreateCommand,
    ThreadMetadataUpdateCommand, WorkflowThreadCreateCommand,
};
pub use command_payloads_extra::{
    CheckpointRollbackCommand, CompletionWakePolicy, ExtraCommandTypes, PreparedRunPhase,
    ProviderApprovalDecision, QueuedRunEditCommand, RuntimeRequestRespondCommand,
    ThreadCreatedRecordCommand, ThreadIssueLinkCommand, ThreadIssueLinkSource,
    ThreadIssueUnlinkCommand, ThreadMergeBackCommand, ThreadPullRequestLinkCommand,
    ThreadTitleRegenerationCompleteCommand, ThreadTitleRegenerationOutcome, ThreadUnsettleCommand,
    ThreadVisitCommand, UserActionReason,
};
pub use command_payloads_pr::{
    DelegatedTaskCompletionAckCommand, DelegatedTaskRequestCommand, ProviderSwitchCommand,
    PullRequestWatchLinkInfo, QueuedRunReorderCommand, RunInterruptCommand, ThreadForkCommand,
    ThreadPullRequestLinkSyncCommand, ThreadPullRequestSyncCommand, ThreadPullRequestSyncExpected,
    ThreadPullRequestWatchCommand, ThreadStopCommand,
};
pub use commands::OrchestrationCommand;
pub use common::{
    ContextHandoffRef, CreationSource, DelegatedCompletionRef, DelegatedFrom, JsonRecord,
    LinkOrigin, LinkedSender, McpClientAccess, NativeRefStrength, OrchestrationActor, PatchValue,
    ProviderRef, ProviderRefsPair, ScheduledTaskRef, SourcePlanRef, ThreadHistoryOrigin,
};
pub use event_payloads::{
    DomainEventPayload, KNOWN_DOMAIN_EVENT_TYPES, is_known_domain_event_type,
};
pub use events::{DomainEvent, StoredDomainEvent};
pub use item_kinds::{ApprovalDecisionMarker, SecretRequestField, TurnItemKind};
pub use items::{
    CheckpointFileSummary, CheckpointFileTotals, DeferredWorkCursor, KNOWN_TURN_ITEM_TYPES,
    PlanStep, ProjectedTurnItem, TurnItem, TurnItemBase, TurnItemStatus, TurnItemVisibility,
    is_known_turn_item_type,
};
pub use model_selection::{
    ContextSourcePoint, LimitRecovery, LimitRecoveryUpdate, ModelSelection,
    OrchestrationNotification, ProviderFailure, ProviderFailureClass, ProviderOptionSelection,
    ProviderOptionValue, ProviderRetry, ThreadForkSourcePoint, WorkspaceStrategy,
    WorkspaceStrategyRepository,
};
pub use run_models::{
    ExecutionNode, Run, RunAttempt, RunBackgroundWorkCancelled, RunStatus, Subagent,
};
pub use run_output::{
    MAX_RUN_OUTPUT_SCHEMA_CHARS, RunOutputPayload, RunOutputSchema, RunOutputSubmitted,
    RunOutputValidationError, SubmitRunOutputCommand, format_run_output_instructions,
    validate_run_output_schema_size,
};
pub use runtime_models::{
    ConversationMessage, ProviderSession, ProviderSessionDetached, ProviderThread, ProviderTurn,
    ProviderTurnWire, RuntimeRequest, is_delivered_provider_turn,
};
pub use state_models::{
    AppThread, RollbackUnrestored, ThreadLineage, ThreadVisit, WorkflowAppThread, WorkflowOrigin,
    WorkflowThreadLineage, is_listed_lineage, is_listed_thread_lineage,
};
pub use stream::{StreamItem, ThreadProjection};
pub use wire_command::{
    ProjectCreateWireCommand, ProjectUpdateWireCommand, ProjectWireCommand, ThreadEnvMode,
    WireClientCommand, decode_wire_client_command, decode_wire_orchestration_command,
    default_wire_provider_capabilities, sha256_digest,
};
