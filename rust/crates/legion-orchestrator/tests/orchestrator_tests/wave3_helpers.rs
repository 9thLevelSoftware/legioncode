use std::error::Error;

use legion_contracts::ids::{MessageId, ProjectId, ProviderInstanceId, RunId, ThreadId};
use legion_contracts::orchestration::{
    AppThread, ConversationMessage, CreationSource, DispatchMode, MessageDispatchCommand,
    ModelSelection, OrchestrationActor, OrchestrationCommand, PatchValue, ProviderInteractionMode,
    Run, RunStatus, ThreadLineage,
};
use legion_contracts::policy::RuntimeMode;

use super::helpers::send_message_cmd;

pub(super) fn basic_model() -> Result<ModelSelection, Box<dyn Error>> {
    Ok(ModelSelection {
        instance_id: ProviderInstanceId::new("codex")?,
        model: "gpt-5".to_owned(),
        options: None,
    })
}

pub(super) fn make_thread(thread_id: &str, project_id: &str) -> Result<AppThread, Box<dyn Error>> {
    let tid = ThreadId::new(thread_id)?;
    Ok(AppThread {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        id: tid.clone(),
        project_id: ProjectId::new(project_id)?,
        title: "Thread".to_owned(),
        provider_instance_id: ProviderInstanceId::new("codex")?,
        model_selection: basic_model()?,
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        linked_pull_request: None,
        pull_requests: None,
        branch_pull_request: None,
        issues: None,
        active_provider_thread_id: None,
        history_origin: None,
        link_origin: None,
        delegated_from: None,
        lineage: ThreadLineage {
            parent_thread_id: None,
            relationship_to_parent: None,
            root_thread_id: tid,
            side_chat: None,
            workflow_step: None,
        },
        forked_from: None,
        created_at: "2026-11-22T00:00:00.000Z".to_owned(),
        updated_at: "2026-11-22T00:00:00.000Z".to_owned(),
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
        opens: PatchValue::Absent,
        title_regeneration: None,
        last_title_regeneration: None,
        title_renamed: None,
        rollback_request_id: None,
        rollback_failure: None,
        rollback_unrestored: None,
        workflow_run: None,
        deleted_at: None,
    })
}

pub(super) fn make_run(
    run_id: &str,
    thread_id: &str,
    ordinal: u64,
    status: RunStatus,
) -> Result<Run, Box<dyn Error>> {
    Ok(Run {
        id: RunId::new(run_id)?,
        thread_id: ThreadId::new(thread_id)?,
        ordinal,
        provider_instance_id: ProviderInstanceId::new("codex")?,
        model_selection: basic_model()?,
        provider_thread_id: None,
        user_message_id: MessageId::new("m-1")?,
        root_node_id: None,
        active_attempt_id: None,
        status,
        queue_position: None,
        queue_held: None,
        steer_when_ready_run_id: None,
        requested_at: "2026-11-22T00:00:00.000Z".to_owned(),
        started_at: None,
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

pub(super) fn make_user_message(
    msg_id: &str,
    thread_id: &str,
    text: &str,
    run_id: Option<&str>,
) -> Result<ConversationMessage, Box<dyn Error>> {
    Ok(ConversationMessage {
        notification: None,
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        scheduled_task_id: None,
        follows_thread_selection: None,
        sender_thread_id: None,
        linked_sender: None,
        idle_compaction: None,
        workflow_script: None,
        id: MessageId::new(msg_id)?,
        thread_id: ThreadId::new(thread_id)?,
        run_id: run_id.map(RunId::new).transpose()?,
        node_id: None,
        role: "user".to_owned(),
        text: text.to_owned(),
        context: None,
        attachments: Vec::new(),
        streaming: false,
        created_at: "2026-11-22T00:00:00.000Z".to_owned(),
        updated_at: "2026-11-22T00:00:00.000Z".to_owned(),
        delegated_completion: None,
    })
}

pub(super) fn make_dispatch_cmd(
    cmd_id: &str,
    thread_id: &str,
    msg_id: &str,
    text: &str,
) -> Result<MessageDispatchCommand, Box<dyn Error>> {
    let OrchestrationCommand::MessageDispatch(msg) = send_message_cmd(
        cmd_id,
        thread_id,
        msg_id,
        text,
        DispatchMode::QueueAfterActive,
    )?
    else {
        return Err("expected MessageDispatch".into());
    };
    Ok(msg)
}
