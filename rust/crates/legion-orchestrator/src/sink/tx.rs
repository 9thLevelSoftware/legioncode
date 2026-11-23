//! Transaction, savepoint, and receipt-idempotent commit execution for [`crate::sink::EventSink`].

use std::sync::{Arc, MutexGuard};

use legion_contracts::ids::CommandId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::outbox::{OutboxSignal, enqueue_effects};
use crate::sink::event_codec::{StoredEventMetadata, insert_thread_events, load_events_by_command};
use crate::sink::projection_apply::apply_events_to_projections;
use crate::sink::types::{
    CommandReceipt, CommitBatch, CommitCheckpoint, CommitCommandInput, CommitCommandOutcome,
    EventPubSub, ReceiptStatus, SinkError,
};

/// Outermost `SQLite` transaction holding buffered notifications until `COMMIT` (`BUG-01`).
pub struct SinkTransaction<'a> {
    pub(crate) guard: Option<MutexGuard<'a, Connection>>,
    pub(crate) batch: CommitBatch,
    pub(crate) pubsub: Arc<EventPubSub>,
    pub(crate) outbox_signal: Option<Arc<OutboxSignal>>,
    pub(crate) next_sp_id: u64,
    pub(crate) settled: bool,
}

impl SinkTransaction<'_> {
    /// Returns a reference to the underlying `SQLite` connection inside this transaction.
    pub fn conn(&self) -> Result<&Connection, SinkError> {
        self.guard
            .as_deref()
            .ok_or_else(|| SinkError::Codec("transaction already settled".to_owned()))
    }

    /// Returns mutable access to the buffered post-commit batch.
    pub fn batch_mut(&mut self) -> &mut CommitBatch {
        &mut self.batch
    }

    /// Opens a nested savepoint. Dropping without `commit()` rolls back both `SQLite` and [`CommitBatch`].
    pub fn savepoint(&mut self) -> Result<SinkSavepoint<'_>, SinkError> {
        let conn = self
            .guard
            .as_deref()
            .ok_or_else(|| SinkError::Codec("transaction already settled".to_owned()))?;
        self.next_sp_id += 1;
        let name = format!("legion_sp_{}", self.next_sp_id);
        conn.execute_batch(&format!("SAVEPOINT {name};"))?;
        let checkpoint = self.batch.checkpoint();
        Ok(SinkSavepoint {
            conn,
            next_sp_id: &mut self.next_sp_id,
            batch: &mut self.batch,
            name,
            checkpoint,
            settled: false,
        })
    }

    /// Executes `f` inside a nested savepoint, committing or rolling back both `SQLite` and [`CommitBatch`].
    pub fn with_savepoint<R>(
        &mut self,
        f: impl FnOnce(&mut SinkSavepoint<'_>) -> Result<R, SinkError>,
    ) -> Result<R, SinkError> {
        let mut sp = self.savepoint()?;
        let out = f(&mut sp)?;
        sp.commit()?;
        Ok(out)
    }

    /// Commits a command inside this transaction using idempotent receipt handling.
    pub fn commit_command(
        &mut self,
        input: &CommitCommandInput,
    ) -> Result<CommitCommandOutcome, SinkError> {
        let conn = self
            .guard
            .as_deref()
            .ok_or_else(|| SinkError::Codec("transaction already settled".to_owned()))?;
        commit_command_on_conn(conn, &mut self.batch, input)
    }

    /// Records a `rejected` command receipt inside this transaction.
    pub fn commit_rejected_command(
        &mut self,
        command_id: &CommandId,
        command_type: &str,
        thread_id: &str,
        error: &str,
        now_iso: &str,
    ) -> Result<CommandReceipt, SinkError> {
        let conn = self.conn()?;
        commit_rejected_on_conn(conn, command_id, command_type, thread_id, error, now_iso)
    }

    /// Commits the outermost `SQLite` transaction, releases the lock, and ONLY THEN emits notifications.
    pub fn commit(mut self) -> Result<(), SinkError> {
        let guard = self
            .guard
            .take()
            .ok_or_else(|| SinkError::Codec("transaction already settled".to_owned()))?;
        self.settled = true;
        guard.execute_batch("COMMIT;")?;
        drop(guard);
        let batch = std::mem::take(&mut self.batch);
        batch.publish_after_commit(&self.pubsub, self.outbox_signal.as_deref());
        Ok(())
    }

    /// Explicitly rolls back the outermost `SQLite` transaction and discards all buffered notifications.
    pub fn rollback(mut self) -> Result<(), SinkError> {
        let guard = self
            .guard
            .take()
            .ok_or_else(|| SinkError::Codec("transaction already settled".to_owned()))?;
        self.settled = true;
        guard.execute_batch("ROLLBACK;")?;
        Ok(())
    }
}

