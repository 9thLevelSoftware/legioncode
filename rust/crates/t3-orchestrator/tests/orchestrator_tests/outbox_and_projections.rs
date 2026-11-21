//! Tests for deterministic `EffectOutboxWorker::drain` and 24h `prune_event_store`.

use std::sync::{Arc, Mutex};

use rusqlite::params;
use t3_contracts::ids::{CheckpointId, CheckpointScopeId, CommandId, RunId, ThreadId};
use t3_contracts::orchestration::{DispatchMode, DomainEventPayload};
use t3_orchestrator::Orchestrator;
use t3_orchestrator::outbox::{
    EffectOutboxWorker, OutboxEffect, OutboxEffectRequest, OutboxStatus, OutboxWorkerOptions,
    PendingOutboxEffect, enqueue_effects, get_effect, reconcile_after_process_loss,
};
use t3_orchestrator::projections::prune_event_store;
use t3_orchestrator::sink::CommitCommandInput;
use t3_persistence::sqlite::SqliteStore;

use super::helpers::{
    TestResult, create_thread_cmd, make_run_completed_event, new_test_orchestrator,
    send_message_cmd,
};

#[tokio::test]
async fn outbox_worker_drain_retries_and_preserves_per_thread_fifo() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let execution_log = Arc::new(Mutex::new(Vec::<(String, u64)>::new()));
    let log_ref = Arc::clone(&execution_log);

    let worker = EffectOutboxWorker::spawn(
        Arc::clone(&db),
        move |effect: OutboxEffect| {
            let log_ref = Arc::clone(&log_ref);
            async move {
                log_ref
                    .lock()
                    .map_err(|e| e.to_string())?
                    .push((effect.id.clone(), effect.attempt_count));
                if effect.id == "eff-1" && effect.attempt_count < 3 {
                    return Err::<(), String>("transient failure".to_owned());
                }
                Ok(())
            }
        },
        OutboxWorkerOptions {
            worker_id: "test-worker".to_owned(),
            max_attempts: 5,
            retry_delay_ms: 0,
        },
    );

    let t0 = "2026-11-20T00:00:00.000Z";
    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let tid = ThreadId::new("thread-fifo")?;
        enqueue_effects(
            &conn,
            &[
                PendingOutboxEffect {
                    id: "eff-1".to_owned(),
                    command_id: CommandId::new("cmd-fifo-1")?,
                    thread_id: tid.clone(),
                    request: OutboxEffectRequest::ProviderTurnStart {
                        run_id: RunId::new("run-1")?,
                    },
                    available_at: None,
                },
                PendingOutboxEffect {
                    id: "eff-2".to_owned(),
                    command_id: CommandId::new("cmd-fifo-2")?,
                    thread_id: tid,
                    request: OutboxEffectRequest::ProviderTurnStart {
                        run_id: RunId::new("run-2")?,
                    },
                    available_at: None,
                },
            ],
            t0,
        )?;
    }

    worker.notify_available();
    let stats = worker.drain().await?;
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.failed_attempts, 2);
    assert_eq!(stats.in_flight, 0);

    let trace = execution_log.lock().map_err(|e| e.to_string())?.clone();
    assert_eq!(
        trace,
        vec![
            ("eff-1".to_owned(), 1),
            ("eff-1".to_owned(), 2),
            ("eff-1".to_owned(), 3),
            ("eff-2".to_owned(), 1),
        ]
    );
    worker.shutdown().await?;
    Ok(())
}

#[test]
fn reconcile_after_process_loss_cancels_process_bound_and_requeues_rollback() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t0 = "2026-11-20T00:00:00.000Z";
    let tid = ThreadId::new("thread-crash")?;

    enqueue_effects(
        conn,
        &[
            PendingOutboxEffect {
                id: "eff-crash-turn".to_owned(),
                command_id: CommandId::new("cmd-c-1")?,
                thread_id: tid.clone(),
                request: OutboxEffectRequest::ProviderTurnStart {
                    run_id: RunId::new("run-c-1")?,
                },
                available_at: None,
            },
            PendingOutboxEffect {
                id: "eff-crash-rb".to_owned(),
                command_id: CommandId::new("cmd-c-2")?,
                thread_id: tid,
                request: OutboxEffectRequest::rollback(
                    CommandId::new("cmd-c-2")?,
                    None,
                    CheckpointId::new("cp-1")?,
                    CheckpointScopeId::new("scope-1")?,
                    true,
                ),
                available_at: None,
            },
        ],
        t0,
    )?;
    conn.execute(
        "UPDATE orchestration_v2_effect_outbox SET status = 'running', lease_owner = 'dead'
         WHERE effect_id IN ('eff-crash-turn', 'eff-crash-rb')",
        [],
    )?;
    let (requeued, cancelled) = reconcile_after_process_loss(conn, "2026-11-20T00:05:00.000Z")?;
    assert_eq!(requeued, 1);
    assert_eq!(cancelled, 1);
    assert_eq!(
        get_effect(conn, "eff-crash-turn")?.map(|e| e.status),
        Some(OutboxStatus::Cancelled)
    );
    assert_eq!(
        get_effect(conn, "eff-crash-rb")?.map(|e| e.status),
        Some(OutboxStatus::Pending)
    );
    Ok(())
}

