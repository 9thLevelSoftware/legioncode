use std::collections::HashSet;
use std::error::Error;

use legion_contracts::ids::{
    NodeId, ProviderDriverKind, ProviderInstanceId, ProviderThreadId, ProviderTurnId, RunAttemptId,
    RunId, ThreadId, TurnItemId,
};
use legion_contracts::orchestration::{
    OrchestrationActor, ProviderTurn, RunAttempt, RunStatus, Subagent, TurnItem, TurnItemBase,
    TurnItemKind, TurnItemStatus,
};
use legion_orchestrator::subagent_delegation::{
    DelegatedTaskProgressState, delegated_task_progress, format_task_not_found,
    is_child_interrupted_by_parent_stop, parent_run_ids_for_child, parent_task_sync_retry_delay_ms,
    pending_stopped_delegated_tasks, provider_turn_for_run, should_wait_for_cancelled_task,
    stopped_delegated_tasks_note,
};
use serde_json::json;

use super::helpers::TestResult;
use super::wave3_helpers::{make_run, make_user_message};

fn make_subagent(
    id: &str,
    tid: &str,
    run_id: Option<&str>,
    title: Option<&str>,
) -> Result<Subagent, Box<dyn Error>> {
    Ok(Subagent {
        id: NodeId::new(id)?,
        thread_id: ThreadId::new(tid)?,
        run_id: run_id.map(RunId::new).transpose()?,
        parent_node_id: NodeId::new("root-1")?,
        origin: "app_owned".to_owned(),
        created_by: OrchestrationActor::Agent,
        driver: ProviderDriverKind::new("codex")?,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        provider_thread_id: None,
        child_thread_id: Some(ThreadId::new(format!("child-{id}"))?),
        remote_child: None,
        native_task_ref: None,
        prompt: format!("Work {id}"),
        title: title.map(str::to_owned),
        model: None,
        model_selection: None,
        executed_model: None,
        completion_wake: None,
        completion_delivery: None,
        cancelled_run_id: None,
        status: "running".to_owned(),
        progress: None,
        workflow: None,
        result: None,
        started_at: Some("2026-11-22T10:00:00.000Z".to_owned()),
        completed_at: None,
        updated_at: "2026-11-22T10:00:00.000Z".to_owned(),
        prompt_omitted: None,
        result_omitted: None,
    })
}

#[test]
fn cancelled_child_with_held_user_message_stops_and_parent_notes_interruption() -> TestResult {
    let mut done_run = make_run("r-1", "child-1", 1, RunStatus::Completed)?;
    done_run.started_at = Some("2026-11-22T10:00:00.000Z".to_owned());
    let mut held_user_run = make_run("r-2", "child-1", 2, RunStatus::Queued)?;
    held_user_run.queue_held = Some(true);
    let held_user_msg = make_user_message("m-2", "child-1", "Manual follow-up", Some("r-2"))?;

    let child_runs = [done_run, held_user_run];
    let child_msgs = [held_user_msg];
    let progress = delegated_task_progress(&child_runs, &child_msgs, &[], &[]);
    assert_eq!(progress.state, DelegatedTaskProgressState::Working);
    assert!(progress.only_awaits_user_run);
    assert!(!should_wait_for_cancelled_task(&progress));

    let pt_id = ProviderThreadId::new("pt-parent")?;
    let mut parent_run = make_run("r-parent", "parent-1", 1, RunStatus::Interrupted)?;
    parent_run.provider_thread_id = Some(pt_id.clone());
    parent_run.delegated_completion = Some(json!({ "disposition": "stopped" }));
    let mut interrupted_task = make_subagent(
        "task-stopped",
        "parent-1",
        Some("r-parent"),
        Some("Audit tests"),
    )?;
    interrupted_task.status = "interrupted".to_owned();
    interrupted_task.completion_delivery = Some(json!({ "state": "disposed" }));
    interrupted_task.completed_at = Some("2026-11-22T10:01:00.000Z".to_owned());

    let parent_runs = [parent_run];
    let stopped_subs = [interrupted_task];
    let pending_stopped = pending_stopped_delegated_tasks(
        Some(&pt_id),
        &parent_runs,
        &[],
        &[],
        &stopped_subs,
        &HashSet::new(),
    );
    assert_eq!(pending_stopped.len(), 1);
    let note = stopped_delegated_tasks_note(&pending_stopped);
    assert!(note.contains("Audit tests (task task-stopped)"));

    let interrupt_item = TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: TurnItemId::new("ti-int")?,
            thread_id: ThreadId::new("child-1")?,
            run_id: None,
            node_id: None,
            provider_thread_id: None,
            provider_turn_id: Some(ProviderTurnId::new("turn-1")?),
            native_item_ref: None,
            parent_item_id: None,
            ordinal: 1,
            status: TurnItemStatus::Completed,
            title: None,
            started_at: Some("2026-11-22T10:00:00.000Z".to_owned()),
            completed_at: None,
            updated_at: "2026-11-22T10:00:00.000Z".to_owned(),
        },
        kind: TurnItemKind::RunInterruptRequest {
            message: "Interrupted by parent".to_owned(),
            unresponsive_at: None,
        },
    };
    assert!(is_child_interrupted_by_parent_stop(
        Some(&json!({ "state": "disposed" })),
        &[interrupt_item],
    ));
    Ok(())
}

