use std::error::Error;

use legion_contracts::ids::{
    CommandId, EventId, MessageId, ProjectId, ProviderInstanceId, RunId, ThreadId,
};
use legion_contracts::orchestration::{
    CreationSource, DispatchMode, DomainEvent, DomainEventPayload, InitialThreadMessage,
    MessageDispatchCommand, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, QueuedRunEditCommand, RunStatus, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::decider::queue_ops::is_slash_command;
use legion_orchestrator::decider::{DeciderError, decide_command};
use legion_orchestrator::sink::CommitCommandInput;
use legion_orchestrator::sink::projection_apply::DeciderThreadState;
use serde_json::json;

use super::helpers::{
    TestResult, create_thread_cmd, make_run_completed_event, new_test_orchestrator,
    send_message_cmd,
};

fn agent_dispatch(
    cmd: &str,
    thread: &str,
    msg: &str,
    text: &str,
    mode: DispatchMode,
) -> Result<OrchestrationCommand, Box<dyn Error>> {
    let base = send_message_cmd(cmd, thread, msg, text, mode)?;
    Ok(match base {
        OrchestrationCommand::MessageDispatch(m) => {
            OrchestrationCommand::MessageDispatch(MessageDispatchCommand {
                created_by: OrchestrationActor::Agent,
                delivery_intent: Some("auto".to_owned()),
                ..m
            })
        }
        other => other,
    })
}

#[test]
fn slash_command_detection_excludes_paths_and_attachments() {
    assert!(is_slash_command("/compact", &[]));
    assert!(is_slash_command("/plugin:review-pr 12", &[]));
    assert!(!is_slash_command("/home/me/index.ts", &[]));
    assert!(!is_slash_command("/ spaced", &[]));
    assert!(!is_slash_command("/compact", &[json!({ "type": "image" })]));
}

#[tokio::test]
async fn agent_fifo_behind_compact_slash_enqueue_and_model_inheritance() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-21T00:00:00.000Z";
    orch.dispatch_command(&create_thread_cmd("c-q", "t-q", "p-1", "Q")?, t0)
        .await?;
    orch.dispatch_command(
        &send_message_cmd(
            "m-1",
            "t-q",
            "msg-1",
            "Running turn",
            DispatchMode::StartImmediately,
        )?,
        t0,
    )
    .await?;

    let compact_steer = send_message_cmd(
        "m-compact",
        "t-q",
        "msg-compact",
        "/compact",
        DispatchMode::SteerActive {
            target_run_id: RunId::new("m-1-run")?,
        },
    )?;
    let compact_res = orch.dispatch_command(&compact_steer, t0).await?;
    assert!(compact_res.outcome.events.iter().any(|ev| matches!(
        &ev.event.payload,
        DomainEventPayload::RunUpdated { run, .. } if run.status == RunStatus::Queued
    )));

    orch.dispatch_command(
        &agent_dispatch(
            "m-agent-1",
            "t-q",
            "msg-agent-1",
            "first",
            DispatchMode::QueueAfterActive,
        )?,
        t0,
    )
    .await?;

    let agent2 = agent_dispatch(
        "m-agent-2",
        "t-q",
        "msg-agent-2",
        "second",
        DispatchMode::SteerActive {
            target_run_id: RunId::new("m-1-run")?,
        },
    )?;
    let agent2_res = orch.dispatch_command(&agent2, t0).await?;
    assert!(agent2_res.outcome.events.iter().any(|ev| matches!(
        &ev.event.payload,
        DomainEventPayload::RunUpdated { run, .. }
            if run.id.as_str() == "m-agent-2-run" && run.status == RunStatus::Queued
    )));

    orch.dispatch_command(
        &OrchestrationCommand::QueuedMessagePromoteToSteer {
            command_id: CommandId::new("promote-compact")?,
            thread_id: ThreadId::new("t-q")?,
            queued_run_id: RunId::new("m-compact-run")?,
            target_run_id: RunId::new("m-1-run")?,
        },
        t0,
    )
    .await?;

    orch.dispatch_command(
        &OrchestrationCommand::QueuedRunEdit(QueuedRunEditCommand {
            context: None,
            command_id: CommandId::new("edit-q1")?,
            thread_id: ThreadId::new("t-q")?,
            run_id: RunId::new("m-agent-1-run")?,
            text: "/review".to_owned(),
            attachments: None,
        }),
        t0,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn queued_message_with_no_model_inherits_live_thread_selection_on_pop() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-21T00:00:00.000Z";
    orch.dispatch_command(&create_thread_cmd("c-q", "t-q", "p-1", "Q")?, t0)
        .await?;
    orch.dispatch_command(
        &send_message_cmd(
            "m-1",
            "t-q",
            "msg-1",
            "Running turn",
            DispatchMode::StartImmediately,
        )?,
        t0,
    )
    .await?;
    orch.dispatch_command(
        &agent_dispatch(
            "m-agent-1",
            "t-q",
            "msg-agent-1",
            "follow",
            DispatchMode::QueueAfterActive,
        )?,
        t0,
    )
    .await?;

    let mut state = orch.load_thread_state(&ThreadId::new("t-q")?)?;
    if let Some(ref mut th) = state.thread {
        th.model_selection = ModelSelection {
            instance_id: ProviderInstanceId::new("claude")?,
            model: "claude-sonnet-4".to_owned(),
            options: None,
        };
    }
    let m1 = state.runs.first().ok_or("missing m-1")?.clone();
    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("end-m1")?,
        command_type: "run.complete".to_owned(),
        actor_kind: "runtime".to_owned(),
        thread_id: ThreadId::new("t-q")?,
        causation_event_id: None,
        correlation_id: None,
        events: vec![
            make_run_completed_event("ev-end-m1", m1, t0)?,
            DomainEvent {
                id: EventId::new("ev-model-switch")?,
                thread_id: ThreadId::new("t-q")?,
                run_id: None,
                node_id: None,
                driver: None,
                provider_instance_id: None,
                raw_event_id: None,
                occurred_at: t0.to_owned(),
                payload: DomainEventPayload::ThreadState {
                    event_type: "thread.model-selection-updated".to_owned(),
                    thread: Box::new(state.thread.clone().ok_or("missing thread")?),
                },
            },
        ],
        outbox_effects: Vec::new(),
        now_iso: t0.to_owned(),
    })?;

    orch.pop_next_queued_run(&CommandId::new("pop-1")?, &ThreadId::new("t-q")?, t0)
        .await?;
    let st_after = orch.load_thread_state(&ThreadId::new("t-q")?)?;
    let popped = st_after
        .runs
        .iter()
        .find(|r| r.id.as_str() == "m-agent-1-run")
        .ok_or("missing agent run")?;
    assert_eq!(popped.status, RunStatus::Running);
    assert_eq!(popped.model_selection.model, "claude-sonnet-4");
    assert_eq!(
        st_after
            .messages_by_id
            .get(&MessageId::new("msg-agent-1")?)
            .and_then(|m| m.follows_thread_selection),
        Some(true)
    );
    Ok(())
}

