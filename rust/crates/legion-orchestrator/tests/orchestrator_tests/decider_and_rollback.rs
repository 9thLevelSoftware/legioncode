//! Tests for `BUG-02` durable rollback ordering and `message.dispatch` / `run.interrupt` / `thread.stop`.

use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, ThreadId};
use legion_contracts::orchestration::{
    DispatchMode, DomainEventPayload, OrchestrationCommand, RunInterruptCommand, RunStatus,
    ThreadStopCommand,
};
use legion_orchestrator::outbox::{OutboxEffect, OutboxWorkerOptions, ROLLBACK_PENDING_INTENT};
use legion_orchestrator::sink::CommitCommandInput;
use legion_orchestrator::sink::projection_apply::load_decider_state;
use legion_orchestrator::{DeciderError, Orchestrator, OrchestratorError};
use rusqlite::params;

use super::helpers::{
    TestResult, create_thread_cmd, make_checkpoint_captured_event, new_test_orchestrator,
    rollback_cmd, send_message_cmd,
};

type RollbackLog = Arc<Mutex<Vec<(String, Option<String>, usize)>>>;

fn spawn_rollback_verifier_worker(
    orch: &Orchestrator,
) -> (legion_orchestrator::outbox::EffectOutboxWorker, RollbackLog) {
    let db = Arc::clone(orch.sink().db());
    let seen: RollbackLog = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let worker = orch.spawn_outbox_worker(
        move |effect: OutboxEffect| {
            let db = Arc::clone(&db);
            let log = Arc::clone(&log);
            async move {
                let conn = db.lock().map_err(|e| e.to_string())?;
                let intent = effect
                    .raw_payload
                    .get("intent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let req_id: Option<String> = conn
                    .query_row(
                        "SELECT json_extract(payload_json, '$.rollbackRequestId')
                         FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
                        params![effect.thread_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let ev_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM orchestration_events
                         WHERE stream_id = ?1 AND event_type = 'checkpoint.rollback-requested'",
                        params![effect.thread_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let outbox_status: String = conn
                    .query_row(
                        "SELECT status FROM orchestration_v2_effect_outbox WHERE effect_id = ?1",
                        params![effect.id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(outbox_status, "running");
                log.lock().map_err(|e| e.to_string())?.push((
                    intent,
                    req_id,
                    usize::try_from(ev_count).unwrap_or(0),
                ));
                Ok::<(), String>(())
            }
        },
        OutboxWorkerOptions::default(),
    );
    (worker, seen)
}

#[tokio::test]
async fn bug_02_checkpoint_rollback_persists_pending_intent_before_git_restore() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-20T00:00:00.000Z";
    let t1 = "2026-11-20T00:01:00.000Z";
    let tid = ThreadId::new("thread-rb")?;
    let db = Arc::clone(orch.sink().db());

    orch.dispatch_command(
        &create_thread_cmd("cmd-c1", "thread-rb", "proj-1", "Rollback test")?,
        t0,
    )
    .await?;
    let cp_ev = make_checkpoint_captured_event("ev-cp-1", "thread-rb", "scope-1", "cp-1", t0)?;
    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("cmd-cap-1")?,
        command_type: "checkpoint.capture".to_owned(),
        thread_id: tid.clone(),
        actor_kind: "server".to_owned(),
        causation_event_id: None,
        correlation_id: None,
        now_iso: t0.to_owned(),
        events: vec![cp_ev],
        outbox_effects: Vec::new(),
    })?;

    let (worker, seen_pending_intent) = spawn_rollback_verifier_worker(&orch);
    let rb = rollback_cmd("cmd-rb-1", "thread-rb", "scope-1", "cp-1", true)?;
    let dispatch = orch.dispatch_command(&rb, t1).await?;
    let intent = dispatch
        .rollback_intent
        .ok_or("expected rollback_intent on checkpoint.rollback")?;
    assert_eq!(intent.status, ROLLBACK_PENDING_INTENT);

    let stats = worker.drain().await?;
    assert_eq!(stats.completed, 1);
    {
        let recorded = seen_pending_intent.lock().map_err(|e| e.to_string())?;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, ROLLBACK_PENDING_INTENT);
        assert_eq!(recorded[0].1.as_deref(), Some("cmd-rb-1"));
        assert_eq!(recorded[0].2, 1);
    }

    let rb2 = rollback_cmd("cmd-rb-2", "thread-rb", "scope-1", "cp-1", true)?;
    let conflict = orch.dispatch_command(&rb2, t1).await;
    assert!(matches!(
        conflict,
        Err(OrchestratorError::Decider(DeciderError::Conflict(_)))
    ));

    let bad_cp = rollback_cmd("cmd-rb-3", "thread-rb", "scope-1", "cp-missing", false)?;
    let decider_state = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        load_decider_state(&conn, &tid)?
    };
    let missing_err = legion_orchestrator::decide_command(&decider_state, &bad_cp, t1);
    assert!(matches!(
        missing_err,
        Err(DeciderError::Conflict(_) | DeciderError::CheckpointNotFound { .. })
    ));

    worker.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn message_dispatch_runs_immediately_then_queues_and_supports_interrupt_and_stop()
-> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-20T00:00:00.000Z";
    let tid = ThreadId::new("thread-msg")?;

    orch.dispatch_command(
        &create_thread_cmd("c0", "thread-msg", "proj-1", "Messaging")?,
        t0,
    )
    .await?;

    let m1 = send_message_cmd(
        "cmd-m1",
        "thread-msg",
        "msg-1",
        "Hello",
        DispatchMode::StartImmediately,
    )?;
    let r1 = orch.dispatch_command(&m1, t0).await?;
    let run1 = r1
        .outcome
        .events
        .iter()
        .find_map(|e| match &e.event.payload {
            DomainEventPayload::RunUpdated { run, .. } => Some((**run).clone()),
            _ => None,
        })
        .ok_or("expected run1")?;
    assert_eq!(run1.status, RunStatus::Running);
    assert_eq!(run1.ordinal, 1);

    let m2 = send_message_cmd(
        "cmd-m2",
        "thread-msg",
        "msg-2",
        "Follow-up",
        DispatchMode::StartImmediately,
    )?;
    let r2 = orch.dispatch_command(&m2, t0).await?;
    let run2_id = r2
        .outcome
        .events
        .iter()
        .find_map(|e| match &e.event.payload {
            DomainEventPayload::RunUpdated { run, .. } => {
                assert_eq!(run.status, RunStatus::Queued);
                assert_eq!(run.ordinal, 2);
                Some(run.id.clone())
            }
            _ => None,
        })
        .ok_or("expected run2")?;

    let int_cmd = OrchestrationCommand::RunInterrupt(RunInterruptCommand {
        command_id: CommandId::new("cmd-int-1")?,
        thread_id: tid.clone(),
        run_id: run1.id,
        reason: Some("user cancelled".to_owned()),
        hold_queue: None,
    });
    let int_res = orch.dispatch_command(&int_cmd, t0).await?;
    assert!(matches!(
        &int_res.outcome.events[0].event.payload,
        DomainEventPayload::RunUpdated { run, .. } if run.status == RunStatus::Interrupted
    ));

    let stop_cmd = OrchestrationCommand::ThreadStop(ThreadStopCommand {
        command_id: CommandId::new("cmd-stop")?,
        thread_id: tid,
        reason: Some("stop all".to_owned()),
        stop_delegated_tasks: Some(true),
    });
    let stop_res = orch.dispatch_command(&stop_cmd, t0).await?;
    assert!(stop_res.outcome.events.iter().any(|e| matches!(
        &e.event.payload,
        DomainEventPayload::RunUpdated { run, .. }
            if run.id == run2_id && run.status == RunStatus::Cancelled
    )));

    let m3 = send_message_cmd(
        "cmd-m3",
        "thread-msg",
        "msg-3",
        "Fresh run",
        DispatchMode::StartImmediately,
    )?;
    let r3 = orch.dispatch_command(&m3, t0).await?;
    assert!(r3.outcome.events.iter().any(|e| matches!(
        &e.event.payload,
        DomainEventPayload::RunUpdated { run, .. }
            if run.status == RunStatus::Running && run.ordinal == 3
    )));
    Ok(())
}