#[test]
fn task_not_found_backoff_retry_and_linear_turn_lookup() -> TestResult {
    let mut open_tasks = Vec::new();
    for idx in 1..=12 {
        open_tasks.push(make_subagent(
            &format!("t-{idx}"),
            "parent-1",
            None,
            Some(&format!("Task {idx}")),
        )?);
    }
    let err_msg = format_task_not_found(
        &ThreadId::new("parent-1")?,
        &NodeId::new("missing")?,
        &open_tasks,
    );
    assert!(err_msg.contains("t-1 (Task 1)"));
    assert!(err_msg.contains("and 2 more"));

    assert_eq!(parent_task_sync_retry_delay_ms(0, 0), Some(1_000));
    assert_eq!(parent_task_sync_retry_delay_ms(5, 10_000), Some(30_000));
    assert_eq!(parent_task_sync_retry_delay_ms(1, 3_600_000), None);
    let ids = parent_run_ids_for_child(&[open_tasks[0].clone()], &ThreadId::new("child-t-1")?);
    assert_eq!(ids.len(), 0);
    let with_run = make_subagent("t-run", "parent-1", Some("r-p"), None)?;
    assert_eq!(
        parent_run_ids_for_child(&[with_run], &ThreadId::new("child-t-run")?).len(),
        1
    );

    let pt_id = ProviderThreadId::new("pt-parent")?;
    let att_id = RunAttemptId::new("att-9")?;
    let pt_turn_id = ProviderTurnId::new("pturn-9")?;
    let mut run_for_turn = make_run("r-turn", "t-1", 1, RunStatus::Completed)?;
    run_for_turn.active_attempt_id = Some(att_id.clone());
    let attempt = RunAttempt {
        id: att_id,
        native_thread_id: None,
        run_id: run_for_turn.id.clone(),
        attempt_ordinal: 1,
        root_node_id: NodeId::new("root-1")?,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        provider_thread_id: pt_id.clone(),
        provider_turn_id: Some(pt_turn_id.clone()),
        reason: "initial".to_owned(),
        status: "completed".to_owned(),
        started_at: None,
        completed_at: None,
    };
    let turn = ProviderTurn {
        id: pt_turn_id,
        provider_thread_id: pt_id,
        node_id: NodeId::new("root-1")?,
        run_attempt_id: None,
        native_turn_ref: None,
        native_thread_ref: None,
        ordinal: 1,
        status: "completed".to_owned(),
        started_at: None,
        completed_at: None,
        token_usage: None,
        turn_token_usage: None,
        model: None,
    };
    assert_eq!(
        provider_turn_for_run(&[turn], &[attempt], &run_for_turn).map(|t| t.id.as_str()),
        Some("pturn-9")
    );
    Ok(())
}
