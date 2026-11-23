//! Property-based tests checking sequence monotonicity, projection parity, active runs, and idempotency.

use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, EventId, MessageId, ThreadId};
use legion_contracts::orchestration::{
    DispatchMode, DomainEvent, DomainEventPayload, OrchestrationCommand, PatchValue, RunStatus,
    ThreadPullRequestLinkCommand, ThreadStopCommand, ThreadUnsettleCommand, UserActionReason,
};
use legion_contracts::pull_requests::ThreadPullRequestLinkSource;
use legion_orchestrator::decide_command;
use legion_orchestrator::projections::{ShellSnapshotOptions, query_shell_snapshot};
use legion_orchestrator::sink::event_codec::load_thread_events;
use legion_orchestrator::sink::projection_apply::{
    DeciderThreadState, load_decider_state, load_thread_projection,
};
use legion_orchestrator::sink::{CommitCommandInput, EventPubSub, EventSink};
use legion_orchestrator::time::format_millis_iso;
use legion_persistence::sqlite::SqliteStore;
use proptest::prelude::*;

use super::helpers::{count_thread_events, create_thread_cmd, send_message_cmd};

#[derive(Debug, Clone)]
enum PropOp {
    Archive,
    Unarchive,
    Settle,
    Unsettle,
    Snooze,
    Unsnooze,
    Pin,
    Unpin,
    SendMessage,
    CompleteOldestActiveRun,
    StopThread,
    LinkPr,
    UnlinkPr,
    ReplayLastAccepted,
}

fn arb_op() -> impl Strategy<Value = PropOp> {
    prop_oneof![
        Just(PropOp::Archive),
        Just(PropOp::Unarchive),
        Just(PropOp::Settle),
        Just(PropOp::Unsettle),
        Just(PropOp::Snooze),
        Just(PropOp::Unsnooze),
        Just(PropOp::Pin),
        Just(PropOp::Unpin),
        Just(PropOp::SendMessage),
        Just(PropOp::CompleteOldestActiveRun),
        Just(PropOp::StopThread),
        Just(PropOp::LinkPr),
        Just(PropOp::UnlinkPr),
        Just(PropOp::ReplayLastAccepted),
    ]
}

fn build_cmd(
    op: &PropOp,
    cmd_id: &str,
    step: usize,
    now_ms: i64,
    tid: &ThreadId,
) -> Result<OrchestrationCommand, String> {
    let cid = CommandId::new(cmd_id).map_err(|e| e.to_string())?;
    Ok(match op {
        PropOp::Archive => OrchestrationCommand::ThreadArchive {
            command_id: cid,
            thread_id: tid.clone(),
        },
        PropOp::Unarchive => OrchestrationCommand::ThreadUnarchive {
            command_id: cid,
            thread_id: tid.clone(),
        },
        PropOp::Settle => OrchestrationCommand::ThreadSettle {
            command_id: cid,
            thread_id: tid.clone(),
            expected_latest_run_id: None,
            settled_at: None,
        },
        PropOp::Unsettle => OrchestrationCommand::ThreadUnsettle(ThreadUnsettleCommand {
            command_id: cid,
            thread_id: tid.clone(),
            reason: UserActionReason::User,
            restore_unsettled_at: PatchValue::default(),
        }),
        PropOp::Snooze => OrchestrationCommand::ThreadSnooze {
            command_id: cid,
            thread_id: tid.clone(),
            snoozed_until: format_millis_iso(now_ms + 3_600_000),
        },
        PropOp::Unsnooze => OrchestrationCommand::ThreadUnsnooze {
            command_id: cid,
            thread_id: tid.clone(),
            reason: UserActionReason::User,
        },
        PropOp::Pin => OrchestrationCommand::ThreadPin {
            command_id: cid,
            thread_id: tid.clone(),
            order_key: None,
        },
        PropOp::Unpin => OrchestrationCommand::ThreadUnpin {
            command_id: cid,
            thread_id: tid.clone(),
        },
        PropOp::SendMessage => send_message_cmd(
            cmd_id,
            tid.as_str(),
            &format!("msg-{step}"),
            &format!("Turn {step}"),
            DispatchMode::StartImmediately,
        )
        .map_err(|e| e.to_string())?,
        PropOp::StopThread => OrchestrationCommand::ThreadStop(ThreadStopCommand {
            command_id: cid,
            thread_id: tid.clone(),
            reason: None,
            stop_delegated_tasks: Some(true),
        }),
        PropOp::LinkPr => {
            OrchestrationCommand::ThreadPullRequestLink(ThreadPullRequestLinkCommand {
                command_id: cid,
                thread_id: tid.clone(),
                host: "github.com".to_owned(),
                repository: "acme/repo".to_owned(),
                number: 7,
                url: "https://github.com/acme/repo/pull/7".to_owned(),
                source: ThreadPullRequestLinkSource::Manual,
                repository_web_url: None,
            })
        }
        PropOp::UnlinkPr => OrchestrationCommand::ThreadPullRequestUnlink {
            command_id: cid,
            thread_id: tid.clone(),
            host: "github.com".to_owned(),
            repository: "acme/repo".to_owned(),
            number: 7,
        },
        PropOp::CompleteOldestActiveRun | PropOp::ReplayLastAccepted => {
            unreachable!("handled before build_cmd")
        }
    })
}

