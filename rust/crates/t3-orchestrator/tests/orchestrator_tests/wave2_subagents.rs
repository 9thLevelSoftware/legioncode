use std::error::Error;

use serde_json::json;
use t3_contracts::ids::{
    MessageId, NodeId, ProviderDriverKind, ProviderInstanceId, ProviderThreadId, RunId, ThreadId,
    TurnItemId,
};
use t3_contracts::orchestration::{
    DomainEventPayload, ModelSelection, OrchestrationActor, Run, RunStatus, Subagent, TurnItem,
    TurnItemBase, TurnItemKind, TurnItemStatus,
};
use t3_orchestrator::subagent_lifecycle::{
    PROVIDER_STOP_REASON_SIGNAL, apply_subagent_progress_preserving_result,
    has_blocking_work_for_idle_compaction, pick_delegation_parent_run,
    resolve_delegated_task_result, should_track_delegated_task_on_parent,
};

fn base_run(thread_id: &str, run_id: &str, status: RunStatus) -> Result<Run, Box<dyn Error>> {
    Ok(Run {
        id: RunId::new(run_id)?,
        thread_id: ThreadId::new(thread_id)?,
        ordinal: 1,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
        provider_thread_id: Some(ProviderThreadId::new("pt-1")?),
        user_message_id: MessageId::new("msg-1")?,
        root_node_id: None,
        active_attempt_id: None,
        status,
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
    })
}

fn turn_item(
    id: &str,
    thread_id: &str,
    run_id: Option<&str>,
    status: TurnItemStatus,
    kind: TurnItemKind,
) -> Result<TurnItem, Box<dyn Error>> {
    Ok(TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: TurnItemId::new(id)?,
            thread_id: ThreadId::new(thread_id)?,
            run_id: run_id.map(RunId::new).transpose()?,
            node_id: None,
            provider_thread_id: None,
            provider_turn_id: None,
            native_item_ref: None,
            parent_item_id: None,
            ordinal: 1,
            status,
            title: None,
            started_at: Some("2026-11-21T00:00:00.000Z".to_owned()),
            completed_at: None,
            updated_at: "2026-11-21T00:00:00.000Z".to_owned(),
        },
        kind,
    })
}

#[test]
fn subagent_delegation_outlives_run_and_compact_ignores_parent_tracking()
-> Result<(), Box<dyn Error>> {
    let ev = super::helpers::make_subagent_thread_event(
        "ev-sub",
        "t-child",
        "t-parent",
        "t-parent",
        "p-1",
        "2026-11-21T00:00:00.000Z",
    )?;
    let app_thread = match ev.payload {
        DomainEventPayload::ThreadState { thread, .. } => *thread,
        _ => unreachable!(),
    };
    let settled = base_run("t-child", "r-settled", RunStatus::Completed)?;
    let pending = vec![json!({ "kind": "subagent", "providerInstanceId": "codex" })];
    assert_eq!(
        pick_delegation_parent_run(&app_thread, std::slice::from_ref(&settled), &pending)
            .map(|r| &r.id),
        Some(&settled.id)
    );
    assert!(!should_track_delegated_task_on_parent(
        &app_thread,
        "/compact",
        &[]
    ));
    assert!(should_track_delegated_task_on_parent(
        &app_thread,
        "Finish module",
        &[]
    ));

    let app_sub = turn_item(
        "item-sub",
        "t-parent",
        None,
        TurnItemStatus::Running,
        TurnItemKind::Subagent {
            subagent_id: NodeId::new("node-sub")?,
            origin: "app_owned".to_owned(),
            driver: ProviderDriverKind::new("codex")?,
            provider_instance_id: ProviderInstanceId::new("codex")?,
            child_thread_id: Some(ThreadId::new("t-child")?),
            remote_child: None,
            prompt: "work".to_owned(),
            progress: None,
            result: None,
            prompt_omitted: None,
            result_omitted: None,
        },
    )?;
    assert!(!has_blocking_work_for_idle_compaction(&[app_sub]));
    Ok(())
}

#[test]
fn delegated_task_preserves_hook_stop_reason_across_progress_updates() -> Result<(), Box<dyn Error>>
{
    let settled = base_run("t-child", "r-settled", RunStatus::Completed)?;
    let signal_item = turn_item(
        &format!("r-settled:{PROVIDER_STOP_REASON_SIGNAL}"),
        "t-child",
        Some("r-settled"),
        TurnItemStatus::Completed,
        TurnItemKind::SystemNotice {
            message: "Stopped by policy hook".to_owned(),
            level: None,
            mcp_sign_in: None,
        },
    )?;
    let final_result =
        resolve_delegated_task_result(&settled, Some("Draft ok"), None, None, &[signal_item]);
    assert!(final_result.contains("Draft ok"));
    assert!(final_result.contains("Stopped by policy hook"));

    let subagent = Subagent {
        id: NodeId::new("node-s")?,
        thread_id: ThreadId::new("t-parent")?,
        run_id: Some(settled.id),
        parent_node_id: NodeId::new("root-n")?,
        origin: "app_owned".to_owned(),
        created_by: OrchestrationActor::Agent,
        driver: ProviderDriverKind::new("codex")?,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        provider_thread_id: None,
        child_thread_id: None,
        remote_child: None,
        native_task_ref: None,
        prompt: "p".to_owned(),
        title: None,
        model: None,
        model_selection: None,
        executed_model: None,
        completion_wake: None,
        completion_delivery: None,
        cancelled_run_id: None,
        status: "completed".to_owned(),
        progress: Some("old progress".to_owned()),
        workflow: None,
        result: Some(final_result.clone()),
        started_at: None,
        completed_at: Some("2026-11-21T00:01:00.000Z".to_owned()),
        updated_at: "2026-11-21T00:01:00.000Z".to_owned(),
        prompt_omitted: None,
        result_omitted: None,
    };
    let after_progress = apply_subagent_progress_preserving_result(
        &subagent,
        Some("late progress".to_owned()),
        "2026-11-21T00:02:00.000Z",
    );
    assert_eq!(
        after_progress.result.as_deref(),
        Some(final_result.as_str())
    );
    Ok(())
}
