use std::error::Error;

use legion_contracts::ids::{
    CommandId, EventId, MessageId, NodeId, ProjectId, ProviderInstanceId, ProviderThreadId, RunId,
    ThreadId, TurnItemId,
};
use legion_contracts::orchestration::{
    ExecutionNode, ModelSelection, OrchestrationCommand, Run, RunStatus, TurnItem, TurnItemBase,
    TurnItemKind, TurnItemStatus,
};
use legion_orchestrator::decider::decide_command;
use legion_orchestrator::scheduled_tasks::{
    ScheduledTaskBindingInput, apply_scheduled_task_thread_unbind,
    resolve_scheduled_task_thread_binding,
};
use legion_orchestrator::session_recovery::{
    build_session_release_termination_events, plan_unexpected_exit_background_work_recovery,
};
use legion_orchestrator::sink::commit_batch::{
    BatchStopReason, ProviderSnapshotScheduler, ProviderStreamPriority, QueuedSnapshotJob,
    STARVATION_PROMOTION_MS, coalesce_stream_snapshots,
};
use legion_orchestrator::sink::projection_apply::DeciderThreadState;

fn build_running_tool_fixture(
    tid: &ThreadId,
    pt: &ProviderThreadId,
) -> Result<(TurnItem, ExecutionNode, Run), Box<dyn Error>> {
    let tool = TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: TurnItemId::new("tool-cut")?,
            thread_id: tid.clone(),
            run_id: Some(RunId::new("run-1")?),
            node_id: Some(NodeId::new("node-cut")?),
            provider_thread_id: Some(pt.clone()),
            provider_turn_id: None,
            native_item_ref: None,
            parent_item_id: None,
            ordinal: 1,
            status: TurnItemStatus::Running,
            title: None,
            started_at: Some("2026-11-21T00:00:00.000Z".to_owned()),
            completed_at: None,
            updated_at: "2026-11-21T00:00:00.000Z".to_owned(),
        },
        kind: TurnItemKind::CommandExecution {
            input: "cargo test".to_owned(),
            output: None,
            output_omitted: None,
            output_indicates_failure: None,
            exit_code: None,
            monitor: None,
            completion_wake: None,
        },
    };
    let node = ExecutionNode {
        id: NodeId::new("node-cut")?,
        thread_id: tid.clone(),
        run_id: Some(RunId::new("run-1")?),
        parent_node_id: None,
        root_node_id: NodeId::new("root-1")?,
        kind: "tool_call".to_owned(),
        status: "running".to_owned(),
        counts_for_run: true,
        provider_thread_id: Some(pt.clone()),
        provider_turn_id: None,
        native_item_ref: None,
        runtime_request_id: None,
        checkpoint_scope_id: None,
        started_at: Some("2026-11-21T00:00:00.000Z".to_owned()),
        completed_at: None,
    };
    let run = Run {
        id: RunId::new("run-1")?,
        thread_id: tid.clone(),
        ordinal: 1,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
        provider_thread_id: Some(pt.clone()),
        user_message_id: MessageId::new("msg-1")?,
        root_node_id: None,
        active_attempt_id: None,
        status: RunStatus::Completed,
        queue_position: None,
        queue_held: None,
        steer_when_ready_run_id: None,
        requested_at: "2026-11-21T00:00:00.000Z".to_owned(),
        started_at: Some("2026-11-21T00:00:00.000Z".to_owned()),
        completed_at: None,
        checkpoint_id: None,
        context_handoff_id: None,
        restart_continuation_of_run_id: None,
        manual_continuation_of_run_id: None,
        usage_limit_continuation_of_run_id: None,
        delegated_task_id: None,
        work_started_at: None,
        restart_cancelled_background_work: None,
        source_plan_ref: None,
        delegated_completion: None,
        workspace_preparation: None,
        fresh_context: None,
        output_schema: None,
        output: None,
    };
    Ok((tool, node, run))
}

#[test]
fn session_release_and_unexpected_exit_recovery() -> Result<(), Box<dyn Error>> {
    let tid = ThreadId::new("t-rel")?;
    let pt = ProviderThreadId::new("pt-1")?;
    let (tool, node, run) = build_running_tool_fixture(&tid, &pt)?;
    let mut ev_seq = 0_u64;
    let mut alloc = || {
        ev_seq += 1;
        EventId::new(format!("ev-rel-{ev_seq}")).unwrap_or_else(|_| unreachable!())
    };
    let term_events = build_session_release_termination_events(
        &tid,
        std::slice::from_ref(&pt),
        std::slice::from_ref(&tool),
        &[node],
        "2026-11-21T00:05:00.000Z",
        &mut alloc,
    );
    assert_eq!(term_events.len(), 2);

    let (detach_evs, detach_cont) = plan_unexpected_exit_background_work_recovery(
        &tid,
        std::slice::from_ref(&pt),
        std::slice::from_ref(&run),
        std::slice::from_ref(&tool),
        false,
        "2026-11-21T00:05:00.000Z",
        &mut alloc,
    );
    assert!(detach_evs.is_empty() && detach_cont.is_empty());

    let (exit_evs, exit_cont) = plan_unexpected_exit_background_work_recovery(
        &tid,
        &[pt],
        &[run],
        &[tool],
        true,
        "2026-11-21T00:05:00.000Z",
        &mut alloc,
    );
    assert_eq!(exit_evs.len(), 1);
    assert_eq!(exit_cont.len(), 1);
    Ok(())
}