async fn seed_prune_fixture(orch: &Orchestrator, t_old: &str) -> TestResult {
    orch.dispatch_command(
        &create_thread_cmd("c-pr-create", "thread-prune", "proj-1", "Prune me")?,
        t_old,
    )
    .await?;
    let msg_res = orch
        .dispatch_command(
            &send_message_cmd(
                "c-pr-msg",
                "thread-prune",
                "m-pr-1",
                "Run 1",
                DispatchMode::StartImmediately,
            )?,
            t_old,
        )
        .await?;
    let run1 = msg_res
        .outcome
        .events
        .iter()
        .find_map(|e| match &e.event.payload {
            DomainEventPayload::RunUpdated { run, .. } => Some((**run).clone()),
            _ => None,
        })
        .ok_or("expected run1")?;
    let complete_ev = make_run_completed_event("ev-run1-done", run1, t_old)?;
    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("c-pr-complete")?,
        command_type: "run.complete".to_owned(),
        thread_id: ThreadId::new("thread-prune")?,
        actor_kind: "provider".to_owned(),
        causation_event_id: None,
        correlation_id: None,
        now_iso: t_old.to_owned(),
        events: vec![complete_ev],
        outbox_effects: Vec::new(),
    })?;

    orch.dispatch_command(
        &create_thread_cmd("c-act-create", "thread-active", "proj-1", "Active")?,
        t_old,
    )
    .await?;
    orch.dispatch_command(
        &send_message_cmd(
            "c-act-msg",
            "thread-active",
            "m-act-1",
            "Still running",
            DispatchMode::StartImmediately,
        )?,
        t_old,
    )
    .await?;
    let conn = orch.sink().db().lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO orchestration_events (
           event_id, aggregate_kind, stream_id, stream_version, event_type,
           occurred_at, actor_kind, payload_json, metadata_json, application_event_version
         )
         VALUES ('ev-legacy-v1', 'thread', 'thread-legacy', 1, 'thread.legacy', ?1, 'server', '{}', '{}', 1)",
        params![t_old],
    )?;
    Ok(())
}

#[tokio::test]
async fn prune_event_store_runs_after_24h_preserving_v1_created_active_runs_and_heads() -> TestResult
{
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-20T00:00:00.000Z";
    let t_old = "2026-11-19T23:00:00.000Z";
    let t_23h = "2026-11-20T23:00:00.000Z";
    let t_24h = "2026-11-21T00:00:00.000Z";

    seed_prune_fixture(&orch, t_old).await?;

    let conn = orch.sink().db().lock().map_err(|e| e.to_string())?;
    let r0 = prune_event_store(&conn, t0)?;
    assert!(!r0.ran);
    assert_eq!(r0.deleted_event_count, 0);

    let r23 = prune_event_store(&conn, t_23h)?;
    assert!(!r23.ran);
    assert_eq!(r23.deleted_event_count, 0);

    let r24 = prune_event_store(&conn, t_24h)?;
    assert!(r24.ran);
    assert!(r24.deleted_event_count >= 2);

    let v1_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_events WHERE application_event_version = 1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v1_count, 1);

    let created_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_events WHERE event_type = 'thread.created'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(created_count, 2);

    let active_run_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_events
         WHERE stream_id = 'thread-active'
           AND COALESCE(json_extract(payload_json, '$.status'), json_extract(payload_json, '$.run.status')) = 'running'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(active_run_count, 1);

    let stream_head_present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_events WHERE stream_id = 'thread-prune'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(stream_head_present, 2);
    Ok(())
}