#[test]
fn thread_create_generate_title_persists_intent_and_rejects_empty_seed() -> TestResult {
    let base_cmd = ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new("cmd-gen-bad")?,
        thread_id: ThreadId::new("t-gen-bad")?,
        project_id: ProjectId::new("p-1")?,
        title: "Draft".to_owned(),
        generate_title: Some(true),
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new("m-empty")?,
            text: "   \n ".to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            output_schema: None,
            workspace_strategy: None,
        }),
    };
    assert!(matches!(
        decide_command(
            &DeciderThreadState::default(),
            &OrchestrationCommand::ThreadCreate(base_cmd.clone()),
            "2026-11-21T00:00:00.000Z",
        ),
        Err(DeciderError::InvalidRequest(_))
    ));

    let ok_cmd = ThreadCreateCommand {
        command_id: CommandId::new("cmd-gen-ok")?,
        thread_id: ThreadId::new("t-gen-ok")?,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new("m-real")?,
            text: "Fix bug".to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: Some("Draft".to_owned()),
            scheduled_task_id: None,
            sender_thread_id: None,
            output_schema: None,
            workspace_strategy: None,
        }),
        ..base_cmd
    };
    let dec = decide_command(
        &DeciderThreadState::default(),
        &OrchestrationCommand::ThreadCreate(ok_cmd),
        "2026-11-21T00:00:00.000Z",
    )?;
    assert_eq!(dec.outbox_effects.len(), 2);
    assert!(
        dec.resulting_thread
            .as_ref()
            .and_then(|t| t.title_regeneration.as_ref())
            .and_then(|o| o.as_ref())
            .is_some()
    );
    Ok(())
}
