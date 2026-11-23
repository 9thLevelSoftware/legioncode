//! Idempotent command receipt replay and target thread resolution for [`crate::Orchestrator`].

use legion_contracts::ids::{CommandId, ThreadId};
use legion_contracts::orchestration::OrchestrationCommand;

use crate::orchestrator::{DispatchResult, OrchestratorError};
use crate::sink::event_codec::load_events_by_command;
use crate::sink::projection_apply::load_thread_projection;
use crate::sink::{
    CommandReceipt, CommitCommandOutcome, ReceiptStatus, SinkError, SinkTransaction,
};

pub(crate) fn replay_existing_receipt(
    tx: SinkTransaction<'_>,
    existing: CommandReceipt,
    command_id: CommandId,
    thread_id: &ThreadId,
) -> Result<DispatchResult, OrchestratorError> {
    if existing.aggregate_id != thread_id.as_str() {
        return Err(SinkError::CommandIdConflict {
            command_id,
            existing_thread_id: existing.aggregate_id,
            requested_thread_id: thread_id.as_str().to_owned(),
        }
        .into());
    }
    if existing.status == ReceiptStatus::Rejected {
        return Err(SinkError::CommandPreviouslyRejected {
            command_id,
            error: existing
                .error
                .unwrap_or_else(|| "command previously rejected".to_owned()),
        }
        .into());
    }
    let events = load_events_by_command(tx.conn()?, &command_id)?;
    let resulting_thread = load_thread_projection(tx.conn()?, thread_id)?;
    tx.rollback()?;
    Ok(DispatchResult {
        outcome: CommitCommandOutcome {
            events,
            receipt: existing,
            replayed: true,
        },
        rollback_intent: None,
        resulting_thread,
    })
}

pub(crate) fn command_thread_id(command: &OrchestrationCommand) -> Option<ThreadId> {
    serde_json::to_value(command)
        .ok()
        .and_then(|val| {
            val.get("threadId")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .and_then(|raw| ThreadId::new(raw).ok())
}