fn execute_prop_sequence(ops: &[PropOp]) -> Result<(), String> {
    let store = SqliteStore::open_in_memory().map_err(|e| e.to_string())?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let pubsub = EventPubSub::new();
    let mut sub = pubsub.subscribe();
    let sink = EventSink::new(Arc::clone(&db), pubsub, None);

    let tid = ThreadId::new("thread-prop").map_err(|e| e.to_string())?;
    let base_ms: i64 = 1_795_132_800_000;
    let create_cmd =
        create_thread_cmd("cmd-0", "thread-prop", "proj-1", "Prop").map_err(|e| e.to_string())?;
    let init_ts = format_millis_iso(base_ms);
    let init_dec = decide_command(&DeciderThreadState::default(), &create_cmd, &init_ts)
        .map_err(|e| e.to_string())?;
    sink.commit_command(&init_dec.into_commit_input("server", &init_ts))
        .map_err(|e| e.to_string())?;
    let _ = sub.drain_ready();

    let mut runner = PropRunner {
        db,
        sink,
        sub,
        tid,
        last_accepted_input: None,
    };
    for (idx, op) in ops.iter().enumerate() {
        let step = idx + 1;
        let now_ms = base_ms + i64::try_from(step).unwrap_or(1) * 1_000;
        let now_iso = format_millis_iso(now_ms);
        runner.step(op, step, now_ms, &now_iso)?;
        runner.verify_invariants(&now_iso)?;
    }
    let _ = MessageId::new("unused-check");
    Ok(())
}

struct PropRunner {
    db: Arc<Mutex<rusqlite::Connection>>,
    sink: EventSink,
    sub: legion_orchestrator::sink::EventSubscriber,
    tid: ThreadId,
    last_accepted_input: Option<CommitCommandInput>,
}

impl PropRunner {
    fn step(&mut self, op: &PropOp, step: usize, now_ms: i64, now_iso: &str) -> Result<(), String> {
        match op {
            PropOp::ReplayLastAccepted => self.replay_last(),
            PropOp::CompleteOldestActiveRun => self.complete_oldest_active_run(step, now_iso),
            _ => {
                let cmd = build_cmd(op, &format!("cmd-{step}"), step, now_ms, &self.tid)?;
                let state = {
                    let conn = self.db.lock().map_err(|e| e.to_string())?;
                    load_decider_state(&conn, &self.tid).map_err(|e| e.to_string())?
                };
                if let Ok(dec) = decide_command(&state, &cmd, now_iso) {
                    let input = dec.into_commit_input("server", now_iso);
                    self.sink
                        .commit_command(&input)
                        .map_err(|e| e.to_string())?;
                    let _ = self.sub.drain_ready();
                    self.last_accepted_input = Some(input);
                }
                Ok(())
            }
        }
    }