#[test]
fn scheduled_task_unbind_and_subprocess_auto_settle_guard() -> Result<(), Box<dyn Error>> {
    let existing = ThreadId::new("t-old")?;
    let proj = ProjectId::new("p-1")?;
    let cleared = resolve_scheduled_task_thread_binding(&ScheduledTaskBindingInput {
        existing_thread_id: Some(&existing),
        existing_project_id: Some(&proj),
        target_project_id: &proj,
        requested_thread_id: None,
        unbind_thread: true,
        thread_mode: "continue",
    });
    assert_eq!(cleared.resolved_thread_id, None);
    assert_eq!(
        apply_scheduled_task_thread_unbind(Some(&existing), &cleared),
        None
    );

    let create_dec = decide_command(
        &DeciderThreadState::default(),
        &super::helpers::create_thread_cmd("c-as", "t-as", "p-1", "AS")?,
        "2026-11-21T00:00:00.000Z",
    )?;
    let state_busy = DeciderThreadState {
        thread: create_dec.resulting_thread,
        has_active_terminal_subprocess: true,
        ..DeciderThreadState::default()
    };
    let auto_dec = decide_command(
        &state_busy,
        &OrchestrationCommand::ThreadAutoSettle {
            command_id: CommandId::new("c-as-run")?,
            thread_id: ThreadId::new("t-as")?,
            snapshot_at: "2026-11-21T00:00:00.000Z".to_owned(),
            settled_at: None,
        },
        "2026-11-21T00:10:00.000Z",
    )?;
    assert_eq!(auto_dec.events.len(), 0);
    Ok(())
}

#[test]
fn commit_batch_scheduler_ranks_focus_promotes_starved_and_coalesces() -> Result<(), Box<dyn Error>>
{
    let t_focused = ThreadId::new("focused")?;
    let t_main = ThreadId::new("main")?;
    let t_sub = ThreadId::new("subagent")?;
    let t_finish = ThreadId::new("finishing")?;

    let mut sched: ProviderSnapshotScheduler<&str> = ProviderSnapshotScheduler::new();
    for (id, thread, delegated, finishing) in [
        (1, &t_sub, true, false),
        (2, &t_main, false, false),
        (3, &t_finish, false, true),
        (4, &t_focused, false, false),
    ] {
        sched.register_stream(
            id,
            Some(ProviderStreamPriority {
                thread_id: thread.clone(),
                delegated,
            }),
        );
        sched.mark_finishing(id, finishing);
        sched.enqueue(
            id,
            QueuedSnapshotJob {
                job_id: id * 10,
                entity_key: format!("message.updated:{thread}"),
                thread_id: thread.clone(),
                queued_at_ms: 1_000,
                bytes: 32,
                payload: "snap",
            },
        );
    }

    let batch = sched
        .next_batch(2_000, |tid| *tid == t_focused)
        .ok_or("expected batch")?;
    let order: Vec<&str> = batch
        .units
        .iter()
        .map(|u| u.job.thread_id.as_str())
        .collect();
    assert_eq!(order, vec!["focused", "finishing", "main", "subagent"]);
    assert_eq!(batch.reason, BatchStopReason::Ready);

    let mut sched2: ProviderSnapshotScheduler<&str> = ProviderSnapshotScheduler::new();
    sched2.register_stream(
        1,
        Some(ProviderStreamPriority {
            thread_id: t_sub.clone(),
            delegated: true,
        }),
    );
    sched2.register_stream(
        2,
        Some(ProviderStreamPriority {
            thread_id: t_main.clone(),
            delegated: false,
        }),
    );
    sched2.enqueue(
        1,
        QueuedSnapshotJob {
            job_id: 1,
            entity_key: "node:s".to_owned(),
            thread_id: t_sub.clone(),
            queued_at_ms: 0,
            bytes: 16,
            payload: "sub",
        },
    );
    sched2.enqueue(
        2,
        QueuedSnapshotJob {
            job_id: 2,
            entity_key: "node:m".to_owned(),
            thread_id: t_main.clone(),
            queued_at_ms: STARVATION_PROMOTION_MS,
            bytes: 16,
            payload: "main",
        },
    );
    let batch2 = sched2
        .next_batch(STARVATION_PROMOTION_MS, |tid| *tid == t_focused)
        .ok_or("expected batch2")?;
    assert_eq!(batch2.units[0].job.thread_id, t_sub);

    let jobs: Vec<QueuedSnapshotJob<usize>> = ["a", "b", "a", "b", "a", "b"]
        .into_iter()
        .enumerate()
        .map(|(i, k)| QueuedSnapshotJob {
            job_id: i as u64,
            entity_key: k.to_owned(),
            thread_id: t_main.clone(),
            queued_at_ms: 0,
            bytes: 8,
            payload: i,
        })
        .collect();
    let kept: Vec<usize> = coalesce_stream_snapshots(jobs)
        .into_iter()
        .map(|u| u.job.payload)
        .collect();
    assert_eq!(kept, vec![0, 4, 5]);
    Ok(())
}
