//! Pure [`OrchestrationCommand`] decider producing atomic events, outbox effects, and intents.
//!
//! `decide_command` performs zero I/O, zero locking, and zero destructive git or provider side effects.
//! Destructive operations such as `checkpoint.rollback` first emit a durable [`RollbackIntent`]
//! (`"rollback.pending"`) plus an outbox row so `EventSink` commits durability BEFORE
//! `EffectOutboxWorker` invokes any git restore (`BUG-02`).
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_contracts::ids::{CommandId, ProjectId, ProviderInstanceId, ThreadId};
//! use t3_contracts::orchestration::{
//!     CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand,
//!     ProviderInteractionMode, ThreadCreateCommand,
//! };
//! use t3_contracts::policy::RuntimeMode;
//! use t3_orchestrator::decider::decide_command;
//! use t3_orchestrator::sink::projection_apply::DeciderThreadState;
//!
//! let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
//!     created_by: OrchestrationActor::User,
//!     creation_source: CreationSource::Web,
//!     command_id: CommandId::new("cmd-create-1")?,
//!     thread_id: ThreadId::new("thread-1")?,
//!     project_id: ProjectId::new("proj-1")?,
//!     title: "Fix nested savepoint leak".to_owned(),
//!     generate_title: None,
//!     model_selection: ModelSelection {
//!         instance_id: ProviderInstanceId::new("codex")?,
//!         model: "gpt-5".to_owned(),
//!         options: None,
//!     },
//!     runtime_mode: RuntimeMode::FullAccess,
//!     interaction_mode: ProviderInteractionMode::Default,
//!     branch: None,
//!     worktree_path: None,
//!     link_origin: None,
//!     delegated_from: None,
//!     imported_native_thread: None,
//!     initial_message: None,
//! });
//! let decision = decide_command(&DeciderThreadState::default(), &cmd, "2026-11-20T00:00:00.000Z")?;
//! assert_eq!(decision.events.len(), 1);
//! # Ok(())
//! # }
//! ```

pub mod create_ops;
pub mod message_ops;
pub mod output_ops;
mod output_schema;
pub mod pr_ops;
mod queue_edit_ops;
pub mod queue_ops;
pub mod run_ops;
pub mod state_ops;
pub mod thread_ops;
pub mod types;

use t3_contracts::orchestration::OrchestrationCommand;

use crate::sink::projection_apply::DeciderThreadState;
pub use output_ops::decide_submit_run_output;
pub use queue_ops::decide_pop_next_queued_run;
pub use types::{CommandDecision, DeciderError, RollbackIntent};

/// Purely evaluates `command` against `state` at `now_iso`, returning events and outbox effects.
pub fn decide_command(
    state: &DeciderThreadState,
    command: &OrchestrationCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    if let Some(res) = decide_thread_state_command(state, command, now_iso) {
        return res;
    }
    match command {
        OrchestrationCommand::MessageDispatch(cmd) => {
            message_ops::decide_message_dispatch(state, cmd, now_iso)
        }
        OrchestrationCommand::QueuedMessagePromoteToSteer {
            command_id,
            thread_id,
            queued_run_id,
            target_run_id,
        } => queue_edit_ops::decide_queued_message_promote_to_steer(
            state,
            command_id,
            thread_id,
            queued_run_id,
            target_run_id,
            now_iso,
        ),
        OrchestrationCommand::QueuedRunEdit(cmd) => {
            queue_edit_ops::decide_queued_run_edit(state, cmd, now_iso)
        }
        OrchestrationCommand::RunInterrupt(cmd) => {
            run_ops::decide_run_interrupt(state, cmd, now_iso)
        }
        OrchestrationCommand::ThreadStop(cmd) => run_ops::decide_thread_stop(state, cmd, now_iso),
        OrchestrationCommand::CheckpointRollback(cmd) => {
            run_ops::decide_checkpoint_rollback(state, cmd, now_iso)
        }
        OrchestrationCommand::ThreadModelSelectionSet {
            command_id,
            thread_id,
            model_selection,
        } => state_ops::decide_thread_model_selection_set(
            state,
            command_id,
            thread_id,
            model_selection,
            now_iso,
        ),
        other => Err(DeciderError::UnsupportedCommand {
            command_type: other.command_type().to_owned(),
        }),
    }
}

fn decide_thread_state_command(
    state: &DeciderThreadState,
    command: &OrchestrationCommand,
    now_iso: &str,
) -> Option<Result<CommandDecision, DeciderError>> {
    Some(match command {
        OrchestrationCommand::ThreadCreate(cmd) => {
            create_ops::decide_thread_create(state, cmd, now_iso)
        }
        OrchestrationCommand::ThreadArchive {
            command_id,
            thread_id,
        } => thread_ops::decide_thread_archive(state, command_id, thread_id, now_iso),
        OrchestrationCommand::ThreadUnarchive {
            command_id,
            thread_id,
        } => thread_ops::decide_thread_unarchive(state, command_id, thread_id, now_iso),
        OrchestrationCommand::ThreadDelete {
            command_id,
            thread_id,
            only_if_unused,
        } => thread_ops::decide_thread_delete(
            state,
            command_id,
            thread_id,
            only_if_unused.unwrap_or(false),
            now_iso,
        ),
        OrchestrationCommand::ThreadSettle {
            command_id,
            thread_id,
            expected_latest_run_id,
            settled_at,
        } => state_ops::decide_thread_settle(
            state,
            command_id,
            thread_id,
            expected_latest_run_id.as_ref(),
            settled_at.as_deref(),
            now_iso,
        ),
        OrchestrationCommand::ThreadAutoSettle {
            command_id,
            thread_id,
            snapshot_at,
            settled_at,
        } => state_ops::decide_thread_auto_settle(
            state,
            command_id,
            thread_id,
            snapshot_at,
            settled_at.as_deref(),
            now_iso,
        ),
        OrchestrationCommand::ThreadUnsettle(cmd) => {
            state_ops::decide_thread_unsettle(state, cmd, now_iso)
        }
        OrchestrationCommand::ThreadSnooze {
            command_id,
            thread_id,
            snoozed_until,
        } => state_ops::decide_thread_snooze(state, command_id, thread_id, snoozed_until, now_iso),
        OrchestrationCommand::ThreadUnsnooze {
            command_id,
            thread_id,
            ..
        } => state_ops::decide_thread_unsnooze(state, command_id, thread_id, now_iso),
        OrchestrationCommand::ThreadPin {
            command_id,
            thread_id,
            order_key,
        } => state_ops::decide_thread_pin(
            state,
            command_id,
            thread_id,
            order_key.as_deref(),
            now_iso,
        ),
        OrchestrationCommand::ThreadUnpin {
            command_id,
            thread_id,
        } => state_ops::decide_thread_unpin(state, command_id, thread_id, now_iso),
        OrchestrationCommand::ThreadPullRequestLink(cmd) => {
            pr_ops::decide_pr_link(state, cmd, now_iso)
        }
        OrchestrationCommand::ThreadPullRequestUnlink {
            command_id,
            thread_id,
            host,
            repository,
            number,
        } => pr_ops::decide_pr_unlink(
            state, command_id, thread_id, host, repository, *number, now_iso,
        ),
        _ => return None,
    })
}
