//! Shared test builders and SQLite inspection helpers (zero `.unwrap()`/`.expect()`/`.panic!()`).

use std::error::Error;

use legion_contracts::ids::{
    CheckpointId, CheckpointRef, CheckpointScopeId, CommandId, EventId, MessageId, NodeId,
    ProjectId, ProviderInstanceId, ThreadId,
};
use legion_contracts::orchestration::{
    AppThread, Checkpoint, CheckpointRollbackCommand, CreationSource, DispatchMode, DomainEvent,
    DomainEventPayload, MessageDispatchCommand, ModelSelection, OrchestrationActor,
    OrchestrationCommand, PatchValue, ProviderInteractionMode, Run, RunStatus, ThreadCreateCommand,
    ThreadLineage,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_persistence::sqlite::SqliteStore;
use rusqlite::{Connection, params};

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn new_test_orchestrator() -> Result<Orchestrator, Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;
    Ok(Orchestrator::from_sqlite_store(store))
}

pub fn create_thread_cmd(
    cmd: &str,
    thread: &str,
    project: &str,
    title: &str,
) -> Result<OrchestrationCommand, Box<dyn Error>> {
    Ok(OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new(cmd)?,
        thread_id: ThreadId::new(thread)?,
        project_id: ProjectId::new(project)?,
        title: title.to_owned(),
        generate_title: None,
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
        initial_message: None,
        title_renamed: None,
        workflow_run: None,
    }))
}

pub fn send_message_cmd(
    cmd: &str,
    thread: &str,
    msg: &str,
    text: &str,
    dispatch_mode: DispatchMode,
) -> Result<OrchestrationCommand, Box<dyn Error>> {
    Ok(OrchestrationCommand::MessageDispatch(
        MessageDispatchCommand {
            notification: None,
            guest_message_id: None,
            created_by: OrchestrationActor::User,
            creation_source: CreationSource::Web,
            scheduled_task_id: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            fresh_context: None,
            output_schema: None,
            command_id: CommandId::new(cmd)?,
            thread_id: ThreadId::new(thread)?,
            message_id: MessageId::new(msg)?,
            text: text.to_owned(),
            context: None,
            attachments: Vec::new(),
            title_seed: None,
            model_selection: None,
            source_plan_ref: None,
            restart_continuation_of_run_id: None,
            usage_limit_continuation_of_run_id: None,
            manual_continuation_of_run_id: None,
            steer_follow_up_of_run_id: None,
            steer_follow_up_after_run_ordinal: None,
            usage_limit_recovery_request_id: None,
            delivery_intent: None,
            delegated_completion: None,
            dispatch_mode,
        },
    ))
}

pub fn rollback_cmd(
    cmd: &str,
    thread: &str,
    scope: &str,
    checkpoint: &str,
    restore_files: bool,
) -> Result<OrchestrationCommand, Box<dyn Error>> {
    Ok(OrchestrationCommand::CheckpointRollback(
        CheckpointRollbackCommand {
            restore_files: Some(restore_files),
            command_id: CommandId::new(cmd)?,
            thread_id: ThreadId::new(thread)?,
            scope_id: CheckpointScopeId::new(scope)?,
            checkpoint_id: CheckpointId::new(checkpoint)?,
        },
    ))
}

pub fn make_subagent_thread_event(
    event_id: &str,
    thread_id: &str,
    parent_id: &str,
    root_id: &str,
    project_id: &str,
    occurred_at: &str,
) -> Result<DomainEvent, Box<dyn Error>> {
    let tid = ThreadId::new(thread_id)?;
    let thread = AppThread {
        created_by: OrchestrationActor::Agent,
        creation_source: CreationSource::Provider,
        id: tid.clone(),
        project_id: ProjectId::new(project_id)?,
        title: format!("Subagent {thread_id}"),
        provider_instance_id: ProviderInstanceId::new("codex")?,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
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
            parent_thread_id: Some(ThreadId::new(parent_id)?),
            relationship_to_parent: Some("subagent".to_owned()),
            root_thread_id: ThreadId::new(root_id)?,
            side_chat: None,
            workflow_step: None,
        },
        forked_from: None,
        created_at: occurred_at.to_owned(),
        updated_at: occurred_at.to_owned(),
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
        title_regeneration: None,
        last_title_regeneration: None,
        title_renamed: None,
        rollback_request_id: None,
        rollback_failure: None,
        rollback_unrestored: None,
        workflow_run: None,
        deleted_at: None,
    };
    Ok(DomainEvent {
        id: EventId::new(event_id)?,
        thread_id: tid,
        run_id: None,
        node_id: None,
        driver: None,
        provider_instance_id: Some(ProviderInstanceId::new("codex")?),
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::ThreadState {
            event_type: "thread.created".to_owned(),
            thread: Box::new(thread),
        },
    })
}

pub fn make_checkpoint_captured_event(
    event_id: &str,
    thread_id: &str,
    scope_id: &str,
    cp_id: &str,
    occurred_at: &str,
) -> Result<DomainEvent, Box<dyn Error>> {
    let tid = ThreadId::new(thread_id)?;
    let node_id = NodeId::new(format!("node-{cp_id}"))?;
    let cp = Checkpoint {
        id: CheckpointId::new(cp_id)?,
        thread_id: tid.clone(),
        scope_id: CheckpointScopeId::new(scope_id)?,
        run_id: None,
        node_id: node_id.clone(),
        parent_checkpoint_id: None,
        ordinal_within_scope: 1,
        app_run_ordinal: Some(1),
        r#ref: CheckpointRef::new(format!("refs/legioncode/checkpoints/{cp_id}"))?,
        status: "ready".to_owned(),
        files: Vec::new(),
        files_total: None,
        captured_at: occurred_at.to_owned(),
    };
    Ok(DomainEvent {
        id: EventId::new(event_id)?,
        thread_id: tid,
        run_id: None,
        node_id: Some(node_id),
        driver: None,
        provider_instance_id: Some(ProviderInstanceId::new("codex")?),
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::CheckpointCaptured(cp),
    })
}

pub fn make_run_completed_event(
    event_id: &str,
    mut run: Run,
    occurred_at: &str,
) -> Result<DomainEvent, Box<dyn Error>> {
    run.status = RunStatus::Completed;
    run.completed_at = Some(occurred_at.to_owned());
    Ok(DomainEvent {
        id: EventId::new(event_id)?,
        thread_id: run.thread_id.clone(),
        run_id: Some(run.id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(run.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::RunUpdated {
            event_type: "run.updated".to_owned(),
            run: Box::new(run),
        },
    })
}

pub fn count_table_rows(conn: &Connection, table: &str) -> Result<i64, Box<dyn Error>> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |row| row.get(0))?)
}

pub fn count_thread_events(conn: &Connection, thread_id: &str) -> Result<i64, Box<dyn Error>> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM orchestration_events WHERE stream_id = ?1",
        params![thread_id],
        |row| row.get(0),
    )?)
}
