//! Pure `thread.create` command decision helper including optional `initial_message` bootstrap.

use t3_contracts::ids::RunId;
use t3_contracts::orchestration::{
    AppThread, ConversationMessage, DomainEvent, DomainEventPayload, InitialThreadMessage,
    PatchValue, Run, RunStatus, ThreadCreateCommand, ThreadLineage,
};

use crate::decider::output_ops::validate_output_request_for_message;
use crate::decider::thread_ops::make_thread_event;
use crate::decider::types::{CommandDecision, DeciderError, derived_effect_id, derived_event_id};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn decide_thread_create(
    state: &DeciderThreadState,
    cmd: &ThreadCreateCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    if state.thread.is_some() {
        return Err(DeciderError::ThreadAlreadyExists(cmd.thread_id.clone()));
    }
    if cmd.generate_title == Some(true) {
        let has_content = cmd
            .initial_message
            .as_ref()
            .is_some_and(|m| !m.text.trim().is_empty() || !m.attachments.is_empty());
        if !has_content {
            return Err(DeciderError::InvalidRequest(
                "Title generation requires a message or attachments. No thread was created."
                    .to_owned(),
            ));
        }
    }
    if let Some(init) = &cmd.initial_message {
        validate_output_request_for_message(
            &init.text,
            &init.attachments,
            init.output_schema.as_ref(),
        )?;
    }
    let thread = build_new_thread(cmd, now_iso);
    let mut events = vec![make_thread_event(
        &cmd.command_id,
        0,
        &cmd.thread_id,
        now_iso,
        "thread.created",
        thread.clone(),
    )?];
    let mut outbox_effects = Vec::new();
    if let Some(init) = &cmd.initial_message {
        append_initial_message(
            cmd,
            &thread,
            init,
            now_iso,
            &mut events,
            &mut outbox_effects,
        )?;
    }

    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "thread.create".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events,
        outbox_effects,
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

fn build_new_thread(cmd: &ThreadCreateCommand, now_iso: &str) -> AppThread {
    let title = if cmd.title.trim().is_empty() {
        "New thread".to_owned()
    } else {
        cmd.title.clone()
    };
    AppThread {
        created_by: cmd.created_by,
        creation_source: cmd.creation_source,
        id: cmd.thread_id.clone(),
        project_id: cmd.project_id.clone(),
        title,
        provider_instance_id: cmd.model_selection.instance_id.clone(),
        model_selection: cmd.model_selection.clone(),
        runtime_mode: cmd.runtime_mode,
        interaction_mode: cmd.interaction_mode,
        branch: cmd.branch.clone(),
        worktree_path: cmd.worktree_path.clone(),
        linked_pull_request: None,
        pull_requests: None,
        branch_pull_request: None,
        issues: None,
        active_provider_thread_id: None,
        history_origin: None,
        link_origin: cmd.link_origin.clone(),
        delegated_from: cmd.delegated_from.clone(),
        lineage: ThreadLineage {
            parent_thread_id: None,
            relationship_to_parent: None,
            root_thread_id: cmd.thread_id.clone(),
            side_chat: None,
        },
        forked_from: None,
        created_at: now_iso.to_owned(),
        updated_at: now_iso.to_owned(),
        archived_at: None,
        settled_override: None,
        settled_at: None,
        unsettled_at: None,
        snoozed_until: None,
        snoozed_at: None,
        last_snooze_wake_at: None,
        limit_recovery: None,
        guest_share: None,
        pinned_at: None,
        auto_settle_disabled_at: None,
        pin_order_key: None,
        active_order_key: None,
        tags: None,
        last_visited_at: None,
        opens: PatchValue::default(),
        title_regeneration: if cmd.generate_title == Some(true) {
            Some(Some(serde_json::json!({
                "status": "pending",
                "generateTitle": true,
            })))
        } else {
            None
        },
        last_title_regeneration: None,
        title_renamed: None,
        rollback_request_id: None,
        rollback_failure: None,
        rollback_unrestored: None,
        deleted_at: None,
    }
}

fn append_initial_message(
    cmd: &ThreadCreateCommand,
    thread: &AppThread,
    init: &InitialThreadMessage,
    now_iso: &str,
    events: &mut Vec<DomainEvent>,
    outbox_effects: &mut Vec<PendingOutboxEffect>,
) -> Result<(), DeciderError> {
    let run_id = RunId::new(format!("{}-run", cmd.command_id.as_str()))
        .map_err(|err| DeciderError::InvalidId(err.to_string()))?;
    let run = build_initial_run(&run_id, thread, init, now_iso);
    let msg = build_initial_message(cmd, thread, init, &run_id, now_iso);
    events.push(DomainEvent {
        id: derived_event_id(&cmd.command_id, 1)?,
        thread_id: cmd.thread_id.clone(),
        run_id: Some(run_id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(thread.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::RunUpdated {
            event_type: "run.created".to_owned(),
            run: Box::new(run),
        },
    });
    events.push(DomainEvent {
        id: derived_event_id(&cmd.command_id, 2)?,
        thread_id: cmd.thread_id.clone(),
        run_id: Some(run_id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(thread.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::MessageUpdated(Box::new(msg)),
    });
    outbox_effects.push(PendingOutboxEffect {
        id: derived_effect_id(&cmd.command_id, 0),
        command_id: cmd.command_id.clone(),
        thread_id: cmd.thread_id.clone(),
        request: OutboxEffectRequest::ProviderTurnStart { run_id },
        available_at: None,
    });
    if cmd.generate_title == Some(true) {
        outbox_effects.push(PendingOutboxEffect {
            id: derived_effect_id(&cmd.command_id, 1),
            command_id: cmd.command_id.clone(),
            thread_id: cmd.thread_id.clone(),
            request: OutboxEffectRequest::ThreadTitleGenerate {
                kind: "initial".to_owned(),
                message_id: Some(init.message_id.clone()),
            },
            available_at: None,
        });
    }
    Ok(())
}

fn build_initial_run(
    run_id: &RunId,
    thread: &AppThread,
    init: &InitialThreadMessage,
    now_iso: &str,
) -> Run {
    Run {
        id: run_id.clone(),
        thread_id: thread.id.clone(),
        ordinal: 1,
        provider_instance_id: thread.provider_instance_id.clone(),
        model_selection: thread.model_selection.clone(),
        provider_thread_id: None,
        user_message_id: init.message_id.clone(),
        root_node_id: None,
        active_attempt_id: None,
        status: RunStatus::Running,
        queue_position: None,
        queue_held: None,
        steer_when_ready_run_id: None,
        requested_at: now_iso.to_owned(),
        started_at: Some(now_iso.to_owned()),
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
        workspace_preparation: init.workspace_strategy.clone(),
        fresh_context: None,
        output_schema: init.output_schema.clone(),
        output: None,
    }
}

fn build_initial_message(
    cmd: &ThreadCreateCommand,
    _thread: &AppThread,
    init: &InitialThreadMessage,
    run_id: &RunId,
    now_iso: &str,
) -> ConversationMessage {
    ConversationMessage {
        notification: None,
        created_by: cmd.created_by,
        creation_source: cmd.creation_source,
        scheduled_task_id: init.scheduled_task_id.clone(),
        follows_thread_selection: None,
        sender_thread_id: init.sender_thread_id.clone(),
        linked_sender: None,
        idle_compaction: None,
        workflow_script: None,
        id: init.message_id.clone(),
        thread_id: cmd.thread_id.clone(),
        run_id: Some(run_id.clone()),
        node_id: None,
        role: "user".to_owned(),
        text: init.text.clone(),
        context: init.context.clone(),
        attachments: init.attachments.clone(),
        streaming: false,
        created_at: now_iso.to_owned(),
        updated_at: now_iso.to_owned(),
        delegated_completion: None,
    }
}