impl Drop for SinkTransaction<'_> {
    fn drop(&mut self) {
        if !self.settled
            && let Some(guard) = self.guard.take()
        {
            let _ = guard.execute_batch("ROLLBACK;");
        }
    }
}

/// Nested `SQLite` savepoint paired with a [`CommitCheckpoint`] (`BUG-01`).
pub struct SinkSavepoint<'a> {
    conn: &'a Connection,
    next_sp_id: &'a mut u64,
    batch: &'a mut CommitBatch,
    name: String,
    checkpoint: CommitCheckpoint,
    settled: bool,
}

impl SinkSavepoint<'_> {
    /// Returns the underlying `SQLite` connection.
    pub const fn conn(&self) -> &Connection {
        self.conn
    }

    /// Opens a deeper nested savepoint inside this savepoint.
    pub fn savepoint(&mut self) -> Result<SinkSavepoint<'_>, SinkError> {
        *self.next_sp_id += 1;
        let name = format!("legion_sp_{}", *self.next_sp_id);
        self.conn.execute_batch(&format!("SAVEPOINT {name};"))?;
        let checkpoint = self.batch.checkpoint();
        Ok(SinkSavepoint {
            conn: self.conn,
            next_sp_id: self.next_sp_id,
            batch: self.batch,
            name,
            checkpoint,
            settled: false,
        })
    }

    /// Commits a command inside this savepoint; notifications stay buffered until outermost `COMMIT`.
    pub fn commit_command(
        &mut self,
        input: &CommitCommandInput,
    ) -> Result<CommitCommandOutcome, SinkError> {
        commit_command_on_conn(self.conn, self.batch, input)
    }

    /// Releases the `SQLite` savepoint into the parent scope while keeping events buffered until outermost commit.
    pub fn commit(mut self) -> Result<(), SinkError> {
        self.conn
            .execute_batch(&format!("RELEASE SAVEPOINT {};", self.name))?;
        self.settled = true;
        Ok(())
    }

    /// Explicitly rolls back this savepoint and truncates buffered notifications back to its checkpoint.
    pub fn rollback(mut self) -> Result<(), SinkError> {
        rollback_savepoint_sql(self.conn, &self.name)?;
        self.batch.rollback_to(self.checkpoint);
        self.settled = true;
        Ok(())
    }
}

impl Drop for SinkSavepoint<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.batch.rollback_to(self.checkpoint);
            let _ = rollback_savepoint_sql(self.conn, &self.name);
        }
    }
}

fn rollback_savepoint_sql(conn: &Connection, name: &str) -> Result<(), rusqlite::Error> {
    conn.execute_batch(&format!(
        "ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name};"
    ))
}

