//! Tests for lock rank hierarchy, command receipt idempotency, and `BUG-01` nested savepoint rollback.

use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, ThreadId};
use legion_orchestrator::locks::{LockError, LockRank, OrchestratorLocks};
use legion_orchestrator::sink::projection_apply::DeciderThreadState;
use legion_orchestrator::sink::{
    CommitCommandInput, EventPubSub, EventSink, ReceiptStatus, SinkError,
};
use legion_orchestrator::{OrchestratorError, decide_command};
use legion_persistence::sqlite::SqliteStore;

use super::helpers::{TestResult, count_table_rows, create_thread_cmd, new_test_orchestrator};

#[tokio::test]
async fn lock_hierarchy_enforces_rank_order_reentrancy_and_child_order() -> TestResult {
    let locks = OrchestratorLocks::new();
    let parent = ThreadId::new("thread-a")?;
    let child_b = ThreadId::new("thread-b")?;
    let child_c = ThreadId::new("thread-c")?;

    // Valid full sequence via LockSession: DeletionGuard -> MessageIntake -> ParentThread -> ChildThread(s).
    {
        let s0 = locks.session();
        let s1 = s0.acquire_deletion_guard().await?;
        assert_eq!(s1.current_rank(), Some(LockRank::DeletionGuard));
        let s2 = s1.acquire_message_intake(parent.clone()).await?;
        assert_eq!(s2.current_rank(), Some(LockRank::MessageIntake));
        let s3 = s2.acquire_parent_thread(parent.clone()).await?;
        assert_eq!(s3.current_rank(), Some(LockRank::ParentThread));
        let s4 = s3
            .acquire_child_threads([child_c.clone(), child_b.clone()])
            .await?;
        assert_eq!(s4.current_rank(), Some(LockRank::ChildThread));
    }

    // Reentrancy on same ThreadId between ParentThread and ChildThread is rejected.
    let reentrant = locks
        .session()
        .acquire_parent_thread(parent.clone())
        .await?
        .acquire_child_thread(parent.clone())
        .await;
    assert!(matches!(
        reentrant,
        Err(LockError::ReentrantThreadLock { thread_id }) if thread_id == parent
    ));

    // Runtime rank inversion ChildThread -> ParentThread is rejected without waiting.
    let inverted = locks
        .with_child_thread(child_b.clone(), async {
            locks
                .with_parent_thread(parent.clone(), async { Ok::<(), LockError>(()) })
                .await
        })
        .await;
    assert!(matches!(
        inverted,
        Err(LockError::RankInversion {
            held: LockRank::ChildThread,
            requested: LockRank::ParentThread,
        })
    ));

    // Out-of-order manual nesting of ChildThread locks (c then b) rejects with ChildKeyOrder.
    let bad_child_order = locks
        .with_child_thread(child_c.clone(), async {
            locks
                .with_child_thread(child_b.clone(), async { Ok::<(), LockError>(()) })
                .await
        })
        .await;
    assert!(matches!(
        bad_child_order,
        Err(LockError::ChildKeyOrder { held, requested })
            if held == child_c && requested == child_b
    ));

    // Multi-child helper canonically sorts [c, b] so b -> c succeeds cleanly.
    let ok_sorted: Result<usize, LockError> = locks
        .with_child_threads([child_c, child_b], async { Ok(2) })
        .await;
    assert_eq!(ok_sorted?, 2);
    Ok(())
}

#[tokio::test]
async fn idempotent_command_receipt_replay_conflict_and_rejection() -> TestResult {
    let orch = new_test_orchestrator()?;
    let mut sub = orch.subscribe();
    let now = "2026-11-20T00:00:00.000Z";
    let cmd = create_thread_cmd("cmd-idem-1", "thread-1", "proj-1", "First thread")?;

    let first = orch.dispatch_command(&cmd, now).await?;
    assert!(!first.outcome.replayed);
    assert_eq!(first.outcome.events.len(), 1);
    assert_eq!(first.outcome.receipt.status, ReceiptStatus::Accepted);
    assert_eq!(sub.drain_ready().len(), 1);

    // Replaying identical CommandId returns persisted event without duplicate rows or PubSub emissions.
    let second = orch
        .dispatch_command(&cmd, "2026-11-20T00:01:00.000Z")
        .await?;
    assert!(second.outcome.replayed);
    assert_eq!(second.outcome.events, first.outcome.events);
    assert_eq!(
        second.outcome.receipt.result_sequence,
        first.outcome.receipt.result_sequence
    );
    assert_eq!(sub.drain_ready().len(), 0);

    // Conflicting ThreadId using same CommandId returns CommandIdConflict.
    let conflict_cmd = create_thread_cmd("cmd-idem-1", "thread-other", "proj-1", "Conflict")?;
    let conflict_res = orch.dispatch_command(&conflict_cmd, now).await;
    assert!(matches!(
        conflict_res,
        Err(OrchestratorError::Sink(SinkError::CommandIdConflict { .. }))
    ));

    // Rejected command persists durable rejection receipt; retry replays CommandPreviouslyRejected.
    let bad_create = create_thread_cmd("cmd-dup-thread", "thread-1", "proj-1", "Duplicate")?;
    let rejected = orch.dispatch_command(&bad_create, now).await;
    assert!(matches!(rejected, Err(OrchestratorError::Decider(_))));
    let retry_rejected = orch.dispatch_command(&bad_create, now).await;
    assert!(matches!(
        retry_rejected,
        Err(OrchestratorError::Sink(
            SinkError::CommandPreviouslyRejected { .. }
        ))
    ));
    assert_eq!(sub.drain_ready().len(), 0);

    let conn = orch.sink().db().lock().map_err(|e| e.to_string())?;
    assert_eq!(count_table_rows(&conn, "orchestration_events")?, 1);
    assert_eq!(
        count_table_rows(&conn, "orchestration_command_receipts")?,
        2
    );
    Ok(())
}