    fn replay_last(&mut self) -> Result<(), String> {
        let Some(ref input) = self.last_accepted_input else {
            return Ok(());
        };
        let pre_count = {
            let conn = self.db.lock().map_err(|e| e.to_string())?;
            count_thread_events(&conn, self.tid.as_str()).map_err(|e| e.to_string())?
        };
        let outcome = self.sink.commit_command(input).map_err(|e| e.to_string())?;
        if !outcome.replayed {
            return Err("replaying committed CommandId returned replayed = false".to_owned());
        }
        if !self.sub.drain_ready().is_empty() {
            return Err("replayed command emitted duplicate PubSub notifications".to_owned());
        }
        let post_count = {
            let conn = self.db.lock().map_err(|e| e.to_string())?;
            count_thread_events(&conn, self.tid.as_str()).map_err(|e| e.to_string())?
        };
        if pre_count != post_count {
            return Err("replayed command altered event row count".to_owned());
        }
        Ok(())
    }

    fn complete_oldest_active_run(&mut self, step: usize, now_iso: &str) -> Result<(), String> {
        let state = {
            let conn = self.db.lock().map_err(|e| e.to_string())?;
            load_decider_state(&conn, &self.tid).map_err(|e| e.to_string())?
        };
        let Some(mut run) = state
            .runs
            .into_iter()
            .find(|r| matches!(r.status, RunStatus::Running | RunStatus::Queued))
        else {
            return Ok(());
        };
        run.status = RunStatus::Completed;
        run.completed_at = Some(now_iso.to_owned());
        let ev = DomainEvent {
            id: EventId::new(format!("ev-comp-{step}")).map_err(|e| e.to_string())?,
            thread_id: self.tid.clone(),
            run_id: Some(run.id.clone()),
            node_id: None,
            driver: None,
            provider_instance_id: Some(run.provider_instance_id.clone()),
            raw_event_id: None,
            occurred_at: now_iso.to_owned(),
            payload: DomainEventPayload::RunUpdated {
                event_type: "run.updated".to_owned(),
                run: Box::new(run),
            },
        };
        let input = CommitCommandInput {
            command_id: CommandId::new(format!("cmd-{step}")).map_err(|e| e.to_string())?,
            command_type: "run.complete".to_owned(),
            thread_id: self.tid.clone(),
            actor_kind: "provider".to_owned(),
            causation_event_id: None,
            correlation_id: None,
            now_iso: now_iso.to_owned(),
            events: vec![ev],
            outbox_effects: Vec::new(),
        };
        self.sink
            .commit_command(&input)
            .map_err(|e| e.to_string())?;
        let _ = self.sub.drain_ready();
        self.last_accepted_input = Some(input);
        Ok(())
    }

    fn verify_invariants(&self, now_iso: &str) -> Result<(), String> {
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        let history = load_thread_events(&conn, &self.tid, 0).map_err(|e| e.to_string())?;
        for (idx, persisted) in history.iter().enumerate() {
            let expected_ver = i64::try_from(idx + 1).map_err(|e| e.to_string())?;
            if persisted.stream_version != expected_ver {
                return Err(format!(
                    "non-contiguous stream_version: expected {expected_ver}, got {}",
                    persisted.stream_version
                ));
            }
            if idx > 0 && persisted.sequence <= history[idx - 1].sequence {
                return Err("global sequence failed monotonic increase".to_owned());
            }
        }
        let latest_thread = history.iter().rev().find_map(|p| match &p.event.payload {
            DomainEventPayload::ThreadState { thread, .. } => Some((**thread).clone()),
            _ => None,
        });
        let projected = load_thread_projection(&conn, &self.tid).map_err(|e| e.to_string())?;
        if latest_thread != projected {
            return Err("thread projection diverged from latest ThreadState event".to_owned());
        }
        let state = load_decider_state(&conn, &self.tid).map_err(|e| e.to_string())?;
        let running_count = state
            .runs
            .iter()
            .filter(|r| r.status == RunStatus::Running)
            .count();
        if running_count > 1 {
            return Err(format!(
                "multiple simultaneous Running runs: {running_count}"
            ));
        }
        let _snap = query_shell_snapshot(&conn, now_iso, ShellSnapshotOptions::default())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn command_sequences_preserve_stream_projection_and_idempotency_invariants(
        ops in prop::collection::vec(arb_op(), 1..25)
    ) {
        let res = execute_prop_sequence(&ops);
        prop_assert!(res.is_ok(), "property invariant failed: {:?}", res.err());
    }
}