pub(crate) fn commit_command_on_conn(
    conn: &Connection,
    batch: &mut CommitBatch,
    input: &CommitCommandInput,
) -> Result<CommitCommandOutcome, SinkError> {
    if let Some(existing) = load_receipt(conn, &input.command_id)? {
        if existing.aggregate_id != input.thread_id.as_str() {
            return Err(SinkError::CommandIdConflict {
                command_id: input.command_id.clone(),
                existing_thread_id: existing.aggregate_id,
                requested_thread_id: input.thread_id.as_str().to_owned(),
            });
        }
        if existing.status == ReceiptStatus::Rejected {
            return Err(SinkError::CommandPreviouslyRejected {
                command_id: input.command_id.clone(),
                error: existing
                    .error
                    .unwrap_or_else(|| "command previously rejected".to_owned()),
            });
        }
        let events = load_events_by_command(conn, &input.command_id)?;
        return Ok(CommitCommandOutcome {
            events,
            receipt: existing,
            replayed: true,
        });
    }

    let meta = StoredEventMetadata {
        actor_kind: input.actor_kind.clone(),
        causation_event_id: input.causation_event_id.clone(),
        correlation_id: input.correlation_id.clone(),
        command_id: Some(input.command_id.clone()),
    };
    let persisted = insert_thread_events(conn, &meta, &input.events)?;
    apply_events_to_projections(conn, &persisted, &input.now_iso)?;
    let result_sequence = persisted.iter().map(|ev| ev.sequence).max().unwrap_or(0);

    let receipt = CommandReceipt {
        command_id: input.command_id.clone(),
        aggregate_kind: "thread".to_owned(),
        aggregate_id: input.thread_id.as_str().to_owned(),
        accepted_at: input.now_iso.clone(),
        result_sequence,
        status: ReceiptStatus::Accepted,
        error: None,
        command_type: input.command_type.clone(),
    };
    insert_receipt(conn, &receipt)?;
    if !input.outbox_effects.is_empty() {
        enqueue_effects(conn, &input.outbox_effects, &input.now_iso)?;
    }
    batch.push_events(&persisted);
    batch.add_outbox_wakeups(input.outbox_effects.len());

    Ok(CommitCommandOutcome {
        events: persisted,
        receipt,
        replayed: false,
    })
}

pub(crate) fn commit_rejected_on_conn(
    conn: &Connection,
    command_id: &CommandId,
    command_type: &str,
    thread_id: &str,
    error: &str,
    now_iso: &str,
) -> Result<CommandReceipt, SinkError> {
    if let Some(existing) = load_receipt(conn, command_id)? {
        if existing.aggregate_id != thread_id {
            return Err(SinkError::CommandIdConflict {
                command_id: command_id.clone(),
                existing_thread_id: existing.aggregate_id,
                requested_thread_id: thread_id.to_owned(),
            });
        }
        return Ok(existing);
    }
    let receipt = CommandReceipt {
        command_id: command_id.clone(),
        aggregate_kind: "thread".to_owned(),
        aggregate_id: thread_id.to_owned(),
        accepted_at: now_iso.to_owned(),
        result_sequence: 0,
        status: ReceiptStatus::Rejected,
        error: Some(error.to_owned()),
        command_type: command_type.to_owned(),
    };
    insert_receipt(conn, &receipt)?;
    Ok(receipt)
}

pub(crate) fn load_receipt(
    conn: &Connection,
    command_id: &CommandId,
) -> Result<Option<CommandReceipt>, SinkError> {
    let row = conn
        .query_row(
            "SELECT command_id, aggregate_kind, aggregate_id, accepted_at,
                    result_sequence, status, error, command_type
             FROM orchestration_command_receipts
             WHERE command_id = ?1
             LIMIT 1",
            params![command_id.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(|t| {
        Ok(CommandReceipt {
            command_id: CommandId::new(t.0).map_err(|e| SinkError::Codec(e.to_string()))?,
            aggregate_kind: t.1,
            aggregate_id: t.2,
            accepted_at: t.3,
            result_sequence: t.4,
            status: ReceiptStatus::from_db(&t.5)?,
            error: t.6,
            command_type: t.7,
        })
    })
    .transpose()
}

fn insert_receipt(conn: &Connection, receipt: &CommandReceipt) -> Result<(), SinkError> {
    conn.execute(
        "INSERT INTO orchestration_command_receipts (
           command_id, aggregate_kind, aggregate_id, accepted_at,
           result_sequence, status, error, command_type
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            receipt.command_id.as_str(),
            receipt.aggregate_kind,
            receipt.aggregate_id,
            receipt.accepted_at,
            receipt.result_sequence,
            receipt.status.as_str(),
            receipt.error.as_deref(),
            receipt.command_type,
        ],
    )?;
    Ok(())
}