#[test]
fn bug_01_nested_savepoint_and_outer_tx_rollback_emit_zero_pubsub_events() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let pubsub = EventPubSub::new();
    let mut sub = pubsub.subscribe();
    let sink = EventSink::new(Arc::clone(&db), Arc::clone(&pubsub), None);
    let now = "2026-11-20T00:00:00.000Z";

    // Case 1: Inner savepoint commits, outer transaction explicitly rolls back -> ZERO PubSub or DB writes.
    {
        let mut tx = sink.begin_tx()?;
        let mut sp = tx.savepoint()?;
        let cmd = create_thread_cmd("cmd-sp-1", "thread-rolled-back", "proj-1", "Should vanish")?;
        let dec = decide_command(&DeciderThreadState::default(), &cmd, now)?;
        sp.commit_command(&dec.into_commit_input("server", now))?;
        sp.commit()?;
        // Even before outer rollback, buffered notifications have NOT fired yet.
        assert_eq!(sub.drain_ready().len(), 0);
        tx.rollback()?;
    }
    assert_eq!(sub.drain_ready().len(), 0);
    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        assert_eq!(count_table_rows(&conn, "orchestration_events")?, 0);
        assert_eq!(
            count_table_rows(&conn, "orchestration_v2_projection_threads")?,
            0
        );
    }

    // Case 2: Outer tx commits after savepoint 1 commits, savepoint 2 rolls back -> ONLY savepoint 1 emits.
    let cmd1 = create_thread_cmd("cmd-keep", "thread-keep", "proj-1", "Keep me")?;
    let cmd2 = create_thread_cmd("cmd-abort", "thread-abort", "proj-1", "Roll me back")?;
    let d1 = decide_command(&DeciderThreadState::default(), &cmd1, now)?;
    let d2 = decide_command(&DeciderThreadState::default(), &cmd2, now)?;

    {
        let mut tx = sink.begin_tx()?;
        let mut sp1 = tx.savepoint()?;
        sp1.commit_command(&d1.into_commit_input("server", now))?;
        sp1.commit()?;

        let mut sp2 = tx.savepoint()?;
        sp2.commit_command(&d2.into_commit_input("server", now))?;
        sp2.rollback()?;

        // Nothing emitted before outermost COMMIT succeeds.
        assert_eq!(sub.drain_ready().len(), 0);
        tx.commit()?;
    }

    let emitted = sub.drain_ready();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].event.thread_id, ThreadId::new("thread-keep")?);
    assert_eq!(
        emitted[0].metadata.command_id,
        Some(CommandId::new("cmd-keep")?)
    );

    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        assert_eq!(count_table_rows(&conn, "orchestration_events")?, 1);
        assert_eq!(
            count_table_rows(&conn, "orchestration_v2_projection_threads")?,
            1
        );
    }
    assert!(sink.get_receipt(&CommandId::new("cmd-abort")?)?.is_none());
    Ok(())
}

#[test]
fn v2_events_always_persist_application_event_version_2_and_empty_input_ok() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let pubsub = EventPubSub::new();
    let sink = EventSink::new(Arc::clone(&db), Arc::clone(&pubsub), None);
    let now = "2026-11-20T00:00:00.000Z";
    let cmd = create_thread_cmd("cmd-ver-1", "thread-ver", "proj-1", "Version check")?;
    let dec = decide_command(&DeciderThreadState::default(), &cmd, now)?;
    let input: CommitCommandInput = dec.into_commit_input("server", now);
    sink.commit_command(&input)?;

    let conn = db.lock().map_err(|e| e.to_string())?;
    let version: i64 = conn.query_row(
        "SELECT application_event_version FROM orchestration_events LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(version, 2);
    Ok(())
}
